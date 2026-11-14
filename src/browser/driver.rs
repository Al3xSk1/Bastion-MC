//! The browser's remote control, after the launcher has confined itself: one task that
//! talks WebDriver BiDi to Firefox over the bridge, decides about every request a page
//! makes, turns the phone's commands into input, and sends the phone what it shows.
//!
//! Requests: the driver asks Firefox to pause every request before it is sent
//! (`network.addIntercept`), and lets it go on only if it is `https:` (or `wss:`) to a host of
//! the open site and within the site's request budget; everything else is failed. A refused
//! navigation of the page the phone shows is said on the phone. (The relay holds the same
//! line on its own, by host name and TLS SNI, for anything that would not pass through here.)
//!
//! Frames: while the phone has asked for them and a site is open, the page is captured as a
//! JPEG at most `frame_rate` times a second; a capture that is identical to the last one is
//! not sent, and while the page does not change, captures become rarer (down to one a
//! second). Any input or navigation brings the full rate back.

use std::{
    collections::{HashMap, HashSet},
    os::unix::net::UnixStream as StdUnixStream,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    sync::{mpsc, oneshot, watch},
    time::timeout,
};

use super::{
    app::BrowserCommand,
    bidi::{self, Incoming, Refusal, Verdict},
    budget::Bucket,
    config::{MAX_VIEWPORT, Site, https_host},
    ws,
};
use crate::{
    egress::Logger,
    phone::{FrameFormat, MAX_FRAME_BYTES, Outbox},
};

pub const APP: &str = "browser";
/// How long Firefox may take to start and answer the WebSocket handshake.
const STARTUP: Duration = Duration::from_secs(90);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const SCREENSHOT_PATIENCE: Duration = Duration::from_secs(10);
const SLOWEST_FRAME_INTERVAL: Duration = Duration::from_secs(1);
/// Captures of another size than the page's that are dropped before they are sent anyway.
const WRONG_SIZE_PATIENCE: u32 = 5;
const MAX_TITLE: usize = 128;
/// The title is read this long after input, and at least this often while a site is open.
const TITLE_AFTER_INPUT: Duration = Duration::from_millis(300);
const TITLE_EVERY: Duration = Duration::from_secs(2);
const MAX_LOGGED_REFUSALS: usize = 256;

/// What the phone app and the launch send the driver.
pub enum DriverMsg {
    Hello(Outbox),
    Command(BrowserCommand, Outbox),
    /// Close the browser (gracefully: Firefox writes its profile) and stop.
    Stop(oneshot::Sender<()>),
    /// The browser's sandbox ended.
    Exited(String),
}

/// What the driver is started with.
pub struct Setup {
    pub sites: Vec<Site>,
    /// Per site: `browser login` was completed for it at some point.
    pub signed_in: Vec<bool>,
    /// Tells the relay which site is open.
    pub open_tx: watch::Sender<Option<usize>>,
    /// The launcher's end of the bridge to Firefox's remote control.
    pub bridge: StdUnixStream,
    pub frame_rate: u32,
    pub quality: u32,
    pub width: u32,
    pub height: u32,
    pub log: Logger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Ignore,
    Screenshot,
    Title,
    Navigate,
    Input,
}

enum Ending {
    Stopped,
    Broken(String),
    Exited(String),
}

struct Driver {
    sites: Vec<Site>,
    signed_in: Vec<bool>,
    /// Per site: every request, API calls.
    budgets: Vec<(Bucket, Bucket)>,
    open_tx: watch::Sender<Option<usize>>,
    writer: OwnedWriteHalf,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    context: Option<String>,
    open: Option<usize>,
    host: Option<String>,
    loading: bool,
    title: String,
    error: Option<String>,
    ready: bool,
    frames_on: bool,
    capture_sent: Option<Instant>,
    /// Captures in a row whose size was not the page size; the size is set again.
    wrong_size: u32,
    viewport_again: bool,
    next_title: Instant,
    last_frame: Vec<u8>,
    unchanged: u32,
    next_capture: Instant,
    base_interval: Duration,
    quality: f64,
    viewport: (u32, u32),
    pointer: (u32, u32),
    outbox: Option<Outbox>,
    state_dirty: bool,
    notice_at: HashMap<&'static str, Instant>,
    log: Logger,
    logged_refusals: HashSet<String>,
    refused: u64,
}

/// Run the driver until it is told to stop or its command channel closes.
pub async fn run(setup: Setup, mut commands: mpsc::Receiver<DriverMsg>) {
    let log = setup.log.clone();
    let sites = setup.sites.clone();
    let signed_in = setup.signed_in.clone();
    let viewport = (setup.width, setup.height);
    let ending = match connect(setup).await {
        Ok((mut driver, mut incoming)) => match driver.start(&mut incoming).await {
            Ok(()) => {
                log(format!(
                    "browser: ready — remote control established; {} site(s) can be opened from the phone",
                    driver.sites.len()
                ));
                driver.state_dirty = true;
                driver.flush_state();
                let ending = driver.serve(&mut incoming, &mut commands).await;
                if !matches!(ending, Ending::Stopped) {
                    let _ = driver.open_tx.send(None);
                }
                ending
            }
            Err(why) => Ending::Broken(why),
        },
        Err(why) => Ending::Broken(why),
    };
    let why = match ending {
        Ending::Stopped => return,
        Ending::Broken(why) => format!("the browser's remote control failed: {why}"),
        Ending::Exited(why) => format!("the browser stopped ({why})"),
    };
    log(format!("browser: {why}"));
    // From here on the phone is told that there is no browser, until the launch ends.
    let state = |out: &Outbox| {
        out.state(
            APP,
            &json!({
                "sites": sites.iter().zip(&signed_in).map(|(s, signed)| json!({"name": s.name, "title": s.title, "signed_in_hint": signed})).collect::<Vec<_>>(),
                "open": null, "host": null, "loading": false, "title": "", "ready": false,
                "error": format!("{why}; it starts again with the next launch"),
                "signed_in_hint": false, "frames": false, "width": viewport.0, "height": viewport.1,
            }),
        )
    };
    while let Some(msg) = commands.recv().await {
        match msg {
            DriverMsg::Hello(out) => {
                state(&out);
            }
            DriverMsg::Command(_, out) => {
                state(&out);
                out.notice(APP, "the browser is not running in this session");
            }
            DriverMsg::Stop(ack) => {
                let _ = ack.send(());
                return;
            }
            DriverMsg::Exited(_) => {}
        }
    }
}

/// The WebSocket handshake over the bridge; a reader task for what Firefox sends.
async fn connect(setup: Setup) -> Result<(Driver, mpsc::Receiver<Result<String, String>>), String> {
    setup.bridge.set_nonblocking(true).map_err(|e| format!("bridge: {e}"))?;
    let stream = tokio::net::UnixStream::from_std(setup.bridge).map_err(|e| format!("bridge: {e}"))?;
    let (mut read, mut writer) = stream.into_split();
    let key = ws::new_key().map_err(|e| e.to_string())?;
    let host = format!("127.0.0.1:{}", super::sandbox::REMOTE_PORT);
    writer
        .write_all(ws::handshake_request(&host, "/session", &key).as_bytes())
        .await
        .map_err(|e| format!("bridge: {e}"))?;
    let mut buf = Vec::with_capacity(1024);
    let end = timeout(STARTUP, async {
        loop {
            if let Some(end) = ws::head_end(&buf) {
                return Ok(end);
            }
            if buf.len() > ws::MAX_HANDSHAKE {
                return Err("the handshake answer is too long".to_string());
            }
            let mut chunk = [0u8; 1024];
            match read.read(&mut chunk).await {
                Ok(0) => return Err("Firefox ended before its remote control answered".to_string()),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(format!("bridge: {e}")),
            }
        }
    })
    .await
    .map_err(|_| "Firefox's remote control did not answer in time".to_string())??;
    ws::check_handshake(&buf[..end], &key)?;
    let mut decoder = ws::Decoder::new();
    decoder.push(&buf[end..]);
    let (tx, incoming) = mpsc::channel(256);
    tokio::spawn(read_messages(read, decoder, tx));
    let now = Instant::now();
    let base_interval =
        Duration::from_millis(1000 / u64::from(setup.frame_rate.clamp(1, super::config::MAX_FRAME_RATE)));
    let budgets = setup
        .sites
        .iter()
        .map(|s| {
            (
                Bucket::per_minute(u64::from(s.limits.requests_per_minute)),
                Bucket::per_minute(u64::from(s.limits.api_calls_per_minute)),
            )
        })
        .collect();
    Ok((
        Driver {
            budgets,
            signed_in: setup.signed_in,
            sites: setup.sites,
            open_tx: setup.open_tx,
            writer,
            next_id: 0,
            pending: HashMap::new(),
            context: None,
            open: None,
            host: None,
            loading: false,
            title: String::new(),
            error: None,
            ready: false,
            frames_on: false,
            capture_sent: None,
            wrong_size: 0,
            viewport_again: false,
            next_title: now,
            last_frame: Vec::new(),
            unchanged: 0,
            next_capture: now,
            base_interval,
            quality: f64::from(setup.quality.clamp(30, 90)) / 100.0,
            viewport: (setup.width, setup.height),
            pointer: (setup.width / 2, setup.height / 2),
            outbox: None,
            state_dirty: false,
            notice_at: HashMap::new(),
            log: setup.log,
            logged_refusals: HashSet::new(),
            refused: 0,
        },
        incoming,
    ))
}

async fn read_messages(mut read: OwnedReadHalf, mut decoder: ws::Decoder, tx: mpsc::Sender<Result<String, String>>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        loop {
            match decoder.next_message() {
                Ok(Some(ws::Message::Text(text))) => {
                    if tx.send(Ok(text)).await.is_err() {
                        return;
                    }
                }
                Ok(Some(ws::Message::Close(_))) => {
                    let _ = tx.send(Err("Firefox closed the remote control".into())).await;
                    return;
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(broken) => {
                    let _ = tx.send(Err(format!("the remote control broke the WebSocket protocol ({broken:?})"))).await;
                    return;
                }
            }
        }
        match read.read(&mut buf).await {
            Ok(0) | Err(_) => {
                let _ = tx.send(Err("the bridge to Firefox closed".into())).await;
                return;
            }
            Ok(n) => decoder.push(&buf[..n]),
        }
    }
}

impl Driver {
    async fn send(&mut self, method: &str, params: Value, pending: Pending) -> Result<u64, String> {
        self.next_id += 1;
        let id = self.next_id;
        let text = json!({"id": id, "method": method, "params": params}).to_string();
        let frame = ws::text_frame(&text).map_err(|e| e.to_string())?;
        self.writer.write_all(&frame).await.map_err(|e| format!("bridge: {e}"))?;
        self.pending.insert(id, pending);
        Ok(id)
    }

    /// Send a command and wait for its answer, handling whatever else arrives meanwhile.
    async fn call(
        &mut self,
        incoming: &mut mpsc::Receiver<Result<String, String>>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let id = self.send(method, params, Pending::Ignore).await?;
        let deadline = Instant::now() + CALL_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let text = match timeout(left, incoming.recv()).await {
                Err(_) => return Err(format!("{method}: no answer")),
                Ok(None) => return Err(format!("{method}: the remote control closed")),
                Ok(Some(Err(why))) => return Err(why),
                Ok(Some(Ok(text))) => text,
            };
            match bidi::parse_incoming(&text) {
                Some(Incoming::Reply { id: got, result }) if got == id => {
                    self.pending.remove(&id);
                    return Ok(result);
                }
                Some(Incoming::Error { id: Some(got), error, message }) if got == id => {
                    self.pending.remove(&id);
                    return Err(format!("{method}: {error}: {message}"));
                }
                Some(other) => self.handle(other).await,
                None => {}
            }
        }
    }

    /// The session, the browser's one page, the events and the request interception.
    async fn start(&mut self, incoming: &mut mpsc::Receiver<Result<String, String>>) -> Result<(), String> {
        let session = self
            .call(
                incoming,
                "session.new",
                json!({"capabilities": {"alwaysMatch": {
                    "acceptInsecureCerts": false,
                    "unhandledPromptBehavior": {"default": "dismiss", "beforeUnload": "accept"}
                }}}),
            )
            .await?;
        let version =
            session["capabilities"]["browserVersion"].as_str().map(|v| bidi::short(v, 32)).unwrap_or_default();
        let tree = self.call(incoming, "browsingContext.getTree", json!({"maxDepth": 0})).await?;
        let contexts: Vec<String> = tree["contexts"]
            .as_array()
            .map(|list| list.iter().filter_map(|c| c["context"].as_str().map(|s| bidi::short(s, 128))).collect())
            .unwrap_or_default();
        let Some(first) = contexts.first().cloned() else { return Err("Firefox has no page".into()) };
        for extra in &contexts[1..] {
            self.send("browsingContext.close", json!({"context": extra}), Pending::Ignore).await?;
        }
        // The page has the (headless) window's focus, so that keys reach what is focused in it.
        self.call(incoming, "browsingContext.activate", json!({"context": first})).await?;
        self.context = Some(first);
        self.call(incoming, "session.subscribe", json!({"events": bidi::EVENTS})).await?;
        let mut missing = Vec::new();
        for event in bidi::OPTIONAL_EVENTS {
            if self.call(incoming, "session.subscribe", json!({"events": [event]})).await.is_err() {
                missing.push(*event);
            }
        }
        if !missing.is_empty() {
            (self.log)(format!("browser: this Firefox does not report {} (not needed)", missing.join(", ")));
        }
        self.call(incoming, "network.addIntercept", json!({"phases": ["beforeRequestSent"]})).await?;
        self.set_viewport().await?;
        self.ready = true;
        (self.log)(format!("browser: Firefox {version} answers its remote control; every request is decided here"));
        Ok(())
    }

    async fn set_viewport(&mut self) -> Result<(), String> {
        let Some(context) = self.context.clone() else { return Ok(()) };
        let (width, height) = self.viewport;
        self.send(
            "browsingContext.setViewport",
            json!({"context": context, "viewport": {"width": width, "height": height}, "devicePixelRatio": 1}),
            Pending::Ignore,
        )
        .await
        .map(|_| ())
    }

    async fn serve(
        &mut self,
        incoming: &mut mpsc::Receiver<Result<String, String>>,
        commands: &mut mpsc::Receiver<DriverMsg>,
    ) -> Ending {
        let mut flush = tokio::time::interval(Duration::from_millis(100));
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            // A capture that was never answered does not stop the frames for good.
            if self.capture_sent.is_some_and(|at| at.elapsed() > SCREENSHOT_PATIENCE) {
                self.capture_sent = None;
            }
            let capture_due = self.capture_wanted();
            let at = tokio::time::Instant::from_std(self.next_capture);
            tokio::select! {
                message = incoming.recv() => match message {
                    Some(Ok(text)) => {
                        if let Some(parsed) = bidi::parse_incoming(&text) {
                            self.handle(parsed).await;
                        }
                    }
                    Some(Err(why)) => return Ending::Broken(why),
                    None => return Ending::Broken("the remote control closed".into()),
                },
                command = commands.recv() => match command {
                    Some(DriverMsg::Hello(out)) => {
                        // A phone that (re)connects has no picture yet: the next capture is
                        // sent even if the page has not changed since the last one.
                        self.outbox = Some(out);
                        self.last_frame.clear();
                        self.state_dirty = true;
                        self.flush_state();
                    }
                    Some(DriverMsg::Command(command, out)) => {
                        self.outbox = Some(out);
                        if let Err(why) = self.command(command).await {
                            return Ending::Broken(why);
                        }
                    }
                    Some(DriverMsg::Stop(ack)) => {
                        self.close_browser(incoming).await;
                        let _ = ack.send(());
                        return Ending::Stopped;
                    }
                    Some(DriverMsg::Exited(why)) => return Ending::Exited(why),
                    None => return Ending::Stopped,
                },
                _ = tokio::time::sleep_until(at), if capture_due => {
                    if let Err(why) = self.capture().await {
                        return Ending::Broken(why);
                    }
                }
                _ = flush.tick() => {
                    if self.ready && self.open.is_some() && Instant::now() >= self.next_title
                        && let Err(why) = self.read_title().await
                    {
                        return Ending::Broken(why);
                    }
                    self.flush_state();
                }
            }
        }
    }

    async fn close_browser(&mut self, incoming: &mut mpsc::Receiver<Result<String, String>>) {
        let _ = self.open_tx.send(None);
        // Firefox writes its profile and exits; the sandbox ends with it.
        let _ = timeout(Duration::from_secs(3), self.call(incoming, "browser.close", json!({}))).await;
        (self.log)("browser: closed".into());
    }

    async fn handle(&mut self, message: Incoming) {
        match message {
            Incoming::Reply { id, result } => self.on_reply(id, Ok(result)),
            Incoming::Error { id: Some(id), error, message } => self.on_reply(id, Err(format!("{error}: {message}"))),
            Incoming::Error { id: None, .. } => {}
            Incoming::Event { method, params } => {
                if let Err(why) = self.on_event(&method, &params).await {
                    (self.log)(format!("browser: {why}"));
                }
            }
        }
    }

    fn on_reply(&mut self, id: u64, result: Result<Value, String>) {
        match (self.pending.remove(&id), result) {
            (Some(Pending::Screenshot), result) => {
                self.capture_sent = None;
                if let Ok(result) = result {
                    self.frame_captured(&result);
                }
                self.next_capture = Instant::now() + self.interval();
            }
            (Some(Pending::Title), Ok(result)) => {
                if result["result"]["type"] == "string" {
                    let title = bidi::short(result["result"]["value"].as_str().unwrap_or_default(), MAX_TITLE);
                    if title != self.title {
                        self.title = title;
                        self.state_dirty = true;
                    }
                }
            }
            (Some(Pending::Navigate), Err(why)) => {
                self.loading = false;
                // A navigation the driver refused itself has been said already.
                if !why.contains("NS_ERROR_ABORT") {
                    self.error = Some("the page could not be loaded".into());
                }
                self.state_dirty = true;
            }
            (Some(Pending::Input), Err(_)) => self.notice("input", "the page did not take that input"),
            _ => {}
        }
    }

    fn frame_captured(&mut self, result: &Value) {
        let Some(data) = result["data"].as_str() else { return };
        // Base64 of at most a frame's size; anything bigger is not sent.
        if data.len() > MAX_FRAME_BYTES / 3 * 4 + 4 {
            return;
        }
        let Ok(bytes) = crate::util::b64_decode(data) else { return };
        if bytes == self.last_frame {
            self.unchanged = self.unchanged.saturating_add(1);
            return;
        }
        self.unchanged = 0;
        let Some((width, height)) = bidi::jpeg_size(&bytes) else { return };
        if u32::from(width) > MAX_VIEWPORT * 2 || u32::from(height) > MAX_VIEWPORT * 2 {
            return;
        }
        // While the first page replaces the browser's initial blank one, Firefox captures its
        // whole window instead of the page size it was given. Such a picture is not sent (the
        // phone maps its taps onto the picture) unless the size never settles.
        if (u32::from(width), u32::from(height)) != self.viewport {
            self.wrong_size = self.wrong_size.saturating_add(1);
            if self.wrong_size <= WRONG_SIZE_PATIENCE {
                self.viewport_again = true;
                return;
            }
        } else {
            self.wrong_size = 0;
        }
        if let Some(out) = &self.outbox {
            out.frame(APP, 0, width, height, FrameFormat::Jpeg, bytes.clone());
        }
        self.last_frame = bytes;
    }

    /// How long until the next capture: the frame rate, slower while nothing changes.
    fn interval(&self) -> Duration {
        let doublings = (self.unchanged / 8).min(4);
        (self.base_interval * (1u32 << doublings)).min(SLOWEST_FRAME_INTERVAL.max(self.base_interval))
    }

    fn capture_wanted(&self) -> bool {
        self.ready
            && self.frames_on
            && self.open.is_some()
            && self.context.is_some()
            && self.capture_sent.is_none()
            && self.outbox.is_some()
    }

    async fn capture(&mut self) -> Result<(), String> {
        let Some(context) = self.context.clone() else { return Ok(()) };
        if std::mem::take(&mut self.viewport_again) {
            self.set_viewport().await?;
        }
        self.capture_sent = Some(Instant::now());
        self.send(
            "browsingContext.captureScreenshot",
            json!({"context": context, "origin": "viewport", "format": {"type": "image/jpeg", "quality": self.quality}}),
            Pending::Screenshot,
        )
        .await
        .map(|_| ())
    }

    /// Input or navigation happened: frames at the full rate again, starting now.
    fn activity(&mut self) {
        self.unchanged = 0;
        let now = Instant::now();
        self.next_capture = self.next_capture.min(now + self.base_interval / 2);
        // Pages change their title without navigating (what was typed, unread counts).
        self.next_title = self.next_title.min(now + TITLE_AFTER_INPUT);
    }

    /// Read the page's title, from an isolated realm (the page's own scripts cannot see or
    /// change what runs there; they can only set the title, which is what is read).
    async fn read_title(&mut self) -> Result<(), String> {
        self.next_title = Instant::now() + TITLE_EVERY;
        let Some(context) = self.context.clone().filter(|_| self.open.is_some()) else { return Ok(()) };
        if self.pending.values().any(|p| *p == Pending::Title) {
            return Ok(());
        }
        self.send(
            "script.evaluate",
            json!({"expression": "document.title", "target": {"context": context, "sandbox": "bastion"}, "awaitPromise": false, "resultOwnership": "none"}),
            Pending::Title,
        )
        .await
        .map(|_| ())
    }

    fn ours(&self, params: &Value) -> bool {
        bidi::field(params, "context").is_some_and(|c| Some(c) == self.context.as_deref())
    }

    async fn on_event(&mut self, method: &str, params: &Value) -> Result<(), String> {
        match method {
            "network.beforeRequestSent" => {
                let Some(paused) = bidi::paused(params) else { return Ok(()) };
                let open = self.open;
                let site = open.and_then(|i| self.sites.get(i));
                let budget = open.and_then(|i| self.budgets.get(i));
                let verdict = bidi::decide(&paused.url, site, paused.api_call, |which| match (budget, which) {
                    (Some((requests, _)), bidi::Budget::Requests) => requests.take(1),
                    (Some((_, api)), bidi::Budget::ApiCalls) => api.take(1),
                    (None, _) => false,
                });
                match verdict {
                    Verdict::Continue => {
                        self.send("network.continueRequest", json!({"request": paused.request}), Pending::Ignore)
                            .await?;
                    }
                    Verdict::Fail { why, host } => {
                        self.refused += 1;
                        self.send("network.failRequest", json!({"request": paused.request}), Pending::Ignore).await?;
                        let shown = paused.navigation && paused.context.as_deref() == self.context.as_deref();
                        self.refusal(why, host.as_deref(), shown);
                    }
                }
            }
            // The host shown is the page's, once it is one: a navigation that is refused
            // or fails leaves the page (and its host) where it was.
            "browsingContext.navigationStarted" if self.ours(params) => {
                self.loading = true;
                self.error = None;
                self.state_dirty = true;
                self.activity();
            }
            "browsingContext.navigationFailed" | "browsingContext.navigationAborted" if self.ours(params) => {
                self.loading = false;
                self.state_dirty = true;
            }
            "browsingContext.domContentLoaded"
            | "browsingContext.load"
            | "browsingContext.fragmentNavigated"
            | "browsingContext.historyUpdated"
                if self.ours(params) =>
            {
                if method == "browsingContext.load" {
                    self.loading = false;
                }
                let url = bidi::field(params, "url").unwrap_or_default();
                let host = https_host(url);
                if host != self.host {
                    self.host = host;
                }
                self.state_dirty = true;
                self.activity();
                self.read_title().await?;
            }
            "browsingContext.contextCreated" => {
                let Some(context) = bidi::field(params, "context").map(|c| bidi::short(c, 128)) else { return Ok(()) };
                let top_level = params.get("parent").is_none_or(Value::is_null);
                if top_level && Some(context.as_str()) != self.context.as_deref() {
                    if self.context.is_none() {
                        self.context = Some(context);
                        self.set_viewport().await?;
                    } else {
                        self.send("browsingContext.close", json!({"context": context}), Pending::Ignore).await?;
                        self.notice("window", "the page tried to open another window; the browser shows one page");
                    }
                }
            }
            "browsingContext.contextDestroyed" if self.ours(params) => {
                self.context = None;
                self.send("browsingContext.create", json!({"type": "tab"}), Pending::Ignore).await?;
            }
            "browsingContext.userPromptOpened" if self.ours(params) => {
                let context = self.context.clone();
                self.send(
                    "browsingContext.handleUserPrompt",
                    json!({"context": context, "accept": false}),
                    Pending::Ignore,
                )
                .await?;
                self.notice("prompt", "a dialog of the page was dismissed");
            }
            "input.fileDialogOpened" => {
                self.notice("file", "the page asked for a file: the browser's sandbox has none to choose");
            }
            _ => {}
        }
        Ok(())
    }

    fn refusal(&mut self, why: Refusal, host: Option<&str>, shown: bool) {
        let site = self.open.and_then(|i| self.sites.get(i)).cloned();
        let site = site.as_ref();
        let host_text = host.unwrap_or("a page");
        let line = match (why, site) {
            (Refusal::Budget, Some(site)) => format!(
                "{}: the request limit ({} a minute) is reached; requests are refused until it refills",
                site.title, site.limits.requests_per_minute
            ),
            (Refusal::ApiBudget, Some(site)) => format!(
                "{}: the API call limit ({} a minute) is reached; calls are refused until it refills",
                site.title, site.limits.api_calls_per_minute
            ),
            (Refusal::OtherHost, Some(site)) => format!("{host_text} is not part of {}", site.title),
            (Refusal::NotHttps, _) => format!("{host_text}: only https pages of the open site can be shown"),
            _ => format!("{host_text}: no site is open"),
        };
        if matches!(why, Refusal::Budget | Refusal::ApiBudget) {
            self.notice("budget", &line);
        } else if shown {
            self.notice("navigation", &format!("refused: {line}"));
        }
        let key = format!("{why:?} {host_text}");
        if self.logged_refusals.len() < MAX_LOGGED_REFUSALS && self.logged_refusals.insert(key) {
            (self.log)(format!(
                "browser: refused {} {} ({why:?}{})",
                if shown { "navigation to" } else { "request to" },
                crate::util::sanitize_line(host_text.as_bytes(), 120),
                site.map(|s| format!(", site {}", s.name)).unwrap_or_default()
            ));
        }
    }

    fn notice(&mut self, kind: &'static str, text: &str) {
        let now = Instant::now();
        if self.notice_at.get(kind).is_some_and(|at| now.duration_since(*at) < Duration::from_secs(1)) {
            return;
        }
        self.notice_at.insert(kind, now);
        if let Some(out) = &self.outbox {
            out.notice(APP, text);
        }
    }

    fn state_json(&self) -> Value {
        let site = self.open.and_then(|i| self.sites.get(i));
        json!({
            "sites": self.sites.iter().zip(&self.signed_in).map(|(s, signed)| json!({"name": s.name, "title": s.title, "signed_in_hint": signed})).collect::<Vec<_>>(),
            "open": site.map(|s| s.name.clone()),
            "host": self.host.clone().filter(|_| site.is_some()),
            "loading": self.loading,
            "title": if site.is_some() { self.title.clone() } else { String::new() },
            "error": self.error.clone(),
            "signed_in_hint": self.open.and_then(|i| self.signed_in.get(i)).copied().unwrap_or(false),
            "ready": self.ready,
            "frames": self.frames_on,
            "width": self.viewport.0,
            "height": self.viewport.1,
            "refused": self.refused,
        })
    }

    fn flush_state(&mut self) {
        if !self.state_dirty {
            return;
        }
        if let Some(out) = &self.outbox
            && out.state(APP, &self.state_json())
        {
            self.state_dirty = false;
        }
    }

    async fn input(&mut self, actions: Value) -> Result<(), String> {
        let Some(context) = self.context.clone() else { return Ok(()) };
        self.send("input.performActions", json!({"context": context, "actions": actions}), Pending::Input).await?;
        self.activity();
        Ok(())
    }

    async fn command(&mut self, command: BrowserCommand) -> Result<(), String> {
        if !self.ready {
            self.notice("starting", "the browser is still starting");
            return Ok(());
        }
        let needs_site = !matches!(
            command,
            BrowserCommand::Open(_) | BrowserCommand::Close | BrowserCommand::Size(..) | BrowserCommand::Frames(_)
        );
        if needs_site && self.open.is_none() {
            self.notice("closed", "no site is open");
            return Ok(());
        }
        if self.context.is_none() {
            self.send("browsingContext.create", json!({"type": "tab"}), Pending::Ignore).await?;
            self.notice("starting", "the browser is preparing its page; try again");
            return Ok(());
        }
        let context = self.context.clone().unwrap_or_default();
        match command {
            BrowserCommand::Open(index) => {
                let Some(site) = self.sites.get(index).cloned() else { return Ok(()) };
                self.open = Some(index);
                let _ = self.open_tx.send(Some(index));
                self.send("input.releaseActions", json!({"context": context}), Pending::Ignore).await?;
                self.send(
                    "browsingContext.navigate",
                    json!({"context": context, "url": site.start, "wait": "none"}),
                    Pending::Navigate,
                )
                .await?;
                self.host = Some(site.start_host.clone());
                self.title = site.title.clone();
                self.loading = true;
                self.error = None;
                self.last_frame.clear();
                self.activity();
            }
            BrowserCommand::Close => {
                self.open = None;
                let _ = self.open_tx.send(None);
                self.send(
                    "browsingContext.navigate",
                    json!({"context": context, "url": "about:blank", "wait": "none"}),
                    Pending::Ignore,
                )
                .await?;
                self.host = None;
                self.title.clear();
                self.loading = false;
                self.error = None;
            }
            BrowserCommand::Back | BrowserCommand::Forward => {
                let delta = if command == BrowserCommand::Back { -1 } else { 1 };
                self.send(
                    "browsingContext.traverseHistory",
                    json!({"context": context, "delta": delta}),
                    Pending::Ignore,
                )
                .await?;
                self.activity();
            }
            BrowserCommand::Reload => {
                self.send("browsingContext.reload", json!({"context": context, "wait": "none"}), Pending::Ignore)
                    .await?;
                self.activity();
            }
            BrowserCommand::Size(width, height) => {
                self.viewport = (width, height);
                self.pointer = (self.pointer.0.min(width - 1), self.pointer.1.min(height - 1));
                self.set_viewport().await?;
                self.last_frame.clear();
                self.activity();
            }
            BrowserCommand::Mouse(kind, x, y, button) => {
                let (x, y) = (x.min(self.viewport.0 - 1), y.min(self.viewport.1 - 1));
                self.pointer = (x, y);
                self.input(bidi::pointer_actions(kind, x, y, button)).await?;
            }
            BrowserCommand::Scroll(dx, dy) => {
                let (x, y) = self.pointer;
                self.input(bidi::scroll_actions(x, y, dx, dy)).await?;
            }
            BrowserCommand::Key(key) => self.input(bidi::key_actions(key)).await?,
            BrowserCommand::Text(text) => {
                if !text.is_empty() {
                    self.input(bidi::text_actions(&text)).await?;
                }
            }
            BrowserCommand::Frames(on) => {
                self.frames_on = on;
                self.last_frame.clear();
                self.next_capture = Instant::now();
            }
        }
        self.state_dirty = true;
        Ok(())
    }
}
