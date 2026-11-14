//! The clip recorder: ffmpeg in a bubblewrap sandbox of its own, recording the game's window
//! into the replay buffer.
//!
//! **What it is given** ([`spec`]): read-only `/usr`, a private `/tmp`, no network (a network
//! namespace with nothing in it), no home directory — and exactly two things of the outside:
//! the socket of the display the game draws on, and the buffer directory, writable. It may
//! execute two programs: this one (as the helper below) and `/usr/bin/ffmpeg`. Landlock and
//! the seccomp policy of the game's sandbox apply (the same init stage starts it); with
//! resource limits on, it runs in a systemd scope of its own (memory, tasks, one CPU).
//!
//! **Which display.** With the nested compositor (gamescope), the game's own inner Xwayland
//! display: the game is the only application on it, so its one viewable window is the
//! game's. On X11 without the compositor, the session's display and its cookie: the window is
//! the one whose client is the game's process (the X-Resource extension says so, where the
//! server can see the process), else the one created inside the game's sandbox (it names the
//! sandbox's host name as its machine and process 2 as its owner). With a Wayland display
//! (a security context): not supported — nothing outside the compositor can read a
//! client's window there, and the compositor offers the recorder no capture interface.
//!
//! **The helper** ([`recorder_main`], `bastion-mc __clip-recorder`, inside the sandbox) finds
//! the window, runs ffmpeg on it (`x11grab -window_id`: the window's own pixels, not a region
//! of the screen, so nothing that overlaps it is recorded), and starts it again when the
//! window changes size (at most [`MAX_RESTARTS`] a minute). It reports on standard output,
//! one word-line at a time ([`Status`]); the launcher reads that only after step 9.
//!
//! What it records: the window's picture, at most `max_width` wide and 30 frames a second,
//! H.264 in one-second MPEG-TS segments. No sound: the game's sandbox has no audio by
//! default here, and the recorder captures none.

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{buffer, x11::X11};
use crate::{
    error::{Error, Result},
    sandbox::{
        self, BaseOptions, HOSTNAME, INIT_PATH, Mount, Network, RUNTIME_DIR, Resolv, Running, SandboxSpec, cgroup,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
};

/// `bastion-mc __clip-recorder <settings as JSON>`.
pub const RECORDER_ARG: &str = "__clip-recorder";
pub const FFMPEG: &str = "/usr/bin/ffmpeg";
/// Inside the sandbox.
const DISPLAY_IN_SANDBOX: &str = "/tmp/.X11-unix/X0";
const COOKIE_IN_SANDBOX: &str = "/run/user/1000/Xauthority";
const BUFFER_IN_SANDBOX: &str = "/buffer";
/// Restarts of ffmpeg within a minute before the helper gives up.
pub const MAX_RESTARTS: usize = 5;
/// The recorder's resource scope.
pub const MEMORY_MAX_MB: u64 = 1024;
pub const TASKS_MAX: u32 = 256;
pub const CPU_QUOTA_PERCENT: u32 = 100;

/// Which window to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// The only application on the display is the game (the nested compositor's display).
    Alone,
    /// The game is one client of the session's display; its process, as the host sees it.
    Among { game_pid: u32 },
}

/// What the helper is told (as JSON on its command line: paths inside the sandbox, numbers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub mode: Mode,
    pub fps: u32,
    pub max_width: u32,
    pub max_height: u32,
    /// Segments kept in the list (the buffer's length in seconds, plus two).
    pub list_size: u32,
    /// Segment files in rotation: more than the list holds, so that a listed segment is not
    /// overwritten while a save reads it.
    pub wrap: u32,
    /// Seconds between thumbnails.
    pub thumb_every: u32,
    pub cookie: bool,
}

impl Settings {
    pub fn new(mode: Mode, buffer_seconds: u32, fps: u32, max_width: u32) -> Settings {
        let list_size = buffer_seconds + 2;
        Settings {
            mode,
            fps,
            max_width,
            max_height: 1080,
            list_size,
            wrap: list_size + 4,
            thumb_every: 2,
            cookie: false,
        }
    }
}

/// What the helper reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// ffmpeg runs on the game's window, which is this large.
    Recording { width: u16, height: u16 },
    /// The game's window was not found (yet).
    Waiting,
    /// ffmpeg ended; the word says why. The helper tries again.
    Stopped(String),
    /// Too many restarts: nothing is recorded for the rest of the session.
    GaveUp,
}

impl Status {
    pub fn line(&self) -> String {
        match self {
            Status::Recording { width, height } => format!("recording {width} {height}"),
            Status::Waiting => "waiting".into(),
            Status::Stopped(word) => format!("stopped {word}"),
            Status::GaveUp => "gave-up".into(),
        }
    }

    /// One line as the helper writes it; anything else is not a status.
    pub fn parse(line: &[u8]) -> Option<Status> {
        let text = std::str::from_utf8(line).ok()?;
        let mut words = text.split(' ');
        let status = match (words.next()?, words.next(), words.next()) {
            ("recording", Some(w), Some(h)) => {
                let side = |s: &str| {
                    (s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u16>().ok()).flatten()
                };
                Status::Recording { width: side(w)?, height: side(h)? }
            }
            ("waiting", None, None) => Status::Waiting,
            ("stopped", Some(word), None)
                if (1..=32).contains(&word.len()) && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') =>
            {
                Status::Stopped(word.to_string())
            }
            ("gave-up", None, None) => Status::GaveUp,
            _ => return None,
        };
        words.next().is_none().then_some(status)
    }
}

// ---------------------------------------------------------------- the launcher's side

/// The display the recorder is pointed at.
pub enum Target {
    /// The nested compositor's inner display (the socket on the host).
    Nested(PathBuf),
    /// The session's X display, its cookie (an Xauthority file reduced to it), the game's PID.
    Session { socket: PathBuf, cookie: Option<Vec<u8>>, game_pid: u32 },
}

/// The recorder's sandbox.
pub fn spec(
    target: &Target,
    buffer: &Path,
    settings: &Settings,
    seccomp: SeccompPolicy,
    limits: Option<cgroup::Limits>,
) -> Result<SandboxSpec> {
    let exec = [PathBuf::from(FFMPEG), PathBuf::from(INIT_PATH)];
    let base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: &exec,
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })?;
    let mut mounts = base.mounts;
    let mut env = base.env;
    let mut settings = settings.clone();
    mounts.push(Mount::Dir { dst: "/tmp/.X11-unix".into(), mode: 0o755 });
    match target {
        Target::Nested(socket) => mounts.push(Mount::RoBind { src: socket.clone(), dst: DISPLAY_IN_SANDBOX.into() }),
        Target::Session { socket, cookie, .. } => {
            mounts.push(Mount::RoBind { src: socket.clone(), dst: DISPLAY_IN_SANDBOX.into() });
            if let Some(cookie) = cookie {
                mounts.push(Mount::RoData { data: cookie.clone(), dst: COOKIE_IN_SANDBOX.into(), mode: 0o400 });
                env.push(("XAUTHORITY".into(), COOKIE_IN_SANDBOX.into()));
                settings.cookie = true;
            }
        }
    }
    mounts.push(Mount::Bind { src: buffer.to_path_buf(), dst: BUFFER_IN_SANDBOX.into() });
    let mut landlock = base.landlock;
    landlock.write.push(BUFFER_IN_SANDBOX.into());
    landlock.unix_sockets.push(DISPLAY_IN_SANDBOX.into());
    landlock.tcp = TcpPolicy::Ports(Vec::new());
    landlock.udp_send = Some(Vec::new());
    debug_assert!(COOKIE_IN_SANDBOX.starts_with(RUNTIME_DIR));
    let json = serde_json::to_string(&settings).map_err(|e| Error::Sandbox(format!("recorder settings: {e}")))?;
    Ok(SandboxSpec {
        mounts,
        network: Network::None,
        cwd: BUFFER_IN_SANDBOX.into(),
        limits,
        policy: InitPolicy {
            exec: INIT_PATH.into(),
            argv: vec![INIT_PATH.into(), RECORDER_ARG.into(), json],
            env,
            landlock,
            seccomp,
        },
    })
}

/// A running recorder.
pub struct Recorder {
    pub running: Running,
}

impl Recorder {
    /// Start it (before step 9: this executes a program). Nothing it prints is read here.
    pub fn start(
        target: &Target,
        buffer: &Path,
        settings: &Settings,
        program: &Path,
        seccomp: SeccompPolicy,
        limits: Option<cgroup::Limits>,
    ) -> Result<Recorder> {
        if !Path::new(FFMPEG).exists() {
            return Err(Error::Sandbox("ffmpeg is not installed (pacman -S ffmpeg)".into()));
        }
        crate::util::require_root_owned(Path::new(FFMPEG))?;
        let mut settings = settings.clone();
        if let Target::Session { game_pid, .. } = target {
            settings.mode = Mode::Among { game_pid: *game_pid };
        }
        let running = sandbox::spawn(spec(target, buffer, &settings, seccomp, limits)?, program)?;
        Ok(Recorder { running })
    }
}

// ---------------------------------------------------------------- the helper, inside the sandbox

fn say(status: &Status) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{}", status.line()).and_then(|_| out.flush());
}

/// Has the launcher's end of standard input closed (or does it close within `wait`)?
fn launcher_gone(wait: Duration) -> bool {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::BorrowedFd;
    // SAFETY: fd 0 stays open for the life of this process.
    let stdin = unsafe { BorrowedFd::borrow_raw(0) };
    let mut fds = [PollFd::new(stdin, PollFlags::POLLIN)];
    let timeout = PollTimeout::try_from(wait.as_millis().min(60_000) as i32).unwrap_or(PollTimeout::NONE);
    match poll(&mut fds, timeout) {
        Ok(0) => false,
        Ok(_) => {
            // Readable: either data (there never is any) or the end.
            let mut byte = [0u8; 1];
            !matches!(nix::unistd::read(stdin, &mut byte), Ok(n) if n > 0)
        }
        Err(nix::errno::Errno::EINTR) => false,
        Err(_) => true,
    }
}

/// The game's window, if it can be seen now.
fn find_window(x: &mut X11, mode: Mode) -> std::result::Result<Option<super::x11::Window>, super::x11::X11Error> {
    let usable = |w: &super::x11::Window| {
        w.viewable && w.input_output && !w.override_redirect && w.width >= 16 && w.height >= 16
    };
    let root = x.root;
    let top: Vec<u32> = x.children(root)?;
    match mode {
        Mode::Alone => {
            let mut best: Option<super::x11::Window> = None;
            for id in top {
                let Ok(w) = x.window(id) else { continue };
                if usable(&w)
                    && best.is_none_or(|b| {
                        u32::from(w.width) * u32::from(w.height) > u32::from(b.width) * u32::from(b.height)
                    })
                {
                    best = Some(w);
                }
            }
            Ok(best)
        }
        Mode::Among { game_pid } => {
            let machine = x.atom(b"WM_CLIENT_MACHINE")?;
            let net_pid = x.atom(b"_NET_WM_PID")?;
            let mut candidates = Vec::new();
            for id in top {
                candidates.push(id);
                // A reparenting window manager puts the client inside a frame.
                if let Ok(children) = x.children(id) {
                    candidates.extend(children.into_iter().take(16));
                }
            }
            let mut found = None;
            for id in candidates {
                let Ok(w) = x.window(id) else { continue };
                if !usable(&w) {
                    continue;
                }
                let matches = match x.client_pid(id) {
                    Ok(Some(pid)) => pid == game_pid,
                    // The server cannot see processes: what the window says about itself.
                    _ => {
                        let host = x.property(id, machine).ok().flatten().map(|(_, v)| v);
                        let pid = x
                            .property(id, net_pid)
                            .ok()
                            .flatten()
                            .filter(|(format, v)| *format == 32 && v.len() == 4)
                            .map(|(_, v)| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
                        host.as_deref() == Some(HOSTNAME.as_bytes()) && pid == Some(2)
                    }
                };
                if matches
                    && found.is_none_or(|f: super::x11::Window| {
                        u32::from(w.width) * u32::from(w.height) > u32::from(f.width) * u32::from(f.height)
                    })
                {
                    found = Some(w);
                }
            }
            Ok(found)
        }
    }
}

/// ffmpeg's command line for one run.
pub fn ffmpeg_args(settings: &Settings, window: u32, run: u32) -> Vec<String> {
    let (fps, w, h) = (settings.fps, settings.max_width, settings.max_height);
    let filter = format!(
        "[0:v]scale=w='min(iw,{w})':h='min(ih,{h})':force_original_aspect_ratio=decrease:force_divisible_by=2,fps={fps},\
         format=yuv420p,split=2[rec][th];[th]fps=1/{every},scale=w='min(iw,320)':h='min(ih,320)':\
         force_original_aspect_ratio=decrease[thumb]",
        every = settings.thumb_every
    );
    // `-y`: a restarted run overwrites the previous run's thumbnail (without it ffmpeg would
    // ask, and with no input to ask on, exit).
    let mut args: Vec<String> =
        ["-hide_banner", "-nostdin", "-nostats", "-y", "-loglevel", "warning", "-f", "x11grab", "-draw_mouse", "0"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    args.extend(["-window_id".into(), format!("0x{window:x}"), "-framerate".into(), fps.to_string()]);
    args.extend(["-i".into(), DISPLAY_IN_SANDBOX.into(), "-filter_complex".into(), filter]);
    args.extend(
        [
            "-map",
            "[rec]",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-tune",
            "zerolatency",
            "-crf",
            "23",
            "-maxrate",
            "6M",
            "-bufsize",
            "12M",
            "-sc_threshold",
            "0",
            "-threads",
            "2",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args.extend(["-g".into(), fps.to_string(), "-keyint_min".into(), fps.to_string()]);
    args.extend(["-f", "segment", "-segment_time", "1"].iter().map(|s| s.to_string()));
    args.extend([
        "-segment_wrap".into(),
        settings.wrap.to_string(),
        "-segment_list_size".into(),
        settings.list_size.to_string(),
    ]);
    args.extend(
        [
            "-segment_list",
            &format!("{BUFFER_IN_SANDBOX}/{}", buffer::LIST),
            "-segment_list_type",
            "csv",
            "-reset_timestamps",
            "0",
            "-segment_format",
            "mpegts",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args.push(format!("{BUFFER_IN_SANDBOX}/r{run}_%03d.ts"));
    args.extend(
        ["-map", "[thumb]", "-c:v", "mjpeg", "-q:v", "5", "-f", "image2", "-update", "1", "-atomic_writing", "1"]
            .iter()
            .map(|s| s.to_string()),
    );
    args.push(format!("{BUFFER_IN_SANDBOX}/{}", buffer::THUMB));
    args
}

/// Remove what an earlier run left (its list and segments): a new run's list must not name
/// files the new run is about to overwrite.
fn clear_buffer() {
    let Ok(entries) = std::fs::read_dir(BUFFER_IN_SANDBOX) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if buffer::is_segment_name(&name) || name == buffer::LIST || name.starts_with(&format!("{}.", buffer::LIST)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn stop(child: &mut Child) {
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(child.id() as i32), nix::sys::signal::Signal::SIGINT);
    for _ in 0..30 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Entry point for `bastion-mc __clip-recorder`, inside the recorder's sandbox. Never returns.
pub fn recorder_main(args: &[OsString]) -> ! {
    let settings: Option<Settings> = args.first().and_then(|a| a.to_str()).and_then(|j| serde_json::from_str(j).ok());
    let Some(settings) = settings else {
        eprintln!("clip recorder: unreadable settings");
        std::process::exit(2)
    };
    let cookie = settings.cookie.then(|| Path::new(COOKIE_IN_SANDBOX));
    let mut restarts: Vec<Instant> = Vec::new();
    let mut run = 0u32;
    let mut waiting_said = false;
    'session: loop {
        if launcher_gone(Duration::ZERO) {
            break;
        }
        let mut x = match X11::connect(Path::new(DISPLAY_IN_SANDBOX), cookie) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("clip recorder: {e}");
                if launcher_gone(Duration::from_secs(2)) {
                    break;
                }
                continue;
            }
        };
        let window = match find_window(&mut x, settings.mode) {
            Ok(Some(window)) => window,
            Ok(None) | Err(_) => {
                if !waiting_said {
                    say(&Status::Waiting);
                    waiting_said = true;
                }
                if launcher_gone(Duration::from_secs(1)) {
                    break;
                }
                continue;
            }
        };
        waiting_said = false;
        restarts.retain(|t| t.elapsed() < Duration::from_secs(60));
        if restarts.len() >= MAX_RESTARTS {
            say(&Status::GaveUp);
            // Stay, doing nothing, until the launcher goes: its relay expects the process.
            while !launcher_gone(Duration::from_secs(60)) {}
            break;
        }
        restarts.push(Instant::now());
        run += 1;
        clear_buffer();
        let child = Command::new(FFMPEG)
            .args(ffmpeg_args(&settings, window.id, run))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => {
                eprintln!("clip recorder: starting ffmpeg: {e}");
                say(&Status::Stopped("no-ffmpeg".into()));
                if launcher_gone(Duration::from_secs(5)) {
                    break;
                }
                continue;
            }
        };
        say(&Status::Recording { width: window.width, height: window.height });
        let mut checked = Instant::now();
        loop {
            if launcher_gone(Duration::from_millis(500)) {
                stop(&mut child);
                break 'session;
            }
            if let Ok(Some(status)) = child.try_wait() {
                say(&Status::Stopped(if status.success() { "ended".into() } else { "failed".into() }));
                if launcher_gone(Duration::from_secs(2)) {
                    break 'session;
                }
                continue 'session;
            }
            if checked.elapsed() >= Duration::from_secs(2) {
                checked = Instant::now();
                // The same window, the same size — else record it afresh.
                let same = matches!(x.window(window.id), Ok(w) if w.width == window.width && w.height == window.height && w.viewable);
                if !same {
                    stop(&mut child);
                    say(&Status::Stopped("window-changed".into()));
                    continue 'session;
                }
            }
        }
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_round_trip_and_nothing_else_is_one() {
        for status in [
            Status::Recording { width: 480, height: 270 },
            Status::Waiting,
            Status::Stopped("window-changed".into()),
            Status::GaveUp,
        ] {
            assert_eq!(Status::parse(status.line().as_bytes()), Some(status));
        }
        for junk in [
            &b"recording 480"[..],
            b"recording 480 270 1",
            b"recording 99999999 1",
            b"stopped Window",
            b"stopped \x1b[2J",
            b"waiting now",
            b"",
            b"gave-up ",
        ] {
            assert_eq!(Status::parse(junk), None, "{}", String::from_utf8_lossy(junk));
        }
    }

    #[test]
    fn the_buffer_keeps_spare_segments_beyond_the_list() {
        let s = Settings::new(Mode::Alone, 30, 30, 1280);
        assert_eq!((s.list_size, s.wrap), (32, 36));
        let args = ffmpeg_args(&s, 0x40003d, 3);
        let at = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].clone());
        assert_eq!(at("-window_id").as_deref(), Some("0x40003d"));
        assert_eq!(at("-segment_wrap").as_deref(), Some("36"));
        assert_eq!(at("-segment_list_size").as_deref(), Some("32"));
        assert_eq!(at("-framerate").as_deref(), Some("30"));
        assert_eq!(at("-draw_mouse").as_deref(), Some("0"));
        assert!(args.contains(&"/buffer/r3_%03d.ts".to_string()));
        assert!(at("-filter_complex").unwrap().contains("min(iw,1280)"));
        assert!(!args.iter().any(|a| a.contains("pulse") || a.contains("alsa")), "no sound is captured");
        assert!(args.contains(&"-y".to_string()), "a restarted run must overwrite the last thumbnail");
    }
}
