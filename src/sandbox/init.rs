//! Second stage: runs *inside* the bubblewrap sandbox, as the same binary, immediately
//! before the JVM. Landlock cannot be applied earlier (a Landlocked process may not mount, so
//! bwrap could not build the sandbox), and seccomp must come last (it forbids the syscalls
//! the earlier steps need).
//!
//! Order matters and every step fails closed — the JVM is never executed half-sandboxed:
//!
//! 1. `no_new_privs`
//! 2. join a fresh, empty session keyring (the user's keyring would otherwise be inherited
//!    and readable: kernel keyrings are not covered by any namespace)
//! 3. `RLIMIT_CORE = 1`: the kernel aborts core dumps itself, even with a piped handler
//! 4. speculation-control hardening (best effort)
//! 5. Landlock
//! 6. seccomp tripwire, then seccomp allowlist
//! 7. close every descriptor except stdin/stdout/stderr
//! 8. `execve` the JVM with exactly the environment from the policy

use std::{
    convert::Infallible,
    ffi::{CString, OsString},
    fs::File,
    io::Read,
    os::{
        fd::{FromRawFd, RawFd},
        unix::ffi::OsStrExt,
    },
};

use nix::sys::resource::{Resource, setrlimit};

use super::{landlock, policy::InitPolicy, seccomp};
use crate::error::{Context, Error, Result};

pub const INIT_ARG: &str = "__sandbox-init";
const MAX_POLICY_BYTES: u64 = 8 << 20;

/// Entry point for `bastion-mc __sandbox-init <policy-fd>`. Never returns.
pub fn main(args: &[OsString]) -> ! {
    let err = match run(args) {
        Ok(never) => match never {},
        Err(e) => e,
    };
    // stderr is the launcher's log pipe.
    eprintln!("bastion-mc sandbox init: {err}");
    eprintln!("bastion-mc sandbox init: refusing to start the JVM without a complete sandbox");
    // SAFETY: immediate process exit, no destructors needed.
    unsafe { libc::_exit(126) }
}

fn run(args: &[OsString]) -> Result<Infallible> {
    let fd: RawFd = args
        .first()
        .and_then(|a| a.to_str())
        .and_then(|a| a.parse().ok())
        .filter(|fd| *fd >= 3)
        .ok_or_else(|| Error::Sandbox("missing policy descriptor".into()))?;
    // SAFETY: the launcher passed this descriptor for us to own; nothing else uses it.
    let file = unsafe { File::from_raw_fd(fd) };
    let mut raw = Vec::new();
    file.take(MAX_POLICY_BYTES).read_to_end(&mut raw).ctx(|| "reading sandbox policy")?;
    let policy: InitPolicy =
        serde_json::from_slice(&raw).map_err(|e| Error::Sandbox(format!("malformed sandbox policy: {e}")))?;

    nix::sys::prctl::set_no_new_privs().ctx(|| "setting no_new_privs")?;
    detach_keyrings()?;
    setrlimit(Resource::RLIMIT_CORE, 1, 1).ctx(|| "disabling core dumps")?;
    harden_speculation();

    landlock::apply(&policy.landlock)?;

    let filters = seccomp::build(&policy.seccomp)?;
    // Prepare everything that allocates or opens files before the filter is active.
    let exec = CString::new(policy.exec.as_os_str().as_bytes())
        .map_err(|_| Error::Sandbox("program path contains NUL".into()))?;
    let argv: Vec<CString> = policy
        .argv
        .iter()
        .map(|a| CString::new(a.as_bytes()).map_err(|_| Error::Sandbox("argument contains NUL".into())))
        .collect::<Result<_>>()?;
    let env: Vec<CString> = policy
        .env
        .iter()
        .map(|(k, v)| {
            if k.is_empty() || k.contains('=') {
                return Err(Error::Sandbox(format!("invalid environment variable name {k:?}")));
            }
            CString::new(format!("{k}={v}")).map_err(|_| Error::Sandbox("environment contains NUL".into()))
        })
        .collect::<Result<_>>()?;

    seccomp::install(&filters)?;

    // Nothing but stdio crosses into the JVM: not the policy, not a bind-mount source, not
    // anything the launcher's parent leaked.
    // SAFETY: plain syscall; closes descriptors 3 and above in this process.
    if unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) } != 0 {
        return Err(Error::io("closing inherited descriptors", std::io::Error::last_os_error()));
    }

    nix::unistd::execve(&exec, &argv, &env).ctx(|| format!("executing {}", policy.exec.display()))
}

/// Replace the inherited session keyring with a new anonymous one.
///
/// Without this, `keyctl(KEYCTL_READ)` from inside a fully namespaced sandbox reads keys in
/// the user's session keyring and, through its link, the user keyring — which is where the
/// Microsoft refresh token lives. (seccomp also denies the keyring syscalls; this is the
/// layer that still holds if the filter is ever loosened.)
fn detach_keyrings() -> Result<()> {
    const KEYCTL_JOIN_SESSION_KEYRING: libc::c_int = 1;
    // SAFETY: keyctl(KEYCTL_JOIN_SESSION_KEYRING, NULL) creates and joins an anonymous keyring.
    let rc = unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_JOIN_SESSION_KEYRING, std::ptr::null::<libc::c_char>()) };
    if rc < 0 {
        return Err(Error::io("joining a fresh session keyring", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Ask the kernel to keep Spectre v2/v4 mitigations forced on for this process and its
/// descendants: the sandbox runs code we do not trust next to processes we do. Unsupported
/// CPUs/kernels return an error, which is fine.
fn harden_speculation() {
    const PR_SET_SPECULATION_CTRL: libc::c_int = 53;
    const PR_SPEC_STORE_BYPASS: libc::c_ulong = 0;
    const PR_SPEC_INDIRECT_BRANCH: libc::c_ulong = 1;
    const PR_SPEC_FORCE_DISABLE: libc::c_ulong = 1 << 3;
    for which in [PR_SPEC_STORE_BYPASS, PR_SPEC_INDIRECT_BRANCH] {
        // SAFETY: prctl with integer arguments only.
        unsafe { libc::prctl(PR_SET_SPECULATION_CTRL, which, PR_SPEC_FORCE_DISABLE, 0, 0) };
    }
}
