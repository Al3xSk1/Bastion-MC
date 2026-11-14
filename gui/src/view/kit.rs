//! The parts every page is built from: labels, panels, buttons, tags.
//!
//! Pages do not style widgets themselves; they pick from here. That is what keeps the five
//! pages looking like one instrument, and it keeps the rules in one place: a caption is
//! always tracked capitals ([`theme::caps`]), a control that cannot be used is still drawn
//! (a button with no message), a panel is always a cut plate.
//!
//! One rule is about room: **nothing is drawn outside the place it was given.** The toolkit
//! does not see to that by itself — a line of text that cannot be broken is drawn right
//! across whatever is beside it. So running text ([`body`], [`small`], [`mono`]) breaks
//! inside a word when a word is longer than the line, and text that has to stay on one
//! line is either the program's own (short, and measured by the tests at every window
//! size) or cut to a number of characters that fits ([`mono_chars`]).

use bastion_mc_proto::Shield;
use std::fmt;

use iced::{
    Center, Color, Element, Fill, Length, Padding,
    widget::{
        Button, Container, Space, Stack, Text, button, canvas, column, container, pick_list, responsive, row, space,
        text,
    },
};

use crate::{
    app::Message,
    hud,
    theme::{self, Tone},
};

/// Tracked capitals in the label colour: what names a thing.
pub fn label<'a>(caption: &str) -> Text<'a> {
    label_in(caption, theme::TEXT_FAINT)
}

pub fn label_in<'a>(caption: &str, color: Color) -> Text<'a> {
    text(theme::caps(caption)).size(theme::SIZE_LABEL).color(color).wrapping(text::Wrapping::None)
}

/// Text that runs over as many lines as it needs. A word that is longer than a line — a
/// path, an address, anything the launcher passes on — is broken rather than drawn over
/// what stands beside it.
pub fn wrapped<'a>(content: impl text::IntoFragment<'a>) -> Text<'a> {
    text(content).wrapping(text::Wrapping::WordOrGlyph)
}

pub fn body<'a>(content: impl text::IntoFragment<'a>) -> Text<'a> {
    wrapped(content).size(theme::SIZE_BODY).color(theme::TEXT)
}

/// Secondary text: explanations, details.
pub fn small<'a>(content: impl text::IntoFragment<'a>) -> Text<'a> {
    wrapped(content).size(theme::SIZE_SMALL).color(theme::TEXT_DIM)
}

/// A read-out: identifiers, numbers, addresses.
pub fn mono<'a>(content: impl text::IntoFragment<'a>) -> Text<'a> {
    wrapped(content).size(theme::SIZE_SMALL).font(theme::MONO).color(theme::TEXT)
}

/// The widest a character of a monospace face is taken to be, in ems. Nearly all of them
/// are 0.6 em wide; this leaves a margin. A test measures the installed face against it.
pub const MONO_ADVANCE: f32 = 0.62;

/// How many characters of the monospace face fit on one line `room` wide at `size`: for
/// text that must stay on one line and comes from outside the program. Where the face is
/// wider than [`MONO_ADVANCE`] says, such a line can still be too long for its place; it
/// is then cut off by the frame it stands in, not drawn across its neighbours.
pub fn mono_chars(room: f32, size: f32) -> usize {
    (room / (MONO_ADVANCE * size)).floor().max(0.0) as usize
}

// ---------------------------------------------------------------------------- drop-downs

const PICKER_SIZE: f32 = theme::SIZE_SMALL + 1.0;
const PICKER_PADDING: [f32; 2] = [8.0, 10.0];
/// What a drop-down's line holds besides its label: the padding on both sides, the arrow
/// and a gap before it.
const PICKER_CHROME: f32 = 2.0 * PICKER_PADDING[1] + 22.0;

/// The room a drop-down that is `width` wide has for a label, in characters.
fn picker_chars(width: f32) -> usize {
    mono_chars(width - PICKER_CHROME, PICKER_SIZE).max(12)
}

/// One entry of a [`picker`]: the option it stands for, and what is shown for it.
#[derive(Debug, Clone)]
struct Entry {
    at: usize,
    label: String,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at
    }
}

impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// Hands `content` the number of characters a drop-down has room for in this place, and
/// stands for what it returns.
pub fn with_picker_room<'a>(content: impl Fn(usize) -> Element<'a, Message> + 'a) -> Element<'a, Message> {
    responsive(move |room| content(picker_chars(room.width))).height(Length::Shrink).into()
}

/// A drop-down list as wide as its place.
///
/// The toolkit draws the label of an entry on one line, however long it is: a long one
/// runs out of the box and across what is beside it. So the labels are made to fit:
/// `label` is told how many characters there is room for (`chars`, from
/// [`with_picker_room`]), and what it returns is shown.
pub fn picker<'a, T: Clone + PartialEq + 'a>(
    options: &'a [T],
    selected: Option<&'a T>,
    placeholder: &'a str,
    chars: usize,
    label: impl Fn(&T, usize) -> String,
    on_select: impl Fn(T) -> Message + 'a,
) -> Element<'a, Message> {
    let entries: Vec<Entry> =
        options.iter().enumerate().map(|(at, option)| Entry { at, label: label(option, chars) }).collect();
    // A choice that is no longer among the options is still shown; it equals none of them.
    let current = selected.map(|selected| Entry {
        at: options.iter().position(|option| option == selected).unwrap_or(usize::MAX),
        label: label(selected, chars),
    });
    // `at` is an index into the very slice the entries were just made from.
    pick_list(entries, current, move |entry: Entry| on_select(options[entry.at].clone()))
        .placeholder(placeholder)
        .width(Fill)
        .padding(PICKER_PADDING)
        .text_size(PICKER_SIZE)
        .font(theme::MONO)
        .style(theme::picker)
        .menu_style(theme::picker_menu)
        .into()
}

/// What stands in the place of a [`picker`] while its choice cannot be changed: the same
/// box and the same label, without the list.
pub fn picked<'a>(label: String) -> Element<'a, Message> {
    well(mono(label).size(PICKER_SIZE).wrapping(text::Wrapping::None)).padding(PICKER_PADDING).into()
}

// ---------------------------------------------------------------------------- panels

/// `content` on a drawn frame that takes its size from the content.
pub fn framed<'a>(frame: hud::Frame, content: impl Into<Element<'a, Message>>) -> Stack<'a, Message> {
    Stack::new().push(content).push_under(canvas(frame).width(Fill).height(Fill))
}

const PANEL_PADDING: Padding = Padding { top: 12.0, right: 16.0, bottom: 16.0, left: 16.0 };
/// The heading line of every panel is this high whatever stands in it, so that the
/// headings of panels that stand side by side are on one line.
const HEADING_HEIGHT: f32 = 26.0;

/// A titled plate as wide as its place allows.
pub fn panel<'a>(title: &str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    panel_with(hud::Frame::plate(), label(title), None, content)
}

/// A panel with its frame, its heading and something at the right end of the heading
/// chosen by the caller.
pub fn panel_with<'a>(
    frame: hud::Frame,
    heading: impl Into<Element<'a, Message>>,
    trailing: Option<Element<'a, Message>>,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    build_panel(frame, heading.into(), trailing, content.into(), false)
}

/// [`panel_with`], as high as its place allows: for panels that have to end on one line
/// with their neighbours. Only for places whose height is known.
pub fn panel_filling<'a>(
    frame: hud::Frame,
    heading: impl Into<Element<'a, Message>>,
    trailing: Option<Element<'a, Message>>,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    build_panel(frame, heading.into(), trailing, content.into(), true)
}

fn build_panel<'a>(
    frame: hud::Frame,
    heading: Element<'a, Message>,
    trailing: Option<Element<'a, Message>>,
    content: Element<'a, Message>,
    filling: bool,
) -> Element<'a, Message> {
    let mut head = row![heading, space().width(Fill)].spacing(theme::S2).align_y(Center).height(HEADING_HEIGHT);
    if let Some(trailing) = trailing {
        head = head.push(trailing);
    }
    let height = if filling { Fill } else { Length::Shrink };
    let inner = column![head, content].spacing(theme::S2).height(height);
    framed(frame, container(inner).padding(PANEL_PADDING).width(Fill).height(height)).into()
}

/// A recessed area.
pub fn well<'a>(content: impl Into<Element<'a, Message>>) -> Container<'a, Message> {
    container(content).padding(theme::S3).width(Fill).style(theme::well)
}

/// A one-pixel horizontal line.
pub fn rule<'a>() -> Element<'a, Message> {
    container(Space::new()).width(Fill).height(1).style(theme::hairline(theme::LINE)).into()
}

/// A caption for a button: tracked capitals, a little smaller than body text.
fn caption<'a>(words: &str) -> Text<'a> {
    text(theme::caps(words)).size(theme::SIZE_LABEL).wrapping(text::Wrapping::None)
}

/// An outlined button. `on_press` of `None` draws it without offering it.
pub fn action<'a>(words: &str, tone: Tone, on_press: Option<Message>) -> Button<'a, Message> {
    button(caption(words)).padding([7, 12]).style(theme::action(tone, false)).on_press_maybe(on_press)
}

/// An outlined button for the heading line of a panel: lower than [`action`], so that it
/// fits the height every heading has.
pub fn tool<'a>(words: &str, tone: Tone, on_press: Option<Message>) -> Button<'a, Message> {
    button(caption(words)).padding([5, 10]).style(theme::action(tone, false)).on_press_maybe(on_press)
}

/// The decisive action of a view: filled.
pub fn solid<'a>(words: &str, tone: Tone, on_press: Option<Message>) -> Button<'a, Message> {
    button(caption(words)).padding([7, 12]).style(theme::action(tone, true)).on_press_maybe(on_press)
}

/// A caption that can be pressed: lesser actions (copy, dismiss).
pub fn link<'a>(words: &str, tone: Tone, on_press: Option<Message>) -> Button<'a, Message> {
    button(caption(words)).padding([5, 8]).style(theme::link(tone)).on_press_maybe(on_press)
}

/// A switch with its caption; pressing either flips it. `on_flip` of `None` draws it
/// without offering it.
pub fn switch<'a>(words: &'a str, on: bool, on_flip: Option<Message>) -> Button<'a, Message> {
    let enabled = on_flip.is_some();
    let block = container(Space::new()).width(14).height(12).style(theme::switch_block(on, enabled));
    let track = container(block)
        .width(36)
        .height(18)
        .padding(2)
        .align_x(if on { iced::Right } else { iced::Left })
        .style(theme::switch_track(on, enabled));
    button(row![track, text(words).size(theme::SIZE_BODY - 1.0)].spacing(theme::S2).align_y(Center))
        .padding(0)
        .style(theme::bare)
        .on_press_maybe(on_flip)
}

/// A small outlined tag that states a fact.
pub fn chip<'a>(words: &str, tone: Tone) -> Element<'a, Message> {
    container(text(theme::caps(words)).size(10).wrapping(text::Wrapping::None))
        .padding([2, 6])
        .style(theme::chip(tone))
        .into()
}

/// A small square that says "this one": filled when `on`, an outline otherwise.
pub fn pip<'a>(color: Color, on: bool) -> Element<'a, Message> {
    container(Space::new())
        .width(8)
        .height(8)
        .style(move |_| container::Style {
            background: on.then_some(color.into()),
            border: iced::Border { color, width: 1.0, radius: 0.0.into() },
            ..container::Style::default()
        })
        .into()
}

pub fn shield<'a>(level: Option<Shield>) -> Element<'a, Message> {
    canvas(hud::ShieldMark(level)).width(hud::ShieldMark::WIDTH).height(hud::ShieldMark::HEIGHT).into()
}

/// The words for a shield level. Never colour alone: these are always printed next to the
/// mark.
pub fn shield_words(level: Option<Shield>) -> &'static str {
    match level {
        Some(Shield::On) => "In force",
        Some(Shield::Partial) => "Reduced",
        Some(Shield::Off) => "Not protected",
        None => "No report",
    }
}

pub fn shield_tone(level: Option<Shield>) -> Tone {
    match level {
        Some(Shield::On) => Tone::Accent,
        Some(Shield::Partial) => Tone::Caution,
        Some(Shield::Off) => Tone::Danger,
        None => Tone::Neutral,
    }
}

/// The colour of the words for a shield level.
pub fn shield_ink(level: Option<Shield>) -> Color {
    if level.is_some() { shield_tone(level).color() } else { theme::TEXT_FAINT }
}

/// What each aspect of a launch is about, in the launcher's order. The protocol names
/// these five; an aspect that is not among them is shown under its own name.
pub const ASPECTS: [(&str, &str); 5] = [
    ("display", "what the game can see and do on your desktop"),
    ("network", "where the game can connect to"),
    ("credential", "what the game holds of your account"),
    ("limits", "how much of this machine the game can use"),
    ("launcher", "what the launcher itself can still reach while the game runs"),
];

/// Aspects a launch may report besides the five: the Bedrock launch's sixth, its game's
/// sign-in pages (shown outside the game; answered only at the terminal or here).
pub const MORE_ASPECTS: [(&str, &str); 1] =
    [("sign-in", "where the game's sign-in pages are shown, and who may answer them")];

pub fn aspect_meaning(aspect: &str) -> Option<&'static str> {
    ASPECTS.iter().chain(MORE_ASPECTS.iter()).find(|(name, _)| *name == aspect).map(|(_, meaning)| *meaning)
}

/// The square with a letter that stands for an account. There is no picture of a player in
/// the launcher's events, and none is made up.
pub fn monogram<'a>(name: &str, side: f32, tone: Tone) -> Element<'a, Message> {
    let letter = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let frame = hud::Frame::toned(tone).cut(side / 5.0).fill(theme::WELL);
    framed(
        frame,
        container(text(letter).size(side * 0.46).font(theme::MONO).color(tone.color())).center(Length::Fixed(side)),
    )
    .into()
}

/// `m:ss`, for countdowns and running times.
pub fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
