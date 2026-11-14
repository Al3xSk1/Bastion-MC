//! Spotify from the phone: the desktop app on this machine over MPRIS, and — when the user
//! configures it — the Spotify Web API with their own application.
//!
//! **What a launch prepares** (before step 9, [`prepare`]):
//!
//! * MPRIS (on by default; no account, nothing leaves the machine): `xdg-dbus-proxy` in a
//!   bubblewrap sandbox of its own, allowed to pass exactly the Spotify app's player
//!   interface ([`mpris`]); one connection to it, kept for the session.
//! * The Web API (off until `[spotify] client_id` is set and `bastion-mc spotify login` has
//!   stored a refresh token): the token keeper ([`keeper`]) and a first access token.
//! * TCP port 443 (or the launcher proxy's port) for the confined launcher, if the Web API
//!   or album art is on: [`Service::confinement_needs`] adds it and nothing else.
//!
//! **What the phone can do** (after step 9, the app `spotify` of the contract): the verbs of
//! [`LIMITS`], each argument checked against its fixed vocabulary — `open` and `queue` take
//! only `spotify:` URIs of the documented kinds (`uri`), `transfer` only a device id from
//! the last `devices` answer, `search` free text that only ever becomes a percent-encoded
//! query value. Transport commands go to the desktop app when it runs, else to the Web API.
//! Album art goes out as frame slot 0, decoded and re-encoded (`picture`).
//!
//! **What the game gets:** the state object and pictures — titles, artists, a cover, the
//! names of the user's Spotify devices. Never a token, a URL, a path, or an id it could use
//! elsewhere (device ids mean something only with the user's token).

pub mod dbus;
pub mod keeper;
pub mod login;
pub mod mpris;
pub mod picture;
pub mod uri;
pub mod web;

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value as Json, json};
use tokio::sync::mpsc;

use self::{
    mpris::{Mpris, Player, Proxy, Repeat},
    picture::Picture,
    uri::Uri,
    web::{Class, Device, Found, Limits, Web, WebError, WebState},
};
use crate::{
    config::Config,
    confine::Confinement,
    error::{Error, Result},
    net::http::HttpClient,
    paths::Paths,
    phone::{Command, FrameFormat, Outbox, PhoneApp, Service, VerbLimit},
    sandbox::policy::SeccompPolicy,
};

/// The app's name on the wire.
pub const APP: &str = "spotify";

// ---------------------------------------------------------------- configuration

/// `[spotify]` in config.toml.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpotifyConfig {
    /// The Spotify desktop app on this machine, over MPRIS (D-Bus, through a filtering
    /// proxy). No account; nothing leaves the machine.
    pub mpris: bool,
    /// Album art for the phone, fetched by the launcher from Spotify's image server
    /// (i.scdn.co) when the phone asks for it.
    pub album_art: bool,
    /// Your own Spotify application's client ID (developer.spotify.com; no secret). Turns on
    /// the Web API once `bastion-mc spotify login` has run.
    pub client_id: Option<String>,
    /// A fixed port for the sign-in's loopback redirect (`http://127.0.0.1:<port>/callback`);
    /// unset, any free port (register `http://127.0.0.1/callback`).
    pub redirect_port: Option<u16>,
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        SpotifyConfig { mpris: true, album_art: true, client_id: None, redirect_port: None }
    }
}

impl SpotifyConfig {
    pub fn validate(&self) -> Result<()> {
        if let Some(id) = &self.client_id
            && !(id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(Error::Config(
                "spotify.client_id must be a Spotify client ID: 32 lowercase hexadecimal characters".into(),
            ));
        }
        if let Some(port) = self.redirect_port
            && port < 1024
        {
            return Err(Error::Config("spotify.redirect_port must be 1024 or higher".into()));
        }
        Ok(())
    }
}

/// The HTTPS client of the Spotify feature: the launcher's (TLS policy, pins, the SOCKS5
/// proxy of `network.launcher_proxy`), restricted to Spotify's three hosts.
pub fn client(cfg: &Config) -> Result<HttpClient> {
    Ok(crate::launch::http_client(cfg)?.only(&web::HOSTS))
}

// ---------------------------------------------------------------- before step 9

/// Where the launch log goes.
pub type Sink = Arc<dyn Fn(String) + Send + Sync>;

/// The longest the Web API's first token may hold up a launch.
const WARM_UP: Duration = Duration::from_secs(12);

/// What [`prepare`] needs to know of the launch.
pub struct Setup<'a> {
    pub cfg: &'a Config,
    pub paths: &'a Paths,
    /// This program (the sandbox's init stage, the token keeper).
    pub program: &'a Path,
    /// `DBUS_SESSION_BUS_ADDRESS` as the launcher found it.
    pub bus_address: Option<String>,
    /// Nothing of this feature may use the network (a burner session): MPRIS only.
    pub offline: bool,
    pub log: Sink,
}

/// The desktop app's player, connected.
struct Live {
    _proxy: Proxy,
    client: Mpris,
    owner: Option<String>,
    player: Player,
}

/// The service as prepared before step 9.
pub struct Prepared {
    mpris: Option<(Live, mpsc::Receiver<dbus::Message>)>,
    mpris_note: String,
    web: Option<Arc<Web>>,
    web_note: String,
    art: Option<HttpClient>,
    ports: Vec<u16>,
}

async fn connect_mpris(setup: &Setup<'_>) -> std::result::Result<(Live, mpsc::Receiver<dbus::Message>), String> {
    let bus = mpris::session_bus(setup.bus_address.as_deref(), setup.paths.runtime.as_deref())?;
    let base =
        setup.paths.runtime.as_deref().map(|r| r.join(crate::paths::APP)).unwrap_or_else(|| setup.paths.cache.clone());
    crate::util::create_private_dir_all(&base).map_err(|e| e.to_string())?;
    let seccomp = SeccompPolicy {
        mode: setup.cfg.sandbox.seccomp,
        tripwire: setup.cfg.sandbox.tripwire,
        extra_allow: Vec::new(),
    };
    let proxy =
        Proxy::start(&bus, &base, setup.program, seccomp, setup.log.clone()).await.map_err(|e| e.to_string())?;
    let stream = proxy.connect().await.map_err(|e| e.to_string())?;
    // The name goes now: once the launcher has confined itself it could not remove it.
    proxy.release_name();
    let (client, signals) = Mpris::open(stream).await.map_err(|e| e.to_string())?;
    let owner = client.owner().await.map_err(|e| e.to_string())?;
    let mut player = Player::default();
    if owner.is_some()
        && let Ok(properties) = client.properties().await
    {
        let refs: Vec<_> = properties.iter().map(|(k, v)| (k.as_str(), v)).collect();
        player.apply(&refs, Instant::now());
    }
    Ok((Live { _proxy: proxy, client, owner, player }, signals))
}

async fn connect_web(setup: &Setup<'_>) -> std::result::Result<Arc<Web>, String> {
    let store = crate::auth::store::TokenStore::open(&setup.cfg.auth, setup.paths).map_err(|e| e.to_string())?;
    if !store.has_spotify().map_err(|e| e.to_string())? {
        return Err("not signed in (`bastion-mc spotify login`)".into());
    }
    let http = client(setup.cfg).map_err(|e| e.to_string())?;
    let keeper = Arc::new(keeper::Client::start(setup.program).map_err(|e| e.to_string())?);
    let api = Arc::new(Web::new(http, keeper));
    // A first token now, while a failure can still be said plainly in the launch log.
    api.warm().await.map_err(|e| e.to_string())?;
    Ok(api)
}

/// [`prepare`] with the Web API client and the image client given, the desktop app found
/// as in a launch (tests: stand-in servers behind a test CA; a real launch cannot reach
/// this — nothing in the configuration names a client).
#[doc(hidden)]
pub async fn prepare_with(setup: Setup<'_>, web: Option<Arc<Web>>, art: Option<HttpClient>) -> Box<dyn Service> {
    let (mpris, mpris_note) = match connect_mpris(&setup).await {
        Ok(found) => (Some(found), "connected (test)".to_string()),
        Err(why) => (None, why),
    };
    let ports = if web.is_some() || art.is_some() { vec![443] } else { Vec::new() };
    Box::new(Prepared { mpris, mpris_note, web, web_note: "given (test)".into(), art, ports })
}

/// Prepare the service (steps 6 to 8): `None` when everything of it is switched off or
/// unusable (the log says why).
pub async fn prepare(setup: Setup<'_>) -> Option<Box<dyn Service>> {
    let s = &setup.cfg.spotify;
    let (mpris, mpris_note) = if !s.mpris {
        (None, "off ([spotify] mpris = false)".to_string())
    } else {
        match connect_mpris(&setup).await {
            Ok((live, signals)) => {
                let note = match &live.owner {
                    Some(_) => "connected through xdg-dbus-proxy; the Spotify app is running",
                    None => "connected through xdg-dbus-proxy; the Spotify app is not running (yet)",
                };
                (Some((live, signals)), note.to_string())
            }
            Err(why) => (None, format!("unavailable — {why}")),
        }
    };
    let (web, web_note) = match (&s.client_id, setup.offline) {
        (None, _) => (None, "off (no [spotify] client_id)".to_string()),
        (Some(_), true) => (None, "off in a session without network".to_string()),
        // Bounded: this runs before the game starts, and a network that does not answer must
        // not hold the game back for long.
        (Some(_), false) => match tokio::time::timeout(WARM_UP, connect_web(&setup)).await {
            Ok(Ok(api)) => (Some(api), "signed in (the token keeper holds the refresh token)".to_string()),
            Ok(Err(why)) => (None, format!("unavailable — {why}")),
            Err(_) => (None, format!("unavailable — Spotify did not answer within {} s", WARM_UP.as_secs())),
        },
    };
    let art = (s.album_art && !setup.offline).then(|| client(setup.cfg).ok()).flatten();
    if mpris.is_none() && web.is_none() {
        (setup.log)(format!("spotify: not prepared — desktop app (MPRIS): {mpris_note}; Web API: {web_note}"));
        return None;
    }
    let mut ports = Vec::new();
    if web.is_some() || art.is_some() {
        // What the HTTPS client connects to: the proxy, if one is set, else port 443.
        let proxy = crate::launch::http_client(setup.cfg).ok().and_then(|h| h.socks5());
        ports.push(proxy.map_or(443, |p| p.port()));
    }
    Some(Box::new(Prepared { mpris, mpris_note, web, web_note, art, ports }))
}

impl Service for Prepared {
    fn name(&self) -> &'static str {
        APP
    }

    fn describe(&self) -> String {
        format!(
            "spotify: desktop app (MPRIS): {}; Web API: {}; album art: {}",
            self.mpris_note,
            self.web_note,
            if self.art.is_some() { "on (i.scdn.co, when the phone asks)" } else { "off" }
        )
    }

    fn confinement_needs(&self, keep: &mut Confinement) {
        keep.tcp_connect.extend(self.ports.iter().copied());
    }

    fn into_app(self: Box<Self>) -> Box<dyn PhoneApp> {
        let (tx, rx) = mpsc::channel(16);
        let prepared = *self;
        let (mpris, signals) = match prepared.mpris {
            Some((live, signals)) => (Some(live), Some(signals)),
            None => (None, None),
        };
        let actor = Actor {
            mpris,
            web: prepared.web,
            web_state: None,
            web_polled: None,
            art_http: prepared.art,
            art_limits: Limits::new(Instant::now()),
            art: None,
            art_wanted: false,
            results: Vec::new(),
            devices: Vec::new(),
            error: None,
            out: None,
            active_until: Instant::now(),
        };
        tokio::spawn(run(actor, rx, signals));
        Box::new(SpotifyApp { tx })
    }
}

// ---------------------------------------------------------------- after step 9

/// How often the phone may use each verb.
pub const LIMITS: &[VerbLimit] = &[
    VerbLimit { verb: "play", per_minute: 60, burst: 10 },
    VerbLimit { verb: "pause", per_minute: 60, burst: 10 },
    VerbLimit { verb: "toggle", per_minute: 60, burst: 10 },
    VerbLimit { verb: "next", per_minute: 60, burst: 10 },
    VerbLimit { verb: "previous", per_minute: 60, burst: 10 },
    // The phone sends up to four of these a second while a slider is dragged.
    VerbLimit { verb: "seek", per_minute: 240, burst: 40 },
    VerbLimit { verb: "volume", per_minute: 240, burst: 40 },
    VerbLimit { verb: "shuffle", per_minute: 20, burst: 5 },
    VerbLimit { verb: "repeat", per_minute: 20, burst: 5 },
    VerbLimit { verb: "open", per_minute: 20, burst: 5 },
    VerbLimit { verb: "queue", per_minute: 20, burst: 5 },
    VerbLimit { verb: "search", per_minute: 10, burst: 3 },
    VerbLimit { verb: "devices", per_minute: 6, burst: 2 },
    VerbLimit { verb: "transfer", per_minute: 6, burst: 2 },
    VerbLimit { verb: "art", per_minute: 20, burst: 5 },
];

/// The longest a single Spotify request may hold up the app.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// After the phone's last word, keep the Web API state fresh for this long.
const ACTIVE: Duration = Duration::from_secs(180);
/// How often the Web API state is read while active.
const POLL: Duration = Duration::from_secs(15);
const MAX_SEEK_MS: u64 = 24 * 3600 * 1000;

enum Input {
    Hello(Outbox),
    Command(Command, Outbox),
}

/// The handle the dispatcher holds.
struct SpotifyApp {
    tx: mpsc::Sender<Input>,
}

impl PhoneApp for SpotifyApp {
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
    mpris: Option<Live>,
    web: Option<Arc<Web>>,
    web_state: Option<WebState>,
    web_polled: Option<Instant>,
    art_http: Option<HttpClient>,
    /// The art budget when there is no Web API client (which has its own).
    art_limits: Limits,
    art: Option<(String, Picture)>,
    /// The phone asked for art this session: send the new cover when the track changes.
    art_wanted: bool,
    results: Vec<Found>,
    devices: Vec<Device>,
    error: Option<String>,
    out: Option<Outbox>,
    active_until: Instant,
}

async fn run(mut actor: Actor, mut inputs: mpsc::Receiver<Input>, mut signals: Option<mpsc::Receiver<dbus::Message>>) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            input = inputs.recv() => match input {
                None => break,
                Some(Input::Hello(out)) => actor.hello(out).await,
                Some(Input::Command(command, out)) => actor.command(command, out).await,
            },
            signal = async {
                match signals.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => match signal {
                Some(signal) => actor.signal(signal).await,
                None => {
                    // The connection to the proxy ended: the desktop app is out of reach for
                    // the rest of the session (a confined launcher cannot connect again).
                    signals = None;
                    actor.lost_mpris();
                }
            },
            _ = tick.tick() => actor.tick().await,
        }
    }
}

/// What the phone is shown of a playback source.
#[derive(Default)]
struct Shown {
    source: &'static str,
    playing: bool,
    track: String,
    artist: String,
    album: String,
    uri: Option<String>,
    duration_ms: Option<u64>,
    position_ms: Option<u64>,
    volume: Option<u8>,
    shuffle: Option<bool>,
    repeat: Option<Repeat>,
    device: Option<String>,
    art: Option<String>,
}

fn one_word(command: &Command) -> std::result::Result<&str, String> {
    match (command.args.as_slice(), &command.text) {
        ([word], None) => Ok(word.as_str()),
        _ => Err(format!("{} takes exactly one argument", command.verb)),
    }
}

fn no_arguments(command: &Command) -> std::result::Result<(), String> {
    if command.args.is_empty() && command.text.is_none() {
        Ok(())
    } else {
        Err(format!("{} takes no arguments", command.verb))
    }
}

fn number(word: &str, max: u64) -> Option<u64> {
    (!word.is_empty() && word.len() <= 10 && word.bytes().all(|b| b.is_ascii_digit()))
        .then(|| word.parse::<u64>().ok())
        .flatten()
        .filter(|n| *n <= max)
}

/// A command's arguments, checked against the verb's vocabulary before anything is done.
struct Checked {
    ms: u64,
    percent: u8,
    on: bool,
    repeat: Repeat,
    uri: Option<Uri>,
    word: String,
}

fn check(command: &Command) -> std::result::Result<Checked, String> {
    let mut c = Checked { ms: 0, percent: 0, on: false, repeat: Repeat::Off, uri: None, word: String::new() };
    match command.verb.as_str() {
        "play" | "pause" | "toggle" | "next" | "previous" | "devices" | "art" => no_arguments(command)?,
        "seek" => c.ms = number(one_word(command)?, MAX_SEEK_MS).ok_or("seek takes a position in milliseconds")?,
        "volume" => c.percent = number(one_word(command)?, 100).ok_or("volume takes a number from 0 to 100")? as u8,
        "shuffle" => {
            c.on = match one_word(command)? {
                "on" => true,
                "off" => false,
                _ => return Err("shuffle takes on or off".into()),
            }
        }
        "repeat" => c.repeat = Repeat::parse(one_word(command)?).ok_or("repeat takes off, track or context")?,
        "open" => {
            c.uri = Some(
                Uri::parse(one_word(command)?)
                    .ok_or("open takes a spotify: URI of a track, album, playlist, artist, episode or show")?,
            )
        }
        "queue" => {
            c.uri = Some(
                Uri::parse(one_word(command)?)
                    .filter(|u| u.kind.is_item())
                    .ok_or("queue takes a spotify: URI of a track or an episode")?,
            )
        }
        "transfer" => c.word = one_word(command)?.to_string(),
        "search" => {
            if !command.args.is_empty() {
                return Err("search takes only text: search -- <words>".into());
            }
            let text = command.text.as_deref().map(str::trim).unwrap_or_default();
            if text.is_empty() || text.chars().count() > 200 {
                return Err("search needs 1 to 200 characters of text".into());
            }
            c.word = text.to_string();
        }
        other => return Err(format!("unknown verb {other}")),
    }
    Ok(c)
}

impl Actor {
    fn player(&self) -> Option<&Live> {
        self.mpris.as_ref().filter(|live| live.owner.is_some())
    }

    fn shown(&self) -> Shown {
        if let Some(live) = self.player() {
            let p = &live.player;
            return Shown {
                source: "mpris",
                playing: p.playback == mpris::Playback::Playing,
                track: p.title.clone(),
                artist: p.artist.clone(),
                album: p.album.clone(),
                uri: None,
                duration_ms: p.length_ms,
                position_ms: p.position_now(Instant::now()),
                volume: p.volume,
                shuffle: p.shuffle,
                repeat: p.repeat,
                device: None,
                art: p.art.clone(),
            };
        }
        if let Some(w) = self.web_state.as_ref().filter(|_| self.web.is_some()) {
            return Shown {
                source: "web",
                playing: w.playing,
                track: w.title.clone(),
                artist: w.artist.clone(),
                album: w.album.clone(),
                uri: w.uri.clone(),
                duration_ms: w.duration_ms,
                position_ms: w.position_ms,
                volume: w.volume,
                shuffle: Some(w.shuffle),
                repeat: w.repeat,
                device: w.device.clone(),
                art: w.art.clone(),
            };
        }
        Shown { source: "none", ..Shown::default() }
    }

    fn state(&self) -> Json {
        let s = self.shown();
        json!({
            "source": s.source,
            "playing": s.playing,
            "track": s.track,
            "artist": s.artist,
            "album": s.album,
            "uri": s.uri,
            "duration_ms": s.duration_ms,
            "position_ms": s.position_ms,
            "volume": s.volume,
            "shuffle": s.shuffle,
            "repeat": s.repeat.map(Repeat::word),
            "device": s.device,
            "art": s.art.is_some() && self.art_http.is_some(),
            "web": if self.web.is_some() { "on" } else { "off" },
            "desktop": match &self.mpris {
                Some(live) if live.owner.is_some() => "running",
                Some(_) => "not running",
                None => "unavailable",
            },
            "results": self.results.iter().map(|f| json!({"uri": f.uri, "title": f.title, "subtitle": f.subtitle})).collect::<Vec<_>>(),
            "devices": self.devices.iter().map(|d| json!({"id": d.id, "name": d.name, "active": d.active})).collect::<Vec<_>>(),
            "error": self.error,
        })
    }

    fn send_state(&self) {
        if let Some(out) = &self.out {
            out.state(APP, &self.state());
        }
    }

    fn notice(&self, text: &str) {
        if let Some(out) = &self.out {
            out.notice(APP, text);
        }
    }

    fn lost_mpris(&mut self) {
        if self.mpris.take().is_some() {
            self.error = Some("the connection to the desktop app's player was lost for this session".into());
            self.send_state();
        }
    }

    async fn refresh_mpris(&mut self) {
        let Some(live) = self.mpris.as_mut() else { return };
        if live.owner.is_none() {
            return;
        }
        if let Ok(Ok(Some(us))) = tokio::time::timeout(CALL_TIMEOUT, live.client.position()).await
            && us >= 0
        {
            live.player.position_ms = Some(((us / 1000) as u64, Instant::now()));
        }
    }

    async fn refresh_web(&mut self) {
        let Some(web) = self.web.clone() else { return };
        if self.player().is_some() {
            return;
        }
        self.web_polled = Some(Instant::now());
        match tokio::time::timeout(CALL_TIMEOUT, web.state()).await {
            Ok(Ok(state)) => self.web_state = state,
            Ok(Err(WebError::Limited { .. })) => {}
            Ok(Err(e)) => self.error = Some(e.to_string()),
            Err(_) => self.error = Some("Spotify did not answer in time".into()),
        }
    }

    async fn hello(&mut self, out: Outbox) {
        self.out = Some(out);
        self.active_until = Instant::now() + ACTIVE;
        self.refresh_mpris().await;
        self.refresh_web().await;
        self.send_state();
        if let (Some((id, picture)), Some(current)) = (&self.art, self.shown().art)
            && *id == current
        {
            self.frame(picture);
        }
    }

    async fn tick(&mut self) {
        let now = Instant::now();
        if self.out.is_some()
            && now < self.active_until
            && self.player().is_none()
            && self.web.is_some()
            && self.web_polled.is_none_or(|t| now.saturating_duration_since(t) >= POLL)
        {
            let before = self.state();
            self.refresh_web().await;
            if self.state() != before {
                self.send_state();
                self.follow_art().await;
            }
        }
    }

    async fn signal(&mut self, signal: dbus::Message) {
        let Some(live) = self.mpris.as_mut() else { return };
        let Some(event) = Mpris::interpret(&signal, live.owner.as_deref()) else { return };
        let mut track_changed = false;
        match event {
            mpris::Event::Owner(None) => {
                live.owner = None;
                live.player = Player::default();
                // The Web API (if any) shows what plays elsewhere from now on.
                self.refresh_web().await;
                track_changed = true;
            }
            mpris::Event::Owner(Some(name)) => {
                live.owner = Some(name);
                live.player = Player::default();
                if let Ok(Ok(properties)) = tokio::time::timeout(CALL_TIMEOUT, live.client.properties()).await {
                    let refs: Vec<_> = properties.iter().map(|(k, v)| (k.as_str(), v)).collect();
                    live.player.apply(&refs, Instant::now());
                }
                track_changed = true;
            }
            mpris::Event::Changed(changed) => {
                track_changed = changed.iter().any(|(k, _)| k == "Metadata");
                let refs: Vec<_> = changed.iter().map(|(k, v)| (k.as_str(), v)).collect();
                live.player.apply(&refs, Instant::now());
            }
            mpris::Event::Seeked(us) => {
                if us >= 0 {
                    live.player.position_ms = Some(((us / 1000) as u64, Instant::now()));
                }
            }
        }
        self.send_state();
        if track_changed {
            self.follow_art().await;
        }
    }

    /// After a track change: the new cover, if the phone has asked for covers.
    async fn follow_art(&mut self) {
        let stale = self.shown().art.is_some_and(|id| self.art.as_ref().is_none_or(|(have, _)| *have != id));
        if self.art_wanted
            && self.out.is_some()
            && stale
            && let Err(why) = self.send_art().await
        {
            self.notice(&why);
        }
    }

    fn frame(&self, picture: &Picture) {
        if let Some(out) = &self.out {
            out.frame(APP, 0, picture.width, picture.height, FrameFormat::Png, picture.png.clone());
        }
    }

    async fn send_art(&mut self) -> std::result::Result<(), String> {
        let http = self.art_http.clone().ok_or("album art is off ([spotify] album_art)")?;
        let id = self.shown().art.ok_or("no cover for what is playing")?;
        if let Some((have, picture)) = &self.art
            && *have == id
        {
            self.frame(picture);
            return Ok(());
        }
        match &self.web {
            Some(web) => web.take(Class::Art),
            None => self.art_limits.take(Class::Art, Instant::now()),
        }
        .map_err(|e| e.to_string())?;
        let jpeg = tokio::time::timeout(CALL_TIMEOUT, web::fetch_art(&http, &id))
            .await
            .map_err(|_| "the image server did not answer in time".to_string())?
            .map_err(|e| e.to_string())?;
        let picture = tokio::task::spawn_blocking(move || picture::jpeg_to_png(&jpeg, 320, 320))
            .await
            .map_err(|_| "decoding the cover failed".to_string())?
            .map_err(|e| e.to_string())?;
        self.frame(&picture);
        self.art = Some((id, picture));
        Ok(())
    }

    async fn command(&mut self, command: Command, out: Outbox) {
        self.out = Some(out);
        self.active_until = Instant::now() + ACTIVE;
        match self.run_command(&command).await {
            Ok(()) => self.error = None,
            Err(why) => {
                self.notice(&why);
                self.error = Some(why);
            }
        }
        self.send_state();
    }

    /// The Web API client, or why there is none.
    fn web(&self) -> std::result::Result<Arc<Web>, String> {
        self.web.clone().ok_or_else(|| {
            "this needs the Spotify Web API: [spotify] client_id and `bastion-mc spotify login` before the launch"
                .into()
        })
    }

    async fn run_command(&mut self, command: &Command) -> std::result::Result<(), String> {
        let verb = command.verb.as_str();
        let c = check(command)?;
        if verb == "art" {
            self.art_wanted = true;
            return self.send_art().await;
        }

        // The desktop app, when it runs, takes the transport commands: no network.
        if let Some(live) = self.player() {
            let client = &live.client;
            let done = match verb {
                "play" => Some(client.play().await),
                "pause" => Some(client.pause().await),
                "toggle" => Some(client.toggle().await),
                "next" => Some(client.next().await),
                "previous" => Some(client.previous().await),
                "seek" => Some(client.seek(c.ms, &live.player).await),
                "volume" => Some(client.volume(c.percent).await),
                "shuffle" => Some(client.shuffle(c.on).await),
                "repeat" => Some(client.repeat(c.repeat).await),
                "open" => Some(client.open_uri(&c.uri.as_ref().map(Uri::text).unwrap_or_default()).await),
                _ => None,
            };
            if let Some(done) = done {
                done.map_err(|e| {
                    format!(
                        "the Spotify app did not take it: {}",
                        crate::util::sanitize_line(e.to_string().as_bytes(), 160)
                    )
                })?;
                if verb == "seek"
                    && let Some(live) = self.mpris.as_mut()
                {
                    live.player.position_ms = Some((c.ms, Instant::now()));
                }
                return Ok(());
            }
        }

        let web = self.web()?;
        let playing = self.web_state.as_ref().is_some_and(|s| s.playing);
        let devices = self.devices.clone();
        let outcome = tokio::time::timeout(CALL_TIMEOUT, async {
            match verb {
                "play" => web.play().await.map(|()| None),
                "pause" => web.pause().await.map(|()| None),
                "toggle" if playing => web.pause().await.map(|()| None),
                "toggle" => web.play().await.map(|()| None),
                "next" => web.next().await.map(|()| None),
                "previous" => web.previous().await.map(|()| None),
                "seek" => web.seek(c.ms).await.map(|()| None),
                "volume" => web.volume(c.percent).await.map(|()| None),
                "shuffle" => web.shuffle(c.on).await.map(|()| None),
                "repeat" => web.repeat(c.repeat).await.map(|()| None),
                "open" => web.open(c.uri.as_ref().expect("checked")).await.map(|()| None),
                "queue" => web.queue(c.uri.as_ref().expect("checked")).await.map(|()| None),
                "search" => web.search(&c.word).await.map(|found| Some(Answer::Found(found))),
                "devices" => web.devices().await.map(|list| Some(Answer::Devices(list))),
                // Only a device the last `devices` answer named: the phone cannot make one up.
                "transfer" => match devices.iter().find(|d| d.id == c.word) {
                    Some(device) => web.transfer(device).await.map(|()| None),
                    None => Err(WebError::Api { status: 0, message: "unknown device (ask for devices first)".into() }),
                },
                _ => Ok(None),
            }
        })
        .await
        .map_err(|_| "Spotify did not answer in time".to_string())?
        .map_err(|e| e.to_string())?;
        match outcome {
            Some(Answer::Found(found)) => self.results = found,
            Some(Answer::Devices(list)) => self.devices = list,
            None => {
                // Spotify applies a command a moment after accepting it.
                tokio::time::sleep(Duration::from_millis(400)).await;
                self.refresh_web().await;
                self.follow_art().await;
            }
        }
        Ok(())
    }
}

enum Answer {
    Found(Vec<Found>),
    Devices(Vec<Device>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_is_checked() {
        let mut c = SpotifyConfig::default();
        assert!(c.mpris && c.album_art && c.client_id.is_none(), "MPRIS on, Web API off by default");
        c.validate().unwrap();
        c.client_id = Some("0123456789abcdef0123456789abcdef".into());
        c.validate().unwrap();
        for bad in ["0123456789ABCDEF0123456789ABCDEF", "0123", "0123456789abcdef0123456789abcdeg"] {
            c.client_id = Some(bad.into());
            assert!(c.validate().is_err(), "{bad}");
        }
        c.client_id = None;
        c.redirect_port = Some(80);
        assert!(c.validate().is_err());
    }

    #[test]
    fn arguments_are_checked_against_the_vocabulary() {
        let cmd = |line: &str| Command::parse(line.as_bytes()).unwrap();
        assert_eq!(check(&cmd("spotify volume 40")).unwrap().percent, 40);
        assert_eq!(check(&cmd("spotify seek 61000")).unwrap().ms, 61_000);
        assert!(check(&cmd("spotify shuffle on")).unwrap().on);
        assert_eq!(check(&cmd("spotify repeat context")).unwrap().repeat, Repeat::Context);
        assert_eq!(check(&cmd("spotify search --  daft punk ")).unwrap().word, "daft punk");
        let open = check(&cmd("spotify open spotify:playlist:37i9dQZF1DXcBWIGoYBM5M")).unwrap();
        assert_eq!(open.uri.unwrap().text(), "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M");
        for refused in [
            "spotify volume 101",
            "spotify volume 40 50",
            "spotify volume -- 40",
            "spotify seek 1e9",
            "spotify play now",
            "spotify shuffle yes",
            "spotify repeat all",
            "spotify open https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
            "spotify open file:///etc/passwd",
            "spotify open spotify:user:alice",
            "spotify queue spotify:album:2noRn2Aes5aoNVsU6iWThc",
            "spotify search daft",
            "spotify search -- ",
            "spotify transfer",
            "spotify rm -rf",
        ] {
            assert!(check(&cmd(refused)).is_err(), "{refused}");
        }
        assert!(LIMITS.iter().all(|l| l.per_minute > 0 && l.burst > 0));
        let mut verbs: Vec<&str> = LIMITS.iter().map(|l| l.verb).collect();
        verbs.sort_unstable();
        let mut expected = vec![
            "play", "pause", "toggle", "next", "previous", "seek", "volume", "shuffle", "repeat", "open", "queue",
            "search", "devices", "transfer", "art",
        ];
        expected.sort_unstable();
        assert_eq!(verbs, expected, "exactly the contract's vocabulary");
    }
}
