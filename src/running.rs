//! Which launcher commands are in progress, so that `bastion-mc scram` can stop them.
//!
//! Every command that can end up *storing* a credential — a launch (it rotates the refresh
//! token), a sign-in, the skin, cape and canary commands (they open a session), a vault
//! re-seal — leaves a marker `running/<pid>` in the cache directory for as long as it runs.
//! Without that, a sign-in that was waiting for the user when scram ran would complete a
//! minute later and put a fresh token back on the machine.
//!
//! A marker holds the kernel's start time of the process that wrote it, what it is doing,
//! and its command name. Pids are reused; the pair (pid, start time) is not, so a marker can
//! only ever point at the process that wrote it. A marker whose process is gone, or is some
//! other process by now, is stale and is removed by whoever looks next. The command name is
//! compared with what the marker itself recorded, not with the name of the process that is
//! looking: a launcher started through a symbolic link or under another file name is still
//! a launcher.
//!
//! Stopping is done through a pidfd: the descriptor is obtained first, the identity is
//! checked after that, and the signal goes to the descriptor — never to a number that might
//! have changed hands in between. "Killed" is only reported for a process whose end the
//! kernel confirmed.
//!
//! Directories a launch creates in the cache (game root, unpacked natives, the nested
//! compositor's directory) are locked by it with `flock`. A lock ends with its process,
//! however that process ends, and cannot be forged by a file: [`sweep_leftovers`] removes
//! only directories nobody holds, even if a marker was lost.
//!
//! The cache directory is the place for all of this because it stays writable after the
//! launcher has confined itself, so the marker can be removed on the way out. Markers are
//! files of the user: another program running as the user can delete or forge them, and
//! scram then misses a launch or kills a process of the user's that the marker names. That
//! program could do both directly. The same goes for a launcher that was compromised by
//! what its game sent: the cache is the one place it can still write. What it cannot do is
//! make its directories look abandoned — a lock is held or it is not — so a launch whose
//! marker is gone still shows up as a locked game directory ([`Sweep::in_use`]), and `scram`
//! reports exactly that instead of "nothing was running".

use std::{
    fs::File,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use nix::fcntl::{Flock, FlockArg};

use crate::{
    error::Result,
    paths::{Paths, validate_id},
    tree, util,
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

/// What a marked process is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    /// Supervising (or preparing) a launch of this instance.
    Launch(String),
    /// Another command that may store a credential: `login`, `skin`, `vault`, …
    Command(String),
}

impl Activity {
    fn encode(&self) -> String {
        match self {
            Activity::Launch(instance) => format!("launch:{instance}"),
            Activity::Command(name) => format!("command:{name}"),
        }
    }

    fn decode(text: &str) -> Option<Self> {
        let valid = |id: &str| validate_id(id).is_ok();
        match text.split_once(':') {
            Some(("launch", instance)) if valid(instance) => Some(Activity::Launch(instance.to_string())),
            Some(("command", name)) if valid(name) => Some(Activity::Command(name.to_string())),
            // Markers written before commands were marked: the instance name alone.
            None if valid(text) => Some(Activity::Launch(text.to_string())),
            _ => None,
        }
    }
}

/// Removes the marker when the command ends, however it ends.
pub struct Marker(PathBuf);

impl Marker {
    /// Mark a launch of `instance`.
    pub fn create(paths: &Paths, instance: &str) -> Result<Self> {
        validate_id(instance)?;
        Self::write(paths, &Activity::Launch(instance.to_string()))
    }

    /// Mark a command other than a launch that may store a credential.
    pub fn for_command(paths: &Paths, name: &str) -> Result<Self> {
        validate_id(name)?;
        Self::write(paths, &Activity::Command(name.to_string()))
    }

    fn write(paths: &Paths, activity: &Activity) -> Result<Self> {
        let dir = dir(paths);
        util::create_private_dir_all(&dir)?;
        prune(&dir);
        let pid = std::process::id();
        let (start, comm) = identity(pid).unwrap_or((0, String::new()));
        let path = dir.join(pid.to_string());
        // The command name goes last: it is the one field that may contain anything.
        util::write_atomic(&path, format!("{start}\n{}\n{comm}\n", activity.encode()).as_bytes(), 0o600)?;
        Ok(Marker(path))
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A command in progress in another process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub pid: u32,
    pub activity: Activity,
    /// Start time recorded in the marker (and found in the kernel).
    start: u64,
}

/// The marker at `path`, if the process it names is still that process.
fn live(path: &Path) -> Option<Running> {
    let pid: u32 = path.file_name()?.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()?;
    let text = String::from_utf8(util::read_small_file(path, 512).ok()?).ok()?;
    let mut lines = text.lines();
    let recorded: u64 = lines.next()?.parse().ok()?;
    let activity = Activity::decode(lines.next()?)?;
    let recorded_comm = lines.next();
    let (start, comm) = identity(pid)?;
    // The same process (start time), still under the name it had when it wrote the marker.
    // Older markers carry no name; the start time alone identifies the process.
    let same = start == recorded && recorded != 0 && recorded_comm.is_none_or(|name| name == comm);
    (same && pid != std::process::id()).then_some(Running { pid, activity, start })
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

/// Commands in progress (other than this process). Stale markers are removed.
pub fn list(paths: &Paths) -> Vec<Running> {
    let dir = dir(paths);
    prune(&dir);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut found: Vec<Running> = entries.flatten().filter_map(|e| live(&e.path())).collect();
    found.sort_by_key(|l| l.pid);
    found
}

/// Why the markers cannot all be read, if they cannot — then "nothing is running" is not
/// known, whatever [`list`] returned. A directory that does not exist is no such reason:
/// nothing has ever run.
pub fn unreadable(paths: &Paths) -> Option<String> {
    let dir = dir(paths);
    match std::fs::read_dir(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(format!("{}: {e}", dir.display())),
        Ok(mut entries) => entries.find_map(|entry| entry.err()).map(|e| format!("{}: {e}", dir.display())),
    }
}

/// What became of a process that was to be stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// SIGKILL was delivered to exactly that process and the kernel reported its end.
    Killed,
    /// It ended by itself before a signal could be sent.
    AlreadyGone,
    /// It could not be stopped, or its end could not be confirmed in time.
    Failed(String),
}

fn pidfd_open(pid: u32) -> std::io::Result<OwnedFd> {
    // SAFETY: plain system call; on success the return value is a new descriptor that
    // nothing else owns.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    if fd < 0 { Err(std::io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) }) }
}

/// SIGKILL to the process `running` names — to that process and no other — and wait until
/// `deadline` for the kernel to report that it has ended.
fn stop(running: &Running, deadline: Instant) -> Stopped {
    // The descriptor first: from here on it refers to one process, whatever happens to the
    // number. Then the identity check, which therefore is about that same process.
    let pidfd = match pidfd_open(running.pid) {
        Ok(fd) => fd,
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => return Stopped::AlreadyGone,
        Err(e) => return Stopped::Failed(format!("no handle on the process ({e})")),
    };
    match identity(running.pid) {
        Some((start, _)) if start == running.start => {}
        // The process the marker named is gone (the pid may already be someone else's).
        _ => return Stopped::AlreadyGone,
    }
    // SAFETY: plain system call on a descriptor this function owns; no siginfo is passed.
    let sent = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            libc::SIGKILL,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if sent != 0 {
        let e = std::io::Error::last_os_error();
        return if e.raw_os_error() == Some(libc::ESRCH) {
            Stopped::AlreadyGone
        } else {
            Stopped::Failed(format!("the signal was refused ({e})"))
        };
    }
    // A pidfd becomes readable when its process has ended.
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let mut fds = [libc::pollfd { fd: pidfd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: one valid pollfd; the timeout is bounded by the deadline.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, left.as_millis().min(i32::MAX as u128) as i32) };
        match ready {
            1.. => return Stopped::Killed,
            0 => return Stopped::Failed("it was sent SIGKILL but had not ended when the wait ran out".into()),
            _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => continue,
            _ => return Stopped::Failed(format!("waiting for its end failed ({})", std::io::Error::last_os_error())),
        }
    }
}

/// Stop every marked command, immediately (SIGKILL), and wait up to `wait` in all for the
/// kernel to confirm. The game and its display server die with their launcher: they were
/// started with a parent-death signal, and the launcher is the only holder of the session
/// token. Returns each process with what became of it.
pub fn stop_all(paths: &Paths, wait: Duration) -> Vec<(Running, Stopped)> {
    let deadline = Instant::now() + wait;
    list(paths)
        .into_iter()
        .map(|running| {
            let outcome = stop(&running, deadline);
            if !matches!(outcome, Stopped::Failed(_)) {
                // The process is gone and cannot remove its marker itself.
                let _ = std::fs::remove_file(dir(paths).join(running.pid.to_string()));
            }
            (running, outcome)
        })
        .collect()
}

// ---------------------------------------------------------------- directories of a launch

/// Held by a launch on a directory it created in the cache. While it is held, a sweep
/// leaves the directory alone.
pub struct DirLock(#[allow(dead_code)] Flock<File>);

fn open_dir(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(path)
}

/// Lock `path` (a directory) for the life of the returned value — at most the life of this
/// process: the kernel releases the lock when the process ends, however it ends.
pub fn lock_dir(path: &Path) -> std::io::Result<DirLock> {
    // Waits: the only other holder there can be is a sweep testing the lock for an instant.
    Flock::lock(open_dir(path)?, FlockArg::LockExclusive).map(DirLock).map_err(|(_, e)| e.into())
}

/// Is some process holding [`lock_dir`] on `path`?
fn in_use(path: &Path) -> bool {
    match open_dir(path).map(|file| Flock::lock(file, FlockArg::LockExclusiveNonblock)) {
        Ok(Ok(_free)) => false,
        Ok(Err((_, nix::errno::Errno::EWOULDBLOCK))) => true,
        // Cannot tell (unreadable, vanished): not provably in use; removal decides.
        _ => false,
    }
}

/// What a sweep did.
#[derive(Debug, Default)]
pub struct Sweep {
    /// Directories removed completely.
    pub removed: usize,
    /// Directories that could not be removed (completely), each with the reason.
    pub failed: Vec<String>,
    /// Left alone: locked by a running launch, or younger than the minimum age.
    pub skipped: usize,
    /// Of those, the directories (by name) that a live process holds a lock on. With no
    /// marked launch left, that process is a launch nobody knows about: its marker is gone.
    pub in_use: Vec<String>,
}

/// Remove what launches that did not end properly left in the cache directory: per-launch
/// game roots (`scratch/*`), unpacked natives (`natives-*`) and display directories
/// (`display-*`). A launcher that is killed — by `scram`, by the kernel, by a power cut —
/// cannot clean up after itself.
///
/// Only when no other launch is in progress, only directories that no process holds a lock
/// on, and only ones untouched for `min_age` (a launch that is starting at this very moment
/// has, at worst, a directory it created a moment ago and has not locked yet). The work
/// stops at `deadline`; what is left then is in [`Sweep::failed`].
pub fn sweep_leftovers(paths: &Paths, min_age: Duration, deadline: Option<Instant>) -> Sweep {
    let mut sweep = Sweep::default();
    if list(paths).iter().any(|running| matches!(running.activity, Activity::Launch(_))) {
        return sweep;
    }
    let old_enough = |path: &Path| {
        std::fs::symlink_metadata(path)
            .ok()
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
    for path in candidates {
        // Only real directories: a link or a file with such a name is nobody's leftover.
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let name = path.file_name().map(|n| util::sanitize_line(n.as_encoded_bytes(), 120)).unwrap_or_default();
        if in_use(&path) {
            sweep.skipped += 1;
            sweep.in_use.push(name);
            continue;
        }
        if !old_enough(&path) {
            sweep.skipped += 1;
            continue;
        }
        // The trees were written by a sandbox: see `tree::remove` for what that takes.
        match tree::remove(&path, deadline) {
            Ok(()) => sweep.removed += 1,
            Err(e) => sweep.failed.push(format!("{name}: {e}")),
        }
    }
    sweep
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

    /// The child half of the tests below: this test binary again, idle — optionally holding
    /// a lock on a directory, as a launch does.
    #[test]
    fn idle_child() {
        if std::env::var_os("BASTION_RUNNING_CHILD").is_some() {
            let _lock = std::env::var_os("BASTION_RUNNING_LOCK").map(|dir| lock_dir(Path::new(&dir)).unwrap());
            println!("ready");
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    /// Start the idle child (under `program`, which is this binary or a link to it) and wait
    /// until it runs its test body.
    fn idle(program: &Path, lock: Option<&Path>) -> std::process::Child {
        use std::io::BufRead;
        let mut command = std::process::Command::new(program);
        command
            .args(["--exact", "running::tests::idle_child", "--test-threads=1", "--nocapture"])
            .env("BASTION_RUNNING_CHILD", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        if let Some(dir) = lock {
            command.env("BASTION_RUNNING_LOCK", dir);
        }
        let mut child = command.spawn().unwrap();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("ready") {
            line.clear();
            assert!(out.read_line(&mut line).unwrap() > 0, "the child ended before it was ready");
        }
        child
    }

    fn marker_for(paths: &Paths, child: &std::process::Child, what: &str) -> PathBuf {
        std::fs::create_dir_all(dir(paths)).unwrap();
        let (start, comm) = identity(child.id()).unwrap();
        let marker = dir(paths).join(child.id().to_string());
        std::fs::write(&marker, format!("{start}\n{what}\n{comm}\n")).unwrap();
        marker
    }

    #[test]
    fn leftovers_of_dead_launches_are_swept_but_not_under_a_running_one() {
        let (root, paths) = paths();
        let zero = Duration::ZERO;
        std::fs::create_dir_all(paths.scratch().join("vanilla-0011223344556677/saves")).unwrap();
        std::fs::write(paths.scratch().join("vanilla-0011223344556677/saves/x"), b"x").unwrap();
        std::fs::create_dir_all(paths.cache.join("natives-aabbccdd")).unwrap();
        std::fs::create_dir_all(paths.cache.join("display-aabbccdd")).unwrap();
        std::fs::create_dir_all(paths.cache.join("something-else")).unwrap();
        std::fs::write(paths.cache.join("natives-file"), b"not a directory").unwrap();
        std::os::unix::fs::symlink(&root, paths.cache.join("display-link")).unwrap();
        // Too young: left alone (it could belong to a launch that is just starting).
        let young = sweep_leftovers(&paths, Duration::from_secs(3600), None);
        assert_eq!((young.removed, young.skipped), (0, 3));
        assert!(young.in_use.is_empty(), "too young is not the same as held by a process");

        // Another launch is in progress: nothing is touched.
        let mut child = idle(&std::env::current_exe().unwrap(), None);
        marker_for(&paths, &child, "launch:vanilla");
        assert_eq!(sweep_leftovers(&paths, zero, None).removed, 0);
        assert!(paths.scratch().join("vanilla-0011223344556677/saves/x").exists());
        child.kill().unwrap();
        let _ = child.wait();

        // A command that is not a launch does not hold up the sweep.
        let mut login = idle(&std::env::current_exe().unwrap(), None);
        marker_for(&paths, &login, "command:login");
        let swept = sweep_leftovers(&paths, zero, None);
        assert_eq!((swept.removed, swept.failed.len()), (3, 0), "{:?}", swept.failed);
        login.kill().unwrap();
        let _ = login.wait();
        assert!(!paths.scratch().join("vanilla-0011223344556677").exists());
        assert!(paths.cache.join("something-else").exists() && paths.cache.join("natives-file").exists());
        assert!(paths.cache.join("display-link").is_symlink() && root.exists(), "a link is not a leftover");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The game root of a launch whose marker was lost — or was never recognised — is not
    /// deleted while that launch lives: its lock says so.
    #[test]
    fn a_directory_locked_by_a_live_launch_survives_a_sweep_even_without_a_marker() {
        let (root, paths) = paths();
        let game_root = paths.scratch().join("vanilla-8899aabbccddeeff");
        std::fs::create_dir_all(game_root.join("saves")).unwrap();
        std::fs::write(game_root.join("saves/level.dat"), b"world").unwrap();
        let mut launch = idle(&std::env::current_exe().unwrap(), Some(&game_root));
        assert!(list(&paths).is_empty(), "no marker anywhere");

        let swept = sweep_leftovers(&paths, Duration::ZERO, None);
        assert_eq!((swept.removed, swept.skipped), (0, 1));
        assert_eq!(swept.in_use, ["vanilla-8899aabbccddeeff"], "and the sweep says whose directory is held");
        assert!(game_root.join("saves/level.dat").exists(), "the running game's files were deleted");

        // The lock ends with the process, however it ends.
        launch.kill().unwrap();
        let _ = launch.wait();
        assert_eq!(sweep_leftovers(&paths, Duration::ZERO, None).removed, 1);
        assert!(!game_root.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_leftover_the_game_made_undeletable_is_reported_not_counted() {
        use std::os::unix::fs::PermissionsExt;
        let (root, paths) = paths();
        let stubborn = paths.scratch().join("vanilla-0123456789abcdef");
        std::fs::create_dir_all(stubborn.join("a/b")).unwrap();
        std::fs::write(stubborn.join("a/b/file"), b"x").unwrap();
        std::fs::set_permissions(stubborn.join("a"), std::fs::Permissions::from_mode(0o000)).unwrap();
        // Mode bits do not stop the sweep…
        let swept = sweep_leftovers(&paths, Duration::ZERO, None);
        assert_eq!((swept.removed, swept.failed.len()), (1, 0), "{:?}", swept.failed);
        // …but a deadline does, and then the result says what is left.
        std::fs::create_dir_all(stubborn.join("a")).unwrap();
        let over = Instant::now() - Duration::from_secs(1);
        let swept = sweep_leftovers(&paths, Duration::ZERO, Some(over));
        assert_eq!(swept.removed, 0);
        assert!(
            swept.failed.len() == 1 && swept.failed[0].starts_with("vanilla-0123456789abcdef: "),
            "{:?}",
            swept.failed
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_the_process_that_wrote_a_marker_is_ever_found_or_killed() {
        let (root, paths) = paths();
        assert!(list(&paths).is_empty());
        // Our own marker is not a command "in progress elsewhere".
        let own = Marker::create(&paths, "vanilla").unwrap();
        assert!(list(&paths).is_empty());
        let text = std::fs::read_to_string(dir(&paths).join(std::process::id().to_string())).unwrap();
        let (own_start, own_comm) = identity(std::process::id()).unwrap();
        assert_eq!(text, format!("{own_start}\nlaunch:vanilla\n{own_comm}\n"));

        // Another process of the same program, with a correct marker.
        let mut child = idle(&std::env::current_exe().unwrap(), None);
        let (start, comm) = identity(child.id()).unwrap();
        let marker = marker_for(&paths, &child, "launch:modded");

        // Markers that must not lead to a signal: wrong start time (a reused pid), garbage,
        // a name that is not a pid, an activity that is not one, a process whose command
        // name is not the one the marker recorded.
        std::fs::write(dir(&paths).join("1"), "1\nlaunch:vanilla\nsystemd\n").unwrap();
        let other = std::process::Command::new("/usr/bin/sleep").arg("30").spawn().unwrap();
        let (other_start, _) = identity(other.id()).unwrap();
        std::fs::write(dir(&paths).join(other.id().to_string()), format!("{other_start}\nlaunch:vanilla\n{comm}\n"))
            .unwrap();
        std::fs::write(dir(&paths).join("4194303"), format!("{}\nlaunch:vanilla\n{comm}\n", start + 1)).unwrap();
        std::fs::write(dir(&paths).join("notes"), "x").unwrap();

        assert_eq!(list(&paths), [Running { pid: child.id(), activity: Activity::Launch("modded".into()), start }]);
        assert!(!dir(&paths).join("1").exists() && !dir(&paths).join("notes").exists(), "stale markers are removed");
        assert!(!dir(&paths).join(other.id().to_string()).exists(), "another program under that pid: stale");

        // A marker with the child's pid but another start time is not the child.
        std::fs::write(&marker, format!("{}\nlaunch:modded\n{comm}\n", start + 1)).unwrap();
        assert!(stop_all(&paths, Duration::from_secs(5)).is_empty());
        assert!(child.try_wait().unwrap().is_none(), "the child must still be running");
        // Nor is one whose activity is not understood.
        for bad in ["launch:../x", "exec:sh", "", "command:"] {
            std::fs::write(&marker, format!("{start}\n{bad}\n{comm}\n")).unwrap();
            assert!(list(&paths).is_empty(), "{bad:?}");
        }

        // The marker as an older launcher wrote it (instance name only, no command name).
        std::fs::write(&marker, format!("{start}\nmodded\n")).unwrap();
        assert_eq!(list(&paths)[0].activity, Activity::Launch("modded".into()));
        let stopped = stop_all(&paths, Duration::from_secs(5));
        assert_eq!(stopped.len(), 1);
        assert_eq!(stopped[0].1, Stopped::Killed, "confirmed by the kernel, not assumed");
        // (The child of this test process stays a zombie until waited for: its end is real.)
        let status = child.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGKILL));
        assert!(!marker.exists(), "the marker of a killed process is removed for it");
        let mut other = other;
        assert!(other.try_wait().unwrap().is_none(), "a process of another program is never touched");
        other.kill().unwrap();
        let _ = other.wait();

        assert!(list(&paths).is_empty(), "nothing is left running");
        drop(own);
        assert!(!dir(&paths).join(std::process::id().to_string()).exists(), "a marker is removed with its command");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A launcher that was started through a symbolic link, or installed under another
    /// name, has another command name than the `bastion-mc` that runs scram. It is found
    /// and stopped all the same: the marker says what its own process is called.
    #[test]
    fn a_launcher_running_under_another_name_is_recognised_and_stopped() {
        let (root, paths) = paths();
        let renamed = root.join("mc-launcher-link");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &renamed).unwrap();
        let mut child = idle(&renamed, None);
        let (_, comm) = identity(child.id()).unwrap();
        let (_, own_comm) = identity(std::process::id()).unwrap();
        assert_ne!(comm, own_comm, "the point of the test: the two processes have different command names");
        assert_eq!(comm, "mc-launcher-lin", "(the kernel keeps 15 bytes)");

        marker_for(&paths, &child, "launch:vanilla");
        let found = list(&paths);
        assert_eq!(found.len(), 1, "pruned as stale: scram would have reported that nothing was running");
        assert_eq!(sweep_leftovers(&paths, Duration::ZERO, None).removed, 0);
        let stopped = stop_all(&paths, Duration::from_secs(5));
        assert!(matches!(stopped.as_slice(), [(running, Stopped::Killed)] if running.pid == child.id()), "{stopped:?}");
        let status = child.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGKILL));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_command_that_is_not_a_launch_is_marked_and_stoppable() {
        let (root, paths) = paths();
        let mut login = idle(&std::env::current_exe().unwrap(), None);
        marker_for(&paths, &login, "command:login");
        let found = list(&paths);
        assert_eq!(found[0].activity, Activity::Command("login".into()));
        let stopped = stop_all(&paths, Duration::from_secs(5));
        assert_eq!(stopped[0].1, Stopped::Killed);
        let _ = login.wait();
        // And a process that ended by itself in the meantime is not "killed".
        let mut gone = idle(&std::env::current_exe().unwrap(), None);
        let (start, comm) = identity(gone.id()).unwrap();
        let running = Running { pid: gone.id(), activity: Activity::Command("login".into()), start };
        gone.kill().unwrap();
        let _ = gone.wait();
        assert_eq!(stop(&running, Instant::now() + Duration::from_secs(2)), Stopped::AlreadyGone, "{comm}");
        assert!(Marker::for_command(&paths, "not a name").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
