//! The system-call filter of the confined launcher.
//!
//! Landlock names files, ports and sockets. Some of what a compromised launcher would reach
//! for has no name of that kind — above all the kernel keyring, where the default token
//! store keeps the Microsoft refresh token: one `keyctl` search and one read, and the
//! long-lived credential is in the attacker's hands. This filter takes those calls away, on
//! every thread (`SECCOMP_FILTER_FLAG_TSYNC`), for the rest of the process's life.
//!
//! Unlike the game's filter this one is a **denylist**. The launcher is a multi-threaded
//! program built on glibc, a resolver, an async runtime and a TLS stack; an allowlist of
//! what all of that may ever call would fail in the middle of somebody's session the day a
//! library took a new path. What is denied instead is a short list of calls of which each
//! is (a) provably not made by a supervising launcher after this point and (b) wanted by an
//! exploit:
//!
//! | calls | verdict | why |
//! |---|---|---|
//! | `keyctl`, `add_key`, `request_key` | `EPERM` | the refresh token in the kernel keyring |
//! | `execve`, `execveat` | `EPERM` | every program the launcher starts was started before this point |
//! | `unshare`, `clone` with a namespace flag, `clone3` (`ENOSYS`: glibc then uses `clone`, whose flags a filter can read) | `EPERM` | a new user namespace is where most kernel exploits start |
//! | `io_uring_*`, `userfaultfd`, `perf_event_open`, `modify_ldt` | `EPERM` | kernel attack surface nothing here uses (`bpf` is in the next row) |
//! | `pidfd_getfd` | `EPERM` | taking descriptors out of other processes |
//! | `ioctl` with `TIOCSTI`, `TIOCLINUX`, `TIOCCONS`, `TIOCSETD` | `EPERM` | typing into the terminal the launcher was started from |
//! | the game sandbox's tripwire list (`ptrace`, `process_vm_*`, `bpf`, the mount API, `setns`, `kexec_*`, modules, …) | the process is killed | exploit-only; a launcher that issues one is no longer the launcher |
//! | anything through the x32 entry points (`nr \| 0x40000000`) | `ENOSYS` | this kernel has `CONFIG_X86_X32_ABI`: without this line every entry above could be reached under its x32 number |
//! | anything from another architecture's table (`int 0x80`) | the process is killed | same reason, for the i386 numbers |
//!
//! The last two rows are what makes a denylist sound here; the comments in
//! `sandbox::seccomp` describe the problem.
//!
//! Not on the list, on purpose: socket families (glibc's resolver opens a netlink socket),
//! `prctl` (threads are named with it), `memfd_secret`/`mlock` (token memory). A call that
//! first appears in a future kernel is allowed until someone adds it here — the price of a
//! denylist, and the reason the game gets an allowlist instead.

use std::collections::BTreeMap;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition as Cond, SeccompFilter,
    SeccompRule, TargetArch, sock_filter,
};

use crate::{
    error::{Error, Result},
    sandbox::seccomp::{CLONE_NAMESPACE_BITS, FORBIDDEN_IOCTLS, TRIPWIRE},
};

/// Denied outright (`EPERM`).
#[rustfmt::skip]
pub const DENIED: &[(&str, i64)] = &[
    ("keyctl", libc::SYS_keyctl), ("add_key", libc::SYS_add_key), ("request_key", libc::SYS_request_key),
    ("execve", libc::SYS_execve), ("execveat", libc::SYS_execveat),
    ("unshare", libc::SYS_unshare),
    ("io_uring_setup", libc::SYS_io_uring_setup), ("io_uring_enter", libc::SYS_io_uring_enter),
    ("io_uring_register", libc::SYS_io_uring_register),
    ("userfaultfd", libc::SYS_userfaultfd), ("perf_event_open", libc::SYS_perf_event_open),
    ("modify_ldt", libc::SYS_modify_ldt),
    ("pidfd_getfd", libc::SYS_pidfd_getfd),
];

/// Bit 30 of the system-call number selects the x32 table.
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

fn seccomp_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Sandbox(format!("confining the launcher: system-call filter: {what}: {e}"))
}

fn compile(rules: BTreeMap<i64, Vec<SeccompRule>>, matched: SeccompAction) -> Result<BpfProgram> {
    // seccompiler's programs begin by killing the process on a foreign architecture.
    SeccompFilter::new(rules, SeccompAction::Allow, matched, TargetArch::x86_64)
        .and_then(BpfProgram::try_from)
        .map_err(|e| seccomp_err("compiling", e))
}

fn rule(arg: u8, len: Len, op: Op, value: u64) -> Result<SeccompRule> {
    Cond::new(arg, len, op, value)
        .and_then(|condition| SeccompRule::new(vec![condition]))
        .map_err(|e| seccomp_err("building a rule", e))
}

/// The filters in installation order. The kernel runs all of them for every call and takes
/// the most severe verdict.
pub fn build() -> Result<Vec<BpfProgram>> {
    // 1. Exploit-only calls: the process dies.
    let tripwire = compile(TRIPWIRE.iter().map(|(_, nr)| (*nr, vec![])).collect(), SeccompAction::KillProcess)?;

    // 2. EPERM.
    let mut denied: BTreeMap<i64, Vec<SeccompRule>> = DENIED.iter().map(|(_, nr)| (*nr, vec![])).collect();
    // clone: one rule per namespace flag — "this bit is set" is all a rule can say.
    denied.insert(
        libc::SYS_clone,
        (0..64)
            .map(|bit| 1u64 << bit)
            .filter(|bit| CLONE_NAMESPACE_BITS & bit != 0)
            .map(|bit| rule(0, Len::Qword, Op::MaskedEq(bit), bit))
            .collect::<Result<_>>()?,
    );
    // ioctl: the kernel reads the request as a 32-bit value.
    denied.insert(
        libc::SYS_ioctl,
        FORBIDDEN_IOCTLS.iter().map(|request| rule(1, Len::Dword, Op::Eq, *request)).collect::<Result<_>>()?,
    );
    let denied = compile(denied, SeccompAction::Errno(libc::EPERM as u32))?;

    // 3. ENOSYS: clone3, and — checked before anything else in this program — every x32
    // system-call number.
    let stmt = |code: u32, k: u32| sock_filter { code: code as u16, jt: 0, jf: 0, k };
    let mut absent = vec![
        stmt(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0), // seccomp_data.nr
        sock_filter { code: (libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K) as u16, jt: 0, jf: 1, k: X32_SYSCALL_BIT },
        stmt(libc::BPF_RET | libc::BPF_K, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
    ];
    // Jumps in a BPF program are relative, so a compiled program can follow as it is.
    absent.extend(compile([(libc::SYS_clone3, vec![])].into(), SeccompAction::Errno(libc::ENOSYS as u32))?);

    Ok(vec![tripwire, denied, absent])
}

/// Install on **every thread** of the process. Each program is installed atomically for all
/// threads or not at all; if a later one fails, the earlier ones stay (the result is then an
/// error, and the caller decides what a partly filtered process is worth).
pub fn install() -> Result<()> {
    for program in build()? {
        seccompiler::apply_filter_all_threads(&program).map_err(|e| seccomp_err("installing on all threads", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_compiles_and_starts_with_the_x32_guard() {
        let programs = build().unwrap();
        assert_eq!(programs.len(), 3);
        for program in &programs {
            assert!(!program.is_empty() && program.len() < 4096, "BPF program length {}", program.len());
        }
        let guard = &programs[2][..3];
        assert_eq!((guard[0].code, guard[0].k), (0x20, 0), "load the system-call number");
        assert_eq!((guard[1].code, guard[1].k, guard[1].jt, guard[1].jf), (0x35, 0x4000_0000, 0, 1));
        assert_eq!((guard[2].code, guard[2].k), (0x06, 0x0005_0000 | libc::ENOSYS as u32), "x32: ENOSYS");
    }

    #[test]
    fn nothing_a_supervisor_needs_is_on_the_lists() {
        // What the launcher still does once confined: relay (sockets, epoll), write the
        // settings back and remove its scratch directory (files), signal and reap the
        // sandbox, start threads, resolve names, wipe and lock token memory.
        let needed = [
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_openat,
            libc::SYS_close,
            libc::SYS_unlinkat,
            libc::SYS_renameat2,
            libc::SYS_rename,
            libc::SYS_mkdirat,
            libc::SYS_fchmodat,
            libc::SYS_fchmodat2,
            libc::SYS_getdents64,
            libc::SYS_fsync,
            libc::SYS_socket,
            libc::SYS_connect,
            libc::SYS_accept4,
            libc::SYS_sendto,
            libc::SYS_recvfrom,
            libc::SYS_sendmmsg,
            libc::SYS_epoll_wait,
            libc::SYS_epoll_ctl,
            libc::SYS_futex,
            libc::SYS_kill,
            libc::SYS_wait4,
            libc::SYS_waitid,
            libc::SYS_pidfd_open,
            libc::SYS_pidfd_send_signal,
            libc::SYS_mmap,
            libc::SYS_mprotect,
            libc::SYS_munmap,
            libc::SYS_madvise,
            libc::SYS_mlock,
            libc::SYS_memfd_secret,
            libc::SYS_prctl,
            libc::SYS_getrandom,
            libc::SYS_exit_group,
            libc::SYS_rt_sigaction,
            libc::SYS_fcntl,
            libc::SYS_flock,
            libc::SYS_seccomp,
        ];
        let listed: Vec<i64> = DENIED.iter().chain(TRIPWIRE).map(|(_, nr)| *nr).chain([libc::SYS_clone3]).collect();
        for nr in needed {
            assert!(!listed.contains(&nr), "system call {nr} is needed after confinement");
        }
        // clone and ioctl are only denied with particular arguments.
        assert!(!listed.contains(&libc::SYS_clone) && !listed.contains(&libc::SYS_ioctl));
        // The reason the filter exists.
        for name in ["keyctl", "add_key", "request_key", "execve", "execveat"] {
            assert!(DENIED.iter().any(|(n, _)| *n == name), "{name}");
        }
        // No call is on both lists (the verdicts would differ).
        for (name, nr) in DENIED {
            assert!(!TRIPWIRE.iter().any(|(_, t)| t == nr), "{name} is denied twice");
        }
    }
}
