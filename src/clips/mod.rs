//! Clips of the game, saved from the phone.
//!
//! Off by default (`[clips] enabled`): recording costs a CPU core. When on, a launch starts
//! the recorder (`recorder`: ffmpeg in a sandbox of its own, before step 9) on the game's
//! window, which keeps the last `buffer_seconds` in one-second segments in a buffer
//! directory (`$XDG_RUNTIME_DIR/bastion-mc/clips-<id>`, memory, removed at exit). The phone's
//! `save` joins the newest segments into one clip file in the clips folder — a confined
//! launcher executes nothing, and it needs nothing executed: transport-stream segments are
//! joined by concatenation (`buffer`). A thumbnail of the moment lies beside it, decoded and
//! re-encoded by the launcher (`spotify::picture`). The folder's index says which files the
//! feature made, and only those are ever removed (`library`).
//!
//! **Limits:** the buffer's length (at most 120 s), saves per minute, a cap on the number of
//! clips and on their total size (the oldest of this feature's clips go first), names
//! cleaned. **Not recorded:** sound (the game's sandbox has no audio by default here, and
//! the recorder takes none); anything with a Wayland display (not supported: say so).
//!
//! **What the game can do through the phone:** save clips (bounded), list them, see their
//! thumbnails, rename and delete them — only clips this feature made, only in the clips
//! folder. It never names a path: a clip is an id the launcher made up.

pub mod buffer;
pub mod library;
pub mod recorder;
pub mod script;
pub mod x11;

use std::{
    collections::VecDeque,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::{Value as Json, json};
use tokio::{io::AsyncBufReadExt, sync::mpsc};

use self::{
    library::{Clip, Library},
    recorder::{Recorder, Settings, Status, Target},
};
use crate::{
    config::Config,
    confine::Confinement,
    error::{Error, Result},
    paths::Paths,
    phone::{Command, FrameFormat, Outbox, PhoneApp, Service, VerbLimit},
    sandbox::{cgroup, policy::SeccompPolicy},
    spotify::picture,
    util,
};

/// The app's name on the wire.
pub const APP: &str = "clips";
/// The buffer directories' prefix.
const BUFFER_PREFIX: &str = "clips-";
/// Clips shown (and given thumbnail slots) in the phone's list.
pub const LISTED: usize = 32;
const THUMB_BOX: (u32, u32) = (320, 320);
/// How long a save waits for the segment being recorded to complete.
const FRESH_SEGMENT: Duration = Duration::from_millis(1500);

// ---------------------------------------------------------------- configuration

/// `[clips]` in config.toml.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipsConfig {
    /// Record the game while it runs, so that the phone can save the last moments.
    pub enabled: bool,
    /// How much is kept: 5 to 120 seconds.
    pub buffer_seconds: u32,
    /// Frames per second, at most 30.
    pub fps: u32,
    /// Width of the recording, at most 1280 (narrower windows keep their size).
    pub max_width: u32,
    /// Saves per minute, 1 to 6.
    pub saves_per_minute: u32,
    /// Clips kept; the oldest of this feature's clips go first.
    pub max_clips: u32,
    /// All clips together, in MiB.
    pub max_total_mb: u64,
    /// Where clips go (an absolute path). Default: `$XDG_DATA_HOME/bastion-mc/clips`.
    pub folder: Option<PathBuf>,
    /// Test support, deliberately not settable from a configuration file: phone command
    /// lines a launch sends to its apps itself after step 9, as if the phone had sent them,
    /// logging what comes back (`script`, `tests/clips_game.rs`).
    #[serde(skip)]
    pub script: Option<Vec<script::Step>>,
}

impl Default for ClipsConfig {
    fn default() -> Self {
        ClipsConfig {
            enabled: false,
            buffer_seconds: 30,
            fps: 30,
            max_width: 1280,
            saves_per_minute: 4,
            max_clips: 50,
            max_total_mb: 2048,
            folder: None,
            script: None,
        }
    }
}

impl ClipsConfig {
    pub fn validate(&self) -> Result<()> {
        let range = |name: &str, value: u64, lo: u64, hi: u64| {
            if (lo..=hi).contains(&value) {
                Ok(())
            } else {
                Err(Error::Config(format!("clips.{name} must be between {lo} and {hi}")))
            }
        };
        range("buffer_seconds", u64::from(self.buffer_seconds), 5, 120)?;
        range("fps", u64::from(self.fps), 5, 30)?;
        range("max_width", u64::from(self.max_width), 320, 1280)?;
        range("saves_per_minute", u64::from(self.saves_per_minute), 1, 6)?;
        range("max_clips", u64::from(self.max_clips), 1, 500)?;
        range("max_total_mb", self.max_total_mb, 16, 65_536)?;
        if let Some(folder) = &self.folder
            && (!folder.is_absolute() || folder.components().any(|c| matches!(c, std::path::Component::ParentDir)))
        {
            return Err(Error::Config("clips.folder must be an absolute path without `..`".into()));
        }
        Ok(())
    }

    /// The clips folder for these paths.
    pub fn folder(&self, paths: &Paths) -> PathBuf {
        self.folder.clone().unwrap_or_else(|| paths.data.join("clips"))
    }
}

// ---------------------------------------------------------------- before step 9

/// Where the launch log goes.
pub type Sink = Arc<dyn Fn(String) + Send + Sync>;

/// The display the game was given.
pub enum GameDisplay {
    /// The nested compositor's inner display: its socket on the host.
    Nested(PathBuf),
    /// The session's X display itself.
    X11 { socket: PathBuf, number: u32 },
    /// A Wayland display: not recordable from outside the compositor.
    Wayland,
}

/// What [`prepare`] needs to know of the launch.
pub struct Setup<'a> {
    pub cfg: &'a Config,
    pub paths: &'a Paths,
    pub program: &'a Path,
    pub instance: &'a str,
    pub display: GameDisplay,
    /// The game's process on the host (the session-display case finds its window by it).
    pub game_pid: Option<u32>,
    /// Resource scopes work on this machine (the launch's own limits are on).
    pub scopes: bool,
    pub log: Sink,
}

/// The service as prepared before step 9.
pub struct Prepared {
    recorder: Option<Recorder>,
    buffer: Option<Buffer>,
    library_dir: PathBuf,
    note: String,
    cfg: ClipsConfig,
    instance: String,
    log: Sink,
}

/// The buffer directory and its lock (a sweep of a later launch leaves a locked one alone).
struct Buffer {
    dir: PathBuf,
    _lock: crate::running::DirLock,
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // The segments go; the directory itself (whose parent a confined launcher may not
        // write) is swept by the next launch.
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// Remove buffer directories that no running launch holds and that are older than a minute.
fn sweep(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(BUFFER_PREFIX) || !path.is_dir() || path.is_symlink() {
            continue;
        }
        let old = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(60));
        let free = std::fs::File::open(&path)
            .ok()
            .is_some_and(|dir| nix::fcntl::Flock::lock(dir, nix::fcntl::FlockArg::LockExclusiveNonblock).is_ok());
        if old && free {
            let _ = crate::tree::remove(&path, Some(Instant::now() + Duration::from_secs(5)));
        }
    }
}

/// The session display's cookie, reduced as for the game (`sandbox::display`).
fn session_cookie(number: u32) -> Option<Vec<u8>> {
    let source = std::env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".Xauthority")))?;
    let raw = util::read_small_file(&source, 256 * 1024).ok()?;
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    Some(crate::sandbox::display::filter_xauthority(&raw, number, host.trim())).filter(|c| !c.is_empty())
}

/// Shown to the phone instead of the folder's real path (which names the user).
fn folder_shown(folder: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|h| folder.strip_prefix(&h).ok().map(|rest| rest.to_path_buf())) {
        Some(rest) => format!("~/{}", util::sanitize_line(rest.as_os_str().as_encoded_bytes(), 120)),
        None => format!(
            "…/{}",
            util::sanitize_line(folder.file_name().map(|n| n.as_encoded_bytes()).unwrap_or(b"clips"), 60)
        ),
    }
}

/// Prepare the service (after the game was started, before step 9): `None` when clips are
/// switched off.
pub fn prepare(setup: Setup<'_>) -> Option<Box<dyn Service>> {
    let cfg = setup.cfg.clips.clone();
    if !cfg.enabled {
        return None;
    }
    let library_dir = cfg.folder(setup.paths);
    if let Err(e) = util::create_private_dir_all(&library_dir) {
        (setup.log)(format!("clips: off — the clips folder cannot be used: {e}"));
        return None;
    }
    let started = (|| -> std::result::Result<(Recorder, Buffer, String), String> {
        let target = match setup.display {
            GameDisplay::Wayland => {
                return Err("recording is not supported with a Wayland display: nothing outside the compositor can \
                            read the game's window there"
                    .into());
            }
            GameDisplay::Nested(socket) => Target::Nested(socket),
            GameDisplay::X11 { socket, number } => Target::Session {
                socket,
                cookie: session_cookie(number),
                game_pid: setup.game_pid.ok_or("the game's process could not be identified")?,
            },
        };
        let how = match &target {
            Target::Nested(_) => "the game's window on the nested compositor's display",
            Target::Session { .. } => "the game's window on the session's X display",
        };
        let base = setup
            .paths
            .runtime
            .as_deref()
            .map(|r| r.join(crate::paths::APP))
            .unwrap_or_else(|| setup.paths.cache.clone());
        util::create_private_dir_all(&base).map_err(|e| e.to_string())?;
        sweep(&base);
        let dir = base.join(format!(
            "{BUFFER_PREFIX}{}",
            util::hex_encode(&util::random_bytes::<6>().map_err(|e| e.to_string())?)
        ));
        util::create_private_dir_all(&dir).map_err(|e| e.to_string())?;
        let lock = crate::running::lock_dir(&dir).map_err(|e| e.to_string())?;
        let buffer = Buffer { dir, _lock: lock };
        let settings = Settings::new(recorder::Mode::Alone, cfg.buffer_seconds, cfg.fps, cfg.max_width);
        let seccomp = SeccompPolicy {
            mode: setup.cfg.sandbox.seccomp,
            tripwire: setup.cfg.sandbox.tripwire,
            extra_allow: Vec::new(),
            ..Default::default()
        };
        let limits = if setup.scopes {
            Some(
                cgroup::Limits::new(&format!("{}-clips", setup.instance), recorder::MEMORY_MAX_MB, recorder::TASKS_MAX)
                    .map_err(|e| e.to_string())?
                    .with_cpu_quota(Some(recorder::CPU_QUOTA_PERCENT)),
            )
        } else {
            None
        };
        let scoped = limits.is_some();
        let recorder = Recorder::start(&target, &buffer.dir, &settings, setup.program, seccomp, limits)
            .map_err(|e| e.to_string())?;
        Ok((
            recorder,
            buffer,
            format!(
                "recording {how} into a {} s buffer ({} fps, at most {} wide, no sound){}",
                cfg.buffer_seconds,
                cfg.fps,
                cfg.max_width,
                if scoped { "; in a resource scope of its own (1 GiB, one CPU), asked for" } else { "" }
            ),
        ))
    })();
    let (recorder, buffer, note) = match started {
        Ok((recorder, buffer, note)) => (Some(recorder), Some(buffer), note),
        Err(why) => (None, None, format!("not recording — {why}")),
    };
    Some(Box::new(Prepared {
        recorder,
        buffer,
        library_dir,
        note,
        cfg,
        instance: setup.instance.to_string(),
        log: setup.log,
    }))
}

impl Service for Prepared {
    fn name(&self) -> &'static str {
        APP
    }

    fn describe(&self) -> String {
        format!("clips: {}; folder {}", self.note, folder_shown(&self.library_dir))
    }

    fn confinement_needs(&self, keep: &mut Confinement) {
        // Saving reads the buffer and writes the folder; nothing else.
        keep.write.push(self.library_dir.clone());
        if let Some(buffer) = &self.buffer {
            keep.write.push(buffer.dir.clone());
        }
    }

    fn into_app(self: Box<Self>) -> Box<dyn PhoneApp> {
        let mut prepared = *self;
        let (tx, rx) = mpsc::channel(32);
        // The recorder's status on a channel of its own: its reader must not keep the app
        // alive (the app ending is what stops the recorder).
        let (status_tx, status_rx) = mpsc::channel(16);
        // The recorder's output: read only now, after step 9.
        if let Some(recorder) = prepared.recorder.as_mut() {
            if let Some(stdout) = recorder.running.child.stdout.take() {
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut lines = tokio::io::BufReader::new(stdout).take(1 << 20).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if let Some(status) = Status::parse(line.as_bytes())
                            && status_tx.send(status).await.is_err()
                        {
                            break;
                        }
                    }
                });
            }
            if let Some(stderr) = recorder.running.child.stderr.take() {
                let log = prepared.log.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut lines = tokio::io::BufReader::new(stderr).take(4 << 20).lines();
                    let mut shown = 0;
                    while let Ok(Some(line)) = lines.next_line().await {
                        if shown < 200 {
                            shown += 1;
                            log(format!("clip recorder: {}", util::sanitize_line(line.as_bytes(), 300)));
                        }
                    }
                });
            }
        }
        let library = Library::open(&prepared.library_dir);
        if let Some(note) = &library.note {
            (prepared.log)(format!("clips: {note}"));
        }
        let actor = Actor {
            library,
            buffer: prepared.buffer.as_ref().map(|b| b.dir.clone()),
            status: if prepared.recorder.is_some() { "starting" } else { "off" },
            window: None,
            error: prepared.recorder.is_none().then(|| prepared.note.clone()),
            saves: VecDeque::new(),
            listed: Vec::new(),
            out: None,
            folder: folder_shown(&prepared.library_dir),
            cfg: prepared.cfg.clone(),
            instance: prepared.instance.clone(),
            log: prepared.log.clone(),
            _keep: (prepared.recorder.take(), prepared.buffer.take()),
        };
        tokio::spawn(run(actor, rx, status_rx));
        Box::new(ClipsApp { tx })
    }
}

/// The phone app over an existing buffer directory, without a recorder (tests: segments
/// made beforehand stand in for a recording). A launch cannot reach this.
#[doc(hidden)]
pub fn app_over_buffer(cfg: ClipsConfig, folder: &Path, buffer: &Path, instance: &str, log: Sink) -> Box<dyn PhoneApp> {
    let (tx, rx) = mpsc::channel(32);
    let library = Library::open(folder);
    let actor = Actor {
        library,
        buffer: Some(buffer.to_path_buf()),
        status: "recording",
        window: None,
        error: None,
        saves: VecDeque::new(),
        listed: Vec::new(),
        out: None,
        folder: folder_shown(folder),
        cfg,
        instance: instance.to_string(),
        log,
        _keep: (None, None),
    };
    tokio::spawn(run(actor, rx, mpsc::channel(1).1));
    Box::new(ClipsApp { tx })
}

// ---------------------------------------------------------------- after step 9

/// How often the phone may use each verb (`save` is bounded again by `saves_per_minute`).
pub const LIMITS: &[VerbLimit] = &[
    VerbLimit { verb: "status", per_minute: 60, burst: 10 },
    VerbLimit { verb: "save", per_minute: 6, burst: 2 },
    VerbLimit { verb: "list", per_minute: 30, burst: 5 },
    VerbLimit { verb: "delete", per_minute: 20, burst: 5 },
    VerbLimit { verb: "rename", per_minute: 20, burst: 5 },
    VerbLimit { verb: "thumb", per_minute: 120, burst: 32 },
];

enum Input {
    Hello(Outbox),
    Command(Command, Outbox),
}

struct ClipsApp {
    tx: mpsc::Sender<Input>,
}

impl PhoneApp for ClipsApp {
    fn name(&self) -> &'static str {
        APP
    }

    fn limits(&self) -> &'static [VerbLimit] {
        LIMITS
    }

    fn hello(&mut self, out: &Outbox) {
        let _ = self.tx.try_send(Input::Hello(out.clone()));
    }

    fn command(&mut self, command: Command, out: &Outbox) {
        if self.tx.try_send(Input::Command(command, out.clone())).is_err() {
            out.notice(APP, "busy — try again in a moment");
        }
    }
}

struct Actor {
    library: Library,
    buffer: Option<PathBuf>,
    /// "starting", "recording", "waiting" (for the window), "stopped", "off".
    status: &'static str,
    window: Option<(u16, u16)>,
    error: Option<String>,
    saves: VecDeque<Instant>,
    /// Ids of the last list sent, in order: thumbnail slots.
    listed: Vec<String>,
    out: Option<Outbox>,
    folder: String,
    cfg: ClipsConfig,
    instance: String,
    log: Sink,
    /// The recorder and the buffer live as long as the app.
    _keep: (Option<Recorder>, Option<Buffer>),
}

/// Until the app's handle is dropped (the end of the session): then the actor, and with it
/// the recorder and the buffer, go.
async fn run(mut actor: Actor, mut inputs: mpsc::Receiver<Input>, mut statuses: mpsc::Receiver<Status>) {
    loop {
        let input = tokio::select! {
            input = inputs.recv() => match input {
                Some(input) => input,
                None => break,
            },
            Some(status) = statuses.recv() => {
                actor.recorder_status(status);
                continue;
            }
        };
        match input {
            Input::Hello(out) => {
                actor.out = Some(out);
                actor.send_state();
            }
            Input::Command(command, out) => {
                actor.out = Some(out);
                match actor.command(&command).await {
                    Ok(Some(text)) => {
                        actor.error = None;
                        actor.notice(&text);
                    }
                    Ok(None) => actor.error = None,
                    Err(why) => {
                        actor.notice(&why);
                        actor.error = Some(why);
                    }
                }
                actor.send_state();
            }
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `YYYY-MM-DD HH-MM-SS` (UTC) of a unix time.
fn stamp(unix: u64) -> String {
    let (days, secs) = (unix / 86_400, unix % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}-{:02}-{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// Everything of a save that touches files: on a blocking thread. (Public for the
/// confined-child test: this is what a confined launcher does when the phone says `save`.)
#[doc(hidden)]
pub fn save_blocking(
    buffer: &Path,
    folder: &Path,
    seconds: f64,
    name: &str,
    id: &str,
) -> std::result::Result<Clip, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let open_dir = |path: &Path| {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|e| format!("cannot open {} ({e})", path.display()))
    };
    let buffer_dir = open_dir(buffer)?;
    // The segment being recorded completes within a second: wait for it, so that the clip
    // ends at the moment of the save, not up to a second before.
    let before = buffer::read_list(&buffer_dir).ok().and_then(|l| l.last().cloned());
    let deadline = Instant::now() + FRESH_SEGMENT;
    let mut list = Vec::new();
    while Instant::now() < deadline {
        match buffer::read_list(&buffer_dir) {
            Ok(l) if !l.is_empty() && (before.is_none() || l.last() != before.as_ref()) => {
                list = l;
                break;
            }
            Ok(l) => list = l,
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if list.is_empty() {
        list = buffer::read_list(&buffer_dir)?;
    }
    let picked = buffer::pick(&list, seconds);
    if picked.is_empty() {
        return Err("nothing has been recorded yet".into());
    }
    let covered: f64 = picked.iter().map(buffer::Segment::seconds).sum();
    let stem = library::stem(name, id);
    let (file, partial) = (format!("{stem}.ts"), format!(".{stem}.ts.part"));
    let folder_dir = open_dir(folder)?;
    let mut out = File::from(
        nix::fcntl::openat(
            &folder_dir,
            partial.as_str(),
            nix::fcntl::OFlag::O_WRONLY
                | nix::fcntl::OFlag::O_CREAT
                | nix::fcntl::OFlag::O_EXCL
                | nix::fcntl::OFlag::O_NOFOLLOW
                | nix::fcntl::OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::from_bits_truncate(0o600),
        )
        .map_err(|e| format!("creating the clip: {e}"))?,
    );
    let joined = buffer::join(&buffer_dir, picked, &mut out).and_then(|bytes| {
        out.sync_all().map_err(|e| e.to_string())?;
        nix::fcntl::renameat2(
            &folder_dir,
            partial.as_str(),
            &folder_dir,
            file.as_str(),
            nix::fcntl::RenameFlags::RENAME_NOREPLACE,
        )
        .map_err(|e| format!("naming the clip: {e}"))?;
        Ok(bytes)
    });
    let bytes = match joined {
        Ok(bytes) => bytes,
        Err(why) => {
            let _ = nix::unistd::unlinkat(&folder_dir, partial.as_str(), nix::unistd::UnlinkatFlags::NoRemoveDir);
            return Err(why);
        }
    };
    // The thumbnail: the recorder's latest, decoded and encoded again here.
    let thumb = (|| -> Option<String> {
        let file = util::openat_regular_nofollow(&buffer_dir, std::ffi::OsStr::new(buffer::THUMB)).ok()?;
        use std::io::Read;
        let mut jpeg = Vec::new();
        file.take(library::MAX_THUMB_BYTES).read_to_end(&mut jpeg).ok()?;
        let picture = picture::jpeg_to_png(&jpeg, THUMB_BOX.0, THUMB_BOX.1).ok()?;
        let name = format!("{stem}.png");
        let fd = nix::fcntl::openat(
            &folder_dir,
            name.as_str(),
            nix::fcntl::OFlag::O_WRONLY
                | nix::fcntl::OFlag::O_CREAT
                | nix::fcntl::OFlag::O_EXCL
                | nix::fcntl::OFlag::O_NOFOLLOW
                | nix::fcntl::OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::from_bits_truncate(0o600),
        )
        .ok()?;
        use std::io::Write;
        File::from(fd).write_all(&picture.png).ok()?;
        Some(name)
    })();
    Ok(Clip {
        id: id.to_string(),
        name: name.to_string(),
        file,
        thumb,
        created: now_unix(),
        seconds: (covered * 10.0).round() / 10.0,
        bytes,
    })
}

impl Actor {
    fn recorder_status(&mut self, status: Status) {
        let (word, window) = match &status {
            Status::Recording { width, height } => ("recording", Some((*width, *height))),
            Status::Waiting => ("waiting", None),
            Status::Stopped(_) => ("stopped", None),
            Status::GaveUp => ("off", None),
        };
        if matches!(status, Status::GaveUp) {
            self.error = Some("the recorder stopped for this session (it failed too often)".into());
        }
        (self.log)(format!("clip recorder: {}", status.line()));
        self.status = word;
        self.window = window;
        self.send_state();
    }

    fn state(&self) -> Json {
        let listed = self.library.newest(LISTED);
        json!({
            "recording": self.status == "recording",
            "recorder": self.status,
            "window": self.window.map(|(w, h)| json!([w, h])),
            "buffer_seconds": self.cfg.buffer_seconds,
            "clips": listed.iter().map(|c| json!({
                "id": c.id,
                "name": c.name,
                "created": c.created,
                "seconds": c.seconds,
                "bytes": c.bytes,
            })).collect::<Vec<_>>(),
            "count": self.library.clips.len(),
            "total_bytes": self.library.total_bytes(),
            "folder": self.folder,
            "error": self.error,
        })
    }

    fn send_state(&mut self) {
        self.listed = self.library.newest(LISTED).iter().map(|c| c.id.clone()).collect();
        if let Some(out) = &self.out {
            out.state(APP, &self.state());
        }
    }

    fn notice(&self, text: &str) {
        if let Some(out) = &self.out {
            out.notice(APP, text);
        }
    }

    fn id_argument(command: &Command) -> std::result::Result<String, String> {
        match command.args.as_slice() {
            [id] if library::is_id(id) => Ok(id.clone()),
            _ => Err(format!("{} takes a clip id", command.verb)),
        }
    }

    async fn command(&mut self, command: &Command) -> std::result::Result<Option<String>, String> {
        match command.verb.as_str() {
            "status" | "list" => {
                if !command.args.is_empty() || command.text.is_some() {
                    return Err(format!("{} takes no arguments", command.verb));
                }
                if command.verb == "list" {
                    // From disk again: another launch may have saved meanwhile.
                    let dir = self.library.dir.clone();
                    if let Ok(library) = tokio::task::spawn_blocking(move || Library::open(&dir)).await
                        && library.note.is_none()
                    {
                        self.library = library;
                    }
                }
                Ok(None)
            }
            "save" => self.save(command).await.map(Some),
            "delete" => {
                let id = Self::id_argument(command)?;
                if command.text.is_some() {
                    return Err("delete takes only a clip id".into());
                }
                let mut library = std::mem::replace(
                    &mut self.library,
                    Library { dir: PathBuf::new(), clips: Vec::new(), note: None },
                );
                let (library, outcome) = tokio::task::spawn_blocking(move || {
                    let outcome = library.delete(&id);
                    (library, outcome)
                })
                .await
                .map_err(|_| "deleting failed".to_string())?;
                self.library = library;
                outcome.map(|clip| Some(format!("deleted {}", clip.name)))
            }
            "rename" => {
                let id = Self::id_argument(command)?;
                let text = command.text.clone().ok_or("rename takes a clip id and a name: rename <id> -- <name>")?;
                let mut library = std::mem::replace(
                    &mut self.library,
                    Library { dir: PathBuf::new(), clips: Vec::new(), note: None },
                );
                let (library, outcome) = tokio::task::spawn_blocking(move || {
                    let outcome = library.rename(&id, &text);
                    (library, outcome)
                })
                .await
                .map_err(|_| "renaming failed".to_string())?;
                self.library = library;
                outcome.map(|clip| Some(format!("renamed to {}", clip.name)))
            }
            "thumb" => {
                let id = Self::id_argument(command)?;
                let slot = self.listed.iter().position(|l| *l == id).ok_or("that clip is not in the last list")?;
                let clip = self.library.find(&id).cloned().ok_or("no such clip")?;
                let name = clip.thumb.ok_or("that clip has no thumbnail")?;
                let path = self.library.dir.join(name);
                let png = tokio::task::spawn_blocking(move || util::read_small_file(&path, library::MAX_THUMB_BYTES))
                    .await
                    .map_err(|_| "reading the thumbnail failed".to_string())?
                    .map_err(|e| format!("the thumbnail cannot be read ({e})"))?;
                // Read back from disk, where another program may have replaced it: only a
                // PNG that says it fits the box is sent.
                let (w, h) = picture::png_dimensions(&png)
                    .filter(|(w, h)| *w <= THUMB_BOX.0 && *h <= THUMB_BOX.1)
                    .ok_or("the thumbnail is not one this feature made")?;
                if let Some(out) = &self.out {
                    out.frame(APP, slot as u8, w as u16, h as u16, FrameFormat::Png, png);
                }
                Ok(None)
            }
            other => Err(format!("unknown verb {other}")),
        }
    }

    async fn save(&mut self, command: &Command) -> std::result::Result<String, String> {
        let buffer = self.buffer.clone().ok_or_else(|| self.error.clone().unwrap_or_else(|| "not recording".into()))?;
        if command.text.is_some() || command.args.len() > 1 {
            return Err("save takes at most a number of seconds".into());
        }
        let max = self.cfg.buffer_seconds;
        let seconds = match command.args.first() {
            None => max,
            Some(word) => match (word.len() <= 3 && word.bytes().all(|b| b.is_ascii_digit()))
                .then(|| word.parse::<u32>().ok())
                .flatten()
            {
                Some(n) if (1..=max).contains(&n) => n,
                _ => return Err(format!("save takes 1 to {max} seconds")),
            },
        };
        let now = Instant::now();
        while self.saves.front().is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60)) {
            self.saves.pop_front();
        }
        if self.saves.len() >= self.cfg.saves_per_minute as usize {
            return Err(format!("at most {} saves a minute", self.cfg.saves_per_minute));
        }
        self.saves.push_back(now);
        let id = util::hex_encode(&util::random_bytes::<6>().map_err(|e| e.to_string())?);
        let name =
            library::clean_name(&format!("{} {}", self.instance, stamp(now_unix()))).unwrap_or_else(|| "clip".into());
        let folder = self.library.dir.clone();
        let (max_clips, max_bytes) = (self.cfg.max_clips as usize, self.cfg.max_total_mb.saturating_mul(1 << 20));
        let mut library =
            std::mem::replace(&mut self.library, Library { dir: PathBuf::new(), clips: Vec::new(), note: None });
        let (library, outcome) = tokio::task::spawn_blocking(move || {
            let outcome = save_blocking(&buffer, &folder, f64::from(seconds), &name, &id).and_then(|clip| {
                library.clips.push(clip.clone());
                library.save()?;
                let removed = library.enforce(max_clips, max_bytes);
                Ok((clip, removed))
            });
            (library, outcome)
        })
        .await
        .map_err(|_| "saving failed".to_string())?;
        self.library = library;
        let (clip, removed) = outcome?;
        for r in &removed {
            match r {
                Ok(name) => (self.log)(format!("clips: removed the oldest clip, {name}, to stay within the limits")),
                Err(why) => (self.log)(format!("clips: {why}")),
            }
        }
        Ok(format!("saved {} ({:.1} s)", clip.name, clip.seconds))
    }
}

/// Every parser of what the recorder's sandbox writes (the segment list, the segments, its
/// status lines) and of the clips index, on arbitrary bytes.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    buffer::fuzz(data);
    library::fuzz(data);
    for line in data.split(|b| *b == b'\n') {
        if let Some(status) = Status::parse(line) {
            assert_eq!(Status::parse(status.line().as_bytes()), Some(status), "a status line reads back");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_is_bounded() {
        let mut c = ClipsConfig::default();
        assert!(!c.enabled, "off by default: recording costs a CPU core");
        c.validate().unwrap();
        for (field, value) in [
            ("buffer_seconds", 121u64),
            ("buffer_seconds", 4),
            ("fps", 31),
            ("max_width", 1281),
            ("saves_per_minute", 7),
        ] {
            let mut c = ClipsConfig::default();
            match field {
                "buffer_seconds" => c.buffer_seconds = value as u32,
                "fps" => c.fps = value as u32,
                "max_width" => c.max_width = value as u32,
                _ => c.saves_per_minute = value as u32,
            }
            assert!(c.validate().is_err(), "{field} = {value}");
        }
        c.folder = Some("relative/clips".into());
        assert!(c.validate().is_err());
        c.folder = Some("/home/u/../etc".into());
        assert!(c.validate().is_err());
        c.folder = Some("/home/u/Videos/minecraft".into());
        c.validate().unwrap();
    }

    #[test]
    fn stamps_and_shown_folders_name_nobody() {
        assert_eq!(stamp(1_763_047_800), "2025-11-13 15-30-00");
        assert_eq!(stamp(0), "1970-01-01 00-00-00");
        assert_eq!(folder_shown(Path::new("/mnt/data/alex/clips")), "…/clips");
        if let Some(home) = std::env::var_os("HOME") {
            let shown = folder_shown(&PathBuf::from(&home).join(".local/share/bastion-mc/clips"));
            assert_eq!(shown, "~/.local/share/bastion-mc/clips");
        }
    }

    #[test]
    fn the_vocabulary_is_the_contracts() {
        let mut verbs: Vec<&str> = LIMITS.iter().map(|l| l.verb).collect();
        verbs.sort_unstable();
        assert_eq!(verbs, ["delete", "list", "rename", "save", "status", "thumb"]);
        assert!(
            LIMITS.iter().find(|l| l.verb == "thumb").unwrap().burst as usize >= LISTED,
            "a whole list's thumbnails"
        );
    }
}
