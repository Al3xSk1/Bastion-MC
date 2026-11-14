//! Drawn elements: everything on screen that is neither text nor a rectangle.
//!
//! Each is a small value describing *what* to show — a [`Gauge`] holds steps, a fraction
//! and shield levels, not angles — and a `canvas` program that turns it into lines. They
//! hold no state and read no clock: motion is a function of the `time` they are given, so
//! the same value always draws the same picture (which is what lets the tests render them).
//!
//! No glyph of any font is used for a symbol. Which symbols a font has differs from machine
//! to machine; a line drawn here looks the same everywhere.
//!
//! Shape carries what colour carries, for readers who cannot tell the hues apart: a
//! protection that is in force is a solid mark, a reduced one is broken or half filled, a
//! missing one is hollow and struck through.

use std::f32::consts::{FRAC_PI_2, TAU};

use bastion_mc_proto::Shield;
use iced::{
    Color, Point, Radians, Rectangle, Renderer, Theme, mouse,
    widget::canvas::{self, Geometry, LineCap, LineJoin, Path, Stroke, path::Arc},
};

use crate::theme::{self, ACCENT, AMBER, LINE, LINE_HI, RED, TEXT_FAINT, alpha};

fn stroke<'a>(color: Color, width: f32) -> Stroke<'a> {
    Stroke::default().with_color(color).with_width(width).with_line_cap(LineCap::Butt).with_line_join(LineJoin::Miter)
}

fn polyline(points: &[Point]) -> Path {
    Path::new(|path| {
        let mut points = points.iter();
        if let Some(first) = points.next() {
            path.move_to(*first);
        }
        for point in points {
            path.line_to(*point);
        }
    })
}

fn polygon(points: &[Point]) -> Path {
    Path::new(|path| {
        let mut points = points.iter();
        if let Some(first) = points.next() {
            path.move_to(*first);
        }
        for point in points {
            path.line_to(*point);
        }
        path.close();
    })
}

/// The colour that stands for a shield level; `None` is "nothing reported".
pub fn shield_color(level: Option<Shield>) -> Color {
    match level {
        Some(Shield::On) => ACCENT,
        Some(Shield::Partial) => AMBER,
        Some(Shield::Off) => RED,
        None => LINE_HI,
    }
}

// ---------------------------------------------------------------------------- frames

/// The outline of a panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub shape: Shape,
    pub fill: Color,
    pub line: Color,
    /// The luminous corner mark; transparent for none.
    pub mark: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// A plate whose top-left and bottom-right corners are cut off by `cut`.
    Plate { cut: f32 },
    /// Four corner brackets with legs of `leg` around an area that has no outline: what
    /// the eye should go to (the device code, the selected entry).
    Brackets { leg: f32 },
}

impl Frame {
    pub const fn plate() -> Self {
        Frame { shape: Shape::Plate { cut: 10.0 }, fill: theme::PLATE, line: LINE, mark: alpha(ACCENT, 0.9) }
    }

    /// A plate that reports something in `tone` (the emergency panel, an account that is
    /// selected).
    pub const fn toned(tone: theme::Tone) -> Self {
        let signal = tone.line();
        Frame { shape: Shape::Plate { cut: 10.0 }, fill: theme::PLATE, line: alpha(signal, 0.45), mark: signal }
    }

    pub const fn brackets(color: Color) -> Self {
        Frame { shape: Shape::Brackets { leg: 12.0 }, fill: theme::WELL, line: Color::TRANSPARENT, mark: color }
    }

    pub const fn fill(self, fill: Color) -> Self {
        Frame { fill, ..self }
    }

    pub const fn cut(self, cut: f32) -> Self {
        match self.shape {
            Shape::Plate { .. } => Frame { shape: Shape::Plate { cut }, ..self },
            Shape::Brackets { .. } => self,
        }
    }
}

impl<Message> canvas::Program<Message> for Frame {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        // Lines are centred on half pixels so that a one-pixel line covers one pixel row.
        let (left, top, right, bottom) = (0.5, 0.5, bounds.width - 0.5, bounds.height - 0.5);
        match self.shape {
            Shape::Plate { cut } => {
                let cut = cut.min(bounds.width / 3.0).min(bounds.height / 3.0);
                let outline = polygon(&[
                    Point::new(left + cut, top),
                    Point::new(right, top),
                    Point::new(right, bottom - cut),
                    Point::new(right - cut, bottom),
                    Point::new(left, bottom),
                    Point::new(left, top + cut),
                ]);
                frame.fill(&outline, self.fill);
                frame.stroke(&outline, stroke(self.line, 1.0));
                if self.mark.a > 0.0 {
                    // Where the plate begins: the cut corner and a little of both edges.
                    let reach = (cut * 1.6).min(bounds.width / 4.0).min(bounds.height / 4.0);
                    let lead = polyline(&[
                        Point::new(left, top + cut + reach),
                        Point::new(left, top + cut),
                        Point::new(left + cut, top),
                        Point::new(left + cut + reach, top),
                    ]);
                    frame.stroke(&lead, stroke(self.mark, 1.5));
                    let tail = polyline(&[Point::new(right, bottom - cut), Point::new(right - cut, bottom)]);
                    frame.stroke(&tail, stroke(alpha(self.mark, self.mark.a * 0.55), 1.5));
                }
            }
            Shape::Brackets { leg } => {
                let leg = leg.min(bounds.width / 3.0).min(bounds.height / 3.0);
                frame.fill_rectangle(Point::ORIGIN, bounds.size(), self.fill);
                for (x, y, dx, dy) in [
                    (left, top, 1.0, 1.0),
                    (right, top, -1.0, 1.0),
                    (right, bottom, -1.0, -1.0),
                    (left, bottom, 1.0, -1.0),
                ] {
                    let corner =
                        polyline(&[Point::new(x + dx * leg, y), Point::new(x, y), Point::new(x, y + dy * leg)]);
                    frame.stroke(&corner, stroke(self.mark, 1.5));
                }
            }
        }
        vec![frame.into_geometry()]
    }
}

// ---------------------------------------------------------------------------- the gauge

/// One stage of the launch sequence, as the outer ring shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Pending,
    Active,
    Done,
    /// The launcher went past it without doing it (no sign-in in demo mode).
    Skipped,
}

/// What moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// Nothing: no launch in progress.
    Still,
    /// A launch is being prepared: a highlight travels along the active step, and the
    /// progress ring turns while there is nothing to measure.
    Sweep,
    /// The game runs: every step is lit, the progress ring is closed and breathes slowly.
    Pulse,
    /// The game is being stopped: closed, amber, still.
    Halt,
}

/// The concentric-ring gauge around the launch control. From the outside in:
///
/// 1. the **sequence**: one arc per step of a launch, clockwise from the top;
/// 2. a **scale** of fifty ticks (two per cent each, a longer one every ten per cent);
/// 3. the **progress** ring, read against that scale;
/// 4. the **protection** ring: one segment per aspect of the launch — solid when in
///    force, broken when reduced, a thin double line when absent, faint when not reported.
///
/// The launch control itself is an ordinary button laid over the middle.
#[derive(Debug, Clone, PartialEq)]
pub struct Gauge {
    pub steps: Vec<Step>,
    pub motion: Motion,
    /// Measurable work, `0.0..=1.0`; `None` when there is nothing to measure.
    pub progress: Option<f32>,
    pub shields: Vec<Option<Shield>>,
    /// Seconds since the launch began. Only read while `motion` moves.
    pub time: f32,
}

impl Gauge {
    /// Side of the square the gauge is designed for; it scales to any other.
    pub const SIZE: f32 = 248.0;
    /// Diameter left free in the middle for the launch control, at [`Gauge::SIZE`].
    pub const CORE: f32 = 124.0;

    const TOP: f32 = -FRAC_PI_2;

    /// Where the highlight on the active step is, `0.0..=1.0` along the arc.
    fn sweep_position(&self) -> f32 {
        (self.time / 1.4).rem_euclid(1.0)
    }

    /// How bright the closed ring is while the game runs: a slow breath, never dark.
    fn pulse_level(&self) -> f32 {
        0.62 + 0.3 * (0.5 + 0.5 * (TAU * self.time / 4.0).cos())
    }
}

fn arc(center: Point, radius: f32, from: f32, to: f32) -> Path {
    Path::new(|path| path.arc(Arc { center, radius, start_angle: Radians(from), end_angle: Radians(to) }))
}

fn on_circle(center: Point, radius: f32, angle: f32) -> Point {
    Point::new(center.x + radius * angle.cos(), center.y + radius * angle.sin())
}

impl<Message> canvas::Program<Message> for Gauge {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let side = bounds.width.min(bounds.height);
        let unit = side / Self::SIZE;
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let signal = if self.motion == Motion::Halt { AMBER } else { ACCENT };

        // 1. The sequence.
        let radius = 119.0 * unit;
        let count = self.steps.len().max(1) as f32;
        let gap = 0.07;
        let span = TAU / count - gap;
        for (i, step) in self.steps.iter().enumerate() {
            let from = Self::TOP + gap / 2.0 + i as f32 * (span + gap);
            // The step being worked on is dim with a highlight travelling along it. One
            // that was reached and is held — the running game — is lit as a whole.
            let working = *step == Step::Active && self.motion == Motion::Sweep;
            let (color, width) = match step {
                Step::Done => (alpha(signal, 0.85), 3.0),
                Step::Active if working => (alpha(signal, 0.4), 3.0),
                Step::Active => (signal, 3.0),
                Step::Pending => (LINE_HI, 3.0),
                Step::Skipped => (LINE_HI, 1.0),
            };
            frame.stroke(&arc(center, radius, from, from + span), stroke(color, width * unit.max(0.75)));
            if working {
                let head = span * 0.28;
                let at = from + (span - head) * self.sweep_position();
                frame.stroke(&arc(center, radius, at, at + head), stroke(signal, 3.0 * unit.max(0.75)));
            }
        }
        // Where the sequence and the scale begin.
        frame.stroke(
            &polyline(&[on_circle(center, 123.0 * unit, Self::TOP), on_circle(center, 114.0 * unit, Self::TOP)]),
            stroke(TEXT_FAINT, 1.0),
        );

        // 2. The scale.
        for tick in 0..50 {
            let angle = Self::TOP + TAU * tick as f32 / 50.0;
            let (length, color) = if tick % 5 == 0 { (7.0, LINE_HI) } else { (3.5, LINE) };
            let outer = 109.0 * unit;
            frame.stroke(
                &polyline(&[on_circle(center, outer, angle), on_circle(center, outer - length * unit, angle)]),
                stroke(color, 1.0),
            );
        }

        // 3. Progress.
        let radius = 96.0 * unit;
        let width = 5.0 * unit.max(0.75);
        frame.stroke(&Path::circle(center, radius), stroke(LINE, 1.0));
        match (self.motion, self.progress) {
            (Motion::Still, _) => {}
            (Motion::Sweep, Some(fraction)) => {
                let fraction = fraction.clamp(0.0, 1.0);
                if fraction > 0.0 {
                    frame.stroke(&arc(center, radius, Self::TOP, Self::TOP + TAU * fraction), stroke(signal, width));
                }
            }
            (Motion::Sweep, None) => {
                let from = Self::TOP + TAU * (self.time / 1.8).rem_euclid(1.0);
                frame.stroke(&arc(center, radius, from, from + TAU * 0.2), stroke(signal, width));
            }
            (Motion::Pulse, _) => {
                frame.stroke(&Path::circle(center, radius), stroke(alpha(signal, self.pulse_level()), width));
            }
            (Motion::Halt, _) => frame.stroke(&Path::circle(center, radius), stroke(alpha(signal, 0.7), width)),
        }

        // 4. Protection.
        let radius = 81.0 * unit;
        let count = self.shields.len().max(1) as f32;
        let gap = 0.16;
        let span = TAU / count - gap;
        for (i, level) in self.shields.iter().enumerate() {
            let from = Self::TOP + gap / 2.0 + i as f32 * (span + gap);
            let color = shield_color(*level);
            match level {
                Some(Shield::On) => frame.stroke(&arc(center, radius, from, from + span), stroke(color, 4.5 * unit)),
                Some(Shield::Partial) => {
                    // Broken into three: there, but not whole.
                    let piece = span / 3.0;
                    for part in 0..3 {
                        let start = from + piece * part as f32;
                        frame.stroke(&arc(center, radius, start, start + piece * 0.62), stroke(color, 4.5 * unit));
                    }
                }
                Some(Shield::Off) => {
                    // Hollow: two thin lines where the solid segment would be.
                    for offset in [-1.75, 1.75] {
                        frame.stroke(
                            &arc(center, radius + offset * unit, from, from + span),
                            stroke(color, 1.0_f32.max(unit)),
                        );
                    }
                }
                None => frame.stroke(&arc(center, radius, from, from + span), stroke(color, 1.0)),
            }
        }

        // The seat of the launch control.
        frame.stroke(&Path::circle(center, (Self::CORE / 2.0 + 5.0) * unit), stroke(LINE, 1.0));
        vec![frame.into_geometry()]
    }
}

// ---------------------------------------------------------------------------- small marks

/// A shield, in one of three states — or an outline when nothing was reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShieldMark(pub Option<Shield>);

impl ShieldMark {
    pub const WIDTH: f32 = 14.0;
    pub const HEIGHT: f32 = 16.0;
}

/// The outline of a shield within `width` × `height`; `half` keeps only its left side.
fn shield_path(width: f32, height: f32, half: bool) -> Path {
    let (left, right, top, bottom, middle) = (1.0, width - 1.0, 1.5, height - 1.0, width / 2.0);
    let waist = top + (bottom - top) * 0.52;
    Path::new(|path| {
        path.move_to(Point::new(left, top));
        if half {
            path.line_to(Point::new(middle, top));
            path.line_to(Point::new(middle, bottom));
        } else {
            path.line_to(Point::new(right, top));
            path.line_to(Point::new(right, waist));
            path.quadratic_curve_to(Point::new(right, bottom - 2.5), Point::new(middle, bottom));
        }
        path.quadratic_curve_to(Point::new(left, bottom - 2.5), Point::new(left, waist));
        path.close();
    })
}

impl<Message> canvas::Program<Message> for ShieldMark {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (width, height) = (bounds.width, bounds.height);
        let color = shield_color(self.0);
        let outline = shield_path(width, height, false);
        match self.0 {
            Some(Shield::On) => frame.fill(&outline, color),
            Some(Shield::Partial) => frame.fill(&shield_path(width, height, true), color),
            Some(Shield::Off) => {
                // Struck through, corner to corner.
                frame.stroke(
                    &polyline(&[Point::new(0.5, height - 1.5), Point::new(width - 0.5, 1.5)]),
                    stroke(color, 1.5),
                );
            }
            None => {}
        }
        frame.stroke(&outline, stroke(color, 1.25).with_line_join(LineJoin::Round));
        vec![frame.into_geometry()]
    }
}

/// The line drawings of the navigation rail, and the mark next to the name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub sign: Sign,
    pub color: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// A bastion in plan: a square keep inside four pointed corners.
    Brand,
    /// The gauge in small: rings around a core.
    Launch,
    /// A head and shoulders.
    Accounts,
    /// A block seen from a corner.
    Instances,
    /// A prompt.
    Console,
    /// A shield.
    Security,
}

impl Glyph {
    pub const SIZE: f32 = 18.0;
}

impl<Message> canvas::Program<Message> for Glyph {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let unit = bounds.width.min(bounds.height) / Self::SIZE;
        let at = |x: f32, y: f32| Point::new(x * unit, y * unit);
        let center = at(9.0, 9.0);
        let line = stroke(self.color, 1.5).with_line_join(LineJoin::Round);
        match self.sign {
            Sign::Brand => {
                let points: Vec<Point> = (0..8)
                    .map(|i| {
                        let radius = if i % 2 == 0 { 8.5 } else { 4.6 } * unit;
                        on_circle(center, radius, -FRAC_PI_2 + TAU * i as f32 / 8.0 + TAU / 8.0)
                    })
                    .collect();
                frame.stroke(&polygon(&points), line);
                frame.fill_rectangle(at(7.0, 7.0), iced::Size::new(4.0 * unit, 4.0 * unit), self.color);
            }
            Sign::Launch => {
                frame.stroke(&Path::circle(center, 7.5 * unit), line);
                frame.stroke(&arc(center, 4.5 * unit, -FRAC_PI_2, TAU * 0.45), line);
                frame.fill(&Path::circle(center, 1.6 * unit), self.color);
            }
            Sign::Accounts => {
                frame.stroke(&Path::circle(at(9.0, 6.0), 3.2 * unit), line);
                frame.stroke(
                    &Path::new(|path| {
                        path.move_to(at(2.5, 16.0));
                        path.quadratic_curve_to(at(2.5, 11.0), at(9.0, 11.0));
                        path.quadratic_curve_to(at(15.5, 11.0), at(15.5, 16.0));
                    }),
                    line,
                );
            }
            Sign::Instances => {
                frame.stroke(
                    &polygon(&[
                        at(9.0, 1.5),
                        at(15.5, 5.25),
                        at(15.5, 12.75),
                        at(9.0, 16.5),
                        at(2.5, 12.75),
                        at(2.5, 5.25),
                    ]),
                    line,
                );
                frame.stroke(&polyline(&[at(2.5, 5.25), at(9.0, 9.0), at(15.5, 5.25)]), line);
                frame.stroke(&polyline(&[at(9.0, 9.0), at(9.0, 16.5)]), line);
            }
            Sign::Console => {
                frame.stroke(&polyline(&[at(3.0, 4.5), at(8.0, 9.0), at(3.0, 13.5)]), line);
                frame.stroke(&polyline(&[at(9.5, 14.0), at(15.5, 14.0)]), line);
            }
            Sign::Security => frame.stroke(&shield_path(bounds.width, bounds.height, false), line),
        }
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gauge(motion: Motion, time: f32) -> Gauge {
        Gauge { steps: vec![Step::Done, Step::Active, Step::Pending], motion, progress: None, shields: vec![], time }
    }

    #[test]
    fn motion_is_a_function_of_the_time_given_and_stays_in_range() {
        for step in 0..2000 {
            let time = step as f32 * 0.037;
            let moving = gauge(Motion::Sweep, time);
            assert!((0.0..=1.0).contains(&moving.sweep_position()), "{time}");
            let level = gauge(Motion::Pulse, time).pulse_level();
            assert!((0.6..=0.93).contains(&level), "the running ring never goes dark or flashes: {level}");
        }
        assert_eq!(gauge(Motion::Sweep, 0.35).sweep_position(), gauge(Motion::Sweep, 0.35).sweep_position());
        // A clock that has not started, or went backwards, is still drawn somewhere sensible.
        assert!((0.0..=1.0).contains(&gauge(Motion::Sweep, -3.2).sweep_position()));
    }

    #[test]
    fn each_shield_level_has_its_own_colour_and_none_is_the_accent_by_default() {
        let all = [Some(Shield::On), Some(Shield::Partial), Some(Shield::Off), None].map(shield_color);
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_ne!(shield_color(None), ACCENT, "an aspect nobody reported on is not shown as protected");
    }
}
