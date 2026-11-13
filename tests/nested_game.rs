//! The real game through the whole launch path, with its display hardening on: inside a
//! nested compositor on an X11 session, or behind a Wayland security context.
//!
//! Opt-in, and alone in this file on purpose: a launch ends with the launcher confining its
//! own process (Landlock, every thread) — here, the test process.
//!
//! ```sh
//! BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=vanilla-latest \
//!     [BASTION_NESTED_LAYERS=…] cargo test --test nested_game -- --nocapture
//! ```
//!
//! `BASTION_E2E_ROOT` holds `config/`, `data/`, `cache/` and `state/` (the four XDG roots)
//! with the instance already installed. The session decides which path is taken: with
//! `WAYLAND_DISPLAY` set, the security context; otherwise X11 and the nested compositor.
//! A window opens for as long as the game takes to draw its first frames.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use bastion_mc::{
    config::{Config, IsolationMode},
    events::{Reporter, StopSignal},
    launch::{LaunchOptions, launch},
    paths::Paths,
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

#[tokio::test(flavor = "multi_thread")]
async fn the_real_client_draws_through_the_hardened_display_path() {
    let (Some(root), Some(instance)) =
        (std::env::var_os("BASTION_E2E_ROOT"), std::env::var("BASTION_E2E_INSTANCE").ok())
    else {
        eprintln!("skipped: set BASTION_E2E_ROOT and BASTION_E2E_INSTANCE");
        return;
    };
    let root = PathBuf::from(root);
    let app = |kind: &str| root.join(kind).join("bastion-mc");
    let paths = Paths {
        config: app("config"),
        data: app("data"),
        cache: app("cache"),
        state: app("state"),
        runtime: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    };
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let mut cfg = Config::load(&paths).expect("configuration");
    // Neither path needs the consent that handing over the session's X server needs.
    cfg.sandbox.allow_x11 = false;
    cfg.sandbox.wayland_security_context = IsolationMode::Require;
    cfg.sandbox.nested_compositor = IsolationMode::Require;
    cfg.sandbox.nested_compositor_args = ["-W", "854", "-H", "480"].map(String::from).to_vec();
    cfg.sandbox.nested_compositor_layers = std::env::var_os("BASTION_NESTED_LAYERS")
        .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
        .unwrap_or_default();
    cfg.validate().expect("valid configuration");

    let reporter = Reporter::capture();
    let stop = StopSignal::never();
    let opts = LaunchOptions {
        instance,
        account: "default".into(),
        demo: true,
        join: None,
        repair: false,
        stop: stop.clone(),
        program: env!("CARGO_BIN_EXE_bastion-mc").into(),
    };

    // Let the game get as far as building its textures, give it a few more seconds of
    // frames, then ask the launcher to stop it.
    let drew = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher = {
        let (reporter, stop, drew) = (reporter.clone(), stop.clone(), drew.clone());
        tokio::spawn(async move {
            let deadline = Instant::now() + Duration::from_secs(240);
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if log_lines(&reporter, Source::Game).iter().any(|l| l.contains("textures/atlas/")) {
                    drew.store(true, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(6)).await;
                    break;
                }
            }
            stop.trigger();
        })
    };
    let outcome = launch(&paths, &cfg, &opts, &reporter).await;
    watcher.abort();
    let drew = drew.load(std::sync::atomic::Ordering::SeqCst);

    let launcher = log_lines(&reporter, Source::Launcher);
    let game = log_lines(&reporter, Source::Game);
    let status: Vec<String> = reporter
        .captured()
        .into_iter()
        .filter_map(|e| match e {
            Event::Status { text } => Some(text),
            _ => None,
        })
        .collect();
    for line in status.iter().filter(|l| l.starts_with("display") || l.starts_with("limits") || l.contains("nested")) {
        println!("status   | {line}");
    }
    for line in launcher.iter().filter(|l| {
        l.starts_with("display") || l.starts_with("launcher:") || l.contains("compositor") || l.contains("summary")
    }) {
        println!("launcher | {}", &line[..line.len().min(220)]);
    }
    for line in game.iter().filter(|l| {
        ["Backend library", "graphics backend", "video driver", "Created window"].iter().any(|k| l.contains(k))
    }) {
        println!("game     | {}", line.trim());
    }
    let failure = || {
        format!(
            "outcome {outcome:?}\n--- launcher ---\n{}\n--- game (tail) ---\n{}",
            launcher.join("\n"),
            game.iter().rev().take(40).rev().cloned().collect::<Vec<_>>().join("\n")
        )
    };
    assert!(drew, "the game never got as far as its textures\n{}", failure());
    assert_eq!(outcome.as_ref().ok(), Some(&130), "stopped on request\n{}", failure());

    if wayland {
        assert!(status.iter().any(|l| l.contains("security context")), "{status:?}");
        assert!(
            game.iter().any(|l| l.contains("video driver: wayland")),
            "the game is a Wayland client\n{}",
            failure()
        );
    } else {
        assert!(status.iter().any(|l| l.contains("nested compositor (gamescope")), "{status:?}");
        assert!(
            status.iter().all(|l| !l.contains("WARNING: X11 lets the game")),
            "the game is not on the session's X server"
        );
        assert!(launcher.iter().any(|l| l.contains("inside the nested compositor")));
        assert!(launcher.iter().any(|l| l.contains("Starting Xwayland")), "compositor output reaches the launch log");
        assert!(game.iter().any(|l| l.contains("video driver: x11")), "{}", failure());
    }
    // Hardware rendering, not a software fallback.
    let backend = game.iter().find(|l| l.contains("graphics backend")).cloned().unwrap_or_default();
    assert!(!backend.is_empty() && !backend.contains("llvmpipe"), "{backend}");
    // Nothing of the session is left behind in the cache.
    let leftovers: Vec<_> = std::fs::read_dir(&paths.cache)
        .map(|d| d.flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().starts_with("display-")).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
