//! The approval prompt of a Realms join, shown outside the game (`[realms] approve`).
//!
//! The launcher sends it as an event when a join waits: the Realm, the server the game would
//! connect to, and — in `code` mode — the six-digit code the user types into the phone's Realms
//! app in the game. This is the only place, besides the launcher's own terminal output, where
//! that code appears; the game never sees it. When the prompt ends the panel says how, until
//! the launch is over.

use std::time::Instant;

use bastion_mc_proto::{ApprovalState, RealmsApproval, RealmsMode};
use iced::{
    Element,
    widget::{column, container},
};

use super::kit;
use crate::{
    app::{App, Message},
    hud,
    theme::{self, Tone},
};

/// The prompt as the window keeps it: the event, and when it runs out on the window's clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmsPrompt {
    pub event: RealmsApproval,
    pub expires_at: Instant,
}

/// A six-digit code in two groups of three, as it is easiest to read and type.
fn grouped(code: &str) -> String {
    if code.len() == 6 && code.is_ascii() { format!("{} {}", &code[..3], &code[3..]) } else { code.to_string() }
}

/// The panel, while there is something to show.
pub fn prompt(app: &App) -> Option<Element<'_, Message>> {
    let prompt = app.realms.as_ref()?;
    let event = &prompt.event;
    let place = column![
        kit::label("Realm"),
        kit::body(event.world.as_str()),
        kit::label("Server"),
        kit::mono(event.address.as_str()),
    ]
    .spacing(theme::S1);
    let body: Element<'_, Message> = match event.state {
        ApprovalState::Waiting => {
            let left = kit::clock(prompt.expires_at.saturating_duration_since(app.now).as_secs());
            let mut parts = column![place].spacing(theme::S3);
            match (&event.code, event.mode) {
                (Some(code), _) => {
                    parts = parts.push(kit::body("To join, type this code into the phone's Realms app in the game:"));
                    parts = parts.push(kit::framed(
                        hud::Frame::brackets(theme::ACCENT),
                        container(kit::mono(grouped(code)).size(theme::SIZE_DISPLAY)).padding([10, 22]),
                    ));
                }
                (None, RealmsMode::Totp) | (None, RealmsMode::Code) => {
                    parts = parts.push(kit::body(
                        "To join, type the code your authenticator app shows now into the phone's Realms app in the game.",
                    ));
                }
            }
            parts
                .push(kit::small(format!(
                    "{left} left, {} attempt(s). If you did not ask to join this Realm, do nothing: it is refused.",
                    event.attempts_left
                )))
                .into()
        }
        ApprovalState::Approved => column![place, kit::chip("Approved", Tone::Accent)].spacing(theme::S3).into(),
        ApprovalState::Refused => column![place, kit::chip("Refused", Tone::Caution)].spacing(theme::S3).into(),
        ApprovalState::Expired => {
            column![place, kit::chip("Not approved in time", Tone::Caution)].spacing(theme::S3).into()
        }
    };
    Some(kit::panel("Realms join", body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_shown_in_two_groups() {
        assert_eq!(grouped("123456"), "123 456");
        assert_eq!(grouped("12345"), "12345");
        assert_eq!(grouped("12345é"), "12345é");
    }
}
