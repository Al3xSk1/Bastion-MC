//! The launch page: the gauge with the launch control in its middle, the instance that
//! would be launched, the stages of a launch, and what the running (or the last) launch is
//! protected by.
//!
//! The block around the gauge does not change its size with the state: it keeps room for
//! the read-out, for three lines of words and for the stop control whether or not they are
//! there, so that pressing the launch control moves nothing. In the two arrangements with
//! columns the panels beside it are stretched to end on one line with it — as long as
//! their words are of the usual length; with unusual ones every panel is as high as its
//! content, and the page scrolls if it has to.
//!
//! "Keeps room" means at least that much: words that need a fourth line, or a read-out too
//! long for one line, make the block higher. Nothing is cut off to keep it still.

use bastion_mc_proto::{Phase, Shield};
use iced::{
    Center, Element, Fill, FillPortion, Length, Padding,
    widget::{Id, Space, button, canvas, column, container, row, stack, text},
};

use super::{Layout, kit};
use crate::{
    app::{App, InstanceChoice, Message, Page, Progress, SEQUENCE, Stage},
    backend::{clean_to, fit},
    hud::{self, Gauge, Motion, Step},
    theme::{self, Tone},
};

/// From this width of the page on, the three parts stand side by side.
const WIDE: f32 = 920.0;
/// Below this one they are stacked.
const NARROW: f32 = 540.0;
/// The side by side arrangement is not stretched beyond this.
const MAX_WIDTH: f32 = 1180.0;
/// The least a column beside the gauge needs.
const SIDE_COLUMN: f32 = 300.0;
/// The gauge grows with the window up to this side.
const GAUGE_MAX: f32 = 336.0;

/// The read-out line under the gauge.
const READOUT_HEIGHT: f32 = 16.0;
/// Room for three lines of words under the gauge.
const WORDS_HEIGHT: f32 = 54.0;
/// The row of the stop control.
const ACTION_HEIGHT: f32 = 30.0;
/// Everything under the gauge, with the spaces between.
const BELOW_GAUGE: f32 = theme::S2 + READOUT_HEIGHT + theme::S1 + WORDS_HEIGHT + theme::S2 + ACTION_HEIGHT;
/// The launcher's words shown under the gauge are cut to this many characters (three lines
/// of the usual letters at the narrowest); the console has them in full.
const WORDS_CHARS: usize = 96;
/// Characters of an instance's name in a sentence under the gauge.
const NAME_CHARS: usize = 28;
/// Characters of the launcher's name for what is being measured ("assets", "libraries").
const MEASURE_CHARS: usize = 12;

/// How high the block around a gauge of `side` is, with the usual words in it.
const fn control_height(side: f32) -> f32 {
    2.0 * theme::S4 + side + BELOW_GAUGE
}

/// The side of the gauge in the wide arrangement: as large as the room allows.
fn gauge_side(layout: Layout) -> f32 {
    let by_width = layout.width.min(MAX_WIDTH) - 2.0 * SIDE_COLUMN - 4.0 * theme::S4;
    let by_height = layout.height - control_height(0.0) - 2.0 * theme::S5;
    by_width.min(by_height).clamp(Gauge::SIZE, GAUGE_MAX).floor()
}

/// Are the words in the instance panel as short as they usually are? Then the panel under
/// it can be stretched to end on one line with the block beside them. With long words in
/// it (a reason why it cannot be launched, a signer with a long name) the panels keep
/// their own heights instead: nothing is squeezed, the columns end where they end.
fn usual_instance(app: &App) -> bool {
    app.selected.as_ref().is_none_or(|choice| {
        let instance = &choice.0;
        let version = instance.minecraft.as_deref().map_or(0, |version| version.chars().count());
        version + instance.kind.chars().count() <= 28
            && instance.error.is_none()
            && instance.detail.as_deref().is_none_or(|detail| detail.chars().count() <= 48)
    })
}

/// The same question for the protection panel: the five aspects, named and described the
/// way the launcher does it, fit a panel of the block's height; anything more gets a
/// panel as high as it needs.
fn usual_protection(app: &App) -> bool {
    app.protection.len() <= kit::ASPECTS.len()
        && app.protection.iter().all(|item| item.aspect.chars().count() <= 12 && item.detail.chars().count() <= 80)
}

pub fn page(app: &App, layout: Layout) -> Element<'_, Message> {
    let page = launch_page(app, layout);
    // A Bedrock sign-in or a Realms join waiting for an answer comes first: it is meant to be
    // acted on now.
    let mut prompts: Vec<Element<'_, Message>> =
        [super::signin::prompt(app), super::realms::prompt(app)].into_iter().flatten().collect();
    if prompts.is_empty() {
        return page;
    }
    prompts.push(page);
    iced::widget::Column::with_children(prompts).spacing(theme::S4).into()
}

fn launch_page(app: &App, layout: Layout) -> Element<'_, Message> {
    let stretch = usual_instance(app);
    let readouts = |width: Length| {
        column![instance(app), sequence(app, stretch)].spacing(theme::S4).width(width).height(if stretch {
            Fill
        } else {
            Length::Shrink
        })
    };
    if layout.width >= WIDE {
        let side = gauge_side(layout);
        let filling = usual_protection(app);
        let block = row![
            readouts(FillPortion(1)),
            control(app, side),
            container(protection(app, Rows::Stacked, filling)).width(FillPortion(1)).height(if filling {
                Fill
            } else {
                Length::Shrink
            }),
        ]
        .spacing(theme::S4);
        // The block stands in the middle of the room it has. In a low window, or when one
        // of its panels is higher than the block, it starts at the top like every other page.
        let centred = stretch && filling;
        let above = if centred { ((layout.height - control_height(side)) / 2.0).floor().max(0.0) } else { 0.0 };
        container(container(block).max_width(MAX_WIDTH))
            .center_x(Fill)
            .padding(Padding { top: above, ..Padding::ZERO })
            .into()
    } else if layout.width >= NARROW {
        column![row![control(app, Gauge::SIZE), readouts(Fill)].spacing(theme::S4), protection(app, Rows::Table, false)]
            .spacing(theme::S4)
            .into()
    } else {
        column![
            container(control(app, Gauge::SIZE)).center_x(Fill),
            instance(app),
            sequence(app, false),
            protection(app, Rows::Stacked, false)
        ]
        .spacing(theme::S4)
        .into()
    }
}

// ------------------------------------------------------------------ the control

fn halting(app: &App) -> bool {
    app.launch().is_some_and(|job| job.stopping || job.phase == Some(Phase::Stopping))
}

fn gauge(app: &App) -> Gauge {
    let motion = match app.launch() {
        None => Motion::Still,
        Some(_) if halting(app) => Motion::Halt,
        Some(job) if job.phase == Some(Phase::Running) => Motion::Pulse,
        Some(_) => Motion::Sweep,
    };
    let steps = app
        .sequence()
        .into_iter()
        .map(|stage| match stage {
            Stage::Pending => Step::Pending,
            Stage::Active => Step::Active,
            Stage::Done => Step::Done,
            Stage::Skipped => Step::Skipped,
        })
        .collect();
    // Before the launcher has reported, the ring shows that there is something to report.
    let shields = if app.protection.is_empty() {
        vec![None; kit::ASPECTS.len()]
    } else {
        app.protection.iter().map(|item| Some(item.shield)).collect()
    };
    Gauge { steps, motion, progress: app.progress.as_ref().map(Progress::fraction), shields, time: app.launch_clock() }
}

/// The widget id of the launch control.
pub fn core_id() -> Id {
    Id::new("launch-core")
}

/// The launch control: pressable exactly when a launch can start; while one is under way
/// it says where that launch is and cannot be pressed (stopping is its own button, so that
/// a second press on "Play" can never stop a game).
fn core(app: &App, scale: f32) -> Element<'_, Message> {
    let launch = app.launch();
    let (word, tone) = match launch {
        Some(_) if halting(app) => ("Stopping", Tone::Caution),
        Some(job) if job.phase == Some(Phase::Running) => ("Running", Tone::Accent),
        Some(_) => ("Launching", Tone::Accent),
        None if app.can_play() => ("Play", Tone::Accent),
        None => ("Play", Tone::Neutral),
    };
    let readout = match launch {
        Some(_) => match (app.session_time(), &app.progress) {
            (Some(time), _) => kit::clock(time.as_secs()),
            (None, Some(progress)) => format!("{}%", progress.percent()),
            (None, None) => String::new(),
        },
        None if app.demo => "demo mode".to_string(),
        None => app
            .account
            .as_ref()
            .filter(|account| account.signed_in)
            .and_then(|account| account.player.as_deref())
            // What fits in the circle whatever the letters; the accounts page has the
            // whole name.
            .map(|player| clean_to(player, 12))
            .unwrap_or_default(),
    };
    let size = if word.len() <= 4 { 24.0 } else { 13.0 } * scale;
    let mut face = column![text(theme::caps(word)).size(size).wrapping(text::Wrapping::None)]
        .spacing(theme::S1 * scale)
        .align_x(Center);
    if !readout.is_empty() {
        face = face.push(
            text(readout).size((theme::SIZE_SMALL * scale).round()).font(theme::MONO).wrapping(text::Wrapping::None),
        );
    }
    let side = ((Gauge::CORE - 4.0) * scale).round();
    button(container(face).id(core_id()).center(Fill))
        .width(side)
        .height(side)
        .padding(0)
        .clip(true)
        .style(theme::core(tone))
        .on_press_maybe(app.can_play().then_some(Message::Play))
        .into()
}

/// What stands under the gauge in words: why nothing can be started, what the launcher
/// said last about the launch under way, or how the last launch ended.
fn words(app: &App) -> String {
    match (app.busy(), app.play_blocker()) {
        (false, Some(reason)) => reason.to_string(),
        _ if !app.launch_status.is_empty() => clean_to(&app.launch_status, WORDS_CHARS),
        (false, None) => match (app.selected.as_ref().map(|choice| choice.label(NAME_CHARS)), app.demo) {
            (Some(name), true) => format!("Ready: {name}, demo mode."),
            // A name that had to be cut ends in "…"; a full stop after that is one too many.
            (Some(name), false) if name.ends_with('…') => format!("Ready: {name}"),
            (Some(name), false) => format!("Ready: {name}."),
            (None, _) => String::new(),
        },
        _ => String::new(),
    }
}

fn control(app: &App, side: f32) -> Element<'_, Message> {
    let scale = side / Gauge::SIZE;
    let dial = stack![canvas(gauge(app)).width(side).height(side), container(core(app, scale)).center(Fill)];
    // What is being measured, as figures; the gauge has the same as an arc. The launcher's
    // name for it is a word ("assets") and stands before the figures; should the two not
    // fit on one line, the figures go under it. Either is narrower than the gauge.
    let readout: Element<'_, Message> = match &app.progress {
        Some(progress) => row![
            Space::new().height(READOUT_HEIGHT),
            row![
                kit::label(&fit(&progress.label, MEASURE_CHARS)),
                kit::mono(progress.amount()).wrapping(text::Wrapping::None)
            ]
            .spacing(theme::S2)
            .align_y(Center)
            .wrap()
            .align_x(Center)
        ]
        .align_y(Center)
        .into(),
        None => Space::new().height(READOUT_HEIGHT).into(),
    };
    // At least three lines high: what needs more makes the block higher, not the words shorter.
    let said =
        row![Space::new().height(WORDS_HEIGHT), kit::body(words(app)).size(13).center().width(Fill)].align_y(Center);
    // The stop control keeps its place whether or not there is something to stop: drawn,
    // not offered, when there is not.
    let stopping = app.launch().is_some_and(|job| job.stopping);
    let stop = kit::action("Stop", Tone::Danger, (app.launch().is_some() && !stopping).then_some(Message::Stop));
    let below = column![
        readout,
        Space::new().height(theme::S1),
        said,
        Space::new().height(theme::S2),
        container(stop).center_x(Fill).center_y(ACTION_HEIGHT)
    ]
    .align_x(Center)
    .width(side);
    kit::framed(
        hud::Frame::brackets(theme::LINE_HI).fill(theme::VOID),
        container(column![dial, Space::new().height(theme::S2), below].align_x(Center))
            .padding(theme::S4)
            .width(side + 2.0 * theme::S4),
    )
    .into()
}

// ------------------------------------------------------------------ instance

fn instance(app: &App) -> Element<'_, Message> {
    let launching = app.launch().is_some();
    // While a launch is under way the choice is shown, not offered: it could not change it.
    let chooser = kit::with_picker_room(move |chars| {
        let list = match (&app.selected, launching) {
            // Same box and label as the list it stands in for: the panel keeps its height.
            (Some(choice), true) => kit::picked(choice.label(chars)),
            _ => kit::picker(
                app.instances.as_slice(),
                app.selected.as_ref(),
                if app.instances.is_empty() { "No instances yet" } else { "Choose an instance" },
                chars,
                InstanceChoice::label,
                Message::InstanceSelected,
            ),
        };
        match &app.selected {
            // The list has room for the beginning of this name only; here is all of it.
            Some(choice) if choice.label(chars) != choice.label(usize::MAX) => {
                column![list, kit::mono(choice.0.id.as_str()).color(theme::TEXT_DIM)].spacing(theme::S2).into()
            }
            _ => list,
        }
    });
    let mut content = column![chooser].spacing(theme::S2);
    if let Some(choice) = &app.selected {
        let instance = &choice.0;
        if let Some(version) = &instance.minecraft {
            content = content.push(kit::small(format!("Minecraft {version}, {}", instance.kind)));
        }
        if let Some(error) = &instance.error {
            // The whole message is on the instances page; here it must not push the
            // sequence out of its panel.
            content = content.push(kit::small(clean_to(error, 160)).color(theme::RED));
        } else if let Some(detail) = &instance.detail {
            content = content.push(kit::small(format!("Signed profile: {}", clean_to(detail, 120))));
        }
    }
    content = content.push(Space::new().height(theme::S1)).push(kit::switch(
        "Demo mode (no account)",
        app.demo,
        (!launching).then_some(Message::DemoToggled(!app.demo)),
    ));
    kit::panel_with(
        hud::Frame::plate(),
        kit::label("Instance"),
        Some(kit::link("Manage", Tone::Neutral, Some(Message::Navigate(Page::Instances))).into()),
        content,
    )
}

// ------------------------------------------------------------------ sequence

fn stage_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Preparing => "Files",
        Phase::Verifying => "Verify",
        Phase::Authenticating => "Sign-in",
        Phase::Starting => "Sandbox",
        Phase::Running => "Game",
        Phase::Stopping => "Stop",
    }
}

/// Where the number of a stage ends and its mark begins, so that the line between two
/// marks can be put under them.
const STAGE_NUMBER: f32 = 16.0;

/// The stages of a launch as a list: the outer ring of the gauge, spelled out. `stretch`
/// spreads the stages over all the height the panel is given.
fn sequence(app: &App, stretch: bool) -> Element<'_, Message> {
    let stages = app.sequence();
    let signal = if halting(app) { theme::AMBER } else { theme::ACCENT };
    let mut list = column![].height(if stretch { Fill } else { Length::Shrink });
    for (number, (phase, stage)) in SEQUENCE.into_iter().zip(stages).enumerate() {
        if number > 0 {
            // The line from the stage before: lit once that stage is behind.
            let passed = matches!(stages[number - 1], Stage::Done | Stage::Skipped);
            let line = if passed { theme::alpha(signal, 0.45) } else { theme::LINE };
            list = list.push(
                row![
                    Space::new().width(STAGE_NUMBER + theme::S2 + 3.5),
                    container(Space::new())
                        .width(1)
                        .height(if stretch { Fill } else { Length::Fixed(theme::S2) })
                        .style(theme::hairline(line))
                ]
                .height(if stretch { Fill } else { Length::Shrink }),
            );
        }
        let (mark, name_color, note): (Element<'_, Message>, _, String) = match stage {
            Stage::Pending => (kit::pip(theme::LINE_HI, false), theme::TEXT_FAINT, String::new()),
            Stage::Active => {
                let note = match (phase, &app.progress, app.session_time()) {
                    (Phase::Preparing, Some(progress), _) => format!("{}%", progress.percent()),
                    (Phase::Running, _, Some(time)) => kit::clock(time.as_secs()),
                    _ => "now".to_string(),
                };
                (kit::pip(signal, true), theme::TEXT, note)
            }
            Stage::Done => (kit::pip(theme::alpha(signal, 0.55), true), theme::TEXT_DIM, "done".to_string()),
            Stage::Skipped => (kit::pip(theme::LINE_HI, false), theme::TEXT_FAINT, "skipped".to_string()),
        };
        list = list.push(
            row![
                text(format!("{:02}", number + 1))
                    .size(theme::SIZE_LABEL)
                    .font(theme::MONO)
                    .color(theme::TEXT_FAINT)
                    .width(STAGE_NUMBER)
                    .wrapping(text::Wrapping::None),
                mark,
                kit::label_in(stage_name(phase), name_color),
                Space::new().width(Fill),
                text(note).size(theme::SIZE_SMALL).font(theme::MONO).color(if stage == Stage::Active {
                    signal
                } else {
                    theme::TEXT_FAINT
                }),
            ]
            .spacing(theme::S2)
            .align_y(Center)
            .padding([2, 0]),
        );
    }
    if stretch {
        kit::panel_filling(hud::Frame::plate(), kit::label("Sequence"), None, list)
    } else {
        kit::panel("Sequence", list)
    }
}

// ------------------------------------------------------------------ protection

/// How the aspects are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rows {
    /// Name and level on one line, the description under them: for a narrow panel.
    Stacked,
    /// Name, level and description side by side: for a panel as wide as the page.
    Table,
}

/// How many of the reported aspects are in force, as a tag: the whole panel in one glance.
fn protection_summary<'a>(app: &App) -> Element<'a, Message> {
    if app.protection.is_empty() {
        // A launch under way reports once the game runs and the launcher has checked it
        // (limits read back, its own confinement applied); until then there is a launch, and
        // nothing known about it.
        return kit::chip(if app.launch().is_some() { "Awaiting report" } else { "No launch yet" }, Tone::Neutral);
    }
    let count = |level: Shield| app.protection.iter().filter(|item| item.shield == level).count();
    match (count(Shield::Off), count(Shield::Partial)) {
        (0, 0) => kit::chip("All in force", Tone::Accent),
        (0, reduced) => kit::chip(&format!("{reduced} reduced"), Tone::Caution),
        (off, _) => kit::chip(&format!("{off} not protected"), Tone::Danger),
    }
}

/// One aspect: its mark, its name, its level in words, and what the launcher said about
/// it. Names and words wrap inside their columns: an aspect this build has never heard of
/// may be called anything.
fn aspect<'a>(rows: Rows, name: &str, level: Option<Shield>, detail: String) -> Element<'a, Message> {
    let mark = container(kit::shield(level)).padding(Padding { top: 1.0, ..Padding::ZERO });
    let cell = |words: &str, color| {
        text(theme::caps(words)).size(theme::SIZE_LABEL).color(color).wrapping(text::Wrapping::WordOrGlyph)
    };
    let name = cell(name, theme::TEXT);
    let words = cell(kit::shield_words(level), kit::shield_ink(level));
    match rows {
        Rows::Stacked => row![
            mark,
            column![row![name, words].spacing(theme::S3).align_y(Center).wrap(), kit::small(detail).width(Fill)]
                .spacing(3)
                .width(Fill),
        ]
        .spacing(theme::S3)
        .width(Fill)
        .into(),
        Rows::Table => row![
            mark,
            container(name).width(100).padding(Padding { top: 2.0, ..Padding::ZERO }),
            container(words).width(124).padding(Padding { top: 2.0, ..Padding::ZERO }),
            container(kit::small(detail).width(Fill)).width(Fill).padding(Padding { top: 1.0, ..Padding::ZERO }),
        ]
        .spacing(theme::S3)
        .width(Fill)
        .into(),
    }
}

/// What the launch runs with, one entry per aspect. Before any launch the five aspects are
/// listed with what each is about, and no level. `filling` spreads the entries over all
/// the height the panel is given, like the stages beside them.
fn protection(app: &App, rows: Rows, filling: bool) -> Element<'_, Message> {
    let entries: Vec<Element<'_, Message>> = if app.protection.is_empty() {
        kit::ASPECTS.iter().map(|(name, meaning)| aspect(rows, name, None, meaning.to_string())).collect()
    } else {
        app.protection.iter().map(|item| aspect(rows, &item.aspect, Some(item.shield), item.detail.clone())).collect()
    };
    let gap = if rows == Rows::Table { theme::S2 + 2.0 } else { theme::S3 };
    let mut list = column![].height(if filling { Fill } else { Length::Shrink });
    for (index, entry) in entries.into_iter().enumerate() {
        if index > 0 {
            list = list.push(Space::new().height(if filling { theme::S2 } else { gap }));
            if filling {
                list = list.push(Space::new().height(Fill));
            }
        }
        list = list.push(entry);
    }
    // What is listed belongs to a launch: the one under way, or — said in the heading — the
    // one before. (The narrow panel has room for the short form only.)
    let heading = kit::label(match (app.launch().is_none() && !app.protection.is_empty(), rows) {
        (false, _) => "Protection",
        (true, Rows::Table) => "Protection of the last launch",
        (true, Rows::Stacked) => "Last launch",
    });
    let summary = Some(protection_summary(app));
    if filling {
        kit::panel_filling(hud::Frame::plate(), heading, summary, list)
    } else {
        kit::panel_with(hud::Frame::plate(), heading, summary, list)
    }
}
