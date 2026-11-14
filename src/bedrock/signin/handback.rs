//! The desktop's handler for the address a Bedrock sign-in ends on, and its way into the
//! running session.
//!
//! **Why.** Microsoft's sign-in page ends by sending the browser to
//! `ms-xal-0000000048183522://auth?code=…&state=…` — an address for the game, which no browser
//! opens. Firefox does not even show it (neither in the address bar nor in the console; only the
//! developer tools' Network panel lists it), so pasting it back by hand is hard. Instead, the Arch
//! package registers bastion-mc as the desktop's handler for that scheme
//! (`packaging/arch/bastion-mc-bedrock-signin.desktop`, `x-scheme-handler/ms-xal-0000000048183522`,
//! `NoDisplay=true`): the browser runs `bastion-mc bedrock hand-back <address>` ([`hand_back`]),
//! which hands the address to the running session through a socket the session bound before it
//! confined itself ([`take`]) and accepts on afterwards (`server`). Pasting stays the fallback.
//!
//! **The rules.**
//!
//! * The socket is `$XDG_RUNTIME_DIR/bastion-mc/bedrock-signin.sock`: in the user's private runtime
//!   directory (bastion-mc's directory in it is 0700, the socket 0600), so only the user's own
//!   programs reach it; the launcher also checks the peer's user id. The game's sandbox has no path
//!   to it (none of the runtime directory is mounted there, and its network namespace has no route).
//! * **One session at a time holds it**, by an exclusive lock on `bedrock-signin.lock` beside it,
//!   taken before the socket is bound and kept for the session. A second Bedrock session running at
//!   the same time does not get the handler: it takes pasted addresses only, and its launch log and
//!   prompt say so. (Why not one socket per session, with the command trying each: the desktop's
//!   handler cannot tell which session a browser tab belongs to, and the address's `state` is the
//!   only key; handing one sign-in's code to every running session to find the one that recognises
//!   it, and teaching each to ignore the others', buys little — two Bedrock sessions at once is
//!   rare, and the fallback works for the second.) A socket name left by a session that could not
//!   remove it (the confined launcher may not remove files in the runtime directory; a killed one
//!   removes nothing) is removed by the next session that takes the lock: with the lock held, the
//!   name belongs to nobody.
//! * **What the session does with an address** is what it does with a pasted one: the same check
//!   (`check::check_final`: under the redirect, a code, the waiting sign-in's state) and the same
//!   outcome — accepted, the game gets the address rebuilt from code and state; refused, the prompt
//!   says why and stays. One difference, on purpose: a refused address from the handler does not
//!   use up one of the prompt's answers. Any web page can make a browser call the handler (with the
//!   user's consent the first time, then perhaps without asking), and an address the page made
//!   carries no state of this sign-in — it cannot get through, but it should not be able to end the
//!   prompt either. Each reason is logged once; the totals at the end of the session.
//! * **The command** prints one plain sentence and exits 0 (handed to the game) or 1 (anything
//!   else). It opens nothing, reads no configuration, writes no file, and never prints the address
//!   (it holds the sign-in's code).

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener as StdUnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use nix::fcntl::{Flock, FlockArg};

use super::REDIRECT_URI;
use crate::{
    error::{Context, Error, Result},
    util,
};

/// The socket's name in bastion-mc's runtime directory.
pub const SOCKET_NAME: &str = "bedrock-signin.sock";
/// The lock that makes one session the handler's.
const LOCK_NAME: &str = "bedrock-signin.lock";
/// The desktop entry the Arch package installs (`/usr/share/applications/`).
pub const DESKTOP_FILE: &str = "bastion-mc-bedrock-signin.desktop";
/// The MIME type of the scheme it handles.
pub const SCHEME_TYPE: &str = "x-scheme-handler/ms-xal-0000000048183522";
/// The longest line either side reads: the address (as long as a pasted answer may be) and
/// its newline; a reply is far shorter.
pub const MAX_LINE: usize = bastion_mc_proto::answer::MAX_ANSWER_TEXT + 1;
/// How long the command waits for the session's verdict. The session answers at once.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The longest socket path (`sockaddr_un`, less the terminating byte).
const MAX_SOCKET_PATH: usize = 107;

/// Where the handler's socket and lock are: bastion-mc's directory in the runtime directory.
pub fn place(runtime: &Path) -> PathBuf {
    runtime.join(crate::paths::APP)
}

// ---------------------------------------------------------------- the session's end

/// The handler's end in the launcher, held by one Bedrock session (see the module header).
pub struct Handback {
    listener: StdUnixListener,
    held: Held,
}

/// What the session holds while it serves the handler: the lock, and the socket's name, which
/// is removed when this is dropped (if the launcher may still remove it) — before the lock is
/// released, so that the name removed is never another session's.
pub struct Held {
    socket: PathBuf,
    lock: Option<Flock<File>>,
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        self.lock = None;
    }
}

/// What [`take`] found.
pub enum Taken {
    /// This session takes the addresses the desktop's handler delivers.
    Bound(Handback),
    /// Another running Bedrock session holds the handler: this one takes pasted addresses only.
    Elsewhere,
    /// There is no runtime directory (`XDG_RUNTIME_DIR` is not set): the handler could not find
    /// the session.
    NoRuntimeDir,
}

/// Take the handler for this session: the lock, then the socket, bound now — before the
/// launcher confines itself (it may bind nothing afterwards).
pub fn take(runtime: Option<&Path>) -> Result<Taken> {
    let Some(runtime) = runtime else { return Ok(Taken::NoRuntimeDir) };
    let dir = place(runtime);
    util::create_private_dir_all(&dir)?;
    let socket = dir.join(SOCKET_NAME);
    if socket.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(Error::Sandbox(format!(
            "{} is too long for a socket (more than {MAX_SOCKET_PATH} bytes)",
            socket.display()
        )));
    }
    let lock_path = dir.join(LOCK_NAME);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&lock_path)
        .ctx(|| format!("opening {}", lock_path.display()))?;
    let lock = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => lock,
        Err((_, nix::errno::Errno::EWOULDBLOCK)) => return Ok(Taken::Elsewhere),
        Err((_, e)) => return Err(Error::io(format!("locking {}", lock_path.display()), e.into())),
    };
    // With the lock held, a name that is still there belongs to a session that is over.
    match std::fs::symlink_metadata(&socket) {
        Ok(meta) if meta.file_type().is_socket() => {
            std::fs::remove_file(&socket).ctx(|| format!("removing the old {}", socket.display()))?
        }
        Ok(_) => {
            return Err(Error::Policy(format!(
                "{} is not a socket; bastion-mc did not put it there (remove it)",
                socket.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(format!("inspecting {}", socket.display()), e)),
    }
    let listener = StdUnixListener::bind(&socket).ctx(|| format!("binding {}", socket.display()))?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .ctx(|| format!("restricting {}", socket.display()))?;
    Ok(Taken::Bound(Handback { listener, held: Held { socket, lock: Some(lock) } }))
}

impl Handback {
    /// The socket's path on the host.
    pub fn socket(&self) -> &Path {
        &self.held.socket
    }

    /// The listener, for the server, and what the session keeps until it ends.
    pub(crate) fn into_parts(self) -> (StdUnixListener, Held) {
        (self.listener, self.held)
    }
}

/// The session's answer on the socket: `ok <sentence>` or `no <sentence>`, one line.
pub(crate) fn verdict_line(accepted: bool, sentence: &str) -> String {
    let clean: String = sentence.chars().filter(|c| !c.is_control()).take(400).collect();
    format!("{} {clean}\n", if accepted { "ok" } else { "no" })
}

// ---------------------------------------------------------------- the command's end

/// What became of an address handed back (`bastion-mc bedrock hand-back`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The running session took it and handed it to the game.
    Accepted,
    /// Refused, with the reason: by the command, before anything was sent, or by the session.
    Refused(String),
    /// No session could be reached, with why.
    NoSession(String),
}

impl Outcome {
    /// The one sentence the command prints.
    pub fn sentence(&self) -> String {
        match self {
            Outcome::Accepted => {
                "bastion-mc: the sign-in address was handed to the Bedrock game; go back to the game".into()
            }
            Outcome::Refused(why) => format!("bastion-mc: the address was not handed to the game: {why}"),
            Outcome::NoSession(why) => format!("bastion-mc: nothing was done: {why}"),
        }
    }

    pub fn exit_code(&self) -> i32 {
        if *self == Outcome::Accepted { 0 } else { 1 }
    }
}

/// Hand `address` to the running Bedrock session (the desktop's handler runs this with the
/// address the browser was sent to). `runtime` is `XDG_RUNTIME_DIR`. Never fails: every outcome
/// is an [`Outcome`].
pub fn hand_back(runtime: Option<&Path>, address: &str) -> Outcome {
    let address = address.trim();
    if bastion_mc_proto::answer::check_answer_text(address).is_err()
        || !address.starts_with(&format!("{REDIRECT_URI}?"))
    {
        return Outcome::Refused(format!(
            "it is not the address a Microsoft sign-in for Minecraft Bedrock ends on ({REDIRECT_URI}?…)"
        ));
    }
    let Some(runtime) = runtime else {
        return Outcome::NoSession("XDG_RUNTIME_DIR is not set, so no running session can be found".into());
    };
    let none = || Outcome::NoSession("no Bedrock session that takes sign-in addresses is running".into());
    let dir = place(runtime);
    if util::require_private(&dir).is_err() {
        return none();
    }
    let mut stream = match UnixStream::connect(dir.join(SOCKET_NAME)) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {
            return none();
        }
        Err(e) => return Outcome::NoSession(format!("the running session could not be reached ({e})")),
    };
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    if stream.write_all(format!("{address}\n").as_bytes()).is_err() {
        return Outcome::NoSession("the running session closed the connection".into());
    }
    let mut reply = Vec::new();
    let _ = stream.take(MAX_LINE as u64).read_to_end(&mut reply);
    let reply = String::from_utf8_lossy(&reply);
    let line = reply.lines().next().unwrap_or("");
    let clean = |text: &str| util::sanitize_line(text.as_bytes(), 300);
    match line.split_once(' ') {
        Some(("ok", _)) => Outcome::Accepted,
        Some(("no", why)) => Outcome::Refused(clean(why)),
        _ => Outcome::NoSession("the running session gave no answer in time".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bmc-hb-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    /// One session holds the handler; a second one at the same time is told to fall back to
    /// pasting; once the first is over, the next takes it — and removes the name the first may
    /// have had to leave.
    #[test]
    fn one_session_holds_the_handler_and_a_second_falls_back() {
        let rt = runtime("take");
        let Taken::Bound(first) = take(Some(&rt)).unwrap() else { panic!("the first session takes it") };
        let socket = first.socket().to_path_buf();
        assert_eq!(socket, rt.join("bastion-mc").join(SOCKET_NAME));
        assert_eq!(std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(socket.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(matches!(take(Some(&rt)).unwrap(), Taken::Elsewhere), "a second session falls back to pasting");
        // A session that ends confined cannot remove the name: forget it without the removal.
        let (listener, mut held) = first.into_parts();
        drop(listener);
        let lock = held.lock.take();
        std::mem::forget(held);
        drop(lock);
        assert!(std::fs::symlink_metadata(&socket).unwrap().file_type().is_socket(), "the name was left");
        let Taken::Bound(next) = take(Some(&rt)).unwrap() else { panic!("free again") };
        drop(next);
        assert!(std::fs::symlink_metadata(&socket).is_err(), "and removed when the session ends");
        assert!(matches!(take(None).unwrap(), Taken::NoRuntimeDir));
        std::fs::remove_dir_all(rt).unwrap();
    }

    /// The desktop entry the Arch package installs hands the scheme to this command, and stays
    /// out of menus; the PKGBUILD installs it where the MIME cache's pacman hook looks.
    #[test]
    fn the_packaged_desktop_entry_hands_the_scheme_to_this_command() {
        let entry = include_str!("../../../packaging/arch/bastion-mc-bedrock-signin.desktop");
        let has = |line: &str| entry.lines().any(|l| l == line);
        assert!(has(&format!("MimeType={SCHEME_TYPE};")), "{entry}");
        assert!(has("Exec=/usr/bin/bastion-mc bedrock hand-back %u"), "{entry}");
        assert!(has("NoDisplay=true") && has("Type=Application") && has("Terminal=false"), "{entry}");
        assert_eq!(SCHEME_TYPE, format!("x-scheme-handler/{}", REDIRECT_URI.split("://").next().unwrap()));
        let pkgbuild = include_str!("../../../packaging/arch/PKGBUILD");
        assert!(
            pkgbuild.contains(&format!(
                "install -Dm0644 -t \"$pkgdir/usr/share/applications/\" packaging/arch/{DESKTOP_FILE}"
            )),
            "the launcher's package installs it"
        );
    }

    #[test]
    fn something_else_at_the_sockets_name_is_refused_not_removed() {
        let rt = runtime("other");
        std::fs::create_dir_all(place(&rt)).unwrap();
        std::fs::set_permissions(place(&rt), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(place(&rt).join(SOCKET_NAME), b"a file").unwrap();
        assert!(matches!(take(Some(&rt)), Err(Error::Policy(_))));
        assert_eq!(std::fs::read(place(&rt).join(SOCKET_NAME)).unwrap(), b"a file");
        std::fs::remove_dir_all(rt).unwrap();
    }

    /// The command: a foreign address is refused before anything is sent; without a session or
    /// a runtime directory it says so; with one, it reports the session's verdict.
    #[test]
    fn the_command_refuses_foreign_addresses_and_reports_what_the_session_said() {
        let rt = runtime("cmd");
        let good = format!("{REDIRECT_URI}?code=M.C5&state=s");
        for foreign in [
            "https://evil.example/",
            "ms-xal-0000000048183522://other?code=a",
            "",
            "  ",
            "ms-xal-0000000048183522://auth",
        ] {
            assert!(matches!(hand_back(Some(&rt), foreign), Outcome::Refused(_)), "{foreign:?}");
        }
        assert!(matches!(hand_back(Some(&rt), &format!("{good}\u{1b}[2J")), Outcome::Refused(_)));
        assert!(matches!(hand_back(None, &good), Outcome::NoSession(why) if why.contains("XDG_RUNTIME_DIR")));
        assert!(matches!(hand_back(Some(&rt), &good), Outcome::NoSession(why) if why.contains("no Bedrock session")));
        // A session that answers.
        let Taken::Bound(handback) = take(Some(&rt)).unwrap() else { panic!() };
        let (listener, _held) = handback.into_parts();
        let server = std::thread::spawn(move || {
            for verdict in [verdict_line(true, "handed"), verdict_line(false, "its state is not this sign-in's")] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = Vec::new();
                let mut byte = [0u8; 1];
                while stream.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
                    line.push(byte[0]);
                }
                assert_eq!(line, format!("{REDIRECT_URI}?code=M.C5&state=s").into_bytes(), "the address, trimmed");
                stream.write_all(verdict.as_bytes()).unwrap();
            }
        });
        let accepted = hand_back(Some(&rt), &format!(" {good} "));
        assert_eq!(accepted, Outcome::Accepted);
        assert_eq!(accepted.exit_code(), 0);
        let refused = hand_back(Some(&rt), &good);
        assert_eq!(refused, Outcome::Refused("its state is not this sign-in's".into()));
        assert_eq!(refused.exit_code(), 1);
        assert!(!refused.sentence().contains("M.C5"), "the sentence never holds the code");
        server.join().unwrap();
        std::fs::remove_dir_all(rt).unwrap();
    }
}
