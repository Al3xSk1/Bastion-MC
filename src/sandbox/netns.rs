//! Private network namespace for "filtered" mode.
//!
//! The helper (`bastion-mc __netns-exec …`, a fresh single-threaded process) does this:
//!
//! 1. `unshare(CLONE_NEWUSER | CLONE_NEWNET)` — it now holds every capability *over this new
//!    network namespace only*.
//! 2. Brings `lo` up. The whole of 127.0.0.0/8 is then local.
//! 3. Adds each allowlisted literal server IP as an extra address on `lo`, so a connection to
//!    it is delivered locally instead of being unroutable.
//! 4. Binds a wildcard TCP listener per allowlisted port (including 443, hence the need for
//!    capabilities) and a UDP socket on 127.0.0.1:53.
//! 5. Sends all of those sockets to the launcher over a UNIX socket (`SCM_RIGHTS`) and closes
//!    its own copies.
//! 6. `execv`s bubblewrap. The exec drops every capability (non-root UID, no ambient set), and
//!    bubblewrap then nests its own user namespace, so the game is two user namespaces away
//!    from any capability over this network namespace.
//!
//! Result: the namespace has no route to anywhere. A packet can leave the machine only if the
//! launcher — which holds the listening ends in the host namespace — decides to relay it.
//! There is no veth, no NAT, no slirp4netns, no setuid helper and no firewall rule to get wrong.

use std::{
    convert::Infallible,
    ffi::{CString, OsString},
    fs::File,
    io::{IoSlice, IoSliceMut, Read},
    net::{Ipv4Addr, TcpListener, UdpSocket},
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
    time::Duration,
};

use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, recvmsg, sendmsg};
use serde::{Deserialize, Serialize};

use crate::error::{Context, Error, Result};

pub const NETNS_ARG: &str = "__netns-exec";
const MAX_FDS_PER_MESSAGE: usize = 128;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetnsSpec {
    /// Wildcard TCP listeners to create, one per port.
    pub tcp_ports: Vec<u16>,
    /// Create the UDP socket for the in-sandbox DNS responder (127.0.0.1:53).
    pub dns: bool,
    /// Literal IPv4 destinations to capture.
    pub local_addrs: Vec<Ipv4Addr>,
}

impl NetnsSpec {
    /// Number of sockets the helper will hand over, in order: DNS (if any), then TCP ports.
    pub fn socket_count(&self) -> usize {
        self.tcp_ports.len() + usize::from(self.dns)
    }
}

/// Entry point for `bastion-mc __netns-exec <spec-fd> <ctrl-fd> <program> [args…]`.
pub fn main(args: &[OsString]) -> ! {
    let err = match run(args) {
        Ok(never) => match never {},
        Err(e) => e,
    };
    eprintln!("bastion-mc network namespace helper: {err}");
    // SAFETY: immediate process exit.
    unsafe { libc::_exit(125) }
}

fn parse_fd(arg: Option<&OsString>) -> Result<RawFd> {
    arg.and_then(|a| a.to_str())
        .and_then(|a| a.parse().ok())
        .filter(|fd| *fd >= 3)
        .ok_or_else(|| Error::Sandbox("bad descriptor argument".into()))
}

fn run(args: &[OsString]) -> Result<Infallible> {
    let spec_fd = parse_fd(args.first())?;
    let ctrl_fd = parse_fd(args.get(1))?;
    let program = args.get(2).ok_or_else(|| Error::Sandbox("missing program".into()))?;

    // SAFETY: both descriptors were passed to us to own.
    let (spec_file, ctrl) = unsafe { (File::from_raw_fd(spec_fd), OwnedFd::from_raw_fd(ctrl_fd)) };
    let mut raw = Vec::new();
    spec_file.take(1 << 20).read_to_end(&mut raw).ctx(|| "reading namespace spec")?;
    let spec: NetnsSpec =
        serde_json::from_slice(&raw).map_err(|e| Error::Sandbox(format!("malformed namespace spec: {e}")))?;

    let (uid, gid) = (nix::unistd::getuid().as_raw(), nix::unistd::getgid().as_raw());
    // SAFETY: plain syscall; this process is single-threaded (required for CLONE_NEWUSER).
    if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) } != 0 {
        return Err(Error::io(
            "creating user+network namespace (are unprivileged user namespaces enabled?)",
            std::io::Error::last_os_error(),
        ));
    }
    // Identity mapping: we stay ourselves, we just gain capabilities over the new namespaces.
    std::fs::write("/proc/self/setgroups", "deny").ctx(|| "writing setgroups")?;
    std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1")).ctx(|| "writing uid_map")?;
    std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1")).ctx(|| "writing gid_map")?;

    configure_loopback(&spec.local_addrs)?;

    let mut sockets: Vec<OwnedFd> = Vec::with_capacity(spec.socket_count());
    if spec.dns {
        sockets.push(UdpSocket::bind((Ipv4Addr::LOCALHOST, 53)).ctx(|| "binding DNS socket 127.0.0.1:53")?.into());
    }
    for port in &spec.tcp_ports {
        sockets.push(
            TcpListener::bind((Ipv4Addr::UNSPECIFIED, *port))
                .ctx(|| format!("binding TCP listener on port {port}"))?
                .into(),
        );
    }
    send_fds(ctrl.as_raw_fd(), &sockets)?;
    drop(sockets);
    drop(ctrl);

    let exec = CString::new(program.as_bytes()).map_err(|_| Error::Sandbox("program path contains NUL".into()))?;
    let argv: Vec<CString> = args[2..]
        .iter()
        .map(|a| CString::new(a.as_bytes()).map_err(|_| Error::Sandbox("argument contains NUL".into())))
        .collect::<Result<_>>()?;
    // Our UID is not 0 and no ambient capabilities are set, so this exec drops every
    // capability we held over the namespaces.
    nix::unistd::execv(&exec, &argv).ctx(|| format!("executing {}", program.to_string_lossy()))
}

fn ifreq(name: &str) -> libc::ifreq {
    // SAFETY: ifreq is plain old data; all-zero is a valid value.
    let mut req: libc::ifreq = unsafe { std::mem::zeroed() };
    for (dst, src) in req.ifr_name.iter_mut().zip(name.bytes().take(libc::IFNAMSIZ - 1)) {
        *dst = src as libc::c_char;
    }
    req
}

fn sockaddr(ip: Ipv4Addr) -> libc::sockaddr {
    let sin = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr { s_addr: u32::from(ip).to_be() },
        sin_zero: [0; 8],
    };
    // SAFETY: sockaddr_in and sockaddr have the same size and alignment requirements here.
    unsafe { std::mem::transmute::<libc::sockaddr_in, libc::sockaddr>(sin) }
}

fn ioctl(sock: &UdpSocket, request: libc::c_ulong, req: &mut libc::ifreq, what: &str) -> Result<()> {
    // SAFETY: `req` is a valid, initialised ifreq for the duration of the call.
    if unsafe { libc::ioctl(sock.as_raw_fd(), request as _, req as *mut libc::ifreq) } != 0 {
        return Err(Error::io(format!("configuring loopback ({what})"), std::io::Error::last_os_error()));
    }
    Ok(())
}

fn configure_loopback(local_addrs: &[Ipv4Addr]) -> Result<()> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ctx(|| "opening configuration socket")?;

    // Bringing `lo` up makes the kernel assign 127.0.0.1/8 on its own.
    let mut req = ifreq("lo");
    ioctl(&sock, libc::SIOCGIFFLAGS, &mut req, "get flags")?;
    // SAFETY: SIOCGIFFLAGS filled the flags member of the union.
    unsafe { req.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short };
    ioctl(&sock, libc::SIOCSIFFLAGS, &mut req, "set up")?;

    for (i, ip) in local_addrs.iter().enumerate() {
        let label = format!("lo:{}", i + 1);
        let mut req = ifreq(&label);
        req.ifr_ifru.ifru_addr = sockaddr(*ip);
        ioctl(&sock, libc::SIOCSIFADDR, &mut req, "add address")?;
        let mut req = ifreq(&label);
        req.ifr_ifru.ifru_netmask = sockaddr(Ipv4Addr::BROADCAST);
        ioctl(&sock, libc::SIOCSIFNETMASK, &mut req, "set netmask")?;
    }
    Ok(())
}

fn send_fds(ctrl: RawFd, fds: &[OwnedFd]) -> Result<()> {
    let raw: Vec<RawFd> = fds.iter().map(|f| f.as_raw_fd()).collect();
    for chunk in raw.chunks(MAX_FDS_PER_MESSAGE) {
        let iov = [IoSlice::new(b"F")];
        let cmsg = [ControlMessage::ScmRights(chunk)];
        sendmsg::<()>(ctrl, &iov, &cmsg, MsgFlags::empty(), None).ctx(|| "passing sockets to the launcher")?;
    }
    Ok(())
}

/// Launcher side: collect exactly `expected` sockets from the helper, or fail.
pub fn recv_fds(ctrl: BorrowedFd<'_>, expected: usize, wait: Duration) -> Result<Vec<OwnedFd>> {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    let mut out: Vec<OwnedFd> = Vec::with_capacity(expected);
    let deadline = std::time::Instant::now() + wait;
    while out.len() < expected {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let timeout = PollTimeout::try_from(left.as_millis().min(i32::MAX as u128) as i32).unwrap_or(PollTimeout::ZERO);
        let mut pfd = [PollFd::new(ctrl, PollFlags::POLLIN)];
        if poll(&mut pfd, timeout).ctx(|| "waiting for the namespace helper")? == 0 {
            return Err(Error::Sandbox("timed out waiting for the network namespace helper".into()));
        }
        let mut byte = [0u8; 1];
        let mut iov = [IoSliceMut::new(&mut byte)];
        let mut space = nix::cmsg_space!([RawFd; MAX_FDS_PER_MESSAGE]);
        let msg = recvmsg::<()>(ctrl.as_raw_fd(), &mut iov, Some(&mut space), MsgFlags::MSG_CMSG_CLOEXEC)
            .ctx(|| "receiving sockets from the namespace helper")?;
        if msg.bytes == 0 {
            return Err(Error::Sandbox(
                "the network namespace helper exited before handing over its sockets (see its message above); \
                 filtered networking needs unprivileged user namespaces"
                    .into(),
            ));
        }
        for cmsg in msg.cmsgs().ctx(|| "decoding control message")? {
            if let ControlMessageOwned::ScmRights(fds) = cmsg {
                // SAFETY: the kernel just installed these descriptors for us; we are the owner.
                out.extend(fds.into_iter().map(|fd| unsafe { OwnedFd::from_raw_fd(fd) }));
            }
        }
    }
    if out.len() != expected {
        return Err(Error::Sandbox(format!("namespace helper sent {} sockets, expected {expected}", out.len())));
    }
    Ok(out)
}
