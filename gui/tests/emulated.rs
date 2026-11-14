//! The whole application, run for real but without a display: iced's emulator boots it,
//! executes its tasks on the real executor, clicks widgets and reads what is on screen.
//!
//! Only the launcher is a stand-in (a script that plays back sessions and writes down how
//! it was called), so this checks the part no unit test can: that events travel from a
//! child process, through the tokio worker and iced's task machinery, into the state and
//! onto the screen — and that a press starts exactly the launcher command it should, in
//! the order it should.
//!
//! Nothing here presses a control that opens a link: that would start the desktop's URL
//! opener for real.

mod support;

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use bastion_mc_gui::{Options, application, theme::caps};
use iced_test::program::Program;
use support::Session;

/// The stand-in launcher. Every command appends its arguments to `<script>.calls`; what
/// `scram` does is chosen by the presence of `<script>.scram-fails`.
const FAKE_LAUNCHER: &str = r#"#!/bin/sh
[ "$1" = "--events" ] || exit 99
shift
echo "$*" >> "$0.calls"
echo '{"event":"hello","protocol":1,"version":"0.0.0"}'
account() {
  echo "{\"event\":\"account\",\"account\":\"$1\",\"signed_in\":$2,$3\"store\":\"kernel keyring\",\"client_id_configured\":true,\"device_flow\":true}"
}
accounts() {
  echo '{"event":"accounts","default":"default","accounts":[{"account":"alt","signed_in":false,"store":"kernel keyring","client_id_configured":true,"device_flow":true},{"account":"default","signed_in":true,"player":"Alex","store":"kernel keyring","client_id_configured":true,"device_flow":true}]}'
}
case "$1" in
  status)
    if [ "$3" = "alt" ]; then account alt false ""; else account default true '"player":"Alex",'; fi
    accounts
    echo '{"event":"instances","instances":[{"id":"vanilla","minecraft":"26.3","kind":"vanilla"},{"id":"modded","minecraft":"26.3","kind":"curated","detail":"serial 2, signed by me"}]}'
    ;;
  launch)
    echo '{"event":"phase","phase":"preparing"}'
    echo '{"event":"progress","label":"assets","done":50,"total":100,"unit":"bytes"}'
    echo '{"event":"phase","phase":"verifying"}'
    echo '{"event":"protection","items":[{"aspect":"network","shield":"on","detail":"filtered: only allowlisted hosts"}]}'
    echo '{"event":"phase","phase":"running"}'
    echo '{"event":"log","source":"game","line":"hello from the stand-in game"}'
    echo '{"event":"exited","code":0}'
    ;;
  login)
    echo '{"event":"device_code","user_code":"ABCD-EFGH","verification_uri":"https://www.microsoft.com/link","expires_in":900}'
    # A sign-in waits for the user; the only way out here is the front end's stop.
    read -r command
    echo "login ended by: ${command:-end of input}" >> "$0.calls"
    echo '{"event":"error","message":"cancelled","code":130}'
    exit 130
    ;;
  scram)
    echo '{"event":"status","text":"no launch was in progress"}'
    echo '{"event":"status","text":"removed the refresh token of account default from the kernel keyring"}'
    if [ -e "$0.scram-fails" ]; then
      echo '{"event":"status","text":"COULD NOT delete the vault file /home/u/vault/alt.age: Permission denied"}'
      echo '{"event":"status","text":""}'
      echo '{"event":"status","text":"This removed the copies on this machine. It revoked NOTHING at Microsoft or Mojang:"}'
      accounts
      exit 1
    fi
    echo '{"event":"status","text":""}'
    echo '{"event":"status","text":"This removed the copies on this machine. It revoked NOTHING at Microsoft or Mojang:"}'
    ;;
  *)
    echo '{"event":"error","message":"the stand-in launcher does not know this command","code":2}'
    exit 2
    ;;
esac
"#;

/// A stand-in launcher of its own for the test called `name` (tests run in parallel).
fn fake_launcher(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("emulated-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bastion-mc");
    std::fs::write(&path, FAKE_LAUNCHER).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// How the stand-in was called, in order.
fn calls(launcher: &Path) -> Vec<String> {
    let log = std::fs::read_to_string(launcher.with_file_name("bastion-mc.calls")).unwrap_or_default();
    log.lines().map(str::to_string).collect()
}

/// A stand-in launcher and the application driving it, for the test called `name`.
///
/// The lock is taken before the stand-in is written: while it is held no other test of
/// this file starts a process, so no child of another thread can be holding the script
/// open for writing at the moment it is executed (which would fail with "text file busy").
fn start(name: &str) -> (PathBuf, Session<impl Program + 'static>) {
    let one_at_a_time = support::one_at_a_time();
    let launcher = fake_launcher(name);
    let program = application(Options { launcher: launcher.clone(), account: "default".into() });
    (launcher, Session::boot(program, one_at_a_time))
}

#[test]
fn boot_play_and_inspect_the_console_end_to_end() {
    let (launcher, mut ui) = start("play");
    // The first status request runs before anything is clicked.
    ui.eventually("Signed in");
    ui.eventually("Alex");
    ui.idle();
    // Play: the launch runs to completion, then the follow-up refresh.
    ui.click(&caps("Play"));
    ui.eventually("The game has exited.");
    ui.idle();
    assert!(ui.shows(&caps("Play")), "the control is back");
    assert!(ui.shows("filtered: only allowlisted hosts"), "what the launch ran with stays on screen");
    // The console collected the output meanwhile, on its own page.
    assert!(!ui.shows("hello from the stand-in game"));
    ui.click(&caps("Console"));
    ui.eventually("hello from the stand-in game");
    assert!(ui.shows("the game exited with status 0"));
    // Demo mode changes the command, and nothing else.
    ui.click(&caps("Launch"));
    ui.click("Demo mode (no account)");
    ui.click(&caps("Play"));
    ui.eventually("The game has exited.");
    ui.idle();

    assert_eq!(
        calls(&launcher),
        [
            "status --account default",
            "launch vanilla --account default",
            "status --account default",
            "launch vanilla --account default --demo",
            "status --account default",
        ],
        "one launcher process per press, with exactly these arguments, and a refresh after each launch"
    );
    let _ = std::fs::remove_dir_all(launcher.parent().unwrap());
}

#[test]
fn accounts_are_listed_switched_and_signed_in_to_end_to_end() {
    let (launcher, mut ui) = start("accounts");
    ui.idle();
    ui.click(&caps("Accounts"));
    // One card per account, from the one status request that was made.
    ui.eventually("plays as Alex");
    assert!(ui.shows("alt") && ui.shows(&caps("In use")) && ui.shows(&caps("Default")));
    // Only the other account can be switched to.
    ui.click(&caps("Switch to"));
    ui.idle();
    assert!(ui.shows("Not signed in"), "the launcher was asked about the account now in use");
    // Its sign-in: the code is shown, and cancelling ends the launcher that waits for it.
    ui.click(&caps("Sign in"));
    ui.eventually("ABCD-EFGH");
    assert!(ui.shows("https://www.microsoft.com/link"));
    ui.click(&caps("Cancel"));
    ui.eventually("Cancelled.");
    ui.idle();
    assert!(!ui.shows("ABCD-EFGH"));

    assert_eq!(
        calls(&launcher),
        ["status --account default", "status --account alt", "login --account alt --device", "login ended by: stop"]
    );
    let _ = std::fs::remove_dir_all(launcher.parent().unwrap());
}

#[test]
fn a_scram_ends_a_pending_sign_in_first_and_says_complete_only_for_a_clean_exit() {
    let (launcher, mut ui) = start("scram");
    ui.idle();
    ui.click(&caps("Accounts"));
    ui.click(&caps("Switch to"));
    ui.idle();
    ui.click(&caps("Sign in"));
    ui.eventually("ABCD-EFGH");

    // Two different presses, on the security page.
    ui.click(&caps("Security"));
    assert!(!ui.shows(&caps("Yes, scram")));
    ui.click(&caps("Scram…"));
    ui.click(&caps("Yes, scram"));
    ui.eventually(&caps("Scram complete"));
    ui.idle();
    assert!(ui.shows("removed the refresh token of account default from the kernel keyring"));
    assert!(!ui.shows(&caps("Scram incomplete")));

    // The order is the point: the launcher that was signing in is gone before the one that
    // removes the tokens starts, so nothing can store a token after the removal.
    assert_eq!(
        calls(&launcher),
        [
            "status --account default",
            "status --account alt",
            "login --account alt --device",
            "login ended by: stop",
            "scram --yes",
            "status --account alt",
        ]
    );
    let _ = std::fs::remove_dir_all(launcher.parent().unwrap());
}

#[test]
fn a_scram_the_launcher_reports_as_incomplete_is_shown_as_incomplete_end_to_end() {
    let (launcher, mut ui) = start("scram-incomplete");
    std::fs::write(launcher.with_file_name("bastion-mc.scram-fails"), "").unwrap();
    ui.idle();
    ui.click(&caps("Security"));
    ui.click(&caps("Scram…"));
    ui.click(&caps("Yes, scram"));
    // Exit status 1 and a "COULD NOT" line: said in so many words, never as done.
    ui.eventually(&caps("Scram incomplete"));
    ui.idle();
    assert!(!ui.shows(&caps("Scram complete")));
    assert!(ui.shows("COULD NOT delete the vault file /home/u/vault/alt.age: Permission denied"));
    assert!(ui.shows("Not everything is confirmed removed: the launcher ended with status 1."));
    // The console has it as well.
    ui.click(&caps("Console"));
    ui.eventually("scram: COULD NOT delete the vault file /home/u/vault/alt.age: Permission denied");
    assert!(ui.shows("scram: INCOMPLETE: the launcher ended with status 1"));

    assert_eq!(calls(&launcher), ["status --account default", "scram --yes", "status --account default"]);
    let _ = std::fs::remove_dir_all(launcher.parent().unwrap());
}
