//! Which launches are in progress, so that `bastion-mc scram` can stop them.
//!
//! A supervising launcher leaves a marker `running/<pid>` in the cache directory holding
//! the kernel's start time of that process and the instance name. Pids are reused; the
//! pair (pid, start time) is not, so a marker can only ever point at the process that
//! wrote it. A marker whose process is gone, or is some other process by now, is stale
//! and is removed by whoever looks next.
//!
//! The cache directory is the place because it stays writable after the launcher has
//! confined itself, so the marker can be removed on the way out.

use std::path::{Path, PathBuf};

use crate::{
    error::Result,
    paths::{Paths, validate_id},
    util,
};

fn dir(paths: &Paths) -> PathBuf {
    paths.cache.join("running")
}

/// Start time of `pid` in clock ticks since boot (field 22 of `/proc/<pid>/stat`), and its
/// command name. The command name may contain spaces and parentheses; the fields are
/// counted from the *last* closing parenthesis.
fn identity(pid: u32) -> Option<(u64, String)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (open, close) = (stat.find('(')?, stat.rfind(')')?);
    let comm = stat.get(open + 1..close)?.to_string();
    let start = stat.get(close + 1..)?.split_ascii_whitespace().nth(19)?.parse().ok()?;
    Some((start, comm))
}

/// Removes the marker when the launch ends, however it ends.
pub struct Marker(PathBuf);

impl Marker {
    pub fn create(paths: &Paths, instance: &str) -> Result<Self> {
        validate_id(instance)?;
        let dir = dir(paths);
        util::create_private_dir_all(&dir)?;
        prune(&dir);
        let pid = std::process::id();
        let (start, _) = identity(pid).unwrap_or((0, String::new()));
        let path = dir.join(pid.to_string());
        util::write_atomic(&path, format!("{start}\n{instance}\n").as_bytes(), 0o600)?;
        Ok(Marker(path))
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub pid: u32,
    pub instance: String,
}

/// The marker at `path`, if the process it names is still that process.
fn live(path: &Path) -> Option<Launch> {
    let pid: u32 = path.file_name()?.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()?;
    let text = String::from_utf8(util::read_small_file(path, 256).ok()?).ok()?;
    let mut lines = text.lines();
    let recorded: u64 = lines.next()?.parse().ok()?;
    let instance = lines.next()?.to_string();
    validate_id(&instance).ok()?;
    let (start, comm) = identity(pid)?;
    // The same process (start time), running the same program as this one (command name).
    let (_, own) = identity(std::process::id())?;
    (start == recorded && recorded != 0 && comm == own && pid != std::process::id()).then_some(Launch { pid, instance })
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let own = path.file_name().and_then(|n| n.to_str()) == Some(&std::process::id().to_string());
        if !own && live(&path).is_none() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Launches in progress (other than this process). Stale markers are removed.
pub fn list(paths: &Paths) -> Vec<Launch> {
    let dir = dir(paths);
    prune(&dir);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut found: Vec<Launch> = entries.flatten().filter_map(|e| live(&e.path())).collect();
    found.sort_by_key(|l| l.pid);
    found
}

/// Kill every launch in progress, immediately (SIGKILL). The game and its display server
/// die with their launcher: they were started with a parent-death signal, and the launcher
/// is the only holder of the session token. Returns what was killed.
pub fn kill_all(paths: &Paths) -> Vec<Launch> {
    let launches = list(paths);
    for launch in &launches {
        // Re-checked immediately before the signal: the pid must still be that process.
        if live(&dir(paths).join(launch.pid.to_string())).is_some() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(launch.pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
    launches
}

/// Remove what launches that did not end properly left in the cache directory: per-launch
/// game roots (`scratch/*`), unpacked natives (`natives-*`) and display directories
/// (`display-*`). A launcher that is killed — by `scram`, by the kernel, by a power cut —
/// cannot clean up after itself.
///
/// Only when no other launch is in progress, and only entries untouched for `min_age`:
/// a launch that is starting at this very moment has a marker or, at worst, a directory it
/// created a moment ago.
pub fn sweep_leftovers(paths: &Paths, min_age: std::time::Duration) -> usize {
    if !list(paths).is_empty() {
        return 0;
    }
    let old_enough = |path: &Path| {
        std::fs::symlink_metadata(path)
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= min_age)
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(paths.scratch()) {
        candidates.extend(entries.flatten().map(|e| e.path()));
    }
    if let Ok(entries) = std::fs::read_dir(&paths.cache) {
        candidates.extend(entries.flatten().map(|e| e.path()).filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("natives-") || n.starts_with("display-"))
        }));
    }
    let mut removed = 0;
    for path in candidates {
        // remove_dir_all does not follow symbolic links; the trees were written by a sandbox.
        if old_enough(&path) && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (PathBuf, Paths) {
        let root =
            std::env::temp_dir().join(format!("bastion-run-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        };
        paths.ensure().unwrap();
        (root, paths)
    }

    /// The child half of the test below: this test binary again, idle.
    #[test]
    fn idle_child() {
        if std::env::var_os("BASTION_RUNNING_CHILD").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    #[test]
    fn leftovers_of_dead_launches_are_swept_but_not_under_a_running_one() {
        let (root, paths) = paths();
        let zero = std::time::Duration::ZERO;
        std::fs::create_dir_all(paths.scratch().join("vanilla-0011223344556677/saves")).unwrap();
        std::fs::write(paths.scratch().join("vanilla-0011223344556677/saves/x"), b"x").unwrap();
        std::fs::create_dir_all(paths.cache.join("natives-aabbccdd")).unwrap();
        std::fs::create_dir_all(paths.cache.join("display-aabbccdd")).unwrap();
        std::fs::create_dir_all(paths.cache.join("something-else")).unwrap();
        std::fs::write(paths.cache.join("natives-file"), b"not a directory").unwrap();
        // Too young: left alone (it could belong to a launch that is just starting).
        assert_eq!(sweep_leftovers(&paths, std::time::Duration::from_secs(3600)), 0);

        // Another launch is in progress: nothing is touched.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "running::tests::idle_child", "--test-threads=1"])
            .env("BASTION_RUNNING_CHILD", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let (_, own_comm) = identity(std::process::id()).unwrap();
        for _ in 0..200 {
            if identity(child.id()).is_some_and(|(_, comm)| comm == own_comm) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::fs::create_dir_all(dir(&paths)).unwrap();
        let marker = dir(&paths).join(child.id().to_string());
        std::fs::write(&marker, format!("{}\nvanilla\n", identity(child.id()).unwrap().0)).unwrap();
        assert_eq!(sweep_leftovers(&paths, zero), 0);
        assert!(paths.scratch().join("vanilla-0011223344556677/saves/x").exists());
        child.kill().unwrap();
        let _ = child.wait();

        assert_eq!(sweep_leftovers(&paths, zero), 3);
        assert!(!paths.scratch().join("vanilla-0011223344556677").exists());
        assert!(paths.cache.join("something-else").exists() && paths.cache.join("natives-file").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_the_process_that_wrote_a_marker_is_ever_found_or_killed() {
        let (root, paths) = paths();
        assert!(list(&paths).is_empty());
        // Our own marker is not a launch "in progress elsewhere".
        let own = Marker::create(&paths, "vanilla").unwrap();
        assert!(list(&paths).is_empty());

        // Another process of the same program, with a correct marker.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "running::tests::idle_child", "--test-threads=1"])
            .env("BASTION_RUNNING_CHILD", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // The command name is set by exec; wait for it.
        let (_, own_comm) = identity(std::process::id()).unwrap();
        for _ in 0..200 {
            if identity(child.id()).is_some_and(|(_, comm)| comm == own_comm) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (start, _) = identity(child.id()).unwrap();
        let marker = dir(&paths).join(child.id().to_string());
        std::fs::write(&marker, format!("{start}\nmodded\n")).unwrap();

        // Markers that must not lead to a signal: wrong start time (a reused pid), a
        // process of another program (pid 1), garbage, a name that is not a pid.
        std::fs::write(dir(&paths).join("1"), "1\nvanilla\n").unwrap();
        let other = std::process::Command::new("/usr/bin/sleep").arg("30").spawn().unwrap();
        let (other_start, _) = identity(other.id()).unwrap();
        std::fs::write(dir(&paths).join(other.id().to_string()), format!("{other_start}\nvanilla\n")).unwrap();
        std::fs::write(dir(&paths).join("4194303"), format!("{}\nvanilla\n", start + 1)).unwrap();
        std::fs::write(dir(&paths).join("notes"), "x").unwrap();

        assert_eq!(list(&paths), [Launch { pid: child.id(), instance: "modded".into() }]);
        assert!(!dir(&paths).join("1").exists() && !dir(&paths).join("notes").exists(), "stale markers are removed");

        // A marker with the child's pid but another start time is not the child.
        std::fs::write(&marker, format!("{}\nmodded\n", start + 1)).unwrap();
        assert!(kill_all(&paths).is_empty());
        assert!(child.try_wait().unwrap().is_none(), "the child must still be running");

        std::fs::write(&marker, format!("{start}\nmodded\n")).unwrap();
        assert_eq!(kill_all(&paths).len(), 1);
        let status = child.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGKILL));
        let mut other = other;
        assert!(other.try_wait().unwrap().is_none(), "a process of another program is never touched");
        other.kill().unwrap();
        let _ = other.wait();

        assert!(list(&paths).is_empty(), "nothing is left running");
        drop(own);
        assert!(!dir(&paths).join(std::process::id().to_string()).exists(), "a marker is removed with its launch");
        std::fs::remove_dir_all(root).unwrap();
    }
}
