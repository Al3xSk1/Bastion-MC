//! The security page: the emergency panel — the two-step scram and what it reported — then
//! what the last launch ran with, in words, the recovery pages and the runbook.
//!
//! Two things on this page can do something outside the window, and both are fenced:
//!
//! * **Open** exists only next to the entries of [`runbook::LINKS`], a list compiled into
//!   the program; [`Message::OpenRunbookLink`] is checked against that same list again
//!   before anything is opened. No other text on the page — the runbook, a scram report —
//!   gets an "open" control, whatever it contains.
//! * **Scram** needs two different buttons in sequence, and reports its result in the
//!   launcher's own lines. "Complete" is said only when the launcher said so.
//!
//! The emergency panel comes first: it is what the page is opened for in a hurry, and what
//! an incomplete scram reported must not be below the fold.

use bastion_mc_proto::{Shield, runbook};
use iced::{
    Center, Element, Fill, FillPortion, Padding,
    widget::{Id, Space, column, container, row, text},
};

use super::{Layout, kit};
use crate::{
    app::{App, Message, ScramOutcome, ScramState},
    backend::SCRAM_WAIT_LIMIT,
    hud,
    runbook::{Block, SCRAM},
    theme::{self, Tone},
};

/// From this width of the page on there are two columns: what can be done on the left,
/// what there is to read on the right.
const WIDE: f32 = 900.0;

pub fn page(app: &App, layout: Layout) -> Element<'_, Message> {
    if layout.width >= WIDE {
        row![
            column![emergency(app), links()].spacing(theme::S4).width(FillPortion(1)),
            column![last_launch(app), runbook_text()].spacing(theme::S4).width(FillPortion(1)),
        ]
        .spacing(theme::S4)
        .into()
    } else {
        column![emergency(app), last_launch(app), links(), runbook_text()].spacing(theme::S4).into()
    }
}

// ------------------------------------------------------------------ the last launch

fn last_launch(app: &App) -> Element<'_, Message> {
    let running = app.launch().is_some();
    let lead = match (app.protection.is_empty(), running) {
        (true, true) => {
            "The launch under way has not reported yet: the read-outs arrive once the launcher has checked the game it \
             started."
        }
        (true, false) => {
            "No game has been started in this window yet. A launch reports these five things (a Bedrock launch a \
             sixth: its sign-in pages):"
        }
        (false, true) => "The game that is running was started with:",
        (false, false) => "The last game was started with:",
    };
    let mut rows = column![kit::body(lead).size(13)].spacing(theme::S3);
    let entry = |name: &str, level: Option<Shield>, said: Option<&str>| {
        let cell = |words: &str, color| {
            text(theme::caps(words)).size(theme::SIZE_LABEL).color(color).wrapping(text::Wrapping::WordOrGlyph)
        };
        let mut words = column![
            row![cell(name, theme::TEXT), cell(kit::shield_words(level), kit::shield_ink(level))]
                .spacing(theme::S3)
                .align_y(Center)
                .wrap()
        ]
        .spacing(3)
        .width(Fill);
        if let Some(said) = said {
            words = words.push(kit::body(said.to_string()).size(13).width(Fill));
        }
        if let Some(meaning) = kit::aspect_meaning(name) {
            words = words.push(kit::small(format!("Covers {meaning}.")).color(theme::TEXT_FAINT).width(Fill));
        }
        row![container(kit::shield(level)).padding(Padding { top: 1.0, ..Padding::ZERO }), words]
            .spacing(theme::S3)
            .width(Fill)
    };
    if app.protection.is_empty() {
        for (name, _) in kit::ASPECTS {
            rows = rows.push(entry(name, None, None));
        }
    } else {
        for item in &app.protection {
            rows = rows.push(entry(&item.aspect, Some(item.shield), Some(item.detail.as_str())));
        }
    }
    // The three marks, once, with what each means.
    let legend = |level: Shield, meaning: &'static str| {
        row![kit::shield(Some(level)), kit::small(meaning).width(Fill)].spacing(theme::S2).align_y(Center)
    };
    rows = rows.push(kit::rule()).push(
        column![
            legend(Shield::On, "In force: the protection is active."),
            legend(Shield::Partial, "Reduced: weaker than it could be on this machine or with this configuration."),
            legend(
                Shield::Off,
                "Not protected: a risk you accepted in the configuration, or this machine cannot do it."
            ),
        ]
        .spacing(theme::S1),
    );
    kit::panel("Protection of the last launch", rows)
}

// ------------------------------------------------------------------ the emergency stop

/// The widget id of the scram control in its current form (ask, confirm, cancel).
pub fn scram_id(step: &'static str) -> Id {
    Id::new(match step {
        "ask" => "scram-ask",
        "confirm" => "scram-confirm",
        _ => "scram-cancel",
    })
}

fn scram_control(app: &App) -> Element<'_, Message> {
    let state = &app.emergency;
    let elapsed = app.scram_time().map(|time| kit::clock(time.as_secs())).unwrap_or_default();
    match state.state {
        ScramState::StoppingSignIn => column![
            kit::body("Ending the sign-in in progress first, so that it cannot store a new token afterwards…")
                .color(theme::AMBER),
            kit::small(format!("Running for {elapsed}. The scram starts the moment it is gone.")),
        ]
        .spacing(theme::S1)
        .into(),
        ScramState::Running => column![
            kit::body(format!("Removing stored sign-ins… still running, {elapsed}")).color(theme::AMBER),
            kit::small(format!(
                "It is not interrupted, and closing this window does not stop it. What it reports appears \
                 below as it goes. Should it not have ended after {} minutes, this window stops waiting and \
                 says so; the launcher is left to finish even then.",
                SCRAM_WAIT_LIMIT.as_secs() / 60
            )),
        ]
        .spacing(theme::S1)
        .into(),
        ScramState::Idle if state.confirming => row![
            kit::body("Stop every running game and remove all stored sign-ins on this machine?").width(Fill),
            container(kit::solid("Yes, scram", Tone::Danger, Some(Message::ConfirmScram))).id(scram_id("confirm")),
            container(kit::action("Cancel", Tone::Neutral, Some(Message::CancelScram))).id(scram_id("cancel")),
        ]
        .spacing(theme::S2)
        .align_y(Center)
        .into(),
        ScramState::Idle => row![
            column![
                kit::body("Scram: remove everything this launcher stores about your sign-ins"),
                kit::small(
                    "Stops running games; deletes refresh tokens, vault files, the Spotify token, the Realms TOTP secret and the phone browser's profile. Revokes nothing at Microsoft."
                ),
            ]
            .spacing(3)
            .width(Fill),
            container(kit::action("Scram…", Tone::Danger, Some(Message::AskScram))).id(scram_id("ask")),
        ]
        .spacing(theme::S2)
        .align_y(Center)
        .into(),
    }
}

/// How the last scram ended. An incomplete one is said in so many words, in red, with the
/// lines that name what is still there set apart from the rest of the report.
fn scram_outcome(app: &App) -> Option<Element<'_, Message>> {
    fn block<'a>(tone: Tone, title: &str, body: Element<'a, Message>) -> Element<'a, Message> {
        container(column![kit::label_in(title, tone.color()).size(13), body].spacing(theme::S2))
            .padding(theme::S3)
            .width(Fill)
            .style(theme::chip(tone))
            .into()
    }
    let state = &app.emergency;
    Some(match state.outcome.as_ref()? {
        ScramOutcome::Complete => block(
            Tone::Accent,
            "Scram complete",
            kit::small(
                "The launcher removed what it had stored and named nothing it could not remove. Nothing is \
                 revoked at Microsoft: continue with phase 2 of the runbook, from another device if this one \
                 may be compromised.",
            )
            .color(theme::TEXT)
            .into(),
        ),
        ScramOutcome::Incomplete(why) => {
            let mut body =
                column![kit::body(format!("Not everything is confirmed removed: {why}.")).size(13)].spacing(theme::S2);
            let mut failed = column![].spacing(2);
            for line in state.failed_lines() {
                failed = failed.push(
                    text(line.text.as_str())
                        .size(theme::SIZE_SMALL + 1.0)
                        .font(theme::MONO)
                        .color(theme::RED)
                        .wrapping(text::Wrapping::WordOrGlyph),
                );
            }
            if state.failures > 0 {
                body = body.push(kit::well(failed));
            }
            body = body.push(
                kit::small(
                    "Sign-ins may still be stored on this machine. Run `bastion-mc scram` in a terminal to try \
                     again, and do the account steps of the runbook now, from another device.",
                )
                .color(theme::TEXT),
            );
            block(Tone::Danger, "Scram incomplete", body.into())
        }
        ScramOutcome::NotRun(why) => block(
            Tone::Danger,
            "Scram did not run",
            kit::body(format!("{why}. Nothing was removed. Run `bastion-mc scram` in a terminal.")).size(13).into(),
        ),
    })
}

fn emergency(app: &App) -> Element<'_, Message> {
    let state = &app.emergency;
    let mut content = column![
        kit::body(
            "If this computer itself may be compromised, do the account steps from another device — \
             a compromised machine can show you fake pages."
        )
        .size(13)
        .color(theme::AMBER),
        scram_control(app),
    ]
    .spacing(theme::S3);
    if let Some(outcome) = scram_outcome(app) {
        content = content.push(outcome);
    }
    if !state.lines.is_empty() {
        let mut report = column![].spacing(2);
        for line in &state.lines {
            report = report.push(
                text(line.text.as_str())
                    .size(theme::SIZE_SMALL)
                    .font(theme::MONO)
                    .color(if line.failed { theme::RED } else { theme::TEXT_DIM })
                    .wrapping(text::Wrapping::WordOrGlyph),
            );
        }
        if state.omitted > 0 {
            report = report.push(kit::small(format!("… and {} more lines, in the console", state.omitted)));
        }
        content = content.push(column![kit::label("What the launcher reported"), kit::well(report)].spacing(theme::S1));
    }
    kit::panel_with(
        hud::Frame::toned(Tone::Danger),
        kit::label_in("Emergency — account compromised?", theme::RED),
        None,
        content,
    )
}

// ------------------------------------------------------------------ the runbook

/// The widget id of the button that opens the `index`-th recovery page.
pub fn open_id(index: usize) -> Id {
    Id::from(format!("runbook-open-{index}"))
}

fn links<'a>() -> Element<'a, Message> {
    let mut list = column![kit::small(
        "These pages come from Microsoft's own help articles; besides them the window opens only the \
         sign-in pages a sign-in asks for. Each opens in your browser; copy the address to use another device."
    )]
    .spacing(theme::S3);
    for (index, (link, what)) in runbook::LINKS.iter().enumerate() {
        list = list.push(
            row![
                column![
                    kit::body(*what).size(13),
                    kit::mono(*link)
                        .color(theme::TEXT_FAINT)
                        .size(theme::SIZE_LABEL)
                        .wrapping(text::Wrapping::WordOrGlyph)
                ]
                .spacing(2)
                .width(Fill),
                container(kit::action("Open", Tone::Neutral, Some(Message::OpenRunbookLink((*link).to_string()))))
                    .id(open_id(index)),
                kit::link("Copy", Tone::Neutral, Some(Message::Copy((*link).to_string()))),
            ]
            .spacing(theme::S2)
            .align_y(Center),
        );
    }
    kit::panel("Recovery pages", list)
}

/// How far a list item stands in from the one above it; also the width its marker has.
const INDENT: f32 = 20.0;

/// The runbook, laid out from its parts ([`crate::runbook`]). Its addresses are text here
/// and nothing else: the controls that open them are the ones in [`links`].
fn runbook_text<'a>() -> Element<'a, Message> {
    let prose = |words: &'static str| kit::body(words).size(13).width(Fill);
    let mut page = column![].spacing(theme::S2);
    for block in SCRAM.iter() {
        let element: Element<'a, Message> = match block {
            Block::Title(title) => kit::body(title.as_str()).size(15).width(Fill).into(),
            Block::Heading(heading) => column![
                Space::new().height(theme::S1),
                kit::body(heading.as_str()).color(theme::ACCENT).width(Fill),
                kit::rule()
            ]
            .spacing(theme::S1)
            .into(),
            Block::Text { level, marker: Some(marker), text: words } => row![
                Space::new().width(INDENT * f32::from(level.saturating_sub(1))),
                text(marker.as_str()).size(13).color(theme::TEXT_FAINT).width(INDENT).wrapping(text::Wrapping::None),
                prose(words)
            ]
            .into(),
            Block::Text { level, marker: None, text: words } => {
                row![Space::new().width(INDENT * f32::from(*level)), prose(words)].into()
            }
            Block::Link { level, url } => row![
                Space::new().width(INDENT * f32::from(*level)),
                kit::mono(url.as_str())
                    .color(theme::TEXT_DIM)
                    .size(theme::SIZE_LABEL)
                    .width(Fill)
                    .wrapping(text::Wrapping::WordOrGlyph)
            ]
            .into(),
            Block::Code(code) => kit::well(kit::mono(code.as_str())).padding([6, 10]).into(),
            Block::Rule => kit::rule(),
        };
        page = page.push(element);
    }
    kit::panel("Runbook", page)
}
