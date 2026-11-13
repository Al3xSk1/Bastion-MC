//! The whole application, run for real but without a display: iced's emulator boots it,
//! executes its tasks on the real executor, clicks widgets and reads what is on screen.
//!
//! Only the launcher is a stand-in (a script that plays back a session), so this checks the
//! part no unit test can: that events travel from a child process, through the tokio worker
//! and iced's task machinery, into the state and onto the screen — and that a click on Play
//! starts exactly the launcher command it should.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bastion_mc_gui::{Options, application};
use iced::{
    Size,
    futures::{StreamExt, channel::mpsc, executor},
};
use iced_test::{
    Emulator, Instruction,
    emulator::{Event, Mode},
};

const FAKE_LAUNCHER: &str = r#"#!/bin/sh
[ "$1" = "--events" ] || exit 99
shift
echo '{"event":"hello","protocol":1,"version":"0.0.0"}'
case "$1" in
  status)
    echo '{"event":"account","account":"default","signed_in":true,"player":"Alex","store":"kernel keyring","client_id_configured":true,"device_flow":false}'
    echo '{"event":"instances","instances":[{"id":"vanilla","minecraft":"26.3","kind":"vanilla"},{"id":"modded","minecraft":"26.3","kind":"curated","detail":"serial 2, signed by me"}]}'
    ;;
  launch)
    echo "$*" >> "$0.launches"
    echo '{"event":"phase","phase":"preparing"}'
    echo '{"event":"progress","label":"assets","done":50,"total":100,"unit":"bytes"}'
    echo '{"event":"phase","phase":"verifying"}'
    echo '{"event":"phase","phase":"running"}'
    echo '{"event":"log","source":"game","line":"hello from the stand-in game"}'
    echo '{"event":"exited","code":0}'
    ;;
  *)
    echo '{"event":"error","message":"the stand-in launcher does not know this command","code":2}'
    exit 2
    ;;
esac
"#;

fn fake_launcher() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("emulated-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bastion-mc");
    std::fs::write(&path, FAKE_LAUNCHER).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Boot the application and run `script` (iced's `.ice` instruction syntax), waiting after
/// every step until the tasks it set off — and theirs — have finished.
fn play(launcher: &Path, script: &[&str]) {
    let program = application(Options { launcher: launcher.to_path_buf(), account: "default".into() });
    let (sender, mut receiver) = mpsc::channel(1);
    let mut emulator = Emulator::new(sender, &program, Mode::Zen, Size::new(880.0, 720.0));
    // The emulator has no notion of a timeout; a hang must still end the test run.
    let finished = Arc::new(AtomicBool::new(false));
    std::thread::spawn({
        let finished = finished.clone();
        move || {
            for _ in 0..600 {
                std::thread::sleep(Duration::from_millis(100));
                if finished.load(Ordering::SeqCst) {
                    return;
                }
            }
            eprintln!("the emulated session did not finish within 60 seconds");
            std::process::abort();
        }
    });

    let mut steps = script.iter();
    loop {
        match executor::block_on(receiver.next()).expect("the emulator never stops by itself") {
            Event::Action(action) => emulator.perform(&program, action),
            Event::Failed(instruction) => panic!("step failed: {instruction}"),
            Event::Ready => {
                let Some(step) = steps.next() else { break };
                let instruction = Instruction::parse(step).unwrap_or_else(|e| panic!("bad step {step:?}: {e}"));
                emulator.run(&program, instruction);
            }
        }
    }
    finished.store(true, Ordering::SeqCst);
}

#[test]
fn boot_play_and_inspect_the_console_end_to_end() {
    let launcher = fake_launcher();
    play(
        &launcher,
        &[
            // The first status request ran before anything was clicked.
            r#"expect "Signed in as Alex""#,
            // Play: the launch runs to completion, then the follow-up refresh.
            r#"click "Play""#,
            r#"expect "The game has exited.""#,
            r#"expect "Play""#,
            // The console collected the output meanwhile; it is collapsed until asked for.
            r#"click "Show console (2 lines)""#,
            r#"expect "hello from the stand-in game""#,
            r#"expect "the game exited with status 0""#,
            // Demo mode changes the command, and nothing else.
            r#"click "Demo mode (no account)""#,
            r#"click "Play""#,
            r#"expect "The game has exited.""#,
        ],
    );
    let launches = std::fs::read_to_string(launcher.with_file_name("bastion-mc.launches")).unwrap();
    assert_eq!(
        launches.lines().collect::<Vec<_>>(),
        vec!["launch vanilla --account default", "launch vanilla --account default --demo"],
        "one launcher process per press, with exactly these arguments"
    );
    let _ = std::fs::remove_dir_all(launcher.parent().unwrap());
}
