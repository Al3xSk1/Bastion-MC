//! The first program inside the browser's sandbox: this binary again
//! (`bastion-mc __browser-stage <policy-fd>`), started by bubblewrap. It hardens itself,
//! then becomes the bridge between the launcher and Firefox's remote control and starts
//! Firefox as its child. Order, every step failing closed (Firefox never starts half
//! confined):
//!
//! 1. `no_new_privs`; a fresh, empty session keyring (the inherited one leads to the user's
//!    keyring, where the Microsoft refresh token can be); no core dumps.
//! 2. Landlock: read `/usr`, `/etc`, `/proc`; execute only Firefox's directory and the
//!    dynamic loader; write only the profile, `/tmp`, `/dev/shm` and the runtime directory;
//!    TCP: connect to the relay's port 443 and the remote-control port, bind only the
//!    remote-control port; no signals or abstract sockets beyond the sandbox.
//! 3. A system-call filter (a denylist, `EPERM`): the kernel keyring, `ptrace` and
//!    `process_vm_*`, `bpf`, `perf_event_open`, `userfaultfd`, `io_uring`, every mount
//!    call, `chroot`, `setns`, `unshare` and `clone` with a namespace flag, kernel modules
//!    and `kexec`, the terminal-injection ioctls; `clone3` and every x32 number fail with
//!    `ENOSYS`. Firefox's own sandbox stays possible: it needs `seccomp` and `prctl`, which
//!    are not on the list, and it runs without user namespaces (as it does under Flatpak).
//! 4. Descriptors other than standard input, output and error are closed.
//! 5. Headless (for the phone): Firefox is started as a child with standard input on
//!    `/dev/null`, and this process copies bytes between its own standard input — a socket
//!    whose other end the launcher holds — and Firefox's remote-control port on the
//!    sandbox's loopback, which the launcher, outside the namespace, cannot reach. A window
//!    for signing in (`browser login`): this process becomes Firefox.
//!
//! The bridge reads and writes bytes; it does not look at them.

use std::{
    collections::BTreeMap,
    convert::Infallible,
    ffi::{CString, OsString},
    fs::File,
    io::{Read, Write},
    net::TcpStream,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use landlock::{
    ABI, Access, AccessFs, AccessNet, BitFlags, CompatLevel, Compatible, NetPort, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope, make_bitflags,
};
use nix::{
    sys::{
        resource::{Resource, setrlimit},
        signal::{Signal, kill},
        wait::{WaitPidFlag, WaitStatus, waitpid},
    },
    unistd::{ForkResult, Pid, fork},
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition as Cond, SeccompFilter,
    SeccompRule, TargetArch, sock_filter,
};
use serde::{Deserialize, Serialize};

use crate::{
    error::{Context, Error, Result},
    sandbox::seccomp::{CLONE_NAMESPACE_BITS, FORBIDDEN_IOCTLS},
};

pub const STAGE_ARG: &str = "__browser-stage";
const MAX_POLICY_BYTES: u64 = 1 << 20;
/// How long Firefox has to open its remote-control port.
const AGENT_WAIT: Duration = Duration::from_secs(90);

/// What the launcher hands the stage (JSON on a sealed memfd). Paths and ports only.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StagePolicy {
    /// Firefox, as seen inside the sandbox.
    pub firefox: PathBuf,
    /// Complete argv, including argv[0].
    pub argv: Vec<String>,
    /// The entire environment of Firefox.
    pub env: Vec<(String, String)>,
    pub read: Vec<PathBuf>,
    pub read_exec: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
    /// Devices that may be opened, read and written (no ioctl).
    pub devices: Vec<PathBuf>,
    /// Pathname sockets that may be connected to (the display, for `browser login`).
    pub unix_sockets: Vec<PathBuf>,
    pub tcp_connect: Vec<u16>,
    pub tcp_bind: Vec<u16>,
    /// Bridge standard input to this port on 127.0.0.1 (headless); `None`: become Firefox.
    pub remote_port: Option<u16>,
}

/// Entry point. Never returns.
pub fn main(args: &[OsString]) -> ! {
    let err = match run(args) {
        Ok(never) => match never {},
        Err(e) => e,
    };
    eprintln!("bastion-mc browser stage: {err}");
    eprintln!("bastion-mc browser stage: refusing to start the browser without its whole sandbox");
    // SAFETY: immediate exit.
    unsafe { libc::_exit(126) }
}

fn run(args: &[OsString]) -> Result<Infallible> {
    let fd: RawFd = args
        .first()
        .and_then(|a| a.to_str())
        .and_then(|a| a.parse().ok())
        .filter(|fd| *fd >= 3)
        .ok_or_else(|| Error::Sandbox("missing policy descriptor".into()))?;
    // SAFETY: the launcher passed this descriptor for us to own.
    let file = unsafe { File::from_raw_fd(fd) };
    let mut raw = Vec::new();
    file.take(MAX_POLICY_BYTES).read_to_end(&mut raw).ctx(|| "reading the browser's policy")?;
    let policy: StagePolicy =
        serde_json::from_slice(&raw).map_err(|e| Error::Sandbox(format!("malformed browser policy: {e}")))?;

    nix::sys::prctl::set_no_new_privs().ctx(|| "setting no_new_privs")?;
    join_fresh_keyring()?;
    setrlimit(Resource::RLIMIT_CORE, 1, 1).ctx(|| "disabling core dumps")?;
    landlock(&policy)?;

    // Everything that allocates is prepared before the filter.
    let exec = cstring(policy.firefox.as_os_str().as_bytes())?;
    let argv: Vec<CString> = policy.argv.iter().map(|a| cstring(a.as_bytes())).collect::<Result<_>>()?;
    let env: Vec<CString> = policy
        .env
        .iter()
        .map(|(k, v)| {
            if k.is_empty() || k.contains('=') {
                return Err(Error::Sandbox(format!("invalid environment variable name {k:?}")));
            }
            cstring(format!("{k}={v}").as_bytes())
        })
        .collect::<Result<_>>()?;
    for program in filter()? {
        seccompiler::apply_filter(&program).map_err(|e| Error::Sandbox(format!("installing the filter: {e}")))?;
    }
    // SAFETY: plain syscall; closes descriptors 3 and above.
    if unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) } != 0 {
        return Err(Error::io("closing inherited descriptors", std::io::Error::last_os_error()));
    }

    let Some(port) = policy.remote_port else {
        return nix::unistd::execve(&exec, &argv, &env).ctx(|| format!("executing {}", policy.firefox.display()));
    };
    // SAFETY: this process is single-threaded; the child only rearranges descriptors and execs.
    match unsafe { fork() }.ctx(|| "starting Firefox")? {
        ForkResult::Child => {
            // Firefox must not hold the bridge: its standard input is /dev/null.
            let null = std::fs::OpenOptions::new().read(true).open("/dev/null");
            if let Ok(null) = null {
                // SAFETY: plain dup2 onto standard input.
                unsafe { libc::dup2(null.as_raw_fd(), 0) };
            }
            let _ = nix::unistd::execve(&exec, &argv, &env);
            // SAFETY: immediate exit of the forked child.
            unsafe { libc::_exit(127) }
        }
        ForkResult::Parent { child } => {
            let code = bridge(port, child);
            // SAFETY: immediate exit; nothing to clean up.
            unsafe { libc::_exit(code) }
        }
    }
}

fn cstring(bytes: &[u8]) -> Result<CString> {
    CString::new(bytes).map_err(|_| Error::Sandbox("an argument contains NUL".into()))
}

/// Replace the inherited session keyring with a new anonymous one (see `sandbox::init`).
fn join_fresh_keyring() -> Result<()> {
    const KEYCTL_JOIN_SESSION_KEYRING: libc::c_int = 1;
    // SAFETY: keyctl(KEYCTL_JOIN_SESSION_KEYRING, NULL) creates and joins an anonymous keyring.
    let rc = unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_JOIN_SESSION_KEYRING, std::ptr::null::<libc::c_char>()) };
    if rc < 0 {
        return Err(Error::io("joining a fresh session keyring", std::io::Error::last_os_error()));
    }
    Ok(())
}

fn landlock_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Sandbox(format!("Landlock: {what}: {e}"))
}

fn landlock(policy: &StagePolicy) -> Result<()> {
    let abi = crate::sandbox::landlock::kernel_abi();
    if abi < crate::sandbox::landlock::MIN_KERNEL_ABI {
        return Err(Error::Sandbox(format!(
            "kernel Landlock ABI is {abi}; at least {} is required",
            crate::sandbox::landlock::MIN_KERNEL_ABI
        )));
    }
    let target = ABI::V9;
    let read = make_bitflags!(AccessFs::{ReadFile | ReadDir});
    let write =
        read | make_bitflags!(AccessFs::{WriteFile | Truncate | MakeReg | MakeDir | RemoveFile | RemoveDir | Refer});
    let file_rights = AccessFs::from_file(target);
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(target))
        .map_err(|e| landlock_err("handling filesystem access", e))?
        .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)
        .map_err(|e| landlock_err("handling network access", e))?
        .scope(Scope::AbstractUnixSocket | Scope::Signal)
        .map_err(|e| landlock_err("scoping", e))?
        .create()
        .map_err(|e| landlock_err("creating the ruleset", e))?;
    let mut grant = |path: &Path, access: BitFlags<AccessFs>, optional: bool| -> Result<()> {
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(_) if optional => return Ok(()),
            Err(e) => return Err(landlock_err(&format!("policy path {}", path.display()), e)),
        };
        let access = if meta.is_dir() { access } else { access & file_rights };
        let fd = PathFd::new(path).map_err(|e| landlock_err(&format!("opening {}", path.display()), e))?;
        (&mut ruleset)
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|e| landlock_err(&format!("adding a rule for {}", path.display()), e))?;
        Ok(())
    };
    for p in &policy.read {
        grant(p, read, true)?;
    }
    for p in &policy.read_exec {
        grant(p, read | AccessFs::Execute, false)?;
    }
    for p in &policy.write {
        grant(p, write, false)?;
    }
    for p in &policy.devices {
        grant(p, make_bitflags!(AccessFs::{ReadFile | WriteFile}), true)?;
    }
    for p in &policy.unix_sockets {
        grant(p, make_bitflags!(AccessFs::{ReadFile | WriteFile | ResolveUnix}), false)?;
    }
    for port in &policy.tcp_connect {
        (&mut ruleset)
            .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
            .map_err(|e| landlock_err(&format!("allowing TCP port {port}"), e))?;
    }
    for port in &policy.tcp_bind {
        (&mut ruleset)
            .add_rule(NetPort::new(*port, AccessNet::BindTcp))
            .map_err(|e| landlock_err(&format!("allowing to bind TCP port {port}"), e))?;
    }
    let status = ruleset.restrict_self().map_err(|e| landlock_err("restrict_self", e))?;
    if status.ruleset == RulesetStatus::NotEnforced || !status.no_new_privs {
        return Err(Error::Sandbox("the Landlock ruleset was not enforced by the kernel".into()));
    }
    Ok(())
}

/// Denied with `EPERM`. Not one of these is needed by Firefox or its own sandbox.
#[rustfmt::skip]
pub const DENIED: &[(&str, i64)] = &[
    ("keyctl", libc::SYS_keyctl), ("add_key", libc::SYS_add_key), ("request_key", libc::SYS_request_key),
    ("ptrace", libc::SYS_ptrace), ("process_vm_readv", libc::SYS_process_vm_readv),
    ("process_vm_writev", libc::SYS_process_vm_writev), ("pidfd_getfd", libc::SYS_pidfd_getfd),
    ("bpf", libc::SYS_bpf), ("perf_event_open", libc::SYS_perf_event_open), ("userfaultfd", libc::SYS_userfaultfd),
    ("io_uring_setup", libc::SYS_io_uring_setup), ("io_uring_enter", libc::SYS_io_uring_enter),
    ("io_uring_register", libc::SYS_io_uring_register),
    ("mount", libc::SYS_mount), ("umount2", libc::SYS_umount2), ("pivot_root", libc::SYS_pivot_root),
    ("chroot", libc::SYS_chroot), ("open_tree", libc::SYS_open_tree), ("move_mount", libc::SYS_move_mount),
    ("fsopen", libc::SYS_fsopen), ("fsconfig", libc::SYS_fsconfig), ("fsmount", libc::SYS_fsmount),
    ("fspick", libc::SYS_fspick), ("mount_setattr", libc::SYS_mount_setattr),
    ("setns", libc::SYS_setns), ("unshare", libc::SYS_unshare),
    ("open_by_handle_at", libc::SYS_open_by_handle_at), ("name_to_handle_at", libc::SYS_name_to_handle_at),
    ("kexec_load", libc::SYS_kexec_load), ("kexec_file_load", libc::SYS_kexec_file_load),
    ("init_module", libc::SYS_init_module), ("finit_module", libc::SYS_finit_module),
    ("delete_module", libc::SYS_delete_module), ("reboot", libc::SYS_reboot), ("swapon", libc::SYS_swapon),
    ("swapoff", libc::SYS_swapoff), ("acct", libc::SYS_acct), ("iopl", libc::SYS_iopl), ("ioperm", libc::SYS_ioperm),
    ("modify_ldt", libc::SYS_modify_ldt), ("syslog", libc::SYS_syslog), ("quotactl", libc::SYS_quotactl),
    ("vhangup", libc::SYS_vhangup),
];

const X32_SYSCALL_BIT: u32 = 0x4000_0000;

fn compile(rules: BTreeMap<i64, Vec<SeccompRule>>, matched: SeccompAction) -> Result<BpfProgram> {
    // seccompiler's programs begin by killing the process on a foreign architecture.
    SeccompFilter::new(rules, SeccompAction::Allow, matched, TargetArch::x86_64)
        .and_then(BpfProgram::try_from)
        .map_err(|e| Error::Sandbox(format!("compiling the browser's filter: {e}")))
}

fn rule(arg: u8, len: Len, op: Op, value: u64) -> Result<SeccompRule> {
    Cond::new(arg, len, op, value)
        .and_then(|condition| SeccompRule::new(vec![condition]))
        .map_err(|e| Error::Sandbox(format!("building a rule of the browser's filter: {e}")))
}

/// The browser's system-call filter, in installation order.
pub fn filter() -> Result<Vec<BpfProgram>> {
    let mut denied: BTreeMap<i64, Vec<SeccompRule>> = DENIED.iter().map(|(_, nr)| (*nr, vec![])).collect();
    denied.insert(
        libc::SYS_clone,
        (0..64)
            .map(|bit| 1u64 << bit)
            .filter(|bit| CLONE_NAMESPACE_BITS & bit != 0)
            .map(|bit| rule(0, Len::Qword, Op::MaskedEq(bit), bit))
            .collect::<Result<_>>()?,
    );
    denied.insert(
        libc::SYS_ioctl,
        FORBIDDEN_IOCTLS.iter().map(|request| rule(1, Len::Dword, Op::Eq, *request)).collect::<Result<_>>()?,
    );
    let denied = compile(denied, SeccompAction::Errno(libc::EPERM as u32))?;
    // ENOSYS for clone3 (glibc then uses clone, whose flags the filter can read) and, checked
    // first, for every x32 number (without this the list above could be reached under them).
    let stmt = |code: u32, k: u32| sock_filter { code: code as u16, jt: 0, jf: 0, k };
    let mut absent = vec![
        stmt(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0),
        sock_filter { code: (libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K) as u16, jt: 0, jf: 1, k: X32_SYSCALL_BIT },
        stmt(libc::BPF_RET | libc::BPF_K, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
    ];
    absent.extend(compile([(libc::SYS_clone3, vec![])].into(), SeccompAction::Errno(libc::ENOSYS as u32))?);
    Ok(vec![denied, absent])
}

/// Wait for Firefox's remote-control port, then copy bytes both ways between it and
/// standard input until either side ends. Returns Firefox's exit status.
fn bridge(port: u16, child: Pid) -> i32 {
    let started = Instant::now();
    let mut agent = loop {
        if let Some(code) = reap(child, false) {
            return code;
        }
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => break stream,
            Err(_) if started.elapsed() < AGENT_WAIT => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => {
                eprintln!("bastion-mc browser stage: Firefox did not open its remote-control port in time");
                return stop(child);
            }
        }
    };
    let _ = agent.set_nodelay(true);
    // SAFETY: standard input is the launcher's socket, owned by this process.
    let mut launcher = unsafe { File::from_raw_fd(0) };
    let mut buf = vec![0u8; 64 * 1024];
    let mut fds = [
        libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: agent.as_raw_fd(), events: libc::POLLIN, revents: 0 },
    ];
    loop {
        if let Some(code) = reap(child, false) {
            return code;
        }
        for fd in &mut fds {
            fd.revents = 0;
        }
        // SAFETY: two valid pollfd entries.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, 500) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return stop(child);
        }
        if fds[0].revents != 0 {
            match launcher.read(&mut buf) {
                // The launcher is done with the browser: so is the browser.
                Ok(0) | Err(_) => return stop(child),
                Ok(n) => {
                    if agent.write_all(&buf[..n]).is_err() {
                        return stop(child);
                    }
                }
            }
        }
        if fds[1].revents != 0 {
            match agent.read(&mut buf) {
                Ok(0) | Err(_) => {
                    // The remote control closed; Firefox decides whether it goes on.
                    let _ = nix::sys::socket::shutdown(0, nix::sys::socket::Shutdown::Write);
                    return reap(child, true).unwrap_or(1);
                }
                Ok(n) => {
                    if launcher.write_all(&buf[..n]).is_err() {
                        return stop(child);
                    }
                }
            }
        }
    }
}

/// Firefox's exit status once it has exited (`block`: wait for it).
fn reap(child: Pid, block: bool) -> Option<i32> {
    let flags = if block { None } else { Some(WaitPidFlag::WNOHANG) };
    loop {
        match waitpid(child, flags) {
            Ok(WaitStatus::Exited(_, code)) => return Some(code),
            Ok(WaitStatus::Signaled(_, signal, _)) => return Some(128 + signal as i32),
            Ok(WaitStatus::StillAlive) => return None,
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => return Some(1),
        }
    }
}

/// Ask Firefox to exit, and make it if it does not within a few seconds.
fn stop(child: Pid) -> i32 {
    let _ = kill(child, Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(code) = reap(child, false) {
            return code;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = kill(child, Signal::SIGKILL);
    reap(child, true).unwrap_or(137)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_compiles_and_starts_with_the_x32_guard() {
        let programs = filter().unwrap();
        assert_eq!(programs.len(), 2);
        let guard = &programs[1][..3];
        assert_eq!((guard[1].code, guard[1].k), (0x35, 0x4000_0000));
        assert_eq!(guard[2].k, 0x0005_0000 | libc::ENOSYS as u32);
    }

    #[test]
    fn what_firefox_and_its_own_sandbox_need_is_not_denied() {
        let needed = [
            libc::SYS_seccomp,
            libc::SYS_prctl,
            libc::SYS_clone,
            libc::SYS_execve,
            libc::SYS_socket,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_connect,
            libc::SYS_memfd_create,
            libc::SYS_mmap,
            libc::SYS_mprotect,
            libc::SYS_sched_setaffinity,
            libc::SYS_sched_getaffinity,
            libc::SYS_futex,
            libc::SYS_kill,
            libc::SYS_tgkill,
            libc::SYS_wait4,
            libc::SYS_getrandom,
        ];
        for nr in needed {
            assert!(!DENIED.iter().any(|(_, d)| *d == nr), "system call {nr} is needed by Firefox");
        }
        for name in ["keyctl", "add_key", "request_key", "ptrace", "unshare", "setns", "mount", "chroot", "bpf"] {
            assert!(DENIED.iter().any(|(n, _)| *n == name), "{name}");
        }
    }
}
