//! The phone, the home screen and the dark menus of the companion mod, against a stand-in
//! game (`tests/java/PhoneUiCheck.java`; no Minecraft, no display).
//!
//! The program plays the game's part (the platform API: screens, keys, the mouse, a clock it
//! moves itself) and the launcher's (app states — well formed, hostile and mutated — pictures,
//! notices), drives every app the way a player would, and reports:
//!
//! * every command line the phone sent: each one must parse with the launcher's own
//!   [`phone::Command::parse`] and fit its app's vocabulary in `docs/notes/round4-contract.md`
//!   (verb, number and shape of arguments, text where text belongs);
//! * every place where something was drawn where it does not belong — outside its clip (text
//!   cut instead of shortened), outside the phone or its screen, off the screen, over other
//!   text, a rectangle turned inside out — on thirteen window sizes from 200 × 150 to
//!   2560 × 1080 GUI units, with ordinary, overlong, wide-script and missing states;
//! * its own scripted checks (the right command for each control, the page coordinates a
//!   click maps to, Escape and the phone key, bounded notices and textures …).
//!
//! What it does not show: anything about the real game. That is the real-game check in
//! `docs/notes/mod-ui.md`.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

use bastion_mc::{companion, phone, util};

fn javac() -> std::ffi::OsString {
    std::env::var_os("JAVAC").unwrap_or_else(|| "javac".into())
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("bmc-phone-ui-{tag}-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
    util::create_private_dir_all(&dir).unwrap();
    dir
}

/// The embedded jar on disk and the check program compiled against it.
fn class_path(dir: &Path) -> String {
    let jar = dir.join("bastion-companion.jar");
    std::fs::write(&jar, companion::JAR).unwrap();
    let classes = dir.join("classes");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/java/PhoneUiCheck.java");
    let output = Command::new(javac())
        .args(["--release", "17", "-proc:none", "-cp"])
        .arg(&jar)
        .arg("-d")
        .arg(&classes)
        .arg(source)
        .output()
        .expect("javac");
    assert!(output.status.success(), "javac failed:\n{}", String::from_utf8_lossy(&output.stderr));
    format!("{}:{}", jar.display(), classes.display())
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit())
}

fn signed(s: &str) -> bool {
    digits(s.strip_prefix('-').unwrap_or(s))
}

fn spotify_uri(s: &str, kinds: &[&str]) -> bool {
    let mut parts = s.splitn(3, ':');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some("spotify"), Some(kind), Some(id))
            if kinds.contains(&kind) && !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())
    )
}

const BROWSER_KEYS: &[&str] = &[
    "Enter",
    "Backspace",
    "Tab",
    "Escape",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "ctrl-a",
    "ctrl-c",
    "ctrl-v",
    "ctrl-x",
    "ctrl-z",
];

/// Whether a parsed command fits its app's vocabulary (round4-contract.md, "App vocabularies").
fn fits_vocabulary(c: &phone::Command) -> Result<(), String> {
    let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
    let text = c.text.as_deref();
    let none = args.is_empty() && text.is_none();
    let ok = match (c.app.as_str(), c.verb.as_str()) {
        ("spotify", "play" | "pause" | "toggle" | "next" | "previous" | "devices" | "art") => none,
        ("spotify", "seek") => matches!(args[..], [ms] if digits(ms)) && text.is_none(),
        ("spotify", "volume") => {
            matches!(args[..], [v] if digits(v) && v.parse::<u32>().unwrap() <= 100) && text.is_none()
        }
        ("spotify", "shuffle") => matches!(args[..], ["on" | "off"]) && text.is_none(),
        ("spotify", "repeat") => matches!(args[..], ["off" | "track" | "context"]) && text.is_none(),
        ("spotify", "open") => {
            matches!(args[..], [uri] if spotify_uri(uri, &["track", "album", "playlist", "artist", "episode", "show"]))
                && text.is_none()
        }
        ("spotify", "queue") => matches!(args[..], [uri] if spotify_uri(uri, &["track", "episode"])) && text.is_none(),
        ("spotify", "search") => args.is_empty() && text.is_some_and(|t| !t.is_empty()),
        ("spotify", "transfer") => args.len() == 1 && text.is_none(),
        ("browser", "open") => args.len() == 1 && text.is_none(),
        ("browser", "close" | "back" | "forward" | "reload") => none,
        ("browser", "size") => matches!(args[..], [w, h] if digits(w) && digits(h)) && text.is_none(),
        ("browser", "mouse") => {
            matches!(args[..], ["move" | "down" | "up", x, y] if digits(x) && digits(y))
                || matches!(args[..], ["move" | "down" | "up", x, y, "left" | "right" | "middle"] if digits(x) && digits(y))
        }
        ("browser", "scroll") => matches!(args[..], [dx, dy] if signed(dx) && signed(dy)) && text.is_none(),
        ("browser", "key") => matches!(args[..], [name] if BROWSER_KEYS.contains(&name)) && text.is_none(),
        ("browser", "text") => args.is_empty() && text.is_some(),
        ("browser", "frames") => matches!(args[..], ["on" | "off"]) && text.is_none(),
        ("clips", "status" | "list") => none,
        ("clips", "save") => (args.is_empty() || matches!(args[..], [s] if digits(s))) && text.is_none(),
        ("clips", "delete" | "thumb") => args.len() == 1 && text.is_none(),
        ("clips", "rename") => args.len() == 1 && text.is_some_and(|t| !t.is_empty()),
        ("realms", "code") => args.is_empty() && text.is_some_and(digits),
        ("realms", "cancel") => none,
        ("settings", "status" | "sync-now") => none,
        _ => false,
    };
    if ok { Ok(()) } else { Err(format!("{c:?} is not in the {} vocabulary", c.app)) }
}

#[test]
fn the_phone_sends_only_commands_of_its_apps_vocabularies_and_draws_nothing_outside_its_panels() {
    let dir = scratch("check");
    let cp = class_path(&dir);
    let output = Command::new("java")
        .args(["-Xmx256m", "-Djava.awt.headless=true", "-cp", &cp, "PhoneUiCheck"])
        .output()
        .expect("java");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the check program failed:\n{stdout}\n{stderr}");
    assert!(stdout.lines().any(|l| l == "done"), "the check program did not finish:\n{stdout}\n{stderr}");

    let problems: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("fail ") || l.starts_with("violation ") || l.starts_with("exception "))
        .collect();
    assert!(problems.is_empty(), "{} problem(s):\n{}", problems.len(), problems.join("\n"));
    let refused: Vec<&str> = stdout.lines().filter(|l| l.starts_with("refused ")).collect();
    assert!(refused.is_empty(), "the phone handed the link lines outside the grammar:\n{}", refused.join("\n"));

    let mut verbs = BTreeSet::new();
    let mut count = 0;
    for line in stdout.lines().filter_map(|l| l.strip_prefix("cmd ")) {
        let command = phone::Command::parse(line.as_bytes())
            .unwrap_or_else(|why| panic!("{line:?} does not parse: {}", why.as_str()));
        fits_vocabulary(&command).unwrap_or_else(|why| panic!("{line:?}: {why}"));
        verbs.insert(format!("{} {}", command.app, command.verb));
        count += 1;
    }
    // What a player can do with the phone, each at least once in the script.
    for expected in [
        "spotify play",
        "spotify pause",
        "spotify next",
        "spotify previous",
        "spotify seek",
        "spotify volume",
        "spotify shuffle",
        "spotify repeat",
        "spotify open",
        "spotify queue",
        "spotify search",
        "spotify devices",
        "spotify transfer",
        "spotify art",
        "browser open",
        "browser close",
        "browser back",
        "browser forward",
        "browser reload",
        "browser size",
        "browser mouse",
        "browser scroll",
        "browser key",
        "browser text",
        "browser frames",
        "clips list",
        "clips save",
        "clips thumb",
        "clips rename",
        "clips delete",
        "realms code",
        "realms cancel",
        "settings status",
        "settings sync-now",
    ] {
        assert!(verbs.contains(expected), "never sent: {expected} (sent: {verbs:?})");
    }
    let checks = stdout.lines().filter(|l| l.starts_with("ok ")).count();
    let frames = stdout.lines().find_map(|l| l.strip_prefix("count frames=")).unwrap_or("?");
    let draws = stdout.lines().find_map(|l| l.strip_prefix("count draws=")).unwrap_or("?");
    println!(
        "{checks} checks passed; {count} command lines parsed; {frames} frames, {draws} drawing operations checked"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
