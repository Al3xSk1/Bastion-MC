//! The Spotify Web API: what the phone can do beyond the desktop app on this machine —
//! search, the queue, devices (play on a phone or a speaker), and playback control when the
//! desktop app is not running.
//!
//! Off until the user configures it: it needs the user's own Spotify application
//! (`[spotify] client_id`; no client secret exists in this program: authorization code with
//! PKCE) and a sign-in (`bastion-mc spotify login`). It talks to [`HOSTS`] and nowhere else,
//! through the launcher's own HTTPS client (TLS 1.3 only, no redirects, bounded bodies, the
//! SOCKS5 proxy of `network.launcher_proxy` if one is set), with the scopes in [`SCOPES`]
//! and no others.
//!
//! **Limits.** Every call is counted against a local budget per kind of call
//! ([`Limits`]) before it leaves: a hostile game can make the phone send commands as fast as
//! the phone's own limits allow, and those must not turn into a flood of requests in the
//! user's name (Spotify rate-limits per application — the user's). A `429` sets a pause for
//! everything, as long as `Retry-After` says (at most an hour).
//!
//! **Tokens.** The access token (an hour) lives in locked memory here. Where it comes from
//! is a [`TokenSource`]: in a launch the token keeper (`keeper`), which holds the refresh
//! token outside this process.
//!
//! Every reply is parsed into typed values and cleaned before anything of it reaches the
//! phone ([`fuzz`] runs the parsers on arbitrary bytes).

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hyper::Method;
use serde::Deserialize;

use super::{
    mpris::{Repeat, art_id, clean_text},
    uri::Uri,
};
use crate::{
    net::http::{HttpClient, form_body, header, sensitive_header},
    secret::Secret,
};

pub const ACCOUNTS_HOST: &str = "accounts.spotify.com";
pub const API_HOST: &str = "api.spotify.com";
/// Album art (`mpris::art_id`).
pub const IMAGE_HOST: &str = "i.scdn.co";
/// Every host the Spotify feature contacts.
pub const HOSTS: [&str; 3] = [ACCOUNTS_HOST, API_HOST, IMAGE_HOST];
/// Reading and changing what plays. Nothing about the account, the library or playlists.
pub const SCOPES: &str = "user-read-playback-state user-modify-playback-state";
/// The largest reply read (a search with five results per type is ~40 KB).
const REPLY_LIMIT: u64 = 512 * 1024;
const TOKEN_REPLY_LIMIT: usize = 64 * 1024;
/// Search results per type (tracks, albums, artists, playlists).
const SEARCH_LIMIT: usize = 5;

/// What went wrong, in words the phone can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebError {
    /// The local budget for this kind of call is used up; try again in `seconds`.
    Limited {
        seconds: u64,
    },
    /// Spotify said 429; nothing is sent for `seconds`.
    RateLimited {
        seconds: u64,
    },
    /// Playback control through the Web API needs Spotify Premium.
    Premium,
    /// No Spotify app is active anywhere to take the command.
    NoDevice,
    /// The sign-in no longer works (refresh refused, revoked, expired): sign in again.
    SignedOut(String),
    /// Any other answer from Spotify.
    Api {
        status: u16,
        message: String,
    },
    Net(String),
}

impl std::fmt::Display for WebError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebError::Limited { seconds } => write!(f, "too many requests from the phone; try again in {seconds} s"),
            WebError::RateLimited { seconds } => {
                write!(f, "Spotify asked for a pause; nothing is sent for {seconds} s")
            }
            WebError::Premium => write!(f, "controlling playback through the Spotify Web API needs Spotify Premium"),
            WebError::NoDevice => {
                write!(f, "no active Spotify device: start playing somewhere, or pick a device (devices, transfer)")
            }
            WebError::SignedOut(why) => {
                write!(f, "the Spotify sign-in no longer works ({why}): run `bastion-mc spotify login`")
            }
            WebError::Api { status, message } => write!(f, "Spotify answered {status}: {message}"),
            WebError::Net(why) => write!(f, "cannot reach Spotify: {why}"),
        }
    }
}

// ---------------------------------------------------------------- limits

/// Kinds of call, each with a budget of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// play, pause, next, previous, seek, volume, shuffle, repeat, open, transfer
    Control,
    /// the player's state, the device list
    Read,
    Search,
    Queue,
    /// a cover image from the image server
    Art,
}

#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    burst: f64,
    per_second: f64,
    last: Instant,
}

impl Bucket {
    fn new(per_minute: u32, burst: u32, now: Instant) -> Bucket {
        Bucket {
            tokens: f64::from(burst),
            burst: f64::from(burst),
            per_second: f64::from(per_minute) / 60.0,
            last: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.burst);
        self.last = now;
    }

    /// Seconds until a token is there (0: now).
    fn wait(&self) -> u64 {
        if self.tokens >= 1.0 { 0 } else { ((1.0 - self.tokens) / self.per_second).ceil().max(1.0) as u64 }
    }
}

/// The local budgets: per class, and for all calls together; and Spotify's own pause.
#[derive(Debug, Clone)]
pub struct Limits {
    control: Bucket,
    read: Bucket,
    search: Bucket,
    queue: Bucket,
    art: Bucket,
    all: Bucket,
    paused_until: Option<Instant>,
}

impl Limits {
    pub fn new(now: Instant) -> Limits {
        Limits {
            control: Bucket::new(30, 8, now),
            read: Bucket::new(20, 6, now),
            search: Bucket::new(10, 3, now),
            queue: Bucket::new(10, 3, now),
            art: Bucket::new(20, 5, now),
            all: Bucket::new(60, 15, now),
            paused_until: None,
        }
    }

    /// Take one call of `class`, or say how long to wait.
    pub fn take(&mut self, class: Class, now: Instant) -> Result<(), WebError> {
        if let Some(until) = self.paused_until {
            if now < until {
                return Err(WebError::RateLimited { seconds: until.saturating_duration_since(now).as_secs().max(1) });
            }
            self.paused_until = None;
        }
        self.all.refill(now);
        let bucket = match class {
            Class::Control => &mut self.control,
            Class::Read => &mut self.read,
            Class::Search => &mut self.search,
            Class::Queue => &mut self.queue,
            Class::Art => &mut self.art,
        };
        bucket.refill(now);
        let wait = bucket.wait().max(self.all.wait());
        if wait > 0 {
            return Err(WebError::Limited { seconds: wait });
        }
        bucket.tokens -= 1.0;
        self.all.tokens -= 1.0;
        Ok(())
    }

    /// Spotify said `429`: pause everything (art from the image server included).
    pub fn pause(&mut self, seconds: u64, now: Instant) {
        let until = now + Duration::from_secs(seconds.clamp(1, 3600));
        self.paused_until = Some(self.paused_until.map_or(until, |u| u.max(until)));
    }
}

/// `Retry-After` in seconds (Spotify sends seconds); missing or unreadable: 5.
pub fn retry_after(headers: &hyper::HeaderMap) -> u64 {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(5)
        .clamp(1, 3600)
}

// ---------------------------------------------------------------- tokens

/// What [`TokenSource::token`] returns: a usable token and how long it stays valid.
pub type TokenFuture<'a> = Pin<Box<dyn Future<Output = Result<(Secret, Duration), WebError>> + Send + 'a>>;
/// Told about each refresh token Spotify rotates in (to store it).
pub type OnRotation = Box<dyn Fn(&Secret) -> Result<(), WebError> + Send + Sync>;

/// Access tokens, from wherever they come.
pub trait TokenSource: Send + Sync {
    /// A usable access token and how long it stays valid. `refused`: the API just refused
    /// the previous one (a `401`).
    fn token(&self, refused: bool) -> TokenFuture<'_>;
}

/// What the token endpoint gave.
pub struct Tokens {
    pub access: Secret,
    pub expires_in: Duration,
    /// A new refresh token, when Spotify rotates it (store it; the old one may be spent).
    pub refresh: Option<Secret>,
    pub scope: String,
}

#[derive(Deserialize)]
struct TokenJson {
    access_token: Secret,
    #[serde(default)]
    token_type: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<Secret>,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct OAuthErrorJson {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// A token goes into an `Authorization` header: printable ASCII without spaces, bounded.
fn token_shaped(token: &Secret) -> bool {
    !token.is_empty() && token.len() <= 4096 && token.expose().iter().all(|b| (0x21..0x7f).contains(b))
}

/// The token endpoint's answer.
pub fn parse_token_reply(status: u16, body: &[u8]) -> Result<Tokens, WebError> {
    if status == 200 {
        let t: TokenJson = serde_json::from_slice(body)
            .map_err(|_| WebError::Api { status, message: "unexpected token reply".into() })?;
        if !t.token_type.eq_ignore_ascii_case("bearer") || !token_shaped(&t.access_token) {
            return Err(WebError::Api { status, message: "unexpected token reply".into() });
        }
        if t.refresh_token.as_ref().is_some_and(|r| !token_shaped(r)) {
            return Err(WebError::Api { status, message: "unexpected refresh token".into() });
        }
        let seconds = if t.expires_in == 0 { 3600 } else { t.expires_in.clamp(60, 86_400) };
        return Ok(Tokens {
            access: t.access_token,
            expires_in: Duration::from_secs(seconds),
            refresh: t.refresh_token,
            scope: clean_text(&t.scope, 300),
        });
    }
    match serde_json::from_slice::<OAuthErrorJson>(body) {
        // The grant is gone (revoked, expired after six months, spent): only a new sign-in helps.
        Ok(e) if e.error == "invalid_grant" => Err(WebError::SignedOut(clean_text(&e.error_description, 160))),
        Ok(e) => Err(WebError::Api {
            status,
            message: format!("{} {}", clean_text(&e.error, 60), clean_text(&e.error_description, 160)).trim().into(),
        }),
        Err(_) => Err(WebError::Api { status, message: "token endpoint error".into() }),
    }
}

fn net(e: crate::Error) -> WebError {
    WebError::Net(crate::util::sanitize_line(e.to_string().as_bytes(), 200))
}

async fn token_request(http: &HttpClient, form: &[(&str, &[u8])]) -> Result<Tokens, WebError> {
    let headers = vec![
        header("content-type", "application/x-www-form-urlencoded").map_err(net)?,
        header("accept", "application/json").map_err(net)?,
    ];
    let reply = http
        .request_secret(
            Method::POST,
            &format!("https://{ACCOUNTS_HOST}/api/token"),
            &headers,
            Some(form_body(form)),
            TOKEN_REPLY_LIMIT,
        )
        .await
        .map_err(net)?;
    parse_token_reply(reply.status, reply.body.expose())
}

/// A refresh token → a fresh access token (and maybe a rotated refresh token).
pub async fn refresh(http: &HttpClient, client_id: &str, refresh: &Secret) -> Result<Tokens, WebError> {
    token_request(
        http,
        &[("grant_type", b"refresh_token"), ("refresh_token", refresh.expose()), ("client_id", client_id.as_bytes())],
    )
    .await
}

/// The authorization code of a sign-in → tokens.
pub async fn redeem(
    http: &HttpClient,
    client_id: &str,
    code: &Secret,
    redirect_uri: &str,
    verifier: &Secret,
) -> Result<Tokens, WebError> {
    token_request(
        http,
        &[
            ("grant_type", b"authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", redirect_uri.as_bytes()),
            ("client_id", client_id.as_bytes()),
            ("code_verifier", verifier.expose()),
        ],
    )
    .await
}

/// A token source that refreshes with a refresh token it holds, keeping what Spotify
/// rotates in memory; `on_rotation` is told about each new refresh token (to store it).
/// The token keeper uses it, and so do the commands that run before any launch.
pub struct Refreshing {
    http: HttpClient,
    client_id: String,
    refresh: tokio::sync::Mutex<Secret>,
    on_rotation: OnRotation,
}

impl Refreshing {
    pub fn new(http: HttpClient, client_id: &str, refresh: Secret, on_rotation: OnRotation) -> Refreshing {
        Refreshing { http, client_id: client_id.to_string(), refresh: tokio::sync::Mutex::new(refresh), on_rotation }
    }
}

impl TokenSource for Refreshing {
    fn token(&self, _refused: bool) -> TokenFuture<'_> {
        Box::pin(async move {
            let mut held = self.refresh.lock().await;
            let tokens = refresh(&self.http, &self.client_id, &held).await?;
            if let Some(next) = tokens.refresh {
                (self.on_rotation)(&next)?;
                *held = next;
            }
            Ok((tokens.access, tokens.expires_in))
        })
    }
}

// ---------------------------------------------------------------- replies

#[derive(Deserialize, Default)]
#[serde(default)]
struct ImageJson {
    url: String,
    width: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NamedJson {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AlbumJson {
    name: String,
    uri: String,
    images: Vec<ImageJson>,
    artists: Vec<NamedJson>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ItemJson {
    #[serde(rename = "type")]
    kind: String,
    uri: String,
    name: String,
    duration_ms: Option<u64>,
    artists: Vec<NamedJson>,
    album: Option<AlbumJson>,
    show: Option<AlbumJson>,
    images: Vec<ImageJson>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DeviceJson {
    id: Option<String>,
    name: String,
    is_active: bool,
    volume_percent: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PlayerJson {
    device: Option<DeviceJson>,
    shuffle_state: bool,
    repeat_state: String,
    progress_ms: Option<u64>,
    is_playing: bool,
    item: Option<ItemJson>,
}

/// A Spotify device, as the phone may name it back (`transfer <id>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub active: bool,
}

/// What `GET /me/player` says, cleaned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebState {
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub uri: Option<String>,
    pub duration_ms: Option<u64>,
    pub position_ms: Option<u64>,
    pub volume: Option<u8>,
    pub shuffle: bool,
    pub repeat: Option<Repeat>,
    pub device: Option<String>,
    /// The cover's image id (`mpris::art_id`), never a URL.
    pub art: Option<String>,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub uri: String,
    pub title: String,
    pub subtitle: String,
}

fn names(list: &[NamedJson]) -> String {
    clean_text(&list.iter().take(6).map(|n| n.name.as_str()).collect::<Vec<_>>().join(", "), 200)
}

/// The cover to show: the smallest image at least 300 pixels wide, else the largest.
fn pick_art(images: &[ImageJson]) -> Option<String> {
    let mut usable: Vec<(u32, String)> =
        images.iter().filter_map(|i| Some((i.width.unwrap_or(0), art_id(&i.url)?))).collect();
    usable.sort_by_key(|(w, _)| *w);
    usable.iter().find(|(w, _)| *w >= 300).or(usable.last()).map(|(_, id)| id.clone())
}

/// A device id the phone may hand back: printable ASCII without spaces or quotes, short.
fn device_id(id: &str) -> Option<String> {
    ((1..=64).contains(&id.len()) && id.bytes().all(|b| (0x21..0x7f).contains(&b) && b != b'"' && b != b'\\'))
        .then(|| id.to_string())
}

fn device(json: &DeviceJson) -> Option<Device> {
    Some(Device { id: device_id(json.id.as_deref()?)?, name: clean_text(&json.name, 80), active: json.is_active })
}

/// The player's state (`200`), or nothing playing (`204`).
pub fn parse_state(status: u16, body: &[u8]) -> Result<Option<WebState>, WebError> {
    if status == 204 || (status == 200 && body.iter().all(u8::is_ascii_whitespace)) {
        return Ok(None);
    }
    let p: PlayerJson = serde_json::from_slice(body)
        .map_err(|_| WebError::Api { status, message: "unexpected player state".into() })?;
    let mut state = WebState {
        playing: p.is_playing,
        position_ms: p.progress_ms,
        shuffle: p.shuffle_state,
        repeat: Repeat::parse(&p.repeat_state),
        volume: p.device.as_ref().and_then(|d| d.volume_percent).map(|v| v.min(100) as u8),
        device: p.device.as_ref().map(|d| clean_text(&d.name, 80)).filter(|n| !n.is_empty()),
        ..WebState::default()
    };
    if let Some(item) = &p.item {
        state.title = clean_text(&item.name, 200);
        state.uri = Uri::parse(&item.uri).map(|u| u.text());
        state.duration_ms = item.duration_ms;
        match item.kind.as_str() {
            "episode" => {
                let show = item.show.as_ref();
                state.artist = show.map(|s| clean_text(&s.name, 200)).unwrap_or_default();
                state.art = pick_art(&item.images).or_else(|| show.and_then(|s| pick_art(&s.images)));
            }
            _ => {
                state.artist = names(&item.artists);
                state.album = item.album.as_ref().map(|a| clean_text(&a.name, 200)).unwrap_or_default();
                state.art = item.album.as_ref().and_then(|a| pick_art(&a.images));
            }
        }
    }
    Ok(Some(state))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DevicesJson {
    devices: Vec<DeviceJson>,
}

pub fn parse_devices(body: &[u8]) -> Result<Vec<Device>, WebError> {
    let d: DevicesJson = serde_json::from_slice(body)
        .map_err(|_| WebError::Api { status: 200, message: "unexpected device list".into() })?;
    Ok(d.devices.iter().filter_map(device).take(16).collect())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OwnerJson {
    display_name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FoundJson {
    uri: String,
    name: String,
    artists: Vec<NamedJson>,
    album: Option<AlbumJson>,
    owner: Option<OwnerJson>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PageJson {
    // Spotify's search returns `null` in place of some playlists.
    items: Vec<Option<FoundJson>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchJson {
    tracks: Option<PageJson>,
    albums: Option<PageJson>,
    artists: Option<PageJson>,
    playlists: Option<PageJson>,
}

pub fn parse_search(body: &[u8]) -> Result<Vec<Found>, WebError> {
    let s: SearchJson = serde_json::from_slice(body)
        .map_err(|_| WebError::Api { status: 200, message: "unexpected search result".into() })?;
    let mut found = Vec::new();
    let pages = [(&s.tracks, "track"), (&s.albums, "album"), (&s.artists, "artist"), (&s.playlists, "playlist")];
    for (page, kind) in pages {
        let Some(page) = page else { continue };
        for item in page.items.iter().flatten().take(SEARCH_LIMIT) {
            let Some(uri) = Uri::parse(&item.uri).filter(|u| u.kind.word() == kind) else { continue };
            let subtitle = match kind {
                "track" => {
                    let album = item.album.as_ref().map(|a| clean_text(&a.name, 100)).unwrap_or_default();
                    format!("{} · {album}", names(&item.artists))
                }
                "album" => format!("Album · {}", names(&item.artists)),
                "artist" => "Artist".into(),
                _ => format!(
                    "Playlist · {}",
                    clean_text(item.owner.as_ref().and_then(|o| o.display_name.as_deref()).unwrap_or(""), 80)
                ),
            };
            found.push(Found {
                uri: uri.text(),
                title: clean_text(&item.name, 120),
                subtitle: clean_text(&subtitle, 160),
            });
        }
    }
    Ok(found)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiErrorBody {
    status: u16,
    message: String,
    reason: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiErrorJson {
    error: Option<ApiErrorBody>,
}

/// What a non-success answer to a command means.
pub fn parse_error(status: u16, body: &[u8]) -> WebError {
    let e = serde_json::from_slice::<ApiErrorJson>(body).ok().and_then(|e| e.error).unwrap_or_default();
    if status == 403 && (e.reason == "PREMIUM_REQUIRED" || e.message.to_ascii_lowercase().contains("premium required"))
    {
        return WebError::Premium;
    }
    if status == 404 && (e.reason == "NO_ACTIVE_DEVICE" || e.message.to_ascii_lowercase().contains("no active device"))
    {
        return WebError::NoDevice;
    }
    let message = clean_text(&e.message, 160);
    WebError::Api { status, message: if message.is_empty() { "request refused".into() } else { message } }
}

// ---------------------------------------------------------------- the client

/// The Web API, for one launch or one command.
pub struct Web {
    http: HttpClient,
    tokens: Arc<dyn TokenSource>,
    current: tokio::sync::Mutex<Option<(Secret, Instant)>>,
    limits: Mutex<Limits>,
}

/// `%`-encode a query value (RFC 3986 unreserved characters stay).
fn query_value(text: &str) -> String {
    String::from_utf8_lossy(&form_body(&[("", text.as_bytes())])[1..]).into_owned()
}

impl Web {
    pub fn new(http: HttpClient, tokens: Arc<dyn TokenSource>) -> Web {
        Web { http, tokens, current: tokio::sync::Mutex::new(None), limits: Mutex::new(Limits::new(Instant::now())) }
    }

    fn limits(&self) -> std::sync::MutexGuard<'_, Limits> {
        self.limits.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Get a first access token now (before step 9, so that a failure is said in the log).
    pub async fn warm(&self) -> Result<(), WebError> {
        self.token(false).await.map(|_| ())
    }

    /// Take one call of `class` from the local budget (art fetches use this too).
    pub fn take(&self, class: Class) -> Result<(), WebError> {
        self.limits().take(class, Instant::now())
    }

    async fn token(&self, refused: bool) -> Result<Secret, WebError> {
        let mut current = self.current.lock().await;
        // A token is used until a minute before it runs out, or until it is refused.
        if !refused
            && let Some((token, until)) = current.as_ref()
            && Instant::now() + Duration::from_secs(60) < *until
        {
            return token.try_clone().map_err(net);
        }
        let (token, valid) = self.tokens.token(refused).await?;
        if !token_shaped(&token) {
            return Err(WebError::SignedOut("the token source gave an unusable token".into()));
        }
        let copy = token.try_clone().map_err(net)?;
        *current = Some((token, Instant::now() + valid));
        Ok(copy)
    }

    /// One request to the API: budget, token (refreshed once on a `401`), `429` handling.
    /// Returns the status and body of anything else.
    async fn call(
        &self,
        class: Class,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(u16, Vec<u8>), WebError> {
        self.take(class)?;
        let body = body.map(|b| bytes::Bytes::from(b.to_string()));
        for attempt in 0..2 {
            let token = self.token(attempt == 1).await?;
            let mut bearer = Secret::with_capacity(token.len() + 7).map_err(net)?;
            bearer.push(b"Bearer ").map_err(net)?;
            bearer.push(token.expose()).map_err(net)?;
            let mut headers = vec![
                sensitive_header("authorization", bearer.expose()).map_err(net)?,
                header("accept", "application/json").map_err(net)?,
            ];
            drop(bearer);
            if body.is_some() {
                headers.push(header("content-type", "application/json").map_err(net)?);
            } else if matches!(method, Method::PUT | Method::POST) {
                // Spotify answers 411 to a body-less PUT or POST without a length.
                headers.push(header("content-length", "0").map_err(net)?);
            }
            let mut connection = self.http.connect(API_HOST).await.map_err(net)?;
            let mut out = Vec::new();
            let head = connection
                .call(method.clone(), &format!("/v1{path}"), &headers, body.clone(), REPLY_LIMIT, &mut out)
                .await
                .map_err(net)?;
            match head.status {
                401 if attempt == 0 => continue,
                401 => return Err(WebError::SignedOut("Spotify refused a freshly refreshed token".into())),
                429 => {
                    let seconds = retry_after(&head.headers);
                    self.limits().pause(seconds, Instant::now());
                    return Err(WebError::RateLimited { seconds });
                }
                status => return Ok((status, out)),
            }
        }
        unreachable!("the loop returns on its second pass")
    }

    async fn command(&self, method: Method, path: &str, body: Option<serde_json::Value>) -> Result<(), WebError> {
        let class = if path.starts_with("/me/player/queue") { Class::Queue } else { Class::Control };
        match self.call(class, method, path, body).await? {
            (200..=204, _) => Ok(()),
            (status, body) => Err(parse_error(status, &body)),
        }
    }

    pub async fn state(&self) -> Result<Option<WebState>, WebError> {
        match self.call(Class::Read, Method::GET, "/me/player?additional_types=episode", None).await? {
            (status @ (200 | 204), body) => parse_state(status, &body),
            (status, body) => Err(parse_error(status, &body)),
        }
    }

    pub async fn play(&self) -> Result<(), WebError> {
        self.command(Method::PUT, "/me/player/play", None).await
    }
    pub async fn pause(&self) -> Result<(), WebError> {
        self.command(Method::PUT, "/me/player/pause", None).await
    }
    pub async fn next(&self) -> Result<(), WebError> {
        self.command(Method::POST, "/me/player/next", None).await
    }
    pub async fn previous(&self) -> Result<(), WebError> {
        self.command(Method::POST, "/me/player/previous", None).await
    }
    pub async fn seek(&self, ms: u64) -> Result<(), WebError> {
        self.command(Method::PUT, &format!("/me/player/seek?position_ms={ms}"), None).await
    }
    pub async fn volume(&self, percent: u8) -> Result<(), WebError> {
        self.command(Method::PUT, &format!("/me/player/volume?volume_percent={}", percent.min(100)), None).await
    }
    pub async fn shuffle(&self, on: bool) -> Result<(), WebError> {
        self.command(Method::PUT, &format!("/me/player/shuffle?state={on}"), None).await
    }
    pub async fn repeat(&self, mode: Repeat) -> Result<(), WebError> {
        self.command(Method::PUT, &format!("/me/player/repeat?state={}", mode.word()), None).await
    }

    /// Play a track or episode, or start an album, playlist, artist or show.
    pub async fn open(&self, uri: &Uri) -> Result<(), WebError> {
        let body = if uri.kind.is_item() {
            serde_json::json!({ "uris": [uri.text()] })
        } else {
            serde_json::json!({ "context_uri": uri.text() })
        };
        self.command(Method::PUT, "/me/player/play", Some(body)).await
    }

    /// Add a track or episode to the queue.
    pub async fn queue(&self, uri: &Uri) -> Result<(), WebError> {
        self.command(Method::POST, &format!("/me/player/queue?uri={}", query_value(&uri.text())), None).await
    }

    pub async fn devices(&self) -> Result<Vec<Device>, WebError> {
        match self.call(Class::Read, Method::GET, "/me/player/devices", None).await? {
            (200, body) => parse_devices(&body),
            (status, body) => Err(parse_error(status, &body)),
        }
    }

    /// Move playback to a device the caller took from [`Web::devices`].
    pub async fn transfer(&self, device: &Device) -> Result<(), WebError> {
        self.command(Method::PUT, "/me/player", Some(serde_json::json!({ "device_ids": [device.id], "play": true })))
            .await
    }

    pub async fn search(&self, text: &str) -> Result<Vec<Found>, WebError> {
        let path = format!("/search?q={}&type=track,album,artist,playlist&limit={SEARCH_LIMIT}", query_value(text));
        match self.call(Class::Search, Method::GET, &path, None).await? {
            (200, body) => parse_search(&body),
            (status, body) => Err(parse_error(status, &body)),
        }
    }

    /// A cover image (JPEG) by its id, from the image server. Counted against the art budget.
    pub async fn art(&self, id: &str) -> Result<Vec<u8>, WebError> {
        if art_id(&format!("https://{IMAGE_HOST}/image/{id}")).is_none() {
            return Err(WebError::Api { status: 0, message: "not an image id".into() });
        }
        self.take(Class::Art)?;
        fetch_art(&self.http, id).await
    }
}

/// A cover image by id, without the API client (art for the desktop app's MPRIS player
/// needs no sign-in). The caller counts it against a budget.
pub async fn fetch_art(http: &HttpClient, id: &str) -> Result<Vec<u8>, WebError> {
    let url = format!("https://{IMAGE_HOST}/image/{id}");
    if art_id(&url).is_none() {
        return Err(WebError::Api { status: 0, message: "not an image id".into() });
    }
    let reply = http
        .request(Method::GET, &url, &Vec::new(), None, super::picture::MAX_INPUT_BYTES as u64)
        .await
        .map_err(net)?;
    match reply.status {
        200 => Ok(reply.body),
        status => Err(WebError::Api { status, message: "the image server refused".into() }),
    }
}

// ---------------------------------------------------------------- fuzzing

/// Every reply parser on arbitrary bytes: they return, and what they let through is clean.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    let clean = |s: &str| s.chars().all(|c| !c.is_control());
    for status in [200, 204, 400, 401, 403, 404, 429] {
        if let Ok(t) = parse_token_reply(status, data) {
            assert!(token_shaped(&t.access) && t.expires_in >= Duration::from_secs(60));
        }
        let _ = parse_error(status, data);
    }
    if let Ok(Some(state)) = parse_state(200, data) {
        assert!(clean(&state.title) && clean(&state.artist) && clean(&state.album));
        assert!(state.uri.iter().all(|u| Uri::parse(u).is_some()));
        assert!(state.volume.is_none_or(|v| v <= 100));
    }
    if let Ok(devices) = parse_devices(data) {
        assert!(devices.len() <= 16 && devices.iter().all(|d| device_id(&d.id).is_some() && clean(&d.name)));
    }
    if let Ok(found) = parse_search(data) {
        assert!(found.iter().all(|f| Uri::parse(&f.uri).is_some() && clean(&f.title) && clean(&f.subtitle)));
    }
    // The sign-in's loopback callback (any local process can connect to that port): a code
    // only ever comes with the right state.
    if let crate::auth::msa::Callback::Code(_) | crate::auth::msa::Callback::Denied(_) =
        crate::auth::msa::parse_callback_at(data, "GOOD-STATE", "/callback")
    {
        assert!(data.windows(16).any(|w| w == b"state=GOOD-STATE"), "a callback accepted without its state");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const STATE: &str = r#"{"device":{"id":"5fbb3ba6aa454b5534c4ba43a8c7e8e45a63ad0e","is_active":true,"is_private_session":false,"is_restricted":false,"name":"Kitchen speaker","type":"Speaker","volume_percent":59},"shuffle_state":false,"smart_shuffle":false,"repeat_state":"context","timestamp":1763047800000,"context":null,"progress_ms":44272,"item":{"album":{"album_type":"album","artists":[{"name":"Daft Punk"}],"images":[{"height":640,"url":"https://i.scdn.co/image/ab67616d0000b2734ce8b4e42588bf18182a1ad2","width":640},{"height":300,"url":"https://i.scdn.co/image/ab67616d00001e024ce8b4e42588bf18182a1ad2","width":300},{"height":64,"url":"https://i.scdn.co/image/ab67616d000048514ce8b4e42588bf18182a1ad2","width":64}],"name":"Discovery","uri":"spotify:album:2noRn2Aes5aoNVsU6iWThc"},"artists":[{"name":"Daft Punk"}],"duration_ms":224693,"explicit":false,"name":"Harder, Better, Faster, Stronger","type":"track","uri":"spotify:track:5W3cjX2J3tjhG8zb6u0qHn"},"currently_playing_type":"track","actions":{"disallows":{"resuming":true}},"is_playing":true}"#;

    #[test]
    fn the_player_state_is_read_and_cleaned() {
        let s = parse_state(200, STATE.as_bytes()).unwrap().unwrap();
        assert!(s.playing);
        assert_eq!(
            (s.title.as_str(), s.artist.as_str(), s.album.as_str()),
            ("Harder, Better, Faster, Stronger", "Daft Punk", "Discovery")
        );
        assert_eq!(s.uri.as_deref(), Some("spotify:track:5W3cjX2J3tjhG8zb6u0qHn"));
        assert_eq!((s.duration_ms, s.position_ms, s.volume), (Some(224_693), Some(44_272), Some(59)));
        assert_eq!((s.shuffle, s.repeat), (false, Some(Repeat::Context)));
        assert_eq!(s.device.as_deref(), Some("Kitchen speaker"));
        assert_eq!(s.art.as_deref(), Some("ab67616d00001e024ce8b4e42588bf18182a1ad2"), "the 300-pixel cover");
        assert_eq!(parse_state(204, b"").unwrap(), None);
        // Hostile text is cleaned; a URI or image of another shape is dropped.
        let hostile = STATE
            .replace("Harder, Better", "Harder\\u001b[2J\\u202e")
            .replace("spotify:track:5W3cjX2J3tjhG8zb6u0qHn", "javascript:alert(1)")
            .replace("https://i.scdn.co/image/ab67616d00001e02", "https://evil.example/image/ab67616d00001e02");
        let s = parse_state(200, hostile.as_bytes()).unwrap().unwrap();
        assert!(
            s.title.starts_with("Harder[2J") && !s.title.contains('\u{1b}') && !s.title.contains('\u{202e}'),
            "{}",
            s.title
        );
        assert_eq!(s.uri, None);
        assert_eq!(s.art.as_deref(), Some("ab67616d0000b2734ce8b4e42588bf18182a1ad2"), "the next usable cover");
    }

    #[test]
    fn devices_and_search_results_are_bounded_and_checked() {
        let devices = parse_devices(
            br#"{"devices":[{"id":"5fbb3ba6aa454b5534c4ba43a8c7e8e45a63ad0e","is_active":false,"name":"Kitchen","type":"Speaker","volume_percent":50},{"id":null,"name":"restricted"},{"id":"has space","name":"x"},{"id":"abc","is_active":true,"name":"Phone\u0007"}]}"#,
        )
        .unwrap();
        assert_eq!(
            devices,
            [
                Device { id: "5fbb3ba6aa454b5534c4ba43a8c7e8e45a63ad0e".into(), name: "Kitchen".into(), active: false },
                Device { id: "abc".into(), name: "Phone".into(), active: true },
            ]
        );
        let found = parse_search(
            br#"{"tracks":{"items":[{"uri":"spotify:track:5W3cjX2J3tjhG8zb6u0qHn","name":"Harder","artists":[{"name":"Daft Punk"}],"album":{"name":"Discovery"}},{"uri":"spotify:album:2noRn2Aes5aoNVsU6iWThc","name":"wrong kind"}]},"playlists":{"items":[null,{"uri":"spotify:playlist:37i9dQZF1DXcBWIGoYBM5M","name":"Mix","owner":{"display_name":"Spotify"}}]},"artists":{"items":[{"uri":"spotify:artist:4tZwfgrHOc3mvqYlEYSvVi","name":"Daft Punk"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            found,
            [
                Found {
                    uri: "spotify:track:5W3cjX2J3tjhG8zb6u0qHn".into(),
                    title: "Harder".into(),
                    subtitle: "Daft Punk · Discovery".into()
                },
                Found {
                    uri: "spotify:artist:4tZwfgrHOc3mvqYlEYSvVi".into(),
                    title: "Daft Punk".into(),
                    subtitle: "Artist".into()
                },
                Found {
                    uri: "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".into(),
                    title: "Mix".into(),
                    subtitle: "Playlist · Spotify".into()
                },
            ]
        );
    }

    #[test]
    fn premium_and_device_failures_are_recognised() {
        let premium = br#"{"error":{"status":403,"message":"Player command failed: Premium required","reason":"PREMIUM_REQUIRED"}}"#;
        assert_eq!(parse_error(403, premium), WebError::Premium);
        let no_device = br#"{"error":{"status":404,"message":"Player command failed: No active device found","reason":"NO_ACTIVE_DEVICE"}}"#;
        assert_eq!(parse_error(404, no_device), WebError::NoDevice);
        assert_eq!(parse_error(500, b"<html>"), WebError::Api { status: 500, message: "request refused".into() });
        let shown = WebError::Premium.to_string();
        assert!(shown.contains("Premium"), "{shown}");
    }

    #[test]
    fn token_replies_are_checked() {
        let ok = parse_token_reply(
            200,
            br#"{"access_token":"BQD-access","token_type":"Bearer","scope":"user-read-playback-state","expires_in":3600,"refresh_token":"AQD-refresh"}"#,
        )
        .unwrap();
        assert_eq!(ok.access.expose(), b"BQD-access");
        assert_eq!(ok.refresh.as_ref().map(|r| r.expose().to_vec()), Some(b"AQD-refresh".to_vec()));
        assert_eq!(ok.expires_in, Duration::from_secs(3600));
        let no_rotation =
            parse_token_reply(200, br#"{"access_token":"BQD","token_type":"bearer","expires_in":1}"#).unwrap();
        assert!(no_rotation.refresh.is_none() && no_rotation.expires_in == Duration::from_secs(60));
        for bad in [
            &br#"{"access_token":"has space","token_type":"Bearer"}"#[..],
            br#"{"access_token":"x","token_type":"mac"}"#,
            br#"{"access_token":"","token_type":"Bearer"}"#,
            br#"{"access_token":"x","token_type":"Bearer","refresh_token":"a\nb"}"#,
            b"not json",
        ] {
            assert!(parse_token_reply(200, bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        assert!(matches!(
            parse_token_reply(400, br#"{"error":"invalid_grant","error_description":"Refresh token revoked"}"#),
            Err(WebError::SignedOut(why)) if why == "Refresh token revoked"
        ));
    }

    #[test]
    fn local_budgets_and_spotifys_pause_hold_calls_back() {
        let t0 = Instant::now();
        let mut limits = Limits::new(t0);
        for _ in 0..3 {
            limits.take(Class::Search, t0).unwrap();
        }
        assert!(matches!(limits.take(Class::Search, t0), Err(WebError::Limited { seconds: 6 })));
        limits.take(Class::Control, t0).expect("another class has its own budget");
        limits.take(Class::Search, t0 + Duration::from_secs(6)).expect("refilled at ten a minute");
        // All classes together: 15 at once at most.
        let mut limits = Limits::new(t0);
        let mut taken = 0;
        for class in [Class::Control, Class::Read, Class::Search, Class::Queue, Class::Art].iter().cycle().take(40) {
            if limits.take(*class, t0).is_ok() {
                taken += 1;
            }
        }
        assert_eq!(taken, 15);
        // A 429 pauses everything.
        let mut limits = Limits::new(t0);
        limits.pause(30, t0);
        assert_eq!(limits.take(Class::Art, t0 + Duration::from_secs(10)), Err(WebError::RateLimited { seconds: 20 }));
        limits.take(Class::Art, t0 + Duration::from_secs(31)).unwrap();
        let mut h = hyper::HeaderMap::new();
        h.insert("retry-after", "120".parse().unwrap());
        assert_eq!(retry_after(&h), 120);
        h.insert("retry-after", "99999999".parse().unwrap());
        assert_eq!(retry_after(&h), 3600);
        h.insert("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap());
        assert_eq!(retry_after(&h), 5);
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(query_value("spotify:track:5W3cjX2J3tjhG8zb6u0qHn"), "spotify%3Atrack%3A5W3cjX2J3tjhG8zb6u0qHn");
        assert_eq!(query_value("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }
}
