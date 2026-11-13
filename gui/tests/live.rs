//! The backend worker against the real launcher and the real game. Opt-in, because it needs
//! an installed instance, downloads nothing only if that instance is complete, and opens a
//! game window:
//!
//! ```text
//! BASTION_GUI_LIVE_LAUNCHER=target/debug/bastion-mc BASTION_GUI_LIVE_INSTANCE=<id> \
//!     cargo test -p bastion-mc-gui --test live -- --nocapture
//! ```
//!
//! (with the XDG variables pointing wherever that instance lives). The game is started in
//! demo mode, so no account is involved, and stopped again as soon as it has produced output.

use std::{path::PathBuf, time::Duration};

use bastion_mc_gui::backend::{self, Outcome, Request, Update};
use bastion_mc_proto::{Event, Phase, Source};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_launch_is_reported_phase_by_phase_and_stops_on_request() {
    let (Some(launcher), Some(instance)) =
        (std::env::var_os("BASTION_GUI_LIVE_LAUNCHER"), std::env::var("BASTION_GUI_LIVE_INSTANCE").ok())
    else {
        eprintln!("skipped: set BASTION_GUI_LIVE_LAUNCHER and BASTION_GUI_LIVE_INSTANCE to run");
        return;
    };
    let request = Request::Launch { instance, account: "default".into(), demo: true };
    let mut updates = backend::spawn(&PathBuf::from(launcher), &request);

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
