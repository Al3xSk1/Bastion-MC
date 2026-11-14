//! The phone's `browser` app: its vocabulary, checked word by word, and the hand-over to the
//! driver. Nothing here waits: a command the driver cannot take right now is dropped and
//! the phone is told.

use serde_json::json;
use tokio::sync::mpsc;

use super::{
    bidi::{Button, Key, MouseKind},
    config::{MAX_VIEWPORT, MIN_VIEWPORT, Site},
    driver::{APP, DriverMsg},
};
use crate::phone::{Command, Outbox, PhoneApp, VerbLimit};

/// How far one `scroll` may move, either way, in pixels.
pub const MAX_SCROLL: i32 = 10_000;

/// The verbs and how often each may be used.
pub const LIMITS: &[VerbLimit] = &[
    VerbLimit { verb: "open", per_minute: 20, burst: 5 },
    VerbLimit { verb: "close", per_minute: 20, burst: 5 },
    VerbLimit { verb: "back", per_minute: 60, burst: 10 },
    VerbLimit { verb: "forward", per_minute: 60, burst: 10 },
    VerbLimit { verb: "reload", per_minute: 30, burst: 5 },
    VerbLimit { verb: "size", per_minute: 20, burst: 5 },
    VerbLimit { verb: "mouse", per_minute: 1800, burst: 120 },
    VerbLimit { verb: "scroll", per_minute: 600, burst: 60 },
    VerbLimit { verb: "key", per_minute: 600, burst: 40 },
    // The phone sends what is typed every 0.1 s while typing goes on (BrowserApp.java).
    VerbLimit { verb: "text", per_minute: 600, burst: 60 },
    VerbLimit { verb: "frames", per_minute: 60, burst: 10 },
];

/// A command of the browser app, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserCommand {
    /// Open the site with this index in the configured list.
    Open(usize),
    Close,
    Back,
    Forward,
    Reload,
    Size(u32, u32),
    Mouse(MouseKind, u32, u32, Button),
    Scroll(i32, i32),
    Key(Key),
    Text(String),
    Frames(bool),
}

fn number<T: std::str::FromStr>(word: &str) -> Option<T> {
    // Digits only (and one leading minus where the type allows it): no `+`, no spaces.
    let digits = word.strip_prefix('-').unwrap_or(word);
    (!digits.is_empty() && digits.len() <= 6 && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| word.parse().ok())
        .flatten()
}

impl BrowserCommand {
    /// Check a command addressed to the browser against the vocabulary. `sites` are the
    /// configured site names, in order. The error is what the phone is told.
    pub fn parse(command: &Command, sites: &[String]) -> Result<BrowserCommand, &'static str> {
        let args: Vec<&str> = command.args.iter().map(String::as_str).collect();
        if command.text.is_some() && command.verb != "text" {
            return Err("this command takes no text");
        }
        let parsed = match (command.verb.as_str(), args.as_slice()) {
            ("open", [name]) => {
                BrowserCommand::Open(sites.iter().position(|s| s == name).ok_or("there is no such site in the list")?)
            }
            ("close", []) => BrowserCommand::Close,
            ("back", []) => BrowserCommand::Back,
            ("forward", []) => BrowserCommand::Forward,
            ("reload", []) => BrowserCommand::Reload,
            ("size", [w, h]) => {
                let (w, h): (u32, u32) = (number(w).ok_or("size: two numbers")?, number(h).ok_or("size: two numbers")?);
                let range = MIN_VIEWPORT..=MAX_VIEWPORT;
                if !range.contains(&w) || !range.contains(&h) {
                    return Err("size: between 160 and 1280 pixels either way");
                }
                BrowserCommand::Size(w, h)
            }
            ("mouse", [kind, x, y, rest @ ..]) if rest.len() <= 1 => {
                let kind = match *kind {
                    "move" => MouseKind::Move,
                    "down" => MouseKind::Down,
                    "up" => MouseKind::Up,
                    _ => return Err("mouse: move, down or up"),
                };
                let x: u32 = number(x).ok_or("mouse: a position in pixels")?;
                let y: u32 = number(y).ok_or("mouse: a position in pixels")?;
                if x >= MAX_VIEWPORT || y >= MAX_VIEWPORT {
                    return Err("mouse: outside the page");
                }
                let button = match rest.first().copied() {
                    None | Some("left") => Button::Left,
                    Some("middle") => Button::Middle,
                    Some("right") => Button::Right,
                    Some(_) => return Err("mouse: left, middle or right"),
                };
                BrowserCommand::Mouse(kind, x, y, button)
            }
            ("scroll", [dx, dy]) => {
                let dx: i32 = number(dx).ok_or("scroll: two numbers")?;
                let dy: i32 = number(dy).ok_or("scroll: two numbers")?;
                if dx.abs() > MAX_SCROLL || dy.abs() > MAX_SCROLL {
                    return Err("scroll: at most 10000 pixels at once");
                }
                BrowserCommand::Scroll(dx, dy)
            }
            ("key", [name]) => {
                BrowserCommand::Key(Key::parse(name).ok_or("key: not one of the keys the phone may press")?)
            }
            ("text", []) => BrowserCommand::Text(command.text.clone().ok_or("text: the text follows ` -- `")?),
            ("frames", ["on"]) => BrowserCommand::Frames(true),
            ("frames", ["off"]) => BrowserCommand::Frames(false),
            _ => return Err("not a command of the browser"),
        };
        Ok(parsed)
    }
}

/// The phone app. Cheap: it only checks commands and hands them on.
pub struct BrowserApp {
    tx: mpsc::Sender<DriverMsg>,
    names: Vec<String>,
    titles: Vec<String>,
}

impl BrowserApp {
    pub fn new(tx: mpsc::Sender<DriverMsg>, sites: &[Site]) -> BrowserApp {
        BrowserApp {
            tx,
            names: sites.iter().map(|s| s.name.clone()).collect(),
            titles: sites.iter().map(|s| s.title.clone()).collect(),
        }
    }
}

impl PhoneApp for BrowserApp {
    fn name(&self) -> &'static str {
        APP
    }

    fn limits(&self) -> &'static [VerbLimit] {
        LIMITS
    }

    fn hello(&mut self, out: &Outbox) {
        // Something to show at once; the driver follows with the whole state.
        let sites: Vec<_> =
            self.names.iter().zip(&self.titles).map(|(name, title)| json!({"name": name, "title": title})).collect();
        out.state(APP, &json!({"sites": sites, "open": null, "host": null, "loading": true, "title": "", "error": null, "signed_in_hint": false, "ready": false}));
        if self.tx.try_send(DriverMsg::Hello(out.clone())).is_err() {
            out.notice(APP, "the browser is busy; it will show its state shortly");
        }
    }

    fn command(&mut self, command: Command, out: &Outbox) {
        match BrowserCommand::parse(&command, &self.names) {
            Ok(parsed) => {
                if self.tx.try_send(DriverMsg::Command(parsed, out.clone())).is_err() {
                    out.notice(APP, "the browser is busy: that was dropped");
                }
            }
            Err(why) => {
                out.notice(APP, why);
            }
        }
    }
}

/// Fuzzing entry (`fuzzing::browser_command`): a command line as the phone sends it, through
/// the contract's parser and then the browser's.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    let sites = ["discord".to_string(), "wiki".to_string()];
    if let Ok(command) = Command::parse(data)
        && command.app == APP
        && let Ok(parsed) = BrowserCommand::parse(&command, &sites)
    {
        match parsed {
            BrowserCommand::Open(i) => assert!(i < sites.len()),
            BrowserCommand::Size(w, h) => {
                assert!((MIN_VIEWPORT..=MAX_VIEWPORT).contains(&w) && (MIN_VIEWPORT..=MAX_VIEWPORT).contains(&h))
            }
            BrowserCommand::Mouse(_, x, y, _) => assert!(x < MAX_VIEWPORT && y < MAX_VIEWPORT),
            BrowserCommand::Scroll(dx, dy) => assert!(dx.abs() <= MAX_SCROLL && dy.abs() <= MAX_SCROLL),
            BrowserCommand::Text(text) => assert!(text.len() <= crate::phone::MAX_TEXT_BYTES),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<BrowserCommand, &'static str> {
        let sites = ["discord".to_string(), "wiki".to_string()];
        BrowserCommand::parse(&Command::parse(line.as_bytes()).unwrap(), &sites)
    }

    #[test]
    fn every_verb_of_the_contract_is_understood() {
        assert_eq!(parse("browser open wiki"), Ok(BrowserCommand::Open(1)));
        assert_eq!(parse("browser close"), Ok(BrowserCommand::Close));
        assert_eq!(parse("browser back"), Ok(BrowserCommand::Back));
        assert_eq!(parse("browser forward"), Ok(BrowserCommand::Forward));
        assert_eq!(parse("browser reload"), Ok(BrowserCommand::Reload));
        assert_eq!(parse("browser size 400 700"), Ok(BrowserCommand::Size(400, 700)));
        assert_eq!(parse("browser mouse down 10 20"), Ok(BrowserCommand::Mouse(MouseKind::Down, 10, 20, Button::Left)));
        assert_eq!(
            parse("browser mouse up 10 20 right"),
            Ok(BrowserCommand::Mouse(MouseKind::Up, 10, 20, Button::Right))
        );
        assert_eq!(
            parse("browser mouse move 0 0 middle"),
            Ok(BrowserCommand::Mouse(MouseKind::Move, 0, 0, Button::Middle))
        );
        assert_eq!(parse("browser scroll 0 -240"), Ok(BrowserCommand::Scroll(0, -240)));
        assert_eq!(parse("browser key Enter"), Ok(BrowserCommand::Key(Key::Named("Enter", '\u{E007}'))));
        assert_eq!(parse("browser key ctrl-c"), Ok(BrowserCommand::Key(Key::Ctrl('c'))));
        assert_eq!(parse("browser text -- hello ☕"), Ok(BrowserCommand::Text("hello ☕".into())));
        assert_eq!(parse("browser frames on"), Ok(BrowserCommand::Frames(true)));
        assert_eq!(parse("browser frames off"), Ok(BrowserCommand::Frames(false)));
        let verbs: Vec<&str> = LIMITS.iter().map(|l| l.verb).collect();
        assert_eq!(
            verbs,
            ["open", "close", "back", "forward", "reload", "size", "mouse", "scroll", "key", "text", "frames"]
        );
    }

    #[test]
    fn anything_else_is_refused_with_a_reason() {
        for line in [
            "browser open https://evil.example/",
            "browser open slack",
            "browser open",
            "browser close now",
            "browser size 100 700",
            "browser size 400 2000",
            "browser size +400 700",
            "browser size 4e2 700",
            "browser mouse click 1 2",
            "browser mouse down 1",
            "browser mouse down -1 2",
            "browser mouse down 1 2 left extra",
            "browser mouse down 1 2 back",
            "browser mouse down 1300 2",
            "browser scroll 0 99999",
            "browser scroll x 1",
            "browser key F12",
            "browser key ctrl-w",
            "browser key Enter Enter",
            "browser text",
            "browser text hello",
            "browser reload -- now",
            "browser frames maybe",
            "browser navigate https://evil.example/",
            "browser eval -- document.cookie",
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
    }
}
