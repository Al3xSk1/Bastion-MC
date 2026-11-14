//! The launcher confines itself once the game is running.
//!
//! Until the sandbox is up the launcher needs most of what a user can do: read the whole
//! store, run `bwrap`, `systemd-run` and `age`, write tokens, bind sockets. Afterwards it is
//! a supervisor and a relay, and the only untrusted input it still handles comes from the
//! game: DNS questions, TLS hellos, HTTP requests, log lines. If a bug in that handling were
//! ever exploited, the attacker would be running as you, outside the sandbox.
//!
//! So at that point the launcher drops what it no longer needs, for **every thread** of the
//! process (the async runtime's workers already exist by then), with two mechanisms:
//!
//! **Landlock** (`LANDLOCK_RESTRICT_SELF_TSYNC`) for what is named by path or port:
//!
//! * filesystem: read-only access to `/etc`, `/usr` and `/proc` (name resolution, the
//!   supervisor's own bookkeeping); write access only to the trees the caller lists — for a
//!   launch that is the instance's `persist/` directory (settings carried back) and the
//!   launcher's cache (scratch directory removed). No `$HOME`, no SSH keys, no browser
//!   profile, no other instance, and not the instance's own configuration or manifest;
//! * no `execve` of anything;
//! * TCP: `connect()` only to the ports the relay serves, `bind()` not at all;
//! * no connections to abstract UNIX sockets of other processes (the X server's, D-Bus's),
//!   and — where the kernel can express it (Landlock ABI 9) — to no pathname socket other
//!   than the name resolver's.
//!
//! **A system-call filter** (seccomp, `SECCOMP_FILTER_FLAG_TSYNC`; see [`filter`]) for what
//! has no path: Landlock does not mediate the kernel keyring, and the default token store
//! keeps the Microsoft refresh token exactly there. After confinement `keyctl`, `add_key`
//! and `request_key` fail, as do `execve`, new namespaces, `io_uring`, `bpf` and the other
//! calls a supervisor never makes and an exploit wants.
//!
//! Signals are deliberately *not* scoped: the launcher must still be able to stop the game,
//! which lives outside this Landlock domain. Descriptors that are already open (the log
//! file, the relay's sockets, the control pipe, the terminal the launcher was started from)
//! keep working; Landlock governs new opens.
//!
//! **What "confined" means, and when it is only partly true.** The whole of the above needs
//! Landlock ABI 9. On an older kernel some of it cannot be enforced ([`support_for`] says
//! what), and nothing here pretends otherwise: [`apply`] returns the parts that are *not* in
//! force, `launcher.confine = "require"` refuses anything less than the whole, and with
//! `"auto"` the launch log, `doctor` and the front end's protection display say "partly".
//!
//! What is **not** restricted even when all of it is in force: UDP (the DNS responder
//! answers to the game's ephemeral ports), TCP by destination (the rules are per port),
//! writing to the terminal the launcher was started from (only the injection ioctls are
//! denied), and the access token the broker holds — a compromised launcher has it.
//!
//! This is not a second sandbox for the game. It bounds what a compromised *launcher* could
//! reach, and it cannot be undone for the rest of the process's life.

pub mod filter;

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
/// Connections to pathname UNIX sockets are mediated from this ABI on. Below it the
/// session's D-Bus and systemd's private socket stay reachable, and with them a way to have
/// a program started outside the confinement.
pub const FULL_ABI: i32 = 9;

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
        // resolv.conf is often a symlink into /run (systemd-resolved, NetworkManager). The
        // daemon that owns it replaces the file when the network changes, so it is the
        // file's directory that has to stay readable — but only a directory of the daemon's
        // own under /run (`/run/systemd/resolve`, `/run/NetworkManager`). Where the file
        // sits directly in `/run`, or anywhere else (a link into a home directory, say),
        // granting its directory would expose everything in it, so only the file is granted
        // (and a configuration rewritten while the game runs is then no longer readable:
        // lookups fall back to glibc's defaults).
        if let Ok(target) = std::fs::canonicalize("/etc/resolv.conf")
            && !target.starts_with("/etc")
        {
            read.push(resolver_grant(&target));
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

/// What to keep readable for a resolver configuration that really lives at `target`: its
/// directory if that is a daemon's own directory under `/run`, otherwise the file alone.
fn resolver_grant(target: &Path) -> PathBuf {
    match target.parent() {
        Some(dir) if dir.starts_with("/run") && dir.components().count() >= 3 => dir.to_path_buf(),
        _ => target.to_path_buf(),
    }
}

/// How much of the confinement a kernel can enforce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// All of what the module header describes.
    Whole,
    /// Some of it. Each entry names one thing that will *not* be in force.
    Partly(Vec<String>),
}

impl Support {
    pub fn gaps(&self) -> &[String] {
        match self {
            Support::Whole => &[],
            Support::Partly(gaps) => gaps,
        }
    }
}

/// What a kernel with Landlock ABI `abi` can enforce. (The system-call filter needs nothing
/// newer than what the game's sandbox already requires.)
pub fn support_for(abi: i32) -> Support {
    if abi >= FULL_ABI {
        Support::Whole
    } else if abi >= MIN_ABI {
        Support::Partly(vec![format!(
            "this kernel's Landlock (ABI {abi}) cannot restrict connections to pathname UNIX sockets (ABI \
             {FULL_ABI} can): the session's D-Bus and systemd's own socket stay reachable, and through them a \
             program can be started outside the confinement"
        )])
    } else {
        Support::Partly(vec![format!(
            "this kernel's Landlock (ABI {abi}) cannot confine every thread of a running process (ABI {MIN_ABI} \
             can): files and the network are NOT restricted; only the system-call filter applies (no kernel \
             keyring, no program execution)"
        )])
    }
}

/// What the running kernel can enforce.
pub fn support() -> Support {
    support_for(kernel_abi())
}

/// How much the caller insists on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Demand {
    /// All of it or an error (`launcher.confine = "require"`).
    Whole,
    /// As much as this kernel can do; what is missing is reported (`"auto"`).
    Possible,
}

/// What is in force after [`apply`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Applied {
    /// Parts of the confinement that are **not** in force. Empty: all of it is.
    pub gaps: Vec<String>,
}

impl Applied {
    pub fn is_whole(&self) -> bool {
        self.gaps.is_empty()
    }
}

fn landlock_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Sandbox(format!("confining the launcher: {what}: {e}"))
}

/// The Landlock half. `abi` decides which rights are asked for; nothing is requested
/// best-effort, so a right the kernel turns out not to have is an error, never a silently
/// thinner ruleset.
fn restrict(confinement: &Confinement, abi: i32) -> Result<()> {
    let pathname_sockets = abi >= FULL_ABI;
    let level = if pathname_sockets { ABI::V9 } else { ABI::V8 };
    let read = make_bitflags!(AccessFs::{ReadFile | ReadDir});
    let write =
        read | make_bitflags!(AccessFs::{WriteFile | Truncate | MakeReg | MakeDir | RemoveFile | RemoveDir | Refer});
    let file_rights = AccessFs::from_file(level);

    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(level))
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
        let mut access = make_bitflags!(AccessFs::{ReadFile | WriteFile});
        if pathname_sockets {
            access |= AccessFs::ResolveUnix;
        }
        grant(path, access, true)?;
    }
    for port in &confinement.tcp_connect {
        (&mut ruleset)
            .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
            .map_err(|e| landlock_err(&format!("allowing TCP port {port}"), e))?;
    }

    // A restriction that silently covered one thread out of many would be worse than none.
    let status = ruleset
        .all_threads(true)
        .map_err(|e| landlock_err("requesting all threads", e))?
        .restrict_self()
        .map_err(|e| landlock_err("restrict_self", e))?;
    if status.ruleset != RulesetStatus::FullyEnforced || !status.no_new_privs || !status.all_threads {
        return Err(Error::Sandbox(format!(
            "confining the launcher: the kernel did not enforce the whole ruleset on all threads ({:?})",
            status.ruleset
        )));
    }
    Ok(())
}

/// Confine the whole process. Irreversible.
///
/// With [`Demand::Whole`], a kernel that cannot enforce all of it is refused *before*
/// anything is changed; a failure after that point is an error too, and the caller must
/// stop (the process may already be partly restricted). With [`Demand::Possible`] the
/// result lists what is not in force; an error then means that not even the system-call
/// filter could be installed.
pub fn apply(confinement: &Confinement, demand: Demand) -> Result<Applied> {
    apply_as(confinement, demand, kernel_abi())
}

/// [`apply`] for a kernel with Landlock ABI `abi`. Passing a lower number than the running
/// kernel's applies exactly what such a kernel would enforce: how the tests see the partial
/// states on a machine that has none.
#[doc(hidden)]
pub fn apply_as(confinement: &Confinement, demand: Demand, abi: i32) -> Result<Applied> {
    let abi = abi.min(kernel_abi());
    let mut gaps = support_for(abi).gaps().to_vec();
    if demand == Demand::Whole && !gaps.is_empty() {
        return Err(Error::Sandbox(format!("the launcher cannot confine itself completely: {}", gaps.join("; "))));
    }
    if abi >= MIN_ABI
        && let Err(e) = restrict(confinement, abi)
    {
        if demand == Demand::Whole {
            return Err(e);
        }
        // What `support_for` said about this kernel no longer describes what happened.
        gaps = vec![format!("Landlock was not applied ({e}): files and the network are NOT restricted")];
    }
    // Last, as in the game's sandbox: nothing above needs a call the filter takes away.
    match filter::install() {
        Ok(()) => {}
        Err(e) if demand == Demand::Whole => return Err(e),
        Err(e) if abi < MIN_ABI || gaps.iter().any(|g| g.starts_with("Landlock was not applied")) => {
            return Err(Error::Sandbox(format!(
                "the launcher could not confine itself at all: {}; {e}",
                gaps.join("; ")
            )));
        }
        Err(e) => gaps.push(format!(
            "the system-call filter was not installed ({e}): the kernel keyring (and with it a refresh token \
             stored there) stays reachable"
        )),
    }
    Ok(Applied { gaps })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_kernel_that_mediates_pathname_sockets_supports_the_whole_confinement() {
        assert_eq!(support_for(10), Support::Whole);
        assert_eq!(support_for(FULL_ABI), Support::Whole);
        let Support::Partly(gaps) = support_for(8) else { panic!("ABI 8 has no pathname-socket control") };
        assert!(gaps.len() == 1 && gaps[0].contains("pathname UNIX sockets") && gaps[0].contains("D-Bus"), "{gaps:?}");
        for old in [7, 6, 0, -1] {
            let Support::Partly(gaps) = support_for(old) else { panic!("ABI {old} cannot confine all threads") };
            assert!(gaps[0].contains("NOT restricted") && gaps[0].contains("system-call filter"), "{gaps:?}");
        }
    }

    #[test]
    fn the_baseline_never_grants_a_shared_directory_for_the_resolver_configuration() {
        let keep = Confinement::supervisor();
        assert!(keep.write.is_empty(), "the baseline writes nowhere");
        // Whatever /etc/resolv.conf points to on this machine: beyond /etc, /usr and /proc,
        // only that file or a daemon's own directory under /run is readable.
        for path in &keep.read {
            let fixed = ["/etc", "/usr", "/proc"].iter().any(|p| path == Path::new(p));
            let daemons_own = path.starts_with("/run") && path.components().count() >= 3;
            assert!(fixed || path.is_file() || daemons_own, "{} is a shared directory", path.display());
        }
        // And for the places a resolver configuration can really be:
        for (target, granted) in [
            ("/run/systemd/resolve/stub-resolv.conf", "/run/systemd/resolve"),
            ("/run/NetworkManager/resolv.conf", "/run/NetworkManager"),
            ("/run/resolv.conf", "/run/resolv.conf"),
            ("/home/alex/dotfiles/resolv.conf", "/home/alex/dotfiles/resolv.conf"),
            ("/var/lib/misc/resolv.conf", "/var/lib/misc/resolv.conf"),
            ("/resolv.conf", "/resolv.conf"),
        ] {
            assert_eq!(resolver_grant(Path::new(target)), Path::new(granted), "{target}");
        }
    }
}
