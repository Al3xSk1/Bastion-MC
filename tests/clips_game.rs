//! Clips and Spotify in a real launch: the real game, the real recorder in its sandbox, the
//! launcher confined in step 9 — and the phone's apps driven by a script of phone commands
//! (`ClipsConfig::script`), since the companion mod's phone is another package's work.
//!
//! Opt-in. The launch runs in a re-executed child of this test (in step 9 it confines the
//! process that runs it: afterwards nothing can be executed there), and this process checks
//! what it made with ffprobe.
//!
//! ```sh
//! DISPLAY=:84 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e \
//!     [BASTION_E2E_CONFIG=$PWD/target/e2e-media-nested/config] BASTION_E2E_INSTANCE=vanilla-latest \
//!     cargo test --test clips_game -- --nocapture
//! ```
//!
//! `BASTION_E2E_ROOT` holds the four XDG roots with the instance installed;
//! `BASTION_E2E_CONFIG` replaces its `config` root (to choose the nested compositor or the
//! session's X display). The game is started in demo mode, its window opens on `$DISPLAY`:
//! only ever a private display (tests/support/private-x11.sh). "The session bus" is a
//! private dbus-daemon with the stand-in player of `tests/spotify.rs`; the Spotify Web API
//! and album art are off (no network for them in this test).

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use bastion_mc::{
    clips::{ClipsConfig, script::Step},
    config::Config,
    events::{Reporter, StopSignal},
    launch::{LaunchOptions, launch},
    paths::Paths,
    spotify::picture,
    util,
};
use bastion_mc_proto::Event;

#[path = "support/mpris_bus.rs"]
mod mpris_bus;

const CHILD: &str = "BASTION_CLIPS_GAME_CHILD";
const BUFFER_SECONDS: u32 = 20;

fn steps() -> Vec<Step> {
    let step = |ms: u64, line: &str| Step { after: Duration::from_millis(ms), line: line.into() };
    vec![
        // The desktop app over MPRIS, after step 9: the connection made before it carries this.
        step(3000, "spotify volume 30"),
        step(1000, "spotify next"),
        step(1000, "spotify open spotify:track:7ouMYWpwJ422jRcDASZB7P"),
        // The game loads (the title screen takes 20–40 s here); the recorder has its window.
        step(45_000, "clips status"),
        step(500, "clips save 8"),
        step(5000, "clips list"),
        step(1000, "clips thumb {last-clip}"),
        step(1000, "clips rename {last-clip} -- demo title screen"),
        step(1000, "clips save 999"),
        step(1000, "clips list"),
    ]
}

/// The re-executed half: the launch itself, its log written to `<dir>/log.txt`.
#[test]
fn launch_child() {
    let Ok(dir) = std::env::var(CHILD) else { return };
    let dir = PathBuf::from(dir);
    let root = PathBuf::from(std::env::var("BASTION_E2E_ROOT").unwrap());
    let app = |root: &Path, sub: &str| root.join(sub).join("bastion-mc");
    let config_root = std::env::var_os("BASTION_E2E_CONFIG").map(PathBuf::from).unwrap_or_else(|| root.join("config"));
    let paths = Paths {
        config: config_root.join("bastion-mc"),
        data: app(&root, "data"),
        cache: app(&root, "cache"),
        state: app(&root, "state"),
        runtime: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    };
    let mut cfg = Config::load(&paths).expect("configuration");
    cfg.spotify.mpris = true;
    cfg.spotify.album_art = false;
    cfg.spotify.client_id = None;
    cfg.clips = ClipsConfig {
        enabled: true,
        buffer_seconds: BUFFER_SECONDS,
        folder: Some(dir.join("clips")),
        script: Some(steps()),
        ..ClipsConfig::default()
    };
    cfg.validate().expect("valid configuration");
    // Opened now: once the launch has confined this process (step 9), nothing outside the
    // directories it keeps can be opened for writing — an open descriptor keeps working.
    let mut log = std::fs::File::create(dir.join("log.txt")).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let reporter = Reporter::capture();
    let stop = StopSignal::never();
    let opts = LaunchOptions {
        instance: std::env::var("BASTION_E2E_INSTANCE").unwrap(),
        account: "default".into(),
        demo: true,
        burner: false,
        join: None,
        repair: false,
        stop: stop.clone(),
        program: env!("CARGO_BIN_EXE_bastion-mc").into(),
    };
    let outcome = runtime.block_on(async {
        // Stop the game once the script is through (or after a generous while).
        let watcher = {
            let (reporter, stop) = (reporter.clone(), stop.clone());
            tokio::spawn(async move {
                let deadline = Instant::now() + Duration::from_secs(300);
                while Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let done = reporter
                        .captured()
                        .iter()
                        .any(|e| matches!(e, Event::Log { line, .. } if line == "phone script: done"));
                    if done {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        break;
                    }
                }
                stop.trigger();
            })
        };
        let outcome = launch(&paths, &cfg, &opts, &reporter).await;
        watcher.abort();
        outcome
    });
    for event in reporter.captured() {
        match event {
            Event::Log { source, line } => writeln!(log, "[{source:?}] {line}").unwrap(),
            Event::Protection { items } => {
                for item in items {
                    writeln!(log, "[protection] {} {:?} {}", item.aspect, item.shield, item.detail).unwrap();
                }
            }
            Event::Status { text } => writeln!(log, "[status] {text}").unwrap(),
            _ => {}
        }
    }
    writeln!(log, "[outcome] {outcome:?}").unwrap();
}

fn ffprobe(clip: &Path) -> serde_json::Value {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=codec_name,width,height,nb_read_frames:format=duration", "-of", "json"])
        .arg(clip)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn clips_and_spotify_in_a_real_launch() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let (Some(_), Some(_), Some(_)) =
        (std::env::var_os("BASTION_E2E_ROOT"), std::env::var_os("BASTION_E2E_INSTANCE"), std::env::var_os("DISPLAY"))
    else {
        eprintln!("skipped: needs BASTION_E2E_ROOT, BASTION_E2E_INSTANCE and a private DISPLAY");
        return;
    };
    if !mpris_bus::available() {
        return;
    }
    let dir = PathBuf::from(format!("/tmp/bmc-game-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
    util::create_private_dir_all(&dir).unwrap();
    let bus = mpris_bus::PrivateBus::start(&dir);
    let standin = mpris_bus::Standin::start(&bus, "org.mpris.MediaPlayer2.spotify");
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["launch_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, &dir)
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .unwrap();
    let log = std::fs::read_to_string(dir.join("log.txt")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    let show = |prefix: &str| {
        for line in lines.iter().filter(|l| l.contains(prefix)) {
            println!("{line}");
        }
    };
    show("spotify:");
    show("clips:");
    show("clip recorder:");
    show("confined itself");
    show("phone script:");
    show("[protection]");
    show("[outcome]");
    let failure = || log.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    assert!(status.success(), "{}", failure());
    let has = |text: &str| lines.iter().any(|l| l.contains(text));
    assert!(has("[outcome] Ok(130)"), "the game was stopped on request:\n{}", failure());

    // Prepared before step 9, and said so.
    assert!(
        has("spotify: desktop app (MPRIS): connected through xdg-dbus-proxy; the Spotify app is running"),
        "{}",
        failure()
    );
    let recorder = lines
        .iter()
        .find(|l| l.contains("clips: recording the game's window"))
        .unwrap_or_else(|| panic!("{}", failure()));
    assert!(
        recorder.contains(&format!("into a {BUFFER_SECONDS} s buffer")) && recorder.contains("no sound"),
        "{recorder}"
    );
    // The confined launcher keeps exactly the clips folder and the buffer for them.
    let confined = lines
        .iter()
        .find(|l| l.contains("launcher: confined itself, on all threads"))
        .unwrap_or_else(|| panic!("{}", failure()));
    assert!(
        confined.contains("for the phone's apps") && confined.contains(&dir.join("clips").display().to_string()),
        "{confined}"
    );
    assert!(has("[protection] launcher On"), "the whole confinement was in force:\n{}", failure());
    assert!(has("clip recorder: recording "), "{}", failure());

    // MPRIS after step 9, in the real launch: the stand-in got the commands.
    let said = standin.said();
    for expected in ["set Volume 0.3", "call Next", "call OpenUri spotify:track:7ouMYWpwJ422jRcDASZB7P"] {
        assert!(said.iter().any(|l| l == expected), "the stand-in did not get {expected:?}: {said:?}");
    }

    // The clips app: saved, listed, thumbnail, renamed, the out-of-range save refused.
    let saved = lines
        .iter()
        .find(|l| l.contains("phone script: < clips notice saved "))
        .unwrap_or_else(|| panic!("{}", failure()));
    assert!(saved.ends_with("(8.0 s)"), "{saved}");
    let thumb = lines
        .iter()
        .find(|l| l.contains("phone script: < clips frame slot=0 "))
        .unwrap_or_else(|| panic!("{}", failure()));
    let size =
        thumb.split(' ').find(|w| w.contains('x') && w.chars().next().is_some_and(|c| c.is_ascii_digit())).unwrap();
    let (tw, th) = size.split_once('x').unwrap();
    assert!(tw.parse::<u32>().unwrap() <= 320 && th.parse::<u32>().unwrap() <= 320, "{thumb}");
    assert!(has("phone script: < clips notice renamed to demo title screen"), "{}", failure());
    assert!(has(&format!("phone script: < clips notice save takes 1 to {BUFFER_SECONDS} seconds")), "{}", failure());

    // The clip itself.
    let folder = dir.join("clips");
    let clip = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "ts"))
        .expect("a clip in the folder");
    assert!(clip.file_name().unwrap().to_string_lossy().starts_with("demo_title_screen-"), "{}", clip.display());
    let probed = ffprobe(&clip);
    let stream = &probed["streams"][0];
    let duration: f64 = probed["format"]["duration"].as_str().unwrap().parse().unwrap();
    let frames: u64 = stream["nb_read_frames"].as_str().unwrap().parse().unwrap();
    let (width, height) = (stream["width"].as_u64().unwrap(), stream["height"].as_u64().unwrap());
    println!(
        "clip: {} — {duration} s, {frames} frames, {width}×{height}, {} bytes",
        clip.display(),
        std::fs::metadata(&clip).unwrap().len()
    );
    assert_eq!(stream["codec_name"], "h264");
    assert!((duration - 8.0).abs() < 0.6, "{duration} s");
    assert!((220..=250).contains(&frames), "{frames} frames for 8 s at 30 fps");
    assert!(width <= 1280 && width.is_multiple_of(2) && height.is_multiple_of(2));
    let png = clip.with_extension("png");
    let (pw, ph) = picture::png_dimensions(&std::fs::read(&png).unwrap()).unwrap();
    assert!(pw <= 320 && ph <= 320, "{pw}×{ph}");
    // What it shows is the game, not a black window: a frame from the middle, its pixels vary.
    let evidence = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/clips-evidence");
    std::fs::create_dir_all(&evidence).unwrap();
    let frame = evidence.join("clip-frame.png");
    let extracted = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", "4", "-i"])
        .arg(&clip)
        .args(["-frames:v", "1"])
        .arg(&frame)
        .status()
        .unwrap();
    assert!(extracted.success());
    let gray = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&frame)
        .args(["-vf", "scale=32:18,format=gray", "-f", "rawvideo", "-"])
        .output()
        .unwrap()
        .stdout;
    let mean = gray.iter().map(|&b| f64::from(b)).sum::<f64>() / gray.len() as f64;
    let spread = (gray.iter().map(|&b| (f64::from(b) - mean).powi(2)).sum::<f64>() / gray.len() as f64).sqrt();
    println!("frame: {} (mean {mean:.0}, spread {spread:.0})", frame.display());
    assert!(spread > 12.0, "the clip's picture is flat (mean {mean:.0}, spread {spread:.0}): not the game");
    std::fs::copy(&png, evidence.join("clip-thumbnail.png")).unwrap();
    // The buffer's segments went with the session.
    let runtime = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("bastion-mc");
    let leftover = std::fs::read_dir(&runtime)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("clips-"))
                .flat_map(|e| std::fs::read_dir(e.path()).map(|d| d.flatten().count()).ok())
                .sum::<usize>()
        })
        .unwrap_or(0);
    assert_eq!(leftover, 0, "segments were left in the buffer");
    drop(standin);
    let _ = std::fs::remove_dir_all(&dir);
}
