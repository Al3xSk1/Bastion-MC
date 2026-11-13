//! Landlock: a second, independent access-control layer inside the bubblewrap namespace.
//!
//! bubblewrap decides what *exists* in the sandbox; Landlock decides what the process may
//! *do* with it, and keeps doing so even if a mount-namespace trick, a leaked descriptor or a
//! bwrap misconfiguration exposed more than intended. It also adds controls namespaces do not
//! have: no `execve` outside the JVM, no `ioctl` on device nodes other than GPU/sound, TCP
//! restricted to allowlisted ports, no signals or abstract-socket connections to anything
//! outside the sandbox, and (kernel ≥ 7.1) no connecting to UNIX sockets other than the
//! display and audio sockets.

use std::path::Path;

use landlock::{
    ABI, Access, AccessFs, AccessNet, BitFlags, CompatLevel, Compatible, NetPort, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope, make_bitflags,
};

use super::policy::{LandlockPolicy, TcpPolicy};
use crate::error::{Error, Result};

/// Highest ABI this code was written and tested against. Rights the running kernel lacks are
/// dropped best-effort, but never below [`MIN_KERNEL_ABI`].
const TARGET_ABI: ABI = ABI::V9;

/// ABI 6 (Linux 6.12) brings signal and abstract-socket scoping; with ABI 5's device-ioctl
/// control and ABI 4's TCP rules below it, that is the floor for calling this a sandbox.
pub const MIN_KERNEL_ABI: i32 = 6;

/// Landlock ABI version of the running kernel (0 or negative: unavailable).
pub fn kernel_abi() -> i32 {
    const LANDLOCK_CREATE_RULESET_VERSION: libc::c_ulong = 1;
    // SAFETY: documented probe form of landlock_create_ruleset: NULL attr, size 0, VERSION flag.
    let v = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    v as i32
}

fn fs_read() -> BitFlags<AccessFs> {
    make_bitflags!(AccessFs::{ReadFile | ReadDir})
}

fn fs_write() -> BitFlags<AccessFs> {
    // Deliberately absent: MakeSym, MakeSock, MakeFifo, MakeChar, MakeBlock, Execute, IoctlDev.
    fs_read() | make_bitflags!(AccessFs::{WriteFile | Truncate | MakeReg | MakeDir | RemoveFile | RemoveDir | Refer})
}

fn landlock_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Sandbox(format!("Landlock: {what}: {e}"))
}

/// Restrict the calling thread (and everything it later executes). Fails closed.
pub fn apply(policy: &LandlockPolicy) -> Result<RulesetStatus> {
    let abi = kernel_abi();
    if abi < MIN_KERNEL_ABI {
        return Err(Error::Sandbox(format!(
            "kernel Landlock ABI is {abi}; at least {MIN_KERNEL_ABI} (Linux 6.12) is required. \
             Is `landlock` in the `lsm=` list?"
        )));
    }

    let mut scopes: BitFlags<Scope> = Scope::Signal.into();
    if policy.scope_abstract_unix {
        scopes |= Scope::AbstractUnixSocket;
    }
    let net: BitFlags<AccessNet> = match policy.tcp {
        TcpPolicy::Ports(_) => AccessNet::BindTcp | AccessNet::ConnectTcp,
        TcpPolicy::AnyConnect => AccessNet::BindTcp.into(),
    };

    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(TARGET_ABI))
        .map_err(|e| landlock_err("handling filesystem access", e))?
        .handle_access(net)
        .map_err(|e| landlock_err("handling network access", e))?
        .scope(scopes)
        .map_err(|e| landlock_err("scoping", e))?
        .create()
        .map_err(|e| landlock_err("creating ruleset", e))?;

    let file_rights = AccessFs::from_file(TARGET_ABI);
    let mut grant = |path: &Path, access: BitFlags<AccessFs>, optional: bool| -> Result<()> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) if optional => return Ok(()),
            Err(e) => return Err(landlock_err(&format!("policy path {}", path.display()), e)),
        };
        // Directory-only rights on a non-directory are an error to the kernel.
        let access = if meta.is_dir() { access } else { access & file_rights };
        let fd = PathFd::new(path).map_err(|e| landlock_err(&format!("opening {}", path.display()), e))?;
        (&mut ruleset)
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|e| landlock_err(&format!("adding rule for {}", path.display()), e))?;
        Ok(())
    };

    for p in &policy.read {
        grant(p, fs_read(), false)?;
    }
    for p in &policy.read_if_present {
        grant(p, fs_read(), true)?;
    }
    for p in &policy.read_exec {
        grant(p, fs_read() | AccessFs::Execute, false)?;
    }
    for p in &policy.write {
        grant(p, fs_write(), false)?;
    }
    for p in &policy.devices {
        grant(p, make_bitflags!(AccessFs::{ReadFile | WriteFile | ReadDir | IoctlDev}), false)?;
    }
    for p in &policy.plain_devices {
        grant(p, make_bitflags!(AccessFs::{ReadFile | WriteFile | ReadDir}), false)?;
    }
    for p in &policy.unix_sockets {
        grant(p, make_bitflags!(AccessFs::{ReadFile | WriteFile | ResolveUnix}), false)?;
    }
    for p in &policy.socket_dirs {
        grant(p, fs_write() | make_bitflags!(AccessFs::{MakeSock | ResolveUnix}), false)?;
    }
    if let TcpPolicy::Ports(ports) = &policy.tcp {
        for port in ports {
            (&mut ruleset)
                .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
                .map_err(|e| landlock_err(&format!("allowing TCP port {port}"), e))?;
        }
    }

    let status = ruleset.restrict_self().map_err(|e| landlock_err("restrict_self", e))?;
    if status.ruleset == RulesetStatus::NotEnforced || !status.no_new_privs {
        return Err(Error::Sandbox("Landlock ruleset was not enforced by the kernel".into()));
    }
    Ok(status.ruleset)
}
