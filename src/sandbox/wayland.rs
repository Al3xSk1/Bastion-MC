//! `wp_security_context_v1`: telling the Wayland compositor that the game is sandboxed.
//!
//! Wayland isolates clients from each other, but a compositor still offers every client
//! everything it implements — on wlroots-style compositors that includes screen capture,
//! virtual keyboards, clipboard managers and layer surfaces, which are meant for trusted
//! desktop components. A compositor cannot know that a connection comes from a sandbox
//! unless it is told.
//!
//! This protocol is how it is told. Before the sandbox starts, the launcher — a trusted
//! client on the ordinary socket — creates a *new* listening socket, hands it to the
//! compositor together with a description ("sandbox engine", application and instance id),
//! and from then on every connection accepted on that socket carries that security context.
//! The game gets only the new socket. Compositors that implement the protocol withhold their
//! privileged interfaces from such connections, and refuse to let them create further
//! contexts.
//!
//! The setup is a dozen requests, so it is spoken directly on the wire instead of pulling a
//! Wayland client library into the launcher: connect, list globals, bind the manager, pass
//! two descriptors, set three strings, commit, and wait for the compositor to confirm.
//!
//! Lifetime: the compositor keeps accepting on the socket until the launcher's end of a
//! pipe closes — that is, exactly as long as the launcher lives.

use std::{
    io::{IoSlice, Read},
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use nix::sys::socket::{ControlMessage, MsgFlags, sendmsg};

use crate::error::{Context, Error, Result};

const MANAGER: &str = "wp_security_context_manager_v1";
/// Identifies this launcher to the compositor (reverse-DNS in form, as the protocol asks;
/// it is a name, not a claim to a domain).
pub const SANDBOX_ENGINE: &str = "org.bastion-mc";
pub const APP_ID: &str = "minecraft";

const TIMEOUT: Duration = Duration::from_secs(5);
/// libwayland never sends a message longer than this.
const MAX_MESSAGE: usize = 4096;
const MAX_GLOBALS: usize = 512;

// Object ids used on the setup connection (the client allocates them).
const DISPLAY: u32 = 1;
const REGISTRY: u32 = 2;
const FIRST_SYNC: u32 = 3;
const MANAGER_ID: u32 = 4;
const CONTEXT: u32 = 5;
const LAST_SYNC: u32 = 6;

/// A global the compositor advertises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    pub name: u32,
    pub interface: String,
    pub version: u32,
}

/// The game's own Wayland socket, with a security context attached to it.
pub struct SecurityContext {
    /// Bind this into the sandbox instead of the compositor's socket.
    pub socket: PathBuf,
    /// While this is open the compositor accepts connections on the socket.
    _alive: OwnedFd,
}

impl SecurityContext {
    /// Remove the socket's name once the sandbox has bound it. The compositor holds the
    /// listening descriptor and the sandbox holds the mount; nothing needs the path.
    pub fn unlink(&self) {
        let _ = std::fs::remove_file(&self.socket);
        if let Some(dir) = self.socket.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

impl Drop for SecurityContext {
    fn drop(&mut self) {
        self.unlink();
    }
}

pub enum Setup {
    Secured(SecurityContext),
    /// The compositor does not implement the protocol.
    Unsupported,
}

fn wl(e: impl std::fmt::Display) -> Error {
    Error::Sandbox(format!("Wayland security context: {e}"))
}

// ---------------------------------------------------------------- wire format

/// One request: header (object, size and opcode) plus arguments, in host byte order.
struct Message(Vec<u8>);

impl Message {
    fn new(object: u32, opcode: u16) -> Self {
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(&object.to_ne_bytes());
        bytes.extend_from_slice(&u32::from(opcode).to_ne_bytes()); // size filled in by `finish`
        Message(bytes)
    }
    fn uint(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_ne_bytes());
        self
    }
    /// A string: length including the terminating NUL, the bytes, padding to four bytes.
    fn string(mut self, value: &str) -> Self {
        let len = value.len() + 1;
        self.0.extend_from_slice(&(len as u32).to_ne_bytes());
        self.0.extend_from_slice(value.as_bytes());
        self.0.resize(self.0.len() + 1 + (4 - len % 4) % 4, 0);
        self
    }
    fn finish(mut self) -> Vec<u8> {
        let size = self.0.len() as u32;
        let opcode = u32::from_ne_bytes(self.0[4..8].try_into().expect("header"));
        self.0[4..8].copy_from_slice(&((size << 16) | opcode).to_ne_bytes());
        self.0
    }
}

struct Event<'a> {
    object: u32,
    opcode: u16,
    args: &'a [u8],
}

fn take_uint(args: &mut &[u8]) -> Option<u32> {
    let (head, tail) = args.split_first_chunk::<4>()?;
    *args = tail;
    Some(u32::from_ne_bytes(*head))
}

fn take_string(args: &mut &[u8]) -> Option<String> {
    let len = take_uint(args)? as usize;
    let padded = len.checked_add(3)? & !3;
    if len == 0 || padded > args.len() {
        return None;
    }
    let text = std::str::from_utf8(&args[..len - 1]).ok()?.to_string();
    *args = &args[padded..];
    Some(text)
}

struct Connection {
    stream: UnixStream,
    buffer: Vec<u8>,
}

impl Connection {
    fn open(compositor: &Path) -> Result<Self> {
        let stream = UnixStream::connect(compositor)
            .ctx(|| format!("connecting to the compositor at {}", compositor.display()))?;
        stream.set_read_timeout(Some(TIMEOUT)).ctx(|| "configuring the compositor connection")?;
        stream.set_write_timeout(Some(TIMEOUT)).ctx(|| "configuring the compositor connection")?;
        Ok(Connection { stream, buffer: Vec::new() })
    }

    fn send(&mut self, message: Message, fds: &[&OwnedFd]) -> Result<()> {
        let bytes = message.finish();
        let raw: Vec<i32> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
        let control = [ControlMessage::ScmRights(&raw)];
        let sent = sendmsg::<()>(
            self.stream.as_raw_fd(),
            &[IoSlice::new(&bytes)],
            if raw.is_empty() { &[] } else { &control },
            MsgFlags::MSG_NOSIGNAL,
            None,
        )
        .map_err(|e| wl(format!("sending to the compositor: {e}")))?;
        // A dozen short messages into an empty socket buffer: a partial write would mean the
        // compositor is not reading at all.
        if sent != bytes.len() { Err(wl("the compositor is not reading")) } else { Ok(()) }
    }

    /// Read events until `until` says stop. A `wl_display.error` ends everything.
    fn pump(&mut self, mut until: impl FnMut(&Event<'_>) -> Result<bool>) -> Result<()> {
        loop {
            while self.buffer.len() >= 8 {
                let object = u32::from_ne_bytes(self.buffer[0..4].try_into().expect("header"));
                let word = u32::from_ne_bytes(self.buffer[4..8].try_into().expect("header"));
                let (size, opcode) = ((word >> 16) as usize, (word & 0xFFFF) as u16);
                if !(8..=MAX_MESSAGE).contains(&size) || size % 4 != 0 {
                    return Err(wl("the compositor sent a malformed message"));
                }
                if self.buffer.len() < size {
                    break;
                }
                let message: Vec<u8> = self.buffer.drain(..size).collect();
                let event = Event { object, opcode, args: &message[8..] };
                if object == DISPLAY && opcode == 0 {
                    // wl_display.error(object, code, message)
                    let mut args = event.args;
                    let (on, code) = (take_uint(&mut args).unwrap_or(0), take_uint(&mut args).unwrap_or(0));
                    let text = take_string(&mut args).unwrap_or_default();
                    return Err(wl(format!(
                        "the compositor reported protocol error {code} on object {on}: {}",
                        crate::util::sanitize_line(text.as_bytes(), 200)
                    )));
                }
                if until(&event)? {
                    return Ok(());
                }
            }
            let mut chunk = [0u8; 4096];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(wl("the compositor closed the connection")),
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(wl(format!("no answer from the compositor ({e})"))),
            }
            if self.buffer.len() > 64 * MAX_MESSAGE {
                return Err(wl("the compositor sent more than a setup connection should receive"));
            }
        }
    }

    /// `wl_display.get_registry` + `wl_display.sync`, then every `wl_registry.global` up to
    /// the sync's `done`.
    fn globals(&mut self) -> Result<Vec<Global>> {
        self.send(Message::new(DISPLAY, 1).uint(REGISTRY), &[])?;
        self.send(Message::new(DISPLAY, 0).uint(FIRST_SYNC), &[])?;
        let mut globals = Vec::new();
        self.pump(|event| {
            if event.object == REGISTRY && event.opcode == 0 {
                let mut args = event.args;
                let (name, interface, version) = (take_uint(&mut args), take_string(&mut args), take_uint(&mut args));
                let (Some(name), Some(interface), Some(version)) = (name, interface, version) else {
                    return Err(wl("the compositor sent a malformed global"));
                };
                if globals.len() >= MAX_GLOBALS {
                    return Err(wl("the compositor advertises an implausible number of globals"));
                }
                globals.push(Global { name, interface, version });
            }
            Ok(event.object == FIRST_SYNC && event.opcode == 0)
        })?;
        Ok(globals)
    }
}

/// What the compositor behind `socket` offers a client connecting there.
pub fn globals(socket: &Path) -> Result<Vec<Global>> {
    Connection::open(socket)?.globals()
}

/// Create a listening socket in `dir` (which must be private to this user) and register it
/// with the compositor at `compositor` as a sandboxed endpoint for `instance`.
pub fn secure(compositor: &Path, dir: &Path, instance: &str) -> Result<Setup> {
    let mut connection = Connection::open(compositor)?;
    let Some(manager) = connection.globals()?.into_iter().find(|g| g.interface == MANAGER) else {
        return Ok(Setup::Unsupported);
    };

    crate::util::create_private_dir_all(dir)?;
    let socket = dir.join("wayland-0");
    let _ = std::fs::remove_file(&socket);
    // bind(2) and listen(2) must both have happened before the descriptor is handed over.
    let listener = UnixListener::bind(&socket).ctx(|| format!("creating {}", socket.display()))?;
    let listener = OwnedFd::from(listener);
    // The compositor watches the read end; it reports hang-up when our write end closes.
    let (hangup, alive) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).ctx(|| "creating the lifetime pipe")?;

    // wl_registry.bind(name, interface, version, id)
    connection.send(Message::new(REGISTRY, 0).uint(manager.name).string(MANAGER).uint(1).uint(MANAGER_ID), &[])?;
    // wp_security_context_manager_v1.create_listener(id, listen_fd, close_fd)
    connection.send(Message::new(MANAGER_ID, 1).uint(CONTEXT), &[&listener, &hangup])?;
    // wp_security_context_v1: set_sandbox_engine, set_app_id, set_instance_id, commit
    connection.send(Message::new(CONTEXT, 1).string(SANDBOX_ENGINE), &[])?;
    connection.send(Message::new(CONTEXT, 2).string(APP_ID), &[])?;
    connection.send(Message::new(CONTEXT, 3).string(instance), &[])?;
    connection.send(Message::new(CONTEXT, 4), &[])?;
    // A round trip: once the compositor answers this, it has processed (and accepted, or
    // answered with an error) everything above.
    connection.send(Message::new(DISPLAY, 0).uint(LAST_SYNC), &[])?;
    connection.pump(|event| Ok(event.object == LAST_SYNC && event.opcode == 0))?;

    // The compositor has its own copies now.
    drop(listener);
    drop(hangup);
    let _ = alive.as_fd();
    Ok(Setup::Secured(SecurityContext { socket, _alive: alive }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::socket::{ControlMessageOwned, MsgFlags as Flags, recvmsg};
    use std::{io::Write, os::fd::FromRawFd};

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bastion-wl-{tag}-{}", crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        dir
    }

    fn event(object: u32, opcode: u16, args: Message) -> Vec<u8> {
        // Reuse the request encoder: events have the same framing.
        let mut m = Message::new(object, opcode);
        m.0.extend_from_slice(&args.0[8..]);
        m.finish()
    }

    /// What a stand-in compositor saw.
    #[derive(Default, Debug)]
    struct Seen {
        requests: Vec<(u32, u16, Vec<u8>)>,
        fds: Vec<OwnedFd>,
    }

    /// A compositor that announces `interfaces`, answers syncs, and records everything.
    /// `error_on_commit` makes it answer the commit with a protocol error instead.
    fn compositor(
        dir: &Path,
        interfaces: &'static [&'static str],
        error_on_commit: bool,
    ) -> (PathBuf, std::thread::JoinHandle<Seen>) {
        // One socket per stand-in: a test may start several in the same directory.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = dir.join(format!("compositor-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut seen = Seen::default();
            let mut buffer = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let mut space = nix::cmsg_space!([i32; 4]);
                let mut iov = [std::io::IoSliceMut::new(&mut chunk)];
                let Ok(msg) = recvmsg::<()>(stream.as_raw_fd(), &mut iov, Some(&mut space), Flags::empty()) else {
                    break;
                };
                for control in msg.cmsgs().unwrap() {
                    if let ControlMessageOwned::ScmRights(fds) = control {
                        // SAFETY: descriptors just received over the socket; owned from here.
                        seen.fds.extend(fds.into_iter().map(|fd| unsafe { OwnedFd::from_raw_fd(fd) }));
                    }
                }
                let n = msg.bytes;
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
                while buffer.len() >= 8 {
                    let object = u32::from_ne_bytes(buffer[0..4].try_into().unwrap());
                    let word = u32::from_ne_bytes(buffer[4..8].try_into().unwrap());
                    let (size, opcode) = ((word >> 16) as usize, (word & 0xFFFF) as u16);
                    if buffer.len() < size {
                        break;
                    }
                    let message: Vec<u8> = buffer.drain(..size).collect();
                    let args = message[8..].to_vec();
                    let mut reply = Vec::new();
                    match (object, opcode) {
                        (DISPLAY, 1) => {
                            for (i, interface) in interfaces.iter().enumerate() {
                                reply.extend(event(
                                    REGISTRY,
                                    0,
                                    Message::new(0, 0).uint(i as u32 + 1).string(interface).uint(1),
                                ));
                            }
                        }
                        (DISPLAY, 0) => {
                            let id = u32::from_ne_bytes(args[0..4].try_into().unwrap());
                            reply.extend(event(id, 0, Message::new(0, 0).uint(0)));
                        }
                        (CONTEXT, 4) if error_on_commit => {
                            reply.extend(event(
                                DISPLAY,
                                0,
                                Message::new(0, 0).uint(CONTEXT).uint(3).string("invalid metadata"),
                            ));
                        }
                        _ => {}
                    }
                    seen.requests.push((object, opcode, args));
                    if !reply.is_empty() && stream.write_all(&reply).is_err() {
                        return seen;
                    }
                }
            }
            seen
        });
        (path, handle)
    }

    #[test]
    fn strings_are_length_prefixed_nul_terminated_and_padded() {
        let m = Message::new(7, 3).string("abc").uint(9).finish();
        let mut expected = Vec::new();
        expected.extend_from_slice(&7u32.to_ne_bytes());
        expected.extend_from_slice(&((20u32 << 16) | 3).to_ne_bytes());
        expected.extend_from_slice(&4u32.to_ne_bytes());
        expected.extend_from_slice(b"abc\0");
        expected.extend_from_slice(&9u32.to_ne_bytes());
        assert_eq!(m, expected);
        // Lengths that need padding, and the empty string.
        for (text, padded) in [("", 4), ("a", 4), ("abcd", 8), ("abcde", 8)] {
            let m = Message::new(1, 0).string(text).finish();
            assert_eq!(m.len(), 8 + 4 + padded, "{text:?}");
            let mut args = &m[8..];
            assert_eq!(take_string(&mut args).as_deref(), Some(text));
            assert!(args.is_empty());
        }
        // Malformed strings are refused, not read out of bounds.
        assert_eq!(take_string(&mut &[5, 0, 0, 0, b'a'][..]), None);
        assert_eq!(take_string(&mut &0u32.to_ne_bytes()[..]), None);
        assert_eq!(take_string(&mut &u32::MAX.to_ne_bytes()[..]), None);
    }

    #[test]
    fn the_whole_setup_against_a_compositor_that_supports_it() {
        let dir = scratch("ok");
        let (path, handle) = compositor(&dir, &["wl_compositor", "wl_shm", MANAGER, "xdg_wm_base"], false);
        let Setup::Secured(context) = secure(&path, &dir.join("private"), "vanilla-26.3").unwrap() else {
            panic!("the stand-in advertises the manager")
        };
        assert!(std::fs::metadata(&context.socket).is_ok(), "the new socket exists for bubblewrap to bind");
        let socket = context.socket.clone();
        let seen = handle.join().unwrap();

        // The requests, in order, with the right objects, opcodes and arguments.
        let ops: Vec<(u32, u16)> = seen.requests.iter().map(|(o, c, _)| (*o, *c)).collect();
        assert_eq!(
            ops,
            [
                (DISPLAY, 1),
                (DISPLAY, 0),
                (REGISTRY, 0),
                (MANAGER_ID, 1),
                (CONTEXT, 1),
                (CONTEXT, 2),
                (CONTEXT, 3),
                (CONTEXT, 4),
                (DISPLAY, 0)
            ]
        );
        let mut bind = &seen.requests[2].2[..];
        assert_eq!(take_uint(&mut bind), Some(3), "the manager's global name");
        assert_eq!(take_string(&mut bind).as_deref(), Some(MANAGER));
        assert_eq!((take_uint(&mut bind), take_uint(&mut bind)), (Some(1), Some(MANAGER_ID)));
        let text = |i: usize| take_string(&mut &seen.requests[i].2[..]).unwrap();
        assert_eq!(
            (text(4), text(5), text(6)),
            (SANDBOX_ENGINE.to_string(), APP_ID.to_string(), "vanilla-26.3".to_string())
        );

        // Two descriptors went over: a listening socket bound to the path, and a pipe end.
        assert_eq!(seen.fds.len(), 2);
        let listening = UnixListener::from(seen.fds[0].try_clone().unwrap());
        assert_eq!(listening.local_addr().unwrap().as_pathname(), Some(socket.as_path()));
        let client =
            UnixStream::connect(&socket).expect("a client of the new socket reaches the compositor's descriptor");
        let (_accepted, _) = listening.accept().unwrap();
        drop(client);

        // The compositor's pipe end reports hang-up exactly when the launcher's end closes.
        let mut pipe = [nix::poll::PollFd::new(seen.fds[1].as_fd(), nix::poll::PollFlags::POLLIN)];
        assert_eq!(nix::poll::poll(&mut pipe, nix::poll::PollTimeout::ZERO).unwrap(), 0, "still alive");
        context.unlink();
        assert!(!socket.exists(), "the name is gone; the descriptor keeps working");
        assert!(UnixStream::connect(&socket).is_err());
        drop(context);
        let mut pipe = [nix::poll::PollFd::new(seen.fds[1].as_fd(), nix::poll::PollFlags::POLLIN)];
        assert_eq!(nix::poll::poll(&mut pipe, nix::poll::PollTimeout::from(2000u16)).unwrap(), 1);
        assert!(pipe[0].revents().unwrap().contains(nix::poll::PollFlags::POLLHUP));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_compositor_without_the_protocol_is_reported_as_such() {
        let dir = scratch("old");
        let (path, handle) = compositor(&dir, &["wl_compositor", "wl_shm", "xdg_wm_base"], false);
        assert!(matches!(secure(&path, &dir.join("private"), "x").unwrap(), Setup::Unsupported));
        let seen = handle.join().unwrap();
        assert!(seen.fds.is_empty(), "nothing was handed over");
        assert!(!dir.join("private").exists(), "and nothing was created");

        let (path, handle) = compositor(&dir, &["wl_shm", MANAGER], false);
        let listed = globals(&path).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1], Global { name: 2, interface: MANAGER.into(), version: 1 });
        drop(handle);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn protocol_errors_and_silence_are_failures() {
        let dir = scratch("err");
        let (path, handle) = compositor(&dir, &[MANAGER], true);
        let error = match secure(&path, &dir.join("private"), "x") {
            Err(e) => e.to_string(),
            Ok(_) => panic!("the compositor rejected the context"),
        };
        assert!(error.contains("protocol error 3") && error.contains("invalid metadata"), "{error}");
        drop(handle);

        // Nobody listening, and a peer that never answers.
        assert!(secure(&dir.join("absent"), &dir.join("p"), "x").is_err());
        let silent = dir.join("silent");
        let listener = UnixListener::bind(&silent).unwrap();
        let hold = std::thread::spawn(move || {
            let accepted = listener.accept();
            std::thread::sleep(Duration::from_secs(7));
            drop(accepted);
        });
        let started = std::time::Instant::now();
        assert!(globals(&silent).is_err());
        assert!(started.elapsed() < Duration::from_secs(7), "bounded by the read timeout");
        drop(hold);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
