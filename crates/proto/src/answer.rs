//! Answers to prompts, written by a front end to the launcher's standard input, and the
//! events that ask for them.
//!
//! Besides [`STOP_COMMAND`](crate::STOP_COMMAND), a front end may write one kind of line:
//!
//! ```text
//! answer <kind> <text>\n
//! ```
//!
//! `kind` is [`AnswerKind`]: `signin` (the address a Bedrock sign-in page ended on, see
//! [`SignInPage`]) or `realms` (the code of a Realms join waiting for approval, where the
//! launcher said that such prompts are answered here: [`Event::Answerable`](crate::Event)).
//! `text` is [`CANCEL`] to refuse the prompt instead of answering it.
//!
//! This channel is **trusted input**: only whoever started the launcher can write to its
//! standard input, never the game. That is the whole point of it — an answer given here is
//! one the game could not have given itself — so the grammar is strict and both sides use
//! the one definition in this file: [`answer_line`] to write, [`parse_control_line`] to read.
//!
//! Bounds: the text is printable ASCII (spaces allowed inside, none at either end), at most
//! [`MAX_ANSWER_TEXT`] bytes. A line that is longer than [`MAX_CONTROL_LINE`] is ignored in
//! full: it is never cut down to something shorter that might then be accepted.

use serde::{Deserialize, Serialize};

/// The first word of an answer line.
pub const ANSWER_COMMAND: &str = "answer";

/// The text that refuses a prompt instead of answering it. No answer of any kind can be this
/// word: a sign-in answer is an address, a Realms answer a code of digits.
pub const CANCEL: &str = "cancel";

/// The longest answer text. The longest answer there is, the address a Microsoft sign-in
/// ends on, must fit into 1023 bytes to be read at all by `mcpelauncher-client` (it reads the
/// address with a single 1024-byte read, newline included); this allows four times that, so
/// that a longer address than expected — the "about 2 KB" some estimates give — still
/// arrives and is refused by the launcher *with the reason* rather than lost on the way. It
/// is far below what a pipe holds (64 KiB) and below what a terminal accepts as one line
/// (4095 bytes).
pub const MAX_ANSWER_TEXT: usize = 4096;

/// The longest control line a launcher reads: `answer`, the longest kind, the text, and the
/// spaces between.
pub const MAX_CONTROL_LINE: usize = ANSWER_COMMAND.len() + 1 + 6 + 1 + MAX_ANSWER_TEXT;

/// What a prompt is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerKind {
    /// The address a Bedrock sign-in page ended on ([`SignInPage`]).
    #[serde(rename = "signin")]
    SignIn,
    /// The approval code of a Realms join (`[realms] approve`).
    Realms,
}

impl AnswerKind {
    pub const ALL: [AnswerKind; 2] = [AnswerKind::SignIn, AnswerKind::Realms];

    /// The word in an answer line.
    pub fn as_str(self) -> &'static str {
        match self {
            AnswerKind::SignIn => "signin",
            AnswerKind::Realms => "realms",
        }
    }

    pub fn parse(word: &str) -> Option<AnswerKind> {
        AnswerKind::ALL.into_iter().find(|kind| kind.as_str() == word)
    }

    /// A small index, for tables with one slot per kind.
    pub fn index(self) -> usize {
        match self {
            AnswerKind::SignIn => 0,
            AnswerKind::Realms => 1,
        }
    }
}

/// An answer as the launcher receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The text, checked against the grammar (not yet against the prompt: that is the job of
    /// whoever asked).
    Text(String),
    /// [`CANCEL`]: the prompt is refused.
    Cancel,
}

/// Why an answer text does not follow the grammar.
pub fn check_answer_text(text: &str) -> Result<(), &'static str> {
    if text.is_empty() {
        return Err("the answer is empty");
    }
    if text.len() > MAX_ANSWER_TEXT {
        return Err("the answer is longer than 4096 characters");
    }
    if !text.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return Err("the answer may only contain printable ASCII characters");
    }
    if text.starts_with(' ') || text.ends_with(' ') {
        return Err("the answer starts or ends with a space");
    }
    Ok(())
}

/// The line to write for `answer` (newline included), or why it cannot be written.
pub fn answer_line(kind: AnswerKind, answer: &Answer) -> Result<String, &'static str> {
    let text = match answer {
        Answer::Text(text) if text == CANCEL => return Err("`cancel` cannot be an answer; use Answer::Cancel"),
        Answer::Text(text) => {
            check_answer_text(text)?;
            text.as_str()
        }
        Answer::Cancel => CANCEL,
    };
    Ok(format!("{ANSWER_COMMAND} {} {text}\n", kind.as_str()))
}

/// One line of a front end's control channel, as the launcher understands it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlLine {
    /// [`STOP_COMMAND`](crate::STOP_COMMAND).
    Stop,
    Answer {
        kind: AnswerKind,
        answer: Answer,
    },
    /// An answer line that breaks the grammar, and why. It is ignored.
    Invalid(&'static str),
    /// Anything else. Ignored, as every unknown line always was.
    Other,
}

/// Parse one control line, without its newline. `line` is bytes: what arrives on a pipe need
/// not be UTF-8.
pub fn parse_control_line(line: &[u8]) -> ControlLine {
    if line == crate::STOP_COMMAND.as_bytes() {
        return ControlLine::Stop;
    }
    let Some(rest) = line.strip_prefix(ANSWER_COMMAND.as_bytes()).and_then(|r| r.strip_prefix(b" ")) else {
        return ControlLine::Other;
    };
    if line.len() > MAX_CONTROL_LINE {
        return ControlLine::Invalid("the answer line is too long");
    }
    let Ok(rest) = std::str::from_utf8(rest) else {
        return ControlLine::Invalid("the answer may only contain printable ASCII characters");
    };
    let Some((word, text)) = rest.split_once(' ') else {
        return ControlLine::Invalid("an answer line is `answer <kind> <text>`");
    };
    let Some(kind) = AnswerKind::parse(word) else {
        return ControlLine::Invalid("the kind of an answer is `signin` or `realms`");
    };
    if text == CANCEL {
        return ControlLine::Answer { kind, answer: Answer::Cancel };
    }
    match check_answer_text(text) {
        Ok(()) => ControlLine::Answer { kind, answer: Answer::Text(text.to_string()) },
        Err(why) => ControlLine::Invalid(why),
    }
}

/// Where a sign-in prompt stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInState {
    /// Open: the page can be shown, and the address it ends on is awaited.
    Waiting,
    /// The address was accepted and handed to the game.
    Done,
    /// Refused: by the user, or because the game stopped waiting or the launch ended.
    Cancelled,
    /// Nobody answered in time.
    Expired,
    /// Too many answers that were not the address this sign-in ended on.
    Failed,
}

/// A Microsoft sign-in page the game asked for (Bedrock), to be shown **outside** the game.
///
/// The front end offers to open `url` in the user's browser — only ever a page on
/// `login.live.com`, checked by the launcher before this event is sent — and asks for the
/// address the browser ends on, which starts with `end` (`ms-xal-…://auth?code=…`). It sends
/// that address as `answer signin <address>`, or `answer signin cancel`.
///
/// Sent when the prompt opens, again while it waits whenever an answer was not accepted (with
/// the reason in `note`), and once when it ends. Carries no credential: the page's address
/// holds only public values (the game's PKCE challenge and its state), and the answer —
/// which holds a one-time code that is useless without the game's PKCE verifier — never
/// appears in an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInPage {
    /// The page to open (`https://login.live.com/oauth20_authorize.srf?…`).
    pub url: String,
    /// What the address the page ends on starts with.
    pub end: String,
    /// Seconds until the prompt runs out.
    pub expires_in: u64,
    /// Answers that may still be given before the prompt fails.
    pub attempts_left: u32,
    pub state: SignInState,
    /// While waiting: why the last answer was not accepted. When ended: why it ended, in a
    /// few words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// This session takes the address from the desktop's handler by itself (bastion-mc's handler
    /// for `ms-xal-0000000048183522` links, where it is registered): pasting it is the fallback.
    /// False when another session holds the handler, or there is none. Absent from the line when
    /// false, so front ends from before it read the event as they did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub handback: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_lines_are_written_and_read_by_the_same_grammar() {
        let url = "ms-xal-0000000048183522://auth?code=M.C5_BAY.2.U.x&state=abc";
        for (kind, answer) in [
            (AnswerKind::SignIn, Answer::Text(url.into())),
            (AnswerKind::SignIn, Answer::Cancel),
            (AnswerKind::Realms, Answer::Text("123456".into())),
            (AnswerKind::Realms, Answer::Text("a text with inner spaces".into())),
        ] {
            let line = answer_line(kind, &answer).unwrap();
            assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "{line:?}");
            let parsed = parse_control_line(line.trim_end_matches('\n').as_bytes());
            assert_eq!(parsed, ControlLine::Answer { kind, answer });
        }
        assert_eq!(answer_line(AnswerKind::SignIn, &Answer::Cancel).unwrap(), "answer signin cancel\n");
        assert_eq!(answer_line(AnswerKind::Realms, &Answer::Text("1".into())).unwrap(), "answer realms 1\n");
    }

    #[test]
    fn text_that_breaks_the_grammar_is_never_written() {
        for bad in ["", " 1", "1 ", "a\nanswer realms 1", "tab\there", "é", "\u{1b}[2J", "cancel"] {
            assert!(answer_line(AnswerKind::Realms, &Answer::Text(bad.into())).is_err(), "{bad:?}");
        }
        assert!(answer_line(AnswerKind::SignIn, &Answer::Text("x".repeat(MAX_ANSWER_TEXT))).is_ok());
        assert!(answer_line(AnswerKind::SignIn, &Answer::Text("x".repeat(MAX_ANSWER_TEXT + 1))).is_err());
    }

    #[test]
    fn the_stop_line_is_exactly_what_it_was() {
        assert_eq!(parse_control_line(b"stop"), ControlLine::Stop);
        for other in [&b"stop\r"[..], b" stop", b"stop ", b"STOP", b"stopp", b"not a command", b""] {
            assert_eq!(parse_control_line(other), ControlLine::Other, "{:?}", String::from_utf8_lossy(other));
        }
    }

    #[test]
    fn hostile_or_broken_answer_lines_are_refused_with_a_reason() {
        let invalid = |line: &[u8]| matches!(parse_control_line(line), ControlLine::Invalid(_));
        assert!(invalid(b"answer signin"), "no text");
        assert!(invalid(b"answer signin "), "empty text");
        assert!(invalid(b"answer phone 1"), "unknown kind");
        assert!(invalid(b"answer SignIn x"), "kinds are lower case");
        assert!(invalid(b"answer signin  x"), "leading space");
        assert!(invalid(b"answer signin x "), "trailing space");
        assert!(invalid(b"answer signin x\r"), "carriage return");
        assert!(invalid(b"answer realms \xff\xfe"), "not UTF-8");
        assert!(invalid(b"answer realms caf\xc3\xa9"), "not ASCII");
        assert!(invalid(b"answer realms \x1b]52;c;aGk=\x07"), "terminal escape");
        let long = format!("answer signin {}", "x".repeat(MAX_ANSWER_TEXT + 1));
        assert!(invalid(long.as_bytes()));
        let longest = format!("answer realms {}", "x".repeat(MAX_ANSWER_TEXT));
        assert!(longest.len() <= MAX_CONTROL_LINE);
        assert!(!invalid(longest.as_bytes()));
        // Not an answer line at all: ignored like any unknown line.
        assert_eq!(parse_control_line(b"answerx signin x"), ControlLine::Other);
        assert_eq!(parse_control_line(b"answer"), ControlLine::Other);
    }

    #[test]
    fn kinds_have_stable_words() {
        assert_eq!(serde_json::to_string(&AnswerKind::SignIn).unwrap(), "\"signin\"");
        assert_eq!(serde_json::to_string(&AnswerKind::Realms).unwrap(), "\"realms\"");
        for kind in AnswerKind::ALL {
            assert_eq!(AnswerKind::parse(kind.as_str()), Some(kind));
            assert_eq!(AnswerKind::ALL[kind.index()], kind);
        }
    }
}
