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
    if let Some(ports) = &policy.udp_send {
        restrict_udp(ports)?;
    }
    Ok(status.ruleset)
}

/// First Landlock ABI with UDP port rules (Linux 7.2).
pub const UDP_ABI: i32 = 10;

/// A second, stacked ruleset for UDP: datagrams only to `send_ports`, binding only to
/// kernel-chosen ephemeral ports (what an unbound socket gets when it first sends).
///
/// Written against the system call interface directly: the `landlock` crate knows the ABI
/// up to version 9, and UDP rules arrived with version 10. Returns whether the kernel has
/// them; without, nothing is changed (the network namespace remains the barrier, as before).
pub fn restrict_udp(send_ports: &[u16]) -> Result<bool> {
    const BIND_UDP: u64 = 1 << 2;
    const CONNECT_SEND_UDP: u64 = 1 << 3;
    const RULE_NET_PORT: libc::c_int = 2;
    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }
    #[repr(C)]
    struct NetPortAttr {
        allowed_access: u64,
        port: u64,
    }
    if kernel_abi() < UDP_ABI {
        return Ok(false);
    }
    let os = |what: &str| landlock_err(what, std::io::Error::last_os_error());
    let attr = RulesetAttr { handled_access_fs: 0, handled_access_net: BIND_UDP | CONNECT_SEND_UDP, scoped: 0 };
    // SAFETY: `attr` is a valid, initialised structure of the size passed; flags are 0.
    let fd = unsafe {
        libc::syscall(libc::SYS_landlock_create_ruleset, &attr as *const RulesetAttr, size_of::<RulesetAttr>(), 0u32)
    };
    if fd < 0 {
        return Err(os("creating the UDP ruleset"));
    }
    // SAFETY: a descriptor just returned by the kernel, owned by nothing else.
    let ruleset = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd as i32) };
    let raw = std::os::fd::AsRawFd::as_raw_fd(&ruleset);
    let rules = std::iter::once(NetPortAttr { allowed_access: BIND_UDP, port: 0 })
        .chain(send_ports.iter().map(|p| NetPortAttr { allowed_access: CONNECT_SEND_UDP, port: u64::from(*p) }));
    for rule in rules {
        // SAFETY: `rule` is a valid landlock_net_port_attr; `raw` is a ruleset descriptor.
        if unsafe { libc::syscall(libc::SYS_landlock_add_rule, raw, RULE_NET_PORT, &rule as *const NetPortAttr, 0u32) }
            != 0
        {
            return Err(os(&format!("allowing UDP port {}", rule.port)));
        }
    }
    // SAFETY: restricts the calling thread; no_new_privs is already set by the caller.
    if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, raw, 0u32) } != 0 {
        return Err(os("enforcing the UDP ruleset"));
    }
    Ok(true)
}
