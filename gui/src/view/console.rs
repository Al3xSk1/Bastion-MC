//! The console page: the launcher's, the relay's and the game's output, most recent last.
//!
//! The lines are only built while this page is shown; on every other page the console
//! costs nothing however much the game prints.

use bastion_mc_proto::Source;
use iced::{
    Center, Element, Fill, Length,
    widget::{Id, column, container, row, scrollable, space, text},
};

use super::{Layout, kit};
use crate::{
    app::{App, Message},
    console::{Level, Line},
    theme::{self, Tone},
};

/// Who said it, then what was said. The name is a word, so that the source of a line does
/// not hang on its colour.
fn line(line: &Line) -> Element<'_, Message> {
    let who = match line.source {
        Source::Launcher => theme::alpha(theme::ACCENT, 0.75),
        Source::Egress | Source::Game => theme::TEXT_FAINT,
    };
    let what = match (line.level, line.source) {
        (Level::Error, _) => theme::RED,
        (Level::Warn, _) => theme::AMBER,
        (Level::Info, Source::Game) => theme::TEXT,
        (Level::Info, Source::Launcher) => theme::ACCENT,
        (Level::Info, Source::Egress) => theme::TEXT_DIM,
    };
    row![
        text(line.source.tag())
            .size(theme::SIZE_LABEL)
            .font(theme::MONO)
            .color(who)
            .width(Length::Fixed(58.0))
            .wrapping(text::Wrapping::None),
        text(&line.text)
            .size(theme::SIZE_SMALL)
            .font(theme::MONO)
            .color(what)
            .width(Fill)
            .wrapping(text::Wrapping::WordOrGlyph),
    ]
    .spacing(theme::S2)
    .into()
}

pub fn page(app: &App, _layout: Layout) -> Element<'_, Message> {
    let console = &app.console;
    let has_lines = !console.is_empty();
    let count = match console.len() {
        0 => "No output yet".to_string(),
        1 => "1 line".to_string(),
        lines => format!("{lines} lines"),
    };
    let mut bar = row![kit::label(&count)].spacing(theme::S3).align_y(Center);
    if console.discarded() > 0 {
        bar = bar.push(kit::small(format!("{} earlier lines are only in the log file", console.discarded())));
    }
    bar = bar
        .push(space().width(Fill))
        .push(kit::link("Copy all", Tone::Neutral, has_lines.then_some(Message::CopyConsole)))
        .push(kit::link("Clear", Tone::Neutral, has_lines.then_some(Message::ClearConsole)));

    let body: Element<'_, Message> = if has_lines {
        // Anchored to the bottom: new output stays in view unless the user scrolled up.
        scrollable(column(console.lines().map(line)).spacing(2).width(Fill))
            .id(Id::new("console"))
            .anchor_bottom()
            .spacing(6)
            .style(theme::scroll)
            .width(Fill)
            .height(Fill)
            .into()
    } else {
        container(kit::small("Launcher and game output appears here.")).center(Fill).into()
    };
    column![bar, container(body).padding(theme::S3).width(Fill).height(Fill).style(theme::well)]
        .spacing(theme::S2)
        .height(Fill)
        .into()
}
