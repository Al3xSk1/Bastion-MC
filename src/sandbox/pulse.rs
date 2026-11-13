//! Does the sound server let its clients load modules into it?
//!
//! The PulseAudio protocol has a `LOAD_MODULE` command. With classic PulseAudio a module is
//! native code; with pipewire-pulse it is one of a built-in set that includes
//! `module-ladspa-sink` (loads a plugin from a path the client names) and the pipe modules
//! (read or create files the client names). The server runs outside the sandbox, and the
//! game can write files into its instance directories — so a sound server that honours
//! `LOAD_MODULE` from the game is a way out of the sandbox, not an audio feature.
//!
//! Both servers accept the command **by default** (PipeWire: `pulse.allow-module-loading`
//! defaults to true). Reading configuration files to find out is guesswork: the effective
//! value depends on which files the running server read. So the launcher asks the server
//! itself, before it exposes the socket: it requests a module that does not exist. A server
//! that refuses module loading answers "access denied" without looking the name up; any
//! other answer means it tried. Nothing is loaded either way.
//!
//! The answer is trusted only in one direction: everything except a clean "access denied" —
//! including a timeout or a reply this code does not understand — counts as "may load".

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleLoading {
    /// The server refused: clients cannot load modules.
    Denied,
    /// The server accepted the request (it only failed because the module does not exist).
    Allowed,
    /// No clear answer. Treated like `Allowed` by callers.
    Unknown(String),
}

const COMMAND_ERROR: u32 = 0;
const COMMAND_REPLY: u32 = 2;
const COMMAND_AUTH: u32 = 8;
const COMMAND_SET_CLIENT_NAME: u32 = 9;
const COMMAND_LOAD_MODULE: u32 = 51;
const COMMAND_UNLOAD_MODULE: u32 = 52;
const ERR_ACCESS: u32 = 1;

/// Protocol revision claimed. The three commands used here have had this shape since 13.
const PROTOCOL_VERSION: u32 = 32;
const CONTROL_CHANNEL: u32 = 0xFFFF_FFFF;
const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PACKET: usize = 64 * 1024;
/// A module name no server has.
const PROBE_MODULE: &str = "module-bastion-mc-probe-does-not-exist";

// ---- tagstruct encoding (the subset needed)

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.push(b'L');
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_string(out: &mut Vec<u8>, value: &str) {
    out.push(b't');
    out.extend_from_slice(value.as_bytes());
    out.push(0);
}

fn put_arbitrary(out: &mut Vec<u8>, value: &[u8]) {
    out.push(b'x');
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
}

fn packet(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&CONTROL_CHANNEL.to_be_bytes());
    out.extend_from_slice(&[0u8; 12]); // offset high/low, flags
    out.extend_from_slice(payload);
    out
}

fn auth(tag: u32) -> Vec<u8> {
    let mut p = Vec::new();
    put_u32(&mut p, COMMAND_AUTH);
    put_u32(&mut p, tag);
    put_u32(&mut p, PROTOCOL_VERSION);
    // No cookie: a server on the user's own socket authorises by peer credentials, and if
    // it does not, the probe ends as `Unknown`, which is the safe answer.
    put_arbitrary(&mut p, &[0u8; 256]);
    packet(&p)
}

fn set_client_name(tag: u32) -> Vec<u8> {
    let mut p = Vec::new();
    put_u32(&mut p, COMMAND_SET_CLIENT_NAME);
    put_u32(&mut p, tag);
    p.push(b'P');
    let name = b"bastion-mc (module-loading check)\0";
    put_string(&mut p, "application.name");
    put_u32(&mut p, name.len() as u32);
    put_arbitrary(&mut p, name);
    p.push(b'N'); // end of property list
    packet(&p)
}

fn load_module(tag: u32) -> Vec<u8> {
    let mut p = Vec::new();
    put_u32(&mut p, COMMAND_LOAD_MODULE);
    put_u32(&mut p, tag);
    put_string(&mut p, PROBE_MODULE);
    put_string(&mut p, "");
    packet(&p)
}

fn unload_module(tag: u32, index: u32) -> Vec<u8> {
    let mut p = Vec::new();
    put_u32(&mut p, COMMAND_UNLOAD_MODULE);
    put_u32(&mut p, tag);
    put_u32(&mut p, index);
    packet(&p)
}

/// The three leading `u32`s of a control packet's payload: command, tag, first argument.
fn read_u32s(payload: &[u8]) -> Option<(u32, u32, Option<u32>)> {
    let field = |at: usize| -> Option<u32> {
        let chunk = payload.get(at..at + 5)?;
        (chunk[0] == b'L').then(|| u32::from_be_bytes([chunk[1], chunk[2], chunk[3], chunk[4]]))
    };
    Some((field(0)?, field(5)?, field(10)))
}

/// Read packets until the reply to `tag` arrives. Returns (command, first argument).
fn reply_to(stream: &mut UnixStream, tag: u32) -> Result<(u32, Option<u32>), String> {
    // A server may interleave unrelated packets; it will not send many.
    for _ in 0..16 {
        let mut header = [0u8; 20];
        stream.read_exact(&mut header).map_err(|e| format!("no reply ({e})"))?;
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let channel = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
        if length > MAX_PACKET {
            return Err("the server sent an implausibly large packet".into());
        }
        let mut payload = vec![0u8; length];
        stream.read_exact(&mut payload).map_err(|e| format!("truncated reply ({e})"))?;
        if channel != CONTROL_CHANNEL {
            continue;
        }
        match read_u32s(&payload) {
            Some((command, seen, first)) if seen == tag && matches!(command, COMMAND_ERROR | COMMAND_REPLY) => {
                return Ok((command, first));
            }
            _ => continue,
        }
    }
    Err("the server did not answer the request".into())
}

fn converse(stream: &mut UnixStream) -> Result<ModuleLoading, String> {
    let send =
        |stream: &mut UnixStream, bytes: Vec<u8>| stream.write_all(&bytes).map_err(|e| format!("send failed ({e})"));

    send(stream, auth(1))?;
    match reply_to(stream, 1)? {
        (COMMAND_REPLY, _) => {}
        (_, code) => return Err(format!("the server refused the connection (error {})", code.unwrap_or(0))),
    }
    send(stream, set_client_name(2))?;
    match reply_to(stream, 2)? {
        (COMMAND_REPLY, _) => {}
        (_, code) => return Err(format!("the server refused the client (error {})", code.unwrap_or(0))),
    }
    send(stream, load_module(3))?;
    match reply_to(stream, 3)? {
        (COMMAND_ERROR, Some(ERR_ACCESS)) => Ok(ModuleLoading::Denied),
        (COMMAND_ERROR, Some(_)) => Ok(ModuleLoading::Allowed),
        (COMMAND_REPLY, index) => {
            // It really loaded something under that name. Take it out again.
            if let Some(index) = index {
                let _ = send(stream, unload_module(4, index));
                let _ = reply_to(stream, 4);
            }
            Ok(ModuleLoading::Allowed)
        }
        _ => Err("the server's answer to the module request was not understood".into()),
    }
}

/// Ask the server behind `socket` whether a client may load modules.
pub fn probe(socket: &Path) -> ModuleLoading {
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(e) => return ModuleLoading::Unknown(format!("cannot connect to {} ({e})", socket.display())),
    };
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    converse(&mut stream).unwrap_or_else(ModuleLoading::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::net::UnixListener, path::PathBuf};

    /// A socket path in a directory of its own, removed again when the test is done.
    struct Scratch(PathBuf);

    impl std::ops::Deref for Scratch {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for Scratch {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    fn socket_path(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "bastion-pulse-{tag}-{}",
            crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir.join("native"))
    }

    fn control(command: u32, tag: u32, value: Option<u32>) -> Vec<u8> {
        let mut p = Vec::new();
        put_u32(&mut p, command);
        put_u32(&mut p, tag);
        if let Some(v) = value {
            put_u32(&mut p, v);
        }
        packet(&p)
    }

    fn read_request(stream: &mut UnixStream) -> Option<(u32, u32, Vec<u8>)> {
        let mut header = [0u8; 20];
        stream.read_exact(&mut header).ok()?;
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let mut payload = vec![0u8; length];
        stream.read_exact(&mut payload).ok()?;
        let (command, tag, _) = read_u32s(&payload)?;
        Some((command, tag, payload))
    }

    /// A stand-in server: answers the handshake, then `on_load` decides the answer to
    /// LOAD_MODULE. Returns what it saw.
    fn serve(path: &Path, on_load: impl Fn(u32) -> Vec<Vec<u8>> + Send + 'static) -> std::thread::JoinHandle<Vec<u32>> {
        let listener = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            while let Some((command, tag, payload)) = read_request(&mut stream) {
                seen.push(command);
                let replies = match command {
                    COMMAND_AUTH => vec![control(COMMAND_REPLY, tag, Some(35))],
                    COMMAND_SET_CLIENT_NAME => vec![control(COMMAND_REPLY, tag, Some(7))],
                    COMMAND_LOAD_MODULE => {
                        assert!(payload.windows(PROBE_MODULE.len()).any(|w| w == PROBE_MODULE.as_bytes()));
                        on_load(tag)
                    }
                    COMMAND_UNLOAD_MODULE => vec![control(COMMAND_REPLY, tag, None)],
                    _ => vec![control(COMMAND_ERROR, tag, Some(2))],
                };
                for reply in replies {
                    if stream.write_all(&reply).is_err() {
                        return seen;
                    }
                }
            }
            seen
        })
    }

    #[test]
    fn a_server_that_refuses_module_loading_is_the_only_safe_answer() {
        let path = socket_path("denied");
        let server = serve(&path, |tag| vec![control(COMMAND_ERROR, tag, Some(ERR_ACCESS))]);
        assert_eq!(probe(&path), ModuleLoading::Denied);
        assert_eq!(server.join().unwrap(), vec![COMMAND_AUTH, COMMAND_SET_CLIENT_NAME, COMMAND_LOAD_MODULE]);
    }

    #[test]
    fn any_other_error_means_the_server_tried() {
        // "No such entity" (pipewire-pulse) and "module initialisation failed" (PulseAudio).
        for code in [5u32, 14, 3] {
            let path = socket_path("allowed");
            let server = serve(&path, move |tag| vec![control(COMMAND_ERROR, tag, Some(code))]);
            assert_eq!(probe(&path), ModuleLoading::Allowed, "error code {code}");
            server.join().unwrap();
        }
    }

    #[test]
    fn a_module_that_was_really_loaded_is_unloaded_again() {
        let path = socket_path("loaded");
        let server = serve(&path, |tag| vec![control(COMMAND_REPLY, tag, Some(42))]);
        assert_eq!(probe(&path), ModuleLoading::Allowed);
        assert_eq!(server.join().unwrap().last(), Some(&COMMAND_UNLOAD_MODULE));
    }

    #[test]
    fn unrelated_packets_before_the_answer_are_skipped() {
        let path = socket_path("noise");
        let server = serve(&path, |tag| {
            let mut data_channel = packet(b"audio");
            data_channel[4..8].copy_from_slice(&3u32.to_be_bytes());
            vec![
                data_channel,                               // not the control channel
                control(61, 0xFFFF_FFFF, None),             // a server-initiated event
                control(COMMAND_REPLY, tag + 100, Some(1)), // a reply to something else
                control(COMMAND_ERROR, tag, Some(ERR_ACCESS)),
            ]
        });
        assert_eq!(probe(&path), ModuleLoading::Denied);
        server.join().unwrap();
    }

    #[test]
    fn everything_unclear_is_unknown_never_denied() {
        // Nobody listening.
        let missing = socket_path("missing");
        assert!(matches!(probe(&missing), ModuleLoading::Unknown(_)));

        // The server hangs up, sends rubbish, or answers without an error code.
        type Reply = fn(u32) -> Vec<Vec<u8>>;
        let cases: [(&str, Reply); 3] = [
            ("hangup", |_| Vec::new()),
            ("rubbish", |_| {
                vec![b"\x00\x00\x00\x04\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00junk".to_vec()]
            }),
            ("no-code", |tag| vec![control(COMMAND_ERROR, tag, None)]),
        ];
        for (name, reply) in cases {
            let path = socket_path(name);
            let listener = UnixListener::bind(&path).unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                while let Some((command, tag, _)) = read_request(&mut stream) {
                    let replies = match command {
                        COMMAND_AUTH | COMMAND_SET_CLIENT_NAME => vec![control(COMMAND_REPLY, tag, Some(1))],
                        _ => {
                            let r = reply(tag);
                            if r.is_empty() {
                                return; // close the connection
                            }
                            r
                        }
                    };
                    for r in replies {
                        let _ = stream.write_all(&r);
                    }
                    if command == COMMAND_LOAD_MODULE {
                        return;
                    }
                }
            });
            let outcome = probe(&path);
            assert!(matches!(outcome, ModuleLoading::Unknown(_)), "{name}: {outcome:?}");
            server.join().unwrap();
        }

        // A server that rejects the handshake.
        let path = socket_path("auth");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            if let Some((_, tag, _)) = read_request(&mut stream) {
                let _ = stream.write_all(&control(COMMAND_ERROR, tag, Some(ERR_ACCESS)));
            }
        });
        assert!(matches!(probe(&path), ModuleLoading::Unknown(_)), "a refused login says nothing about modules");
        server.join().unwrap();
    }

    #[test]
    fn packets_have_the_documented_framing() {
        let p = load_module(3);
        assert_eq!(&p[..4], &((p.len() - 20) as u32).to_be_bytes(), "length covers the payload only");
        assert_eq!(&p[4..8], &[0xff; 4], "control channel");
        assert_eq!(&p[8..20], &[0; 12]);
        assert_eq!(&p[20..25], &[b'L', 0, 0, 0, 51]);
        assert_eq!(&p[25..30], &[b'L', 0, 0, 0, 3]);
        assert_eq!(p[30], b't');
        assert_eq!(*p.last().unwrap(), 0, "strings are NUL-terminated");
        assert_eq!(read_u32s(&p[20..]), Some((COMMAND_LOAD_MODULE, 3, None)));
    }
}
