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
    /// Unit name without the `.scope` suffix: `[A-Za-z0-9:_.\-]` only.
    pub unit: String,
}

impl Limits {
    pub fn new(instance: &str, memory_max_mb: u64, tasks_max: u32) -> crate::Result<Self> {
        let unique = util::hex_encode(&util::random_bytes::<4>()?);
        Ok(Limits {
            memory_max_bytes: memory_max_mb.saturating_mul(1 << 20),
            tasks_max,
            unit: format!("bastion-mc-{instance}-{unique}"),
        })
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
        for property in [
            format!("MemoryMax={}", self.memory_max_bytes),
            "MemorySwapMax=0".to_string(),
            format!("TasksMax={}", self.tasks_max),
        ] {
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

/// Prove that a scope with limits can actually be created, by creating one around
/// `/usr/bin/true`. Cheap (tens of milliseconds), and it turns every way this can fail —
/// no user manager, controllers not delegated, policy — into one clear answer *before* the
/// game is started.
pub async fn preflight() -> std::result::Result<PathBuf, String> {
    let program = locate()?;
    // A fresh name every time: unit names are unique per user manager, and a probe must not
    // fail because another one (another launcher, an earlier attempt) used the same name.
    let probe = Limits::new("probe", 64, 16).map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(&program);
    command
        .args(probe.command(OsStr::new("/usr/bin/true"), &[]))
        .env_clear()
        .envs(manager_env())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .map_err(|_| "systemd-run did not answer within 15 seconds".to_string())?
        .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
    if output.status.success() {
        Ok(program)
    } else {
        Err(format!(
            "systemd-run could not create a scope with resource limits: {}",
            util::sanitize_line(output.stderr.trim_ascii(), 300)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_command_carries_the_limits_and_then_the_program_verbatim() {
        let limits = Limits { memory_max_bytes: 6 << 30, tasks_max: 2048, unit: "bastion-mc-vanilla-0a1b2c3d".into() };
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
    fn unit_names_are_unique_and_plain() {
        let a = Limits::new("vanilla-26.3", 4096, 2048).unwrap();
        let b = Limits::new("vanilla-26.3", 4096, 2048).unwrap();
        assert_ne!(a.unit, b.unit);
        assert!(a.unit.starts_with("bastion-mc-vanilla-26.3-"));
        assert!(a.unit.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_')));
        assert_eq!(a.memory_max_bytes, 4096 << 20);
    }
}
