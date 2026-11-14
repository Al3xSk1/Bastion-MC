//! The stub: what runs when the game's client starts `mcpelauncher-webview` inside the
//! sandbox.
//!
//! It is the launcher's own executable, mounted at the stub paths; `main.rs` sends it here
//! when `argv[0]`'s file name is [`STUB_NAME`], before anything else happens — no
//! configuration is read, nothing is logged or written, no environment variable is looked at.
//! The client calls it as `mcpelauncher-webview <start-url> <end-url-prefix>` and reads its
//! standard output with **one 1024-byte read** (`XalWebViewQt::exec_get_stdout`), then its
//! standard error, then waits for it; exit status 0 means "here is the address".
//!
//! So the stub: connects to [`SANDBOX_SOCKET`], sends the two arguments as one request frame,
//! waits for the reply, and then either writes the address and a newline to standard output
//! in a single write and exits 0, or writes nothing there, a short reason to standard error,
//! and exits 1. It keeps its end of the connection open while it waits: the launcher takes a
//! closed connection to mean that the game stopped waiting, and withdraws the prompt.
//!
//! The stub is a convenience for the real client, not a boundary: anything in the sandbox can
//! connect to the socket and send whatever it likes. Every check is the launcher's.
//!
//! **How long it waits** ([`STUB_TIMEOUT`]): a real sign-in can take many minutes — typing an
//! address and a password, a second factor that arrives by mail or text message, a new
//! account's forms, then finding the final address in the browser and pasting it. The
//! launcher gives a prompt [`server::PROMPT_TTL`](super::server::PROMPT_TTL) (15 minutes) and
//! always answers when it ends; the stub waits a minute longer, so that its own limit only
//! ever matters if the launcher is stuck. If the launcher is gone, the connection closes and
//! the stub ends at once.
//!
//! System calls, for the sandbox's filter. Its own: `socket` (`AF_UNIX`, `SOCK_STREAM`),
//! `connect`, `setsockopt` (`SO_SNDTIMEO`, `SO_RCVTIMEO`), `write`/`sendto`, `recvfrom`,
//! `close`. Starting and ending (the loader, glibc, Rust's runtime): `execve`, `brk`,
//! `arch_prctl`, `set_tid_address`, `set_robust_list`, `rseq`, `prlimit64`, `mmap`, `munmap`,
//! `mprotect`, `openat`, `read`, `pread64`, `fstat`, `newfstatat`, `access`, `getrandom`,
//! `rt_sigaction`, `rt_sigprocmask`, `sigaltstack`, `poll`, `exit_group`, `exit`. Checked by
//! `tests/signin_stub.rs`: the stub completes a sign-in under exactly this allowlist, every
//! other call failing with `ENOSYS` (on this machine's glibc and toolchain; another build may
//! differ, and the game's filter must allow the list in any case).

use std::{
    ffi::OsString,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

use super::{
    SANDBOX_SOCKET, STUB_NAME,
    check::MAX_FINAL,
    server::PROMPT_TTL,
    wire::{self, Reply},
};

/// How long the stub waits for the launcher's reply (see the module header).
pub const STUB_TIMEOUT: Duration = Duration::from_secs(PROMPT_TTL.as_secs() + 60);
/// How long sending the request may take. The launcher reads requests as they arrive.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Is this program running as the stub? (`argv[0]`'s file name is [`STUB_NAME`].)
pub fn is_stub(argv0: Option<&OsString>) -> bool {
    argv0.and_then(|a| Path::new(a).file_name()).is_some_and(|name| name == STUB_NAME)
}

/// The stub's whole life. `args` are the arguments after `argv[0]`.
pub fn main(args: &[OsString]) -> ! {
    let code = run(args, Path::new(SANDBOX_SOCKET), STUB_TIMEOUT, &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(code)
}

/// [`main`] with the socket, the time limit and the output streams given: the exit status.
pub fn run(args: &[OsString], socket: &Path, timeout: Duration, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match ask(args, socket, timeout) {
        Ok(url) => {
            let mut line = url.into_bytes();
            line.push(b'\n');
            // One write: the client reads once.
            match out.write_all(&line).and_then(|()| out.flush()) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        }
        Err(why) => {
            let _ = writeln!(err, "mcpelauncher-webview (bastion-mc): {}", printable(&why, 300));
            1
        }
    }
}

/// Printable ASCII only, at most `max` characters.
fn printable(text: &str, max: usize) -> String {
    text.chars().map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '?' }).take(max).collect()
}

fn ask(args: &[OsString], socket: &Path, timeout: Duration) -> Result<String, String> {
    let [start, end] = args else {
        return Err("expected two arguments: the sign-in page and the prefix of the address it ends on".into());
    };
    let (Some(start), Some(end)) = (start.to_str(), end.to_str()) else {
        return Err("an argument is not UTF-8".into());
    };
    let frame = wire::request_frame(start, end).ok_or("the request is too large")?;
    let deadline = Instant::now() + timeout;
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("the launcher cannot be reached: {e}"))?;
    stream.set_write_timeout(Some(SEND_TIMEOUT)).map_err(|e| e.to_string())?;
    stream.write_all(&frame).map_err(|e| format!("sending the request: {e}"))?;
    let mut header = [0u8; 4];
    read_until(&mut stream, &mut header, deadline)?;
    let len = wire::body_len(header)?;
    let mut body = vec![0u8; len];
    read_until(&mut stream, &mut body, deadline)?;
    match wire::parse_reply(&body)? {
        Reply::Url(url) => {
            // The launcher checked it; checked again here, because what the client is given
            // must be what it asked for, whatever went wrong in between.
            let fits = url.len() <= MAX_FINAL && url.bytes().all(|b| b.is_ascii_graphic());
            if fits && url.starts_with(end) {
                Ok(url)
            } else {
                Err("the launcher's reply is not an address under the prefix asked for".into())
            }
        }
        Reply::Error(why) => Err(why),
    }
}

/// Fill `buf` before `deadline`.
fn read_until(stream: &mut UnixStream, buf: &mut [u8], deadline: Instant) -> Result<(), String> {
    let mut filled = 0;
    while filled < buf.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("no answer from the launcher in time".into());
        }
        stream.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Err("the launcher closed the connection".into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err("no answer from the launcher in time".into());
            }
            Err(e) => return Err(format!("reading the reply: {e}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_is_recognised_by_its_file_name_alone() {
        assert!(is_stub(Some(&OsString::from("mcpelauncher-webview"))));
        assert!(is_stub(Some(&OsString::from("/bastion/bin/mcpelauncher-webview"))));
        assert!(is_stub(Some(&OsString::from("/usr/share/mcpelauncher/bin/mcpelauncher-webview"))));
        assert!(!is_stub(Some(&OsString::from("bastion-mc"))));
        assert!(!is_stub(Some(&OsString::from("/usr/bin/mcpelauncher-webview-qt"))));
        assert!(!is_stub(None));
    }

    #[test]
    fn without_two_arguments_or_a_launcher_it_fails_quietly_on_standard_output() {
        let socket = std::env::temp_dir().join(format!("bmc-stub-none-{}.sock", std::process::id()));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run(&[OsString::from("one")], &socket, Duration::from_secs(1), &mut out, &mut err), 1);
        assert!(out.is_empty());
        assert!(String::from_utf8(err).unwrap().contains("two arguments"));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args = [OsString::from("https://login.live.com/"), OsString::from("ms-xal-x")];
        assert_eq!(run(&args, &socket, Duration::from_secs(1), &mut out, &mut err), 1);
        assert!(out.is_empty());
        assert!(String::from_utf8(err).unwrap().contains("cannot be reached"));
    }

    #[test]
    fn reasons_reach_standard_error_printable_and_bounded() {
        assert_eq!(printable("a\u{1b}[2Jb\n\u{e9}", 100), "a?[2Jb??");
        assert_eq!(printable(&"x".repeat(400), 300).len(), 300);
    }
}
