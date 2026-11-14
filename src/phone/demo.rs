//! A stand-in for every service of the phone, for testing the phone in the real game without
//! Spotify, a browser, a recorder or a Realms account: five apps (`spotify`, `browser`,
//! `clips`, `realms`, `settings`) with the contract's vocabularies and limits, fake but
//! plausible state, pictures (album art as PNG, the browser's page and clip thumbnails as
//! RGBA8), and answers to every verb.
//!
//! **Only by an opt-in test switch** (`CompanionConfig::demo_phone`, which no configuration
//! file can set, or `BASTION_PHONE_DEMO=1` in the launcher's environment): nothing here is
//! real, and the launch log says so. Every command the demo receives is written to the launch
//! log as the dispatcher handed it over (made printable), so that a test can compare what the
//! phone sent with what arrived.
//!
//! The browser's page shows where the last `mouse down` landed (a cross) and what `text`
//! typed (in a 3×5 pixel font, upper case), so that a screenshot shows the coordinate mapping.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use super::{Command, FrameFormat, Outbox, PhoneApp, Service, VerbLimit};
use crate::{confine::Confinement, util};

/// Where the demo writes what it received (the launcher's own voice).
pub type Logger = Arc<dyn Fn(String) + Send + Sync>;

/// The code the demo's Realms prompt accepts.
pub const REALMS_CODE: &str = "424242";

/// The five demo services, one per app.
pub fn services(logger: Logger) -> Vec<Box<dyn Service>> {
    ["spotify", "browser", "clips", "realms", "settings"]
        .into_iter()
        .map(|name| Box::new(DemoService { name, logger: logger.clone() }) as Box<dyn Service>)
        .collect()
}

struct DemoService {
    name: &'static str,
    logger: Logger,
}

impl Service for DemoService {
    fn name(&self) -> &'static str {
        self.name
    }

    fn describe(&self) -> String {
        format!("phone: {} — DEMO (a test switch): fake state, nothing real behind it", self.name)
    }

    fn confinement_needs(&self, _keep: &mut Confinement) {}

    fn into_app(self: Box<Self>) -> Box<dyn PhoneApp> {
        let logger = self.logger;
        match self.name {
            "spotify" => Box::new(Spotify::new(logger)),
            "browser" => Box::new(Browser::new(logger)),
            "clips" => Box::new(Clips::new(logger)),
            "realms" => Box::new(Realms::new(logger)),
            _ => Box::new(Settings::new(logger)),
        }
    }
}

/// The command as it arrived, printable, for the launch log.
fn log_command(logger: &Logger, command: &Command) {
    let mut line = format!("phone demo: received {} {}", command.app, command.verb);
    for arg in &command.args {
        line.push(' ');
        line.push_str(arg);
    }
    if let Some(text) = &command.text {
        line.push_str(" -- ");
        line.push_str(text);
    }
    logger(util::sanitize_line(line.as_bytes(), 700));
}

fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A task that calls `tick` every second until the app is dropped.
struct Ticker(Option<tokio::task::JoinHandle<()>>);

impl Ticker {
    fn start(&mut self, mut tick: impl FnMut() + Send + 'static) {
        if self.0.is_none() && tokio::runtime::Handle::try_current().is_ok() {
            self.0 = Some(tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                interval.tick().await;
                loop {
                    interval.tick().await;
                    tick();
                }
            }));
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

// ---------------------------------------------------------------- pictures

/// A PNG of RGBA8 pixels: stored (uncompressed) deflate blocks, filter 0 on every row.
pub fn png(width: u16, height: u16, rgba: &[u8]) -> Vec<u8> {
    fn crc(data: &[&[u8]]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for part in data {
            for byte in *part {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
                }
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc(&[kind, data]).to_be_bytes());
    }
    let (w, h) = (usize::from(width), usize::from(height));
    let mut raw = Vec::with_capacity(h * (1 + 4 * w));
    for row in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[row * 4 * w..(row + 1) * 4 * w]);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&u32::from(width).to_be_bytes());
    ihdr.extend_from_slice(&u32::from(height).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// An RGBA8 picture to draw on.
struct Canvas {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize, colour: [u8; 3]) -> Canvas {
        let mut rgba = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            rgba.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
        }
        Canvas { width, height, rgba }
    }

    fn rect(&mut self, x: i64, y: i64, w: i64, h: i64, colour: [u8; 3]) {
        for yy in y.max(0)..(y + h).min(self.height as i64) {
            for xx in x.max(0)..(x + w).min(self.width as i64) {
                let at = (yy as usize * self.width + xx as usize) * 4;
                self.rgba[at..at + 3].copy_from_slice(&colour);
            }
        }
    }

    /// Text in a 3×5 font scaled by `scale`; letters upper case, unknown characters as a box.
    fn text(&mut self, x: i64, y: i64, scale: i64, text: &str, colour: [u8; 3]) {
        let mut cx = x;
        for c in text.chars() {
            if cx + 3 * scale > self.width as i64 {
                break;
            }
            let glyph = glyph(c.to_ascii_uppercase());
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..3 {
                    if bits & (0b100 >> col) != 0 {
                        self.rect(cx + col * scale, y + row as i64 * scale, scale, scale, colour);
                    }
                }
            }
            cx += 4 * scale;
        }
    }
}

/// A 3×5 glyph: five rows of three bits, the highest bit on the left.
fn glyph(c: char) -> [u8; 5] {
    match c {
        ' ' => [0, 0, 0, 0, 0],
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b110, 0b101, 0b101, 0b101, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b110, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b110, 0b001, 0b010, 0b100, 0b111],
        '3' => [0b110, 0b001, 0b010, 0b001, 0b110],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b110, 0b001, 0b110],
        '6' => [0b011, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b110],
        '.' => [0, 0, 0, 0, 0b010],
        ',' => [0, 0, 0, 0b010, 0b100],
        ':' => [0, 0b010, 0, 0b010, 0],
        '-' => [0, 0, 0b111, 0, 0],
        '_' => [0, 0, 0, 0, 0b111],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '!' => [0b010, 0b010, 0b010, 0, 0b010],
        '?' => [0b110, 0b001, 0b010, 0, 0b010],
        '(' => [0b001, 0b010, 0b010, 0b010, 0b001],
        ')' => [0b100, 0b010, 0b010, 0b010, 0b100],
        '\'' => [0b010, 0b010, 0, 0, 0],
        '@' => [0b111, 0b101, 0b111, 0b100, 0b011],
        '#' => [0b101, 0b111, 0b101, 0b111, 0b101],
        '+' => [0, 0b010, 0b111, 0b010, 0],
        '=' => [0, 0b111, 0, 0b111, 0],
        _ => [0b111, 0b101, 0b101, 0b101, 0b111],
    }
}

/// Album art: a gradient with the track number drawn on it, as PNG, 128×128.
fn album_art(track: usize) -> Vec<u8> {
    let (w, h) = (128usize, 128usize);
    let mut canvas = Canvas::new(w, h, [0, 0, 0]);
    let hue = [(30, 185, 84), (220, 80, 60), (70, 110, 230), (200, 160, 40)][track % 4];
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * 4;
            let shade = (x + y) as u32 * 255 / (w + h) as u32;
            canvas.rgba[at] = ((hue.0 as u32 * (255 - shade) + 20 * shade) / 255) as u8;
            canvas.rgba[at + 1] = ((hue.1 as u32 * (255 - shade) + 20 * shade) / 255) as u8;
            canvas.rgba[at + 2] = ((hue.2 as u32 * (255 - shade) + 30 * shade) / 255) as u8;
        }
    }
    canvas.text(12, 50, 6, &format!("{}", track + 1), [255, 255, 255]);
    png(w as u16, h as u16, &canvas.rgba)
}

// ---------------------------------------------------------------- spotify

const SPOTIFY_LIMITS: &[VerbLimit] = &[
    VerbLimit { verb: "play", per_minute: 60, burst: 5 },
    VerbLimit { verb: "pause", per_minute: 60, burst: 5 },
    VerbLimit { verb: "toggle", per_minute: 60, burst: 5 },
    VerbLimit { verb: "next", per_minute: 60, burst: 5 },
    VerbLimit { verb: "previous", per_minute: 60, burst: 5 },
    VerbLimit { verb: "seek", per_minute: 120, burst: 10 },
    VerbLimit { verb: "volume", per_minute: 240, burst: 20 },
    VerbLimit { verb: "shuffle", per_minute: 30, burst: 5 },
    VerbLimit { verb: "repeat", per_minute: 30, burst: 5 },
    VerbLimit { verb: "open", per_minute: 30, burst: 5 },
    VerbLimit { verb: "queue", per_minute: 30, burst: 5 },
    VerbLimit { verb: "search", per_minute: 20, burst: 3 },
    VerbLimit { verb: "devices", per_minute: 10, burst: 2 },
    VerbLimit { verb: "transfer", per_minute: 10, burst: 2 },
    VerbLimit { verb: "art", per_minute: 20, burst: 3 },
];

const TRACKS: &[(&str, &str, &str, u64)] = &[
    ("Lantern Light", "The Night Ferries", "Harbour Songs", 214_000),
    ("Cobblestone", "Mira Okafor", "Old Town", 187_000),
    ("Signal Fires", "The Night Ferries", "Harbour Songs", 241_000),
    ("Paper Planes at Dawn", "Quiet Engines", "Morning Static", 199_000),
];

struct SpotifyState {
    track: usize,
    playing: bool,
    position_ms: u64,
    since: u64,
    volume: u64,
    shuffle: bool,
    repeat: &'static str,
    results: Vec<Value>,
    devices: Vec<Value>,
    error: Option<String>,
}

impl SpotifyState {
    fn position(&self) -> u64 {
        let elapsed = if self.playing { now_millis().saturating_sub(self.since) } else { 0 };
        (self.position_ms + elapsed).min(TRACKS[self.track].3)
    }

    fn json(&self) -> Value {
        let (track, artist, album, duration) = TRACKS[self.track];
        json!({
            "source": "web", "playing": self.playing, "track": track, "artist": artist, "album": album,
            "duration_ms": duration, "position_ms": self.position(), "volume": self.volume,
            "shuffle": self.shuffle, "repeat": self.repeat, "results": self.results, "devices": self.devices,
            "error": self.error, "demo": true
        })
    }

    fn settle(&mut self) {
        self.position_ms = self.position();
        self.since = now_millis();
    }
}

struct Spotify {
    logger: Logger,
    state: Arc<Mutex<SpotifyState>>,
    ticker: Ticker,
}

impl Spotify {
    fn new(logger: Logger) -> Spotify {
        let state = SpotifyState {
            track: 0,
            playing: true,
            position_ms: 42_000,
            since: now_millis(),
            volume: 65,
            shuffle: false,
            repeat: "off",
            results: Vec::new(),
            devices: Vec::new(),
            error: None,
        };
        Spotify { logger, state: Arc::new(Mutex::new(state)), ticker: Ticker(None) }
    }

    fn send(&self, out: &Outbox) {
        let json = self.state.lock().unwrap().json();
        out.state("spotify", &json);
    }

    fn art(&self, out: &Outbox) {
        let track = self.state.lock().unwrap().track;
        out.frame("spotify", 0, 128, 128, FrameFormat::Png, album_art(track));
    }
}

impl PhoneApp for Spotify {
    fn name(&self) -> &'static str {
        "spotify"
    }

    fn limits(&self) -> &'static [VerbLimit] {
        SPOTIFY_LIMITS
    }

    fn hello(&mut self, out: &Outbox) {
        self.send(out);
        self.art(out);
        let (state, out) = (self.state.clone(), out.clone());
        self.ticker.start(move || {
            let json = state.lock().unwrap().json();
            out.state("spotify", &json);
        });
    }

    fn command(&mut self, command: Command, out: &Outbox) {
        log_command(&self.logger, &command);
        let arg = command.args.first().map(String::as_str);
        let mut art = false;
        {
            let mut s = self.state.lock().unwrap();
            s.settle();
            s.error = None;
            match (command.verb.as_str(), arg) {
                ("play", _) => s.playing = true,
                ("pause", _) => s.playing = false,
                ("toggle", _) => s.playing = !s.playing,
                ("next", _) => {
                    s.track = (s.track + 1) % TRACKS.len();
                    s.position_ms = 0;
                    art = true;
                }
                ("previous", _) => {
                    s.track = (s.track + TRACKS.len() - 1) % TRACKS.len();
                    s.position_ms = 0;
                    art = true;
                }
                ("seek", Some(ms)) => match ms.parse::<u64>() {
                    Ok(ms) => s.position_ms = ms.min(TRACKS[s.track].3),
                    Err(_) => s.error = Some("seek needs milliseconds".into()),
                },
                ("volume", Some(v)) => match v.parse::<u64>() {
                    Ok(v) if v <= 100 => s.volume = v,
                    _ => s.error = Some("volume is 0 to 100".into()),
                },
                ("shuffle", Some("on")) => s.shuffle = true,
                ("shuffle", Some("off")) => s.shuffle = false,
                ("repeat", Some("off")) => s.repeat = "off",
                ("repeat", Some("track")) => s.repeat = "track",
                ("repeat", Some("context")) => s.repeat = "context",
                ("open", Some(uri)) | ("queue", Some(uri)) => {
                    if uri.starts_with("spotify:") {
                        if command.verb == "open" {
                            s.track = (s.track + 2) % TRACKS.len();
                            s.position_ms = 0;
                            s.playing = true;
                            art = true;
                        }
                    } else {
                        s.error = Some("only spotify: links".into());
                    }
                }
                ("search", _) => {
                    let query = command.text.clone().unwrap_or_default();
                    s.results = TRACKS
                        .iter()
                        .enumerate()
                        .map(|(i, (title, artist, album, _))| {
                            json!({"uri": format!("spotify:track:demo{i}{}", "a".repeat(17)), "title": title,
                                   "subtitle": format!("{artist} · {album} (for \"{query}\")")})
                        })
                        .collect();
                }
                ("devices", _) => {
                    s.devices = vec![
                        json!({"id": "demo-desktop", "name": "This computer (demo)", "active": true}),
                        json!({"id": "demo-speaker", "name": "Kitchen speaker (demo)", "active": false}),
                    ];
                }
                ("transfer", Some(id)) => {
                    for device in &mut s.devices {
                        let active = device["id"] == id;
                        device["active"] = Value::Bool(active);
                    }
                }
                ("art", _) => art = true,
                _ => s.error = Some(format!("{} needs an argument", command.verb)),
            }
        }
        self.send(out);
        if art {
            self.art(out);
        }
    }
}

// ---------------------------------------------------------------- browser

const BROWSER_LIMITS: &[VerbLimit] = &[
    VerbLimit { verb: "open", per_minute: 20, burst: 3 },
    VerbLimit { verb: "close", per_minute: 20, burst: 3 },
    VerbLimit { verb: "back", per_minute: 60, burst: 5 },
    VerbLimit { verb: "forward", per_minute: 60, burst: 5 },
    VerbLimit { verb: "reload", per_minute: 20, burst: 3 },
    VerbLimit { verb: "size", per_minute: 30, burst: 5 },
    VerbLimit { verb: "mouse", per_minute: 1800, burst: 60 },
    VerbLimit { verb: "scroll", per_minute: 600, burst: 30 },
    VerbLimit { verb: "key", per_minute: 600, burst: 30 },
    VerbLimit { verb: "text", per_minute: 300, burst: 20 },
    VerbLimit { verb: "frames", per_minute: 30, burst: 5 },
];

const SITES: &[(&str, &str)] = &[("discord", "Discord"), ("wiki", "Minecraft Wiki"), ("maps", "Server map")];

struct Browser {
    logger: Logger,
    open: Option<&'static str>,
    width: u16,
    height: u16,
    mouse: Option<(i64, i64)>,
    typed: String,
    frames: bool,
    history: usize,
}

impl Browser {
    fn new(logger: Logger) -> Browser {
        Browser {
            logger,
            open: None,
            width: 360,
            height: 640,
            mouse: None,
            typed: String::new(),
            frames: false,
            history: 0,
        }
    }

    fn json(&self) -> Value {
        let sites: Vec<Value> = SITES.iter().map(|(name, title)| json!({"name": name, "title": title})).collect();
        json!({
            "sites": sites, "open": self.open, "host": self.open.map(|n| format!("{n}.example (demo)")),
            "loading": false, "title": self.open.map(|n| format!("{n} — demo page")), "error": null,
            "signed_in_hint": self.open == Some("discord"), "demo": true
        })
    }

    fn page(&self, out: &Outbox) {
        if !self.frames || self.open.is_none() {
            return;
        }
        let (w, h) = (usize::from(self.width), usize::from(self.height));
        let mut page = Canvas::new(w, h, [24, 26, 32]);
        page.rect(0, 0, w as i64, 22, [54, 57, 63]);
        page.text(6, 6, 2, &format!("{} DEMO PAGE {}X{}", self.open.unwrap_or(""), w, h), [220, 220, 220]);
        for y in (40..h as i64).step_by(40) {
            page.rect(0, y, w as i64, 1, [40, 42, 50]);
        }
        let typed: String = self.typed.chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
        page.text(6, 30, 2, &format!("TYPED: {typed}"), [120, 200, 255]);
        if let Some((x, y)) = self.mouse {
            page.rect(x - 8, y, 17, 1, [255, 80, 80]);
            page.rect(x, y - 8, 1, 17, [255, 80, 80]);
            page.text(6, h as i64 - 14, 2, &format!("MOUSE {x},{y}"), [255, 120, 120]);
        }
        out.frame("browser", 0, self.width, self.height, FrameFormat::Rgba8, page.rgba);
    }
}

impl PhoneApp for Browser {
    fn name(&self) -> &'static str {
        "browser"
    }

    fn limits(&self) -> &'static [VerbLimit] {
        BROWSER_LIMITS
    }

    fn hello(&mut self, out: &Outbox) {
        out.state("browser", &self.json());
        self.page(out);
    }

    fn command(&mut self, command: Command, out: &Outbox) {
        log_command(&self.logger, &command);
        let args: Vec<&str> = command.args.iter().map(String::as_str).collect();
        let mut state_changed = true;
        match (command.verb.as_str(), args.as_slice()) {
            ("open", [site]) => match SITES.iter().find(|(name, _)| name == site) {
                Some((name, _)) => {
                    self.open = Some(name);
                    self.history += 1;
                }
                None => {
                    out.notice("browser", "that site is not in the list");
                }
            },
            ("close", []) => self.open = None,
            ("back", []) | ("forward", []) | ("reload", []) => {}
            ("size", [w, h]) => {
                if let (Ok(w), Ok(h)) = (w.parse::<u16>(), h.parse::<u16>())
                    && (64..=1280).contains(&w)
                    && (64..=1280).contains(&h)
                {
                    (self.width, self.height) = (w, h);
                }
            }
            ("mouse", [action, x, y, ..]) => {
                state_changed = false;
                if let (Ok(x), Ok(y)) = (x.parse::<i64>(), y.parse::<i64>())
                    && *action == "down"
                {
                    self.mouse = Some((x, y));
                }
            }
            ("scroll", [_, _]) | ("key", [_]) => state_changed = false,
            ("text", []) => {
                state_changed = false;
                self.typed.push_str(command.text.as_deref().unwrap_or(""));
                let excess = self.typed.chars().count().saturating_sub(200);
                self.typed = self.typed.chars().skip(excess).collect();
            }
            ("frames", ["on"]) => self.frames = true,
            ("frames", ["off"]) => self.frames = false,
            _ => {
                out.notice("browser", "the demo does not understand that");
            }
        }
        if state_changed {
            out.state("browser", &self.json());
        }
        self.page(out);
    }
}

// ---------------------------------------------------------------- clips

const CLIPS_LIMITS: &[VerbLimit] = &[
    VerbLimit { verb: "status", per_minute: 30, burst: 5 },
    VerbLimit { verb: "save", per_minute: 6, burst: 2 },
    VerbLimit { verb: "list", per_minute: 20, burst: 3 },
    VerbLimit { verb: "delete", per_minute: 20, burst: 3 },
    VerbLimit { verb: "rename", per_minute: 20, burst: 3 },
    VerbLimit { verb: "thumb", per_minute: 120, burst: 32 },
];

struct Clip {
    id: String,
    name: String,
    created: u64,
    seconds: u64,
}

struct Clips {
    logger: Logger,
    clips: Vec<Clip>,
    next: usize,
}

impl Clips {
    fn new(logger: Logger) -> Clips {
        let base = now_millis() / 1000;
        let clips = [("Creeper ambush", 30), ("Sunset over the ferry", 45), ("Nether portal", 20)]
            .iter()
            .enumerate()
            .map(|(i, (name, seconds))| Clip {
                id: format!("demo{i}"),
                name: (*name).into(),
                created: base - 3600 * (3 - i as u64),
                seconds: *seconds,
            })
            .collect();
        Clips { logger, clips, next: 3 }
    }

    fn json(&self) -> Value {
        let clips: Vec<Value> = self
            .clips
            .iter()
            .map(|c| {
                json!({"id": c.id, "name": c.name, "created": c.created, "seconds": c.seconds,
                            "bytes": c.seconds * 650_000})
            })
            .collect();
        json!({"recording": true, "buffer_seconds": 60, "clips": clips, "folder": "~/Videos/bastion-mc (demo)",
               "error": null, "demo": true})
    }

    fn thumb(&self, out: &Outbox, index: usize) {
        let Some(clip) = self.clips.get(index) else { return };
        let mut canvas = Canvas::new(96, 54, [30 + (index as u8 * 40) % 200, 60, 90]);
        canvas.rect(0, 40, 96, 14, [10, 10, 10]);
        canvas.text(3, 44, 1, &format!("{} {}S", clip.id, clip.seconds), [255, 255, 255]);
        out.frame("clips", index as u8, 96, 54, FrameFormat::Rgba8, canvas.rgba);
    }
}

impl PhoneApp for Clips {
    fn name(&self) -> &'static str {
        "clips"
    }

    fn limits(&self) -> &'static [VerbLimit] {
        CLIPS_LIMITS
    }

    fn hello(&mut self, out: &Outbox) {
        out.state("clips", &self.json());
    }

    fn command(&mut self, command: Command, out: &Outbox) {
        log_command(&self.logger, &command);
        let arg = command.args.first().map(String::as_str);
        match (command.verb.as_str(), arg) {
            ("save", seconds) => {
                let seconds = seconds.and_then(|s| s.parse::<u64>().ok()).unwrap_or(30).clamp(1, 60);
                self.clips.push(Clip {
                    id: format!("demo{}", self.next),
                    name: format!("Clip {}", self.next + 1),
                    created: now_millis() / 1000,
                    seconds,
                });
                self.next += 1;
                out.notice("clips", &format!("saved the last {seconds} seconds (demo)"));
            }
            ("list", _) => {
                for index in 0..self.clips.len().min(32) {
                    self.thumb(out, index);
                }
            }
            ("delete", Some(id)) => self.clips.retain(|c| c.id != id),
            ("rename", Some(id)) => {
                let name = command.text.clone().unwrap_or_default();
                if let Some(clip) = self.clips.iter_mut().find(|c| c.id == id) {
                    clip.name = name.chars().take(60).collect();
                }
            }
            ("thumb", Some(id)) => {
                if let Some(index) = self.clips.iter().position(|c| c.id == id) {
                    self.thumb(out, index);
                }
            }
            _ => {}
        }
        out.state("clips", &self.json());
    }
}

// ---------------------------------------------------------------- realms

const REALMS_LIMITS: &[VerbLimit] =
    &[VerbLimit { verb: "code", per_minute: 6, burst: 3 }, VerbLimit { verb: "cancel", per_minute: 10, burst: 3 }];

struct RealmsState {
    expires_in: u64,
    attempts_left: u64,
    prompt: bool,
    last: Option<&'static str>,
    quiet: u64,
}

impl RealmsState {
    fn json(&self) -> Value {
        let prompt = self.prompt.then(|| {
            json!({"world": "Demo Realm", "address": "demo.realms.example:25565", "expires_in": self.expires_in,
                   "attempts_left": self.attempts_left})
        });
        json!({"mode": "code", "prompt": prompt, "last": self.last, "demo": true})
    }

    fn renew(&mut self) {
        (self.prompt, self.expires_in, self.attempts_left, self.quiet) = (true, 120, 3, 0);
    }
}

struct Realms {
    logger: Logger,
    state: Arc<Mutex<RealmsState>>,
    ticker: Ticker,
}

impl Realms {
    fn new(logger: Logger) -> Realms {
        let mut state = RealmsState { expires_in: 0, attempts_left: 0, prompt: false, last: None, quiet: 0 };
        state.renew();
        Realms { logger, state: Arc::new(Mutex::new(state)), ticker: Ticker(None) }
    }
}

impl PhoneApp for Realms {
    fn name(&self) -> &'static str {
        "realms"
    }

    fn limits(&self) -> &'static [VerbLimit] {
        REALMS_LIMITS
    }

    fn hello(&mut self, out: &Outbox) {
        out.state("realms", &self.state.lock().unwrap().json());
        (self.logger)(format!("phone demo: the Realms prompt accepts the code {REALMS_CODE}"));
        let (state, out) = (self.state.clone(), out.clone());
        self.ticker.start(move || {
            let mut s = state.lock().unwrap();
            if s.prompt {
                s.expires_in = s.expires_in.saturating_sub(1);
                if s.expires_in == 0 {
                    (s.prompt, s.last) = (false, Some("expired"));
                }
            } else {
                // A new prompt a while after the last one ended.
                s.quiet += 1;
                if s.quiet >= 20 {
                    s.renew();
                }
            }
            out.state("realms", &s.json());
        });
    }

    fn command(&mut self, command: Command, out: &Outbox) {
        log_command(&self.logger, &command);
        {
            let mut s = self.state.lock().unwrap();
            match command.verb.as_str() {
                "code" if s.prompt => {
                    let code = command.text.as_deref().unwrap_or("").trim().to_string();
                    if code == REALMS_CODE {
                        (s.prompt, s.last, s.quiet) = (false, Some("approved"), 0);
                        out.notice("realms", "approved (demo)");
                    } else {
                        s.attempts_left = s.attempts_left.saturating_sub(1);
                        if s.attempts_left == 0 {
                            (s.prompt, s.last, s.quiet) = (false, Some("refused"), 0);
                        }
                    }
                }
                "cancel" if s.prompt => (s.prompt, s.last, s.quiet) = (false, Some("refused"), 0),
                _ => {}
            }
        }
        out.state("realms", &self.state.lock().unwrap().json());
    }
}

// ---------------------------------------------------------------- settings

const SETTINGS_LIMITS: &[VerbLimit] =
    &[VerbLimit { verb: "status", per_minute: 30, burst: 5 }, VerbLimit { verb: "sync-now", per_minute: 6, burst: 2 }];

struct Settings {
    logger: Logger,
    last_sync: u64,
}

impl Settings {
    fn new(logger: Logger) -> Settings {
        Settings { logger, last_sync: now_millis() / 1000 - 600 }
    }

    fn json(&self) -> Value {
        json!({"sync": true, "last_sync": self.last_sync, "keys": 37, "instances": 4, "error": null, "demo": true})
    }
}

impl PhoneApp for Settings {
    fn name(&self) -> &'static str {
        "settings"
    }

    fn limits(&self) -> &'static [VerbLimit] {
        SETTINGS_LIMITS
    }

    fn hello(&mut self, out: &Outbox) {
        out.state("settings", &self.json());
    }

    fn command(&mut self, command: Command, out: &Outbox) {
        log_command(&self.logger, &command);
        if command.verb == "sync-now" {
            self.last_sync = now_millis() / 1000;
            out.notice("settings", "settings synced (demo)");
        }
        out.state("settings", &self.json());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone::{Outbound, dispatch::Dispatcher};

    fn demo() -> (Dispatcher, Arc<Mutex<Vec<String>>>, tokio::sync::mpsc::Receiver<Outbound>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let logger: Logger = Arc::new(move |line| sink.lock().unwrap().push(line));
        let apps = services(logger.clone()).into_iter().map(|s| s.into_app()).collect();
        let (out, rx) = Outbox::new(256);
        let dispatcher = Dispatcher::new(apps, out, Arc::new(move |line| logger(line)));
        (dispatcher, log, rx)
    }

    fn drain(rx: &mut tokio::sync::mpsc::Receiver<Outbound>) -> Vec<Outbound> {
        let mut all = Vec::new();
        while let Ok(m) = rx.try_recv() {
            all.push(m);
        }
        all
    }

    #[test]
    fn the_demo_announces_all_five_apps_and_sends_state_and_art() {
        let (mut dispatcher, _, mut rx) = demo();
        assert_eq!(dispatcher.app_names(), ["spotify", "browser", "clips", "realms", "settings"]);
        dispatcher.hello();
        let sent = drain(&mut rx);
        let Outbound::State { app: "phone", json } = &sent[0] else { panic!("{sent:?}") };
        assert_eq!(json, r#"{"apps":["spotify","browser","clips","realms","settings"],"theme":"dark"}"#);
        for app in ["spotify", "browser", "clips", "realms", "settings"] {
            assert!(sent.iter().any(|m| matches!(m, Outbound::State { app: a, .. } if *a == app)), "{app}");
        }
        let art = sent.iter().find_map(|m| match m {
            Outbound::Frame { app: "spotify", format: FrameFormat::Png, bytes, .. } => Some(bytes.clone()),
            _ => None,
        });
        assert!(art.unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
        for m in &sent {
            if let Outbound::State { json, .. } = m {
                assert!(serde_json::from_str::<Value>(json).unwrap().is_object());
            }
        }
    }

    #[test]
    fn every_verb_of_every_app_is_answered_and_logged_verbatim() {
        let (mut dispatcher, log, mut rx) = demo();
        let t = Duration::from_secs(1);
        for line in [
            "spotify next",
            "spotify search -- lantern light",
            "spotify volume 30",
            "spotify devices",
            "spotify transfer demo-speaker",
            "browser open discord",
            "browser size 320 480",
            "browser frames on",
            "browser mouse down 100 200 left",
            "browser text -- hello phone",
            "clips list",
            "clips save 15",
            "clips rename demo1 -- Best moment",
            "realms code -- 111111",
            "realms code -- 424242",
            "settings sync-now",
        ] {
            assert_eq!(dispatcher.command(line.as_bytes(), t), Ok(()), "{line}");
        }
        let logged = log.lock().unwrap().clone();
        assert!(logged.contains(&"phone demo: received browser text -- hello phone".to_string()), "{logged:#?}");
        assert!(logged.contains(&"phone demo: received browser mouse down 100 200 left".to_string()));
        let sent = drain(&mut rx);
        // The browser's page at the size asked, as RGBA8.
        let page = sent.iter().rev().find_map(|m| match m {
            Outbound::Frame { app: "browser", width, height, format: FrameFormat::Rgba8, bytes, .. } => {
                Some((*width, *height, bytes.len()))
            }
            _ => None,
        });
        assert_eq!(page, Some((320, 480, 320 * 480 * 4)));
        // Thumbnails after `list`, one per clip, in their slots.
        let thumbs: Vec<u8> = sent
            .iter()
            .filter_map(|m| match m {
                Outbound::Frame { app: "clips", slot, .. } => Some(*slot),
                _ => None,
            })
            .collect();
        assert_eq!(thumbs, [0, 1, 2]);
        let realms = sent.iter().rev().find_map(|m| match m {
            Outbound::State { app: "realms", json } => Some(json.clone()),
            _ => None,
        });
        assert!(realms.unwrap().contains(r#""last":"approved""#));
        assert!(sent.iter().any(|m| matches!(m, Outbound::Notice { app: "settings", .. })));
    }

    #[test]
    fn the_png_encoder_writes_what_a_decoder_reads_back() {
        let rgba: Vec<u8> = (0..70 * 300 * 4).map(|i| (i * 7 % 251) as u8).collect();
        let bytes = png(70, 300, &rgba);
        let decoder = png_decode(&bytes);
        assert_eq!(decoder, rgba, "stored deflate blocks across the 64 KiB boundary read back exactly");
    }

    /// Reads back a PNG written by [`png`] (stored blocks only): the test's own check.
    fn png_decode(bytes: &[u8]) -> Vec<u8> {
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let mut at = 8;
        let (mut width, mut zlib) = (0usize, Vec::new());
        while at < bytes.len() {
            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let data = &bytes[at + 8..at + 8 + len];
            match kind {
                b"IHDR" => width = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize,
                b"IDAT" => zlib.extend_from_slice(data),
                _ => {}
            }
            at += 12 + len;
        }
        let mut raw = Vec::new();
        let mut i = 2;
        loop {
            let last = zlib[i] & 1;
            let len = u16::from_le_bytes([zlib[i + 1], zlib[i + 2]]) as usize;
            raw.extend_from_slice(&zlib[i + 5..i + 5 + len]);
            i += 5 + len;
            if last == 1 {
                break;
            }
        }
        raw.chunks(1 + 4 * width).flat_map(|row| row[1..].to_vec()).collect()
    }
}
