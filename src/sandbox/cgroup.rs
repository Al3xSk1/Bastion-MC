//! Resource limits for the sandbox: a transient systemd scope.
//!
//! Namespaces, Landlock and seccomp decide what the game may *touch*; none of them limits
//! how much it may *use*. A hostile mod can allocate until the desktop starts swapping or
//! fork until the user's process table is full. The kernel mechanism for that is a control
//! group, and on a systemd machine an unprivileged user gets one by asking their user
//! manager for a scope.
//!
//! The launcher therefore starts the sandbox as
//! `systemd-run --user --scope -p MemoryMax=… -p MemorySwapMax=0 -p TasksMax=… -- bwrap …`.
//! `systemd-run --scope` registers the scope and then *becomes* the command, so the process
//! tree, the inherited descriptors and the parent-death signal are exactly what they are
//! without it; only the control group differs.
//!
//! * `MemoryMax` — the game is killed by the kernel inside its own group when it exceeds
//!   this, instead of pushing the rest of the session into the out-of-memory killer.
//! * `MemorySwapMax=0` — nothing of the game is ever written to swap. Besides keeping the
//!   desktop responsive, this is what keeps the JVM heap (and the session token in it) off
//!   the swap device.
//! * `TasksMax` — a fork bomb ends at this many threads and processes.
//! * `CPUQuota` (optional) — the most CPU time the group may use, in percent of one CPU
//!   (`400%` = four CPUs). Off by default for the game, which legitimately uses many cores
//!   for chunk generation; set `sandbox.cpu_quota_percent` to bound a miner or a busy loop.
//!
//! **Asked for is not enforced.** `systemd-run` exiting with status 0 means that a scope was
//! created — not that the kernel holds the limits: a user manager that was not delegated a
//! controller accepts the property and enforces nothing. So the limits are **read back from
//! the kernel** ([`verify`]): the process must be in the scope that was asked for, and that
//! control group's `memory.max`, `memory.swap.max`, `pids.max` (and `cpu.max`) must hold the
//! values. [`preflight`] does that with a probe scope before anything is started, and the
//! launch does it again for the real sandbox once it runs. What this establishes is what
//! the kernel's own files say; that the kernel acts on them is its business (and is
//! exercised by `tests/sandbox.rs`: a thread bomb stops, a memory hog is killed).

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use crate::util;

pub const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";

/// Limits for one sandbox, and the name of the scope that carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub memory_max_bytes: u64,
    pub tasks_max: u32,
    /// `CPUQuota=` in percent of one CPU; `None` leaves CPU time unbounded.
    pub cpu_quota_percent: Option<u32>,
    /// Unit name without the `.scope` suffix: `[A-Za-z0-9:_.\-]` only.
    pub unit: String,
}

impl Limits {
    pub fn new(instance: &str, memory_max_mb: u64, tasks_max: u32) -> crate::Result<Self> {
        let unique = util::hex_encode(&util::random_bytes::<4>()?);
        Ok(Limits {
            memory_max_bytes: memory_max_mb.saturating_mul(1 << 20),
            tasks_max,
            cpu_quota_percent: None,
            unit: format!("bastion-mc-{instance}-{unique}"),
        })
    }

    pub fn with_cpu_quota(mut self, percent: Option<u32>) -> Self {
        self.cpu_quota_percent = percent;
        self
    }

    /// `systemd-run` arguments that put `program args…` into the scope.
    pub fn command(&self, program: &OsStr, args: &[OsString]) -> Vec<OsString> {
        let mut argv: Vec<OsString> = [
            "--user",
            "--scope",
            "--quiet",   // nothing but the command's own output on stderr
            "--collect", // do not keep a failed unit around
            "--same-dir",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        argv.push(format!("--unit={}", self.unit).into());
        argv.push("--description=Minecraft in the bastion-mc sandbox".into());
        let mut properties = vec![
            format!("MemoryMax={}", self.memory_max_bytes),
            "MemorySwapMax=0".to_string(),
            format!("TasksMax={}", self.tasks_max),
        ];
        if let Some(percent) = self.cpu_quota_percent {
            properties.push(format!("CPUQuota={percent}%"));
        }
        for property in properties {
            argv.push("-p".into());
            argv.push(property.into());
        }
        argv.push("--".into());
        argv.push(program.to_os_string());
        argv.extend(args.iter().cloned());
        argv
    }
}

/// The environment `systemd-run --user` needs to find the user manager, and nothing more.
pub fn manager_env() -> Vec<(OsString, OsString)> {
    ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
        .collect()
}

/// The root-owned `systemd-run`, if a user manager is reachable.
pub fn locate() -> std::result::Result<PathBuf, String> {
    let program = util::require_root_owned(Path::new(SYSTEMD_RUN)).map_err(|e| e.to_string())?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    if !Path::new(&runtime).join("systemd/private").exists() {
        return Err("no systemd user manager is running for this session".into());
    }
    Ok(program)
}

/// Why a process's control group is not what [`Limits`] asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotVerified {
    /// The process is not (yet) in the scope.
    NotInScope(String),
    /// It is in the scope, but the kernel does not hold a limit that was asked for.
    NotEnforced(String),
}

impl std::fmt::Display for NotVerified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotVerified::NotInScope(why) | NotVerified::NotEnforced(why) => f.write_str(why),
        }
    }
}

const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// [`verify`] against a given hierarchy root and `/proc/<pid>/cgroup` text.
fn verify_in(root: &Path, membership: &str, limits: &Limits) -> std::result::Result<(), NotVerified> {
    let not_in = |why: String| Err(NotVerified::NotInScope(why));
    let Some(group) = membership.lines().find_map(|line| line.strip_prefix("0::")) else {
        return not_in("this system does not use the unified control-group hierarchy (cgroup v2)".into());
    };
    // The kernel wrote this path; it is still only used if it is a plain absolute one.
    let relative = Path::new(group.strip_prefix('/').unwrap_or(""));
    if group.len() > 4096 || crate::paths::validate_relative(relative).is_err() {
        return not_in("the kernel reported an unusable control-group path".into());
    }
    let scope = format!("{}.scope", limits.unit);
    if relative.file_name().and_then(|name| name.to_str()) != Some(scope.as_str()) {
        return not_in(format!(
            "the process is not in the scope {scope} (it is in {})",
            util::sanitize_line(group.as_bytes(), 200)
        ));
    }
    let dir = root.join(relative);
    let mut wanted = vec![
        ("memory.max", limits.memory_max_bytes.to_string(), "memory"),
        ("memory.swap.max", "0".to_string(), "memory"),
        ("pids.max", limits.tasks_max.to_string(), "pids"),
    ];
    if let Some(percent) = limits.cpu_quota_percent {
        // CPUQuota=N% is N ms of CPU time per 100 ms period.
        wanted.push(("cpu.max", format!("{} 100000", u64::from(percent) * 1000), "cpu"));
    }
    for (knob, expected, controller) in wanted {
        let found = match util::read_small_file(&dir.join(knob), 256) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).trim().to_string(),
            Err(_) => {
                return Err(NotVerified::NotEnforced(format!(
                    "the scope has no {knob}: the {controller} controller is not available to this user's systemd \
                     manager (not delegated), so that limit is not enforced"
                )));
            }
        };
        if found != expected {
            return Err(NotVerified::NotEnforced(format!(
                "the scope's {knob} is {:?}, not {expected:?}: the limit that was asked for is not the one in force",
                util::sanitize_line(found.as_bytes(), 60)
            )));
        }
    }
    Ok(())
}

/// Read the limits back from the kernel: is `pid` in the scope `limits` names, and does that
/// control group hold exactly these limits?
pub fn verify(pid: u32, limits: &Limits) -> std::result::Result<(), NotVerified> {
    let membership = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .map_err(|e| NotVerified::NotInScope(format!("the control group of process {pid} cannot be read ({e})")))?;
    verify_in(Path::new(CGROUP_ROOT), &membership, limits)
}

/// [`verify`] for a process that has only just been started: being moved into the scope and
/// the limits being written take the user manager a moment. Waits up to `patience`.
pub async fn verify_started(pid: u32, limits: &Limits, patience: Duration) -> std::result::Result<(), NotVerified> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        match verify(pid, limits) {
            Ok(()) => return Ok(()),
            Err(why) if tokio::time::Instant::now() >= deadline => return Err(why),
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

/// What the probe scope runs: something that stays long enough to be looked at.
const PROBE_PROGRAM: &str = "/usr/bin/sleep";

/// Prove that a scope **with enforced limits** can be had, by creating one around a
/// sleeping process and reading its limits back from the kernel ([`verify`]). `cpu_quota`:
/// whether a CPU quota will be asked for (it needs one more controller). Takes a fraction
/// of a second, and turns every way this can fail — no user manager, a controller that was
/// not delegated, policy — into one answer *before* the game is started.
pub async fn preflight(cpu_quota: bool) -> std::result::Result<PathBuf, String> {
    let program = locate()?;
    util::require_root_owned(Path::new(PROBE_PROGRAM)).map_err(|e| e.to_string())?;
    // A fresh name every time: unit names are unique per user manager, and a probe must not
    // fail because another one (another launcher, an earlier attempt) used the same name.
    let probe = Limits::new("probe", 64, 16).map_err(|e| e.to_string())?.with_cpu_quota(cpu_quota.then_some(50));
    let mut child = tokio::process::Command::new(&program)
        .args(probe.command(OsStr::new(PROBE_PROGRAM), &["30".into()]))
        .env_clear()
        .envs(manager_env())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
    let pid = child.id().ok_or("the probe ended before it could be examined")?;
    let verified = tokio::select! {
        verified = verify_started(pid, &probe, Duration::from_secs(15)) => verified.map_err(|why| why.to_string()),
        // systemd-run gave up (it only ends this early when it could not create the scope).
        _ = child.wait() => {
            use tokio::io::AsyncReadExt;
            let mut stderr = Vec::new();
            if let Some(pipe) = child.stderr.as_mut() {
                let _ = pipe.take(4096).read_to_end(&mut stderr).await;
            }
            Err(format!(
                "systemd-run could not create a scope with resource limits: {}",
                util::sanitize_line(stderr.trim_ascii(), 300)
            ))
        }
    };
    // The probe has served its purpose; the scope goes with it.
    let _ = child.start_kill();
    let _ = child.wait().await;
    verified.map(|()| program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_command_carries_the_limits_and_then_the_program_verbatim() {
        let limits = Limits {
            memory_max_bytes: 6 << 30,
            tasks_max: 2048,
            cpu_quota_percent: None,
            unit: "bastion-mc-vanilla-0a1b2c3d".into(),
        };
        let argv = limits
            .command(OsStr::new("/usr/bin/bwrap"), &["--args".into(), "7".into(), "--".into(), "/bastion/init".into()]);
        let text: Vec<&str> = argv.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            text,
            [
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--same-dir",
                "--unit=bastion-mc-vanilla-0a1b2c3d",
                "--description=Minecraft in the bastion-mc sandbox",
                "-p",
                "MemoryMax=6442450944",
                "-p",
                "MemorySwapMax=0",
                "-p",
                "TasksMax=2048",
                "--",
                "/usr/bin/bwrap",
                "--args",
                "7",
                "--",
                "/bastion/init",
            ]
        );
    }

    #[test]
    fn a_cpu_quota_is_one_more_property() {
        let limits = Limits::new("x", 1024, 64).unwrap().with_cpu_quota(Some(250));
        let argv = limits.command(OsStr::new("/usr/bin/true"), &[]);
        let text: Vec<&str> = argv.iter().map(|a| a.to_str().unwrap()).collect();
        let at = text.iter().position(|a| *a == "CPUQuota=250%").expect("CPUQuota property");
        assert_eq!(text[at - 1], "-p");
        assert!(at < text.iter().position(|a| *a == "--").unwrap());
        let none = Limits::new("x", 1024, 64).unwrap().command(OsStr::new("/usr/bin/true"), &[]);
        assert!(!none.iter().any(|a| a.to_str().unwrap().starts_with("CPUQuota")));
    }

    /// A control-group hierarchy as the kernel would show it, in a directory of files.
    fn hierarchy(knobs: &[(&str, &str)]) -> (PathBuf, String, Limits) {
        let root =
            std::env::temp_dir().join(format!("bastion-cg-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let limits = Limits {
            memory_max_bytes: 6 << 30,
            tasks_max: 4096,
            cpu_quota_percent: None,
            unit: "bastion-mc-vanilla-0a1b2c3d".into(),
        };
        let group = "/user.slice/user-1000.slice/user@1000.service/app.slice/bastion-mc-vanilla-0a1b2c3d.scope";
        std::fs::create_dir_all(root.join(&group[1..])).unwrap();
        for (knob, value) in knobs {
            std::fs::write(root.join(&group[1..]).join(knob), format!("{value}\n")).unwrap();
        }
        (root, format!("0::{group}\n"), limits)
    }

    #[test]
    fn limits_count_only_when_the_kernel_holds_them() {
        let all =
            [("memory.max", "6442450944"), ("memory.swap.max", "0"), ("pids.max", "4096"), ("cpu.max", "max 100000")];
        let (root, membership, limits) = hierarchy(&all);
        let scope_dir = root.join(&membership.trim()[4..]);
        assert_eq!(verify_in(&root, &membership, &limits), Ok(()));

        // Asked for a CPU quota: the kernel has to hold that too.
        let with_cpu = limits.clone().with_cpu_quota(Some(250));
        let err = verify_in(&root, &membership, &with_cpu).unwrap_err();
        assert!(matches!(&err, NotVerified::NotEnforced(why) if why.contains("cpu.max")), "{err:?}");
        std::fs::write(scope_dir.join("cpu.max"), "250000 100000\n").unwrap();
        assert_eq!(verify_in(&root, &membership, &with_cpu), Ok(()));

        // A limit the manager accepted but did not apply.
        std::fs::write(scope_dir.join("memory.max"), "max\n").unwrap();
        let err = verify_in(&root, &membership, &limits).unwrap_err();
        assert!(
            matches!(&err, NotVerified::NotEnforced(why) if why.contains("memory.max") && why.contains("\"max\"")),
            "{err:?}"
        );
        std::fs::remove_dir_all(root).unwrap();

        // A controller that was not delegated: the file does not exist at all.
        let (root, membership, limits) = hierarchy(&[("pids.max", "4096")]);
        let err = verify_in(&root, &membership, &limits).unwrap_err();
        assert!(matches!(&err, NotVerified::NotEnforced(why) if why.contains("not delegated")), "{err:?}");

        // Not in the scope: some other group, the old hierarchy, a path that is not one.
        for elsewhere in [
            "0::/user.slice/user-1000.slice/session-2.scope\n",
            "0::/user.slice/bastion-mc-vanilla-0a1b2c3d.scope/child\n",
            "12:memory:/user.slice\n1:name=systemd:/x\n",
            "0::/../../etc/bastion-mc-vanilla-0a1b2c3d.scope\n",
            "0::\n",
            "",
        ] {
            let err = verify_in(&root, elsewhere, &limits).unwrap_err();
            assert!(matches!(err, NotVerified::NotInScope(_)), "{elsewhere:?}: {err:?}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unit_names_are_unique_and_plain() {
        let a = Limits::new("vanilla-26.3", 4096, 2048).unwrap();
        let b = Limits::new("vanilla-26.3", 4096, 2048).unwrap();
        assert_ne!(a.unit, b.unit);
        assert!(a.unit.starts_with("bastion-mc-vanilla-26.3-"));
        assert!(a.unit.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_')));
        assert_eq!(a.memory_max_bytes, 4096 << 20);
    }
}
