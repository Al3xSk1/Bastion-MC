//! The dialer: one small process, started before the launcher confines itself, that opens the
//! TCP connection to an approved Realm's server.
//!
//! The confined launcher may connect only to the ports it knew before step 9 (Landlock); a
//! Realm's address — and its port — is known only when Realms answers a join, long after. So the
//! launcher starts this helper beforehand and, for each approved join, asks it for one
//! connection and receives the connected socket (`SCM_RIGHTS`); the launcher then relays the
//! game's bytes over it as it relays any game connection. The launcher's own confinement is not
//! widened.
//!
//! The dialer is this program again (`bastion-mc __realms-dial`), in a sandbox of its own built
//! like the game's (bubblewrap, a fresh user, PID, IPC and UTS namespace, no files of yours, a
//! read-only `/usr`, Landlock, the system-call filter), but with the host's network: it exists
//! to connect out. What it does is fixed: read a request of 20 bytes from its standard input (a
//! UNIX socket to the launcher), refuse any address that is not public (loopback, private, link
//! local, multicast, …), connect within 10 s, hand the socket back — at most
//! [`MAX_DIALS`] times, then it refuses everything. It never binds, sends no UDP, resolves no
//! names, and ends when the launcher does.
//!
//! What remains: a launcher that was taken over could use the dialer for up to [`MAX_DIALS`]
//! connections to public addresses on ports its own confinement would not allow. Without the
//! dialer it could not; without Realms approvals there is no dialer.

use std::{
    ffi::OsString,
    io::{IoSlice, IoSliceMut, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixStream,
    },
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, recvmsg, sendmsg};

use crate::{
    error::{Error, Result},
    sandbox::{
        self, BaseOptions, INIT_PATH, Network, Resolv, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
};

pub const DIAL_ARG: &str = "__realms-dial";
/// Test support only: also connect to loopback addresses (the stand-in Realm of the tests).
const ALLOW_LOOPBACK: &str = "--allow-loopback";
/// Connections one dialer makes, whatever it is asked.
pub const MAX_DIALS: u32 = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_LEN: usize = 20;
const VERSION: u8 = 1;

/// Why the dialer said no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Status {
    Connected = 0,
    NotPublic = 1,
    Exhausted = 2,
    Failed = 3,
    Malformed = 4,
}

fn encode(addr: SocketAddr) -> [u8; REQUEST_LEN] {
    let mut out = [0u8; REQUEST_LEN];
    out[0] = VERSION;
    match addr.ip() {
        IpAddr::V4(v4) => {
            out[1] = 4;
            out[2..6].copy_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            out[1] = 6;
            out[2..18].copy_from_slice(&v6.octets());
        }
    }
    out[18..20].copy_from_slice(&addr.port().to_be_bytes());
    out
}

fn decode(request: &[u8; REQUEST_LEN]) -> Option<SocketAddr> {
    if request[0] != VERSION {
        return None;
    }
    let port = u16::from_be_bytes([request[18], request[19]]);
    let ip: IpAddr = match request[1] {
        4 if request[6..18].iter().all(|b| *b == 0) => {
            Ipv4Addr::new(request[2], request[3], request[4], request[5]).into()
        }
        6 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&request[2..18]);
            Ipv6Addr::from(octets).into()
        }
        _ => return None,
    };
    (port != 0).then_some(SocketAddr::new(ip, port))
}

/// May the dialer connect there? Public addresses only (the relay's own rule, `egress::is_public`);
/// loopback as well only in the tests.
fn permitted(addr: &SocketAddr, allow_loopback: bool) -> bool {
    crate::egress::is_public(addr.ip()) || (allow_loopback && addr.ip().is_loopback())
}

// ---------------------------------------------------------------- the helper

/// Entry point of `bastion-mc __realms-dial`, inside its sandbox. Never returns.
pub fn main(args: &[OsString]) -> ! {
    let allow_loopback = args.iter().any(|a| a == ALLOW_LOOPBACK);
    let code = match serve(allow_loopback) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("bastion-mc dialer: {e}");
            1
        }
    };
    // SAFETY: immediate process exit; nothing needs to run.
    unsafe { libc::_exit(code) }
}

fn serve(allow_loopback: bool) -> std::io::Result<()> {
    // SAFETY: descriptor 0 is the socket to the launcher; this process owns it from here on.
    let mut channel = unsafe { UnixStream::from_raw_fd(0) };
    let mut dials = 0u32;
    loop {
        let mut request = [0u8; REQUEST_LEN];
        match channel.read_exact(&mut request) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let (status, errno, stream) = match decode(&request) {
            None => (Status::Malformed, 0, None),
            Some(_) if dials >= MAX_DIALS => (Status::Exhausted, 0, None),
            Some(addr) if !permitted(&addr, allow_loopback) => (Status::NotPublic, 0, None),
            Some(addr) => {
                dials += 1;
                match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                    Ok(stream) => (Status::Connected, 0, Some(stream)),
                    Err(e) => (Status::Failed, e.raw_os_error().unwrap_or(0), None),
                }
            }
        };
        let mut reply = [0u8; 5];
        reply[0] = status as u8;
        reply[1..5].copy_from_slice(&(errno as u32).to_be_bytes());
        let iov = [IoSlice::new(&reply)];
        match &stream {
            Some(stream) => {
                let fds = [stream.as_raw_fd()];
                sendmsg::<()>(channel.as_raw_fd(), &iov, &[ControlMessage::ScmRights(&fds)], MsgFlags::empty(), None)?;
            }
            None => {
                channel.write_all(&reply)?;
            }
        }
        drop(stream); // the launcher has its own copy now
    }
}

// ---------------------------------------------------------------- the launcher's side

/// The running dialer, as the launcher holds it. Asking it for a connection blocks a thread of
/// the blocking pool, never the async runtime.
pub struct Dialer {
    channel: Arc<Mutex<UnixStream>>,
    /// The sandbox; dropping it ends the dialer.
    _sandbox: Mutex<sandbox::Running>,
}

impl Dialer {
    /// Start the dialer. `program` is this program (the sandbox's init stage and the dialer).
    /// `log` receives what the dialer prints (it prints only why it failed).
    pub fn start(program: &Path, log: Arc<dyn Fn(String) + Send + Sync>) -> Result<Dialer> {
        Self::start_with(program, log, false)
    }

    /// [`Dialer::start`] for the tests, whose stand-in Realm listens on the loopback address.
    #[doc(hidden)]
    pub fn start_allowing_loopback(program: &Path, log: Arc<dyn Fn(String) + Send + Sync>) -> Result<Dialer> {
        Self::start_with(program, log, true)
    }

    fn start_with(program: &Path, log: Arc<dyn Fn(String) + Send + Sync>, allow_loopback: bool) -> Result<Dialer> {
        let exec = [std::path::PathBuf::from(INIT_PATH)];
        let base = sandbox::base(&BaseOptions {
            tmp_size_mb: 16,
            jvm: None,
            exec_allow: &exec,
            resolv: Resolv::None,
            expose_timezone: false,
            trust: None,
        })?;
        let mut landlock = base.landlock;
        landlock.tcp = TcpPolicy::AnyConnect; // bind stays denied
        landlock.udp_send = Some(Vec::new()); // no UDP at all, not even DNS: it is given addresses
        landlock.write.clear(); // it writes no file anywhere
        let mut argv = vec!["bastion-mc-dialer".to_string(), DIAL_ARG.to_string()];
        if allow_loopback {
            argv.push(ALLOW_LOOPBACK.into());
        }
        let spec = SandboxSpec {
            mounts: base.mounts,
            network: Network::Open,
            policy: InitPolicy {
                exec: INIT_PATH.into(),
                argv,
                env: vec![("PATH".into(), "/usr/bin".into())],
                landlock,
                seccomp: SeccompPolicy::default(),
            },
            cwd: "/tmp".into(),
            limits: None,
        };
        let mut running = sandbox::spawn(spec, program)?;
        let channel = running.channel.try_clone().map_err(|e| Error::io("duplicating the dialer channel", e))?;
        channel.set_read_timeout(Some(REPLY_TIMEOUT)).map_err(|e| Error::io("configuring the dialer channel", e))?;
        if let Some(out) = running.child.stdout.take() {
            drain(out, log.clone());
        }
        if let Some(err) = running.child.stderr.take() {
            drain(err, log);
        }
        Ok(Dialer { channel: Arc::new(Mutex::new(channel)), _sandbox: Mutex::new(running) })
    }

    /// One connection to `addr`, made by the dialer.
    pub async fn dial(&self, addr: SocketAddr) -> std::result::Result<tokio::net::TcpStream, String> {
        let channel = self.channel.clone();
        let stream = tokio::task::spawn_blocking(move || {
            let mut channel = channel.lock().map_err(|_| "the dialer is unusable".to_string())?;
            ask(&mut channel, addr)
        })
        .await
        .map_err(|_| "the dialer request was abandoned".to_string())??;
        stream.set_nonblocking(true).map_err(|e| format!("configuring the Realm connection: {e}"))?;
        let stream =
            tokio::net::TcpStream::from_std(stream).map_err(|e| format!("registering the Realm connection: {e}"))?;
        let _ = stream.set_nodelay(true);
        Ok(stream)
    }
}

/// What the dialer prints (only why it failed) goes to the launch log.
fn drain(pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static, log: Arc<dyn Fn(String) + Send + Sync>) {
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log(format!("realms dialer: {}", crate::util::sanitize_line(line.as_bytes(), 300)));
        }
    });
}

/// Send one request and receive the answer (and the socket) on `channel`.
fn ask(channel: &mut UnixStream, addr: SocketAddr) -> std::result::Result<TcpStream, String> {
    channel.write_all(&encode(addr)).map_err(|e| format!("the dialer is gone ({e})"))?;
    let mut reply = [0u8; 5];
    let mut space = nix::cmsg_space!([RawFd; 1]);
    let mut iov = [IoSliceMut::new(&mut reply)];
    let message = recvmsg::<()>(channel.as_raw_fd(), &mut iov, Some(&mut space), MsgFlags::MSG_CMSG_CLOEXEC)
        .map_err(|e| format!("no answer from the dialer ({e})"))?;
    let mut fd: Option<OwnedFd> = None;
    for cmsg in message.cmsgs().map_err(|e| format!("a malformed answer from the dialer ({e})"))? {
        if let ControlMessageOwned::ScmRights(fds) = cmsg {
            for raw in fds {
                // SAFETY: the kernel just installed this descriptor in our table for us.
                let owned = unsafe { OwnedFd::from_raw_fd(raw) };
                if fd.is_none() {
                    fd = Some(owned);
                }
            }
        }
    }
    let received = message.bytes;
    if received != reply.len() {
        return Err("a short answer from the dialer".into());
    }
    let errno = u32::from_be_bytes([reply[1], reply[2], reply[3], reply[4]]);
    match (reply[0], fd) {
        (0, Some(fd)) => {
            let stream = TcpStream::from(fd);
            match stream.peer_addr() {
                Ok(peer) if peer == addr => Ok(stream),
                Ok(peer) => Err(format!("the dialer connected to {peer}, not to {addr}")),
                Err(e) => Err(format!("the dialer's socket is not connected ({e})")),
            }
        }
        (1, _) => Err(format!("{addr} is not a public address; the dialer does not connect there")),
        (2, _) => Err(format!("the dialer has made its {MAX_DIALS} connections for this session")),
        (3, _) => Err(format!("cannot connect to {addr} ({})", std::io::Error::from_raw_os_error(errno as i32))),
        _ => Err("an unexpected answer from the dialer".into()),
    }
}

/// For the fuzz targets: the request decoder never accepts what the encoder would not produce.
#[doc(hidden)]
pub fn fuzz_request(data: &[u8]) {
    if data.len() < REQUEST_LEN {
        return;
    }
    let mut request = [0u8; REQUEST_LEN];
    request.copy_from_slice(&data[..REQUEST_LEN]);
    if let Some(addr) = decode(&request) {
        assert_eq!(encode(addr), request, "decode accepted a request encode would not produce");
        assert_ne!(addr.port(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_encode_and_decode_exactly() {
        for addr in ["203.0.113.9:31234", "[2001:db8::1]:25565", "1.2.3.4:1"] {
            let addr: SocketAddr = addr.parse().unwrap();
            assert_eq!(decode(&encode(addr)), Some(addr));
        }
        let mut bad = encode("203.0.113.9:1".parse().unwrap());
        bad[0] = 2;
        assert_eq!(decode(&bad), None, "unknown version");
        let mut bad = encode("203.0.113.9:1".parse().unwrap());
        bad[1] = 5;
        assert_eq!(decode(&bad), None, "unknown family");
        let mut bad = encode("203.0.113.9:1".parse().unwrap());
        bad[10] = 1;
        assert_eq!(decode(&bad), None, "stray bytes after an IPv4 address");
        assert_eq!(decode(&encode("203.0.113.9:0".parse().unwrap())), None, "port 0");
    }

    #[test]
    fn only_public_addresses_are_permitted() {
        for ok in ["203.0.113.9:1", "8.8.8.8:25565", "[2606:4700::1]:443"] {
            assert!(permitted(&ok.parse().unwrap(), false), "{ok}");
        }
        for no in [
            "127.0.0.1:1",
            "10.0.0.1:1",
            "192.168.1.1:1",
            "169.254.1.1:1",
            "100.64.0.1:1",
            "[::1]:1",
            "[fe80::1]:1",
            "0.0.0.0:1",
        ] {
            assert!(!permitted(&no.parse().unwrap(), false), "{no}");
        }
        assert!(permitted(&"127.0.0.1:1".parse().unwrap(), true), "the tests' loopback stand-in");
        assert!(!permitted(&"10.0.0.1:1".parse().unwrap(), true), "but nothing else private");
    }
}
