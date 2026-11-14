//! The view: a pure function from [`App`] to widgets.
//!
//! Nothing here changes state or does work, and nothing reads a clock: what moves is drawn
//! from [`App::now`]. Every interaction is a [`Message`]; whether a control is offered is
//! decided by asking the state (`can_play`, `can_sign_in`, …), so the rules live in one
//! place and are tested without a window.
//!
//! The window is a navigation rail and a stage:
//!
//! ```text
//!  ┌────────┬──────────────────────────────────────────────┐
//!  │ BASTION│ PAGE TITLE ─────────────────────── activity  │
//!  │        │ (notice, when there is one)                  │
//!  │ Launch │                                              │
//!  │ Accounts   one of the five pages; it scrolls when     │
//!  │ Instances  the window is too small for it             │
//!  │ Console│                                              │
//!  │ Security                                              │
//!  │        │                                              │
//!  │ account│ status line ──────────────── launcher version│
//!  └────────┴──────────────────────────────────────────────┘
//! ```
//!
//! Layout rules: everything derives from the room there is, never from a guess about it.
//! The window's width decides whether the rail shrinks to its marks
//! ([`Layout::COMPACT_BELOW`]); a page is handed the exact width and height it has
//! ([`Layout`]) and chooses its arrangement by them. Text wraps, lists scroll, and no
//! control is ever placed where it cannot be reached. The right edge of the content is the
//! same on every page and does not move when a scroll bar appears: the bar has a gutter of
//! its own.
//!
//! Nothing is drawn outside the place it was given, whatever the launcher says and however
//! long it is (see [`kit`]): the tests measure every paragraph of text on every page, in
//! every state, at thirteen window sizes, against the room it has.

mod accounts;
mod console;
mod instances;
pub mod kit;
mod launch;
pub mod realms;
mod security;
#[cfg(test)]
mod tests;
mod tiles;

use bastion_mc_proto::{Phase, Shield};
use iced::{
    Center, Element, Fill, Length, Padding,
    widget::{Id, Space, button, canvas, column, container, responsive, row, scrollable, space, text, tooltip},
};

use crate::{
    app::{App, JobKind, Message, Notice, Page, ScramOutcome, ScramState},
    backend::clean_to,
    hud::{Glyph, Sign},
    theme::{self, Tone},
};

/// Characters of an account label that fit next to its mark in the rail.
const RAIL_LABEL_CHARS: usize = 14;

/// What a page needs to know about its place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// Width a page has for its content.
    pub width: f32,
    /// Height that is visible of a page at a time; what is higher scrolls.
    pub height: f32,
}

impl Layout {
    /// Narrower windows get the compact rail: its marks without their names.
    pub const COMPACT_BELOW: f32 = 800.0;
    pub(crate) const RAIL: f32 = 188.0;
    pub(crate) const RAIL_COMPACT: f32 = 58.0;
    /// Margin of the stage: the distance of the content from the rail and from the
    /// window's right edge.
    const MARGIN: f32 = 20.0;
    /// The part of the right margin the scroll bar of a page lives in.
    const GUTTER: f32 = 12.0;
    const SCROLL_BAR: f32 = 6.0;

    pub(crate) const fn rail_width(compact: bool) -> f32 {
        if compact { Self::RAIL_COMPACT } else { Self::RAIL }
    }
}

pub fn root(app: &App) -> Element<'_, Message> {
    responsive(move |window| {
        let compact = window.width < Layout::COMPACT_BELOW;
        row![
            rail(app, compact),
            container(Space::new()).width(1).height(Fill).style(theme::hairline(theme::LINE)),
            stage(app, window.height)
        ]
        .into()
    })
    .into()
}

// ------------------------------------------------------------------ rail

/// The widget id of a rail entry (tests press the entries by it).
pub fn nav_id(page: Page) -> Id {
    Id::new(match page {
        Page::Launch => "nav-launch",
        Page::Accounts => "nav-accounts",
        Page::Instances => "nav-instances",
        Page::Console => "nav-console",
        Page::Security => "nav-security",
    })
}

fn sign(page: Page) -> Sign {
    match page {
        Page::Launch => Sign::Launch,
        Page::Accounts => Sign::Accounts,
        Page::Instances => Sign::Instances,
        Page::Console => Sign::Console,
        Page::Security => Sign::Security,
    }
}

fn glyph<'a>(sign: Sign, color: iced::Color) -> Element<'a, Message> {
    canvas(Glyph { sign, color }).width(Glyph::SIZE).height(Glyph::SIZE).into()
}

/// What needs attention behind a rail entry, if anything: the colour of its marker.
fn attention(app: &App, page: Page) -> Option<Tone> {
    match page {
        Page::Launch => app
            .launch()
            .map(|job| if job.stopping || job.phase == Some(Phase::Stopping) { Tone::Caution } else { Tone::Accent }),
        // A sign-in is waiting for the user.
        Page::Accounts => app.login.as_ref().map(|_| Tone::Caution),
        Page::Security => {
            // A scram that is running, or that did not do all of its work, outranks
            // everything else on that page.
            let scram = app.emergency.active()
                || matches!(app.emergency.outcome, Some(ScramOutcome::Incomplete(_) | ScramOutcome::NotRun(_)));
            let any = |level: Shield| app.protection.iter().any(|item| item.shield == level);
            if scram || any(Shield::Off) {
                Some(Tone::Danger)
            } else if any(Shield::Partial) {
                Some(Tone::Caution)
            } else {
                None
            }
        }
        Page::Instances | Page::Console => None,
    }
}

fn nav_entry(app: &App, page: Page, compact: bool) -> Element<'_, Message> {
    let current = app.page == page;
    let color = if current { theme::ACCENT } else { theme::TEXT_DIM };
    let marker: Element<'_, Message> = match attention(app, page) {
        Some(tone) => kit::pip(tone.color(), true),
        None => Space::new().width(8).height(8).into(),
    };
    let face: Element<'_, Message> = if compact {
        column![glyph(sign(page), color), marker].spacing(theme::S1).align_x(Center).into()
    } else {
        row![
            glyph(sign(page), color),
            text(theme::caps(page.title())).size(theme::SIZE_SMALL).wrapping(text::Wrapping::None),
            space().width(Fill),
            marker
        ]
        .spacing(theme::S3)
        .align_y(Center)
        .into()
    };
    let entry = button(container(face).id(nav_id(page)).center_x(Fill))
        .width(Fill)
        .padding(if compact { [10, 4] } else { [11, 14] })
        .style(theme::nav(current))
        .on_press(Message::Navigate(page));
    // The selected entry carries a bar on its outer edge.
    let bar = container(Space::new()).width(2).height(Fill).style(theme::hairline(if current {
        theme::ACCENT
    } else {
        theme::DECK
    }));
    let entry: Element<'_, Message> = row![bar, entry].height(Length::Shrink).into();
    if compact {
        tooltip(entry, container(kit::small(page.title())).padding([4, 8]).style(theme::well), tooltip::Position::Right)
            .into()
    } else {
        entry
    }
}

fn rail(app: &App, compact: bool) -> Element<'_, Message> {
    let brand: Element<'_, Message> = if compact {
        container(glyph(Sign::Brand, theme::ACCENT)).center_x(Fill).into()
    } else {
        row![
            glyph(Sign::Brand, theme::ACCENT),
            column![
                text(theme::caps("Bastion")).size(15).color(theme::TEXT).wrapping(text::Wrapping::None),
                kit::label("MC launcher"),
            ]
            .spacing(2)
        ]
        .spacing(theme::S3)
        .align_y(Center)
        .padding([0, 14])
        .into()
    };
    let mut entries = column![].spacing(2);
    for page in Page::ALL {
        entries = entries.push(nav_entry(app, page, compact));
    }
    container(
        column![brand, Space::new().height(theme::S4), entries, space().height(Fill), rail_account(app, compact)]
            .height(Fill),
    )
    .padding([18, 0])
    .width(Layout::rail_width(compact))
    .height(Fill)
    .clip(true)
    .style(theme::rail)
    .into()
}

/// Who a launch would play as. Pressing it leads to the accounts.
fn rail_account(app: &App, compact: bool) -> Element<'_, Message> {
    let (name, state, tone) = match &app.account {
        _ if app.account_name.is_empty() => ("", "No account yet", Tone::Neutral),
        None => (app.account_name.as_str(), "Checking…", Tone::Neutral),
        Some(account) if account.signed_in => (app.account_name.as_str(), "Signed in", Tone::Accent),
        Some(_) => (app.account_name.as_str(), "Not signed in", Tone::Neutral),
    };
    let shown = app.account.as_ref().and_then(|account| account.player.as_deref()).unwrap_or(name);
    let face: Element<'_, Message> = if compact {
        container(kit::monogram(shown, 30.0, tone)).center_x(Fill).into()
    } else {
        column![
            kit::label("Account"),
            row![
                kit::monogram(shown, 30.0, tone),
                column![
                    // A label can be as long as 64 characters; the rail shows its beginning.
                    kit::mono(clean_to(name, RAIL_LABEL_CHARS)).wrapping(text::Wrapping::None),
                    text(state).size(theme::SIZE_LABEL).color(tone.color()).wrapping(text::Wrapping::None)
                ]
                .spacing(2)
            ]
            .spacing(theme::S2)
            .align_y(Center)
        ]
        .spacing(theme::S2)
        .into()
    };
    button(container(face).id(Id::new("rail-account")).width(Fill))
        .width(Fill)
        .padding(if compact { [8, 4] } else { [10, 14] })
        .style(theme::row_button(false))
        .on_press(Message::Navigate(Page::Accounts))
        .into()
}

// ------------------------------------------------------------------ stage

/// The widget id of the scrolling area a page lives in.
pub fn page_id() -> Id {
    Id::new("page")
}

/// The share of the window's height a notice may take. A longer one scrolls inside its
/// banner: the page under it keeps most of the window whatever the launcher has to say.
const NOTICE_SHARE: f32 = 0.38;
/// A notice is never held to less than this, however low the window: four lines.
const NOTICE_LEAST: f32 = 96.0;

fn stage(app: &App, window_height: f32) -> Element<'_, Message> {
    let gutter = Padding { right: Layout::GUTTER, ..Padding::ZERO };
    // The page is handed the room that is left between the header and the footer, measured,
    // not estimated: a notice of any length above it is already accounted for.
    let body = responsive(move |room| {
        let layout = Layout { width: (room.width - Layout::GUTTER).max(160.0), height: room.height };
        let page: Element<'_, Message> = match app.page {
            Page::Launch => launch::page(app, layout),
            Page::Accounts => accounts::page(app, layout),
            Page::Instances => instances::page(app, layout),
            Page::Console => console::page(app, layout),
            Page::Security => security::page(app, layout),
        };
        // The console fills its place and scrolls its lines itself; every other page
        // scrolls as a whole when the window is too small for it.
        if app.page == Page::Console {
            container(page).padding(gutter).height(Fill).into()
        } else {
            let bar = scrollable::Scrollbar::new().width(Layout::SCROLL_BAR).scroller_width(Layout::SCROLL_BAR);
            scrollable(container(page).padding(gutter).width(Fill))
                .id(page_id())
                .direction(scrollable::Direction::Vertical(bar))
                .style(theme::scroll)
                .width(Fill)
                .height(Fill)
                .into()
        }
    });
    let mut content = column![container(header(app)).padding(gutter)].spacing(theme::S3);
    if let Some(notice) = &app.notice {
        let room = (window_height * NOTICE_SHARE).max(NOTICE_LEAST);
        content = content.push(container(notice_banner(notice, room)).padding(gutter));
    }
    content = content.push(body).push(container(footer(app)).padding(gutter));
    container(content)
        .padding(Padding { top: 14.0, right: Layout::MARGIN - Layout::GUTTER, bottom: 10.0, left: Layout::MARGIN })
        .width(Fill)
        .height(Fill)
        .style(theme::stage)
        .into()
}

/// What the launcher is doing, in two or three words, for the top of every page.
fn activity(app: &App) -> (&'static str, Tone) {
    match app.emergency.state {
        ScramState::Running => return ("Scram in progress", Tone::Danger),
        ScramState::StoppingSignIn => return ("Scram: ending the sign-in", Tone::Danger),
        ScramState::Idle => {}
    }
    let Some(job) = &app.job else { return ("Ready", Tone::Neutral) };
    match job.kind {
        JobKind::Refresh => ("Checking", Tone::Accent),
        JobKind::SignIn if job.stopping => ("Cancelling", Tone::Caution),
        JobKind::SignIn => ("Signing in", Tone::Accent),
        JobKind::SignOut => ("Signing out", Tone::Accent),
        JobKind::Versions => ("Fetching versions", Tone::Accent),
        JobKind::CreateInstance => ("Creating instance", Tone::Accent),
        JobKind::Launch if job.stopping || job.phase == Some(Phase::Stopping) => ("Stopping", Tone::Caution),
        JobKind::Launch if job.phase == Some(Phase::Running) => ("Game running", Tone::Accent),
        JobKind::Launch => ("Launching", Tone::Accent),
    }
}

fn header(app: &App) -> Element<'_, Message> {
    let (words, tone) = activity(app);
    row![
        text(theme::caps(app.page.title())).size(13).color(theme::ACCENT).wrapping(text::Wrapping::None),
        container(Space::new()).width(Fill).height(1).style(theme::hairline(theme::LINE)),
        kit::pip(tone.line(), app.job.is_some() || app.emergency.active()),
        kit::label_in(words, if tone == Tone::Neutral { theme::TEXT_FAINT } else { tone.color() }),
    ]
    .spacing(theme::S3)
    .align_y(Center)
    .into()
}

/// The widget id of the scrolling area a long notice lives in.
pub fn notice_id() -> Id {
    Id::new("notice")
}

/// The banner of a notice, at most `room` high. A notice that needs more scrolls in it;
/// the controls beside the words stay where they are.
fn notice_banner(notice: &Notice, room: f32) -> Element<'_, Message> {
    let tone = notice.severity.tone();
    let bar = scrollable::Scrollbar::new().width(Layout::SCROLL_BAR).scroller_width(Layout::SCROLL_BAR);
    let words = scrollable(
        // The right padding keeps the words clear of the scroll bar when there is one.
        container(kit::body(notice.text.as_str()))
            .padding(Padding { top: 10.0, right: Layout::GUTTER, bottom: 10.0, left: 4.0 })
            .width(Fill),
    )
    .id(notice_id())
    .direction(scrollable::Direction::Vertical(bar))
    .style(theme::scroll)
    .width(Fill)
    .height(Length::Shrink);
    container(
        row![
            container(Space::new()).width(3).height(Fill).style(theme::hairline(tone.color())),
            container(words).max_height(room).width(Fill),
            // Messages can contain paths and settings to act on; text is not selectable.
            kit::link("Copy", Tone::Neutral, Some(Message::Copy(notice.text.clone()))),
            kit::link("Dismiss", Tone::Neutral, Some(Message::DismissNotice)),
            Space::new().width(theme::S1),
        ]
        .spacing(theme::S2)
        .align_y(Center)
        .height(Length::Shrink),
    )
    .width(Fill)
    .style(theme::notice(notice.severity))
    .into()
}

fn footer(app: &App) -> Element<'_, Message> {
    let version = match &app.launcher_version {
        Some(version) => format!("launcher {version}"),
        None => "launcher not heard from yet".to_string(),
    };
    row![
        kit::small(app.status.as_str()).width(Fill),
        text(version).size(theme::SIZE_LABEL).font(theme::MONO).color(theme::TEXT_FAINT).wrapping(text::Wrapping::None),
    ]
    .spacing(theme::S3)
    .align_y(Center)
    .into()
}
