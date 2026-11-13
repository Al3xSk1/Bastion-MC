//! The look: one dark theme, and the few container styles the stock widgets do not cover.
//!
//! The theme is fixed rather than following the desktop setting: the launcher has no light
//! mode, and not asking the desktop means no D-Bus client in this process.

use iced::{
    Background, Border, Color, Theme, color,
    theme::Palette,
    widget::{container, text},
};

use crate::app::Severity;

const SURFACE: Color = color!(0x1b1e25);
const SURFACE_BORDER: Color = color!(0x2b303a);
const WELL: Color = color!(0x0e1014);

pub fn theme() -> Theme {
    Theme::custom(
        "Bastion Dark",
        Palette {
            background: color!(0x14161b),
            text: color!(0xe4e7ec),
            primary: color!(0x5b9df9),
            success: color!(0x3fb96c),
            warning: color!(0xe2a93b),
            danger: color!(0xe5645b),
        },
    )
}

fn boxed(background: Color, border: Color, radius: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(background)),
        border: Border { color: border, width: 1.0, radius: radius.into() },
        ..container::Style::default()
    }
}

/// A section of the window.
pub fn card(_theme: &Theme) -> container::Style {
    boxed(SURFACE, SURFACE_BORDER, 8.0)
}

/// Recessed areas: the console, the device code.
pub fn well(_theme: &Theme) -> container::Style {
    boxed(WELL, SURFACE_BORDER, 6.0)
}

/// A banner for something the user must not miss.
pub fn notice(severity: Severity) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.palette();
        let accent = match severity {
            Severity::Error => palette.danger,
            Severity::Warning => palette.warning,
        };
        boxed(Color { a: 0.14, ..accent }, accent, 8.0)
    }
}

/// Small print.
pub fn muted(theme: &Theme) -> text::Style {
    text::Style { color: Some(Color { a: 0.62, ..theme.palette().text }) }
}

/// Section labels.
pub fn label(theme: &Theme) -> text::Style {
    text::Style { color: Some(Color { a: 0.45, ..theme.palette().text }) }
}
