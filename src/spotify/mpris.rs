//! The Spotify desktop app over MPRIS — through `xdg-dbus-proxy`, never on the session bus
//! directly.
//!
//! The session bus is where the user's programs talk to each other: password managers,
//! portals (screen capture among them), `systemd --user` (which starts programs). A process
//! with a connection to it can reach all of that. So the launcher never connects to it.
//! Before step 9 it starts `xdg-dbus-proxy` in a bubblewrap sandbox of its own ([`Proxy`]),
//! given the bus's socket and nothing else, with a filter that lets a client:
//!
//! * call methods of `org.mpris.MediaPlayer2.Player` and `org.freedesktop.DBus.Properties`
//!   on `/org/mpris/MediaPlayer2` of the name `org.mpris.MediaPlayer2.spotify`, and nothing
//!   of any other name;
//! * receive that name's `PropertiesChanged` and `Seeked` signals on that path;
//! * make the calls to the bus itself that the proxy always allows (`Hello`, `AddMatch`,
//!   `GetNameOwner`, …), which see only that name.
//!
//! The launcher connects to the proxy's socket once, keeps the connection for the session and
//! removes the socket's name, all before step 9. A program the proxy is told to start: none
//! (`NO_AUTO_START` on every call; the filter would refuse activation of other names anyway).
//!
//! What the player says is untrusted: whoever owns the name on the bus can send anything
//! (`dbus` parses strictly; titles and the like are cleaned and cut here). The art URL is
//! never fetched as it is: only the image id is taken from it (`art_id`).

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use super::dbus::{self, Connection, Message, Value};
use crate::{
    error::{Error, Result},
    sandbox::{
        self, BaseOptions, INIT_PATH, Mount, Network, RUNTIME_DIR, Resolv, Running, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
    util,
};

/// The bus name of the Spotify desktop app.
pub const PLAYER: &str = "org.mpris.MediaPlayer2.spotify";
pub const OBJECT: &str = "/org/mpris/MediaPlayer2";
pub const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const BUS: &str = "org.freedesktop.DBus";
const BUS_OBJECT: &str = "/org/freedesktop/DBus";
/// The proxy, as installed by the distribution (`pacman -S xdg-dbus-proxy`, a dependency of
/// flatpak).
pub const PROXY_PROGRAM: &str = "/usr/bin/xdg-dbus-proxy";
/// Inside the proxy's sandbox.
const BUS_IN_SANDBOX: &str = "/run/user/1000/bus";
const LISTEN_DIR_IN_SANDBOX: &str = "/run/user/1000/proxy";
const SOCKET_NAME: &str = "bus";
const DIR_PREFIX: &str = "mpris-";
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// Lines of the proxy's own output that reach the launch log.
const MAX_PROXY_LINES: usize = 20;

/// The session bus's socket, from its address (`DBUS_SESSION_BUS_ADDRESS`), or the systemd
/// default `$XDG_RUNTIME_DIR/bus`. Only `unix:path=` addresses: an abstract or TCP address
/// cannot be handed to a sandbox as a single file.
pub fn session_bus(address: Option<&str>, runtime: Option<&Path>) -> std::result::Result<PathBuf, String> {
    let path = match address.filter(|a| !a.is_empty()) {
        Some(address) => {
            // Several addresses may be listed (`;`); the first usable one is what a client uses.
            let first = address.split(';').next().unwrap_or_default();
            let Some(rest) = first.strip_prefix("unix:") else {
                return Err(format!("the session bus address is not a UNIX socket ({})", clip(first)));
            };
            let path = rest.split(',').find_map(|kv| kv.strip_prefix("path=")).ok_or_else(|| {
                format!(
                    "the session bus address names no socket file (abstract sockets cannot be sandboxed): {}",
                    clip(first)
                )
            })?;
            PathBuf::from(unescape(path).ok_or("the session bus address is malformed")?)
        }
        None => runtime.ok_or("there is no session bus (no DBUS_SESSION_BUS_ADDRESS, no XDG_RUNTIME_DIR)")?.join("bus"),
    };
    if !path.is_absolute() {
        return Err("the session bus socket is not an absolute path".into());
    }
    use std::os::unix::fs::FileTypeExt;
    match std::fs::metadata(&path) {
        Ok(meta) if meta.file_type().is_socket() => Ok(path),
        Ok(_) => Err(format!("{} is not a socket", path.display())),
        Err(e) => Err(format!("no session bus at {} ({e})", path.display())),
    }
}

fn clip(text: &str) -> String {
    util::sanitize_line(text.as_bytes(), 80)
}

/// D-Bus addresses escape bytes as `%xx`.
fn unescape(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().filter(|s| !s.contains('\0'))
}

/// The proxy's command line after the program name: ready notification on its standard
/// input, the bus, where to listen, and the filter.
pub fn proxy_argv() -> Vec<String> {
    vec![
        "xdg-dbus-proxy".into(),
        // Writes one byte to fd 0 when it listens; exits when fd 0 is closed.
        "--fd=0".into(),
        format!("unix:path={BUS_IN_SANDBOX}"),
        format!("{LISTEN_DIR_IN_SANDBOX}/{SOCKET_NAME}"),
        "--filter".into(),
        format!("--call={PLAYER}={PLAYER_INTERFACE}.*@{OBJECT}"),
        format!("--call={PLAYER}={PROPERTIES}.*@{OBJECT}"),
        format!("--broadcast={PLAYER}={PROPERTIES}.PropertiesChanged@{OBJECT}"),
        format!("--broadcast={PLAYER}={PLAYER_INTERFACE}.Seeked@{OBJECT}"),
    ]
}

/// Where the proxy's output goes (the launch log).
pub type Sink = std::sync::Arc<dyn Fn(String) + Send + Sync>;

/// A running proxy.
pub struct Proxy {
    running: Running,
    socket: PathBuf,
    dir: PathBuf,
}

/// The proxy's sandbox: the bus socket, a directory to listen in, nothing else.
fn spec(bus: &Path, dir: &Path, seccomp: SeccompPolicy) -> Result<SandboxSpec> {
    let exec = [PathBuf::from(PROXY_PROGRAM), PathBuf::from(INIT_PATH)];
    let base = sandbox::base(&BaseOptions {
        tmp_size_mb: 16,
        jvm: None,
        exec_allow: &exec,
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })?;
    let mut mounts = base.mounts;
    mounts.push(Mount::RoBind { src: bus.to_path_buf(), dst: BUS_IN_SANDBOX.into() });
    mounts.push(Mount::Bind { src: dir.to_path_buf(), dst: LISTEN_DIR_IN_SANDBOX.into() });
    let mut landlock = base.landlock;
    landlock.unix_sockets.push(BUS_IN_SANDBOX.into());
    landlock.socket_dirs.push(LISTEN_DIR_IN_SANDBOX.into());
    landlock.tcp = TcpPolicy::Ports(Vec::new());
    landlock.udp_send = Some(Vec::new());
    debug_assert!(BUS_IN_SANDBOX.starts_with(RUNTIME_DIR));
    let env = base.env;
    Ok(SandboxSpec {
        mounts,
        network: Network::None,
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy { exec: PROXY_PROGRAM.into(), argv: proxy_argv(), env, landlock, seccomp },
    })
}

/// Remove proxy directories of earlier launches that are gone (no live socket can be in an
/// unlocked directory older than a minute: the name is removed right after connecting).
fn sweep(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let old = entry
            .metadata()
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(60));
        if name.starts_with(DIR_PREFIX) && old {
            let _ = std::fs::remove_file(entry.path().join(SOCKET_NAME));
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

impl Proxy {
    /// Start the proxy for the bus at `bus`, listening in a new private directory under
    /// `base` (the runtime directory: socket paths are limited to 107 bytes), and wait until
    /// it listens.
    pub async fn start(bus: &Path, base: &Path, program: &Path, seccomp: SeccompPolicy, sink: Sink) -> Result<Proxy> {
        util::require_root_owned(Path::new(PROXY_PROGRAM))
            .map_err(|e| Error::Sandbox(format!("xdg-dbus-proxy is needed for Spotify over MPRIS: {e}")))?;
        sweep(base);
        let dir = base.join(format!("{DIR_PREFIX}{}", util::hex_encode(&util::random_bytes::<6>()?)));
        let socket = dir.join(SOCKET_NAME);
        if socket.as_os_str().len() > 107 {
            return Err(Error::Sandbox(format!("{} is too long for a socket", socket.display())));
        }
        util::create_private_dir_all(&dir)?;
        let started = async {
            let mut running = sandbox::spawn(spec(bus, &dir, seccomp)?, program)?;
            for stream in [running.child.stdout.take().map(Either::Out), running.child.stderr.take().map(Either::Err)]
                .into_iter()
                .flatten()
            {
                tokio::spawn(relay(stream, sink.clone()));
            }
            let mut ready = tokio::net::UnixStream::from_std({
                let clone = running.channel.try_clone().map_err(|e| Error::io("the proxy's channel", e))?;
                clone.set_nonblocking(true).map_err(|e| Error::io("the proxy's channel", e))?;
                clone
            })
            .map_err(|e| Error::io("the proxy's channel", e))?;
            let mut byte = [0u8; 1];
            use tokio::io::AsyncReadExt;
            let signalled = tokio::select! {
                read = tokio::time::timeout(START_TIMEOUT, ready.read(&mut byte)) => match read {
                    Ok(Ok(1)) => Ok(()),
                    Ok(Ok(_)) => Err("it ended before it was ready".to_string()),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_) => Err(format!("it was not ready after {} seconds", START_TIMEOUT.as_secs())),
                },
                status = running.child.wait() => Err(match status {
                    Ok(status) => format!("it exited during start-up ({status})"),
                    Err(e) => e.to_string(),
                }),
            };
            if let Err(why) = signalled {
                let _ = running.stop(Duration::from_secs(2)).await;
                return Err(Error::Sandbox(format!("xdg-dbus-proxy did not start: {why}")));
            }
            Ok(Proxy { running, socket: socket.clone(), dir: dir.clone() })
        }
        .await;
        if started.is_err() {
            let _ = std::fs::remove_file(&socket);
            let _ = std::fs::remove_dir(&dir);
        }
        started
    }

    /// Connect to the proxy (once per session; before step 9).
    pub async fn connect(&self) -> Result<tokio::net::UnixStream> {
        tokio::net::UnixStream::connect(&self.socket).await.map_err(|e| Error::io("connecting to xdg-dbus-proxy", e))
    }

    /// Remove the socket's name. Nothing needs it once the launcher is connected, and no
    /// other program should find a second way in. Must happen before step 9: a confined
    /// launcher cannot remove anything in the runtime directory.
    pub fn release_name(&self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir(&self.dir);
    }

    /// Whether the proxy process is still there.
    pub fn alive(&mut self) -> bool {
        matches!(self.running.child.try_wait(), Ok(None))
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        // Closing our end of its standard input is what ends it (`--fd=0`); the process
        // handle kills it besides.
        let _ = self.running.channel.shutdown(std::net::Shutdown::Both);
        self.release_name();
    }
}

enum Either {
    Out(tokio::process::ChildStdout),
    Err(tokio::process::ChildStderr),
}

async fn relay(stream: Either, sink: Sink) {
    async fn lines(stream: impl tokio::io::AsyncRead + Unpin, sink: Sink) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
        let mut reader = BufReader::new(stream);
        let (mut buf, mut shown) = (Vec::new(), 0usize);
        loop {
            buf.clear();
            match (&mut reader).take(4096).read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if shown < MAX_PROXY_LINES {
                shown += 1;
                sink(format!("spotify: xdg-dbus-proxy: {}", util::sanitize_line(buf.trim_ascii_end(), 300)));
            }
        }
    }
    match stream {
        Either::Out(s) => lines(s, sink).await,
        Either::Err(s) => lines(s, sink).await,
    }
}

// ---------------------------------------------------------------- the player

/// Text from the player, cleaned for the phone and the log: no control characters, no
/// bidirectional overrides, at most `max` characters.
pub fn clean_text(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
        })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Playback {
    Playing,
    Paused,
    #[default]
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Off,
    Track,
    Context,
}

impl Repeat {
    pub fn word(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::Track => "track",
            Repeat::Context => "context",
        }
    }

    pub fn parse(word: &str) -> Option<Repeat> {
        match word {
            "off" => Some(Repeat::Off),
            "track" => Some(Repeat::Track),
            "context" => Some(Repeat::Context),
            _ => None,
        }
    }

    fn from_loop_status(status: &str) -> Option<Repeat> {
        match status {
            "None" => Some(Repeat::Off),
            "Track" => Some(Repeat::Track),
            "Playlist" => Some(Repeat::Context),
            _ => None,
        }
    }

    fn loop_status(self) -> &'static str {
        match self {
            Repeat::Off => "None",
            Repeat::Track => "Track",
            Repeat::Context => "Playlist",
        }
    }
}

/// What the player said about itself, cleaned.
#[derive(Debug, Clone, Default)]
pub struct Player {
    pub playback: Playback,
    /// `mpris:trackid`, needed for `SetPosition`.
    pub track_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub length_ms: Option<u64>,
    /// The position when it was last read, and when that was.
    pub position_ms: Option<(u64, Instant)>,
    pub volume: Option<u8>,
    pub shuffle: Option<bool>,
    pub repeat: Option<Repeat>,
    /// The id of the cover image (`art_id`), never the URL as the player gave it.
    pub art: Option<String>,
}

/// The Spotify image id in an art URL: `https://i.scdn.co/image/<hex>` (current clients) or
/// `https://open.spotify.com/image/<hex>` (older ones). Anything else: none.
pub fn art_id(url: &str) -> Option<String> {
    let id =
        url.strip_prefix("https://i.scdn.co/image/").or_else(|| url.strip_prefix("https://open.spotify.com/image/"))?;
    ((16..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| id.to_string())
}

impl Player {
    /// Apply properties of `org.mpris.MediaPlayer2.Player` (all of them from `GetAll`, or the
    /// changed ones from `PropertiesChanged`). Values of the wrong type are ignored.
    pub fn apply(&mut self, properties: &[(&str, &Value)], now: Instant) {
        for (name, value) in properties {
            match *name {
                "PlaybackStatus" => {
                    self.playback = match value.as_str() {
                        Some("Playing") => Playback::Playing,
                        Some("Paused") => Playback::Paused,
                        _ => Playback::Stopped,
                    }
                }
                "Metadata" => {
                    if let Some(entries) = value.as_dict() {
                        self.apply_metadata(&entries);
                    }
                }
                "Position" => {
                    if let Some(us) = value.as_i64().filter(|us| *us >= 0) {
                        self.position_ms = Some(((us / 1000) as u64, now));
                    }
                }
                "Volume" => {
                    if let Some(v) = value.as_f64() {
                        self.volume = Some((v.clamp(0.0, 1.0) * 100.0).round() as u8);
                    }
                }
                "Shuffle" => self.shuffle = value.as_bool().or(self.shuffle),
                "LoopStatus" => self.repeat = value.as_str().and_then(Repeat::from_loop_status).or(self.repeat),
                _ => {}
            }
        }
    }

    fn apply_metadata(&mut self, entries: &[(&str, &Value)]) {
        let get = |key: &str| entries.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        self.track_id =
            get("mpris:trackid").and_then(Value::as_str).filter(|p| dbus::is_object_path(p)).map(str::to_string);
        self.title = get("xesam:title").and_then(Value::as_str).map(|t| clean_text(t, 200)).unwrap_or_default();
        self.album = get("xesam:album").and_then(Value::as_str).map(|t| clean_text(t, 200)).unwrap_or_default();
        self.artist = get("xesam:artist")
            .and_then(Value::as_str_list)
            .map(|names| clean_text(&names.into_iter().take(8).collect::<Vec<_>>().join(", "), 200))
            .unwrap_or_default();
        self.length_ms = get("mpris:length").and_then(Value::as_i64).filter(|us| *us > 0).map(|us| (us / 1000) as u64);
        self.art = get("mpris:artUrl").and_then(Value::as_str).and_then(art_id);
        // A new track starts at its beginning until the player says otherwise.
        self.position_ms = None;
    }

    /// The position now: the last one read, advanced by the time since if playing.
    pub fn position_now(&self, now: Instant) -> Option<u64> {
        let (at, when) = self.position_ms?;
        let advanced = match self.playback {
            Playback::Playing => at + now.saturating_duration_since(when).as_millis() as u64,
            _ => at,
        };
        Some(self.length_ms.map_or(advanced, |len| advanced.min(len)))
    }
}

// ---------------------------------------------------------------- the client

/// A connection to the player through the proxy.
pub struct Mpris {
    connection: Connection,
}

/// What a signal means for the client.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The player's name has a new owner (the app started), or none (it quit).
    Owner(Option<String>),
    /// Changed properties of the player.
    Changed(Vec<(String, Value)>),
    /// The position jumped (µs).
    Seeked(i64),
}

impl Mpris {
    /// Authenticate, register for the player's signals. The returned receiver gets every
    /// signal; [`Mpris::interpret`] says what one means.
    pub async fn open(stream: tokio::net::UnixStream) -> Result<(Mpris, mpsc::Receiver<Message>)> {
        let uid = nix::unistd::getuid().as_raw();
        let (connection, signals) = Connection::open(stream, uid).await?;
        let rules = [
            format!("type='signal',sender='{BUS}',interface='{BUS}',member='NameOwnerChanged',arg0='{PLAYER}'"),
            format!(
                "type='signal',sender='{PLAYER}',path='{OBJECT}',interface='{PROPERTIES}',member='PropertiesChanged'"
            ),
            format!("type='signal',sender='{PLAYER}',path='{OBJECT}',interface='{PLAYER_INTERFACE}',member='Seeked'"),
        ];
        for rule in rules {
            connection.call(Message::call(BUS, BUS_OBJECT, BUS, "AddMatch", vec![Value::Str(rule)])).await?;
        }
        Ok((Mpris { connection }, signals))
    }

    pub fn closed(&self) -> Option<String> {
        self.connection.closed()
    }

    /// The connection itself (tests: what the proxy lets through).
    #[doc(hidden)]
    pub fn raw(&self) -> &Connection {
        &self.connection
    }

    /// The unique name that owns the player's name, if the app is running.
    pub async fn owner(&self) -> Result<Option<String>> {
        match self
            .connection
            .call(Message::call(BUS, BUS_OBJECT, BUS, "GetNameOwner", vec![Value::Str(PLAYER.into())]))
            .await
        {
            Ok(reply) => {
                Ok(reply.body.first().and_then(Value::as_str).filter(|n| dbus::is_bus_name(n)).map(str::to_string))
            }
            Err(Error::Other(e)) if e.contains("NameHasNoOwner") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every property of the player.
    pub async fn properties(&self) -> Result<Vec<(String, Value)>> {
        let reply = self
            .connection
            .call(Message::call(PLAYER, OBJECT, PROPERTIES, "GetAll", vec![Value::Str(PLAYER_INTERFACE.into())]))
            .await?;
        let entries = reply
            .body
            .first()
            .and_then(Value::as_dict)
            .ok_or_else(|| Error::Format("MPRIS: GetAll did not answer with a{sv}".into()))?;
        Ok(entries.into_iter().map(|(k, v)| (k.to_string(), v.unvariant().clone())).collect())
    }

    /// The current position (µs).
    pub async fn position(&self) -> Result<Option<i64>> {
        let reply = self
            .connection
            .call(Message::call(
                PLAYER,
                OBJECT,
                PROPERTIES,
                "Get",
                vec![Value::Str(PLAYER_INTERFACE.into()), Value::Str("Position".into())],
            ))
            .await?;
        Ok(reply.body.first().and_then(Value::as_i64))
    }

    async fn player_call(&self, member: &str, body: Vec<Value>) -> Result<()> {
        self.connection.call(Message::call(PLAYER, OBJECT, PLAYER_INTERFACE, member, body)).await.map(|_| ())
    }

    async fn set(&self, property: &str, value: Value) -> Result<()> {
        self.connection
            .call(Message::call(
                PLAYER,
                OBJECT,
                PROPERTIES,
                "Set",
                vec![Value::Str(PLAYER_INTERFACE.into()), Value::Str(property.into()), Value::Variant(Box::new(value))],
            ))
            .await
            .map(|_| ())
    }

    pub async fn play(&self) -> Result<()> {
        self.player_call("Play", vec![]).await
    }
    pub async fn pause(&self) -> Result<()> {
        self.player_call("Pause", vec![]).await
    }
    pub async fn toggle(&self) -> Result<()> {
        self.player_call("PlayPause", vec![]).await
    }
    pub async fn next(&self) -> Result<()> {
        self.player_call("Next", vec![]).await
    }
    pub async fn previous(&self) -> Result<()> {
        self.player_call("Previous", vec![]).await
    }

    /// Jump to `ms` in the current track: `SetPosition` with its id (what Spotify wants), or
    /// a relative `Seek` when the id is unknown.
    pub async fn seek(&self, ms: u64, player: &Player) -> Result<()> {
        let us = i64::try_from(ms.saturating_mul(1000)).unwrap_or(i64::MAX);
        match &player.track_id {
            Some(id) => self.player_call("SetPosition", vec![Value::Path(id.clone()), Value::I64(us)]).await,
            None => {
                let now = player.position_now(Instant::now()).unwrap_or(0) as i64 * 1000;
                self.player_call("Seek", vec![Value::I64(us - now)]).await
            }
        }
    }

    pub async fn volume(&self, percent: u8) -> Result<()> {
        self.set("Volume", Value::Double(f64::from(percent.min(100)) / 100.0)).await
    }

    pub async fn shuffle(&self, on: bool) -> Result<()> {
        self.set("Shuffle", Value::Bool(on)).await
    }

    pub async fn repeat(&self, mode: Repeat) -> Result<()> {
        self.set("LoopStatus", Value::Str(mode.loop_status().into())).await
    }

    /// `OpenUri` with a URI the caller has checked (`uri::parse`).
    pub async fn open_uri(&self, uri: &str) -> Result<()> {
        self.player_call("OpenUri", vec![Value::Str(uri.into())]).await
    }

    /// What a signal means, if it is one of the three the client asked for and comes from
    /// whom it must: name changes from the bus itself, the rest from the player's current
    /// owner (`owner`).
    pub fn interpret(signal: &Message, owner: Option<&str>) -> Option<Event> {
        let (interface, member) = (signal.interface.as_deref()?, signal.member.as_deref()?);
        match (interface, member) {
            (BUS, "NameOwnerChanged") if signal.sender.as_deref() == Some(BUS) => {
                let [name, _old, new] = signal.body.as_slice() else { return None };
                if name.as_str() != Some(PLAYER) {
                    return None;
                }
                let new = new.as_str()?;
                Some(Event::Owner((!new.is_empty() && dbus::is_bus_name(new)).then(|| new.to_string())))
            }
            (PROPERTIES, "PropertiesChanged") if owner.is_some() && signal.sender.as_deref() == owner => {
                if signal.path.as_deref() != Some(OBJECT) {
                    return None;
                }
                let [iface, changed, _invalidated] = signal.body.as_slice() else { return None };
                if iface.as_str() != Some(PLAYER_INTERFACE) {
                    return None;
                }
                let entries = changed.as_dict()?;
                Some(Event::Changed(entries.into_iter().map(|(k, v)| (k.to_string(), v.unvariant().clone())).collect()))
            }
            (PLAYER_INTERFACE, "Seeked") if owner.is_some() && signal.sender.as_deref() == owner => {
                Some(Event::Seeked(signal.body.first()?.as_i64()?))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_socket_file_counts_as_the_session_bus() {
        let dir =
            std::env::temp_dir().join(format!("bmc-bus-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("bus");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let address = format!("unix:path={},guid=0123", socket.display());
        assert_eq!(session_bus(Some(&address), None).unwrap(), socket);
        assert_eq!(session_bus(None, Some(&dir)).unwrap(), socket, "the systemd default");
        assert_eq!(session_bus(Some(""), Some(&dir)).unwrap(), socket);
        let escaped = format!("unix:path={}", socket.display().to_string().replace('-', "%2d"));
        assert_eq!(session_bus(Some(&escaped), None).unwrap(), socket);
        for refused in
            ["unix:abstract=/tmp/dbus-x", "tcp:host=localhost,port=1", "unix:path=relative/bus", "unix:path=%zz"]
        {
            assert!(session_bus(Some(refused), None).is_err(), "{refused}");
        }
        std::fs::write(dir.join("file"), b"").unwrap();
        assert!(session_bus(Some(&format!("unix:path={}", dir.join("file").display())), None).is_err());
        assert!(session_bus(None, None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_filter_names_the_player_and_its_object_only() {
        let argv = proxy_argv();
        assert_eq!(argv[1], "--fd=0");
        assert!(argv.contains(&"--filter".to_string()));
        for rule in argv.iter().filter(|a| a.starts_with("--call=") || a.starts_with("--broadcast=")) {
            assert!(rule.contains(&format!("={PLAYER}=")) && rule.ends_with(&format!("@{OBJECT}")), "{rule}");
        }
        assert!(!argv.iter().any(|a| a.starts_with("--talk") || a.starts_with("--own") || a.starts_with("--see")));
    }

    fn metadata(title: &str, art: &str) -> Value {
        Value::string_dict(vec![
            ("mpris:trackid".into(), Value::Path("/com/spotify/track/4uLU6hMCjMI75M1A2tKUQC".into())),
            ("mpris:length".into(), Value::U64(213_000_000)),
            ("mpris:artUrl".into(), Value::Str(art.into())),
            ("xesam:title".into(), Value::Str(title.into())),
            ("xesam:album".into(), Value::Str("Discovery".into())),
            (
                "xesam:artist".into(),
                Value::Array {
                    element: "s".into(),
                    items: vec![Value::Str("Daft Punk".into()), Value::Str("X".into())],
                },
            ),
        ])
    }

    #[test]
    fn player_properties_are_applied_cleaned_and_bounded() {
        let mut p = Player::default();
        let now = Instant::now();
        let meta =
            metadata("One More\u{202e}Time\u{7}", "https://i.scdn.co/image/ab67616d0000b2734ce8b4e42588bf18182a1ad2");
        p.apply(
            &[
                ("PlaybackStatus", &Value::Str("Playing".into())),
                ("Metadata", &meta),
                ("Volume", &Value::Double(0.734)),
                ("Shuffle", &Value::Bool(true)),
                ("LoopStatus", &Value::Str("Playlist".into())),
                ("Position", &Value::I64(61_000_000)),
            ],
            now,
        );
        assert_eq!(p.playback, Playback::Playing);
        assert_eq!(p.title, "One MoreTime", "control and bidi characters removed");
        assert_eq!((p.artist.as_str(), p.album.as_str()), ("Daft Punk, X", "Discovery"));
        assert_eq!(p.length_ms, Some(213_000));
        assert_eq!(p.volume, Some(73));
        assert_eq!((p.shuffle, p.repeat), (Some(true), Some(Repeat::Context)));
        assert_eq!(p.art.as_deref(), Some("ab67616d0000b2734ce8b4e42588bf18182a1ad2"));
        assert_eq!(p.position_now(now + Duration::from_millis(1500)), Some(62_500));
        // Wrong types change nothing; a volume out of range is clamped.
        p.apply(&[("Shuffle", &Value::Str("yes".into())), ("Volume", &Value::Double(7.0))], now);
        assert_eq!((p.shuffle, p.volume), (Some(true), Some(100)));
        // Art from anywhere else is not taken.
        for url in [
            "https://evil.example/image/ab67616d0000b273",
            "http://i.scdn.co/image/ab67616d0000b273abcdef",
            "https://i.scdn.co/image/../../x",
            "file:///home/u/cover.jpg",
            "https://i.scdn.co/image/AB67616D0000B273",
        ] {
            assert_eq!(art_id(url), None, "{url}");
        }
        assert_eq!(art_id("https://open.spotify.com/image/ab67616d0000b273aa").as_deref(), Some("ab67616d0000b273aa"));
        let long = "x".repeat(1000);
        p.apply(&[("Metadata", &metadata(&long, ""))], now);
        assert_eq!(p.title.chars().count(), 200);
        assert_eq!(p.art, None);
    }

    #[test]
    fn signals_count_only_from_whom_they_must_come() {
        let signal = |sender: &str, interface: &str, member: &str, body: Vec<Value>| Message {
            kind: dbus::Kind::Signal,
            flags: 0,
            serial: 1,
            path: Some(if interface == BUS { BUS_OBJECT.into() } else { OBJECT.into() }),
            interface: Some(interface.into()),
            member: Some(member.into()),
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: Some(sender.into()),
            body,
        };
        let owner_changed = |sender: &str, name: &str, new: &str| {
            signal(
                sender,
                BUS,
                "NameOwnerChanged",
                vec![Value::Str(name.into()), Value::Str(String::new()), Value::Str(new.into())],
            )
        };
        assert_eq!(
            Mpris::interpret(&owner_changed(BUS, PLAYER, ":1.9"), None),
            Some(Event::Owner(Some(":1.9".into())))
        );
        assert_eq!(Mpris::interpret(&owner_changed(BUS, PLAYER, ""), Some(":1.9")), Some(Event::Owner(None)));
        assert_eq!(
            Mpris::interpret(&owner_changed(":1.5", PLAYER, ":1.5"), None),
            None,
            "only the bus says who owns a name"
        );
        assert_eq!(Mpris::interpret(&owner_changed(BUS, "org.other", ":1.5"), None), None);
        let changed = |sender: &str| {
            signal(
                sender,
                PROPERTIES,
                "PropertiesChanged",
                vec![
                    Value::Str(PLAYER_INTERFACE.into()),
                    Value::string_dict(vec![("Volume".into(), Value::Double(0.5))]),
                    Value::Array { element: "s".into(), items: vec![] },
                ],
            )
        };
        assert_eq!(
            Mpris::interpret(&changed(":1.9"), Some(":1.9")),
            Some(Event::Changed(vec![("Volume".into(), Value::Double(0.5))]))
        );
        assert_eq!(Mpris::interpret(&changed(":1.7"), Some(":1.9")), None, "not the player's owner");
        assert_eq!(Mpris::interpret(&changed(":1.9"), None), None, "no player known");
        let seeked = signal(":1.9", PLAYER_INTERFACE, "Seeked", vec![Value::I64(5_000_000)]);
        assert_eq!(Mpris::interpret(&seeked, Some(":1.9")), Some(Event::Seeked(5_000_000)));
    }
}
