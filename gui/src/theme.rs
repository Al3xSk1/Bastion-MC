//! The look: one dark "workshop" theme — gunmetal surfaces, thin luminous line work.
//!
//! Three rules hold the design together, and everything in [`crate::view`] follows them:
//!
//! * **One accent, two alarms.** [`ACCENT`] (a cool cyan) means "in force / selected /
//!   proceed". [`AMBER`] means caution, [`RED`] means danger or "not protected". No other
//!   hue is used, so a colour on screen always says one of those three things. Colour is
//!   never the only carrier: every state also has a word, and the drawn marks differ in
//!   shape (see [`crate::hud`]).
//! * **Lines, not cards.** Surfaces are flat and nearly black; structure comes from hairlines
//!   and cut corners ([`crate::hud::Frame`]). Nothing is rounded except the launch control.
//! * **Text first.** Body text is [`TEXT`] on [`PLATE`] (contrast about 14:1); the faintest
//!   text used for anything a person has to read, [`TEXT_FAINT`], stays above 4.5:1. A unit
//!   test recomputes those ratios, so the palette cannot drift into something unreadable.
//!
//! Fonts: nothing is bundled. Text uses [`UI_FONT`] where it is installed and whatever
//! sans-serif the system has otherwise; read-outs use the system's monospace face. The
//! layout never depends on the metrics of one particular font, and only on glyphs every
//! text font has (letters, digits, punctuation, the dash, the ellipsis and the six-per-em
//! space used for tracking): every symbol on screen is drawn as geometry.
//!
//! The theme is fixed rather than following the desktop setting: the launcher has no light
//! mode, and not asking the desktop means no D-Bus client in this process.

use iced::{
    Background, Border, Color, Font, Shadow, Theme, Vector, border, color,
    overlay::menu,
    theme::Palette,
    widget::{button, container, pick_list, progress_bar, rule, scrollable, text_input},
};

use crate::app::Severity;

// ---------------------------------------------------------------------------- palette

/// The window: near-black gunmetal.
pub const VOID: Color = color!(0x0a0d10);
/// The navigation rail, a shade below the window.
pub const DECK: Color = color!(0x070a0c);
/// Panels.
pub const PLATE: Color = color!(0x10161b);
/// A panel that is selected or under the pointer.
pub const PLATE_HI: Color = color!(0x16202a);
/// Recessed areas: the console, the device code, input fields.
pub const WELL: Color = color!(0x05080a);
/// Hairlines.
pub const LINE: Color = color!(0x1f2b35);
/// Hairlines that have to be seen: scales, outlines of controls.
pub const LINE_HI: Color = color!(0x33485a);

/// In force, selected, go.
pub const ACCENT: Color = color!(0x3fd9e6);
/// Caution: weaker than it could be.
pub const AMBER: Color = color!(0xf5b041);
/// Danger: not protected, failed, destructive.
pub const RED: Color = color!(0xff5f56);

pub const TEXT: Color = color!(0xdbe6ec);
/// Secondary text.
pub const TEXT_DIM: Color = color!(0x9bb0bd);
/// Labels and captions: the faintest text that still has to be read.
pub const TEXT_FAINT: Color = color!(0x738a9a);
/// Controls that are shown but cannot be used right now.
pub const TEXT_OFF: Color = color!(0x4a5c69);

/// `color` at `alpha`: the dim and glowing variants of the three signal colours.
pub const fn alpha(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

/// What a coloured element is saying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Nothing in particular.
    Neutral,
    /// In force, selected, the way forward.
    Accent,
    Caution,
    Danger,
}

impl Tone {
    pub const fn color(self) -> Color {
        match self {
            Tone::Neutral => TEXT_DIM,
            Tone::Accent => ACCENT,
            Tone::Caution => AMBER,
            Tone::Danger => RED,
        }
    }

    /// The colour of a line drawn in this tone.
    pub const fn line(self) -> Color {
        match self {
            Tone::Neutral => LINE_HI,
            other => other.color(),
        }
    }
}

// ---------------------------------------------------------------------------- type

/// Preferred text face: GNOME's interface font, which Arch installs with GTK. Where it is
/// missing the text system falls back to the sans-serif faces that are there; nothing in
/// the layout assumes this one.
pub const UI_FONT: Font = Font::with_name("Adwaita Sans");
/// Read-outs, identifiers, the console: the system's monospace face.
pub const MONO: Font = Font::MONOSPACE;

/// Tracked capitals.
pub const SIZE_LABEL: f32 = 11.0;
pub const SIZE_SMALL: f32 = 12.0;
pub const SIZE_BODY: f32 = 14.0;
pub const SIZE_TITLE: f32 = 17.0;
/// Large read-outs: the device code.
pub const SIZE_DISPLAY: f32 = 34.0;

/// The space put between the letters of a label: a sixth of an em in every font.
const TRACKING: char = '\u{2006}';

/// Upper case with letter spacing — the form of every label and button caption.
///
/// The toolkit has no letter-spacing setting, so the spacing is a character: one
/// six-per-em space between neighbouring characters. Tests look captions up through this
/// same function.
pub fn caps(label: &str) -> String {
    let mut out = String::with_capacity(label.len() * 4);
    for (i, c) in label.chars().flat_map(char::to_uppercase).enumerate() {
        if i > 0 {
            out.push(TRACKING);
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------- spacing

/// The spacing scale. Everything is a multiple of four.
pub const S1: f32 = 4.0;
pub const S2: f32 = 8.0;
pub const S3: f32 = 12.0;
pub const S4: f32 = 16.0;
pub const S5: f32 = 24.0;

// ---------------------------------------------------------------------------- theme

pub fn theme() -> Theme {
    // The stock widget styles are all replaced below; this palette only keeps anything
    // that is not (a tooltip, a future widget) in the same colours.
    Theme::custom(
        "Bastion Workshop",
        Palette { background: VOID, text: TEXT, primary: ACCENT, success: ACCENT, warning: AMBER, danger: RED },
    )
}

fn flat(background: Color) -> container::Style {
    container::Style { background: Some(Background::Color(background)), ..container::Style::default() }
}

fn outlined(background: Color, line: Color) -> container::Style {
    container::Style { border: Border { color: line, width: 1.0, radius: 0.0.into() }, ..flat(background) }
}

/// The window behind everything.
pub fn stage(_theme: &Theme) -> container::Style {
    flat(VOID)
}

/// The navigation rail.
pub fn rail(_theme: &Theme) -> container::Style {
    flat(DECK)
}

/// Recessed areas: the console, the device code, the runbook text.
pub fn well(_theme: &Theme) -> container::Style {
    outlined(WELL, LINE)
}

/// A one-pixel line as a container (a rule that can sit in a row or a stack).
pub fn hairline(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| flat(color)
}

/// A banner for something the user must not miss: a tinted plate with a solid edge.
pub fn notice(severity: Severity) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let signal = severity.tone().color();
        outlined(blend(PLATE, signal, 0.10), alpha(signal, 0.55))
    }
}

/// A small outlined tag: "DEFAULT", "SIGNED IN".
pub fn chip(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let signal = tone.color();
        container::Style { text_color: Some(signal), ..outlined(blend(PLATE, signal, 0.08), alpha(signal, 0.5)) }
    }
}

/// `over` laid on `base` at `amount`, as an opaque colour (borders drawn over a
/// translucent fill would otherwise show the fill through).
pub fn blend(base: Color, over: Color, amount: f32) -> Color {
    let mix = |a: f32, b: f32| a + (b - a) * amount;
    Color::from_rgb(mix(base.r, over.r), mix(base.g, over.g), mix(base.b, over.b))
}

pub fn rule_line(color: Color) -> impl Fn(&Theme) -> rule::Style {
    move |_| rule::Style { color, radius: 0.0.into(), fill_mode: rule::FillMode::Full, snap: true }
}

// ---------------------------------------------------------------------------- buttons

fn button_style(text: Color, background: Option<Color>, line: Option<Color>) -> button::Style {
    button::Style {
        background: background.map(Background::Color),
        text_color: text,
        border: Border {
            color: line.unwrap_or(Color::TRANSPARENT),
            width: if line.is_some() { 1.0 } else { 0.0 },
            radius: 0.0.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// An outlined button in `tone`. `solid` fills it: the one decisive action of a view.
///
/// A disabled button keeps its place and its caption — it is shown, not offered — and
/// loses its colour.
pub fn action(tone: Tone, solid: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let signal = tone.line();
        let caption = if tone == Tone::Neutral { TEXT } else { signal };
        match status {
            button::Status::Disabled => button_style(TEXT_OFF, None, Some(LINE)),
            _ if solid => {
                let fill = match status {
                    button::Status::Hovered => blend(signal, Color::WHITE, 0.14),
                    button::Status::Pressed => blend(signal, VOID, 0.2),
                    _ => signal,
                };
                button_style(VOID, Some(fill), Some(fill))
            }
            button::Status::Hovered => button_style(caption, Some(blend(PLATE, signal, 0.22)), Some(signal)),
            button::Status::Pressed => button_style(caption, Some(blend(PLATE, signal, 0.34)), Some(signal)),
            _ => button_style(caption, Some(blend(PLATE, signal, 0.08)), Some(alpha(signal, 0.6))),
        }
    }
}

/// A caption that can be pressed, with no outline: secondary actions inside a row.
pub fn link(tone: Tone) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let signal = if tone == Tone::Neutral { TEXT_DIM } else { tone.color() };
        match status {
            button::Status::Disabled => button_style(TEXT_OFF, None, None),
            button::Status::Hovered => button_style(blend(signal, Color::WHITE, 0.3), Some(PLATE_HI), None),
            button::Status::Pressed => button_style(signal, Some(LINE), None),
            _ => button_style(signal, None, None),
        }
    }
}

/// An entry of the navigation rail.
pub fn nav(current: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match (current, status) {
        (true, _) => button_style(ACCENT, Some(PLATE_HI), None),
        (false, button::Status::Hovered) => button_style(TEXT, Some(PLATE), None),
        (false, button::Status::Pressed) => button_style(TEXT, Some(PLATE_HI), None),
        (false, _) => button_style(TEXT_DIM, None, None),
    }
}

/// A whole row or card that can be pressed (an instance, an account).
pub fn row_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered | button::Status::Pressed if !selected => button_style(TEXT, Some(PLATE_HI), None),
        _ => button_style(TEXT, None, None),
    }
}

/// The launch control: the one round thing in the window, in the middle of the gauge.
pub fn core(tone: Tone) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let signal = tone.line();
        let (text, fill, line) = match status {
            button::Status::Disabled => {
                (if tone == Tone::Neutral { TEXT_OFF } else { signal }, WELL, alpha(signal, 0.45))
            }
            button::Status::Hovered => (VOID, signal, signal),
            button::Status::Pressed => (VOID, blend(signal, VOID, 0.2), signal),
            button::Status::Active => (signal, blend(WELL, signal, 0.14), signal),
        };
        button::Style {
            background: Some(Background::Color(fill)),
            text_color: text,
            border: Border { color: line, width: 1.5, radius: border::Radius::new(1000.0) },
            shadow: Shadow::default(),
            snap: false,
        }
    }
}

// ---------------------------------------------------------------------------- inputs

pub fn input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let line = match status {
        text_input::Status::Focused { .. } => ACCENT,
        text_input::Status::Hovered => LINE_HI,
        _ => LINE,
    };
    text_input::Style {
        background: Background::Color(WELL),
        border: Border { color: line, width: 1.0, radius: 0.0.into() },
        icon: TEXT_FAINT,
        placeholder: TEXT_FAINT,
        value: if status == text_input::Status::Disabled { TEXT_OFF } else { TEXT },
        selection: alpha(ACCENT, 0.35),
    }
}

pub fn picker(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let line = match status {
        pick_list::Status::Opened { .. } => ACCENT,
        pick_list::Status::Hovered => LINE_HI,
        pick_list::Status::Active => LINE,
    };
    pick_list::Style {
        text_color: TEXT,
        placeholder_color: TEXT_FAINT,
        handle_color: ACCENT,
        background: Background::Color(WELL),
        border: Border { color: line, width: 1.0, radius: 0.0.into() },
    }
}

pub fn picker_menu(_theme: &Theme) -> menu::Style {
    menu::Style {
        background: Background::Color(PLATE),
        border: Border { color: LINE_HI, width: 1.0, radius: 0.0.into() },
        text_color: TEXT,
        selected_text_color: VOID,
        selected_background: Background::Color(ACCENT),
        shadow: Shadow { color: alpha(Color::BLACK, 0.6), offset: Vector::new(0.0, 4.0), blur_radius: 12.0 },
    }
}

/// A switch is drawn from two rectangles: a track and a block that sits left (off) or
/// right (on). Position says the state; colour only repeats it.
pub fn switch_track(on: bool, enabled: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| match (on, enabled) {
        (true, true) => outlined(blend(WELL, ACCENT, 0.18), alpha(ACCENT, 0.6)),
        (true, false) => outlined(WELL, alpha(ACCENT, 0.3)),
        (false, true) => outlined(WELL, LINE_HI),
        (false, false) => outlined(WELL, LINE),
    }
}

pub fn switch_block(on: bool, enabled: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        flat(match (on, enabled) {
            (true, true) => ACCENT,
            (true, false) => alpha(ACCENT, 0.4),
            (false, true) => TEXT_DIM,
            (false, false) => TEXT_OFF,
        })
    }
}

/// A button that is only its content: the switch and its caption.
pub fn bare(_theme: &Theme, status: button::Status) -> button::Style {
    button_style(if status == button::Status::Disabled { TEXT_OFF } else { TEXT }, None, None)
}

pub fn bar(_theme: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(LINE),
        bar: Background::Color(ACCENT),
        border: Border::default(),
    }
}

/// Scrolling areas: a hairline track, a thin block that lights up under the pointer.
pub fn scroll(_theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let rail = |active: bool| scrollable::Rail {
        background: Some(Background::Color(alpha(LINE, 0.6))),
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(if active { ACCENT } else { LINE_HI }),
            border: Border::default(),
        },
    };
    let (vertical, horizontal) = match status {
        scrollable::Status::Active { .. } => (false, false),
        scrollable::Status::Hovered { is_vertical_scrollbar_hovered, is_horizontal_scrollbar_hovered, .. } => {
            (is_vertical_scrollbar_hovered, is_horizontal_scrollbar_hovered)
        }
        scrollable::Status::Dragged { is_vertical_scrollbar_dragged, is_horizontal_scrollbar_dragged, .. } => {
            (is_vertical_scrollbar_dragged, is_horizontal_scrollbar_dragged)
        }
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail(vertical),
        horizontal_rail: rail(horizontal),
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(alpha(PLATE, 0.9)),
            border: Border { color: LINE_HI, width: 1.0, radius: 0.0.into() },
            shadow: Shadow::default(),
            icon: TEXT_DIM,
        },
    }
}

impl Severity {
    pub const fn tone(self) -> Tone {
        match self {
            Severity::Error => Tone::Danger,
            Severity::Warning => Tone::Caution,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2 relative luminance of an opaque sRGB colour.
    fn luminance(color: Color) -> f32 {
        let linear = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_keeps_its_contrast_on_every_surface() {
        for surface in [VOID, DECK, PLATE, PLATE_HI, WELL] {
            assert!(contrast(TEXT, surface) >= 12.0, "body text on {surface:?}: {}", contrast(TEXT, surface));
            assert!(contrast(TEXT_DIM, surface) >= 7.0, "secondary text on {surface:?}");
            // 4.5:1 is the WCAG AA figure for small text; labels are small.
            assert!(contrast(TEXT_FAINT, surface) >= 4.5, "labels on {surface:?}: {}", contrast(TEXT_FAINT, surface));
            for signal in [ACCENT, AMBER, RED] {
                assert!(contrast(signal, surface) >= 5.0, "{signal:?} on {surface:?}: {}", contrast(signal, surface));
            }
        }
        // A solid button carries dark text on the signal colour.
        for signal in [ACCENT, AMBER, RED] {
            assert!(contrast(VOID, signal) >= 5.0, "caption on a solid {signal:?} button");
        }
        // Text on a notice or a chip sits on a tinted plate.
        for signal in [AMBER, RED, ACCENT] {
            assert!(contrast(TEXT, blend(PLATE, signal, 0.10)) >= 10.0);
            assert!(contrast(signal, blend(PLATE, signal, 0.08)) >= 4.5);
        }
    }

    #[test]
    fn the_three_signal_colours_differ_in_lightness_as_well_as_hue() {
        // Someone who cannot tell the hues apart still sees three different greys (and the
        // interface never relies on that alone: every state has a word and a shape).
        let (accent, amber, red) = (luminance(ACCENT), luminance(AMBER), luminance(RED));
        assert!(accent > amber && amber > red, "{accent} {amber} {red}");
        assert!(amber - red > 0.1, "caution and danger must not look alike");
    }

    #[test]
    fn labels_are_upper_case_with_one_tracking_space_between_characters() {
        assert_eq!(caps("Play"), "P\u{2006}L\u{2006}A\u{2006}Y");
        assert_eq!(caps("sign in"), "S\u{2006}I\u{2006}G\u{2006}N\u{2006} \u{2006}I\u{2006}N");
        assert_eq!(caps(""), "");
        assert_eq!(caps("a"), "A");
        // Characters whose upper case is longer still come out one by one.
        assert_eq!(caps("ß"), "S\u{2006}S");
    }
}
