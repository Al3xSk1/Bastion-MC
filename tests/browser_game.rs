//! The phone's browser in a real launch (Mojang's demo mode, no account): Firefox is started
//! in its own sandbox before the launcher confines itself, its relay and remote control
//! serve after that, the phone's app opens a site and gets a frame (the launch's built-in
//! self-test, `[browser] self_test`), and nothing of the browser's profile can be found from
//! inside the game's sandbox.
//!
//! Opt-in, it starts the game: `BASTION_E2E_ROOT` (a prepared data root with Minecraft
//! installed, `BASTION_E2E_INSTANCE`, default `vanilla-latest`), `BASTION_BROWSER_TESTS=1`,
//! and `DISPLAY` naming a PRIVATE display (tests/support/private-x11.sh).

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use bastion_mc::{
    browser::config::SiteEntry,
    config::Config,
    events::{Reporter, StopSignal},
    launch::{LaunchOptions, launch},
    paths::Paths,
    util,
};
use bastion_mc_proto::{Event, Source};

#[path = "support/browser_sites.rs"]
mod sites;

/// Every log line of the launch, in order, with its source.
fn lines(reporter: &Reporter) -> Vec<(Source, String)> {
    reporter
        .captured()
        .into_iter()
        .filter_map(|event| match event {
            Event::Log { source, line } => Some((source, line)),
            _ => None,
        })
        .collect()
}

fn position(lines: &[(Source, String)], needle: &str) -> Option<usize> {
    lines.iter().position(|(_, l)| l.contains(needle))
}

/// Processes below this one whose command name is `java`.
fn game_processes() -> Vec<i32> {
    let me = std::process::id() as i32;
    let parent_of = |pid: i32| -> Option<i32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        if std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|c| c.trim() == "java").unwrap_or(false) {
            let mut at = pid;
            for _ in 0..32 {
                match parent_of(at) {
                    Some(p) if p == me => {
                        found.push(pid);
                        break;
                    }
                    Some(p) if p > 1 => at = p,
                    _ => break,
                }
            }
        }
    }
    found
}

/// Looks into a process's root on request. The test process calls the launch itself and
/// is confined with it at step 9 (no program execution, no files beyond its own), so what
/// inspects the game's sandbox is a helper started before that: it reads
/// `<pid> <profile path> <marker>` lines and answers with one line of findings.
const INSPECTOR: &str = r#"
import os, sys
for line in sys.stdin:
    pid, profile, marker = line.split()
    root = f"/proc/{pid}/root"
    try:
        os.listdir(root); readable = True
    except OSError as e:
        readable = False
    there = os.path.exists(root + profile)
    walked, hits = 0, []
    for top, dirs, files in os.walk(root):
        rel = top[len(root):]
        # Kernel views, and the host's own read-only /usr (the profile is not under it).
        dirs[:] = [d for d in dirs if not (rel == "" and d in ("proc", "sys", "dev", "usr"))]
        for name in dirs + files:
            walked += 1
            if name in (marker, "cookies.sqlite", "user.js", "cert9.db", "key4.db"):
                hits.append(rel + "/" + name)
        if walked > 1000000:
            break
    print(f"readable={readable} profile={there} walked={walked} hits={','.join(hits) or '-'}", flush=True)
"#;

#[tokio::test(flavor = "multi_thread")]
async fn the_browser_starts_before_the_confinement_serves_after_it_and_stays_out_of_the_games_sight() {
    let Some(root) = std::env::var_os("BASTION_E2E_ROOT").map(PathBuf::from) else {
        eprintln!("skipped: set BASTION_E2E_ROOT (and BASTION_BROWSER_TESTS=1, DISPLAY=:<private>)");
        return;
    };
    if std::env::var_os("BASTION_BROWSER_TESTS").is_none() {
        eprintln!("skipped: set BASTION_BROWSER_TESTS=1 as well");
        return;
    }
    let instance = std::env::var("BASTION_E2E_INSTANCE").unwrap_or_else(|_| "vanilla-latest".into());
    let app = |kind: &str| root.join(kind).join("bastion-mc");
    let paths = Paths {
        config: app("config"),
        data: app("data"),
        cache: app("cache"),
        state: app("state"),
        runtime: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    };
    // A fresh profile with the test authority in it, and a marker to look for from the game.
    let profile = paths.data.join("browser/profile");
    let _ = std::fs::remove_dir_all(&profile);
    let ca = sites::TestCa::new();
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("browser-game-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    ca.install(&profile, &scratch);
    let marker = format!("bastion-browser-marker-{}", util::hex_encode(&util::random_bytes::<6>().unwrap()));
    std::fs::write(profile.join(&marker), b"if the game can read this, the profile is not private").unwrap();
    let (addr, seen) = sites::stand_in(&ca).await;

    let mut cfg = Config::load(&paths).expect("configuration");
    cfg.browser.enabled = true;
    cfg.browser.sites = vec![SiteEntry {
        name: Some("chat".into()),
        title: Some("Chat".into()),
        start: Some("https://chat.example/".into()),
        hosts: Some(vec!["chat.example".into(), "*.chat.example".into()]),
        ..Default::default()
    }];
    cfg.browser.test_upstreams = sites::NAMES.iter().map(|n| (n.to_string(), addr)).collect();
    cfg.browser.self_test = true;
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
    let mut inspector = std::process::Command::new("/usr/bin/python3")
        .args(["-I", "-c", INSPECTOR])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    let mut ask = inspector.stdin.take().unwrap();
    let mut answers = std::io::BufReader::new(inspector.stdout.take().unwrap());
    // Once the phone's app has had its first frame: look into the game's sandbox, then stop.
    let watcher = {
        let (reporter, stop, profile, marker) = (reporter.clone(), stop.clone(), profile.clone(), marker.clone());
        tokio::spawn(async move {
            let deadline = Instant::now() + Duration::from_secs(240);
            let mut looked = None;
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let all = lines(&reporter);
                if position(&all, "browser self-test: first frame").is_some()
                    && position(&all, "launcher: confined itself").is_some()
                {
                    let games = game_processes();
                    let mut report = Vec::new();
                    for pid in &games {
                        use std::io::{BufRead, Write};
                        writeln!(ask, "{pid} {} {marker}", profile.display()).unwrap();
                        ask.flush().unwrap();
                        let mut line = String::new();
                        answers.read_line(&mut line).unwrap();
                        report.push((*pid, line.trim().to_string()));
                    }
                    looked = Some(report);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    break;
                }
            }
            stop.trigger();
            looked
        })
    };
    let outcome = launch(&paths, &cfg, &opts, &reporter).await;
    let looked = watcher.await.unwrap();
    let all = lines(&reporter);
    let show = || all.iter().map(|(s, l)| format!("{s:?} | {l}")).collect::<Vec<_>>().join("\n");
    for (source, line) in all.iter().filter(|(_, l)| l.contains("browser") || l.contains("confined")) {
        println!("{source:?} | {line}");
    }
    println!("outcome: {outcome:?}");

    // Started before the launcher confined itself; answering only after.
    let started = position(&all, "browser: Firefox").unwrap_or_else(|| panic!("not started:\n{}", show()));
    let confined = position(&all, "launcher: confined itself").unwrap_or_else(|| panic!("not confined:\n{}", show()));
    let ready = position(&all, "browser: ready").unwrap_or_else(|| panic!("never ready:\n{}", show()));
    assert!(started < confined && confined < ready, "order: started {started}, confined {confined}, ready {ready}");
    let confinement = &all[confined].1;
    assert!(confinement.contains("443"), "port 443 is kept for the relay: {confinement}");
    // The phone's app opened the site through the relay and got a picture of it.
    assert!(position(&all, "browser relay: ALLOW chat.example (site chat, SNI verified)").is_some(), "{}", show());
    let frame =
        position(&all, "browser self-test: first frame 800x600").unwrap_or_else(|| panic!("no frame:\n{}", show()));
    assert!(frame > confined);
    assert!(seen.paths_of("chat.example").contains(&"/".to_string()));
    // Closed at the end, Firefox first.
    assert!(position(&all, "browser: closed").is_some(), "{}", show());

    // From inside the game's sandbox: no profile, no marker, none of Firefox's files.
    let _ = inspector.kill();
    let _ = inspector.wait();
    let looked = looked.expect("the game's sandbox was looked into");
    assert!(!looked.is_empty(), "no game process was found");
    for (pid, findings) in looked {
        println!("game process {pid}, seen from outside through /proc/{pid}/root: {findings}");
        let field = |name: &str| {
            findings.split(' ').find_map(|f| f.strip_prefix(&format!("{name}="))).unwrap_or("").to_string()
        };
        assert_eq!(field("readable"), "True", "the game's root could not be read; nothing was shown");
        let walked: usize = field("walked").parse().unwrap();
        assert!(walked > 100, "too little was seen to mean anything ({walked} paths)");
        assert!(walked < 1_000_000, "the walk stopped before it had seen everything");
        assert_eq!(field("profile"), "False", "the browser's profile is visible in the game's sandbox");
        assert_eq!(field("hits"), "-", "browser files visible in the game's sandbox");
    }
}
