//! The accounts page: one card per local account, the sign-in in progress, and a tile to
//! add a label.
//!
//! An account here is a *label* with its own stored token (`bastion-mc --account`). The
//! cards show what the launcher last said about each (`Event::Accounts`): whether a token
//! is stored, the player name it saw at the last sign-in, and where the token rests. There
//! is no picture of the player in that, and none is invented: the square holds a letter.
//!
//! Every card has the same rows whatever its state, so that a button never moves when the
//! state of its account changes. Cards that stand side by side are the same height
//! ([`Tiles`]) and keep their buttons at the bottom, so the buttons of one line of cards are
//! on the same lines however many lines the texts above them take.

use std::time::Instant;

use iced::{
    Center, Element, Fill, Length,
    widget::{Id, Space, column, container, row, text, text_input},
};

use super::{Layout, kit, tiles::Tiles};
use crate::{
    app::{AccountEntry, App, JobKind, LoginPrompt, Message, is_sign_in_link, is_valid_id},
    backend::fit,
    hud,
    theme::{self, Tone},
};

/// The narrowest a card may get. Its two rows of buttons fit in this with room to spare
/// for a wider font.
const CARD_MIN: f32 = 300.0;
/// The height of a row of buttons on a card.
const ROW_HEIGHT: f32 = 30.0;
/// A label of up to this many characters has its tags beside it.
const LABEL_BESIDE_TAGS: usize = 11;
/// Characters of an account label in the heading of the sign-in panel.
const HEADING_LABEL_CHARS: usize = 20;

pub fn page(app: &App, layout: Layout) -> Element<'_, Message> {
    let mut content = column![].spacing(theme::S4);
    // The OAuth client is one setting for all accounts; any summary can tell.
    let client_missing =
        app.accounts.iter().filter_map(|entry| entry.summary.as_ref()).any(|a| !a.client_id_configured);
    if client_missing {
        content = content.push(kit::panel_with(
            hud::Frame::toned(Tone::Caution),
            kit::label_in("No OAuth client ID is configured", theme::AMBER),
            None,
            kit::small(
                "Set auth.client_id in config.toml to your own Azure application to sign in. \
                 Demo mode works without an account.",
            ),
        ));
    }
    if app.job_kind() == Some(JobKind::SignIn) {
        content = content.push(sign_in(app));
    }

    let known = match app.accounts.len() + app.accounts_hidden {
        1 => "1 local account".to_string(),
        count => format!("{count} local accounts"),
    };
    content = content.push(
        row![
            kit::label(&known),
            Space::new().width(Fill),
            kit::link("Refresh", Tone::Neutral, (!app.busy()).then_some(Message::Refresh))
        ]
        .align_y(Center),
    );

    let columns = (((layout.width + theme::S4) / (CARD_MIN + theme::S4)).floor() as usize).clamp(1, 3);
    let mut tiles: Vec<Element<'_, Message>> = app.accounts.iter().map(|entry| card(app, entry)).collect();
    tiles.push(new_account(app));
    let mut tiles = tiles.into_iter().peekable();
    while tiles.peek().is_some() {
        // A short last line keeps its tiles as wide as those above (see `Tiles`).
        let line: Vec<Element<'_, Message>> = tiles.by_ref().take(columns).collect();
        content = content.push(Tiles::new(line, columns, theme::S4));
    }
    if app.accounts_hidden > 0 {
        content = content.push(kit::small(format!(
            "{} more accounts are not shown here; `bastion-mc account list` prints all of them.",
            app.accounts_hidden
        )));
    }
    content.into()
}

/// The widget id of a control on the card of `label` (tests press the controls by it).
pub fn control_id(action: &str, label: &str) -> Id {
    Id::from(format!("account-{action}-{label}"))
}

fn tagged<'a>(action: &str, label: &str, control: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(control).id(control_id(action, label)).into()
}

fn field<'a>(name: &str, value: Element<'a, Message>) -> Element<'a, Message> {
    row![
        container(kit::label(name))
            .width(Length::Fixed(58.0))
            .padding(iced::Padding { top: 2.0, ..iced::Padding::ZERO }),
        value
    ]
    .spacing(theme::S2)
    .into()
}

fn card<'a>(app: &'a App, entry: &'a AccountEntry) -> Element<'a, Message> {
    let label = entry.label.as_str();
    let idle = !app.busy();
    let in_use = label == app.account_name;
    let is_default = app.default_account.as_deref() == Some(label);
    let summary = entry.summary.as_ref();
    let signing_in = app.job.as_ref().is_some_and(|job| job.kind == JobKind::SignIn && job.subject == label);

    let (state, tone) = match summary {
        None if app.busy() => ("Checking…", Tone::Neutral),
        None => ("Not reported by the launcher yet", Tone::Neutral),
        Some(_) if signing_in => ("Signing in…", Tone::Caution),
        Some(account) if account.signed_in => ("Signed in", Tone::Accent),
        Some(_) => ("Not signed in", Tone::Neutral),
    };
    let player = summary.and_then(|account| account.player.as_deref());

    let mut tags = row![].spacing(theme::S1);
    if in_use {
        tags = tags.push(kit::chip("In use", Tone::Accent));
    }
    if is_default {
        tags = tags.push(kit::chip("Default", Tone::Neutral));
    }
    // A label may be 64 characters long: it wraps rather than being cut off, and the tags
    // then stand under it instead of taking half of its line.
    let long = label.chars().count() > LABEL_BESIDE_TAGS;
    let mut names = column![
        kit::mono(label).size(15).wrapping(text::Wrapping::WordOrGlyph).width(Fill),
        match player {
            Some(player) => kit::small(format!("plays as {player}")),
            None => kit::small("no player name known").color(theme::TEXT_FAINT),
        }
        .width(Fill),
    ]
    .spacing(3)
    .width(Fill);
    let mut head = row![kit::monogram(player.unwrap_or(label), 40.0, tone)].spacing(theme::S3).align_y(Center);
    if long {
        names = names.push(tags);
        head = head.push(names);
    } else {
        head = head.push(names).push(tags);
    }

    let state_color = if tone == Tone::Neutral { theme::TEXT_DIM } else { tone.color() };
    let token = match summary {
        Some(account) => kit::small(account.store.as_str()),
        None => kit::small("—"),
    };
    let facts = column![
        field("State", kit::small(state).color(state_color).width(Fill).into()),
        field("Token", token.width(Fill).into()),
    ]
    .spacing(theme::S1);

    // The second row: which account is used. A control that does not apply leaves its place
    // empty rather than the row, so the rows of all cards are on the same lines.
    let mut choose = row![].spacing(theme::S2).height(Length::Fixed(ROW_HEIGHT)).align_y(Center);
    if !in_use {
        choose = choose.push(tagged(
            "use",
            label,
            kit::action("Switch to", Tone::Accent, idle.then(|| Message::AccountSelected(label.to_string()))),
        ));
    }
    if !is_default {
        choose = choose.push(tagged(
            "default",
            label,
            kit::action("Make default", Tone::Neutral, idle.then(|| Message::MakeDefaultAccount(label.to_string()))),
        ));
    }
    // The first row: the session. Where there is nothing to offer, it says why.
    let mut session = row![].spacing(theme::S2).height(Length::Fixed(ROW_HEIGHT)).align_y(Center);
    let unavailable = |why: &'static str| kit::small(why).color(theme::TEXT_FAINT);
    match summary {
        _ if signing_in => {
            let stopping = app.job.as_ref().is_some_and(|job| job.stopping);
            session = session.push(tagged(
                "cancel",
                label,
                kit::action("Cancel sign-in", Tone::Caution, (!stopping).then_some(Message::Stop)),
            ));
        }
        Some(account) if account.signed_in => {
            session = session.push(tagged(
                "sign-out",
                label,
                kit::action(
                    "Sign out",
                    Tone::Neutral,
                    app.can_sign_out(entry).then(|| Message::SignOut { account: label.to_string() }),
                ),
            ));
        }
        Some(account) if account.client_id_configured => {
            let offered = app.can_sign_in(entry);
            let sign_in = |device_code| offered.then(|| Message::SignIn { account: label.to_string(), device_code });
            session = session.push(tagged(
                "sign-in",
                label,
                kit::solid("Sign in", Tone::Accent, sign_in(account.device_flow)),
            ));
            if !account.device_flow {
                session = session.push(tagged(
                    "device-code",
                    label,
                    kit::action("Device code", Tone::Neutral, sign_in(true)),
                ));
            }
        }
        Some(_) => session = session.push(unavailable("Signing in needs an OAuth client ID.")),
        None if app.busy() => session = session.push(unavailable("Asking the launcher…")),
        None => session = session.push(unavailable("The launcher has not described this account.")),
    }

    let frame = if in_use { hud::Frame::toned(Tone::Accent) } else { hud::Frame::plate() };
    // The card fills the height of its line, and the room it gains goes above the buttons.
    let buttons = container(column![session, choose].spacing(theme::S3)).height(Fill).align_y(iced::Bottom);
    let content = column![head, kit::rule(), facts, buttons].spacing(theme::S3).height(Fill);
    container(kit::framed(frame, container(content).padding(theme::S4).width(Fill).height(Fill)))
        .id(control_id("card", label))
        .into()
}

/// The tile that adds a label.
fn new_account(app: &App) -> Element<'_, Message> {
    let can_add = app.can_add_account();
    let typed = app.new_account.as_str();
    let hint: Option<&str> = if typed.is_empty() {
        None
    } else if !is_valid_id(typed) {
        Some("Labels use a-z, 0-9, '.', '_' and '-', and start with a letter or digit.")
    } else if typed == app.account_name {
        Some("That is the account in use.")
    } else {
        None
    };
    let mut content = column![
        kit::small(
            "A label for another account, for example \"alt\". It gets its own stored token once you sign in to it."
        ),
        row![
            text_input("label", typed)
                .on_input(Message::NewAccountName)
                .on_submit_maybe(can_add.then_some(Message::AddAccount))
                .id(Id::new("new-account"))
                .font(theme::MONO)
                .size(theme::SIZE_SMALL + 1.0)
                .padding([7, 10])
                .style(theme::input)
                .width(Fill),
            kit::action("Add", Tone::Accent, can_add.then_some(Message::AddAccount)),
        ]
        .spacing(theme::S2)
        .align_y(Center),
    ]
    .spacing(theme::S3);
    if let Some(hint) = hint {
        content = content.push(kit::small(hint).color(theme::RED));
    }
    kit::panel("New account", content)
}

// ------------------------------------------------------------------ sign-in

fn countdown(prompt: &LoginPrompt, now: Instant) -> String {
    kit::clock(prompt.expires_at().saturating_duration_since(now).as_secs())
}

/// The sign-in under way: what the user has to do, and the way out of it.
fn sign_in(app: &App) -> Element<'_, Message> {
    let account = app.job.as_ref().map(|job| job.subject.as_str()).unwrap_or_default();
    let stopping = app.job.as_ref().is_some_and(|job| job.stopping);
    let cancel = kit::tool("Cancel", Tone::Neutral, (!stopping).then_some(Message::Stop));
    let body: Element<'_, Message> = match &app.login {
        // Nothing to act on yet: the launcher is talking to Microsoft.
        None => kit::small("Contacting Microsoft…").into(),
        Some(prompt @ LoginPrompt::DeviceCode { user_code, verification_uri, .. }) => {
            let mut buttons = row![
                kit::solid("Copy code", Tone::Accent, Some(Message::Copy(user_code.clone()))),
                kit::action("Copy link", Tone::Neutral, Some(Message::Copy(verification_uri.clone()))),
            ]
            .spacing(theme::S2);
            // Only a Microsoft sign-in address gets a button that opens it.
            if is_sign_in_link(verification_uri) {
                buttons = buttons.push(kit::action(
                    "Open link",
                    Tone::Neutral,
                    Some(Message::OpenLink(verification_uri.clone())),
                ));
            }
            column![
                kit::body("Open this page in any browser and enter the code:"),
                kit::mono(verification_uri.as_str()).size(theme::SIZE_BODY).color(theme::ACCENT),
                // A code is nine characters and stands on one line; one that is longer than
                // its place is wide goes on to a second line rather than out of the frame.
                kit::framed(
                    hud::Frame::brackets(theme::ACCENT),
                    container(kit::mono(user_code.as_str()).size(theme::SIZE_DISPLAY)).padding([10, 22]),
                ),
                buttons.wrap().vertical_spacing(theme::S2),
                kit::small(format!("The code expires in {}", countdown(prompt, app.now))),
            ]
            .spacing(theme::S3)
            .into()
        }
        Some(prompt @ LoginPrompt::Browser { url, .. }) => {
            let mut buttons = row![].spacing(theme::S2);
            if is_sign_in_link(url) {
                buttons =
                    buttons.push(kit::solid("Open sign-in page", Tone::Accent, Some(Message::OpenLink(url.clone()))));
            }
            buttons = buttons.push(kit::action("Copy link", Tone::Neutral, Some(Message::Copy(url.clone()))));
            column![
                kit::body("Sign in with Microsoft in your browser. This window continues by itself once you are done."),
                buttons.wrap().vertical_spacing(theme::S2),
                kit::small(format!("Waiting for the browser, {} left", countdown(prompt, app.now))),
            ]
            .spacing(theme::S3)
            .into()
        }
    };
    // The heading is one line: it has the beginning of a long label, the card has all of it.
    let whose = kit::mono(fit(account, HEADING_LABEL_CHARS)).wrapping(text::Wrapping::None);
    kit::panel_with(
        hud::Frame::toned(Tone::Accent),
        row![kit::label_in("Sign-in", theme::ACCENT), whose].spacing(theme::S3).align_y(Center),
        Some(cancel.into()),
        body,
    )
}
