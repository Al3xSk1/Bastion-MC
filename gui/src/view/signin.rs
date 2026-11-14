//! The Bedrock sign-in prompt: a Microsoft sign-in page the game asked for, shown outside the
//! game.
//!
//! The launcher sends the page's address (only ever on `login.live.com`, for Minecraft's own
//! client — it checked) and waits for the address the page ends on. This panel offers to open
//! the page in the user's browser, takes the final address pasted back, and writes it to the
//! launcher as an answer (`answer signin …`); or refuses the sign-in. The game never sees the
//! page, the password or the browser's cookies — only that final address, which the launcher
//! checks again before handing it over.
//!
//! The address the page ends on belongs to the game (`ms-xal-…://auth?code=…`). Where bastion-mc's
//! handler for that scheme is registered (the Arch package registers it) and this session holds
//! it (`SignInPage::handback`), the browser hands the address back by itself and the panel says
//! so. Otherwise — and as the fallback if nothing happens — the user pastes it: the browser does
//! not show it (Firefox stays on the page before, with nothing in the address bar or the
//! console); its developer tools' Network panel lists the failed redirect with the address in its
//! `Location` header, which is where the panel sends the user.

use std::time::Instant;

use bastion_mc_proto::{SignInPage, SignInState};
use iced::{
    Center, Element, Fill,
    widget::{Id, column, row, text_input},
};

use super::kit;
use crate::{
    app::{App, Message, is_sign_in_link},
    hud,
    theme::{self, Tone},
};

/// The paste field's identity: focused when a prompt opens.
pub const ANSWER_INPUT: &str = "signin-answer";

/// The caption of the control that opens the page in the user's browser.
pub const OPEN_PAGE: &str = "Open the sign-in page";

/// What the panel says when the session takes the address from the desktop's handler.
pub const HANDS_BACK: &str = "When you have signed in, the page hands the result back to bastion-mc by itself, where \
                              bastion-mc's handler for these links is registered (the Arch package registers it; your \
                              browser may ask once whether to open such links with bastion-mc: allow it).";

/// The prompt as the window keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInPrompt {
    /// The launcher's event, made printable and bounded.
    pub event: SignInPage,
    /// When it runs out on the window's clock.
    pub expires_at: Instant,
    /// What is typed (pasted) in the field.
    pub draft: String,
    /// An answer was written and the launcher's verdict is awaited.
    pub sent: bool,
}

impl SignInPrompt {
    pub fn waiting(&self) -> bool {
        self.event.state == SignInState::Waiting
    }

    /// Why the draft cannot be sent as it is, if it cannot: checked here only for immediate
    /// feedback (an answer that is plainly wrong would cost one of the prompt's attempts).
    /// The launcher has the final word.
    pub fn draft_problem(&self) -> Option<&'static str> {
        let draft = self.draft.trim();
        if draft.is_empty() {
            return None;
        }
        if bastion_mc_proto::answer::check_answer_text(draft).is_err() {
            return Some("That is not an address: it holds characters an address cannot hold.");
        }
        if !draft.starts_with(&format!("{}?", self.event.end)) {
            return Some(
                "The address to paste starts with ms-xal-0000000048183522://auth? — copy the one the browser ended on.",
            );
        }
        None
    }

    pub fn can_send(&self) -> bool {
        self.waiting() && !self.sent && !self.draft.trim().is_empty() && self.draft_problem().is_none()
    }
}

/// The panel, while a prompt waits. (An ended one leaves the page: the launch's words say how
/// it ended.)
pub fn prompt(app: &App) -> Option<Element<'_, Message>> {
    let prompt = app.signin.as_ref().filter(|p| p.waiting())?;
    let event = &prompt.event;
    let mut buttons = row![].spacing(theme::S2);
    // Only a Microsoft sign-in page gets a control that opens it.
    if is_sign_in_link(&event.url) {
        buttons = buttons.push(kit::solid(OPEN_PAGE, Tone::Accent, Some(Message::OpenLink(event.url.clone()))));
    }
    buttons = buttons.push(kit::action("Copy link", Tone::Neutral, Some(Message::Copy(event.url.clone()))));
    let field = text_input("ms-xal-0000000048183522://auth?code=…", &prompt.draft)
        .on_input_maybe((!prompt.sent).then_some(Message::SignInAnswerChanged))
        .on_submit_maybe(prompt.can_send().then_some(Message::SendSignInAnswer))
        .id(Id::new(ANSWER_INPUT))
        .font(theme::MONO)
        .size(theme::SIZE_SMALL + 1.0)
        .padding([7, 10])
        .style(theme::input)
        .width(Fill);
    let steps = "Before the last step, open the browser's developer tools (F12) on the Network tab; afterwards \
                 select the last request (it failed, status 302), copy the Location header of its response, and paste \
                 it here.";
    let mut parts = column![
        kit::body(
            "The game asks you to sign in to your Microsoft account. The page is Microsoft's own, on \
             login.live.com, and opens in your browser: the game never sees your password."
        ),
        buttons.wrap().vertical_spacing(theme::S2),
    ]
    .spacing(theme::S3);
    parts = if event.handback {
        parts.push(kit::body(HANDS_BACK)).push(kit::small(format!(
            "If nothing happens, paste the address the sign-in ended on (ms-xal-0000000048183522://auth?code=…); \
             your browser does not show it. {steps}"
        )))
    } else {
        parts.push(kit::small(format!(
            "The sign-in ends on an address your browser cannot open and does not show \
             (ms-xal-0000000048183522://auth?code=…). {steps}"
        )))
    };
    let mut parts = parts.push(
        row![
            field,
            kit::action("Send", Tone::Accent, prompt.can_send().then_some(Message::SendSignInAnswer)),
            kit::tool("Cancel", Tone::Neutral, (!prompt.sent).then_some(Message::CancelSignIn)),
        ]
        .spacing(theme::S2)
        .align_y(Center),
    );
    if let Some(problem) = prompt.draft_problem() {
        parts = parts.push(kit::small(problem).color(theme::RED));
    } else if prompt.sent {
        parts = parts.push(kit::small("Sent. The launcher is checking it…"));
    } else if let Some(note) = &event.note {
        parts = parts.push(kit::small(format!("Not accepted: {note}.")).color(theme::RED));
    }
    let left = kit::clock(prompt.expires_at.saturating_duration_since(app.now).as_secs());
    let parts = parts.push(kit::small(format!(
        "{left} left, {} attempt(s). Only the address the sign-in ended on goes to the game. If you did not ask to sign in, \
         press Cancel.",
        event.attempts_left
    )));
    Some(kit::panel_with(
        hud::Frame::toned(Tone::Accent),
        kit::label_in("Microsoft sign-in for Bedrock", theme::ACCENT),
        None,
        parts,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(draft: &str) -> SignInPrompt {
        SignInPrompt {
            event: SignInPage {
                url: "https://login.live.com/oauth20_authorize.srf?client_id=0000000048183522".into(),
                end: "ms-xal-0000000048183522://auth".into(),
                expires_in: 900,
                attempts_left: 10,
                state: SignInState::Waiting,
                note: None,
                handback: false,
            },
            expires_at: Instant::now(),
            draft: draft.into(),
            sent: false,
        }
    }

    #[test]
    fn a_draft_is_sendable_only_when_it_looks_like_the_final_address() {
        assert!(!prompt("").can_send());
        assert!(prompt("").draft_problem().is_none(), "an empty field is not an error yet");
        assert!(prompt("  ms-xal-0000000048183522://auth?code=a&state=b  ").can_send(), "pasted with spaces");
        assert!(prompt("https://login.live.com/").draft_problem().unwrap().contains("starts with"));
        assert!(prompt("ms-xal-0000000048183522://auth").draft_problem().is_some(), "no query");
        assert!(prompt("ms-xal-0000000048183522://auth?code=\u{e9}").draft_problem().unwrap().contains("characters"));
        let mut sent = prompt("ms-xal-0000000048183522://auth?code=a&state=b");
        sent.sent = true;
        assert!(!sent.can_send(), "one answer at a time");
    }
}
