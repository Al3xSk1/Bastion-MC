//! The launcher confines itself once the game is running.
//!
//! Until the sandbox is up the launcher needs most of what a user can do: read the whole
//! store, run `bwrap`, `systemd-run` and `age`, write tokens, bind sockets. Afterwards it is
//! a supervisor and a relay, and the only untrusted input it still handles comes from the
//! game: DNS questions, TLS hellos, HTTP requests, log lines. If a bug in that handling were
//! ever exploited, the attacker would be running as you, outside the sandbox.
//!
//! So at that point the launcher drops what it no longer needs, with Landlock, for **every
//! thread** of the process (`LANDLOCK_RESTRICT_SELF_TSYNC`; the async runtime's workers
//! already exist by then):
//!
//! * filesystem: read-only access to `/etc`, `/usr` and `/proc` (name resolution, the
//!   supervisor's own bookkeeping); write access only to the instance directory and the
//!   launcher's cache (settings carried back, scratch directory removed). No `$HOME`, no
//!   SSH keys, no browser profile, no other instance.
//! * no `execve` of anything;
//! * TCP: `connect()` only to the ports the relay serves, `bind()` not at all;
//! * no connections to abstract UNIX sockets of other processes (the X server's, D-Bus's),
//!   and — where the kernel can express it — to no pathname socket other than the name
//!   resolver's.
//!
//! Signals are deliberately *not* scoped: the launcher must still be able to stop the game,
//! which lives outside this Landlock domain. Descriptors that are already open (the log
//! file, the relay's sockets, the control pipe) keep working; Landlock governs new opens.
//!
//! This is not a second sandbox for the game. It bounds what a compromised *launcher* could
//! reach, and it cannot be undone for the rest of the process's life.

use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, AccessNet, BitFlags, CompatLevel, Compatible, NetPort, PathBeneath, PathFd,
    RestrictSelfAttr, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope, make_bitflags,
};

use crate::{
    error::{Error, Result},
    sandbox::landlock::kernel_abi,
};

/// Restricting all threads at once needs this Landlock ABI. Without it only the calling
/// thread could be confined, which would be a false promise in a multi-threaded process.
pub const MIN_ABI: i32 = 8;
const TARGET_ABI: ABI = ABI::V9;

/// What the launcher keeps.
#[derive(Debug, Clone, Default)]
pub struct Confinement {
    /// Readable trees (missing ones are skipped).
    pub read: Vec<PathBuf>,
    /// Trees in which files and directories may be read, created, changed and removed.
    pub write: Vec<PathBuf>,
    /// Pathname UNIX sockets that may be connected to (missing ones are skipped).
    pub unix_sockets: Vec<PathBuf>,
    /// TCP ports `connect()` may target. `bind()` is always denied.
    pub tcp_connect: Vec<u16>,
}

impl Confinement {
    /// The baseline every supervising launcher needs: name resolution and its own `/proc`.
    pub fn supervisor() -> Self {
        let mut read: Vec<PathBuf> = ["/etc", "/usr", "/proc"].iter().map(PathBuf::from).collect();
        // resolv.conf is often a symlink into /run (systemd-resolved, NetworkManager).
        if let Ok(target) = std::fs::canonicalize("/etc/resolv.conf")
            && !target.starts_with("/etc")
            && let Some(dir) = target.parent()
        {
            read.push(dir.to_path_buf());
        }
        Confinement {
            read,
            write: Vec::new(),
            // glibc's `resolve` and `nscd` lookups; without them it falls back to files/dns.
            unix_sockets: vec!["/run/systemd/resolve/io.systemd.Resolve".into(), "/run/nscd/socket".into()],
            // DNS over TCP (large answers, SRV lookups).
            tcp_connect: vec![53],
        }
    }
}

/// Can this kernel confine all threads of a process?
pub fn supported() -> std::result::Result<(), String> {
    let abi = kernel_abi();
    if abi >= MIN_ABI {
        Ok(())
    } else {
        Err(format!(
            "the kernel's Landlock ABI is {abi}; confining every thread of a running process needs ABI {MIN_ABI}"
        ))
    }
}

fn landlock_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Sandbox(format!("confining the launcher: {what}: {e}"))
}

/// Confine the whole process. Irreversible. Fails without changing anything if the kernel
/// cannot apply the restriction to all threads.
pub fn apply(confinement: &Confinement) -> Result<()> {
    supported().map_err(Error::Sandbox)?;

    let read = make_bitflags!(AccessFs::{ReadFile | ReadDir});
    let write =
        read | make_bitflags!(AccessFs::{WriteFile | Truncate | MakeReg | MakeDir | RemoveFile | RemoveDir | Refer});
    let file_rights = AccessFs::from_file(TARGET_ABI);

    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(TARGET_ABI))
        .map_err(|e| landlock_err("handling filesystem access", e))?
        .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)
        .map_err(|e| landlock_err("handling network access", e))?
        .scope(Scope::AbstractUnixSocket)
        .map_err(|e| landlock_err("scoping", e))?
        .create()
        .map_err(|e| landlock_err("creating the ruleset", e))?;

    let mut grant = |path: &Path, access: BitFlags<AccessFs>, optional: bool| -> Result<()> {
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(_) if optional => return Ok(()),
            Err(e) => return Err(landlock_err(&format!("{}", path.display()), e)),
        };
        let access = if meta.is_dir() { access } else { access & file_rights };
        let fd = PathFd::new(path).map_err(|e| landlock_err(&format!("opening {}", path.display()), e))?;
        (&mut ruleset)
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|e| landlock_err(&format!("adding a rule for {}", path.display()), e))?;
        Ok(())
    };
    for path in &confinement.read {
        grant(path, read, true)?;
    }
    for path in &confinement.write {
        grant(path, write, false)?;
    }
    for path in &confinement.unix_sockets {
        grant(path, make_bitflags!(AccessFs::{ReadFile | WriteFile | ResolveUnix}), true)?;
    }
    for port in &confinement.tcp_connect {
        (&mut ruleset)
            .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
            .map_err(|e| landlock_err(&format!("allowing TCP port {port}"), e))?;
    }

    // Everything above degrades gracefully on older kernels; this does not. A restriction
    // that silently covered one thread out of many would be worse than none.
    let status = ruleset
        .set_compatibility(CompatLevel::HardRequirement)
        .all_threads(true)
        .map_err(|e| landlock_err("requesting all threads", e))?
        .restrict_self()
        .map_err(|e| landlock_err("restrict_self", e))?;
    if status.ruleset == RulesetStatus::NotEnforced || !status.no_new_privs {
        return Err(Error::Sandbox("confining the launcher: the kernel did not enforce the ruleset".into()));
    }
    Ok(())
}
