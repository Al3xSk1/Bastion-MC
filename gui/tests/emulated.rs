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
/// `scram` does is chosen by the presence of `<script>.scram-fails`; with `<script>.bedrock` it
/// lists a Bedrock instance first, reports the default account signed out, and plays a Bedrock
/// session.
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
    if [ "$3" = "alt" ] || [ -e "$0.bedrock" ]; then account "${3:-default}" false ""; else account default true '"player":"Alex",'; fi
    accounts
    if [ -e "$0.bedrock" ]; then
      echo '{"event":"instances","instances":[{"id":"phone","minecraft":"1.26.62.1","kind":"bedrock","detail":"Minecraft Bedrock, the Android build"},{"id":"vanilla","minecraft":"26.3","kind":"vanilla"}]}'
    else
      echo '{"event":"instances","instances":[{"id":"vanilla","minecraft":"26.3","kind":"vanilla"},{"id":"modded","minecraft":"26.3","kind":"curated","detail":"serial 2, signed by me"}]}'
    fi
    ;;
  launch)
    if [ -e "$0.bedrock" ]; then
      # A Bedrock session: the launcher takes answers on its input, asks for a sign-in (the
      # first answer is not accepted), then for the approval of a Realms join.
      echo '{"event":"phase","phase":"preparing"}'
      echo '{"event":"answerable","kind":"signin"}'
      echo '{"event":"answerable","kind":"realms"}'
      echo '{"event":"protection","items":[{"aspect":"display","shield":"on","detail":"no display"},{"aspect":"network","shield":"on","detail":"filtered: only allowlisted Bedrock hosts, by name, through the launcher"},{"aspect":"credential","shield":"partial","detail":"the game signs in to Xbox itself"},{"aspect":"sign-in","shield":"on","detail":"shown outside the game"},{"aspect":"limits","shield":"on","detail":"limits"},{"aspect":"launcher","shield":"on","detail":"confined"}]}'
      echo '{"event":"phase","phase":"running"}'
      page() {
        echo "{\"event\":\"sign_in_page\",\"url\":\"https://login.live.com/oauth20_authorize.srf?client_id=0000000048183522&state=s\",\"end\":\"ms-xal-0000000048183522://auth\",\"expires_in\":900,\"attempts_left\":$1,\"state\":\"$2\"$3}"
      }
      page 10 waiting ""
      read -r line; echo "$line" >> "$0.calls"
      page 9 waiting ',"note":"the state does not match this sign-in"'
      read -r line; echo "$line" >> "$0.calls"
      page 9 done ""
      echo '{"event":"realms_approval","world":"Our Realm","address":"203.0.113.9:19132","mode":"code","code":"123456","expires_in":120,"attempts_left":3,"state":"waiting"}'
      read -r line; echo "$line" >> "$0.calls"
      echo '{"event":"realms_approval","world":"Our Realm","address":"203.0.113.9:19132","mode":"code","expires_in":0,"attempts_left":3,"state":"approved"}'
      echo '{"event":"exited","code":0}'
      exit 0
    fi
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
    start_with(name, &[])
}

/// [`start`], with the stand-in's switches (`<script>.<flag>` files) made before it is first
/// called.
fn start_with(name: &str, flags: &[&str]) -> (PathBuf, Session<impl Program + 'static>) {
    let one_at_a_time = support::one_at_a_time();
    let launcher = fake_launcher(name);
    for flag in flags {
        std::fs::write(launcher.with_file_name(format!("bastion-mc.{flag}")), "").unwrap();
    }
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

#[test]
fn a_bedrock_sign_in_and_a_realms_join_are_answered_on_the_launchers_input_end_to_end() {
    // A Bedrock instance, listed first (so chosen), and no Java account signed in, no demo
    // mode: Play is offered all the same, since the game signs in inside its session.
    let (launcher, mut ui) = start_with("bedrock", &["bedrock"]);
    ui.idle();
    ui.click(&caps("Play"));
    ui.eventually(&caps("Microsoft sign-in for Bedrock"));
    // The sixth aspect of a Bedrock launch is on the page.
    assert!(ui.shows(&caps("sign-in")));
    // The field for the address has the keyboard as soon as the prompt is up.
    let address = "ms-xal-0000000048183522://auth?code=M.C507_BL2.2.U.0e5b2bbb&state=s";
    ui.type_text(address);
    ui.click(&caps("Send"));
    ui.eventually("Not accepted: the state does not match this sign-in.");
    // The address stays in the field, to be corrected or sent again.
    ui.click(&caps("Send"));
    // Accepted: the prompt leaves the page, and the launch's words say so.
    ui.eventually("Sign-in: handed to the game, which finishes it.");
    assert!(!ui.shows(&caps("Microsoft sign-in for Bedrock")));
    // The Realms join of a session without the phone: approved here.
    ui.eventually("123 456");
    ui.click(&caps("Approve join"));
    ui.eventually("The game has exited.");
    ui.idle();

    assert_eq!(
        calls(&launcher),
        [
            "status --account default".to_string(),
            "launch phone".to_string(),
            format!("answer signin {address}"),
            format!("answer signin {address}"),
            "answer realms 123456".to_string(),
            "status --account default".to_string(),
        ],
        "each answer is one line in the protocol's grammar, written to the launcher that asked"
    );
    let _ = std::fs::remove_dir_all(launcher.parent().unwrap());
}
