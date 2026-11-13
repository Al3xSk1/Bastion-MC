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
//! How the launcher learns the inner display: gamescope starts one child once its display
//! is ready. That child is this program again in a tiny mode ([`READY_ARG`]): it prints the
//! `DISPLAY` it was given and then waits for its standard input to close. Closing it is how
//! the launcher ends the compositor.

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
const STARTUP: Duration = Duration::from_secs(30);
const RECENT_LINES: usize = 12;

/// Where the compositor's own output goes (the launch log).
pub type Sink = Arc<dyn Fn(String) + Send + Sync>;
type Recent = Arc<Mutex<VecDeque<String>>>;

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

        let mut landlock = base.landlock;
        landlock.devices.extend(self.devices);
        landlock.unix_sockets.extend(self.sockets);
        // A display server creates sockets: the inner X display, its own Wayland socket.
        landlock.socket_dirs = vec![X11_DIR.into(), RUNTIME_DIR.into()];
        landlock.tcp = TcpPolicy::Ports(vec![]);

        let mut env = base.env;
        env.extend(self.env);
        // Shader caches and the like go to the sandbox's own tmpfs.
        set_env(&mut env, "HOME", "/tmp");

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
) {
    let mut reader = BufReader::with_capacity(16 * 1024, stream);
    let mut buf = Vec::with_capacity(512);
    loop {
        buf.clear();
        let mut limited = (&mut reader).take(16 * 1024);
        match limited.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
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
    }
}

/// Start the compositor and wait until its inner display accepts clients.
pub async fn start(plan: Plan, launcher_exe: &Path, sink: Sink) -> Result<Session> {
    util::create_private_dir_all(&plan.dir)?;
    let dir = plan.dir.clone();
    let started = async {
        let mut running = super::spawn(plan.spec()?, launcher_exe)?;
        let recent: Recent = Arc::default();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(relay(running.child.stdout.take().expect("piped"), sink.clone(), recent.clone(), Some(tx)));
        tokio::spawn(relay(running.child.stderr.take().expect("piped"), sink, recent.clone(), None));

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
        let display = match answer {
            Ok(display) => display,
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
        Ok(Session { running, display: number, socket, dir: dir.clone() })
    }
    .await;
    if started.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    started
}

impl Session {
    /// Give the game's sandbox the inner display: one socket. No cookie is involved — the
    /// inner server admits its own user and nobody else, by the peer's credentials.
    pub fn attach(&self, mounts: &mut Vec<Mount>, sockets: &mut Vec<PathBuf>, env: &mut Vec<(String, String)>) {
        let dst = PathBuf::from(format!("{X11_DIR}/X{}", self.display));
        mounts.push(Mount::RoBind { src: self.socket.clone(), dst: dst.clone() });
        sockets.push(dst);
        set_env(env, "DISPLAY", &format!(":{}", self.display));
        set_env(env, "XDG_SESSION_TYPE", "x11");
    }

    /// Host process id of the compositor's sandbox.
    pub fn process_id(&self) -> Option<u32> {
        self.running.child.id()
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
        // The process itself is killed with its handle; this is what it leaves behind.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Entry point for `bastion-mc __display-ready`, the compositor's one child. Runs inside the
/// compositor's sandbox. Never returns.
pub fn ready_main() -> ! {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let mut out = std::io::stdout();
    let announced = writeln!(out, "{READY_PREFIX}{display}").and_then(|_| out.flush()).is_ok();
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
        }
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
        relay(input.as_bytes(), sink, recent.clone(), Some(tx)).await;
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
