//! The phone: what the companion mod's in-game phone may ask of the launcher, and what the
//! launcher sends back.
//!
//! This module is the contract between the parts of round 4 (docs/notes/round4-contract.md).
//! The services — Spotify, the browser, clips, Realms approval, settings sync — implement
//! [`Service`] (what they set up before the launcher confines itself) and [`PhoneApp`] (how
//! they answer the phone afterwards). The companion channel, protocol 2, carries
//! [`Command`]s from the mod and [`Outbound`] messages to it; a dispatcher between the two
//! enforces each app's [`VerbLimit`]s.
//!
//! Everything in a [`Command`] was written by code inside the game, which is hostile by
//! assumption. An app does only what its fixed vocabulary allows, never treats an argument
//! as a path, a URL or code, and holds nothing the phone could ask to have handed over.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::confine::Confinement;

pub mod demo;
pub mod dispatch;

/// The longest command line the mod may send (bytes).
pub const MAX_COMMAND_BYTES: usize = 1024;
/// At most this many arguments before the free text.
pub const MAX_ARGS: usize = 8;
/// One argument (bytes).
pub const MAX_ARG_BYTES: usize = 64;
/// An app or verb name (bytes).
pub const MAX_NAME_BYTES: usize = 16;
/// The free text after ` -- ` (bytes, UTF-8).
pub const MAX_TEXT_BYTES: usize = 512;
/// One state message to the mod: an app's state as a JSON object (bytes).
pub const MAX_STATE_BYTES: usize = 64 * 1024;
/// One image to the mod (bytes).
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
/// One notice (bytes).
pub const MAX_NOTICE_BYTES: usize = 256;

/// A command from the phone: `<app> <verb> [arg]... [-- <text>]`.
///
/// `app` and `verb` are `[a-z0-9-]`, 1 to 16 bytes, not starting with `-`. Arguments are
/// printable ASCII words without spaces, at most [`MAX_ARGS`] of [`MAX_ARG_BYTES`] each, and
/// never `--`. Free text (a search query, what is typed into a web page) follows ` -- `: UTF-8
/// of at most [`MAX_TEXT_BYTES`], without control characters or bidirectional overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub app: String,
    pub verb: String,
    pub args: Vec<String>,
    pub text: Option<String>,
}

/// Why a command line was refused: one word per reason, for counting and for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Empty,
    TooLong,
    NotUtf8,
    ControlCharacter,
    BadName,
    TooManyArguments,
    BadArgument,
    TextTooLong,
}

impl Refused {
    pub fn as_str(self) -> &'static str {
        match self {
            Refused::Empty => "empty",
            Refused::TooLong => "too long",
            Refused::NotUtf8 => "not UTF-8",
            Refused::ControlCharacter => "control character",
            Refused::BadName => "bad app or verb name",
            Refused::TooManyArguments => "too many arguments",
            Refused::BadArgument => "bad argument",
            Refused::TextTooLong => "text too long",
        }
    }
}

/// Characters that change how text is displayed without being visible: refused everywhere,
/// so that nothing the game sends can make a log line or a page read differently.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn is_name(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= MAX_NAME_BYTES
        && !word.starts_with('-')
        && word.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_argument(word: &str) -> bool {
    !word.is_empty() && word.len() <= MAX_ARG_BYTES && word != "--" && word.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

impl Command {
    /// Parse one command line (without its line ending). Strict: anything outside the
    /// grammar above is refused, nothing is repaired.
    pub fn parse(line: &[u8]) -> Result<Command, Refused> {
        if line.len() > MAX_COMMAND_BYTES {
            return Err(Refused::TooLong);
        }
        let line = std::str::from_utf8(line).map_err(|_| Refused::NotUtf8)?;
        if line.chars().any(|c| c.is_control() || is_bidi_control(c)) {
            return Err(Refused::ControlCharacter);
        }
        let (head, text) = match line.split_once(" -- ") {
            Some((head, text)) => (head, Some(text)),
            None => (line, None),
        };
        if let Some(text) = text
            && text.len() > MAX_TEXT_BYTES
        {
            return Err(Refused::TextTooLong);
        }
        let mut words = head.split(' ');
        let app = words.next().filter(|w| !w.is_empty()).ok_or(Refused::Empty)?;
        let verb = words.next().ok_or(Refused::BadName)?;
        if !is_name(app) || !is_name(verb) {
            return Err(Refused::BadName);
        }
        let mut args = Vec::new();
        for word in words {
            if args.len() == MAX_ARGS {
                return Err(Refused::TooManyArguments);
            }
            if !is_argument(word) {
                return Err(Refused::BadArgument);
            }
            args.push(word.to_string());
        }
        Ok(Command { app: app.into(), verb: verb.into(), args, text: text.map(str::to_string) })
    }
}

/// The encoding of an image sent to the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameFormat {
    Png = 1,
    Jpeg = 2,
    /// Uncompressed, 8 bits per channel, rows top to bottom, no padding.
    Rgba8 = 3,
}

/// What the launcher sends to the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    /// The state of one app, a JSON object; replaces the app's previous state.
    State { app: &'static str, json: String },
    /// An image for one app (a page of the browser, album art, a clip's thumbnail).
    /// `slot` tells images of the same app apart (each slot shows the newest image).
    Frame { app: &'static str, slot: u8, width: u16, height: u16, format: FrameFormat, bytes: Vec<u8> },
    /// One line for the phone's notification area, printable characters only.
    Notice { app: &'static str, text: String },
}

/// Where an app puts what it sends to the phone. Cheap to clone. Sending never blocks: when
/// the mod reads too slowly, messages are dropped and counted, so that a stalled game cannot
/// stall a service. Anything over its size limit is refused (`false`), never cut.
#[derive(Clone)]
pub struct Outbox {
    tx: tokio::sync::mpsc::Sender<Outbound>,
    dropped: Arc<AtomicU64>,
}

impl Outbox {
    /// An outbox and the receiving end the channel's writer (or a test) reads from.
    pub fn new(capacity: usize) -> (Outbox, tokio::sync::mpsc::Receiver<Outbound>) {
        let (tx, rx) = tokio::sync::mpsc::channel(capacity.max(1));
        (Outbox { tx, dropped: Arc::default() }, rx)
    }

    fn send(&self, message: Outbound) -> bool {
        let sent = self.tx.try_send(message).is_ok();
        if !sent {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        sent
    }

    /// Send an app's state. `value` must be a JSON object of at most [`MAX_STATE_BYTES`].
    pub fn state(&self, app: &'static str, value: &serde_json::Value) -> bool {
        if !value.is_object() {
            return false;
        }
        let json = value.to_string();
        json.len() <= MAX_STATE_BYTES && self.send(Outbound::State { app, json })
    }

    /// Send an image of at most [`MAX_FRAME_BYTES`]; for [`FrameFormat::Rgba8`] the length
    /// must be exactly `width * height * 4`.
    pub fn frame(
        &self,
        app: &'static str,
        slot: u8,
        width: u16,
        height: u16,
        format: FrameFormat,
        bytes: Vec<u8>,
    ) -> bool {
        let fits = bytes.len() <= MAX_FRAME_BYTES && !bytes.is_empty();
        let exact = format != FrameFormat::Rgba8 || bytes.len() == usize::from(width) * usize::from(height) * 4;
        fits && exact
            && width > 0
            && height > 0
            && self.send(Outbound::Frame { app, slot, width, height, format, bytes })
    }

    /// Send a one-line notice: made printable and cut to [`MAX_NOTICE_BYTES`] at a character boundary.
    pub fn notice(&self, app: &'static str, text: &str) -> bool {
        let mut clean = String::new();
        for c in text.chars().filter(|c| !c.is_control() && !is_bidi_control(*c)) {
            if clean.len() + c.len_utf8() > MAX_NOTICE_BYTES {
                break;
            }
            clean.push(c);
        }
        self.send(Outbound::Notice { app, text: clean })
    }

    /// Messages dropped so far because the phone did not keep up.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// How often the phone may use one verb of an app: a token bucket of `burst` tokens that
/// refills at `per_minute`. Verbs an app does not list are refused by the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerbLimit {
    pub verb: &'static str,
    pub per_minute: u32,
    pub burst: u32,
}

/// One app of the phone, implemented by a launcher service, used after the launcher has
/// confined itself. Called on the async runtime: never block; spawn longer work.
pub trait PhoneApp: Send {
    /// The app's name on the wire (`spotify`, `browser`, `clips`, `realms`, `settings`).
    fn name(&self) -> &'static str;
    /// The verbs this app accepts and how often each may be used.
    fn limits(&self) -> &'static [VerbLimit];
    /// The phone connected (or reconnected): send the current state.
    fn hello(&mut self, out: &Outbox);
    /// A command from the phone, already parsed, addressed to this app, within its limits.
    fn command(&mut self, command: Command, out: &Outbox);
}

/// A service as the launch prepares it, before the launcher confines itself (steps 6 to 8):
/// every helper process it needs has been started and every socket connected by then,
/// because the confined launcher can do neither.
pub trait Service: Send {
    /// The app's name, as [`PhoneApp::name`].
    fn name(&self) -> &'static str;
    /// One line for the launch log: what was prepared, or why the service is off.
    fn describe(&self) -> String;
    /// Add what the confined launcher still needs for this service: directories it writes,
    /// TCP ports it connects to. Nothing else can be added (no program execution, no new
    /// pathname sockets).
    fn confinement_needs(&self, keep: &mut Confinement);
    /// After the confinement (step 10): the app that answers the phone.
    fn into_app(self: Box<Self>) -> Box<dyn PhoneApp>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_follow_the_grammar_and_nothing_else() {
        let c = Command::parse(b"spotify volume 40").unwrap();
        assert_eq!(
            (c.app.as_str(), c.verb.as_str(), c.args.as_slice(), c.text),
            ("spotify", "volume", &["40".to_string()][..], None)
        );
        let c = Command::parse("spotify search -- Daft Punk – Harder, Better ☕".as_bytes()).unwrap();
        assert_eq!(c.text.as_deref(), Some("Daft Punk – Harder, Better ☕"));
        assert!(c.args.is_empty());
        let c = Command::parse(b"browser text -- ").unwrap();
        assert_eq!(c.text.as_deref(), Some(""), "empty text is text");
        let c = Command::parse(b"browser mouse down 120 48 left").unwrap();
        assert_eq!(c.args, ["down", "120", "48", "left"]);
        // An emoji sequence with a zero-width joiner is ordinary text.
        assert!(Command::parse("browser text -- 👩\u{200D}💻".as_bytes()).is_ok());

        for (line, why) in [
            (&b""[..], Refused::Empty),
            (b"spotify", Refused::BadName),
            (b"Spotify play", Refused::BadName),
            (b"-x play", Refused::BadName),
            (b"seventeen-letters play", Refused::BadName),
            (b"spotify  play", Refused::BadName),
            (b"spotify play  x", Refused::BadArgument),
            (b"spotify play --", Refused::BadArgument),
            (b"spotify play \xc3\xa9", Refused::BadArgument),
            (b"a b 1 2 3 4 5 6 7 8 9", Refused::TooManyArguments),
            (b"spotify play\tx", Refused::ControlCharacter),
            (b"spotify play\n", Refused::ControlCharacter),
            (b"spotify search -- \xe2\x80\xae evil", Refused::ControlCharacter),
            (b"spotify play \xff", Refused::NotUtf8),
        ] {
            assert_eq!(Command::parse(line), Err(why), "{:?}", String::from_utf8_lossy(line));
        }
        let long_arg = format!("a b {}", "x".repeat(MAX_ARG_BYTES + 1));
        assert_eq!(Command::parse(long_arg.as_bytes()), Err(Refused::BadArgument));
        let long_text = format!("a b -- {}", "x".repeat(MAX_TEXT_BYTES + 1));
        assert_eq!(Command::parse(long_text.as_bytes()), Err(Refused::TextTooLong));
        let too_long = format!("a b -- {}", "x".repeat(MAX_COMMAND_BYTES));
        assert_eq!(Command::parse(too_long.as_bytes()), Err(Refused::TooLong));
    }

    #[test]
    fn the_outbox_refuses_what_is_too_large_and_never_blocks() {
        let (out, mut rx) = Outbox::new(2);
        assert!(out.state("spotify", &serde_json::json!({"playing": true})));
        assert!(!out.state("spotify", &serde_json::json!([1, 2])), "only objects");
        let big = serde_json::json!({ "x": "y".repeat(MAX_STATE_BYTES) });
        assert!(!out.state("spotify", &big), "refused, not cut");
        assert!(!out.frame("browser", 0, 2, 2, FrameFormat::Rgba8, vec![0; 15]), "RGBA must be exact");
        assert!(out.frame("browser", 0, 2, 2, FrameFormat::Rgba8, vec![0; 16]));
        // Full: dropped and counted, not waited for.
        assert!(!out.notice("clips", "saved"));
        assert_eq!(out.dropped(), 1);
        assert!(matches!(rx.try_recv(), Ok(Outbound::State { app: "spotify", .. })));
        assert!(matches!(rx.try_recv(), Ok(Outbound::Frame { width: 2, .. })));
        assert!(out.notice("clips", &format!("a\u{7}b\u{202e}c{}", "é".repeat(MAX_NOTICE_BYTES))));
        let Ok(Outbound::Notice { text, .. }) = rx.try_recv() else { panic!("a notice") };
        assert!(text.starts_with("abc") && text.len() <= MAX_NOTICE_BYTES && !text.contains('\u{7}'));
    }
}
