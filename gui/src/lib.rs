//! Graphical front end for bastion-mc.
//!
//! Three layers, each ignorant of the one above it:
//!
//! * [`backend`] — the worker. Runs the launcher as a child process and turns its event
//!   stream into typed updates on a tokio channel. Knows nothing about widgets.
//! * [`app`] — state and logic. Folds updates and user input into [`app::App`]; decides what
//!   may be pressed. Knows nothing about layout, and is tested without a window.
//! * [`view`] — a pure function from state to widgets, styled by [`theme`].
//!
//! The launcher is deliberately *not* linked into this program. Tokens exist only in the
//! launcher process; this one holds a window, fonts and a GPU context, and nothing secret.

pub mod app;
pub mod backend;
pub mod console;
pub mod hud;
pub mod runbook;
pub mod theme;
pub mod view;

use std::path::PathBuf;

use iced::{Size, window};

pub use app::{App, Message};

/// The size the window opens with: wide enough for the launch page to stand in three
/// columns around its gauge.
pub const WINDOW_SIZE: Size = Size::new(1180.0, 720.0);
/// The smallest the window can be made. Every page is still usable at this size: the rail
/// shrinks to its marks and the pages fold into one column or scroll.
pub const WINDOW_MIN_SIZE: Size = Size::new(640.0, 520.0);

pub struct Options {
    /// The `bastion-mc` binary to drive.
    pub launcher: PathBuf,
    /// Local account label (the launcher's `--account`); empty for the launcher's default.
    pub account: String,
}

/// The application, ready to run: in a window ([`run`]) or headlessly, in a test.
pub fn application(
    options: Options,
) -> iced::Application<impl iced::Program<State = App, Message = Message, Theme = iced::Theme>> {
    iced::application(move || App::boot(options.launcher.clone(), options.account.clone()), App::update, view::root)
        .title("bastion-mc")
        .theme(theme::theme())
        .subscription(App::subscription)
        .window(window::Settings {
            size: WINDOW_SIZE,
            min_size: Some(WINDOW_MIN_SIZE),
            platform_specific: window::settings::PlatformSpecific {
                // Wayland app-id / X11 WM_CLASS; matches the desktop entry.
                application_id: "bastion-mc-gui".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        // Closing is a message: a running game is shut down properly first (see `app`).
        .exit_on_close_request(false)
}

/// Open the window and run until it is closed.
pub fn run(options: Options) -> iced::Result {
    application(options).run()
}
