//! A display server of the game's own, for sessions whose display server has no isolation.
//!
//! On X11 every client can read all keyboard input, inject input into other windows and
//! capture the screen; handing the game the session's X socket hands it the session. A
//! *nested* compositor closes that: gamescope runs as an ordinary client of the session's
//! display, shows one window, and inside that window is a complete display server
//! (Xwayland) with exactly one application on it. The game is given that inner server's
//! socket and nothing of the outer one. What X11 lets a client do to "every other client"
//! now means: to itself.
//!
//! ```text
//!   session X server ◀── socket + cookie ── gamescope ── Xwayland     (compositor sandbox)
//!                                                           ▲
//!                                      /tmp/.X11-unix/XN    │  the only thing shared
//!                                                           │
//!                                                         game        (game sandbox)
//! ```
//!
//! The compositor does face the game — it parses the game's X11 requests — and it holds the
//! session's display. So it gets a sandbox of its own, built from the same parts as the
//! game's (bubblewrap namespaces, Landlock, seccomp, optionally a resource scope): no home
//! directory, no network, no D-Bus, read-only `/usr`, the GPU, the session's display socket.
//! A bug in Xwayland or gamescope that the game can reach is then *needed* to get at the
//! session's display — instead of the display being handed over by design — and yields
//! nothing beyond it.
//!
//! **The clipboard.** gamescope bridges the CLIPBOARD and PRIMARY selections between the
//! session and its inner display, both ways, and has no switch for it. Left alone, the game
//! could read whatever is copied in the session (passwords from a password manager) and
//! plant text there. The launcher preloads a small library into the compositor
//! (`native/noclip.c`, built with the launcher and embedded in it) that makes the four SDL
//! calls carrying the bridge do nothing. `tests/nested.rs` checks both directions.
//!
//! A preload can fail without a sound — the dynamic linker skips it for a program with file
//! capabilities, and a compositor built to reach SDL differently never calls it. So the
//! launcher does not assume the cut: the library reports from inside the compositor
//! ([`ClipboardCut`]), and only the report "active" lets the launch say "clipboard cut".
//! **"Active" establishes** that the library was loaded into the process that started the
//! launcher's ready child (the compositor), that the dynamic linker resolves the six SDL
//! text-clipboard functions to the library's own definitions, that an SDL library providing
//! them stands behind it (so the compositor gets them by name, not from a private copy), and
//! that SDL's clipboard interface for arbitrary data, which the library does not cover, does
//! not exist in the compositor's symbol scope. **It does not establish** that this build of
//! the compositor has no other way of moving a selection (another backend, direct X11 or
//! Wayland calls): that is a property of the compositor, checked by `tests/nested.rs` for
//! gamescope 3.17 as packaged by Arch and for nothing else. Without the report the launch
//! log and the front end say that the cut is not confirmed, and
//! `sandbox.nested_compositor = "require"` stops the launch.
//!
//! How the launcher learns the inner display: gamescope starts one child once its display
//! is ready. That child is this program again in a tiny mode ([`READY_ARG`]): it prints the
//! `DISPLAY` it was given (and the cut-off's report) and then waits for its standard input
//! to close. Closing it is how the launcher ends the compositor.

use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::Shutdown,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use super::{
    BaseOptions, INIT_PATH, Mount, Network, RUNTIME_DIR, Resolv, Running, SandboxSpec, cgroup, display,
    policy::{InitPolicy, SeccompPolicy, TcpPolicy},
};
use crate::{
    error::{Error, Result},
    util,
};

pub const GAMESCOPE: &str = "/usr/bin/gamescope";
/// Everything the compositor executes: itself, the helper it starts its child through, the
/// X server, and what that X server runs to compile its keymap (`sh -c xkbcomp …`).
const PROGRAMS: [&str; 5] =
    [GAMESCOPE, "/usr/bin/gamescopereaper", "/usr/bin/Xwayland", "/usr/bin/xkbcomp", "/usr/bin/sh"];
/// `bastion-mc __display-ready`: the compositor's child.
pub const READY_ARG: &str = "__display-ready";
const READY_PREFIX: &str = "bastion-mc-display ";
/// What the compositor needs beyond the game's syscall allowlist. Before it runs xkbcomp
/// the X server "drops privileges" — `setgid(getgid())`, `setuid(getuid())` — and gives up
/// if either call fails. In this sandbox (no capabilities, `no_new_privs`, a user namespace
/// with a single mapped id) the two calls can change nothing.
const EXTRA_SYSCALLS: [&str; 2] = ["setgid", "setuid"];
const X11_DIR: &str = "/tmp/.X11-unix";
/// The clipboard cut-off, preloaded into the compositor.
const NOCLIP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libbastion_noclip.so"));
const NOCLIP_PATH: &str = "/bastion/noclip.so";
/// Where the preloaded library leaves its report, for the ready child to pass on.
const NOCLIP_STATUS_ENV: &str = "BASTION_MC_NOCLIP";
const NOCLIP_FIELD: &str = "noclip=";
const STARTUP: Duration = Duration::from_secs(30);
const RECENT_LINES: usize = 12;

/// Whether the clipboard bridge of the compositor that was started is known to be cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardCut {
    /// The cut-off reported from inside the compositor that the bridge calls land in it.
    /// (What that does and does not establish: see the module documentation.)
    Active,
    /// Not established. The text says what was (not) seen.
    Unconfirmed(String),
}

impl ClipboardCut {
    /// From the word the preloaded library left (`None`: it left none).
    fn from_report(word: Option<&str>) -> Self {
        ClipboardCut::Unconfirmed(match word {
            Some("active") => return ClipboardCut::Active,
            None => "the cut-off library did not report from inside the compositor: it was not loaded (the \
                     dynamic linker skips a preload it may not or cannot load)"
                .into(),
            Some("shadowed") => "the compositor defines SDL's clipboard functions itself, so the preloaded cut-off \
                                 is never called"
                .into(),
            Some("no-sdl") => "the compositor does not get SDL's clipboard functions from the dynamic linker (no \
                               SDL library provided them at start-up), so the preloaded cut-off is never called"
                .into(),
            Some("data-interface") => "the compositor's SDL has a clipboard interface for arbitrary data, which \
                                       the cut-off does not cover"
                .into(),
            Some(other) => format!("the cut-off library reported something unknown ({other:?})"),
        })
    }
}

/// Where the compositor's own output goes (the launch log).
pub type Sink = Arc<dyn Fn(String) + Send + Sync>;
type Recent = Arc<Mutex<VecDeque<String>>>;

/// Lets the launcher stop reading the compositor's output for a while.
///
/// The compositor talks to the game: once the game runs, what the compositor prints is
/// influenced by the game. The launcher reads nothing of that kind before it has confined
/// itself, so between starting the game and that moment it holds this output back
/// ([`Session::hold_output`]). `held` tells the two relay tasks to step aside (or, once a
/// hold has been given up without being released, to stop); `pass` is what a relay holds
/// while it reads and handles a line, so that whoever takes it exclusively knows that no
/// relay is in the middle of one.
#[derive(Clone)]
struct Gate {
    held: tokio::sync::watch::Receiver<Flow>,
    pass: Arc<tokio::sync::RwLock<()>>,
}

/// What the relays may do with the compositor's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// Read and log it.
    Open,
    /// Leave it in the pipes for now.
    Held,
    /// Never read it again: a hold ended without being released, i.e. the launch gave up
    /// before the launcher had confined itself (step 9 refused). What the game made the
    /// compositor print meanwhile is thrown away with the pipes, unread.
    Discarded,
}

/// While this exists, nothing the compositor prints is read. [`OutputHold::release`] resumes
/// the relays; what the compositor wrote meanwhile waited in its pipes (a compositor that
/// prints more than a pipe holds waits too — its own delay, for as long as the hold lasts).
/// Dropped without being released, it stops the relays for good instead: a launch that
/// returns early is not confined, so it must not start reading what the game influenced.
pub struct OutputHold {
    held: Arc<tokio::sync::watch::Sender<Flow>>,
    released: bool,
    _pass: tokio::sync::OwnedRwLockWriteGuard<()>,
}

impl OutputHold {
    /// The launcher has confined itself: the relays may read again (step 10).
    pub fn release(mut self) {
        self.released = true;
        self.held.send_replace(Flow::Open);
    }
}

impl Drop for OutputHold {
    fn drop(&mut self) {
        if !self.released {
            self.held.send_replace(Flow::Discarded);
        }
    }
}

/// Are gamescope and what it runs installed as system programs?
///
/// `layers` is test support (see `SandboxConfig::nested_compositor_layers`): programs found
/// in a layer are taken from there.
pub fn locate(layers: &[PathBuf]) -> std::result::Result<(), String> {
    for program in PROGRAMS {
        let relative = program.strip_prefix("/usr/").expect("system path");
        if layers.iter().any(|layer| layer.join(relative).is_file()) {
            continue;
        }
        util::require_root_owned(Path::new(program)).map_err(|e| {
            if Path::new(program).exists() {
                e.to_string()
            } else {
                format!("{program} is not installed (pacman -S gamescope)")
            }
        })?;
    }
    Ok(())
}

/// Everything needed to start the compositor; made when the display is planned, used when
/// the game is about to start.
pub struct Plan {
    /// What the compositor is given of the session: the display socket, a cookie for it,
    /// the GPU.
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub sockets: Vec<PathBuf>,
    pub devices: Vec<PathBuf>,
    /// Extra gamescope options (output size, full screen, …).
    pub args: Vec<String>,
    /// A private directory on the host that becomes the compositor's `/tmp/.X11-unix`. The
    /// inner display's socket appears in it, and is bound into the game's sandbox from it.
    pub dir: PathBuf,
    pub layers: Vec<PathBuf>,
    pub limits: Option<cgroup::Limits>,
    pub seccomp: SeccompPolicy,
    /// Do not accept a compositor whose clipboard bridge is not known to be cut
    /// (`sandbox.nested_compositor = "require"`).
    pub require_clipboard_cut: bool,
    /// Test support, not reachable from a configuration file: `false` starts the compositor
    /// without the cut-off library, to see that its absence is noticed.
    pub preload_clipboard_cut: bool,
}

fn set_env(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    env.retain(|(k, _)| k != key);
    env.push((key.into(), value.into()));
}

impl Plan {
    /// The compositor's sandbox.
    fn spec(self) -> Result<SandboxSpec> {
        let mut exec: Vec<PathBuf> = PROGRAMS.iter().map(PathBuf::from).collect();
        exec.push(INIT_PATH.into());
        let mut base = super::base(&BaseOptions {
            tmp_size_mb: 256,
            jvm: None,
            exec_allow: &exec,
            resolv: Resolv::None,
            expose_timezone: false,
            trust: None,
        })?;
        if !self.layers.is_empty() {
            // Test support: `/usr` as it would be with more packages installed.
            let Some(Mount::RoBind { dst, .. }) = base.mounts.first() else {
                return Err(Error::Sandbox("the base system does not start with /usr".into()));
            };
            debug_assert_eq!(dst, Path::new("/usr"));
            let mut layers = vec![PathBuf::from("/usr")];
            layers.extend(self.layers);
            base.mounts[0] = Mount::RoOverlay { layers, dst: "/usr".into() };
        }
        let mut mounts = base.mounts;
        // Before the session's socket, which is mounted inside it.
        mounts.push(Mount::Bind { src: self.dir, dst: X11_DIR.into() });
        mounts.extend(self.mounts);
        if self.preload_clipboard_cut {
            mounts.push(Mount::RoData { data: NOCLIP.to_vec(), dst: NOCLIP_PATH.into(), mode: 0o444 });
        }

        let mut landlock = base.landlock;
        landlock.devices.extend(self.devices);
        landlock.unix_sockets.extend(self.sockets);
        // A display server creates sockets: the inner X display, its own Wayland socket.
        landlock.socket_dirs = vec![X11_DIR.into(), RUNTIME_DIR.into()];
        landlock.tcp = TcpPolicy::Ports(vec![]);
        landlock.udp_send = Some(Vec::new());

        let mut env = base.env;
        env.extend(self.env);
        // Shader caches and the like go to the sandbox's own tmpfs.
        set_env(&mut env, "HOME", "/tmp");
        // Whatever the session's environment said: the report comes from the library or not
        // at all.
        env.retain(|(key, _)| key != NOCLIP_STATUS_ENV && key != "LD_PRELOAD");
        if self.preload_clipboard_cut {
            // gamescope only: the library takes itself out of the environment once loaded.
            set_env(&mut env, "LD_PRELOAD", NOCLIP_PATH);
        }

        let mut seccomp = self.seccomp;
        for name in EXTRA_SYSCALLS {
            if !seccomp.extra_allow.iter().any(|have| have == name) {
                seccomp.extra_allow.push(name.into());
            }
        }

        let mut argv = vec!["gamescope".to_string()];
        argv.extend(self.args);
        argv.extend(["--".into(), INIT_PATH.into(), READY_ARG.into()]);
        Ok(SandboxSpec {
            mounts,
            network: Network::None,
            cwd: "/tmp".into(),
            limits: self.limits,
            policy: InitPolicy { exec: GAMESCOPE.into(), argv, env, landlock, seccomp },
        })
    }
}

/// A running nested compositor.
pub struct Session {
    running: Running,
    /// The inner display's number.
    pub display: u32,
    /// The inner display's socket, on the host.
    socket: PathBuf,
    dir: PathBuf,
    /// What is known about this compositor's clipboard bridge.
    pub clipboard: ClipboardCut,
    /// Tells a sweep of the cache that `dir` is in use (`running::sweep_leftovers`).
    _in_use: Option<crate::running::DirLock>,
    /// The switch and the lock of the output relays' [`Gate`].
    held: Arc<tokio::sync::watch::Sender<Flow>>,
    pass: Arc<tokio::sync::RwLock<()>>,
}

/// Remove terminal colour sequences (`ESC [ … letter`): gamescope colours its log even when
/// nobody is looking.
fn strip_ansi(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut bytes = raw.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if byte == 0x1b && bytes.peek() == Some(&b'[') {
            bytes.next();
            for b in bytes.by_ref() {
                if (0x40..=0x7e).contains(&b) {
                    break;
                }
            }
        } else {
            out.push(byte);
        }
    }
    out
}

/// Start-up chatter that says nothing about this launch: one line per bundled display
/// profile, one per pixel format.
fn is_routine(line: &str) -> bool {
    line.contains("[Info]") && (line.contains(" scriptmgr: ") || line.contains(" vulkan:   "))
        || line.ends_with("supported DRM formats for sampling usage:")
}

async fn relay(
    stream: impl AsyncRead + Unpin,
    sink: Sink,
    recent: Recent,
    mut ready: Option<tokio::sync::oneshot::Sender<String>>,
    mut gate: Gate,
) {
    let mut reader = BufReader::with_capacity(16 * 1024, stream);
    let mut buf = Vec::with_capacity(512);
    'lines: loop {
        buf.clear();
        // At most this much is taken for one line, however the reading is interrupted.
        let mut limited = (&mut reader).take(16 * 1024);
        let _pass = loop {
            // Not while the output is held: then nothing is read, and nothing is handled.
            // Never again once a hold was given up unreleased. (An error means that the
            // session — the one who could hold — is gone.)
            let flow = gate.held.wait_for(|flow| *flow != Flow::Held).await.map(|flow| *flow);
            if matches!(flow, Ok(Flow::Discarded)) {
                break 'lines;
            }
            let pass = gate.pass.clone().read_owned().await;
            let read = tokio::select! {
                biased;
                // A hold begins: step aside. What was read of the line so far stays in `buf`
                // (reading up to a delimiter may be interrupted and taken up again).
                Ok(_) = gate.held.wait_for(|flow| *flow != Flow::Open) => None,
                read = limited.read_until(b'\n', &mut buf) => Some(read),
            };
            match read {
                None => continue,
                // The end of the stream — unless part of a line was read before a hold.
                Some(Ok(0)) if buf.is_empty() => break 'lines,
                Some(Err(_)) => break 'lines,
                Some(Ok(_)) => break pass,
            }
        };
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        let line = util::sanitize_line(&strip_ansi(&buf), 400);
        if let Some(display) = line.strip_prefix(READY_PREFIX)
            && let Some(tx) = ready.take()
        {
            let _ = tx.send(display.to_string());
            continue;
        }
        if line.is_empty() || is_routine(&line) {
            continue;
        }
        if let Ok(mut recent) = recent.lock() {
            if recent.len() == RECENT_LINES {
                recent.pop_front();
            }
            recent.push_back(line.clone());
        }
        sink(format!("display server: {line}"));
    }
}

/// Start the compositor and wait until its inner display accepts clients.
pub async fn start(plan: Plan, launcher_exe: &Path, sink: Sink) -> Result<Session> {
    util::create_private_dir_all(&plan.dir)?;
    let dir = plan.dir.clone();
    let require_clipboard_cut = plan.require_clipboard_cut;
    let mut in_use = crate::running::lock_dir(&dir).ok();
    let (held, held_rx) = tokio::sync::watch::channel(Flow::Open);
    let (held, pass) = (Arc::new(held), Arc::new(tokio::sync::RwLock::new(())));
    let gate = Gate { held: held_rx, pass: pass.clone() };
    let started = async {
        let mut running = super::spawn(plan.spec()?, launcher_exe)?;
        let recent: Recent = Arc::default();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (out, err) = (running.child.stdout.take().expect("piped"), running.child.stderr.take().expect("piped"));
        tokio::spawn(relay(out, sink.clone(), recent.clone(), Some(tx), gate.clone()));
        tokio::spawn(relay(err, sink, recent.clone(), None, gate));

        let answer = tokio::select! {
            answer = tokio::time::timeout(STARTUP, rx) => match answer {
                Ok(Ok(display)) => Ok(display),
                Ok(Err(_)) => Err("it stopped before its display was ready".to_string()),
                Err(_) => Err(format!("its display was not ready after {} seconds", STARTUP.as_secs())),
            },
            status = running.child.wait() => Err(match status {
                Ok(status) => format!("it exited during start-up ({status})"),
                Err(e) => format!("waiting for it failed: {e}"),
            }),
        };
        let announced = match answer {
            Ok(announced) => announced,
            Err(why) => {
                let _ = running.stop(Duration::from_secs(2)).await;
                // The output tasks may still be draining; give the last lines a moment.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let last: Vec<String> = recent.lock().map(|r| r.iter().cloned().collect()).unwrap_or_default();
                return Err(Error::Sandbox(format!(
                    "the nested compositor (gamescope) did not start: {why}{}",
                    if last.is_empty() { String::new() } else { format!(". Its last output: {}", last.join(" | ")) }
                )));
            }
        };
        // "<display>", then what the cut-off library left for the ready child to pass on.
        let mut fields = announced.split_ascii_whitespace();
        let display = fields.next().unwrap_or_default().to_string();
        let clipboard = ClipboardCut::from_report(fields.find_map(|field| field.strip_prefix(NOCLIP_FIELD)));
        let Some(number) = display::x11_display_number(&display) else {
            let _ = running.stop(Duration::from_secs(2)).await;
            return Err(Error::Sandbox(format!("the nested compositor announced an unusable display {display:?}")));
        };
        let socket = dir.join(format!("X{number}"));
        if !display::is_socket(&socket) {
            let _ = running.stop(Duration::from_secs(2)).await;
            return Err(Error::Sandbox(format!(
                "the nested compositor announced display :{number}, but its socket did not appear"
            )));
        }
        if let (true, ClipboardCut::Unconfirmed(why)) = (require_clipboard_cut, &clipboard) {
            let _ = running.stop(Duration::from_secs(2)).await;
            return Err(Error::Sandbox(format!(
                "sandbox.nested_compositor = \"require\", but it is not established that this compositor's clipboard \
                 bridge is cut: {why}. With \"auto\" the launch continues and says that the game may read and set the \
                 session's clipboard."
            )));
        }
        Ok(Session {
            running,
            display: number,
            socket,
            dir: dir.clone(),
            clipboard,
            _in_use: in_use.take(),
            held: held.clone(),
            pass: pass.clone(),
        })
    }
    .await;
    if started.is_err() {
        drop(in_use);
        let _ = crate::tree::remove(&dir, Some(std::time::Instant::now() + Duration::from_secs(5)));
    }
    started
}

impl Session {
    /// Give the game's sandbox the inner display: one socket. No cookie is involved — the
    /// inner server admits its own user and nobody else, by the peer's credentials.
    pub fn attach(&self, mounts: &mut Vec<Mount>, sockets: &mut Vec<PathBuf>, env: &mut Vec<(String, String)>) {
        let dst = PathBuf::from(format!("{X11_DIR}/X{}", self.display));
        mounts.push(Mount::RoBind { src: self.socket.clone(), dst: dst.clone() });
        sockets.push(dst.clone());
        // The socket's path, not `:N`: for `:N` libxcb first tries the abstract socket
        // `@/tmp/.X11-unix/XN`, and gives up without trying the file if that is refused.
        // With the host's network namespace (`network.mode = "open"`) that abstract name
        // can belong to a display of the *session*: Landlock's abstract-socket scoping
        // refuses it, and the game would have no display at all. A path is only ever
        // opened as a file. (`tests/nested.rs` covers both namespaces.)
        set_env(env, "DISPLAY", &dst.display().to_string());
        set_env(env, "XDG_SESSION_TYPE", "x11");
    }

    /// The inner display's socket on the host (the clip recorder is pointed at it).
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Host process id of the compositor's sandbox.
    pub fn process_id(&self) -> Option<u32> {
        self.running.child.id()
    }

    /// Stop reading the compositor's output until the returned value is released (or, if it
    /// is dropped unreleased, for good). Returns once neither output relay is reading or
    /// handling a line.
    ///
    /// For the stretch between starting the game and the launcher confining itself: from
    /// the moment the game exists, what the compositor prints is no longer only its own.
    pub async fn hold_output(&self) -> OutputHold {
        self.held.send_replace(Flow::Held);
        // Granted when both relays have let go of theirs — which they do as soon as they
        // see the switch, also in the middle of waiting for output.
        let pass = self.pass.clone().write_owned().await;
        OutputHold { held: self.held.clone(), released: false, _pass: pass }
    }

    /// Resolves when the compositor is gone (its window was closed, or it crashed).
    pub async fn exited(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.running.child.wait().await
    }

    /// End the compositor: close its child's standard input, which is what that child waits
    /// for; insist if that is not enough.
    pub async fn stop(mut self) {
        let _ = self.running.channel.shutdown(Shutdown::Both);
        if tokio::time::timeout(Duration::from_secs(5), self.running.child.wait()).await.is_err() {
            let _ = self.running.stop(Duration::from_secs(3)).await;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The process itself is killed with its handle; this is what it leaves behind. It was
        // written by the compositor's sandbox, which the game talks to: removed the careful
        // way, and for a bounded time (what stays is swept by a later launch).
        let _ = crate::tree::remove(&self.dir, Some(std::time::Instant::now() + Duration::from_secs(5)));
    }
}

/// Entry point for `bastion-mc __display-ready`, the compositor's one child. Runs inside the
/// compositor's sandbox. Never returns.
pub fn ready_main() -> ! {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    // What the preloaded cut-off left in the compositor's environment (this process is the
    // compositor's child, and the library itself is not loaded here). Passed on only if it
    // is one plain word; its absence is a report too.
    let report = std::env::var(NOCLIP_STATUS_ENV)
        .ok()
        .filter(|word| (1..=32).contains(&word.len()) && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
        .map(|word| format!(" {NOCLIP_FIELD}{word}"))
        .unwrap_or_default();
    let mut out = std::io::stdout();
    let announced = writeln!(out, "{READY_PREFIX}{display}{report}").and_then(|_| out.flush()).is_ok();
    if announced {
        let mut stdin = std::io::stdin();
        let mut scratch = [0u8; 256];
        loop {
            match stdin.read(&mut scratch) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }
    // SAFETY: immediate process exit; nothing to unwind.
    unsafe { libc::_exit(if announced { 0 } else { 1 }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            mounts: vec![
                Mount::RoBind { src: "/tmp/.X11-unix/X0".into(), dst: "/tmp/.X11-unix/X0".into() },
                Mount::RoData { data: b"cookie".to_vec(), dst: "/run/user/1000/Xauthority".into(), mode: 0o400 },
                Mount::DevBind { src: "/dev/dri".into(), dst: "/dev/dri".into() },
            ],
            env: vec![
                ("DISPLAY".into(), ":0".into()),
                ("XAUTHORITY".into(), "/run/user/1000/Xauthority".into()),
                ("HOME".into(), "/somewhere/else".into()),
            ],
            sockets: vec!["/tmp/.X11-unix/X0".into()],
            devices: vec!["/dev/dri".into()],
            args: vec!["-W".into(), "1920".into(), "-H".into(), "1080".into()],
            dir: "/home/u/.cache/bastion-mc/display-00".into(),
            layers: Vec::new(),
            limits: None,
            seccomp: SeccompPolicy::default(),
            require_clipboard_cut: true,
            preload_clipboard_cut: true,
        }
    }

    #[test]
    fn only_the_report_active_counts_as_a_cut_clipboard() {
        assert_eq!(ClipboardCut::from_report(Some("active")), ClipboardCut::Active);
        for (word, says) in [
            (None, "was not loaded"),
            (Some("shadowed"), "defines SDL's clipboard functions itself"),
            (Some("no-sdl"), "does not get SDL's clipboard functions from the dynamic linker"),
            (Some("data-interface"), "arbitrary data"),
            (Some("ACTIVE"), "unknown"),
            (Some(""), "unknown"),
        ] {
            match ClipboardCut::from_report(word) {
                ClipboardCut::Unconfirmed(why) => assert!(why.contains(says), "{word:?}: {why}"),
                ClipboardCut::Active => panic!("{word:?} was taken for a cut clipboard"),
            }
        }
    }

    #[test]
    fn without_the_library_nothing_is_preloaded_and_no_report_can_be_inherited() {
        let mut plan = plan();
        plan.env.push((NOCLIP_STATUS_ENV.into(), "active".into()));
        plan.env.push(("LD_PRELOAD".into(), "/somewhere/else.so".into()));
        plan.preload_clipboard_cut = false;
        let spec = plan.spec().unwrap();
        assert!(spec.policy.env.iter().all(|(key, _)| key != "LD_PRELOAD" && key != NOCLIP_STATUS_ENV));
        assert!(!spec.mounts.iter().any(|m| matches!(m, Mount::RoData { dst, .. } if dst == Path::new(NOCLIP_PATH))));

        // With it: exactly the library, and still no report from outside.
        let mut plan = self::plan();
        plan.env.push((NOCLIP_STATUS_ENV.into(), "active".into()));
        let spec = plan.spec().unwrap();
        let env =
            |key: &str| spec.policy.env.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect::<Vec<_>>();
        assert_eq!(env("LD_PRELOAD"), [NOCLIP_PATH]);
        assert!(env(NOCLIP_STATUS_ENV).is_empty(), "the status can only come from the library itself");
    }

    #[test]
    fn the_compositor_gets_a_sandbox_with_no_network_and_only_its_own_programs() {
        let spec = plan().spec().unwrap();
        assert!(matches!(spec.network, Network::None));
        assert_eq!(spec.policy.exec, Path::new(GAMESCOPE));
        assert_eq!(
            spec.policy.argv,
            ["gamescope", "-W", "1920", "-H", "1080", "--", "/bastion/init", "__display-ready"],
            "user options first, then the one child"
        );
        let landlock = &spec.policy.landlock;
        for program in PROGRAMS.iter().chain([&INIT_PATH]) {
            assert!(landlock.read_exec.iter().any(|p| p == Path::new(program)), "{program} must be executable");
        }
        // The dynamic loader and the programs above; no JVM, no general /usr/bin.
        assert_eq!(landlock.read_exec.len(), PROGRAMS.len() + 2);
        assert_eq!(landlock.socket_dirs, [PathBuf::from("/tmp/.X11-unix"), PathBuf::from("/run/user/1000")]);
        assert!(matches!(&landlock.tcp, TcpPolicy::Ports(ports) if ports.is_empty()));
        assert!(landlock.scope_abstract_unix);
        assert!(landlock.devices.contains(&PathBuf::from("/dev/dri")));
        assert!(!landlock.write.iter().any(|p| p.starts_with("/home")));

        // The shared directory is mounted before the session's socket inside it.
        let position = |wanted: &str| {
            spec.mounts
                .iter()
                .position(|m| match m {
                    Mount::Bind { dst, .. } | Mount::RoBind { dst, .. } => dst == Path::new(wanted),
                    _ => false,
                })
                .unwrap_or_else(|| panic!("no mount at {wanted}"))
        };
        assert!(position("/tmp/.X11-unix") < position("/tmp/.X11-unix/X0"));
        assert!(
            matches!(&spec.mounts[position("/tmp/.X11-unix")], Mount::Bind { src, .. } if src.ends_with("display-00"))
        );
        assert!(matches!(spec.mounts.first(), Some(Mount::RoBind { dst, .. }) if dst == Path::new("/usr")));

        let env =
            |key: &str| spec.policy.env.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect::<Vec<_>>();
        assert_eq!(spec.policy.seccomp.extra_allow, ["setgid", "setuid"]);
        assert_eq!(env("HOME"), ["/tmp"], "exactly one HOME, inside the sandbox");
        assert_eq!(env("DISPLAY"), [":0"]);
        assert_eq!(env("XDG_RUNTIME_DIR"), ["/run/user/1000"]);
        assert!(env("DBUS_SESSION_BUS_ADDRESS").is_empty() && env("WAYLAND_DISPLAY").is_empty());
    }

    #[test]
    fn layers_replace_the_plain_usr_mount() {
        let mut plan = plan();
        plan.layers = vec!["/somewhere/gamescope/usr".into()];
        let spec = plan.spec().unwrap();
        let Some(Mount::RoOverlay { layers, dst }) = spec.mounts.first() else { panic!("expected an overlay") };
        assert_eq!(dst, Path::new("/usr"));
        assert_eq!(layers, &[PathBuf::from("/usr"), PathBuf::from("/somewhere/gamescope/usr")]);
    }

    #[test]
    fn colour_sequences_are_removed_before_logging() {
        let raw = b"[gamescope] [\x1b[0;34mInfo\x1b[0m]  \x1b[0;37mwlserver:\x1b[0m Starting Xwayland on :1";
        assert_eq!(strip_ansi(raw), b"[gamescope] [Info]  wlserver: Starting Xwayland on :1");
        assert_eq!(strip_ansi(b"plain"), b"plain");
        assert_eq!(strip_ansi(b"cut \x1b[0;3"), b"cut ", "an unterminated sequence swallows the rest, not memory");
        assert_eq!(strip_ansi(b"lone \x1b escape"), b"lone \x1b escape");
    }

    /// A gate nobody holds, and the two handles a [`Session`] keeps to hold it.
    fn open_gate() -> (Gate, Arc<tokio::sync::watch::Sender<Flow>>, Arc<tokio::sync::RwLock<()>>) {
        let (held, held_rx) = tokio::sync::watch::channel(Flow::Open);
        let pass = Arc::new(tokio::sync::RwLock::new(()));
        (Gate { held: held_rx, pass: pass.clone() }, Arc::new(held), pass)
    }

    /// What `Session::hold_output` does, for a relay that is not attached to a compositor.
    async fn hold(held: &Arc<tokio::sync::watch::Sender<Flow>>, pass: &Arc<tokio::sync::RwLock<()>>) -> OutputHold {
        held.send_replace(Flow::Held);
        OutputHold { held: held.clone(), released: false, _pass: pass.clone().write_owned().await }
    }

    /// Between the start of the game and the launcher's confinement nothing the compositor
    /// prints is read — not a line that arrives during the hold, and not the rest of a line
    /// whose beginning had already arrived. Afterwards nothing is missing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn held_output_is_not_read_until_it_is_released_and_nothing_is_lost() {
        use tokio::io::AsyncWriteExt;
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink: Sink = {
            let lines = lines.clone();
            Arc::new(move |line| lines.lock().unwrap().push(line))
        };
        let seen = || lines.lock().unwrap().clone();
        // A pipe of 64 bytes: what the relay has not read stays visible as back-pressure.
        let (mut compositor, launcher) = tokio::io::duplex(64);
        let (gate, held, pass) = open_gate();
        let task = tokio::spawn(relay(launcher, sink, Arc::default(), None, gate));

        compositor.write_all(b"before the game\nhalf a li").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(seen(), ["display server: before the game"]);

        // The relay is waiting for the rest of a line at this moment; the hold is granted all
        // the same, at once.
        let holding = tokio::time::timeout(Duration::from_secs(5), hold(&held, &pass)).await.expect("hold granted");
        compositor.write_all(b"ne\nprinted while held\n").await.unwrap();
        // More than the pipe holds: this write only completes if somebody reads.
        let mut flood = tokio::spawn(async move {
            compositor.write_all(&[b'x'; 200]).await.unwrap();
            compositor.write_all(b"\nlast\n").await.unwrap();
            compositor
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(seen(), ["display server: before the game"], "nothing is handled while the output is held");
        assert!(!flood.is_finished(), "nothing is even read: the writer is still waiting for room");

        holding.release();
        let compositor = tokio::time::timeout(Duration::from_secs(5), &mut flood).await.expect("resumed").unwrap();
        drop(compositor);
        tokio::time::timeout(Duration::from_secs(5), task).await.expect("the relay ends with its stream").unwrap();
        let seen = seen();
        assert_eq!(seen.len(), 5, "{seen:?}");
        assert_eq!(seen[1], "display server: half a line", "the interrupted line arrives whole");
        assert_eq!(seen[2], "display server: printed while held");
        assert_eq!(seen[3], format!("display server: {}", "x".repeat(200)));
        assert_eq!(seen[4], "display server: last");
    }

    /// A launch that gives up before the launcher has confined itself (step 9 refused) drops
    /// its hold unreleased: what the compositor printed meanwhile is never read — the relays
    /// stop instead of resuming.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hold_given_up_unreleased_discards_the_output_unread() {
        use tokio::io::AsyncWriteExt;
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink: Sink = {
            let lines = lines.clone();
            Arc::new(move |line| lines.lock().unwrap().push(line))
        };
        let (mut compositor, launcher) = tokio::io::duplex(64);
        let (gate, held, pass) = open_gate();
        let task = tokio::spawn(relay(launcher, sink, Arc::default(), None, gate));
        compositor.write_all(b"before the game\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let holding = tokio::time::timeout(Duration::from_secs(5), hold(&held, &pass)).await.expect("hold granted");
        compositor.write_all(b"printed while held\n").await.unwrap();
        drop(holding);
        tokio::time::timeout(Duration::from_secs(5), task).await.expect("the relay stops at once").unwrap();
        compositor.write_all(b"after\n").await.ok();
        assert_eq!(*lines.lock().unwrap(), ["display server: before the game"], "nothing held was read");
    }

    #[tokio::test]
    async fn the_ready_line_is_taken_once_and_everything_else_is_logged() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink: Sink = {
            let lines = lines.clone();
            Arc::new(move |line| lines.lock().unwrap().push(line))
        };
        let recent: Recent = Arc::default();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let input = format!(
            "starting\n\x1b[0;34mInfo\x1b[0m ok\n{READY_PREFIX}:7\n{READY_PREFIX}:9\n\n{}\n\
             [gamescope] [Info]  scriptmgr: Running script file 'x.lua' (id: 3)\n\
             [gamescope] [Info]  vulkan:   AR24 (0x34325241)\n",
            "x".repeat(1000)
        );
        relay(input.as_bytes(), sink, recent.clone(), Some(tx), open_gate().0).await;
        assert_eq!(rx.await.unwrap(), ":7");
        let lines = lines.lock().unwrap();
        assert_eq!(lines[0], "display server: starting");
        assert_eq!(lines[1], "display server: Info ok");
        assert_eq!(lines[2], format!("display server: {READY_PREFIX}:9"), "a second announcement is just output");
        assert!(lines[3].ends_with("…[truncated]") && lines[3].chars().count() < 450);
        assert_eq!(lines.len(), 4, "blank lines and routine start-up chatter are dropped");
        assert_eq!(recent.lock().unwrap().len(), 4);
    }

    #[test]
    fn a_missing_gamescope_is_reported_with_the_package_to_install() {
        // On a machine with gamescope installed this is simply Ok.
        if Path::new(GAMESCOPE).exists() {
            assert!(locate(&[]).is_ok() || locate(&[]).unwrap_err().contains("root"));
            return;
        }
        let why = locate(&[]).unwrap_err();
        assert!(why.contains("pacman -S gamescope"), "{why}");
    }
}
