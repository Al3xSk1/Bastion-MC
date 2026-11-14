//! The companion mod in the real game, through the whole launch path: injected into a signed
//! Fabric profile, loaded by Fabric Loader, its mixins applied, its hello received by the
//! launcher — and each feature measured by the mod's scripted self-test.
//!
//! Opt-in, and alone in this file on purpose: in step 9 a launch confines the launcher's own
//! process (Landlock and a system-call filter, every thread) — here, the test process.
//!
//! ```sh
//! DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=fab \
//!     [BASTION_COMPANION_WORLD=1] [BASTION_NESTED_LAYERS=…] cargo test --test companion_game -- --nocapture
//! ```
//!
//! Which display the game gets is the launcher's decision, as in any launch: on an X11
//! session with gamescope installed (or given in `BASTION_NESTED_LAYERS`), a nested
//! compositor. `BASTION_COMPANION_DIRECT_X11=1` switches that off for the test
//! (`sandbox.nested_compositor = "off"`), so that the game is a client of `$DISPLAY` itself —
//! which the data root's configuration must allow (`allow_x11`). With `WAYLAND_DISPLAY` set
//! (and no `DISPLAY`), it is the Wayland path: a security context where the compositor has
//! one. The test prints which it was, in the launcher's words (`display  | …`).
//!
//! `BASTION_COMPANION_KEYPRESS=1` (with `BASTION_COMPANION_WORLD`): the zoom key is *really*
//! pressed, twice, by `tests/support/x11-key.py` through the X server's XTEST extension —
//! once while playing, once with the chat open. The key goes to whatever has the keyboard
//! focus on `$DISPLAY`: only ever on a private display that nothing else is using.
//!
//! `BASTION_E2E_ROOT` holds `config/`, `data/`, `cache/` and `state/` (the four XDG roots)
//! with a signed Fabric profile of a tested Minecraft version already installed. The game is
//! started in demo mode: no account is involved. A window opens on the display named by the
//! environment, so use one nobody is looking at (`tests/support/private-x11.sh`).
//!
//! Without `BASTION_COMPANION_WORLD` the self-test stays at the title screen: it calls the
//! camera's real field-of-view method (with the injected callback in it) under a scripted key
//! and clock, and the other hooks with objects of the game's own types. With it, the
//! self-test then enters Mojang's demo world the way the title screen's button does — it is
//! created in the instance's `saves/` on first use — and reads the camera, the HUD and the
//! light map of the running world. Nothing presses a key or moves a mouse in either mode:
//! the key's state is scripted inside the mod. (At its end the self-test watches the camera
//! for a few seconds with nothing scripted, while playing and with the chat open: nothing
//! may zoom then — unless the key is really pressed, see `BASTION_COMPANION_KEYPRESS`.)

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

use bastion_mc::{
    companion::{self, CompanionConfig, SelfTest},
    config::{Config, IsolationMode},
    events::{Reporter, StopSignal},
    launch::{LaunchOptions, launch},
    paths::Paths,
    util,
};
use bastion_mc_proto::{Event, Source};

fn log_lines(reporter: &Reporter, source: Source) -> Vec<String> {
    reporter
        .captured()
        .into_iter()
        .filter_map(|event| match event {
            Event::Log { source: s, line } if s == source => Some(line),
            _ => None,
        })
        .collect()
}

/// The text after `selftest: ` of every self-test line, in order.
fn self_test_lines(game: &[String]) -> Vec<String> {
    game.iter()
        .filter_map(|line| {
            let text = &line[line.find("selftest: ")? + "selftest: ".len()..];
            Some(text.split("]]>").next().unwrap_or(text).trim().to_string())
        })
        .collect()
}

/// `name=value` fields of the first self-test line that starts with `prefix`.
fn measured(lines: &[String], prefix: &str) -> BTreeMap<String, String> {
    let line = lines.iter().find(|l| l.starts_with(prefix)).unwrap_or_else(|| panic!("no self-test line {prefix:?}"));
    line.split(' ').filter_map(|field| field.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn number(fields: &BTreeMap<String, String>, name: &str) -> f64 {
    fields
        .get(name)
        .and_then(|v| v.split('/').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no number {name} in {fields:?}"))
}

fn close(actual: f64, expected: f64, tolerance: f64, what: &str) {
    assert!((actual - expected).abs() <= tolerance, "{what}: {actual}, expected {expected} ± {tolerance}");
}

/// What the tangent rule gives for a field of view narrowed from `base` to `zoomed` degrees.
fn turn_scale(base: f64, zoomed: f64) -> f64 {
    (zoomed.to_radians() / 2.0).tan() / (base.to_radians() / 2.0).tan()
}

/// `tests/support/x11-key.py`: presses a key on `$DISPLAY` each time it is told to. Started
/// before the launch — the launch ends with this process confined, and a confined process
/// starts nothing — and told through a pipe that is already open by then.
struct KeyPresser {
    child: Child,
    said: BufReader<ChildStdout>,
}

/// The keysym of the default zoom key (`c`) and how long it is held, as the helper reads it.
const PRESS_ZOOM_KEY: &str = "0x63 1500";

impl KeyPresser {
    fn start() -> (KeyPresser, ChildStdin) {
        let mut child = Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/x11-key.py"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("starting tests/support/x11-key.py");
        let orders = child.stdin.take().expect("piped");
        let mut said = BufReader::new(child.stdout.take().expect("piped"));
        let mut line = String::new();
        said.read_line(&mut line).expect("the key helper's first line");
        assert_eq!(line.trim(), "ready", "the key helper could not use the display");
        (KeyPresser { child, said }, orders)
    }

    /// What the helper reported, once its orders have ended.
    fn finish(mut self) -> Vec<String> {
        let lines = self.said.lines().map_while(Result::ok).collect();
        let _ = self.child.wait();
        lines
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_companion_loads_in_the_real_game_and_its_features_do_what_they_say() {
    let Some(root) = std::env::var_os("BASTION_E2E_ROOT") else {
        eprintln!("skipped: set BASTION_E2E_ROOT (and BASTION_E2E_INSTANCE, a signed Fabric profile; default \"fab\")");
        return;
    };
    let instance = std::env::var("BASTION_E2E_INSTANCE").unwrap_or_else(|_| "fab".into());
    let world = std::env::var_os("BASTION_COMPANION_WORLD").is_some_and(|v| !v.is_empty());
    let keypress = std::env::var_os("BASTION_COMPANION_KEYPRESS").is_some_and(|v| !v.is_empty());
    assert!(world || !keypress, "BASTION_COMPANION_KEYPRESS needs BASTION_COMPANION_WORLD");
    let direct_x11 = std::env::var_os("BASTION_COMPANION_DIRECT_X11").is_some_and(|v| !v.is_empty());
    let root = PathBuf::from(root);
    let app = |kind: &str| root.join(kind).join("bastion-mc");
    let paths = Paths {
        config: app("config"),
        data: app("data"),
        cache: app("cache"),
        state: app("state"),
        runtime: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    };
    let mut cfg = Config::load(&paths).expect("configuration");
    cfg.sandbox.nested_compositor_layers = std::env::var_os("BASTION_NESTED_LAYERS")
        .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
        .unwrap_or_default();
    if direct_x11 {
        cfg.sandbox.nested_compositor = IsolationMode::Off;
    }
    // Every feature on, with the documented defaults, and the scripted self-test.
    cfg.companion = CompanionConfig {
        hud: true,
        fullbright: true,
        self_test: Some(if world { SelfTest::World } else { SelfTest::Title }),
        ..CompanionConfig::default()
    };
    let (factor, gamma) = (cfg.companion.zoom_factor, cfg.companion.fullbright_gamma);
    cfg.validate().expect("valid configuration");

    let reporter = Reporter::capture();
    let stop = StopSignal::never();
    let opts = LaunchOptions {
        instance,
        account: "default".into(),
        demo: true,
        burner: false,
        join: None,
        repair: false,
        stop: stop.clone(),
        program: env!("CARGO_BIN_EXE_bastion-mc").into(),
    };

    // Stop the game once the self-test has reported, or after a generous while. Meanwhile,
    // if asked to: each time the self-test says that it is watching the camera, have the
    // zoom key pressed.
    let (presser, mut orders) = match keypress.then(KeyPresser::start) {
        Some((presser, orders)) => (Some(presser), Some(orders)),
        None => (None, None),
    };
    let watcher = {
        let (reporter, stop) = (reporter.clone(), stop.clone());
        tokio::spawn(async move {
            let deadline = Instant::now() + Duration::from_secs(if world { 600 } else { 300 });
            let mut pressed = 0;
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let lines = self_test_lines(&log_lines(&reporter, Source::Game));
                let watching = lines.iter().filter(|l| l.starts_with("world watching ")).count();
                if let Some(orders) = orders.as_mut().filter(|_| watching > pressed) {
                    pressed = watching;
                    let _ = writeln!(orders, "{PRESS_ZOOM_KEY}").and_then(|_| orders.flush());
                }
                if lines.iter().any(|l| l.starts_with("done") || l.starts_with("failed")) {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    break;
                }
            }
            stop.trigger();
        })
    };
    let outcome = launch(&paths, &cfg, &opts, &reporter).await;
    watcher.abort();
    let _ = watcher.await; // gone, and with it the helper's orders
    let presses = presser.map(KeyPresser::finish);

    let launcher = log_lines(&reporter, Source::Launcher);
    let game = log_lines(&reporter, Source::Game);
    let tests = self_test_lines(&game);
    for line in launcher.iter().filter(|l| l.contains("companion")) {
        println!("launcher | {line}");
    }
    // Which display the game had, in the launcher's own words (what a front end shows as the
    // display's protection): a nested compositor, a Wayland security context, or the
    // session's display itself.
    let display = reporter
        .captured()
        .into_iter()
        .find_map(|event| match event {
            Event::Protection { items } => items.into_iter().find(|item| item.aspect == "display").map(|i| i.detail),
            _ => None,
        })
        .unwrap_or_else(|| "not reported".into());
    println!("display  | {display}");
    assert!(
        !direct_x11 || display.starts_with("the session's X11 display"),
        "BASTION_COMPANION_DIRECT_X11 was set, yet the game's display was: {display}"
    );
    for line in game.iter().filter(|l| l.contains("companion channel") || l.contains("bastion-companion")) {
        println!("game     | {}", line.trim());
    }
    for line in &tests {
        println!("selftest | {line}");
    }
    let failure = || {
        format!(
            "outcome {outcome:?}\n--- launcher ---\n{}\n--- game (tail) ---\n{}",
            launcher.join("\n"),
            game.iter().rev().take(60).rev().cloned().collect::<Vec<_>>().join("\n")
        )
    };

    // The launcher says what it injected: version, digest, features.
    let digest = util::hex_encode(&util::hash_bytes(util::HashAlg::Sha256, companion::JAR));
    let injected = launcher.iter().find(|l| l.starts_with("companion: ")).unwrap_or_else(|| panic!("{}", failure()));
    assert!(
        injected.contains(&format!("bastion-companion {} added", companion::VERSION))
            && injected.contains(&format!("sha256 {digest}"))
            && injected.contains("features: zoom, hud, fullbright;"),
        "{injected}"
    );

    // Fabric Loader lists it among the mods it loads.
    assert!(
        game.iter().any(|l| l.trim() == format!("- bastion-companion {}", companion::VERSION)),
        "Fabric Loader does not list the mod\n{}",
        failure()
    );

    // Mixin applied every mixin of the mod without complaint: it reports nothing at warning
    // level or above, never mentions the mod's mixins (it does so only to report a problem),
    // and the mod itself found each of its hooks in place — or it would have switched the
    // feature off, said so, and left it out of its hello.
    let mut complaints = Vec::new();
    for line in &game {
        let lower = line.to_lowercase();
        let about_us = lower.contains("mixin") || lower.contains("bastion-companion");
        let severe = ["level=\"warn\"", "level=\"error\"", "level=\"fatal\"", "/warn]", "/error]", "/fatal]"]
            .iter()
            .any(|mark| lower.contains(mark));
        if (about_us && severe)
            || lower.contains("bastion-companion.mixins.json")
            || lower.contains("bastion.companion.mixin")
        {
            complaints.push(line.clone());
        }
    }
    assert!(complaints.is_empty(), "warnings or errors about the mod or from Mixin:\n{}", complaints.join("\n"));
    assert!(!tests.iter().any(|l| l.contains(" off") || l.contains("failed")), "{tests:#?}");

    // The hello arrived at the launcher, and the launcher logged it as something claimed
    // from inside the game.
    let hello = format!(
        "companion channel #1: hello: mod version {}, protocol 1, features: zoom, hud, fullbright",
        companion::VERSION
    );
    assert!(game.contains(&hello), "no hello in the log\n{}", failure());
    let summary =
        launcher.iter().find(|l| l.starts_with("companion channel: ")).unwrap_or_else(|| panic!("{}", failure()));
    assert_eq!(summary, "companion channel: 1 connection(s), 1 hello, 0 diagnostic(s)");

    // The game got as far as the title screen: that is the screen the self-test found once
    // the game reported that it had finished loading.
    let loaded = measured(&tests, "loaded ");
    assert_eq!(loaded["screen"], "net.minecraft.client.gui.screens.TitleScreen", "{tests:#?}");
    assert_eq!(loaded["player"], "none");
    assert!(tests.iter().any(|l| l.starts_with("done")), "the self-test did not finish: {tests:#?}\n{}", failure());

    // Zoom, through the camera's own method.
    let zoom = measured(&tests, "zoom ");
    let idle = number(&zoom, "fov_idle");
    assert!((30.0..=110.0).contains(&idle), "the game's own field of view: {idle}");
    assert_eq!(zoom["key"], "key.keyboard.c");
    assert_eq!(zoom["screen_open"], "true", "the title screen is a screen");
    close(number(&zoom, "fov_key_held_real_state"), idle, 0.0, "with a screen open the key does nothing");
    let zoomed = number(&zoom, "fov_zoomed");
    close(zoomed, idle / factor, 0.01, "held, as if playing: the angle divided by the factor");
    let first = number(&zoom, "fov_first_frame");
    assert!(first < idle && first > zoomed + 1.0, "the transition in is gradual: {idle} → {first} → {zoomed}");
    close(number(&zoom, "turn_scale"), turn_scale(idle, zoomed), 1e-4, "mouse movement scales with the picture");
    close(number(&zoom, "moved"), 100.0 * turn_scale(idle, zoomed), 0.01, "a movement of 100 while zoomed");
    assert_eq!(zoom["wheel_used"], "true", "the wheel is taken while the key is held");
    close(number(&zoom, "wheel_factor"), factor * 1.15, 1e-3, "one notch");
    close(number(&zoom, "fov_wheel"), idle / (factor * 1.15), 0.01, "the angle after one notch");
    let out = number(&zoom, "fov_first_frame_out");
    assert!(out > number(&zoom, "fov_wheel") + 1.0 && out < idle, "the transition out is gradual: {out}");
    close(number(&zoom, "fov_after"), idle, 0.0, "released: exactly the game's own angle again");
    assert_eq!(zoom["wheel_used_when_released"], "false", "the wheel is the game's again");

    // HUD readout: outside a world it is the frame rate alone.
    let hud = tests.iter().find(|l| l.starts_with("hud ")).expect("hud line");
    assert!(hud.contains("lines=[") && hud.contains(" fps]"), "{hud}");

    // Fullbright: the game's brightness raised to the configured value, the Darkness effect's
    // part still subtracted.
    let bright = measured(&tests, "fullbright ");
    let option = number(&bright, "option");
    assert!((0.0..=1.0).contains(&option), "the game's own brightness option: {option}");
    close(number(&bright, "brightness"), gamma, 1e-3, "raised to the configured value");
    close(number(&bright, "brightness_under_darkness_0.3"), gamma - option.min(0.3), 1e-3, "minus the darkness pulse");

    if world {
        let entered = measured(&tests, "world entered ");
        println!("in the world: {entered:?}");
        // The self-test measures only once nothing has been in front of the world for a
        // while (the demo's greeting arrives a moment after the world does).
        assert_eq!(entered["screen"], "none", "a screen was open while the self-test measured: {tests:#?}");
        let zoom = measured(&tests, "world zoom ");
        let (idle, zoomed) = (number(&zoom, "fov_idle"), number(&zoom, "fov_zoomed"));
        close(zoomed, idle / factor, 0.05, "in the world, the camera's field of view with the key held");
        close(number(&zoom, "fov_after"), idle, 0.0, "and after releasing it");
        assert_eq!(zoom["zoom_live"], "true");
        let (plain, narrow) = (number(&zoom, "turn_plain"), number(&zoom, "turn_zoomed"));
        if plain.abs() > 0.5 {
            // The game turned the player by its own mouse handling: zoomed, the same
            // movement turns by the tangent ratio of the two angles.
            close(narrow / plain, turn_scale(idle, zoomed), 0.02, "the turn for the same mouse movement, zoomed");
        } else {
            println!(
                "NOT MEASURED: the game did not turn the player for injected mouse movement (window_active={}, \
                 mouse_grabbed={}), so the sensitivity scaling was not observed in the world",
                entered["window_active"], entered["mouse_grabbed"]
            );
        }
        // The wheel through the game's own scroll method: the zoom's while the key is held
        // (the hotbar does not move), the hotbar's otherwise.
        let wheel = measured(&tests, "world wheel ");
        assert_eq!(wheel["slot_after_notch_while_zoomed"], wheel["slot_before"], "{wheel:?}");
        close(number(&wheel, "factor_before"), factor, 1e-3, "the configured factor before the notch");
        close(number(&wheel, "factor_after"), factor * 1.15, 1e-3, "one notch while zoomed");
        assert_ne!(wheel["slot_after_notch_released"], wheel["slot_before"], "{wheel:?}");
        // With nothing scripted: the camera while playing, and with the chat open.
        let watching_chat = measured(&tests, "world watching state=chat ");
        assert_eq!(watching_chat["screen"], "net.minecraft.client.gui.screens.ChatScreen");
        let (playing, chat) =
            (measured(&tests, "world watched state=playing "), measured(&tests, "world watched state=chat "));
        let typed = tests.iter().find(|l| l.starts_with("world chat typed=")).expect("what the chat received");
        match &presses {
            Some(presses) => {
                // The key was really pressed, by the X server as far as the game can tell.
                println!("key presses: {presses:?}; {typed}");
                assert_eq!(presses.len(), 2, "one press while playing, one with the chat open: {presses:?}");
                assert!(presses.iter().all(|p| p.starts_with("pressed keysym=0x63 ")), "{presses:?}");
                close(number(&playing, "fov_min"), idle / factor, 0.05, "the zoom key, really pressed while playing");
                close(number(&playing, "fov_end"), idle, 0.0, "and released again");
                close(number(&chat, "fov_min"), idle, 0.0, "the same key with the chat open does not zoom");
                // There it is a letter — several, the key repeats while it is held.
                let letters = typed.strip_prefix("world chat typed=[").and_then(|t| t.strip_suffix(']')).unwrap_or("");
                assert!(!letters.is_empty() && letters.bytes().all(|b| b == b'c'), "{typed}");
            }
            None => {
                close(number(&playing, "fov_min"), idle, 0.0, "nobody pressed the key: nothing zooms");
                close(number(&chat, "fov_min"), idle, 0.0, "nobody pressed the key: nothing zooms");
                assert_eq!(typed, "world chat typed=[]");
            }
        }
        let hud = measured(&tests, "world hud ");
        assert!(number(&hud, "frames_drawn_in_1s") >= 1.0, "the readout is drawn every frame: {hud:?}");
        let text = tests.iter().find(|l| l.starts_with("world hud ")).unwrap();
        assert!(text.contains(" fps | XYZ ") && text.contains(" | facing "), "{text}");
        // The compass word is the game's own idea of where the player faces.
        assert!(text.contains(&format!("facing {} (", hud["direction"])), "{text}");
        assert_eq!(hud["hud_live"], "true");
        // And it is not drawn with the HUD hidden or the game's debug overlay up, comes back
        // afterwards, and keeps position and facing to itself under reduced debug information.
        let conditions = measured(&tests, "world hud_conditions ");
        assert_eq!(conditions["frames_while_hud_hidden"], "0", "{conditions:?}");
        assert_eq!(conditions["frames_while_debug_overlay"], "0", "{conditions:?}");
        assert!(number(&conditions, "frames_afterwards") >= 1.0, "{conditions:?}");
        assert_eq!(conditions["hud_live"], "true");
        let reduced = tests.iter().find(|l| l.starts_with("world hud_conditions ")).unwrap();
        let reduced = &reduced[reduced.find("lines_with_reduced_debug_info=").unwrap()..];
        assert!(reduced.ends_with(" fps]") && !reduced.contains('|') && !reduced.contains("XYZ"), "{reduced}");
        let bright = measured(&tests, "world fullbright ");
        close(number(&bright, "brightness"), gamma, 1e-3, "the light map's brightness in the running world");
        assert!(number(&bright, "applied") >= 1.0);
        assert_eq!(bright["fullbright_live"], "true");
    }

    assert_eq!(outcome.as_ref().ok(), Some(&130), "stopped on request\n{}", failure());
}
