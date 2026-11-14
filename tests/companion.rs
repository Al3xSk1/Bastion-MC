//! The companion mod outside the game.
//!
//! * In a plain JVM with no Minecraft at all, every lookup the mod makes fails — the
//!   situation it must survive on a game version it does not fit. These tests run it there
//!   and check that it only ever switches features off, says so once, and keeps talking to
//!   the launcher in the protocol the launcher parses.
//! * In a plain JVM with a *stand-in* game (`tests/java/fakegame`: classes with the names and
//!   members the mod looks up, which call its hooks where Mixin would have put them), the
//!   mod's own logic runs on every `cargo test`: the zoom's arithmetic and its rules (no zoom
//!   with a screen open, the wheel, the eased transition, the scaled mouse movement), what
//!   the HUD writes, the brightness override, a hook that starts failing, and a game one of
//!   whose methods has changed. None of this says anything about the real game.
//! * In the real sandbox (bubblewrap, Landlock, seccomp; a Python payload in place of the
//!   JVM, as in `tests/sandbox.rs`): what an injection adds to it, seen from inside.
//! * In the real sandbox with the system JVM, started like the game (`tests/jvm.rs`): the
//!   mod reads the launcher's read-only settings — and no other file, whatever a JVM option
//!   names — and its report reaches the launcher.
//! * Opt-in (needs the real jars): the compile-only stand-ins under `java/stubs` against the
//!   real Fabric Loader and Mixin classes.
//!
//!   ```sh
//!   BASTION_E2E_ROOT=$PWD/target/e2e cargo test --test companion
//!   ```
//!
//! The mod inside the real game is `tests/companion_game.rs`.

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bastion_mc::{
    BOOTSTRAP_CLASS, BOOTSTRAP_CLASS_NAME,
    companion::{
        self, CompanionConfig,
        channel::{self, Message, Voice},
    },
    config::{InstanceConfig, JavaMatch},
    instance::{Instance, Kind},
    java,
    mojang::args::{Vars, build_frame},
    paths::Paths,
    profile::manifest::ProfileManifest,
    sandbox::{
        self, BaseOptions, Mount, Network, Resolv, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
    util,
};

#[path = "../src/companion/jar.rs"]
mod jar;

fn javac() -> std::ffi::OsString {
    std::env::var_os("JAVAC").unwrap_or_else(|| "javac".into())
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("bmc-companion-{tag}-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
    util::create_private_dir_all(&dir).unwrap();
    dir
}

/// The embedded jar on disk, and the test program compiled against it.
fn offline_class_path(dir: &Path) -> String {
    let jar = dir.join("bastion-companion.jar");
    std::fs::write(&jar, companion::JAR).unwrap();
    let classes = dir.join("classes");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/java/CompanionOffline.java");
    let status = Command::new(javac())
        .args(["--release", "17", "-proc:none", "-cp"])
        .arg(&jar)
        .arg("-d")
        .arg(&classes)
        .arg(source)
        .status()
        .expect("javac");
    assert!(status.success(), "javac failed");
    format!("{}:{}", jar.display(), classes.display())
}

struct Run {
    stdout: String,
    stderr: String,
    /// What the mod sent to the launcher's socket, line by line.
    channel: Vec<String>,
}

/// Run the mod in a plain JVM with `settings` as its settings file (`None`: no file). With
/// `listen`, a socket stands in for the launcher's and `{channel}` in the settings is its
/// path. The program is released (its standard input closed) once `expect_lines` lines have
/// arrived on that socket and a line containing `await_log` has appeared on its standard
/// error — the mod's channel thread works on its own time — or after half a minute.
fn run_offline(
    dir: &Path,
    class_path: &str,
    settings: Option<&str>,
    listen: bool,
    expect_lines: usize,
    await_log: &str,
) -> Run {
    run_program(dir, class_path, &["CompanionOffline"], settings, listen, expect_lines, await_log)
}

/// As [`run_offline`], for any of the test programs: `program` is the main class and its
/// arguments.
fn run_program(
    dir: &Path,
    class_path: &str,
    program: &[&str],
    settings: Option<&str>,
    listen: bool,
    expect_lines: usize,
    await_log: &str,
) -> Run {
    let socket = dir.join("channel");
    let _ = std::fs::remove_file(&socket);
    let listener = listen.then(|| {
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    });
    let config = dir.join("config.properties");
    let _ = std::fs::remove_file(&config);
    if let Some(text) = settings {
        std::fs::write(&config, text.replace("{channel}", socket.to_str().unwrap())).unwrap();
    }
    let mut child = Command::new("java")
        .arg(format!("-Dbastion.companion.config={}", config.display()))
        .args(["-Xmx64m", "-cp", class_path])
        .args(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("java");
    let deadline = Instant::now() + Duration::from_secs(30);

    // Standard error, as it comes.
    let logged = Arc::new(Mutex::new(String::new()));
    let reader = {
        let (logged, stderr) = (logged.clone(), child.stderr.take().unwrap());
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut text = logged.lock().unwrap();
                text.push_str(&line);
                text.push('\n');
            }
        })
    };

    let mut channel = Vec::new();
    if let Some(listener) = listener {
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break Some(stream),
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break None,
            }
        };
        if let Some(stream) = stream {
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            let mut lines = BufReader::new(stream).lines();
            while channel.len() < expect_lines {
                match lines.next() {
                    Some(Ok(line)) => channel.push(line),
                    _ => break,
                }
            }
        }
    }
    while !logged.lock().unwrap().contains(await_log) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(child.stdin.take()); // "seen enough"
    let mut stdout = String::new();
    child.stdout.take().unwrap().read_to_string(&mut stdout).unwrap();
    let status = child.wait().unwrap();
    reader.join().unwrap();
    let stderr = logged.lock().unwrap().clone();
    assert!(status.success(), "the program failed: {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    Run { stdout, stderr, channel }
}

/// The line the mod logs once it has started, whatever its settings.
const STARTED: &str = "conveniences only";

const EVERYTHING_ON: &str = "zoom=true\nzoom.key=key.keyboard.c\nzoom.factor=4\nzoom.wheel=true\nhud=true\n\
                             fullbright=true\nfullbright.gamma=15\nchannel={channel}\n";

#[test]
fn where_the_game_is_not_what_it_expects_each_feature_switches_itself_off_and_says_so_once() {
    let dir = scratch("off");
    let class_path = offline_class_path(&dir);
    let run = run_offline(&dir, &class_path, Some(EVERYTHING_ON), true, 4, STARTED);

    // The "game" got its own values back from 2000 rounds of every hook, and nothing threw.
    for line in ["fov=70.0", "wheel=false", "live=false,false,false", "done"] {
        assert!(run.stdout.lines().any(|l| l == line), "{line} missing from:\n{}\n{}", run.stdout, run.stderr);
    }

    // The launcher's own parser understands every line the mod sent: a hello that claims no
    // feature, then one diagnostic per feature.
    let messages: Vec<Message> = run
        .channel
        .iter()
        .map(|line| channel::parse_line(line.as_bytes()).unwrap_or_else(|| panic!("not protocol: {line:?}")))
        .collect();
    assert_eq!(
        messages[0],
        Message::Hello { protocol: channel::PROTOCOL, version: companion::VERSION.into(), features: Vec::new() }
    );
    let mut diagnosed = BTreeMap::new();
    for message in &messages[1..] {
        let Message::Diagnostic { feature, text } = message else { panic!("a second hello: {message:?}") };
        assert!(
            text.starts_with("disabled: lookup failed (ClassNotFoundException: net.minecraft.")
                || text.contains("com.mojang."),
            "{text}"
        );
        *diagnosed.entry(feature.as_str()).or_insert(0) += 1;
    }
    assert_eq!(diagnosed, BTreeMap::from([("zoom", 1), ("hud", 1), ("fullbright", 1)]));

    // One line per feature in the log, however often the hooks ran. (No SLF4J here: the
    // mod's log lines are on standard error.)
    for feature in ["zoom", "hud", "fullbright"] {
        let lines = run.stderr.lines().filter(|l| l.contains(&format!("{feature} disabled: "))).count();
        assert_eq!(lines, 1, "{feature}:\n{}", run.stderr);
    }
    assert!(run.stderr.lines().count() <= 6, "the mod is talkative:\n{}", run.stderr);
    assert!(
        run.stderr.contains(&format!("version {}, features: - (conveniences only", companion::VERSION)),
        "{}",
        run.stderr
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn without_settings_or_without_a_channel_the_mod_stays_inert_and_the_game_is_unaffected() {
    let dir = scratch("inert");
    let class_path = offline_class_path(&dir);

    // No settings file at all (the mod loaded somewhere the launcher did not put it).
    let run = run_offline(&dir, &class_path, None, false, 0, "no launcher channel configured");
    assert!(run.stdout.contains("fov=70.0") && run.stdout.contains("live=false,false,false"), "{}", run.stdout);
    assert!(
        run.stderr.contains("no settings file") && run.stderr.contains("every feature stays off"),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("disabled:"), "nothing was switched on, so nothing is reported off:\n{}", run.stderr);

    // Features off, no channel configured: nothing to say beyond the one start-up line.
    let quiet = "zoom=false\nhud=false\nfullbright=false\n";
    let run = run_offline(&dir, &class_path, Some(quiet), false, 0, "no launcher channel configured");
    assert!(run.stderr.contains("no launcher channel configured"), "{}", run.stderr);
    assert!(!run.stderr.contains("disabled:"), "{}", run.stderr);

    // A channel is configured but nobody listens: said once, and the mod carries on.
    let run = run_offline(&dir, &class_path, Some(EVERYTHING_ON), false, 0, "launcher channel unavailable");
    assert_eq!(run.stderr.lines().filter(|l| l.contains("launcher channel unavailable")).count(), 1, "{}", run.stderr);
    assert!(run.stdout.contains("done"));

    // Settings a launcher would never write are not taken at face value.
    let odd = "zoom=yes\nzoom.factor=1e99\nzoom.key=\nhud=TRUE\nfullbright=1\nfullbright.gamma=NaN\n=x\nnonsense\n";
    let run = run_offline(&dir, &class_path, Some(odd), false, 0, STARTED);
    assert!(run.stdout.contains("live=false,false,false"), "only the exact word true switches a feature on");
    std::fs::remove_dir_all(dir).unwrap();
}

// ---------------------------------------------------------------- with a stand-in game

/// Every `.java` file below `dir`, sorted.
fn java_sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap_or_else(|e| panic!("{}: {e}", next.display())).flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "java") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The embedded jar on disk, and the stand-in game with its driver compiled against it.
/// With `changed`, the classes under `tests/java/fakegame-changed` take the place of their
/// namesakes: a game version in which something the mod relies on is no longer as it was.
fn fake_game_class_path(dir: &Path, changed: bool) -> String {
    let jar = dir.join("bastion-companion.jar");
    std::fs::write(&jar, companion::JAR).unwrap();
    let java = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/java");
    let (game, other) = (java.join("fakegame"), java.join("fakegame-changed"));
    let mut sources = vec![java.join("CompanionFakeGame.java")];
    let mut replaced = 0;
    for source in java_sources(&game) {
        let replacement = other.join(source.strip_prefix(&game).unwrap());
        if changed && replacement.is_file() {
            replaced += 1;
            sources.push(replacement);
        } else {
            sources.push(source);
        }
    }
    assert_eq!(replaced, if changed { java_sources(&other).len() } else { 0 }, "a changed class replaces a namesake");
    let classes = dir.join(if changed { "game-changed" } else { "game" });
    let output = Command::new(javac())
        .args(["--release", "17", "-proc:none", "-Xlint:all", "-Werror", "-cp"])
        .arg(&jar)
        .arg("-d")
        .arg(&classes)
        .args(&sources)
        .output()
        .expect("javac");
    assert!(output.status.success(), "javac failed:\n{}", String::from_utf8_lossy(&output.stderr));
    format!("{}:{}", jar.display(), classes.display())
}

/// The `name=value` lines a test program printed.
fn shown(run: &Run) -> BTreeMap<String, String> {
    run.stdout.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn value<'a>(values: &'a BTreeMap<String, String>, name: &str) -> &'a str {
    values.get(name).unwrap_or_else(|| panic!("the program printed no {name}: {values:#?}"))
}

/// `name` as numbers: one, or several separated by `/`.
fn numbers(values: &BTreeMap<String, String>, name: &str) -> Vec<f64> {
    value(values, name)
        .split('/')
        .map(|n| n.parse().unwrap_or_else(|_| panic!("{name} is not a number: {:?}", values[name])))
        .collect()
}

fn near(actual: f64, expected: f64, what: &str) {
    assert!((actual - expected).abs() <= 1e-3, "{what}: {actual}, expected {expected}");
}

/// The stand-in's numbers for the two keys it knows by name.
const KEY_C: &str = "6";
const KEY_V: &str = "25";
/// The stand-in camera's field of view, and what the tangent rule makes of narrowing it.
const BASE_FOV: f64 = 70.0;
fn turn_scale(zoomed: f64) -> f64 {
    (zoomed.to_radians() / 2.0).tan() / (BASE_FOV.to_radians() / 2.0).tan()
}

#[test]
fn with_a_stand_in_game_every_feature_does_what_it_says() {
    let dir = scratch("game");
    let class_path = fake_game_class_path(&dir, false);
    // Two lines are expected on the channel: the hello, and the report of the hook that the
    // program breaks at its end.
    let run =
        run_program(&dir, &class_path, &["CompanionFakeGame", KEY_C], Some(EVERYTHING_ON), true, 2, "hud disabled");
    let v = shown(&run);
    let context = format!("--- stdout ---\n{}\n--- stderr ---\n{}", run.stdout, run.stderr);
    assert_eq!(value(&v, "live"), "true,true,true", "every lookup succeeds against the stand-in\n{context}");

    // Zoom. The key is held throughout; it zooms only while playing.
    for (state, why) in [
        ("fov_no_world", "no player: not in a world"),
        ("fov_screen_open", "a screen is open: the key is a letter to type"),
        ("fov_loading", "a loading overlay is in front of the world"),
        ("fov_typing", "a screen was opened while the key was down"),
        ("fov_released", "the key was released"),
        ("fov_wheel_limit_out", "the wheel was turned back as far as it goes: no zoom, never a wider view"),
    ] {
        assert_eq!(value(&v, state), "70.0", "{state} ({why}): exactly the game's own angle\n{context}");
    }
    near(numbers(&v, "fov_zoomed")[0], BASE_FOV / 4.0, "held while playing: the angle divided by the factor");
    let first = numbers(&v, "fov_first_frame")[0];
    assert!(first < BASE_FOV - 1.0 && first > BASE_FOV / 4.0 + 1.0, "the way in is gradual: one frame gives {first}");
    let out = numbers(&v, "fov_first_frame_out")[0];
    assert!(out > BASE_FOV / 4.0 + 1.0 && out < BASE_FOV - 1.0, "the way out is gradual: one frame gives {out}");
    // The same hand movement turns the picture by the same distance on screen.
    let turned = numbers(&v, "turned_zoomed");
    near(turned[0], 100.0 * turn_scale(BASE_FOV / 4.0), "horizontal mouse movement while zoomed");
    near(turned[1], -40.0 * turn_scale(BASE_FOV / 4.0), "vertical mouse movement while zoomed");
    assert_eq!(value(&v, "turned_released"), "100.0/-40.0", "and is left exactly alone otherwise");
    // The wheel changes the factor while the key is held, within limits, and is the game's
    // again afterwards; releasing the key forgets the change.
    near(numbers(&v, "fov_two_notches")[0], BASE_FOV / (4.0 * 1.15 * 1.15), "two notches of the wheel");
    near(numbers(&v, "fov_wheel_limit_in")[0], BASE_FOV / 50.0, "the wheel stops at the largest factor");
    assert_eq!(value(&v, "scrolled_while_zoomed"), "0.0", "no wheel movement reached the hotbar meanwhile");
    near(numbers(&v, "fov_pressed_again")[0], BASE_FOV / 4.0, "pressed again: the configured factor");
    assert_eq!(value(&v, "scrolled_released"), "1.0", "with the key up the wheel is the game's");

    // HUD readout: position and facing only where the debug screen would show them, and
    // nothing while the HUD is hidden or the debug screen is open.
    assert_eq!(
        value(&v, "hud"),
        "2,2,ffffffff,true:144 fps | 2,12,ffffffff,true:XYZ 12.3 / 64.0 / -7.9 | \
         2,22,ffffffff,true:facing east (+X), yaw -90.5, pitch 12.3"
    );
    assert_eq!(value(&v, "hud_reduced_info"), "2,2,ffffffff,true:144 fps", "reduced debug info: the frame rate alone");
    assert_eq!((value(&v, "hud_hidden"), value(&v, "hud_debug_screen")), ("0", "0"));

    // Fullbright: the stand-in's brightness option is 0.5.
    assert_eq!(value(&v, "brightness"), "15.0", "raised to the configured value");
    assert_eq!(value(&v, "brightness_darkness"), "14.7", "the Darkness effect still takes its 0.3");
    assert_eq!(value(&v, "brightness_option_above"), "19.7", "an option already above the value is not lowered");

    // The hook that failed 500 times in a row: that feature is off, said once, and the rest
    // is untouched.
    assert_eq!(value(&v, "live_after_failure"), "true,false,true", "{context}");
    assert_eq!(run.stderr.lines().filter(|l| l.contains("disabled: ")).count(), 1, "{}", run.stderr);
    assert!(run.stderr.lines().count() <= 3, "the mod is talkative:\n{}", run.stderr);
    let messages: Vec<Message> = run
        .channel
        .iter()
        .map(|line| channel::parse_line(line.as_bytes()).unwrap_or_else(|| panic!("not protocol: {line:?}")))
        .collect();
    let features = ["zoom", "hud", "fullbright"].map(String::from).to_vec();
    assert_eq!(
        messages,
        [
            Message::Hello { protocol: channel::PROTOCOL, version: companion::VERSION.into(), features },
            Message::Diagnostic {
                feature: "hud".into(),
                text: "disabled: the HUD hook failed (IllegalStateException: the renderer has changed)".into()
            },
        ]
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_game_in_which_one_method_has_changed_loses_that_feature_and_keeps_the_others() {
    let dir = scratch("changed");
    // The camera's field-of-view method takes another argument, and no callback is in it.
    let class_path = fake_game_class_path(&dir, true);
    let run =
        run_program(&dir, &class_path, &["CompanionFakeGame", KEY_C], Some(EVERYTHING_ON), true, 3, "hud disabled");
    let v = shown(&run);
    assert_eq!(value(&v, "live"), "false,true,true", "{}\n{}", run.stdout, run.stderr);
    // No zoom at all: every angle is the game's, mouse movement and wheel are untouched.
    for state in ["fov_no_world", "fov_first_frame", "fov_zoomed", "fov_two_notches", "fov_wheel_limit_in"] {
        assert_eq!(value(&v, state), "70.0", "{state}");
    }
    assert_eq!(value(&v, "turned_zoomed"), "100.0/-40.0");
    assert_eq!(value(&v, "scrolled_while_zoomed"), "-198.0", "2 + 200 notches in, 400 out: all of it the game's");
    // The other two work as before.
    assert!(value(&v, "hud").contains("144 fps") && value(&v, "hud").contains("facing east (+X)"));
    assert_eq!(value(&v, "brightness"), "15.0");
    let messages: Vec<Message> =
        run.channel.iter().map(|line| channel::parse_line(line.as_bytes()).expect("protocol")).collect();
    assert_eq!(
        messages[..2],
        [
            Message::Hello {
                protocol: channel::PROTOCOL,
                version: companion::VERSION.into(),
                features: ["hud", "fullbright"].map(String::from).to_vec()
            },
            Message::Diagnostic {
                feature: "zoom".into(),
                text: "disabled: lookup failed (NoSuchMethodException: net.minecraft.client.Camera.calculateFov is \
                       not as this mod expects it)"
                    .into()
            },
        ]
    );
    assert_eq!(run.stderr.lines().filter(|l| l.contains("zoom disabled: ")).count(), 1, "{}", run.stderr);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_zoom_settings_are_honoured_and_a_key_the_game_does_not_know_switches_zoom_off() {
    let dir = scratch("keys");
    let class_path = fake_game_class_path(&dir, false);
    let settings = |key: &str| {
        format!("zoom=true\nzoom.key={key}\nzoom.factor=2.5\nzoom.wheel=false\nhud=false\nfullbright=false\n")
    };

    // Another key, another factor, the wheel left to the game; the other features off.
    let run = run_program(
        &dir,
        &class_path,
        &["CompanionFakeGame", KEY_V],
        Some(&settings("key.keyboard.v")),
        false,
        0,
        STARTED,
    );
    let v = shown(&run);
    assert_eq!(value(&v, "live"), "true,false,false", "{}\n{}", run.stdout, run.stderr);
    near(numbers(&v, "fov_zoomed")[0], BASE_FOV / 2.5, "the configured factor");
    near(numbers(&v, "turned_zoomed")[0], 100.0 * turn_scale(BASE_FOV / 2.5), "mouse movement at that factor");
    near(numbers(&v, "fov_two_notches")[0], BASE_FOV / 2.5, "zoom.wheel=false: the wheel does not change the factor");
    assert_eq!(value(&v, "scrolled_while_zoomed"), "-198.0", "and every notch reaches the game");
    assert_eq!(value(&v, "hud"), "", "the readout is off: nothing is drawn");
    assert_eq!(value(&v, "brightness"), "0.5", "fullbright is off: the game's own value");
    assert_eq!(value(&v, "live_after_failure"), "true,false,false");

    // The configured key is the one that is read: holding C does nothing when V is set.
    let run = run_program(
        &dir,
        &class_path,
        &["CompanionFakeGame", KEY_C],
        Some(&settings("key.keyboard.v")),
        false,
        0,
        STARTED,
    );
    assert_eq!(value(&shown(&run), "fov_zoomed"), "70.0", "{}", run.stdout);

    // Names the launcher would never write, in case the settings file is not the launcher's:
    // a key the game does not know, and one that is not on the keyboard. The game is asked
    // at first use; the feature then switches itself off, once, and nothing zooms.
    for (key, complaint) in [
        ("key.keyboard.nosuchkey", "the game knows no key named key.keyboard.nosuchkey"),
        ("key.mouse.4", "not a keyboard key the game knows: key.mouse.4"),
        ("key.keyboard.900", "not a keyboard key the game knows: key.keyboard.900"),
    ] {
        let run =
            run_program(&dir, &class_path, &["CompanionFakeGame", KEY_C], Some(&settings(key)), false, 0, "disabled");
        let v = shown(&run);
        assert_eq!(value(&v, "live"), "true,false,false", "{key}: the lookups themselves succeed");
        assert_eq!(value(&v, "fov_zoomed"), "70.0", "{key}");
        assert_eq!(value(&v, "live_after_failure"), "false,false,false", "{key}");
        let said: Vec<&str> = run.stderr.lines().filter(|l| l.contains("zoom disabled: ")).collect();
        assert_eq!(said.len(), 1, "{key}: said once\n{}", run.stderr);
        assert!(said[0].contains(complaint), "{key}: {}", said[0]);
    }
    // A key the game knows only by number is taken under that name, as in options.txt.
    let run = run_program(
        &dir,
        &class_path,
        &["CompanionFakeGame", "300"],
        Some(&settings("key.keyboard.300")),
        false,
        0,
        STARTED,
    );
    near(numbers(&shown(&run), "fov_zoomed")[0], BASE_FOV / 2.5, "a key named by its number");
    std::fs::remove_dir_all(dir).unwrap();
}

// ---------------------------------------------------------------- in the sandbox

/// A signed-profile instance as `instance::load` would return it for a Fabric profile.
fn fabric_instance() -> Instance {
    let manifest = format!(
        r#"{{"format":"bastion-mc/profile/v1","id":"fab","serial":1,"minecraft":{{"version":"26.3"}},
            "loader":{{"kind":"fabric","version":"0.20.1","main_class":"net.fabricmc.loader.impl.launch.knot.KnotClient",
                      "libraries":[{{"name":"net.fabricmc:fabric-loader:0.20.1","url":"https://maven.fabricmc.net/",
                                     "sha256":"{}","size":1}}]}}}}"#,
        "0".repeat(64)
    );
    Instance {
        id: "fab".into(),
        dir: "/nonexistent".into(),
        config: InstanceConfig::default(),
        minecraft: "26.3".into(),
        kind: Kind::Curated {
            manifest: Box::new(ProfileManifest::parse_verified(manifest.as_bytes(), "fab").expect("manifest")),
            signed_by: "test".into(),
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn in_the_sandbox_the_mod_is_two_read_only_files_and_a_socket_to_the_launcher() {
    use tokio::io::AsyncReadExt;
    let dir = scratch("box");
    let paths = Paths {
        config: dir.join("config"),
        data: dir.join("data"),
        cache: dir.join("cache"),
        state: dir.join("state"),
        runtime: Some(dir.join("rt")),
    };
    util::create_private_dir_all(&dir.join("rt")).unwrap();
    let plan = companion::plan(&CompanionConfig::default(), &fabric_instance(), 25, &paths).expect("plan");
    assert!(plan.injected(), "{}", plan.describe());

    // The sandbox every launch starts from, with the injection's additions and nothing else.
    let python = std::fs::canonicalize("/usr/bin/python3").expect("python3");
    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: std::slice::from_ref(&python),
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })
    .expect("base");
    let rights_before = format!(
        "{:?}",
        (
            &base.landlock.read,
            &base.landlock.read_exec,
            &base.landlock.write,
            &base.landlock.devices,
            &base.landlock.socket_dirs
        )
    );
    let mut jvm_args = Vec::new();
    plan.attach(&mut base.mounts, &mut jvm_args, &mut base.landlock.unix_sockets);
    let rights_after = format!(
        "{:?}",
        (
            &base.landlock.read,
            &base.landlock.read_exec,
            &base.landlock.write,
            &base.landlock.devices,
            &base.landlock.socket_dirs
        )
    );
    assert_eq!(rights_before, rights_after, "no readable, writable or executable path was added to the policy");
    assert_eq!(base.landlock.unix_sockets, [PathBuf::from("/bastion/companion/channel")]);
    base.landlock.tcp = TcpPolicy::Ports(vec![]);

    let script = r#"
import errno, hashlib, os, socket
def attempt(f):
    try:
        f(); return "ok"
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
def show(k, v): print(f"{k}={v}", flush=True)
while os.read(0, 4096): pass   # until the test says "go on" by closing our standard input
d = "/bastion/companion"
jar, settings, channel = d + "/bastion-companion.jar", d + "/config.properties", d + "/channel"
show("listing", ",".join(sorted(os.listdir(d))))
show("jar_sha256", hashlib.sha256(open(jar, "rb").read()).hexdigest())
show("settings", open(settings).read().replace("\n", "|"))
show("write_jar", attempt(lambda: open(jar, "ab")))
show("write_settings", attempt(lambda: open(settings, "w")))
show("unlink_jar", attempt(lambda: os.unlink(jar)))
show("rename_settings", attempt(lambda: os.rename(settings, d + "/renamed")))
show("chmod_jar", attempt(lambda: os.chmod(jar, 0o755)))
show("create", attempt(lambda: open(d + "/another.jar", "w")))
show("mkdir", attempt(lambda: os.mkdir(d + "/sub")))
show("exec_jar", attempt(lambda: os.execv(jar, [jar])))
show("bind_beside", attempt(lambda: socket.socket(socket.AF_UNIX, socket.SOCK_STREAM).bind(d + "/other")))
show("unlink_channel", attempt(lambda: os.unlink(channel)))
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
show("connect", attempt(lambda: s.connect(channel)))
s.sendall(b"hello 1 9.9.9 zoom\n\x1b[31mnot the protocol\n")
s.close()
"#;
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::None,
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: python,
            argv: vec!["python3".into(), "-I".into(), "-c".into(), script.into()],
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let mut running = sandbox::spawn(spec, Path::new(env!("CARGO_BIN_EXE_bastion-mc"))).expect("spawn sandbox");

    // As the launch does: serve once the sandbox has the socket, and take its name away.
    // The payload waits meanwhile, so everything it does happens in the state a game finds:
    // the socket's name no longer exists on the host.
    let said = Arc::new(Mutex::new(Vec::new()));
    let sink = said.clone();
    let has_socket = running.payload_pid().await.is_some();
    assert!(has_socket, "the payload is running (it waits for its standard input to end)");
    plan.release_name();
    let serving =
        plan.serve(Arc::new(move |voice, line| sink.lock().unwrap().push((voice, line)))).expect("a channel");
    let leftovers: Vec<_> = std::fs::read_dir(dir.join("rt")).unwrap().flatten().map(|e| e.path()).collect();
    assert!(
        leftovers.iter().all(|p| std::fs::read_dir(p).map(|d| d.count() == 0).unwrap_or(true)),
        "the socket's name is gone from the host once the sandbox holds it: {leftovers:?}"
    );
    running.channel.shutdown(std::net::Shutdown::Write).unwrap(); // "go on"

    let (mut stdout, mut stderr) = (String::new(), String::new());
    let (mut out, mut err) = (running.child.stdout.take().unwrap(), running.child.stderr.take().unwrap());
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut stdout), err.read_to_string(&mut stderr));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("the payload timed out");
    let line = |key: &str| -> String {
        stdout
            .lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .unwrap_or_else(|| panic!("no `{key}=` line.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"))
            .to_string()
    };
    assert_eq!(status.code(), Some(0), "{stdout}\n{stderr}");

    // What the game finds: exactly these three, the jar byte for byte, the settings as rendered.
    assert_eq!(line("listing"), "bastion-companion.jar,channel,config.properties");
    assert_eq!(line("jar_sha256"), util::hex_encode(&util::hash_bytes(util::HashAlg::Sha256, companion::JAR)));
    assert!(line("settings").contains("|zoom=true|zoom.key=key.keyboard.c|"), "{}", line("settings"));
    assert!(line("settings").ends_with("|channel=/bastion/companion/channel|"), "{}", line("settings"));
    // Nothing of it can be changed, replaced, added to or run. (Which refusal comes first —
    // read-only mount, Landlock, a busy mount point — is the kernel's business.)
    for denied in [
        "write_jar",
        "write_settings",
        "unlink_jar",
        "rename_settings",
        "chmod_jar",
        "create",
        "mkdir",
        "exec_jar",
        "bind_beside",
        "unlink_channel",
    ] {
        assert!(
            matches!(line(denied).as_str(), "EACCES" | "EROFS" | "EBUSY" | "EPERM" | "EXDEV" | "ENOEXEC"),
            "{denied} = {}",
            line(denied)
        );
    }
    // The one new thing it can do: talk to the launcher, which logs it and nothing more.
    assert_eq!(line("connect"), "ok");
    let deadline = Instant::now() + Duration::from_secs(10);
    while serving.stats().strangers < 1 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let stats = serving.stats();
    assert_eq!((stats.accepted, stats.hellos, stats.strangers), (1, 1, 1), "{stats:?}");
    assert_eq!(
        serving.finish(),
        "companion channel: 1 connection(s), 1 hello, 0 diagnostic(s), 1 line(s) that are not part of the protocol"
    );
    let said = said.lock().unwrap().clone();
    assert_eq!(
        said[0],
        (Voice::Claimed, "companion channel #1: hello: mod version 9.9.9, protocol 1, features: zoom".to_string())
    );
    assert!(said[1].0 == Voice::Claimed && !said[1].1.contains('\x1b'), "{:?}", said[1]);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The mod itself, in the system JVM, in the sandbox as a launch assembles it — started
/// through the launch bootstrap like the game, with the injection's files, option and socket.
/// There is no Minecraft, so the one feature the default configuration switches on reports
/// itself off; what is tested is everything around that: the JVM loads the read-only jar,
/// the mod reads the launcher's settings, and its report reaches the launcher through
/// Landlock and seccomp.
///
/// And it reads *only* the launcher's settings. The program first writes a settings file of
/// its own where the game can write, and the JVM carries the mod's settings property naming
/// that file — after the launcher's options, where a signed profile's would be. Had the mod
/// read it, other features would be on and there would be no channel.
#[tokio::test(flavor = "multi_thread")]
async fn a_jvm_in_the_sandbox_runs_the_mod_on_the_launchers_settings_not_on_a_file_the_game_wrote() {
    use std::io::Write;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    const PLANTED: &str = "/game/planted.properties";
    let dir = scratch("jvm");
    let jar = dir.join("bastion-companion.jar");
    std::fs::write(&jar, companion::JAR).unwrap();
    let classes = dir.join("classes");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/java/CompanionSandboxed.java");
    let output = Command::new(javac())
        .args(["--release", "17", "-proc:none", "-Xlint:all", "-Werror", "-cp"])
        .arg(&jar)
        .arg("-d")
        .arg(&classes)
        .arg(source)
        .output()
        .expect("javac");
    assert!(output.status.success(), "javac failed:\n{}", String::from_utf8_lossy(&output.stderr));

    let jvms = java::discover().expect("system JVMs");
    let jvm = java::select(&jvms, companion::MIN_JAVA, JavaMatch::Newer, None).expect("a JVM");
    let paths = Paths {
        config: dir.join("config"),
        data: dir.join("data"),
        cache: dir.join("cache"),
        state: dir.join("state"),
        runtime: Some(dir.join("rt")),
    };
    util::create_private_dir_all(&dir.join("rt")).unwrap();
    let plan = companion::plan(&CompanionConfig::default(), &fabric_instance(), jvm.major, &paths).expect("plan");
    assert!(plan.injected(), "{}", plan.describe());

    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: Some(&jvm),
        exec_allow: &[],
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })
    .expect("base");
    let mut jvm_args = Vec::new();
    plan.attach(&mut base.mounts, &mut jvm_args, &mut base.landlock.unix_sockets);
    base.mounts.push(Mount::RoData {
        data: BOOTSTRAP_CLASS.to_vec(),
        dst: format!("/bastion/boot/{BOOTSTRAP_CLASS_NAME}.class").into(),
        mode: 0o444,
    });
    base.mounts.push(Mount::RoBind { src: classes, dst: "/bastion/test".into() });
    // The game directory: the one place the game can write.
    base.mounts.push(Mount::Dir { dst: "/game".into(), mode: 0o700 });
    base.landlock.write.push("/game".into());
    base.landlock.tcp = TcpPolicy::Ports(vec![]);

    let class_path = "/bastion/boot:/bastion/companion/bastion-companion.jar:/bastion/test";
    let mut argv = vec!["java".to_string(), "-Xmx64m".into(), "-cp".into(), class_path.into()];
    // The launcher's own option (nothing reads it here), then — as a profile's options
    // follow the launcher's and win — the mod's settings property, naming the planted file.
    argv.extend(jvm_args);
    argv.push(format!("-Dbastion.companion.config={PLANTED}"));
    argv.extend(java::hardening_flags(jvm.major, false));
    argv.push(BOOTSTRAP_CLASS_NAME.into());
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::None,
        cwd: "/game".into(),
        limits: None,
        policy: InitPolicy {
            exec: jvm.java.clone(),
            argv,
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let frame = build_frame("CompanionSandboxed", &[PLANTED.to_string()], &Vars::default()).unwrap();
    let mut running = sandbox::spawn(spec, Path::new(env!("CARGO_BIN_EXE_bastion-mc"))).expect("spawn sandbox");
    running.channel.write_all(frame.expose()).unwrap();
    running.channel.shutdown(std::net::Shutdown::Write).unwrap();

    let said = Arc::new(Mutex::new(Vec::new()));
    let sink = said.clone();
    if running.payload_pid().await.is_some() {
        plan.release_name();
    }
    let serving =
        plan.serve(Arc::new(move |voice, line| sink.lock().unwrap().push((voice, line)))).expect("a channel");

    // The program's standard error as it comes (the mod logs there: no SLF4J in this JVM),
    // its standard output until it says that the mod has started.
    let mut err = running.child.stderr.take().unwrap();
    let logged = tokio::spawn(async move {
        let mut text = String::new();
        let _ = err.read_to_string(&mut text).await;
        text
    });
    let mut out = tokio::io::BufReader::new(running.child.stdout.take().unwrap()).lines();
    let mut stdout = Vec::new();
    let ready = tokio::time::timeout(Duration::from_secs(90), async {
        while let Ok(Some(line)) = out.next_line().await {
            let last = line == "ready=ok";
            stdout.push(line);
            if last {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    // Whatever the mod sent has arrived when the launcher has counted it.
    let deadline = Instant::now() + Duration::from_secs(10);
    while ready && serving.stats().diagnostics < 1 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let stats = serving.stats();
    let summary = serving.finish();
    running.stop(Duration::from_secs(10)).await.expect("stopping the sandbox");
    let stderr = logged.await.unwrap();
    let context = format!("--- stdout ---\n{}\n--- stderr ---\n{stderr}", stdout.join("\n"));
    assert!(ready, "the program did not get as far as starting the mod\n{context}");
    let shown = |key: &str| -> &str {
        stdout
            .iter()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .unwrap_or_else(|| panic!("no `{key}=` line\n{context}"))
    };

    // The precondition: the planted file is there, it says something else than the launcher
    // does, and the property names it.
    assert_eq!(shown("planted"), "zoom=false|hud=true|fullbright=true|");
    assert_eq!(shown("property"), PLANTED);
    // The launcher's settings were used: zoom was switched on (and, with no game to hook
    // into, off again), the other two were never touched, and there is a channel.
    assert_eq!(shown("live"), "false,false,false");
    assert!(stderr.contains("zoom disabled: lookup failed (ClassNotFoundException"), "{context}");
    for never in ["hud disabled", "fullbright disabled", "no launcher channel configured", "no settings file"] {
        assert!(!stderr.contains(never), "the mod read the planted settings ({never:?})\n{context}");
    }
    // And the launcher has its report, through the socket Landlock grants.
    assert_eq!(
        (stats.accepted, stats.hellos, stats.diagnostics, stats.strangers),
        (1, 1, 1, 0),
        "{stats:?}\n{context}"
    );
    assert_eq!(summary, "companion channel: 1 connection(s), 1 hello, 1 diagnostic(s)");
    let said = said.lock().unwrap().clone();
    assert_eq!(
        said[0],
        (
            Voice::Claimed,
            format!("companion channel #1: hello: mod version {}, protocol 1, features: none", companion::VERSION)
        )
    );
    assert!(
        said[1].0 == Voice::Claimed
            && said[1].1.starts_with("companion channel #1: zoom: disabled: lookup failed (ClassNotFoundException"),
        "{:?}",
        said[1]
    );
    std::fs::remove_dir_all(dir).unwrap();
}

// ---------------------------------------------------------------- the stand-ins

fn find_jar(libraries: &Path, artifact: &str) -> PathBuf {
    let dir = libraries.join("net/fabricmc").join(artifact);
    let mut found = Vec::new();
    for version in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).flatten() {
        for file in std::fs::read_dir(version.path()).into_iter().flatten().flatten() {
            if file.path().extension().is_some_and(|e| e == "jar") {
                found.push(file.path());
            }
        }
    }
    found.sort();
    found.pop().unwrap_or_else(|| panic!("no {artifact} jar below {}", dir.display()))
}

/// What `javap -v -p` says about a class, reduced to what must agree: each member's
/// declaration (without `throws`) and descriptor, with its annotation default if it has
/// one; and, under the key "<meta>", the `@Target` and `@Retention` of an annotation type.
fn shape(class_path: &Path, class: &str) -> BTreeMap<String, String> {
    let output = Command::new("javap").args(["-v", "-p", "-cp"]).arg(class_path).arg(class).output().expect("javap");
    assert!(output.status.success(), "javap {class}: {}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let mut members = BTreeMap::new();
    let mut meta = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(descriptor) = trimmed.strip_prefix("descriptor: ") {
            let declaration = lines[at - 1].trim().split(" throws ").next().unwrap().trim_end_matches(';').to_string();
            // The block of this member ends at the next blank line, or with the class.
            let block: Vec<&str> =
                lines[at..].iter().map(|l| l.trim()).take_while(|l| !l.is_empty() && *l != "}").collect();
            let default = block
                .iter()
                .position(|l| l.starts_with("default_value:"))
                // The constant-pool indices on that line differ between two class files;
                // the lines after it spell the value out.
                .map(|i| block[i + 1..].join(" "))
                .unwrap_or_default();
            members.insert(format!("{declaration} {descriptor}"), default);
        }
        if trimmed.starts_with("java.lang.annotation.Target(") || trimmed.starts_with("java.lang.annotation.Retention(")
        {
            meta.push(format!("{trimmed} {}", lines[at + 1].trim()));
        }
    }
    meta.sort();
    members.insert("<meta>".into(), meta.join(" "));
    members
}

#[test]
fn the_stand_ins_match_the_real_fabric_and_mixin_types() {
    let Some(root) = std::env::var_os("BASTION_E2E_ROOT") else {
        eprintln!("skipped: set BASTION_E2E_ROOT to a data root that holds Fabric Loader and Mixin");
        return;
    };
    let libraries = PathBuf::from(root).join("data/bastion-mc/libraries");
    let real = [find_jar(&libraries, "fabric-loader"), find_jar(&libraries, "sponge-mixin")];
    println!("checked against {} and {}", real[0].display(), real[1].display());
    let real_path = std::env::join_paths(&real).unwrap();
    let dir = scratch("stubs");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    // 1. Compiled against the real jars instead of the stand-ins, the mod is the same bytes:
    //    for everything the mod uses, the two are indistinguishable to the compiler —
    //    including where an annotation ends up in the class file (its retention).
    let against_real = jar::build_against(root, &javac(), companion::VERSION, &dir.join("real"), Some(&real))
        .expect("building against the real jars");
    assert!(against_real.jar == companion::JAR, "the mod differs when compiled against the real jars");
    assert_eq!(against_real.warnings.trim(), "", "javac has remarks when the real types are in view");

    // 2. Every stand-in declares only members the real type has, with the same descriptor
    //    and, for annotation members, the same default; annotation types have the same
    //    @Target and @Retention.
    let built = jar::build(root, &javac(), companion::VERSION, &dir.join("ours")).unwrap();
    assert!(built.jar == companion::JAR);
    let stubs = dir.join("ours/stubs");
    let mut classes = Vec::new();
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().into_string().unwrap();
            if entry.path().is_dir() {
                walk(&entry.path(), &format!("{prefix}{name}."), out);
            } else if let Some(class) = name.strip_suffix(".class") {
                out.push(format!("{prefix}{class}"));
            }
        }
    }
    walk(&stubs, "", &mut classes);
    classes.sort();
    assert_eq!(
        classes,
        [
            "net.fabricmc.api.ClientModInitializer",
            "org.spongepowered.asm.mixin.Mixin",
            "org.spongepowered.asm.mixin.injection.At",
            "org.spongepowered.asm.mixin.injection.Coerce",
            "org.spongepowered.asm.mixin.injection.Inject",
            "org.spongepowered.asm.mixin.injection.callback.CallbackInfo",
            "org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable",
        ],
        "a stand-in was added or removed: extend this test's expectations with it"
    );
    let mut compared = 0;
    for class in &classes {
        let (ours, theirs) = (shape(&stubs, class), shape(Path::new(&real_path), class));
        for (member, default) in &ours {
            let real_default = theirs.get(member).unwrap_or_else(|| {
                panic!("{class}: the stand-in declares `{member}`, the real type does not. It has:\n{theirs:#?}")
            });
            assert_eq!(default, real_default, "{class}: `{member}` differs");
            compared += 1;
        }
    }
    println!("{compared} members and annotation properties of {} types agree", classes.len());
    // The comparison itself reads what it claims to read: spot checks on the real types.
    let inject = shape(Path::new(&real_path), "org.spongepowered.asm.mixin.injection.Inject");
    assert_eq!(inject["public abstract int require() ()I"], "-1");
    assert_eq!(inject["public abstract boolean cancellable() ()Z"], "false");
    assert!(inject["<meta>"].contains("RetentionPolicy;.RUNTIME") && inject["<meta>"].contains("ElementType;.METHOD"));
    let mixin = shape(Path::new(&real_path), "org.spongepowered.asm.mixin.Mixin");
    assert!(mixin["<meta>"].contains("RetentionPolicy;.CLASS"), "{mixin:#?}");
    let coerce = shape(Path::new(&real_path), "org.spongepowered.asm.mixin.injection.Coerce");
    assert!(
        coerce["<meta>"].contains("ElementType;.PARAMETER") && !coerce["<meta>"].contains("Retention"),
        "{coerce:#?}"
    );
    assert!(compared >= 2 * classes.len(), "the comparison looked at too little: {compared}");
    std::fs::remove_dir_all(dir).unwrap();
}
