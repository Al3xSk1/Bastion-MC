//! WebDriver BiDi, the part the phone's browser uses: what Firefox's messages say, what the
//! phone's commands become, and which requests a page may make.
//!
//! Everything here is a pure function of its input, so that it can be tested and fuzzed
//! without a browser. Messages from Firefox are hostile input (the browser renders pages
//! from the internet): they are parsed into [`Incoming`] with every field optional, and a
//! message that does not have the expected shape is ignored.

use serde_json::{Value, json};

use super::config::{Site, https_host};

/// One message from the browser.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// The answer to command `id`.
    Reply {
        id: u64,
        result: Value,
    },
    /// Command `id` (or a message the browser could not attribute) failed.
    Error {
        id: Option<u64>,
        error: String,
        message: String,
    },
    Event {
        method: String,
        params: Value,
    },
}

/// Read one text message. `None` for anything that is not one of the three shapes.
pub fn parse_incoming(text: &str) -> Option<Incoming> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    match object.get("type")?.as_str()? {
        "success" => Some(Incoming::Reply {
            id: object.get("id")?.as_u64()?,
            result: object.get("result").cloned().unwrap_or(Value::Null),
        }),
        "error" => Some(Incoming::Error {
            id: object.get("id").and_then(Value::as_u64),
            error: short(object.get("error").and_then(Value::as_str).unwrap_or("error"), 64),
            message: short(object.get("message").and_then(Value::as_str).unwrap_or(""), 200),
        }),
        "event" => Some(Incoming::Event {
            method: short(object.get("method")?.as_str()?, 64),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        }),
        _ => None,
    }
}

/// `text` made printable and cut to `max` characters.
pub fn short(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
        .take(max)
        .collect()
}

/// A string field of an event's parameters.
pub fn field<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

/// The events the driver subscribes to; without any of these it does not work.
pub const EVENTS: &[&str] = &[
    "browsingContext.contextCreated",
    "browsingContext.contextDestroyed",
    "browsingContext.navigationStarted",
    "browsingContext.navigationFailed",
    "browsingContext.fragmentNavigated",
    "browsingContext.domContentLoaded",
    "browsingContext.load",
    "browsingContext.userPromptOpened",
    "network.beforeRequestSent",
];

/// Events it uses where the browser has them (newer additions to WebDriver BiDi; Firefox
/// 159 refuses `navigationAborted`).
pub const OPTIONAL_EVENTS: &[&str] =
    &["browsingContext.historyUpdated", "browsingContext.navigationAborted", "input.fileDialogOpened"];

/// What happens to a request the browser paused for the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Continue,
    /// Refused; `notice` is set when the phone should be told (a page it shows navigated).
    Fail {
        why: Refusal,
        host: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No site is open.
    NothingOpen,
    /// Not `https:` (or `wss:`): the relay serves nothing else.
    NotHttps,
    /// A host the open site does not list.
    OtherHost,
    /// The open site's budget of requests for this minute is used up.
    Budget,
    /// The open site's budget of API calls for this minute is used up.
    ApiBudget,
}

/// Which of a site's request budgets a request is taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// Every request.
    Requests,
    /// API calls (`fetch`, `XMLHttpRequest`, WebSocket, beacons), in addition.
    ApiCalls,
}

/// One paused request, as far as the decision needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paused {
    pub request: String,
    pub url: String,
    /// The browsing context it belongs to (absent for some background requests).
    pub context: Option<String>,
    /// Part of a navigation (a page, not something a page loads).
    pub navigation: bool,
    /// Made by a script for data, not to show something: an API call.
    pub api_call: bool,
}

/// Is a request with this `destination` and `initiatorType` (WebDriver BiDi's names, from
/// Fetch) an API call? Requests for data have no destination; WebSocket handshakes and
/// beacons are named by their initiator.
pub fn is_api_call(destination: Option<&str>, initiator: Option<&str>) -> bool {
    matches!(initiator, Some("fetch" | "xmlhttprequest" | "beacon" | "websocket" | "eventsource"))
        || (destination == Some("")
            && !matches!(initiator, Some("img" | "css" | "link" | "script" | "font" | "video" | "audio")))
}

/// The paused request in a `network.beforeRequestSent` event, if the event is one.
pub fn paused(params: &Value) -> Option<Paused> {
    if params.get("isBlocked").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let request = params.get("request")?;
    Some(Paused {
        request: short(request.get("request")?.as_str()?, 128),
        url: request.get("url")?.as_str()?.chars().take(8192).collect(),
        context: field(params, "context").map(|c| short(c, 128)),
        navigation: params.get("navigation").is_some_and(|n| n.is_string()),
        api_call: is_api_call(
            request.get("destination").and_then(Value::as_str),
            request.get("initiatorType").and_then(Value::as_str),
        ),
    })
}

/// The host a request goes to, if it goes to an `https:` or `wss:` URL.
pub fn request_host(url: &str) -> Option<String> {
    match url.get(..6) {
        Some(s) if s.eq_ignore_ascii_case("wss://") => https_host(&format!("https://{}", &url[6..])),
        _ => https_host(url),
    }
}

/// Requests that never reach the network.
fn is_local(url: &str) -> bool {
    let lower: String = url.chars().take(6).collect::<String>().to_ascii_lowercase();
    lower.starts_with("data:") || lower.starts_with("blob:") || lower.starts_with("about:")
}

/// Decide about a paused request: `open` is the open site, `take` takes one request from
/// one of its budgets (and says whether there was one). An API call is taken from both.
pub fn decide(url: &str, open: Option<&Site>, api_call: bool, mut take: impl FnMut(Budget) -> bool) -> Verdict {
    if is_local(url) {
        return Verdict::Continue;
    }
    let host = request_host(url);
    let Some(site) = open else { return Verdict::Fail { why: Refusal::NothingOpen, host } };
    let Some(name) = host.clone() else { return Verdict::Fail { why: Refusal::NotHttps, host } };
    if !site.allows(&name) {
        return Verdict::Fail { why: Refusal::OtherHost, host };
    }
    if api_call && !take(Budget::ApiCalls) {
        return Verdict::Fail { why: Refusal::ApiBudget, host };
    }
    if !take(Budget::Requests) {
        return Verdict::Fail { why: Refusal::Budget, host };
    }
    Verdict::Continue
}

// ---------------------------------------------------------------- input

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Move,
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left = 0,
    Middle = 1,
    Right = 2,
}

/// The keys the phone may press (the contract's fixed list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Named(&'static str, char),
    /// Control with a letter: `ctrl-a`, `-c`, `-v`, `-x`, `-z`.
    Ctrl(char),
}

const NAMED_KEYS: &[(&str, char)] = &[
    ("Enter", '\u{E007}'),
    ("Backspace", '\u{E003}'),
    ("Tab", '\u{E004}'),
    ("Escape", '\u{E00C}'),
    ("Delete", '\u{E017}'),
    ("Home", '\u{E011}'),
    ("End", '\u{E010}'),
    ("PageUp", '\u{E00E}'),
    ("PageDown", '\u{E00F}'),
    ("ArrowUp", '\u{E013}'),
    ("ArrowDown", '\u{E015}'),
    ("ArrowLeft", '\u{E012}'),
    ("ArrowRight", '\u{E014}'),
];
const CONTROL: char = '\u{E009}';

impl Key {
    /// `Enter` (or `enter`), …, `ctrl-a`. Nothing else.
    pub fn parse(name: &str) -> Option<Key> {
        if let Some(letter) = name.strip_prefix("ctrl-") {
            return match letter {
                "a" | "c" | "v" | "x" | "z" => letter.chars().next().map(Key::Ctrl),
                _ => None,
            };
        }
        NAMED_KEYS
            .iter()
            .find(|(n, _)| *n == name || n.to_ascii_lowercase() == name)
            .map(|(n, code)| Key::Named(n, *code))
    }
}

pub fn pointer_actions(kind: MouseKind, x: u32, y: u32, button: Button) -> Value {
    let mut actions = vec![json!({"type": "pointerMove", "x": x, "y": y, "origin": "viewport", "duration": 0})];
    match kind {
        MouseKind::Move => {}
        MouseKind::Down => actions.push(json!({"type": "pointerDown", "button": button as u8})),
        MouseKind::Up => actions.push(json!({"type": "pointerUp", "button": button as u8})),
    }
    json!([{"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"}, "actions": actions}])
}

pub fn scroll_actions(x: u32, y: u32, dx: i32, dy: i32) -> Value {
    json!([{"type": "wheel", "id": "wheel", "actions": [
        {"type": "scroll", "x": x, "y": y, "deltaX": dx, "deltaY": dy, "origin": "viewport", "duration": 0}
    ]}])
}

pub fn key_actions(key: Key) -> Value {
    let actions = match key {
        Key::Named(_, code) => vec![json!({"type": "keyDown", "value": code}), json!({"type": "keyUp", "value": code})],
        Key::Ctrl(letter) => vec![
            json!({"type": "keyDown", "value": CONTROL}),
            json!({"type": "keyDown", "value": letter}),
            json!({"type": "keyUp", "value": letter}),
            json!({"type": "keyUp", "value": CONTROL}),
        ],
    };
    json!([{"type": "key", "id": "keyboard", "actions": actions}])
}

/// Characters that belong to the one before them: combining marks, joiners, variation
/// selectors, emoji modifiers and tags. (An approximation of grapheme clusters, without
/// Unicode's tables: each key action carries one cluster.)
fn extends(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}' | '\u{0483}'..='\u{0489}' | '\u{0591}'..='\u{05BD}' | '\u{0610}'..='\u{061A}'
        | '\u{064B}'..='\u{065F}' | '\u{0900}'..='\u{0903}' | '\u{093A}'..='\u{094F}' | '\u{1AB0}'..='\u{1AFF}'
        | '\u{1DC0}'..='\u{1DFF}' | '\u{200C}' | '\u{200D}' | '\u{20D0}'..='\u{20FF}' | '\u{FE00}'..='\u{FE0F}'
        | '\u{FE20}'..='\u{FE2F}' | '\u{1F3FB}'..='\u{1F3FF}' | '\u{E0020}'..='\u{E007F}' | '\u{E0100}'..='\u{E01EF}')
}

fn regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// `text` split into the pieces one key action each types.
pub fn clusters(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut after_joiner = false;
    for c in text.chars() {
        let pair = regional_indicator(c)
            && out.last().is_some_and(|l| l.chars().count() == 1 && l.chars().all(regional_indicator));
        match out.last_mut() {
            Some(last) if extends(c) || after_joiner || pair => last.push(c),
            _ => out.push(c.to_string()),
        }
        after_joiner = c == '\u{200D}';
    }
    out
}

pub fn text_actions(text: &str) -> Value {
    let mut actions = Vec::new();
    for cluster in clusters(text) {
        // A line break is typed as Enter; other control characters never get here.
        let value = if cluster == "\n" { "\u{E007}".to_string() } else { cluster };
        actions.push(json!({"type": "keyDown", "value": value}));
        actions.push(json!({"type": "keyUp", "value": value}));
    }
    json!([{"type": "key", "id": "keyboard", "actions": actions}])
}

// ---------------------------------------------------------------- pictures

/// Width and height of a baseline or progressive JPEG, from its frame header. `None` for
/// anything that is not a well-formed JPEG up to that header.
pub fn jpeg_size(data: &[u8]) -> Option<(u16, u16)> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2;
    for _ in 0..256 {
        // Fill bytes before a marker are allowed.
        while *data.get(at)? == 0xFF && *data.get(at + 1)? == 0xFF {
            at += 1;
        }
        if *data.get(at)? != 0xFF {
            return None;
        }
        let marker = *data.get(at + 1)?;
        at += 2;
        match marker {
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            0xD9 | 0xDA => return None, // end of image or scan before any frame header
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]));
        if len < 2 {
            return None;
        }
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let segment = data.get(at..at + len)?;
            if segment.len() < 7 {
                return None;
            }
            let height = u16::from_be_bytes([segment[3], segment[4]]);
            let width = u16::from_be_bytes([segment[5], segment[6]]);
            return (width > 0 && height > 0).then_some((width, height));
        }
        at += len;
    }
    None
}

/// Fuzzing entry (`fuzzing::bidi`): parse a message, take what the driver would take from
/// it, and decide about it as a paused request.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    if let Some(Incoming::Event { method, params }) = parse_incoming(&text) {
        assert!(method.chars().count() <= 64);
        if let Some(p) = paused(&params) {
            assert!(p.request.chars().count() <= 128 && p.url.chars().count() <= 8192);
            let site = super::config::SiteEntry {
                name: Some("chat".into()),
                start: Some("https://chat.example/".into()),
                hosts: Some(vec!["chat.example".into(), "*.cdn.example".into()]),
                ..Default::default()
            }
            .resolve()
            .expect("a valid site");
            if let Verdict::Continue = decide(&p.url, Some(&site), p.api_call, |_| true) {
                let host = request_host(&p.url);
                assert!(is_local(&p.url) || host.as_deref().is_some_and(|h| site.allows(h)), "{:?}", p.url);
            }
        }
    }
    if let Some(Incoming::Error { error, message, .. }) = parse_incoming(&text) {
        assert!(error.chars().count() <= 64 && message.chars().count() <= 200);
    }
    if let Some((w, h)) = jpeg_size(data) {
        assert!(w > 0 && h > 0);
    }
    let _ = clusters(&text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::config::SiteEntry;

    fn site() -> Site {
        SiteEntry {
            name: Some("chat".into()),
            start: Some("https://chat.example/".into()),
            hosts: Some(vec!["chat.example".into(), "*.cdn.example".into()]),
            ..Default::default()
        }
        .resolve()
        .unwrap()
    }

    #[test]
    fn messages_are_read_by_shape_and_nothing_else() {
        assert_eq!(
            parse_incoming(r#"{"type":"success","id":7,"result":{"data":"x"}}"#),
            Some(Incoming::Reply { id: 7, result: json!({"data": "x"}) })
        );
        assert_eq!(
            parse_incoming(r#"{"type":"error","id":3,"error":"no such frame","message":"gone"}"#),
            Some(Incoming::Error { id: Some(3), error: "no such frame".into(), message: "gone".into() })
        );
        let Some(Incoming::Event { method, params }) =
            parse_incoming(r#"{"type":"event","method":"browsingContext.load","params":{"url":"https://a/"}}"#)
        else {
            panic!("an event")
        };
        assert_eq!((method.as_str(), field(&params, "url")), ("browsingContext.load", Some("https://a/")));
        for bad in [
            "",
            "[]",
            "{}",
            r#"{"type":"success"}"#,
            r#"{"type":"success","id":-1}"#,
            r#"{"type":"event"}"#,
            r#"{"type":"other","id":1}"#,
            "{\"type\":\"error\",\"message\":\"\u{202e}x\"}",
        ] {
            let parsed = parse_incoming(bad);
            if let Some(Incoming::Error { message, .. }) = &parsed {
                assert!(!message.contains('\u{202e}'));
            } else {
                assert_eq!(parsed, None, "{bad}");
            }
        }
    }

    #[test]
    fn requests_go_through_only_to_the_open_sites_hosts_and_within_its_budget() {
        let site = site();
        let yes = |_| true;
        assert_eq!(decide("https://chat.example/x", Some(&site), false, yes), Verdict::Continue);
        assert_eq!(decide("wss://gateway.cdn.example/?v=9", Some(&site), true, yes), Verdict::Continue);
        assert_eq!(decide("data:image/png;base64,AAAA", None, false, yes), Verdict::Continue);
        assert_eq!(decide("about:blank", None, false, yes), Verdict::Continue);
        let fail = |url: &str, why: Refusal| {
            let got = decide(url, Some(&site), false, |_| true);
            assert!(matches!(got, Verdict::Fail { why: w, .. } if w == why), "{url}: {got:?}");
        };
        fail("https://evil.example/", Refusal::OtherHost);
        fail("https://chat.example.evil.example/", Refusal::OtherHost);
        fail("https://cdn.example/", Refusal::OtherHost);
        fail("http://chat.example/", Refusal::NotHttps);
        fail("https://chat.example:8443/", Refusal::NotHttps);
        fail("ftp://chat.example/", Refusal::NotHttps);
        fail("https://127.0.0.1/session", Refusal::OtherHost);
        fail("https://127.0.0.1:9222/session", Refusal::NotHttps);
        assert!(matches!(
            decide("https://chat.example/", None, false, yes),
            Verdict::Fail { why: Refusal::NothingOpen, .. }
        ));
        assert!(matches!(
            decide("https://chat.example/", Some(&site), false, |_| false),
            Verdict::Fail { why: Refusal::Budget, host: Some(h) } if h == "chat.example"
        ));
        // An API call needs a token of both budgets; a page's script only of the first.
        let only_requests = |b: Budget| b == Budget::Requests;
        assert!(matches!(
            decide("https://chat.example/api", Some(&site), true, only_requests),
            Verdict::Fail { why: Refusal::ApiBudget, .. }
        ));
        assert_eq!(decide("https://chat.example/app.js", Some(&site), false, only_requests), Verdict::Continue);
        // The budgets are only spent on requests that would go through.
        let mut spent = 0;
        let _ = decide("https://evil.example/", Some(&site), true, |_| {
            spent += 1;
            true
        });
        assert_eq!(spent, 0);
    }

    #[test]
    fn api_calls_are_told_apart_from_what_a_page_loads_to_show_itself() {
        let calls = [
            (Some(""), Some("fetch")),
            (Some(""), Some("xmlhttprequest")),
            (Some(""), None),
            (None, Some("websocket")),
            (Some(""), Some("beacon")),
        ];
        for (destination, initiator) in calls {
            assert!(is_api_call(destination, initiator), "{destination:?} {initiator:?}");
        }
        let shows = [
            (Some("script"), Some("script")),
            (Some("style"), Some("link")),
            (Some("image"), Some("img")),
            (Some("document"), None),
            (Some("font"), Some("css")),
            (Some("worker"), None),
        ];
        for (destination, initiator) in shows {
            assert!(!is_api_call(destination, initiator), "{destination:?} {initiator:?}");
        }
    }

    #[test]
    fn a_paused_request_is_recognised_and_bounded() {
        let event = json!({
            "context": "ctx-1", "isBlocked": true, "navigation": "nav-1", "redirectCount": 0,
            "request": {"request": "8-abc", "url": "https://chat.example/", "method": "GET"}
        });
        assert_eq!(
            paused(&event),
            Some(Paused {
                request: "8-abc".into(),
                url: "https://chat.example/".into(),
                context: Some("ctx-1".into()),
                navigation: true,
                api_call: false,
            })
        );
        let mut not_blocked = event.clone();
        not_blocked["isBlocked"] = json!(false);
        assert_eq!(paused(&not_blocked), None);
        let mut sub = event.clone();
        sub["navigation"] = Value::Null;
        assert!(!paused(&sub).unwrap().navigation);
    }

    #[test]
    fn the_phones_input_becomes_these_actions() {
        assert_eq!(Key::parse("Enter"), Some(Key::Named("Enter", '\u{E007}')));
        assert_eq!(Key::parse("enter"), Some(Key::Named("Enter", '\u{E007}')));
        assert_eq!(Key::parse("ctrl-v"), Some(Key::Ctrl('v')));
        for bad in ["ctrl-w", "ctrl-t", "F12", "ctrl-shift-i", "Meta", "ENTER", "", "ctrl-"] {
            assert_eq!(Key::parse(bad), None, "{bad}");
        }
        let ctrl = key_actions(Key::Ctrl('a'));
        let values: Vec<&str> =
            ctrl[0]["actions"].as_array().unwrap().iter().map(|a| a["value"].as_str().unwrap()).collect();
        assert_eq!(values, ["\u{E009}", "a", "a", "\u{E009}"]);
        let click = pointer_actions(MouseKind::Down, 10, 20, Button::Right);
        assert_eq!(
            click[0]["actions"][0],
            json!({"type": "pointerMove", "x": 10, "y": 20, "origin": "viewport", "duration": 0})
        );
        assert_eq!(click[0]["actions"][1], json!({"type": "pointerDown", "button": 2}));
        let wheel = scroll_actions(5, 6, 0, -120);
        assert_eq!(wheel[0]["actions"][0]["deltaY"], -120);
        assert_eq!(clusters("ab"), ["a", "b"]);
        assert_eq!(clusters("e\u{301}x"), ["e\u{301}", "x"]);
        assert_eq!(clusters("👩\u{200D}💻!"), ["👩\u{200D}💻", "!"]);
        assert_eq!(clusters("👍🏽"), ["👍🏽"]);
        assert_eq!(clusters("🇩🇪🇫🇷"), ["🇩🇪", "🇫🇷"]);
        let typed = text_actions("hé");
        assert_eq!(typed[0]["actions"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn jpeg_sizes_are_read_from_the_frame_header() {
        // SOI, APP0 (16 bytes), SOF0 (17 bytes: 8 bit, 600 high, 800 wide, 3 components).
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpeg.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x02, 0x58, 0x03, 0x20, 0x03]);
        jpeg.extend_from_slice(&[1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        assert_eq!(jpeg_size(&jpeg), Some((800, 600)));
        assert_eq!(jpeg_size(&jpeg[..jpeg.len() - 12]), None, "cut inside the frame header");
        assert_eq!(jpeg_size(b"\x89PNG\r\n"), None);
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]), None, "a scan before any frame header");
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), None, "a length below two");
    }
}
