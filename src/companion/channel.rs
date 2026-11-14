//! The launcher's end of the companion channel: one UNIX socket, bound into the sandbox, on
//! which the launcher listens.
//!
//! **Everything that arrives is untrusted.** The companion mod connects to this socket, but
//! so can every other mod and every exploit in the same JVM, and nothing distinguishes
//! them: a "hello" proves nothing about who sent it. Version 1 of the protocol therefore
//! carries information only, in one direction, and the launcher does exactly one thing with
//! it — it writes it to the launch log, as a claim made from inside the game
//! ([`Voice::Claimed`], logged like the game's own output). Nothing received changes what
//! the launcher does.
//!
//! What bounds a hostile peer:
//!
//! * **memory** — a line is at most [`MAX_LINE`] bytes; a longer one is discarded as it
//!   arrives, without being buffered. At most [`MAX_CONNECTIONS`] connections are open at
//!   once and [`MAX_ACCEPTED`] are accepted in a session; further ones are closed unread.
//! * **time** — each connection is read at no more than [`BYTES_PER_SECOND`], so the
//!   launcher's work is bounded however fast the peer writes (a peer that writes faster
//!   blocks on its own socket).
//! * **the log** — at most [`BURST`] lines at once and [`PER_SECOND`] per second after
//!   that, for all connections together, and [`MAX_LOGGED`] in a session; the rest is
//!   counted, not written (the launcher says so when the 1st, 10th, 100th, … line is
//!   dropped, which is at most twenty notes). Each line is rebuilt from validated tokens or
//!   passed through [`util::sanitize_line`]; no byte from the peer reaches a terminal or
//!   the log raw.
//!
//! The protocol (version 1), one message per line, fields separated by single spaces:
//!
//! ```text
//! hello <protocol> <mod version> <feature>[,<feature>…]     ("-" for no feature)
//! diag <feature> <free text>
//! ```
//!
//! **Protocol 2** (the phone; docs/notes/round4-contract.md): a connection whose hello says
//! `2` — when this launch serves the phone — switches to frames after that line, in both
//! directions: `u32 length (big-endian; kind byte and payload) | u8 kind | payload`. From the
//! mod: COMMAND (0x01, a command line of at most [`MAX_COMMAND`] bytes, handed to the
//! [`Dispatcher`]) and DIAG (0x02, a protocol 1 `diag` line of at most [`MAX_DIAG`] bytes).
//! To the mod: STATE, FRAME and NOTICE ([`encode`]). A frame of another kind or over its
//! length ends the connection (said once); at most [`PHONE_MESSAGES_PER_SECOND`] frames and
//! [`PHONE_BYTES_PER_SECOND`] are read per second from all connections together (frames over
//! the rate are counted and dropped; reading pauses for bytes). The newest connection that
//! says hello 2 is the phone: what the services send goes there, and when it cannot keep up,
//! a newer state of an app or picture of a slot replaces the one still waiting. A hello of
//! protocol 1 is served exactly as before.
//!
//! Not guaranteed: that a hello is followed by anything, that diagnostics are true, or
//! that silence means the mod is absent — it works without the channel.

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    error::{Context, Error, Result},
    paths::APP,
    phone::{Outbound, dispatch::Dispatcher},
    util,
};

/// The protocol of a hello that only reports (served by every launch).
pub const PROTOCOL: u32 = 1;
/// The protocol of the phone (framed, both directions).
pub const PHONE_PROTOCOL: u32 = 2;
/// Frame kinds (protocol 2).
pub const KIND_COMMAND: u8 = 0x01;
pub const KIND_DIAG: u8 = 0x02;
pub const KIND_STATE: u8 = 0x81;
pub const KIND_FRAME: u8 = 0x82;
pub const KIND_NOTICE: u8 = 0x83;
/// Longest payloads the mod may send, per kind.
pub const MAX_COMMAND: usize = crate::phone::MAX_COMMAND_BYTES;
pub const MAX_DIAG: usize = 512;
/// What all connections together may send in protocol 2, per second.
pub const PHONE_MESSAGES_PER_SECOND: u32 = 50;
pub const PHONE_BYTES_PER_SECOND: usize = 200 * 1024;
/// Notices waiting for a phone that does not keep up; older ones are dropped.
const MAX_WAITING_NOTICES: usize = 32;
/// Longest line, in bytes, without its `\n` (a `\r` before it counts towards the limit).
pub const MAX_LINE: usize = 512;
/// Connections open at the same time.
pub const MAX_CONNECTIONS: usize = 4;
/// Connections accepted over a whole session.
pub const MAX_ACCEPTED: u64 = 64;
/// Lines that may be logged in one go, and per second after that.
pub const BURST: u32 = 20;
pub const PER_SECOND: u32 = 2;
/// Lines logged over a whole session.
pub const MAX_LOGGED: u64 = 2000;
/// How fast one connection is read.
pub const BYTES_PER_SECOND: usize = 64 * 1024;
/// Free text of a diagnostic, in characters.
const MAX_TEXT: usize = 200;
/// Lines that are not part of the protocol are quoted (sanitised) this many times; after
/// that they are only counted.
const QUOTED_STRANGERS: u64 = 3;
/// `sun_path` holds 108 bytes including the terminating NUL.
const MAX_SOCKET_PATH: usize = 107;
/// The socket's private directory is `companion-` and twelve hexadecimal digits; the socket
/// in it is `channel`.
const DIR_PREFIX: &str = "companion-";
const SOCKET_FILE: &str = "channel";
/// Leftover directories examined per launch (see [`sweep`]), and how old a socket must be
/// before it can count as left over.
const MAX_SWEPT: usize = 64;
const LEFTOVER_AGE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------- messages

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello {
        protocol: u32,
        version: String,
        features: Vec<String>,
    },
    Diagnostic {
        feature: String,
        /// Sanitised and bounded.
        text: String,
    },
}

fn is_version(token: &str) -> bool {
    (1..=32).contains(&token.len())
        && token.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

fn is_feature(token: &str) -> bool {
    (1..=32).contains(&token.len())
        && token.as_bytes()[0].is_ascii_lowercase()
        && token.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

/// One line (without its newline) as a message of protocol version 1, or `None` if it is
/// not one. Strict: exact field count, single spaces, small alphabets.
pub fn parse_line(raw: &[u8]) -> Option<Message> {
    if raw.len() > MAX_LINE {
        return None;
    }
    let (verb, rest) = raw.split_at(raw.iter().position(|b| *b == b' ')?);
    let rest = &rest[1..];
    match verb {
        b"hello" => {
            let text = std::str::from_utf8(rest).ok()?;
            let mut fields = text.split(' ');
            let (protocol, version, features) = (fields.next()?, fields.next()?, fields.next()?);
            if fields.next().is_some() || !is_version(version) {
                return None;
            }
            if !(1..=4).contains(&protocol.len()) || !protocol.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let protocol: u32 = protocol.parse().ok()?;
            let features: Vec<String> = match features {
                "-" => Vec::new(),
                list => list.split(',').map(str::to_string).collect(),
            };
            (features.len() <= 16 && features.iter().all(|f| is_feature(f))).then_some(Message::Hello {
                protocol,
                version: version.to_string(),
                features,
            })
        }
        b"diag" => {
            let (feature, text) = rest.split_at(rest.iter().position(|b| *b == b' ')?);
            let feature = std::str::from_utf8(feature).ok().filter(|f| is_feature(f))?;
            let text = &text[1..];
            (!text.is_empty()).then(|| Message::Diagnostic {
                feature: feature.to_string(),
                text: util::sanitize_line(text, MAX_TEXT),
            })
        }
        _ => None,
    }
}

impl Message {
    /// The message as a sentence for the launch log. Built from validated tokens and
    /// sanitised text only.
    pub fn describe(&self) -> String {
        match self {
            Message::Hello { protocol, version, features } => format!(
                "hello: mod version {version}, protocol {protocol}{}, features: {}",
                if *protocol == PROTOCOL || *protocol == PHONE_PROTOCOL {
                    ""
                } else {
                    " (this launcher speaks 1 and 2)"
                },
                if features.is_empty() { "none".to_string() } else { features.join(", ") }
            ),
            Message::Diagnostic { feature, text } => format!("{feature}: {text}"),
        }
    }
}

// ---------------------------------------------------------------- one connection's bytes

/// What [`Lines::feed`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Line(&'a [u8]),
    /// A line exceeded [`MAX_LINE`]; its bytes were dropped as they came.
    TooLong,
}

/// Cuts a byte stream into lines while holding at most [`MAX_LINE`] bytes.
#[derive(Default)]
pub struct Lines {
    buffer: Vec<u8>,
    /// Inside a line that is already too long: drop bytes up to the next newline.
    skipping: bool,
}

impl Lines {
    /// Bytes currently held.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    pub fn feed(&mut self, data: &[u8], mut each: impl FnMut(Piece<'_>)) {
        for &byte in data {
            if byte == b'\n' {
                if self.skipping {
                    self.skipping = false;
                } else {
                    if self.buffer.last() == Some(&b'\r') {
                        self.buffer.pop();
                    }
                    if !self.buffer.is_empty() {
                        each(Piece::Line(&self.buffer));
                    }
                }
                self.buffer.clear();
            } else if self.skipping {
                // Dropped.
            } else if self.buffer.len() == MAX_LINE {
                self.buffer.clear();
                self.skipping = true;
                each(Piece::TooLong);
            } else {
                self.buffer.push(byte);
            }
        }
    }
}

// ---------------------------------------------------------------- protocol 2 frames

/// Cuts a protocol 2 byte stream (from the mod) into frames while holding at most one frame
/// of at most [`MAX_COMMAND`] bytes.
#[derive(Default)]
pub struct Frames {
    header: [u8; 5],
    have: usize,
    kind: u8,
    want: usize,
    payload: Vec<u8>,
}

impl Frames {
    /// Bytes currently held.
    pub fn buffered(&self) -> usize {
        self.have.min(5) + self.payload.len()
    }

    /// Feeds bytes; `each` gets every complete frame (kind, payload). The first frame that is
    /// not one the mod may send ends the stream: the reason, and nothing after it is read.
    pub fn feed(&mut self, data: &[u8], mut each: impl FnMut(u8, &[u8])) -> std::result::Result<(), String> {
        let mut at = 0;
        while at < data.len() {
            if self.have < 5 {
                let n = (5 - self.have).min(data.len() - at);
                self.header[self.have..self.have + n].copy_from_slice(&data[at..at + n]);
                self.have += n;
                at += n;
                if self.have < 5 {
                    break;
                }
                let length = u32::from_be_bytes([self.header[0], self.header[1], self.header[2], self.header[3]]);
                self.kind = self.header[4];
                let max = match self.kind {
                    KIND_COMMAND => MAX_COMMAND,
                    KIND_DIAG => MAX_DIAG,
                    kind => return Err(format!("a frame of a kind the mod does not send ({kind:#04x})")),
                };
                if length == 0 || (length - 1) as usize > max {
                    return Err(format!("a frame of kind {:#04x} announcing {length} bytes", self.kind));
                }
                self.want = (length - 1) as usize;
                self.payload.clear();
            } else {
                let n = (self.want - self.payload.len()).min(data.len() - at);
                self.payload.extend_from_slice(&data[at..at + n]);
                at += n;
            }
            if self.have == 5 && self.payload.len() == self.want {
                each(self.kind, &self.payload);
                self.have = 0;
                self.payload.clear();
            }
        }
        Ok(())
    }
}

/// A message for the phone as it goes on the wire.
pub fn encode(message: &Outbound) -> Vec<u8> {
    let (kind, app, body): (u8, &str, Vec<u8>) = match message {
        Outbound::State { app, json } => (KIND_STATE, app, json.as_bytes().to_vec()),
        Outbound::Frame { app, slot, width, height, format, bytes } => {
            let mut body = Vec::with_capacity(6 + bytes.len());
            body.push(*slot);
            body.extend_from_slice(&width.to_be_bytes());
            body.extend_from_slice(&height.to_be_bytes());
            body.push(*format as u8);
            body.extend_from_slice(bytes);
            (KIND_FRAME, app, body)
        }
        Outbound::Notice { app, text } => (KIND_NOTICE, app, text.as_bytes().to_vec()),
    };
    let length = 1 + app.len() + 1 + body.len();
    let mut out = Vec::with_capacity(4 + length);
    out.extend_from_slice(&(length as u32).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(app.as_bytes());
    out.push(0);
    out.extend_from_slice(&body);
    out
}

/// What waits for a phone that reads slowly: a newer state of an app, or picture of an
/// app's slot, takes the place of the one still waiting; notices queue (the oldest go
/// beyond [`MAX_WAITING_NOTICES`]).
#[derive(Default)]
pub struct Waiting {
    queue: std::collections::VecDeque<Outbound>,
}

impl Waiting {
    pub fn push(&mut self, message: Outbound) {
        match &message {
            Outbound::State { app, .. } => {
                self.queue.retain(|w| !matches!(w, Outbound::State { app: a, .. } if a == app));
            }
            Outbound::Frame { app, slot, .. } => {
                self.queue.retain(|w| !matches!(w, Outbound::Frame { app: a, slot: s, .. } if a == app && s == slot));
            }
            Outbound::Notice { .. } => {
                let notices = self.queue.iter().filter(|w| matches!(w, Outbound::Notice { .. })).count();
                if notices >= MAX_WAITING_NOTICES
                    && let Some(oldest) = self.queue.iter().position(|w| matches!(w, Outbound::Notice { .. }))
                {
                    self.queue.remove(oldest);
                }
            }
        }
        self.queue.push_back(message);
    }

    pub fn pop(&mut self) -> Option<Outbound> {
        self.queue.pop_front()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

// ---------------------------------------------------------------- the session

/// Whose statement a log line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    /// The launcher's own: counts and limits.
    Launcher,
    /// What something inside the game said on the channel. To be shown like game output.
    Claimed,
}

pub type Logger = Arc<dyn Fn(Voice, String) + Send + Sync>;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub accepted: u64,
    pub refused: u64,
    pub hellos: u64,
    pub diagnostics: u64,
    pub strangers: u64,
    pub too_long: u64,
    /// Lines that were understood or quotable but not written: over the rate or the total.
    pub dropped: u64,
    /// Protocol 2: frames read, commands among them, frames over the global rate, and
    /// connections ended for breaking the framing.
    pub frames: u64,
    pub commands: u64,
    pub over_rate: u64,
    pub broken: u64,
}

/// Everything the launcher keeps about the channel, for all connections together. Time is
/// passed in (as time since the session began) so that the limits can be tested.
pub struct Session {
    logger: Logger,
    stats: Stats,
    open: usize,
    /// Token bucket in thousandths of a line.
    tokens: u64,
    refilled: Duration,
    logged: u64,
    /// The number of dropped lines at which the launcher next says that it is dropping.
    next_drop_note: u64,
    /// The phone's dispatcher, when this launch serves protocol 2.
    phone: Option<Dispatcher>,
    /// Protocol 2 frames from all connections: a token bucket in thousandths of a frame.
    frame_tokens: u64,
    frame_refilled: Duration,
    /// Bytes from all connections: a token bucket in bytes.
    byte_tokens: u64,
    byte_refilled: Duration,
    /// Said once each: a frame over the rate, a connection that broke the framing.
    noted_rate: bool,
    noted_broken: bool,
}

/// One accepted connection.
pub struct Connection {
    number: u64,
    lines: Lines,
    mode: Mode,
}

/// What a connection speaks.
enum Mode {
    /// Lines (protocol 1, and every connection until its hello).
    Text,
    /// Frames, after a hello of protocol 2.
    Phone(Frames),
    /// It broke the framing: nothing more is read.
    Broken,
}

impl Connection {
    pub fn buffered(&self) -> usize {
        match &self.mode {
            Mode::Phone(frames) => self.lines.buffered() + frames.buffered(),
            _ => self.lines.buffered(),
        }
    }

    /// Whether it speaks protocol 2 (it is, or was, the phone).
    pub fn is_phone(&self) -> bool {
        matches!(self.mode, Mode::Phone(_))
    }

    /// Whether it broke the framing (the reader closes it).
    pub fn is_broken(&self) -> bool {
        matches!(self.mode, Mode::Broken)
    }
}

impl Session {
    pub fn new(logger: Logger) -> Self {
        Session {
            logger,
            stats: Stats::default(),
            open: 0,
            tokens: u64::from(BURST) * 1000,
            refilled: Duration::ZERO,
            logged: 0,
            next_drop_note: 1,
            phone: None,
            frame_tokens: u64::from(PHONE_MESSAGES_PER_SECOND) * 1000,
            frame_refilled: Duration::ZERO,
            byte_tokens: PHONE_BYTES_PER_SECOND as u64,
            byte_refilled: Duration::ZERO,
            noted_rate: false,
            noted_broken: false,
        }
    }

    /// A session that serves the phone (protocol 2) through `dispatcher`.
    pub fn with_phone(logger: Logger, dispatcher: Dispatcher) -> Self {
        Session { phone: Some(dispatcher), ..Session::new(logger) }
    }

    /// The phone's dispatcher, if this session serves it.
    pub fn phone(&self) -> Option<&Dispatcher> {
        self.phone.as_ref()
    }

    /// A phone connection is ready to be written to: greet it (its state and every app's).
    pub fn greet_phone(&mut self) {
        if let Some(dispatcher) = self.phone.as_mut() {
            dispatcher.hello();
        }
    }

    /// `bytes` were read at `now`, from any connection: how long to pause so that all of
    /// them together stay within [`PHONE_BYTES_PER_SECOND`].
    pub fn charge(&mut self, bytes: usize, now: Duration) -> Duration {
        let capacity = PHONE_BYTES_PER_SECOND as u64;
        let elapsed = now.saturating_sub(self.byte_refilled).as_millis().min(u128::from(u32::MAX)) as u64;
        self.byte_refilled = self.byte_refilled.max(now);
        self.byte_tokens = (self.byte_tokens + elapsed * capacity / 1000).min(capacity);
        let bytes = bytes as u64;
        if self.byte_tokens >= bytes {
            self.byte_tokens -= bytes;
            Duration::ZERO
        } else {
            let missing = bytes - self.byte_tokens;
            self.byte_tokens = 0;
            Duration::from_millis(missing * 1000 / capacity + 1)
        }
    }

    /// Whether one more protocol 2 frame may be acted on at `now`.
    fn frame_allowed(&mut self, now: Duration) -> bool {
        let capacity = u64::from(PHONE_MESSAGES_PER_SECOND) * 1000;
        let elapsed = now.saturating_sub(self.frame_refilled).as_millis().min(u128::from(u32::MAX)) as u64;
        self.frame_refilled = self.frame_refilled.max(now);
        self.frame_tokens = (self.frame_tokens + elapsed * u64::from(PHONE_MESSAGES_PER_SECOND)).min(capacity);
        if self.frame_tokens >= 1000 {
            self.frame_tokens -= 1000;
            true
        } else {
            false
        }
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// A peer connected. `None`: over a limit; the caller closes it unread.
    pub fn open(&mut self) -> Option<Connection> {
        if self.open >= MAX_CONNECTIONS || self.stats.accepted >= MAX_ACCEPTED {
            self.stats.refused += 1;
            return None;
        }
        self.open += 1;
        self.stats.accepted += 1;
        Some(Connection { number: self.stats.accepted, lines: Lines::default(), mode: Mode::Text })
    }

    pub fn close(&mut self, connection: Connection) {
        drop(connection);
        self.open = self.open.saturating_sub(1);
    }

    /// Whether one more line may be written at `now`. A refusal is counted, and at the 1st,
    /// 10th, 100th, … the launcher says that it is dropping lines — in its own voice, and
    /// outside the limit it reports on: there can be no more than twenty such notes.
    fn may_log(&mut self, now: Duration) -> bool {
        let elapsed = now.saturating_sub(self.refilled).as_millis().min(u128::from(u32::MAX)) as u64;
        self.refilled = self.refilled.max(now);
        self.tokens = (self.tokens + elapsed * u64::from(PER_SECOND)).min(u64::from(BURST) * 1000);
        if self.tokens >= 1000 && self.logged < MAX_LOGGED {
            self.tokens -= 1000;
            self.logged += 1;
            return true;
        }
        self.stats.dropped += 1;
        if self.stats.dropped == self.next_drop_note {
            self.next_drop_note = self.next_drop_note.saturating_mul(10);
            (self.logger)(
                Voice::Launcher,
                format!(
                    "companion channel: more lines than the log accepts ({BURST} at once, {PER_SECOND} per second, \
                     {MAX_LOGGED} in a session); {} dropped so far",
                    self.stats.dropped
                ),
            );
        }
        false
    }

    /// Bytes from `connection`, read at `now`. True when the connection has just become the
    /// phone (its hello said protocol 2 and this session serves the phone): its writer is due.
    pub fn received(&mut self, connection: &mut Connection, data: &[u8], now: Duration) -> bool {
        let mut became_phone = false;
        let mut rest = data;
        while !rest.is_empty() {
            match &mut connection.mode {
                Mode::Broken => return became_phone,
                Mode::Phone(_) => {
                    let Mode::Phone(mut frames) = std::mem::replace(&mut connection.mode, Mode::Broken) else {
                        unreachable!()
                    };
                    let number = connection.number;
                    let fed = frames.feed(rest, |kind, payload| self.frame(number, kind, payload, now));
                    match fed {
                        Ok(()) => connection.mode = Mode::Phone(frames),
                        Err(why) => self.broke(number, &why),
                    }
                    return became_phone;
                }
                Mode::Text => {
                    // Up to and including the next line end; the rest may be frames.
                    let end = rest.iter().position(|b| *b == b'\n').map_or(rest.len(), |at| at + 1);
                    let (chunk, after) = rest.split_at(end);
                    rest = after;
                    if self.lines_received(connection, chunk, now) {
                        connection.mode = Mode::Phone(Frames::default());
                        became_phone = true;
                    }
                }
            }
        }
        became_phone
    }

    /// A connection broke the framing: counted, said once, and it is read no more.
    fn broke(&mut self, number: u64, why: &str) {
        self.stats.broken += 1;
        if !self.noted_broken {
            self.noted_broken = true;
            (self.logger)(
                Voice::Launcher,
                format!(
                    "companion channel #{number}: closed — {} (further such connections are only counted)",
                    util::sanitize_line(why.as_bytes(), 120)
                ),
            );
        }
    }

    /// One protocol 2 frame from the mod.
    fn frame(&mut self, number: u64, kind: u8, payload: &[u8], now: Duration) {
        self.stats.frames += 1;
        if !self.frame_allowed(now) {
            self.stats.over_rate += 1;
            if !self.noted_rate {
                self.noted_rate = true;
                (self.logger)(
                    Voice::Launcher,
                    format!(
                        "companion channel #{number}: more than {PHONE_MESSAGES_PER_SECOND} messages a second; the \
                         excess is dropped (and only counted from now on)"
                    ),
                );
            }
            return;
        }
        match kind {
            KIND_COMMAND => {
                self.stats.commands += 1;
                if let Some(dispatcher) = self.phone.as_mut() {
                    let _ = dispatcher.command(payload, now);
                }
            }
            _ => self.line(number, payload, now),
        }
    }

    /// Lines of the text protocol: true when one of them was a hello of protocol 2 that
    /// switches this connection to frames (only where the phone is served).
    fn lines_received(&mut self, connection: &mut Connection, data: &[u8], now: Duration) -> bool {
        let number = connection.number;
        let mut lines = std::mem::take(&mut connection.lines);
        let mut switch = false;
        let phone = self.phone.is_some();
        lines.feed(data, |piece| match piece {
            Piece::TooLong => {
                self.stats.too_long += 1;
                if self.stats.too_long == 1 && self.may_log(now) {
                    (self.logger)(
                        Voice::Launcher,
                        format!(
                            "companion channel #{number}: a line longer than {MAX_LINE} bytes was discarded (further \
                             ones are only counted)"
                        ),
                    );
                }
            }
            Piece::Line(line) => {
                if let Some(Message::Hello { protocol: PHONE_PROTOCOL, .. }) = parse_line(line) {
                    switch = phone;
                }
                self.line(number, line, now);
            }
        });
        connection.lines = lines;
        switch
    }

    /// One message of the text protocol (or a DIAG frame's line): parsed, counted, logged.
    fn line(&mut self, number: u64, line: &[u8], now: Duration) {
        match parse_line(line) {
            Some(message) => {
                match message {
                    Message::Hello { .. } => self.stats.hellos += 1,
                    Message::Diagnostic { .. } => self.stats.diagnostics += 1,
                }
                if self.may_log(now) {
                    (self.logger)(Voice::Claimed, format!("companion channel #{number}: {}", message.describe()));
                }
            }
            None => {
                self.stats.strangers += 1;
                if self.stats.strangers <= QUOTED_STRANGERS && self.may_log(now) {
                    (self.logger)(
                        Voice::Claimed,
                        format!(
                            "companion channel #{number}: not part of the protocol, ignored: {:?}",
                            util::sanitize_line(line, 80)
                        ),
                    );
                }
            }
        }
    }

    /// One line for the end of the launch log.
    pub fn summary(&self) -> String {
        let s = &self.stats;
        let mut text = format!(
            "companion channel: {} connection(s), {} hello, {} diagnostic(s)",
            s.accepted, s.hellos, s.diagnostics
        );
        for (count, what) in [
            (s.commands, "phone command(s)"),
            (s.strangers, "line(s) that are not part of the protocol"),
            (s.too_long, "over-long line(s)"),
            (s.dropped, "line(s) not logged (rate limit)"),
            (s.refused, "connection(s) refused (too many)"),
            (s.over_rate, "frame(s) over the rate"),
            (s.broken, "connection(s) closed for breaking the framing"),
        ] {
            if count > 0 {
                text.push_str(&format!(", {count} {what}"));
            }
        }
        text
    }
}

// ---------------------------------------------------------------- the socket

/// The listening socket, from its creation until the sandbox has it.
pub struct Endpoint {
    listener: UnixListener,
    path: PathBuf,
}

/// Remove what earlier launches could not: a launcher that never learnt that its sandbox
/// held the socket keeps the name (see [`Endpoint::unlink`]), then confines itself, and from
/// then on can remove nothing outside the instance's persist/ and the cache — the name
/// outlives it. Only a launch that adds the companion runs this sweep.
///
/// A socket nobody listens on refuses connections; that is how a leftover is told from one
/// in use (whose launcher sees one connection that says nothing). A socket younger than
/// [`LEFTOVER_AGE`] is never a leftover: another launch may have bound it an instant ago
/// and not be listening yet. Only what this module itself creates is touched: a real
/// directory with this module's kind of name that holds a socket called `channel` and
/// nothing else. Links are never followed, and the connection attempt cannot block. Best
/// effort: whatever fails is left where it is.
fn sweep(base: &Path) {
    use std::os::{fd::AsRawFd, unix::fs::FileTypeExt};

    use nix::sys::socket::{AddressFamily, SockFlag, SockType, UnixAddr, connect, socket};

    let Ok(entries) = std::fs::read_dir(base) else { return };
    for entry in entries.flatten().take(MAX_SWEPT) {
        let ours = entry.file_name().to_str().and_then(|name| name.strip_prefix(DIR_PREFIX)).is_some_and(|rest| {
            rest.len() == 12 && rest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        let (dir, path) = (entry.path(), entry.path().join(SOCKET_FILE));
        let is_dir = std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_dir());
        let old_socket = std::fs::symlink_metadata(&path).is_ok_and(|meta| {
            meta.file_type().is_socket()
                && meta.modified().ok().and_then(|bound| bound.elapsed().ok()).is_some_and(|age| age >= LEFTOVER_AGE)
        });
        if !ours || !is_dir || !old_socket {
            continue;
        }
        let refused = UnixAddr::new(&path).is_ok_and(|address| {
            socket(AddressFamily::Unix, SockType::Stream, SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC, None)
                .is_ok_and(|probe| connect(probe.as_raw_fd(), &address) == Err(nix::errno::Errno::ECONNREFUSED))
        });
        if refused && std::fs::remove_file(&path).is_ok() {
            // Fails, and leaves the directory, if anything else is in it.
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

impl Endpoint {
    /// Bind and listen in a new private directory: under the session's runtime directory
    /// if there is one (short paths; a socket's path is limited to 107 bytes), else under
    /// the launcher's cache.
    pub fn create(runtime: Option<&Path>, cache: &Path) -> Result<Endpoint> {
        let base = runtime.map(|dir| dir.join(APP)).unwrap_or_else(|| cache.to_path_buf());
        sweep(&base);
        let dir = base.join(format!("{DIR_PREFIX}{}", util::hex_encode(&util::random_bytes::<6>()?)));
        let path = dir.join(SOCKET_FILE);
        if path.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(Error::Sandbox(format!(
                "{} is too long for a socket (more than {MAX_SOCKET_PATH} bytes)",
                path.display()
            )));
        }
        util::create_private_dir_all(&dir)?;
        let listener = UnixListener::bind(&path).ctx(|| format!("creating {}", path.display())).inspect_err(|_| {
            let _ = std::fs::remove_dir(&dir);
        })?;
        let endpoint = Endpoint { listener, path };
        endpoint.listener.set_nonblocking(true).ctx(|| "preparing the companion socket")?;
        Ok(endpoint)
    }

    /// The socket's path on the host, to bind into the sandbox.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the socket's name. Once the sandbox holds its bind mount nothing needs the
    /// path, and no process outside the sandbox should find a way to connect.
    ///
    /// Not guaranteed to succeed later on: a launcher that has confined itself can no longer
    /// remove anything in the runtime directory. A name that is left behind that way is
    /// removed by the next launch ([`sweep`]).
    pub fn unlink(&self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }

    /// Start reading (on the async runtime). Needs only the descriptors that already exist,
    /// so it works after the launcher has confined itself; the name should be gone by then
    /// ([`Endpoint::unlink`], which does need an unconfined launcher).
    pub fn serve(self, logger: Logger) -> Result<Serving> {
        self.serve_phone(logger, None)
    }

    /// As [`Endpoint::serve`], and with `phone` the phone (protocol 2) as well: its commands
    /// go to the dispatcher, and what its outbox's receiving end yields goes to the newest
    /// connection that said hello 2.
    pub fn serve_phone(self, logger: Logger, phone: Option<Phone>) -> Result<Serving> {
        let (session, writer) = match phone {
            Some(Phone { dispatcher, outbound }) => {
                let session = Arc::new(Mutex::new(Session::with_phone(logger, dispatcher)));
                let (sockets, takeovers) = tokio::sync::mpsc::channel(4);
                let task = tokio::spawn(write_phone(outbound, takeovers, session.clone()));
                (session, Some((sockets, task)))
            }
            None => (Arc::new(Mutex::new(Session::new(logger))), None),
        };
        let listener = self.listener.try_clone().ctx(|| "duplicating the companion socket")?;
        let listener = tokio::net::UnixListener::from_std(listener).ctx(|| "registering the companion socket")?;
        let sockets = writer.as_ref().map(|(sockets, _)| sockets.clone());
        let task = tokio::spawn(accept_loop(listener, session.clone(), sockets));
        Ok(Serving { session, task, writer: writer.map(|(_, task)| task), _endpoint: self })
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.unlink();
    }
}

/// What the phone needs to be served: its dispatcher, and the receiving end of the outbox
/// its apps send with.
pub struct Phone {
    pub dispatcher: Dispatcher,
    pub outbound: tokio::sync::mpsc::Receiver<Outbound>,
}

/// The channel while the game runs.
pub struct Serving {
    session: Arc<Mutex<Session>>,
    task: tokio::task::JoinHandle<()>,
    writer: Option<tokio::task::JoinHandle<()>>,
    _endpoint: Endpoint,
}

impl Serving {
    pub fn stats(&self) -> Stats {
        locked(&self.session).stats.clone()
    }

    /// The phone's summary line, if the phone is served.
    pub fn phone_summary(&self) -> Option<String> {
        locked(&self.session).phone().map(Dispatcher::summary)
    }

    /// Stop reading; the summary line for the log.
    pub fn finish(self) -> String {
        self.task.abort();
        if let Some(writer) = &self.writer {
            writer.abort();
        }
        locked(&self.session).summary()
    }
}

/// Writes what the phone's apps send to the newest phone connection. Bytes go out a write
/// at a time, and between two writes everything that has arrived meanwhile is taken in, so
/// that a phone that reads slowly gets the newest state and picture of each app rather than
/// every one in between ([`Waiting`]). A new phone connection takes over: what was waiting
/// is dropped, and the dispatcher greets the newcomer (which sends every state again).
async fn write_phone(
    mut outbound: tokio::sync::mpsc::Receiver<Outbound>,
    mut takeovers: tokio::sync::mpsc::Receiver<tokio::net::unix::OwnedWriteHalf>,
    session: Arc<Mutex<Session>>,
) {
    let mut socket: Option<tokio::net::unix::OwnedWriteHalf> = None;
    let mut waiting = Waiting::default();
    let mut current: Vec<u8> = Vec::new();
    let mut written = 0usize;
    loop {
        while let Ok(message) = outbound.try_recv() {
            waiting.push(message);
        }
        if socket.is_some() && written == current.len() {
            match waiting.pop() {
                Some(next) => {
                    current = encode(&next);
                    written = 0;
                }
                None => {
                    current.clear();
                    written = 0;
                }
            }
        }
        let busy = socket.is_some() && written < current.len();
        tokio::select! {
            result = async {
                let socket = socket.as_mut().expect("busy means a socket");
                socket.write(&current[written..]).await
            }, if busy => {
                match result {
                    Ok(0) | Err(_) => {
                        socket = None;
                        current.clear();
                        written = 0;
                        waiting.clear();
                    }
                    Ok(n) => written += n,
                }
            }
            message = outbound.recv() => match message {
                Some(message) => waiting.push(message),
                None => return,
            },
            takeover = takeovers.recv() => match takeover {
                Some(next) => {
                    socket = Some(next);
                    current.clear();
                    written = 0;
                    waiting.clear();
                    locked(&session).greet_phone();
                }
                None => return,
            },
        }
    }
}

/// The session, whatever happened while it was last locked. The log sink is called with the
/// lock held, and a sink can panic: on a terminal the launch log's lines are printed with
/// `println!`, which panics once standard output has gone (a closed pipe). Where panics
/// unwind, that ends the reader that was logging (its connection's slot stays taken) and
/// marks the lock as poisoned. What the lock protects is counters and a token bucket, each
/// updated before the sink is called and so valid after any interruption — and the launch
/// reads them at its very end, before it carries settings back and removes the game's
/// scratch directory. A convenience must not be what stops that.
///
/// Only builds that unwind are helped by this (debug builds, the tests): the release profile
/// aborts on any panic (`panic = "abort"` in Cargo.toml), and there a sink that panics ends
/// the launcher whatever this module does.
fn locked(session: &Mutex<Session>) -> MutexGuard<'_, Session> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn accept_loop(
    listener: tokio::net::UnixListener,
    session: Arc<Mutex<Session>>,
    phone: Option<tokio::sync::mpsc::Sender<tokio::net::unix::OwnedWriteHalf>>,
) {
    let start = Instant::now();
    // Owned here: when this task ends, every connection's reader ends with it.
    let mut readers = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    // Out of descriptors, for instance: do not spin.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                };
                let admitted = locked(&session).open();
                match admitted {
                    Some(connection) => {
                        readers.spawn(read_connection(stream, connection, session.clone(), start, phone.clone()));
                    }
                    // Closed unread; a peer that keeps trying is slowed to a crawl.
                    None => {
                        drop(stream);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
            Some(_) = readers.join_next() => {}
        }
    }
}

async fn read_connection(
    stream: tokio::net::UnixStream,
    mut connection: Connection,
    session: Arc<Mutex<Session>>,
    start: Instant,
    phone: Option<tokio::sync::mpsc::Sender<tokio::net::unix::OwnedWriteHalf>>,
) {
    let (mut reader, writer) = stream.into_split();
    // Kept until the connection becomes the phone, then handed to the phone's writer.
    let mut writer = Some(writer);
    let mut buffer = [0u8; 4096];
    let (mut window, mut in_window) = (Instant::now(), 0usize);
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let (became_phone, broken, pause) = {
                    let mut session = locked(&session);
                    let became = session.received(&mut connection, &buffer[..n], start.elapsed());
                    let pause = session.charge(n, start.elapsed());
                    (became, connection.is_broken(), pause)
                };
                if broken {
                    break;
                }
                if became_phone && let (Some(phone), Some(half)) = (&phone, writer.take()) {
                    let _ = phone.send(half).await;
                }
                in_window += n;
                if in_window >= BYTES_PER_SECOND {
                    tokio::time::sleep_until((window + Duration::from_secs(1)).into()).await;
                } else if pause > Duration::ZERO {
                    tokio::time::sleep(pause).await;
                }
                if window.elapsed() >= Duration::from_secs(1) {
                    (window, in_window) = (Instant::now(), 0);
                }
            }
        }
    }
    locked(&session).close(connection);
}

/// For `fuzzing`: arbitrary bytes as one connection's input, whole and in pieces.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    fn run(data: &[u8], cuts: impl Fn(usize) -> usize) -> (Vec<(Voice, String)>, Stats) {
        let said = Arc::new(Mutex::new(Vec::new()));
        let sink = said.clone();
        let mut session = Session::new(Arc::new(move |voice, line| sink.lock().unwrap().push((voice, line))));
        let mut connection = session.open().expect("the first connection is admitted");
        let (mut at, mut round) = (0, 0);
        while at < data.len() {
            let end = at.saturating_add(cuts(round).max(1)).min(data.len());
            // All at the same instant: the outcome must not depend on how the bytes were cut.
            session.received(&mut connection, &data[at..end], Duration::from_secs(1));
            assert!(connection.buffered() <= MAX_LINE, "{} bytes held", connection.buffered());
            (at, round) = (end, round + 1);
        }
        let stats = session.stats().clone();
        session.close(connection);
        let said = said.lock().unwrap().clone();
        (said, stats)
    }
    if let Some(message) = parse_line(data) {
        let text = message.describe();
        assert!(text.chars().count() <= MAX_LINE + 64 && !text.chars().any(char::is_control), "{text:?}");
        if let Message::Hello { version, features, .. } = &message {
            assert!(is_version(version) && features.iter().all(|f| is_feature(f)) && features.len() <= 16);
        }
    }
    let (whole, stats) = run(data, |_| usize::MAX);
    assert!(whole.len() <= BURST as usize + 20, "{} lines logged at one instant", whole.len());
    for (_, line) in &whole {
        assert!(line.chars().count() <= 400 && !line.chars().any(char::is_control), "{line:?}");
    }
    // Byte by byte, and in pieces whose sizes depend on the input.
    let seed = data.iter().fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(usize::from(*b)));
    let byte_by_byte = run(data, |_| 1);
    let uneven = run(data, |round| 1 + (seed.wrapping_mul(round + 3) >> 3) % 97);
    for (pieces, piecewise) in [byte_by_byte, uneven] {
        assert_eq!(pieces, whole, "the outcome depends on how the bytes arrived");
        assert_eq!(piecewise, stats);
    }
}

/// A frame as the mod sends it (for tests and the fuzz harness's seeds).
#[doc(hidden)]
pub fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = ((1 + payload.len()) as u32).to_be_bytes().to_vec();
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

/// An app for the harness: two verbs, records what reaches it.
struct Probe(Arc<Mutex<Vec<String>>>);

const PROBE_LIMITS: &[crate::phone::VerbLimit] = &[
    crate::phone::VerbLimit { verb: "play", per_minute: 60, burst: 3 },
    crate::phone::VerbLimit { verb: "search", per_minute: 60, burst: 3 },
];

impl crate::phone::PhoneApp for Probe {
    fn name(&self) -> &'static str {
        "spotify"
    }
    fn limits(&self) -> &'static [crate::phone::VerbLimit] {
        PROBE_LIMITS
    }
    fn hello(&mut self, out: &crate::phone::Outbox) {
        out.state("spotify", &serde_json::json!({"hello": true}));
    }
    fn command(&mut self, command: crate::phone::Command, out: &crate::phone::Outbox) {
        self.0.lock().unwrap().push(format!("{} {:?} {:?}", command.verb, command.args, command.text));
        out.notice("spotify", "ok");
    }
}

/// For `fuzzing`: arbitrary bytes after a hello of protocol 2, as one phone connection's
/// input — whole, byte by byte, and in uneven pieces. Whatever the bytes, the outcome
/// (what is logged, what reaches the app, the counts) does not depend on how they were cut,
/// at most one bounded frame is held, nothing logged contains a control character, and
/// what reaches the app is a command within the grammar, the vocabulary and the limits.
#[doc(hidden)]
pub fn fuzz_phone(data: &[u8]) {
    type Outcome = (Vec<(Voice, String)>, Vec<String>, Stats, Vec<String>);
    fn run(data: &[u8], cuts: impl Fn(usize) -> usize) -> Outcome {
        let said = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let notes = Arc::new(Mutex::new(Vec::new()));
        let (sink, note_sink) = (said.clone(), notes.clone());
        let (out, mut outbound) = crate::phone::Outbox::new(1024);
        let dispatcher = Dispatcher::new(
            vec![Box::new(Probe(seen.clone()))],
            out,
            Arc::new(move |line| note_sink.lock().unwrap().push(line)),
        );
        let mut session =
            Session::with_phone(Arc::new(move |voice, line| sink.lock().unwrap().push((voice, line))), dispatcher);
        let mut connection = session.open().expect("the first connection is admitted");
        let mut input = b"hello 2 0.1.0 phone\n".to_vec();
        input.extend_from_slice(data);
        let (mut at, mut round) = (0, 0);
        let mut became = 0;
        while at < input.len() {
            let end = at.saturating_add(cuts(round).max(1)).min(input.len());
            if session.received(&mut connection, &input[at..end], Duration::from_secs(1)) {
                became += 1;
            }
            assert!(connection.buffered() <= MAX_LINE + MAX_COMMAND + 5, "{} bytes held", connection.buffered());
            (at, round) = (end, round + 1);
        }
        assert_eq!(became, 1, "the hello switches the connection once");
        assert!(connection.is_phone() || connection.is_broken());
        let stats = session.stats().clone();
        session.close(connection);
        while outbound.try_recv().is_ok() {}
        let said = said.lock().unwrap().clone();
        let seen = seen.lock().unwrap().clone();
        let notes = notes.lock().unwrap().clone();
        (said, seen, stats, notes)
    }
    let whole = run(data, |_| usize::MAX);
    for (_, line) in &whole.0 {
        assert!(line.chars().count() <= 400 && !line.chars().any(char::is_control), "{line:?}");
    }
    for line in &whole.3 {
        assert!(!line.chars().any(char::is_control), "{line:?}");
    }
    assert!(whole.1.len() <= 6, "more commands reached the app than its buckets allow at one instant");
    let seed = data.iter().fold(11usize, |h, b| h.wrapping_mul(31).wrapping_add(usize::from(*b)));
    for pieces in [run(data, |_| 1), run(data, |round| 1 + (seed.wrapping_mul(round + 5) >> 3) % 61)] {
        assert_eq!(pieces, whole, "the outcome depends on how the bytes arrived");
    }
    // Everything the launcher would send back encodes to frames the mod's limits accept.
    let mut waiting = Waiting::default();
    waiting.push(Outbound::Notice { app: "spotify", text: String::from_utf8_lossy(data).chars().take(64).collect() });
    while let Some(message) = waiting.pop() {
        let bytes = encode(&message);
        let length = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        assert_eq!(length + 4, bytes.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a session under test has logged.
    type Said = Arc<Mutex<Vec<(Voice, String)>>>;

    fn collecting() -> (Session, Said) {
        let said = Arc::new(Mutex::new(Vec::new()));
        let sink = said.clone();
        (Session::new(Arc::new(move |voice, line| sink.lock().unwrap().push((voice, line)))), said)
    }

    fn hello(features: &[&str]) -> Message {
        Message::Hello {
            protocol: 1,
            version: "0.1.0".into(),
            features: features.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn the_two_messages_of_version_one_are_understood() {
        assert_eq!(parse_line(b"hello 1 0.1.0 zoom,hud"), Some(hello(&["zoom", "hud"])));
        assert_eq!(parse_line(b"hello 1 0.1.0 -"), Some(hello(&[])));
        assert_eq!(
            parse_line(b"diag zoom disabled: lookup failed (NoSuchMethodException: Camera.calculateFov)"),
            Some(Message::Diagnostic {
                feature: "zoom".into(),
                text: "disabled: lookup failed (NoSuchMethodException: Camera.calculateFov)".into()
            })
        );
        assert_eq!(hello(&["zoom", "hud"]).describe(), "hello: mod version 0.1.0, protocol 1, features: zoom, hud");
        assert_eq!(hello(&[]).describe(), "hello: mod version 0.1.0, protocol 1, features: none");
        let phone = parse_line(b"hello 2 9.9 zoom").unwrap();
        assert_eq!(phone.describe(), "hello: mod version 9.9, protocol 2, features: zoom");
        let newer = parse_line(b"hello 3 9.9 zoom").unwrap();
        assert!(newer.describe().contains("protocol 3 (this launcher speaks 1 and 2)"), "{}", newer.describe());
    }

    #[test]
    fn anything_else_is_not_a_message() {
        for bad in [
            &b""[..],
            b"hello",
            b"hello ",
            b"hello 1",
            b"hello 1 0.1.0",
            b"hello 1 0.1.0 ",
            b"hello 1 0.1.0 zoom extra",
            b"hello  1 0.1.0 zoom",
            b"hello one 0.1.0 zoom",
            b"hello 99999 0.1.0 zoom",
            b"hello -1 0.1.0 zoom",
            b"hello +1 0.1.0 zoom",
            b"hello 1 0.1.0/../x zoom",
            b"hello 1 0.1.0 Zoom",
            b"hello 1 0.1.0 zoom,",
            b"hello 1 0.1.0 ,zoom",
            b"hello 1 0.1.0 zoom,,hud",
            b"hello 1 0.1.0 9lives",
            b"hello 1 0.1.0 a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q",
            b"hello 1 0.1.0 zo\xc3\xb6m",
            b"hello 1 0.1.0 zoom\0",
            b"HELLO 1 0.1.0 zoom",
            b" hello 1 0.1.0 zoom",
            b"diag",
            b"diag zoom",
            b"diag zoom ",
            b"diag Zoom text",
            b"diag  text",
            b"approve server evil.example",
            b"\x1b[31mhello 1 0.1.0 zoom",
            b"{\"hello\":1}",
        ] {
            assert_eq!(parse_line(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
        assert_eq!(parse_line(&[b'x'; MAX_LINE + 1]), None);
        let long_version = format!("hello 1 {} zoom", "1".repeat(33));
        assert_eq!(parse_line(long_version.as_bytes()), None);
    }

    #[test]
    fn free_text_is_made_safe_to_print_and_bounded() {
        let Some(Message::Diagnostic { text, .. }) =
            parse_line(b"diag zoom \x1b]0;title\x07 \x1b[2J\rSECURITY \xe2\x80\xae reversed \xff\xfe \0 end")
        else {
            panic!("a diagnostic")
        };
        assert!(!text.chars().any(|c| c.is_control()), "{text:?}");
        assert!(!text.contains('\u{202e}'), "bidi override survived: {text:?}");
        assert!(text.contains("SECURITY") && text.ends_with("end"));
        let mut long = b"diag hud ".to_vec();
        long.extend(std::iter::repeat_n(b'A', 400));
        let Some(Message::Diagnostic { text, .. }) = parse_line(&long) else { panic!("a diagnostic") };
        assert!(text.chars().count() <= MAX_TEXT + 16, "{}", text.len());
    }

    #[test]
    fn lines_are_cut_without_holding_more_than_one() {
        let mut lines = Lines::default();
        let mut seen: Vec<String> = Vec::new();
        let mut feed = |lines: &mut Lines, data: &[u8]| {
            lines.feed(data, |piece| {
                seen.push(match piece {
                    Piece::Line(line) => String::from_utf8_lossy(line).into_owned(),
                    Piece::TooLong => "<too long>".into(),
                })
            })
        };
        feed(&mut lines, b"one\ntw");
        feed(&mut lines, b"o\r\n\n\r\nthree");
        assert_eq!(lines.buffered(), 5, "an unfinished line waits");
        feed(&mut lines, b"\n");
        // A megabyte without a newline: reported once, never held.
        for _ in 0..256 {
            feed(&mut lines, &[b'x'; 4096]);
            assert!(lines.buffered() <= MAX_LINE);
        }
        feed(&mut lines, b"tail of the long line\nfour\n");
        // Exactly the limit is still a line.
        let mut exact = vec![b'y'; MAX_LINE];
        exact.push(b'\n');
        feed(&mut lines, &exact);
        assert_eq!(seen[..5], ["one", "two", "three", "<too long>", "four"]);
        assert_eq!(seen[5].len(), MAX_LINE);
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn what_is_logged_says_who_is_speaking() {
        let (mut session, said) = collecting();
        let mut connection = session.open().unwrap();
        session.received(
            &mut connection,
            b"hello 1 0.1.0 zoom\ndiag zoom disabled: lookup failed\nGIVE ME THE TOKEN\n",
            Duration::ZERO,
        );
        let said = said.lock().unwrap().clone();
        assert_eq!(
            said,
            [
                (
                    Voice::Claimed,
                    "companion channel #1: hello: mod version 0.1.0, protocol 1, features: zoom".to_string()
                ),
                (Voice::Claimed, "companion channel #1: zoom: disabled: lookup failed".to_string()),
                (
                    Voice::Claimed,
                    "companion channel #1: not part of the protocol, ignored: \"GIVE ME THE TOKEN\"".to_string()
                ),
            ]
        );
        assert_eq!(
            session.stats(),
            &Stats { accepted: 1, hellos: 1, diagnostics: 1, strangers: 1, ..Default::default() }
        );
        assert_eq!(
            session.summary(),
            "companion channel: 1 connection(s), 1 hello, 1 diagnostic(s), 1 line(s) that are not part of the protocol"
        );
    }

    #[test]
    fn a_flood_is_counted_not_logged() {
        let (mut session, said) = collecting();
        let mut connection = session.open().unwrap();
        let flood: Vec<u8> = b"diag zoom again\n".repeat(10_000);
        session.received(&mut connection, &flood, Duration::from_millis(5));
        let claimed = |said: &Said| said.lock().unwrap().iter().filter(|(voice, _)| *voice == Voice::Claimed).count();
        assert_eq!(claimed(&said), BURST as usize, "the burst, and no more");
        assert_eq!(session.stats().dropped, 10_000 - u64::from(BURST));
        assert_eq!(session.stats().diagnostics, 10_000, "counted all the same");
        let notes = |said: &Said| -> Vec<String> {
            said.lock().unwrap().iter().filter(|(v, _)| *v == Voice::Launcher).map(|(_, l)| l.clone()).collect()
        };
        // The launcher says so at the 1st, 10th, 100th and 1000th dropped line.
        assert_eq!(notes(&said).len(), 4, "{:?}", notes(&said));
        assert!(notes(&said)[3].ends_with("1000 dropped so far"), "{}", notes(&said)[3]);

        // Four seconds later the allowance has grown by PER_SECOND lines a second.
        session.received(&mut connection, &flood, Duration::from_millis(5) + Duration::from_secs(4));
        assert_eq!(claimed(&said), BURST as usize + 4 * PER_SECOND as usize);
        // It never grows beyond the burst, however long the pause.
        session.received(&mut connection, &flood, Duration::from_secs(3600));
        assert_eq!(claimed(&said), 2 * BURST as usize + 4 * PER_SECOND as usize);
        assert_eq!(notes(&said).len(), 5, "one more, at the 10000th dropped line");
    }

    #[test]
    fn a_session_logs_a_bounded_number_of_lines_however_long_it_lasts() {
        let (mut session, said) = collecting();
        let mut connection = session.open().unwrap();
        // One line a second for two hours stays under the rate limit, but not under the total.
        for second in 0..7200u64 {
            session.received(&mut connection, b"diag hud tick\n", Duration::from_secs(second));
        }
        let claimed = said.lock().unwrap().iter().filter(|(voice, _)| *voice == Voice::Claimed).count();
        assert_eq!(claimed as u64, MAX_LOGGED);
        assert_eq!(session.stats().dropped, 7200 - MAX_LOGGED);
        let notes = said.lock().unwrap().len() - claimed;
        assert_eq!(notes, 4, "at the 1st, 10th, 100th and 1000th of the 5200 dropped lines");
    }

    #[test]
    fn connections_are_limited_at_once_and_in_total() {
        let (mut session, _) = collecting();
        let mut open: Vec<Connection> = (0..MAX_CONNECTIONS).map(|_| session.open().unwrap()).collect();
        assert!(session.open().is_none(), "one too many at once");
        session.close(open.pop().unwrap());
        open.push(session.open().expect("a slot was freed"));
        for connection in open {
            session.close(connection);
        }
        let mut admitted = MAX_CONNECTIONS as u64 + 1;
        while let Some(connection) = session.open() {
            session.close(connection);
            admitted += 1;
            assert!(admitted <= MAX_ACCEPTED, "more than the session's total were admitted");
        }
        assert_eq!(admitted, MAX_ACCEPTED);
        assert_eq!(session.stats().accepted, MAX_ACCEPTED);
        assert_eq!(session.stats().refused, 2);
        assert!(session.summary().contains("2 connection(s) refused"));
    }

    #[test]
    fn over_long_and_strange_lines_are_mentioned_a_few_times_then_only_counted() {
        let (mut session, said) = collecting();
        let mut connection = session.open().unwrap();
        let mut data = Vec::new();
        for _ in 0..5 {
            data.extend(std::iter::repeat_n(b'z', MAX_LINE * 3));
            data.push(b'\n');
            data.extend_from_slice(b"what is this\n");
        }
        session.received(&mut connection, &data, Duration::ZERO);
        let said = said.lock().unwrap().clone();
        assert_eq!(said.iter().filter(|(_, l)| l.contains("longer than 512 bytes")).count(), 1);
        assert_eq!(said.iter().filter(|(_, l)| l.contains("not part of the protocol")).count(), 3);
        assert_eq!((session.stats().too_long, session.stats().strangers), (5, 5));
        assert_eq!(connection.buffered(), 0);
    }

    #[test]
    fn the_fuzz_harness_accepts_ordinary_traffic() {
        fuzz(b"hello 1 0.1.0 zoom,hud\ndiag zoom disabled: lookup failed\n");
        fuzz(&b"diag zoom again\n".repeat(200));
        fuzz(&[b'q'; 5000]);
        fuzz(b"");
    }

    #[tokio::test]
    async fn a_real_socket_carries_lines_to_the_log_and_ends_with_a_summary() {
        use tokio::io::AsyncWriteExt;
        let root = std::env::temp_dir()
            .join(format!("bmc-ch-{}", crate::util::hex_encode(&crate::util::random_bytes::<4>().unwrap())));
        crate::util::create_private_dir_all(&root).unwrap();
        let endpoint = Endpoint::create(None, &root).unwrap();
        let path = endpoint.path().to_path_buf();
        assert_eq!(path.file_name().unwrap(), "channel");
        let said = Arc::new(Mutex::new(Vec::new()));
        let sink = said.clone();
        let serving = endpoint.serve(Arc::new(move |voice, line| sink.lock().unwrap().push((voice, line)))).unwrap();

        let mut peer = tokio::net::UnixStream::connect(&path).await.unwrap();
        peer.write_all(b"hello 1 0.1.0 zoom\ndiag zoom disab").await.unwrap();
        peer.write_all(b"led: lookup failed\n\x1b[2Jnot a message\n").await.unwrap();
        // More connections than are served at once: the extra ones are closed unread.
        let mut extra = Vec::new();
        for _ in 0..MAX_CONNECTIONS + 2 {
            extra.push(tokio::net::UnixStream::connect(&path).await.unwrap());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while (serving.stats().strangers < 1 || serving.stats().refused < 3) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let stats = serving.stats();
        assert_eq!((stats.hellos, stats.diagnostics, stats.strangers), (1, 1, 1), "{stats:?}");
        assert_eq!((stats.accepted, stats.refused), (MAX_CONNECTIONS as u64, 3), "{stats:?}");
        let summary = serving.finish();
        assert!(summary.starts_with("companion channel: 4 connection(s), 1 hello, 1 diagnostic(s)"), "{summary}");
        let said = said.lock().unwrap().clone();
        assert_eq!(
            said[0],
            (Voice::Claimed, "companion channel #1: hello: mod version 0.1.0, protocol 1, features: zoom".into())
        );
        assert_eq!(said[1], (Voice::Claimed, "companion channel #1: zoom: disabled: lookup failed".into()));
        assert!(said[2].1.contains("not part of the protocol") && !said[2].1.contains('\x1b'), "{:?}", said[2]);
        assert!(!path.exists(), "the socket's name is removed with the endpoint");
        assert!(!path.parent().unwrap().exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_logger_that_fails_does_not_take_the_end_of_the_launch_with_it() {
        use tokio::io::AsyncWriteExt;
        let root = std::env::temp_dir()
            .join(format!("bmc-lg-{}", crate::util::hex_encode(&crate::util::random_bytes::<4>().unwrap())));
        crate::util::create_private_dir_all(&root).unwrap();
        let endpoint = Endpoint::create(None, &root).unwrap();
        let path = endpoint.path().to_path_buf();
        // What a log sink does when its output is gone (a closed terminal, a front end that
        // has exited): it panics — here without the noise, and while the session is locked.
        let serving = endpoint.serve(Arc::new(|_, _| std::panic::resume_unwind(Box::new("the log is gone")))).unwrap();
        let mut peer = tokio::net::UnixStream::connect(&path).await.unwrap();
        peer.write_all(b"hello 1 0.1.0 zoom\n").await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        // The launch's last words about the channel are still to be had: its counts.
        while serving.stats().hellos < 1 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(serving.stats().hellos, 1);
        // And the channel goes on counting for whoever connects next.
        let mut second = tokio::net::UnixStream::connect(&path).await.unwrap();
        second.write_all(b"diag zoom still here\n").await.unwrap();
        while serving.stats().diagnostics < 1 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(serving.finish(), "companion channel: 2 connection(s), 1 hello, 1 diagnostic(s)");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn names_left_behind_by_dead_launchers_are_removed_and_nothing_else_is() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir()
            .join(format!("bmc-sw-{}", crate::util::hex_encode(&crate::util::random_bytes::<4>().unwrap())));
        crate::util::create_private_dir_all(&root).unwrap();
        let entry = |name: &str| {
            let dir = root.join(name);
            std::fs::create_dir(&dir).unwrap();
            dir
        };
        // A socket in `dir`, bound two minutes ago as far as its time stamp says.
        let bound_a_while_ago = |dir: &Path| {
            use nix::sys::{stat, time::TimeSpec};
            let listener = UnixListener::bind(dir.join("channel")).unwrap();
            let then = std::time::SystemTime::now() - Duration::from_secs(120);
            let then = TimeSpec::from_duration(then.duration_since(std::time::UNIX_EPOCH).unwrap());
            let flags = stat::UtimensatFlags::NoFollowSymlink;
            stat::utimensat(nix::fcntl::AT_FDCWD, &dir.join("channel"), &then, &then, flags).unwrap();
            listener
        };
        // A launcher that is gone: its socket's name is still there, nobody listens.
        let dead = entry("companion-0123456789ab");
        drop(bound_a_while_ago(&dead));
        // One that is running and kept its name.
        let live = entry("companion-ba9876543210");
        let listening = bound_a_while_ago(&live);
        // One that another launch bound this instant and is not listening on yet, as far as
        // anyone can tell from outside.
        let fresh = entry("companion-ffffffffffff");
        drop(UnixListener::bind(fresh.join("channel")).unwrap());
        // A dead socket with something else next to it: the socket goes, the rest stays.
        let mixed = entry("companion-aaaaaaaaaaaa");
        drop(bound_a_while_ago(&mixed));
        std::fs::write(mixed.join("note"), b"x").unwrap();
        // Things this module did not make, however similar.
        let other_name = entry("companion-0123456789AB");
        drop(bound_a_while_ago(&other_name));
        let not_a_socket = entry("companion-bbbbbbbbbbbb");
        std::fs::write(not_a_socket.join("channel"), b"a file").unwrap();
        let elsewhere = entry("elsewhere");
        drop(bound_a_while_ago(&elsewhere));
        symlink(&elsewhere, root.join("companion-cccccccccccc")).unwrap();
        let linked = entry("companion-dddddddddddd");
        symlink(elsewhere.join("channel"), linked.join("channel")).unwrap();
        let empty = entry("companion-eeeeeeeeeeee");

        // What every new endpoint does first.
        let endpoint = Endpoint::create(None, &root).unwrap();

        assert!(!dead.exists(), "the dead launcher's directory is gone");
        assert!(live.join("channel").exists(), "a socket somebody listens on is left alone");
        assert!(!mixed.join("channel").exists() && mixed.join("note").exists());
        for untouched in [
            fresh.join("channel"),
            other_name.join("channel"),
            not_a_socket.join("channel"),
            elsewhere.join("channel"),
            root.join("companion-cccccccccccc"),
            linked.join("channel"),
            empty,
        ] {
            assert!(std::fs::symlink_metadata(&untouched).is_ok(), "{} was removed", untouched.display());
        }
        assert!(endpoint.path().exists(), "the new endpoint itself is not swept");
        // The live one saw the probe as a connection that said nothing.
        listening.set_nonblocking(true).unwrap();
        assert!(listening.accept().is_ok());
        drop(endpoint);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn phone_session() -> (Session, Said, Arc<Mutex<Vec<String>>>, tokio::sync::mpsc::Receiver<Outbound>) {
        let said = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = said.clone();
        let (out, outbound) = crate::phone::Outbox::new(64);
        let dispatcher = Dispatcher::new(vec![Box::new(Probe(seen.clone()))], out, Arc::new(|_| {}));
        let session =
            Session::with_phone(Arc::new(move |voice, line| sink.lock().unwrap().push((voice, line))), dispatcher);
        (session, said, seen, outbound)
    }

    #[test]
    fn a_hello_of_protocol_two_switches_the_connection_to_frames_in_the_same_read() {
        let (mut session, said, seen, _) = phone_session();
        let mut connection = session.open().unwrap();
        let mut bytes = b"hello 2 0.1.0 phone,home\n".to_vec();
        bytes.extend(frame(KIND_COMMAND, b"spotify play"));
        bytes.extend(frame(KIND_DIAG, b"diag home disabled: no adapter"));
        bytes.extend(frame(KIND_COMMAND, b"spotify search -- Daft Punk"));
        assert!(session.received(&mut connection, &bytes, Duration::ZERO), "it became the phone");
        assert!(connection.is_phone());
        assert_eq!(*seen.lock().unwrap(), ["play [] None", "search [] Some(\"Daft Punk\")"]);
        let said = said.lock().unwrap().clone();
        assert_eq!(said[0].1, "companion channel #1: hello: mod version 0.1.0, protocol 2, features: phone, home");
        assert_eq!(said[1].1, "companion channel #1: home: disabled: no adapter", "a DIAG frame is a protocol 1 line");
        let stats = session.stats();
        assert_eq!((stats.hellos, stats.diagnostics, stats.frames, stats.commands), (1, 1, 3, 2));
        assert_eq!(
            session.summary(),
            "companion channel: 1 connection(s), 1 hello, 1 diagnostic(s), 2 phone command(s)"
        );
        // A protocol 1 hello on another connection stays a line protocol.
        let mut other = session.open().unwrap();
        assert!(!session.received(&mut other, b"hello 1 0.1.0 zoom\ndiag zoom ok\n", Duration::ZERO));
        assert!(!other.is_phone());
    }

    #[test]
    fn without_the_phone_a_hello_of_protocol_two_is_only_logged() {
        let (mut session, said) = collecting();
        let mut connection = session.open().unwrap();
        let mut bytes = b"hello 2 0.1.0 phone\n".to_vec();
        bytes.extend(frame(KIND_COMMAND, b"spotify play"));
        assert!(!session.received(&mut connection, &bytes, Duration::ZERO));
        assert!(!connection.is_phone());
        assert_eq!(session.stats().hellos, 1);
        assert!(said.lock().unwrap()[0].1.contains("protocol 2"));
    }

    #[test]
    fn a_frame_the_mod_may_not_send_ends_the_connection_and_is_said_once() {
        for bad in [
            frame(KIND_STATE, b"spotify\0{}"),
            frame(0x7f, b"x"),
            vec![0, 0, 0, 0, KIND_COMMAND],
            [((MAX_COMMAND + 2) as u32).to_be_bytes().to_vec(), vec![KIND_COMMAND]].concat(),
            [((MAX_DIAG + 2) as u32).to_be_bytes().to_vec(), vec![KIND_DIAG]].concat(),
        ] {
            let (mut session, said, seen, _) = phone_session();
            let mut connection = session.open().unwrap();
            let mut bytes = b"hello 2 0.1.0 -\n".to_vec();
            bytes.extend(frame(KIND_COMMAND, b"spotify play"));
            bytes.extend(&bad);
            bytes.extend(frame(KIND_COMMAND, b"spotify play"));
            session.received(&mut connection, &bytes, Duration::ZERO);
            assert!(connection.is_broken(), "{bad:?}");
            assert_eq!(seen.lock().unwrap().len(), 1, "nothing after the bad frame is read");
            // Nothing more is read from it, whatever comes.
            session.received(&mut connection, &frame(KIND_COMMAND, b"spotify play"), Duration::ZERO);
            assert_eq!(seen.lock().unwrap().len(), 1);
            let notes: Vec<String> =
                said.lock().unwrap().iter().filter(|(v, _)| *v == Voice::Launcher).map(|(_, l)| l.clone()).collect();
            assert_eq!(notes.len(), 1, "{notes:?}");
            assert!(notes[0].starts_with("companion channel #1: closed — "), "{notes:?}");
            assert_eq!(session.stats().broken, 1);
        }
    }

    #[test]
    fn frames_over_the_global_rate_are_dropped_and_counted() {
        let (mut session, said, _, _) = phone_session();
        let mut connection = session.open().unwrap();
        let mut bytes = b"hello 2 0.1.0 -\n".to_vec();
        for _ in 0..80 {
            bytes.extend(frame(KIND_DIAG, b"diag zoom again"));
        }
        session.received(&mut connection, &bytes, Duration::from_secs(1));
        let stats = session.stats().clone();
        assert_eq!(stats.frames, 80);
        assert_eq!(stats.over_rate, 80 - u64::from(PHONE_MESSAGES_PER_SECOND));
        assert_eq!(stats.diagnostics, u64::from(PHONE_MESSAGES_PER_SECOND));
        let notes = said.lock().unwrap().iter().filter(|(_, l)| l.contains("messages a second")).count();
        assert_eq!(notes, 1);
        // A second later there is room for more.
        session.received(&mut connection, &frame(KIND_DIAG, b"diag zoom later"), Duration::from_secs(2));
        assert_eq!(session.stats().diagnostics, u64::from(PHONE_MESSAGES_PER_SECOND) + 1);
        assert!(session.summary().contains("30 frame(s) over the rate"), "{}", session.summary());
    }

    #[test]
    fn the_byte_budget_of_all_connections_asks_for_a_pause_when_spent() {
        let (mut session, _) = collecting();
        assert_eq!(session.charge(PHONE_BYTES_PER_SECOND, Duration::ZERO), Duration::ZERO);
        let pause = session.charge(PHONE_BYTES_PER_SECOND / 2, Duration::ZERO);
        assert!(pause >= Duration::from_millis(400) && pause <= Duration::from_millis(600), "{pause:?}");
        assert_eq!(session.charge(1000, Duration::from_secs(5)), Duration::ZERO, "refilled");
    }

    #[test]
    fn frames_to_the_phone_are_encoded_as_the_contract_says_and_newer_ones_replace_waiting_ones() {
        let state = Outbound::State { app: "spotify", json: "{\"a\":1}".into() };
        assert_eq!(encode(&state), [&[0, 0, 0, 16, KIND_STATE][..], b"spotify\0{\"a\":1}"].concat());
        let picture = Outbound::Frame {
            app: "clips",
            slot: 3,
            width: 2,
            height: 1,
            format: crate::phone::FrameFormat::Rgba8,
            bytes: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        assert_eq!(
            encode(&picture),
            [&[0, 0, 0, 21, KIND_FRAME][..], b"clips\0", &[3, 0, 2, 0, 1, 3], &[1, 2, 3, 4, 5, 6, 7, 8]].concat()
        );
        let notice = Outbound::Notice { app: "realms", text: "approved".into() };
        assert_eq!(encode(&notice), [&[0, 0, 0, 16, KIND_NOTICE][..], b"realms\0approved"].concat());

        let mut waiting = Waiting::default();
        waiting.push(Outbound::State { app: "spotify", json: "{\"n\":1}".into() });
        waiting.push(picture.clone());
        waiting.push(Outbound::State { app: "browser", json: "{}".into() });
        waiting.push(Outbound::State { app: "spotify", json: "{\"n\":2}".into() });
        waiting.push(Outbound::Frame {
            app: "clips",
            slot: 3,
            width: 2,
            height: 1,
            format: crate::phone::FrameFormat::Rgba8,
            bytes: vec![9; 8],
        });
        for i in 0..40 {
            waiting.push(Outbound::Notice { app: "clips", text: format!("{i}") });
        }
        let all: Vec<Outbound> = std::iter::from_fn(|| waiting.pop()).collect();
        let states: Vec<&str> = all
            .iter()
            .filter_map(|m| match m {
                Outbound::State { json, .. } => Some(json.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(states, ["{}", "{\"n\":2}"], "the newer spotify state replaced the older one");
        let frames: Vec<&[u8]> = all
            .iter()
            .filter_map(|m| match m {
                Outbound::Frame { bytes, .. } => Some(bytes.as_slice()),
                _ => None,
            })
            .collect();
        assert_eq!(frames, [&[9u8; 8][..]], "the newer picture of slot 3 replaced the older one");
        let notices = all.iter().filter(|m| matches!(m, Outbound::Notice { .. })).count();
        assert_eq!(notices, MAX_WAITING_NOTICES, "the oldest notices went");
    }

    #[tokio::test]
    async fn a_real_socket_serves_the_phone_and_a_newer_phone_takes_over() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let root = std::env::temp_dir()
            .join(format!("bmc-ph-{}", crate::util::hex_encode(&crate::util::random_bytes::<4>().unwrap())));
        crate::util::create_private_dir_all(&root).unwrap();
        let endpoint = Endpoint::create(None, &root).unwrap();
        let path = endpoint.path().to_path_buf();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (out, outbound) = crate::phone::Outbox::new(64);
        let dispatcher = Dispatcher::new(vec![Box::new(Probe(seen.clone()))], out.clone(), Arc::new(|_| {}));
        let serving = endpoint.serve_phone(Arc::new(|_, _| {}), Some(Phone { dispatcher, outbound })).unwrap();

        async fn read_frame(stream: &mut tokio::net::UnixStream) -> (u8, Vec<u8>) {
            let mut header = [0u8; 5];
            tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut header)).await.unwrap().unwrap();
            let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
            let mut payload = vec![0; length - 1];
            stream.read_exact(&mut payload).await.unwrap();
            (header[4], payload)
        }

        let mut phone = tokio::net::UnixStream::connect(&path).await.unwrap();
        phone.write_all(b"hello 2 0.1.0 phone\n").await.unwrap();
        // Greeted: the phone state, then the app's.
        let (kind, payload) = read_frame(&mut phone).await;
        assert_eq!(
            (kind, payload.as_slice()),
            (KIND_STATE, &b"phone\0{\"apps\":[\"spotify\"],\"theme\":\"dark\"}"[..])
        );
        let (kind, payload) = read_frame(&mut phone).await;
        assert_eq!((kind, payload.as_slice()), (KIND_STATE, &b"spotify\0{\"hello\":true}"[..]));
        // A command, and the app's answer.
        phone.write_all(&frame(KIND_COMMAND, b"spotify play")).await.unwrap();
        let (kind, payload) = read_frame(&mut phone).await;
        assert_eq!((kind, payload.as_slice()), (KIND_NOTICE, &b"spotify\0ok"[..]));
        assert_eq!(seen.lock().unwrap().len(), 1);
        // A picture well over a socket buffer arrives whole.
        let big = vec![7u8; 1024 * 1024];
        assert!(out.frame("spotify", 0, 512, 512, crate::phone::FrameFormat::Rgba8, big.clone()));
        let (kind, payload) = read_frame(&mut phone).await;
        assert_eq!(kind, KIND_FRAME);
        assert_eq!(payload.len(), 8 + 6 + big.len());
        // A second phone takes over and is greeted; the first gets nothing more.
        let mut second = tokio::net::UnixStream::connect(&path).await.unwrap();
        second.write_all(b"hello 2 0.1.0 phone\n").await.unwrap();
        let (kind, payload) = read_frame(&mut second).await;
        assert_eq!(kind, KIND_STATE);
        assert!(payload.starts_with(b"phone\0"));
        let summary = serving.phone_summary().unwrap();
        assert!(summary.contains("2 connection(s) greeted, 1 command(s), 1 accepted"), "{summary}");
        assert_eq!(
            serving.finish(),
            "companion channel: 2 connection(s), 2 hello, 0 diagnostic(s), 1 phone command(s)"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_phone_fuzz_harness_accepts_ordinary_traffic() {
        fuzz_phone(&[frame(KIND_COMMAND, b"spotify play"), frame(KIND_DIAG, b"diag zoom x")].concat());
        fuzz_phone(&frame(KIND_COMMAND, b"spotify search -- x").repeat(20));
        fuzz_phone(&[0xff; 40]);
        fuzz_phone(b"");
    }

    #[test]
    fn a_socket_path_that_would_not_fit_is_refused_before_anything_is_created() {
        let deep = std::env::temp_dir().join("x".repeat(120));
        let error = Endpoint::create(None, &deep).err().expect("refused").to_string();
        assert!(error.contains("too long for a socket"), "{error}");
        assert!(!deep.exists());
    }
}
