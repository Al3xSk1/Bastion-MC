//! The front end against the real launcher and the real game. Opt-in, because it needs an
//! installed instance, downloads nothing only if that instance is complete, and opens a
//! game window:
//!
//! ```text
//! BASTION_GUI_LIVE_LAUNCHER=target/debug/bastion-mc BASTION_GUI_LIVE_INSTANCE=<id> \
//!     cargo test -p bastion-mc-gui --test live -- --nocapture --test-threads=1
//! ```
//!
//! (with the XDG variables pointing wherever that instance lives). The game is started in
//! demo mode, so no account is involved, and stopped again as soon as it has produced
//! output. One game at a time: hence `--test-threads=1`.
//!
//! Two tests: the backend worker alone, and the whole application — state, view and worker,
//! run by iced's emulator without a window of its own — pressing Play and Stop. The second
//! begins, like the real window, by asking the launcher for the state of the default
//! account; with the default token store that reads that account's entry in the kernel
//! keyring. Point `XDG_CONFIG_HOME` at a configuration with `auth.token_store = "none"`
//! where that is not wanted.

mod support;

use std::{path::PathBuf, time::Duration};

use bastion_mc_gui::{
    Options, application,
    backend::{self, Outcome, Request, Update},
    theme::caps,
};
use bastion_mc_proto::{Event, Phase, Source};
use support::Session;

/// What the tests need from the environment; `None` (and a note) when they are not asked for.
fn asked_for() -> Option<(PathBuf, String)> {
    match (std::env::var_os("BASTION_GUI_LIVE_LAUNCHER"), std::env::var("BASTION_GUI_LIVE_INSTANCE").ok()) {
        (Some(launcher), Some(instance)) => Some((PathBuf::from(launcher), instance)),
        _ => {
            eprintln!("skipped: set BASTION_GUI_LIVE_LAUNCHER and BASTION_GUI_LIVE_INSTANCE to run");
            None
        }
    }
}

/// A real client takes its time to verify its files and to come up.
const LAUNCH_PATIENCE: Duration = Duration::from_secs(240);

#[test]
fn the_application_plays_and_stops_a_real_demo_game() {
    let Some((launcher, instance)) = asked_for() else { return };
    let turn = support::one_at_a_time();
    let mut ui = Session::boot(application(Options { launcher, account: String::new() }), turn);
    ui.idle();

    // Choose the instance on its page, then demo mode and Play on the launch page.
    ui.click(&caps("Instances"));
    ui.eventually(&instance);
    ui.click(&instance);
    ui.click(&caps("Launch"));
    assert!(ui.shows(&caps("No launch yet")), "nothing is claimed about a launch that has not happened");
    ui.click("Demo mode (no account)");
    assert!(ui.shows("demo mode"));
    ui.click(&caps("Play"));
    ui.eventually_within("The game is running.", LAUNCH_PATIENCE);
    eprintln!("the game is running");

    // What the launcher reported about this launch is on screen: all five aspects, each
    // with a level.
    assert!(!ui.shows(&caps("No launch yet")) && !ui.shows(&caps("No report")));
    for aspect in ["display", "network", "credential", "limits", "launcher"] {
        assert!(ui.shows(&caps(aspect)), "{aspect}");
    }
    assert!(ui.shows("no account and no token in this session"), "demo mode: the game holds nothing of an account");
    assert!(ui.shows(&caps("Running")) && ui.shows(&caps("Game running")));

    // The console has the launcher's and the game's output.
    ui.click(&caps("Console"));
    ui.eventually("auth: demo mode — no account, no token");
    assert!(!ui.shows("Launcher and game output appears here."));

    // Stop is its own control; the launch control says how it ended and is offered again.
    ui.click(&caps("Launch"));
    ui.click(&caps("Stop"));
    ui.eventually_within("The game was stopped.", LAUNCH_PATIENCE);
    ui.idle();
    assert!(ui.shows(&caps("Play")));
    eprintln!("ok: played and stopped {instance} through the window's own controls");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_launch_is_reported_phase_by_phase_and_stops_on_request() {
    let Some((launcher, instance)) = asked_for() else { return };
    let request = Request::Launch { instance, account: "default".into(), demo: true };
    let mut updates = backend::spawn(&launcher, &request);

    let (mut handle, mut phases, mut game_lines, mut egress_lines, mut exited) = (None, Vec::new(), 0u32, 0u32, None);
    let mut stop_sent = false;
    let outcome = loop {
        let update = tokio::time::timeout(Duration::from_secs(300), updates.recv())
            .await
            .expect("no update for five minutes")
            .expect("the worker always ends with Finished");
        match update {
            Update::Started(h) => handle = Some(h),
            Update::Event(Event::Phase { phase }) => {
                eprintln!("phase: {phase:?}");
                phases.push(phase);
            }
            Update::Event(Event::Progress { label, done, total, .. }) => eprintln!("progress: {label} {done}/{total}"),
            Update::Event(Event::Log { source: Source::Game, line }) => {
                game_lines += 1;
                if game_lines <= 5 {
                    eprintln!("game: {line}");
                }
                // The client is up and talking: that is all this test needs from it.
                if game_lines >= 25 && !stop_sent {
                    stop_sent = true;
                    eprintln!("stopping after {game_lines} lines of game output");
                    handle.as_ref().expect("Started comes first").stop();
                }
            }
            Update::Event(Event::Log { source: Source::Egress, .. }) => egress_lines += 1,
            Update::Event(Event::Exited { code }) => exited = Some(code),
            Update::Event(Event::Error { message, code }) => panic!("launcher error {code}: {message}"),
            Update::Event(_) => {}
            Update::Stray(text) => eprintln!("stray: {text}"),
            Update::Finished(outcome) => break outcome,
        }
    };

    assert_eq!(
        phases,
        vec![Phase::Preparing, Phase::Verifying, Phase::Starting, Phase::Running, Phase::Stopping],
        "demo mode has no authentication phase"
    );
    assert!(game_lines >= 25, "only {game_lines} lines of game output");
    assert_eq!(exited, Some(130), "stopped on request");
    assert_eq!(outcome, Outcome::Exited(130));
    eprintln!("ok: {game_lines} game lines, {egress_lines} egress lines");
}
