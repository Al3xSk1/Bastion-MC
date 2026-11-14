//! The contract between the launcher and anything that drives it.
//!
//! A front end never links the launcher. It starts `bastion-mc --events <command …>` and
//! reads one JSON object per line from the child's standard output; each line is an
//! [`Event`]. The only thing it can say back is [`STOP_COMMAND`] on the child's standard
//! input (closing that pipe means the same). That is the whole interface: no token, key or
//! credential ever crosses it, in either direction.
//!
//! Compatibility: events carry a `"event"` tag. A front end must ignore tags it does not
//! know ([`Event::Unknown`]) and should check [`Event::Hello`] before trusting the rest.

pub mod runbook;

use serde::{Deserialize, Serialize};

/// Bumped whenever an existing event changes meaning. New events do not need a bump.
pub const PROTOCOL: u32 = 1;

/// The global command-line switch that selects this protocol. It must be the first argument.
pub const EVENTS_FLAG: &str = "--events";

/// Written (followed by a newline) to the launcher's standard input to end the current
/// command: a running game is shut down gracefully, a pending sign-in is abandoned.
pub const STOP_COMMAND: &str = "stop";

/// Longest line a reader needs to accept. The launcher never emits more.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Always the first event of a command.
    Hello {
        protocol: u32,
        version: String,
    },
    /// A human-readable progress note from the launcher itself.
    Status {
        text: String,
    },
    /// Where a launch currently is.
    Phase {
        phase: Phase,
    },
    /// Measurable work (downloads). `done == total` is sent exactly once, at the end.
    Progress {
        label: String,
        done: u64,
        total: u64,
        unit: Unit,
    },
    /// Device-code sign-in: show `user_code`, send the user to `verification_uri`.
    DeviceCode {
        user_code: String,
        verification_uri: String,
        expires_in: u64,
    },
    /// Browser sign-in: the user has to open `url`; the launcher waits on a loopback port.
    BrowserLogin {
        url: String,
        timeout_secs: u64,
    },
    /// Sign-in state of one account. Never contains a credential.
    Account(Account),
    /// Every local account, and which one is used when none is named.
    Accounts {
        default: String,
        accounts: Vec<Account>,
    },
    Instances {
        instances: Vec<Instance>,
    },
    Versions {
        latest_release: String,
        latest_snapshot: String,
        versions: Vec<Version>,
    },
    /// Which protections this launch runs with. Sent once, just before the game starts.
    Protection {
        items: Vec<Protection>,
    },
    /// One sanitised line of output (printable characters only, bounded length).
    Log {
        source: Source,
        line: String,
    },
    /// The game process ended. `code` follows shell conventions (130 = stopped on request).
    Exited {
        code: i32,
    },
    /// The command failed; `code` is the launcher's exit status. Always the last event.
    Error {
        message: String,
        code: i32,
    },
    /// An event this build does not know. Skip it.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Resolving, downloading and hashing game files.
    Preparing,
    /// Re-hashing every file from the descriptor that will be mounted.
    Verifying,
    Authenticating,
    /// Building and entering the sandbox.
    Starting,
    Running,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Bytes,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Launcher,
    Game,
    /// The egress relay: which connections were allowed or denied.
    Egress,
}

impl Source {
    pub fn tag(self) -> &'static str {
        match self {
            Source::Launcher => "launcher",
            Source::Game => "game",
            Source::Egress => "egress",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Local account label (`--account`), not a Microsoft identity.
    pub account: String,
    /// A refresh token is stored for this account.
    pub signed_in: bool,
    /// Player name seen at the last successful sign-in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<String>,
    /// Where the refresh token rests ("kernel keyring …").
    pub store: String,
    /// `auth.client_id` is set. Without it only demo mode works.
    pub client_id_configured: bool,
    /// The configured default sign-in flow is the device-code flow.
    pub device_flow: bool,
}

/// How well one aspect of a launch is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shield {
    /// The protection is in force.
    On,
    /// Weaker than it could be on this machine or with this configuration.
    Partial,
    /// Not protected: the user accepted a risk, or the machine cannot do it.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protection {
    /// `display`, `network`, `credential`, `limits` or `launcher`.
    pub aspect: String,
    pub shield: Shield,
    /// A few words: what is in force.
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minecraft: Option<String>,
    /// "vanilla" or "curated".
    pub kind: String,
    /// For curated instances: serial and signer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Set when the instance cannot be launched (bad signature, missing version, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub id: String,
    /// "release", "snapshot", "old_beta", "old_alpha".
    pub kind: String,
    /// ISO date (`YYYY-MM-DD`).
    pub released: String,
}

impl Event {
    /// One line of the stream, including the trailing newline.
    pub fn to_line(&self) -> String {
        // Serialising these types cannot fail: no maps with non-string keys, no floats.
        let mut line = serde_json::to_string(self).unwrap_or_else(|_| r#"{"event":"unknown"}"#.to_string());
        line.push('\n');
        line
    }

    /// Parse one line. `None` means "not an event" (stray text, a truncated line): the
    /// caller decides whether to show it as plain output.
    pub fn parse_line(line: &str) -> Option<Event> {
        let line = line.trim_end_matches(['\n', '\r']);
        if line.len() > MAX_LINE_BYTES || !line.starts_with('{') {
            return None;
        }
        serde_json::from_str(line).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_as_single_lines() {
        let events = vec![
            Event::Hello { protocol: PROTOCOL, version: "0.1.0".into() },
            Event::Status { text: "resolving \"26.3\"\n\ttab".into() },
            Event::Phase { phase: Phase::Authenticating },
            Event::Progress { label: "assets".into(), done: 5, total: 10, unit: Unit::Bytes },
            Event::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://www.microsoft.com/link".into(),
                expires_in: 900,
            },
            Event::BrowserLogin { url: "https://login.microsoftonline.com/x?a=b&c=d".into(), timeout_secs: 300 },
            Event::Account(Account { account: "default".into(), signed_in: true, ..Default::default() }),
            Event::Accounts {
                default: "work".into(),
                accounts: vec![Account { account: "work".into(), ..Default::default() }],
            },
            Event::Instances {
                instances: vec![Instance { id: "a".into(), kind: "vanilla".into(), ..Default::default() }],
            },
            Event::Versions { latest_release: "26.3".into(), latest_snapshot: "26.4-s1".into(), versions: vec![] },
            Event::Protection {
                items: vec![Protection {
                    aspect: "display".into(),
                    shield: Shield::Partial,
                    detail: "session socket".into(),
                }],
            },
            Event::Log { source: Source::Game, line: "[12:00:00] [Render thread/INFO]: hi \u{1F600}".into() },
            Event::Exited { code: 130 },
            Event::Error { message: "nope".into(), code: 4 },
        ];
        for event in events {
            let line = event.to_line();
            assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "{line:?}");
            assert_eq!(Event::parse_line(&line), Some(event));
        }
    }

    #[test]
    fn the_wire_format_is_stable() {
        assert_eq!(Event::Phase { phase: Phase::Running }.to_line(), "{\"event\":\"phase\",\"phase\":\"running\"}\n");
        assert_eq!(
            Event::Log { source: Source::Egress, line: "x".into() }.to_line(),
            "{\"event\":\"log\",\"source\":\"egress\",\"line\":\"x\"}\n"
        );
        let account = Event::Account(Account {
            account: "default".into(),
            signed_in: false,
            player: None,
            store: "kernel keyring".into(),
            client_id_configured: false,
            device_flow: false,
        });
        assert_eq!(
            account.to_line(),
            "{\"event\":\"account\",\"account\":\"default\",\"signed_in\":false,\"store\":\"kernel keyring\",\
             \"client_id_configured\":false,\"device_flow\":false}\n"
        );
    }

    #[test]
    fn unknown_and_malformed_input() {
        assert_eq!(Event::parse_line(r#"{"event":"from_the_future","x":1}"#), Some(Event::Unknown));
        assert_eq!(Event::parse_line("plain text from a stray println"), None);
        assert_eq!(Event::parse_line(r#"{"event":"status"}"#), None, "missing field");
        assert_eq!(Event::parse_line(r#"{"event":"exited","code":"zero"}"#), None, "wrong type");
        assert_eq!(Event::parse_line(""), None);
        let huge = format!(r#"{{"event":"status","text":"{}"}}"#, "x".repeat(MAX_LINE_BYTES));
        assert_eq!(Event::parse_line(&huge), None, "over-long lines are not parsed");
    }
}
