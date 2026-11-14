//! seccomp-bpf policy for the JVM.
//!
//! Two stacked filters (the kernel takes the most restrictive verdict):
//!
//! 1. **Tripwire** — a short list of syscalls that no JVM, driver or game ever issues but
//!    exploit chains do (`ptrace`, `process_vm_*`, `bpf`, `kexec_*`, module loading, the whole
//!    mount API, `setns`, …). Verdict: `KILL_PROCESS`. The launcher reports the SIGSYS death
//!    as a probable compromise.
//!
//! 2. **Allowlist** — everything not explicitly allowed fails with `ENOSYS`. This is an
//!    allowlist rather than the denylist the requirements sketched, for two concrete reasons
//!    found on the target system:
//!      * the Arch kernel is built with `CONFIG_X86_X32_ABI=y`. A denylist keyed on x86-64
//!        syscall numbers is bypassed by issuing the same call with the x32 bit set
//!        (`nr | 0x40000000`). Under an allowlist those numbers simply are not on the list.
//!      * syscalls added by future kernels are denied until someone reviews them.
//!
//!    `ENOSYS` (not `EPERM`) is deliberate: glibc and HotSpot treat it as "kernel too old" and
//!    fall back, which is exactly what must happen for `clone3` (its flags live behind a
//!    pointer the filter cannot inspect, so thread creation must use `clone`).
//!
//! Explicitly denied because the requirements name them or because they defeat other layers:
//! `io_uring_*`, `userfaultfd`, `perf_event_open`, `unshare`, `keyctl`/`add_key`/`request_key`
//! (kernel keyrings are not namespaced — without this the sandbox could read the refresh
//! token), `open_by_handle_at`, `pidfd_getfd`, `seccomp`, `landlock_*`, every set*id call,
//! `mknod`, `splice`/`vmsplice`/`tee`, legacy AIO, SysV/POSIX message queues.
//!
//! Argument-filtered: `clone` (no namespace flags), `socket` (AF_UNIX/AF_INET/AF_INET6 only —
//! no netlink, packet, AF_ALG, vsock, …), `socketpair` (AF_UNIX), `ioctl` (no TIOCSTI,
//! TIOCLINUX, TIOCCONS, TIOCSETD), `prctl` (fixed option list), `kcmp` (KCMP_FILE only),
//! `personality` (query only).

use std::collections::BTreeMap;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition as Cond, SeccompFilter,
    SeccompRule, TargetArch,
};

use super::policy::SeccompPolicy;
use crate::{
    config::SeccompMode,
    error::{Error, Result},
};

macro_rules! sys {
    ($($name:ident),* $(,)?) => { &[ $( (stringify!($name), libc::$name as i64) ),* ] };
}

/// Syscalls allowed without argument inspection.
#[rustfmt::skip]
const ALLOW: &[(&str, i64)] = sys![
    // files and directories
    SYS_read, SYS_write, SYS_open, SYS_openat, SYS_openat2, SYS_close, SYS_close_range, SYS_creat,
    SYS_stat, SYS_fstat, SYS_lstat, SYS_newfstatat, SYS_statx, SYS_lseek, SYS_pread64, SYS_pwrite64,
    SYS_readv, SYS_writev, SYS_preadv, SYS_pwritev, SYS_preadv2, SYS_pwritev2, SYS_access,
    SYS_faccessat, SYS_faccessat2, SYS_readlink, SYS_readlinkat, SYS_getdents, SYS_getdents64,
    SYS_getcwd, SYS_chdir, SYS_fchdir, SYS_rename, SYS_renameat, SYS_renameat2, SYS_mkdir, SYS_mkdirat,
    SYS_rmdir, SYS_unlink, SYS_unlinkat, SYS_link, SYS_linkat, SYS_chmod, SYS_fchmod, SYS_fchmodat,
    SYS_fchmodat2, SYS_chown, SYS_fchown, SYS_lchown, SYS_fchownat, SYS_umask, SYS_truncate,
    SYS_ftruncate, SYS_fallocate, SYS_fsync, SYS_fdatasync, SYS_sync, SYS_syncfs, SYS_sync_file_range,
    SYS_utime, SYS_utimes, SYS_utimensat, SYS_futimesat, SYS_fadvise64, SYS_readahead, SYS_sendfile,
    SYS_copy_file_range, SYS_dup, SYS_dup2, SYS_dup3, SYS_fcntl, SYS_flock, SYS_statfs, SYS_fstatfs,
    SYS_pipe, SYS_pipe2, SYS_getxattr, SYS_lgetxattr, SYS_fgetxattr, SYS_listxattr, SYS_llistxattr,
    SYS_flistxattr,
    // event notification
    SYS_poll, SYS_ppoll, SYS_select, SYS_pselect6, SYS_epoll_create, SYS_epoll_create1, SYS_epoll_ctl,
    SYS_epoll_wait, SYS_epoll_pwait, SYS_epoll_pwait2, SYS_eventfd, SYS_eventfd2, SYS_signalfd,
    SYS_signalfd4, SYS_timerfd_create, SYS_timerfd_settime, SYS_timerfd_gettime, SYS_inotify_init,
    SYS_inotify_init1, SYS_inotify_add_watch, SYS_inotify_rm_watch,
    // memory (a JIT needs mmap/mprotect with PROT_EXEC; W^X cannot be enforced on a JVM)
    SYS_mmap, SYS_munmap, SYS_mprotect, SYS_mremap, SYS_brk, SYS_madvise, SYS_mincore, SYS_msync,
    SYS_mlock, SYS_mlock2, SYS_munlock, SYS_mlockall, SYS_munlockall, SYS_membarrier, SYS_mbind,
    SYS_set_mempolicy, SYS_get_mempolicy, SYS_memfd_create, SYS_pkey_alloc, SYS_pkey_free,
    SYS_pkey_mprotect, SYS_mseal,
    // processes and threads
    SYS_fork, SYS_vfork, SYS_execve, SYS_execveat, SYS_exit, SYS_exit_group, SYS_wait4, SYS_waitid,
    SYS_getpid, SYS_getppid, SYS_gettid, SYS_getpgrp, SYS_getpgid, SYS_getsid, SYS_setsid, SYS_setpgid,
    SYS_getuid, SYS_geteuid, SYS_getgid, SYS_getegid, SYS_getgroups, SYS_getresuid, SYS_getresgid,
    SYS_set_tid_address, SYS_set_robust_list, SYS_get_robust_list, SYS_futex, SYS_futex_waitv,
    SYS_sched_yield, SYS_sched_getaffinity, SYS_sched_setaffinity, SYS_sched_getparam,
    SYS_sched_setparam, SYS_sched_getscheduler, SYS_sched_setscheduler, SYS_sched_get_priority_max,
    SYS_sched_get_priority_min, SYS_sched_rr_get_interval, SYS_sched_getattr, SYS_sched_setattr,
    SYS_getpriority, SYS_setpriority, SYS_ioprio_get, SYS_ioprio_set, SYS_getrlimit, SYS_setrlimit,
    SYS_prlimit64, SYS_getrusage, SYS_times, SYS_uname, SYS_sysinfo, SYS_getcpu, SYS_arch_prctl,
    SYS_rseq, SYS_capget, SYS_pidfd_open, SYS_pidfd_send_signal,
    // signals and timers
    SYS_kill, SYS_tkill, SYS_tgkill, SYS_rt_sigaction, SYS_rt_sigprocmask, SYS_rt_sigreturn,
    SYS_rt_sigpending, SYS_rt_sigtimedwait, SYS_rt_sigqueueinfo, SYS_rt_tgsigqueueinfo,
    SYS_rt_sigsuspend, SYS_sigaltstack, SYS_pause, SYS_alarm, SYS_getitimer, SYS_setitimer,
    SYS_timer_create, SYS_timer_settime, SYS_timer_gettime, SYS_timer_getoverrun, SYS_timer_delete,
    SYS_nanosleep, SYS_clock_nanosleep, SYS_clock_gettime, SYS_clock_getres, SYS_gettimeofday, SYS_time,
    SYS_restart_syscall, SYS_getrandom,
    // sockets (creation is argument-filtered below)
    SYS_bind, SYS_listen, SYS_accept, SYS_accept4, SYS_connect, SYS_getsockname, SYS_getpeername,
    SYS_sendto, SYS_recvfrom, SYS_sendmsg, SYS_recvmsg, SYS_sendmmsg, SYS_recvmmsg, SYS_shutdown,
    SYS_setsockopt, SYS_getsockopt,
    // SysV shm/sem inside the private IPC namespace (ALSA dmix, MIT-SHM probes)
    SYS_shmget, SYS_shmat, SYS_shmdt, SYS_shmctl, SYS_semget, SYS_semop, SYS_semtimedop, SYS_semctl,
];

/// Newer syscalls the `libc` crate has no constant for yet (x86-64 numbers).
const ALLOW_NUMERIC: &[(&str, i64)] = &[
    ("SYS_uretprobe", 335),        // only reachable from the kernel's own uprobe trampoline
    ("SYS_map_shadow_stack", 453), // CET shadow stacks for new threads
    ("SYS_futex_wake", 454),
    ("SYS_futex_wait", 455),
    ("SYS_futex_requeue", 456),
];

/// Never legitimate for the game; reaching one of these means hostile code is running.
/// (The confined launcher's own filter, `confine::filter`, kills on the same list.)
#[rustfmt::skip]
pub(crate) const TRIPWIRE: &[(&str, i64)] = sys![
    SYS_ptrace, SYS_process_vm_readv, SYS_process_vm_writev, SYS_bpf, SYS_kexec_load,
    SYS_kexec_file_load, SYS_init_module, SYS_finit_module, SYS_delete_module, SYS_mount, SYS_umount2,
    SYS_pivot_root, SYS_chroot, SYS_open_tree, SYS_move_mount, SYS_fsopen, SYS_fsconfig, SYS_fsmount,
    SYS_fspick, SYS_mount_setattr, SYS_setns, SYS_open_by_handle_at, SYS_reboot, SYS_swapon,
    SYS_swapoff, SYS_acct, SYS_iopl, SYS_ioperm,
];

/// Known syscalls that are denied (ENOSYS). Listed so `sandbox.extra_syscalls` can name them
/// and so the tests can assert they stay denied.
#[rustfmt::skip]
const DENIED_NAMED: &[(&str, i64)] = sys![
    SYS_io_uring_setup, SYS_io_uring_enter, SYS_io_uring_register, SYS_userfaultfd, SYS_perf_event_open,
    SYS_unshare, SYS_clone3, SYS_keyctl, SYS_add_key, SYS_request_key, SYS_pidfd_getfd, SYS_seccomp,
    SYS_landlock_create_ruleset, SYS_landlock_add_rule, SYS_landlock_restrict_self, SYS_setuid,
    SYS_setgid, SYS_setreuid, SYS_setregid, SYS_setresuid, SYS_setresgid, SYS_setgroups, SYS_setfsuid,
    SYS_setfsgid, SYS_capset, SYS_mknod, SYS_mknodat, SYS_symlink, SYS_symlinkat, SYS_splice,
    SYS_vmsplice, SYS_tee, SYS_io_setup, SYS_io_submit, SYS_io_getevents, SYS_io_destroy, SYS_io_cancel,
    SYS_mq_open, SYS_mq_unlink, SYS_msgget, SYS_msgsnd, SYS_msgrcv, SYS_msgctl, SYS_name_to_handle_at,
    SYS_fanotify_init, SYS_fanotify_mark, SYS_syslog, SYS_sethostname, SYS_setdomainname,
    SYS_settimeofday, SYS_clock_settime, SYS_clock_adjtime, SYS_adjtimex, SYS_modify_ldt, SYS_quotactl,
    SYS_setxattr, SYS_lsetxattr, SYS_fsetxattr, SYS_removexattr, SYS_lremovexattr, SYS_fremovexattr,
    SYS_remap_file_pages, SYS_migrate_pages, SYS_move_pages, SYS_process_madvise, SYS_process_mrelease,
    SYS_memfd_secret, SYS_vhangup, SYS_lookup_dcookie,
];

/// Allowed only with the argument checks built in `allowlist_rules`.
const FILTERED: &[(&str, i64)] =
    sys![SYS_clone, SYS_socket, SYS_socketpair, SYS_ioctl, SYS_prctl, SYS_kcmp, SYS_personality];

const AF_UNIX: u64 = libc::AF_UNIX as u64;
const AF_INET: u64 = libc::AF_INET as u64;
const AF_INET6: u64 = libc::AF_INET6 as u64;

/// Every CLONE_NEW* bit that `clone(2)` accepts.
pub(crate) const CLONE_NAMESPACE_BITS: u64 = (libc::CLONE_NEWNS
    | libc::CLONE_NEWCGROUP
    | libc::CLONE_NEWUTS
    | libc::CLONE_NEWIPC
    | libc::CLONE_NEWUSER
    | libc::CLONE_NEWPID
    | libc::CLONE_NEWNET) as u64;

/// Terminal ioctls that inject input or hijack a console.
pub(crate) const FORBIDDEN_IOCTLS: &[u64] = &[
    0x5412, // TIOCSTI: push characters into a terminal's input queue
    0x541C, // TIOCLINUX: virtual-console selection/paste
    0x541D, // TIOCCONS: redirect console output
    0x5423, // TIOCSETD: change line discipline
];

const ALLOWED_PRCTL: &[u64] = &[
    libc::PR_SET_NAME as u64,
    libc::PR_GET_NAME as u64,
    libc::PR_SET_PDEATHSIG as u64,
    libc::PR_GET_PDEATHSIG as u64,
    libc::PR_GET_DUMPABLE as u64,
    libc::PR_SET_DUMPABLE as u64,
    libc::PR_GET_KEEPCAPS as u64,
    libc::PR_SET_TIMERSLACK as u64,
    libc::PR_GET_TIMERSLACK as u64,
    libc::PR_SET_NO_NEW_PRIVS as u64,
    libc::PR_GET_NO_NEW_PRIVS as u64,
    libc::PR_GET_SECCOMP as u64,
    libc::PR_CAPBSET_READ as u64,
    libc::PR_SET_THP_DISABLE as u64,
    libc::PR_GET_THP_DISABLE as u64,
    libc::PR_SET_CHILD_SUBREAPER as u64,
    libc::PR_GET_CHILD_SUBREAPER as u64,
    libc::PR_GET_SPECULATION_CTRL as u64,
    0x5356_4d41, // PR_SET_VMA (name anonymous mappings)
    65,          // PR_SET_MDWE
    66,          // PR_GET_MDWE
    0x4155_5856, // PR_GET_AUXV
];

fn cond(arg: u8, len: Len, op: Op, value: u64) -> Result<Cond> {
    Cond::new(arg, len, op, value).map_err(|e| Error::Sandbox(format!("seccomp condition: {e}")))
}

fn rule(conds: Vec<Cond>) -> Result<SeccompRule> {
    SeccompRule::new(conds).map_err(|e| Error::Sandbox(format!("seccomp rule: {e}")))
}

/// Resolve a syscall name (`"openat"` or `"SYS_openat"`) from any of the tables.
pub fn syscall_number(name: &str) -> Option<i64> {
    let bare = name.strip_prefix("SYS_").unwrap_or(name);
    [ALLOW, ALLOW_NUMERIC, TRIPWIRE, DENIED_NAMED, FILTERED]
        .iter()
        .flat_map(|t| t.iter())
        .find(|(n, _)| n.strip_prefix("SYS_") == Some(bare))
        .map(|(_, nr)| *nr)
}

fn allowlist_rules(policy: &SeccompPolicy) -> Result<BTreeMap<i64, Vec<SeccompRule>>> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for (_, nr) in ALLOW.iter().chain(ALLOW_NUMERIC) {
        rules.insert(*nr, vec![]);
    }

    // clone: threads and processes yes, namespaces no.
    rules.insert(libc::SYS_clone, vec![rule(vec![cond(0, Len::Qword, Op::MaskedEq(CLONE_NAMESPACE_BITS), 0)?])?]);

    // socket: only the three families a game client needs.
    let families = [AF_UNIX, AF_INET, AF_INET6];
    rules.insert(
        libc::SYS_socket,
        families.iter().map(|f| rule(vec![cond(0, Len::Dword, Op::Eq, *f)?])).collect::<Result<_>>()?,
    );
    rules.insert(libc::SYS_socketpair, vec![rule(vec![cond(0, Len::Dword, Op::Eq, AF_UNIX)?])?]);

    // ioctl: the kernel reads the request as a 32-bit value, so compare 32 bits.
    rules.insert(
        libc::SYS_ioctl,
        vec![rule(FORBIDDEN_IOCTLS.iter().map(|c| cond(1, Len::Dword, Op::Ne, *c)).collect::<Result<_>>()?)?],
    );

    rules.insert(
        libc::SYS_prctl,
        ALLOWED_PRCTL.iter().map(|o| rule(vec![cond(0, Len::Dword, Op::Eq, *o)?])).collect::<Result<_>>()?,
    );

    // kcmp(…, KCMP_FILE, …): Mesa compares DRM file descriptions with it.
    rules.insert(libc::SYS_kcmp, vec![rule(vec![cond(2, Len::Dword, Op::Eq, 0)?])?]);
    // personality(0xffffffff) only queries.
    rules.insert(libc::SYS_personality, vec![rule(vec![cond(0, Len::Dword, Op::Eq, 0xffff_ffff)?])?]);

    for name in &policy.extra_allow {
        let nr = syscall_number(name)
            .ok_or_else(|| Error::Config(format!("sandbox.extra_syscalls: unknown syscall {name:?}")))?;
        if TRIPWIRE.iter().any(|(_, t)| *t == nr) {
            return Err(Error::Config(format!(
                "sandbox.extra_syscalls: {name:?} is on the tripwire list and cannot be allowed"
            )));
        }
        // An unconditional rule here would silently *replace* the argument checks (naming
        // `ioctl` would re-enable TIOCSTI, `clone` namespace flags, `socket` netlink, …).
        if FILTERED.iter().any(|(_, f)| *f == nr) {
            return Err(Error::Config(format!(
                "sandbox.extra_syscalls: {name:?} is already allowed with argument checks and cannot be widened"
            )));
        }
        rules.insert(nr, vec![]);
    }
    Ok(rules)
}

fn compile(
    rules: BTreeMap<i64, Vec<SeccompRule>>,
    mismatch: SeccompAction,
    matched: SeccompAction,
) -> Result<BpfProgram> {
    SeccompFilter::new(rules, mismatch, matched, TargetArch::x86_64)
        .and_then(BpfProgram::try_from)
        .map_err(|e| Error::Sandbox(format!("compiling seccomp filter: {e}")))
}

/// Build the filters in installation order.
pub fn build(policy: &SeccompPolicy) -> Result<Vec<BpfProgram>> {
    let mut programs = Vec::new();
    match policy.mode {
        SeccompMode::Enforce => {
            if policy.tripwire {
                let rules = TRIPWIRE.iter().map(|(_, nr)| (*nr, vec![])).collect();
                programs.push(compile(rules, SeccompAction::Allow, SeccompAction::KillProcess)?);
            }
            // Must be last: once installed, `seccomp(2)` itself is no longer permitted.
            programs.push(compile(
                allowlist_rules(policy)?,
                SeccompAction::Errno(libc::ENOSYS as u32),
                SeccompAction::Allow,
            )?);
        }
        // Diagnostics: violations are logged by the kernel (audit/journal) and then allowed.
        SeccompMode::Log => programs.push(compile(allowlist_rules(policy)?, SeccompAction::Log, SeccompAction::Allow)?),
    }
    Ok(programs)
}

/// Install on the calling thread. The caller must be single-threaded (the init stage is).
pub fn install(programs: &[BpfProgram]) -> Result<()> {
    for program in programs {
        seccompiler::apply_filter(program).map_err(|e| Error::Sandbox(format!("installing seccomp filter: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_compile_and_fit() {
        let programs = build(&SeccompPolicy::default()).unwrap();
        assert_eq!(programs.len(), 2);
        for p in &programs {
            assert!(!p.is_empty() && p.len() < 4096, "BPF program length {}", p.len());
        }
        let log = build(&SeccompPolicy { mode: SeccompMode::Log, ..Default::default() }).unwrap();
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn tables_are_disjoint_and_dangerous_calls_are_not_allowed() {
        let allowed: Vec<i64> = ALLOW.iter().chain(ALLOW_NUMERIC).map(|(_, n)| *n).collect();
        for (name, nr) in TRIPWIRE {
            assert!(!allowed.contains(nr), "{name} is both allowed and on the tripwire");
        }
        for name in [
            "ptrace",
            "bpf",
            "mount",
            "kexec_load",
            "io_uring_setup",
            "userfaultfd",
            "keyctl",
            "add_key",
            "request_key",
            "unshare",
            "setns",
            "process_vm_readv",
            "perf_event_open",
            "open_by_handle_at",
            "pidfd_getfd",
            "clone3",
            "seccomp",
            "init_module",
            "pivot_root",
        ] {
            let nr = syscall_number(name).unwrap_or_else(|| panic!("{name} missing from tables"));
            assert!(!allowed.contains(&nr), "{name} must not be on the allowlist");
        }
        // Spot-check numbers against the x86-64 table.
        assert_eq!(syscall_number("read"), Some(0));
        assert_eq!(syscall_number("SYS_openat"), Some(257));
        assert_eq!(syscall_number("ptrace"), Some(101));
        assert_eq!(syscall_number("io_uring_setup"), Some(425));
        assert_eq!(syscall_number("no_such_call"), None);
    }

    #[test]
    fn extra_syscalls_cannot_reopen_the_tripwire() {
        let ok = SeccompPolicy { extra_allow: vec!["splice".into()], ..Default::default() };
        assert!(build(&ok).is_ok());
        let bad = SeccompPolicy { extra_allow: vec!["ptrace".into()], ..Default::default() };
        assert!(matches!(build(&bad), Err(Error::Config(_))));
        let unknown = SeccompPolicy { extra_allow: vec!["frobnicate".into()], ..Default::default() };
        assert!(build(&unknown).is_err());
        // Naming an argument-filtered syscall must not strip its filter.
        for filtered in ["ioctl", "clone", "socket", "prctl", "socketpair", "kcmp", "personality"] {
            let p = SeccompPolicy { extra_allow: vec![filtered.into()], ..Default::default() };
            assert!(matches!(build(&p), Err(Error::Config(_))), "{filtered}");
        }
    }
}
