//! The phone's browser in its real sandbox: Firefox, the stage, the relay and the driver
//! exactly as a launch prepares them (`browser::prepare`, `Service::into_app`), driven
//! through the phone app's own interface.
//!
//! The sites are stand-ins served by this test over HTTPS, with certificates from a test
//! certificate authority that is put into the browser's profile with `certutil` (NSS's own
//! tool) before Firefox starts. Nothing here touches the internet unless
//! `BASTION_NET_TESTS=1` is set as well.
//!
//! Opt-in (it starts Firefox, a few seconds each): `BASTION_BROWSER_TESTS=1`. The window of
//! `browser login` is tested only when `BASTION_BROWSER_LOGIN_DISPLAY=:N` names a private X
//! display (tests/support/private-x11.sh) — never a desktop.

use std::{
    path::{Path, PathBuf},
    process::Command as Process,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bastion_mc::{
    browser::{self, config::SiteEntry},
    config::Config,
    paths::Paths,
    phone::{Command, Outbound, Outbox, PhoneApp, Service},
};
use tokio::sync::mpsc;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

fn enabled() -> bool {
    if std::env::var_os("BASTION_BROWSER_TESTS").is_none() {
        eprintln!("skipped: set BASTION_BROWSER_TESTS=1 (starts the installed Firefox)");
        return false;
    }
    true
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("browser-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn paths(root: &Path) -> Paths {
    Paths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        state: root.join("state"),
        runtime: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    }
}

#[path = "support/browser_sites.rs"]
mod sites;
use sites::{NAMES, Seen, TestCa, stand_in};

// ------------------------------------------------------------------ the browser, as a launch has it

struct Phone {
    app: Box<dyn PhoneApp>,
    out: Outbox,
    rx: mpsc::Receiver<Outbound>,
    state: serde_json::Value,
    notices: Vec<String>,
    frames: Vec<(u16, u16, usize, Instant)>,
    last_frame: Vec<u8>,
    log: Arc<Mutex<Vec<String>>>,
    stopper: Option<browser::Stopper>,
    seen: Seen,
    _root: PathBuf,
}

fn site(name: &str, start: &str, requests_per_minute: Option<u32>) -> SiteEntry {
    SiteEntry {
        name: Some(name.into()),
        title: Some("Chat".into()),
        start: Some(start.into()),
        hosts: Some(vec!["chat.example".into(), "*.chat.example".into()]),
        requests_per_minute,
        ..Default::default()
    }
}

impl Phone {
    async fn start(name: &str, sites: Vec<SiteEntry>, frame_rate: u32) -> Phone {
        let root = scratch(name);
        let paths = paths(&root);
        let ca = TestCa::new();
        ca.install(&paths.data.join("browser/profile"), &root);
        let (addr, seen) = stand_in(&ca).await;
        let mut cfg = Config::default();
        cfg.browser.enabled = true;
        cfg.browser.sites = sites;
        cfg.browser.frame_rate = frame_rate;
        cfg.browser.test_upstreams = NAMES.iter().map(|n| (n.to_string(), addr)).collect();
        cfg.validate().unwrap();
        Phone::with(root, paths, cfg, seen).await
    }

    async fn with(root: PathBuf, paths: Paths, cfg: Config, seen: Seen) -> Phone {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let logger: bastion_mc::egress::Logger = Arc::new(move |line: String| {
            eprintln!("[log] {line}");
            sink.lock().unwrap().push(line)
        });
        let mut service = browser::prepare(&paths, &cfg, "test", Path::new(LAUNCHER), false, logger).expect("enabled");
        assert!(service.is_running(), "{}", service.describe());
        let stopper = service.stopper();
        let mut keep = bastion_mc::confine::Confinement::default();
        service.confinement_needs(&mut keep);
        let mut app = Box::new(service).into_app();
        assert_eq!((app.name(), app.limits().len()), ("browser", 11));
        let (out, rx) = Outbox::new(512);
        app.hello(&out);
        Phone {
            app,
            out,
            rx,
            state: serde_json::Value::Null,
            notices: Vec::new(),
            frames: Vec::new(),
            last_frame: Vec::new(),
            log,
            stopper,
            seen,
            _root: root,
        }
    }

    fn send(&mut self, line: &str) {
        let command = Command::parse(line.as_bytes()).unwrap_or_else(|e| panic!("{line}: {e:?}"));
        self.app.command(command, &self.out);
    }

    fn take(&mut self, message: Outbound) {
        match message {
            Outbound::State { json, .. } => self.state = serde_json::from_str(&json).unwrap(),
            Outbound::Notice { text, .. } => {
                eprintln!("[notice] {text}");
                self.notices.push(text)
            }
            Outbound::Frame { width, height, bytes, format, .. } => {
                assert_eq!(format, bastion_mc::phone::FrameFormat::Jpeg);
                assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
                self.frames.push((width, height, bytes.len(), Instant::now()));
                self.last_frame = bytes;
            }
        }
    }

    /// Take messages until `done` holds (or panic after `patience`).
    async fn until(&mut self, what: &str, patience: Duration, done: impl Fn(&Phone) -> bool) {
        let deadline = Instant::now() + patience;
        while !done(self) {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(message)) => self.take(message),
                _ => panic!(
                    "{what}: not within {patience:?}\nstate: {}\nnotices: {:?}\nframes: {}\nlog:\n{}",
                    self.state,
                    self.notices,
                    self.frames.len(),
                    self.log.lock().unwrap().join("\n")
                ),
            }
        }
    }

    /// Take whatever arrives for `time`.
    async fn drain(&mut self, time: Duration) {
        let deadline = Instant::now() + time;
        while let Ok(Some(message)) =
            tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), self.rx.recv()).await
        {
            self.take(message);
        }
    }

    fn title(&self) -> &str {
        self.state["title"].as_str().unwrap_or_default()
    }

    async fn ready(&mut self) {
        self.until("the browser is ready", Duration::from_secs(90), |p| p.state["ready"] == true).await;
    }

    async fn stop(mut self) {
        if let Some(stopper) = self.stopper.take() {
            stopper.stop().await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stand_in_site_opens_renders_and_takes_input() {
    if !enabled() {
        return;
    }
    let sites = vec![site("chat", "https://chat.example/", None), site("wd", "https://chat.example/webdriver", None)];
    let mut phone = Phone::start("input", sites, 10).await;
    phone.ready().await;
    assert_eq!(phone.state["sites"][0]["name"], "chat");
    assert_eq!(phone.state["open"], serde_json::Value::Null);
    phone.send("browser open chat");
    phone.until("the page's title", Duration::from_secs(30), |p| p.title() == "Chat stand-in").await;
    assert_eq!(phone.state["host"], "chat.example");
    assert_eq!(phone.state["open"], "chat");

    phone.send("browser frames on");
    phone.until("a frame", Duration::from_secs(20), |p| !p.frames.is_empty()).await;
    assert_eq!((phone.frames[0].0, phone.frames[0].1), (800, 600), "the page at the configured size");

    // A tap into the input, then typing reaches it (its handler writes the text into the title).
    phone.send("browser mouse down 100 20");
    phone.send("browser mouse up 100 20");
    phone.send("browser text -- héllo ☕");
    phone.until("typed text", Duration::from_secs(20), |p| p.title() == "typed:héllo ☕").await;
    // A click lands where the phone says.
    phone.send("browser mouse move 400 350");
    phone.send("browser mouse down 400 350");
    phone.send("browser mouse up 400 350");
    phone.until("the click", Duration::from_secs(20), |p| p.title() == "clicked").await;
    // Enter submits the form; the stand-in sees what was typed.
    phone.send("browser mouse down 100 20");
    phone.send("browser mouse up 100 20");
    phone.send("browser key ctrl-a");
    phone.send("browser text -- ok");
    phone.send("browser key Enter");
    phone.until("the form was sent", Duration::from_secs(20), |p| p.title() == "sent:msg=ok").await;
    assert!(phone.seen.paths_of("chat.example").iter().any(|p| p == "/sent?msg=ok"));
    phone.send("browser back");
    phone
        .until("back", Duration::from_secs(20), |p| p.title().starts_with("typed:") || p.title() == "Chat stand-in")
        .await;

    // Another size: the next frame has it.
    let before = phone.frames.len();
    phone.send("browser size 400 700");
    phone
        .until("a frame of the new size", Duration::from_secs(20), |p| {
            p.frames[before..].iter().any(|f| (f.0, f.1) == (400, 700))
        })
        .await;

    // Pages get the dark colour scheme. They can tell that the browser is remote-controlled:
    // Firefox 159 sets `navigator.webdriver` while its remote agent runs, whatever the
    // preferences say (tried: `dom.webdriver.enabled`, `remote.prefs.recommended`).
    phone.send("browser open wd");
    phone.until("the page's own report", Duration::from_secs(20), |p| p.title().starts_with("webdriver:")).await;
    assert_eq!(phone.title(), "webdriver:true dark:true");

    // Closing: nothing is open, and no host is shown.
    phone.send("browser close");
    phone.until("closed", Duration::from_secs(10), |p| p.state["open"].is_null()).await;
    assert!(phone.state["host"].is_null() && phone.title().is_empty());
    phone.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn other_hosts_are_refused_and_never_reached() {
    if !enabled() {
        return;
    }
    let sites =
        vec![site("chat", "https://chat.example/link", None), site("tracker", "https://chat.example/tracker", None)];
    let mut phone = Phone::start("refuse", sites, 10).await;
    phone.ready().await;
    phone.send("browser open chat");
    phone.until("the link page", Duration::from_secs(30), |p| p.title() == "Link stand-in").await;
    // The whole page is a link to a host the site does not list: a tap follows it.
    phone.send("browser mouse down 200 200");
    phone.send("browser mouse up 200 200");
    phone
        .until("the refusal on the phone", Duration::from_secs(20), |p| {
            p.notices.iter().any(|n| n == "refused: evil.example is not part of Chat")
        })
        .await;
    phone.drain(Duration::from_millis(1500)).await;
    assert_eq!(phone.title(), "Link stand-in", "the page stays where it was");
    assert_eq!(phone.state["host"], "chat.example");

    // What a page loads from other hosts is refused too (without a word on the phone);
    // what it loads from a listed host below a wildcard is not.
    phone.send("browser open tracker");
    phone.until("the tracker page", Duration::from_secs(30), |p| p.title() == "Tracker stand-in").await;
    phone
        .until("the allowed image", Duration::from_secs(20), |p| {
            p.seen.paths_of("cdn.chat.example").contains(&"/ok.png".to_string())
        })
        .await;
    phone.drain(Duration::from_secs(1)).await;
    assert!(phone.state["refused"].as_u64().unwrap() >= 2, "{}", phone.state);
    let hosts = phone.seen.hosts();
    assert!(
        !hosts.iter().any(|h| h == "evil.example" || h == "tracker.example"),
        "the stand-in was reached for {hosts:?}"
    );
    let log = phone.log.lock().unwrap().join("\n");
    assert!(log.contains("refused navigation to evil.example (OtherHost, site chat)"), "{log}");
    assert!(log.contains("refused request to tracker.example (OtherHost, site tracker)"), "{log}");
    assert!(!log.contains("ALLOW evil.example") && !log.contains("ALLOW tracker.example"), "{log}");
    phone.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sites_budgets_refuse_what_is_beyond_them() {
    if !enabled() {
        return;
    }
    // Six requests a minute: the page and five of its twelve images.
    let sites = vec![
        site("chat", "https://chat.example/many", Some(6)),
        SiteEntry { api_calls_per_minute: Some(5), ..site("calls", "https://chat.example/calls", None) },
    ];
    let mut phone = Phone::start("budget", sites, 10).await;
    phone.ready().await;
    phone.send("browser open chat");
    phone
        .until("the request limit is said", Duration::from_secs(30), |p| {
            p.notices.iter().any(|n| n.contains("the request limit (6 a minute) is reached"))
        })
        .await;
    phone.drain(Duration::from_secs(2)).await;
    let reached = phone.seen.paths_of("chat.example");
    assert!(!reached.is_empty() && reached.len() <= 6, "the stand-in saw {reached:?}");
    assert!(phone.state["refused"].as_u64().unwrap() >= 6, "{}", phone.state);

    // Five API calls a minute: the page loads, five of its twelve calls go out.
    phone.send("browser open calls");
    phone
        .until("the API call limit is said", Duration::from_secs(30), |p| {
            p.notices.iter().any(|n| n.contains("the API call limit (5 a minute) is reached"))
        })
        .await;
    phone.drain(Duration::from_secs(2)).await;
    let calls: Vec<String> =
        phone.seen.paths_of("chat.example").into_iter().filter(|p| p.starts_with("/api/")).collect();
    assert_eq!(calls.len(), 5, "API calls that reached the stand-in: {calls:?}");
    assert!(phone.seen.paths_of("chat.example").contains(&"/calls".to_string()), "the page itself is not an API call");
    phone.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frames_follow_changes_and_never_exceed_the_rate() {
    if !enabled() {
        return;
    }
    let sites =
        vec![site("anim", "https://chat.example/anim", None), site("static", "https://chat.example/static", None)];
    let mut phone = Phone::start("frames", sites, 15).await;
    phone.ready().await;
    phone.send("browser open anim");
    phone.until("the animated page", Duration::from_secs(30), |p| p.title() == "Anim stand-in").await;
    assert!(phone.frames.is_empty(), "no frames before the phone asks for them");
    phone.send("browser frames on");
    phone.until("a first frame", Duration::from_secs(20), |p| !p.frames.is_empty()).await;
    let start = phone.frames.len();
    let window = Duration::from_secs(4);
    phone.drain(window).await;
    let frames = &phone.frames[start..];
    let rate = frames.len() as f64 / window.as_secs_f64();
    println!("animated page: {} frames in {window:?} ({rate:.1} per second)", frames.len());
    assert!(frames.len() >= 8, "a page that keeps changing keeps sending: {} frames", frames.len());
    assert!(rate <= 15.5, "{rate} frames per second");
    let shortest = frames.windows(2).map(|w| w[1].3.duration_since(w[0].3)).min().unwrap();
    assert!(shortest >= Duration::from_millis(50), "two frames {shortest:?} apart");

    // A page that does not change: its first frame, then (almost) nothing.
    phone.send("browser open static");
    phone.until("the static page", Duration::from_secs(30), |p| p.title() == "Static stand-in").await;
    phone.drain(Duration::from_secs(2)).await;
    let settled = phone.frames.len();
    phone.drain(Duration::from_secs(4)).await;
    println!("static page: {} frame(s) in 4 s after it settled", phone.frames.len() - settled);
    assert!(phone.frames.len() - settled <= 1, "an unchanged page was sent again");
    // A phone that connects again gets a picture although nothing changed.
    let before = phone.frames.len();
    let out = phone.out.clone();
    phone.app.hello(&out);
    phone.until("a picture for the phone that came back", Duration::from_secs(10), |p| p.frames.len() > before).await;

    // Off means off.
    phone.send("browser frames off");
    phone.send("browser open anim");
    phone.drain(Duration::from_secs(1)).await;
    let off = phone.frames.len();
    phone.drain(Duration::from_secs(3)).await;
    assert_eq!(phone.frames.len(), off, "frames while the phone did not ask for them");
    phone.stop().await;
}

/// `browser login` on a private display: the window appears, the site sets its session in
/// the profile, Ctrl+Q closes the window the way a person would, and the in-game browser
/// then starts signed in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_in_happens_in_a_window_and_the_game_browser_inherits_the_session() {
    if !enabled() {
        return;
    }
    let Some(display) = std::env::var("BASTION_BROWSER_LOGIN_DISPLAY").ok() else {
        eprintln!("skipped: set BASTION_BROWSER_LOGIN_DISPLAY=:N, a PRIVATE X display (tests/support/private-x11.sh)");
        return;
    };
    let number: u32 = display.trim_start_matches(':').parse().expect("a display like :83");
    let socket = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
    assert!(socket.exists(), "{} does not exist", socket.display());
    let root = scratch("login");
    let paths = paths(&root);
    let ca = TestCa::new();
    ca.install(&paths.data.join("browser/profile"), &root);
    let (addr, seen) = stand_in(&ca).await;
    let mut cfg = Config::default();
    cfg.browser.enabled = true;
    cfg.browser.sites = vec![site("chat", "https://chat.example/login", None)];
    cfg.browser.test_upstreams = NAMES.iter().map(|n| (n.to_string(), addr)).collect();
    cfg.validate().unwrap();

    let reporter = bastion_mc::events::Reporter::capture();
    let desktop = browser::sandbox::Desktop::X11 { number, socket, cookie: None };
    let login = {
        let (paths, cfg, reporter) = (paths.clone(), cfg.clone(), reporter.clone());
        tokio::spawn(async move {
            browser::login::run_on(&paths, &cfg, "chat", Path::new(LAUNCHER), &reporter, desktop).await
        })
    };
    // The page sets the session; the window is shown (a picture of the display is kept).
    let deadline = Instant::now() + Duration::from_secs(60);
    while !seen.paths_of("chat.example").iter().any(|p| p == "/login") {
        assert!(Instant::now() < deadline, "the login page was never requested");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    let shot = root.join("login-window.png");
    let grab = Process::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/x11-shot.py"))
        .arg(&shot)
        .env("DISPLAY", &display)
        .env("XAUTHORITY", "/dev/null")
        .output()
        .expect("x11-shot.py");
    assert!(grab.status.success(), "capturing the private display: {}", String::from_utf8_lossy(&grab.stderr));
    let described = String::from_utf8_lossy(&grab.stdout).to_string();
    println!("the login window as it was shown: {} ({})", shot.display(), described.trim());
    let colours: usize =
        described.split(", ").nth(1).and_then(|c| c.split(' ').next()).and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(colours > 20, "the display shows next to nothing: {described}");
    let pressed = Process::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/x11-chord.py"))
        .args(["ffe3", "71"]) // Control_L + q
        .env("DISPLAY", &display)
        .env("XAUTHORITY", "/dev/null")
        .output()
        .expect("x11-chord.py");
    assert!(pressed.status.success(), "{}", String::from_utf8_lossy(&pressed.stderr));
    let code = tokio::time::timeout(Duration::from_secs(30), login).await.expect("the window closed").unwrap().unwrap();
    assert_eq!(code, 0, "closed normally: {:?}", reporter.captured());
    assert!(browser::read_logins(&paths).contains_key("chat"));

    // The in-game browser, same profile: the stand-in sees the session cookie.
    cfg.browser.sites = vec![site("chat", "https://chat.example/", None)];
    let mut phone = Phone::with(root, paths, cfg, seen).await;
    phone.ready().await;
    assert_eq!(phone.state["sites"][0]["signed_in_hint"], true);
    phone.send("browser open chat");
    phone.until("signed in", Duration::from_secs(30), |p| p.title() == "signed in as tester").await;
    assert_eq!(phone.state["signed_in_hint"], true);
    assert!(phone.seen.cookies().iter().any(|c| c.contains("session=tester")));
    phone.stop().await;
}

/// The internet, without signing in anywhere: a public page and Discord's public login page
/// render inside the sandbox; what Discord's page asked for beyond the preset is printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_com_and_discords_login_page_render_in_the_sandbox() {
    if !enabled() {
        return;
    }
    if std::env::var_os("BASTION_NET_TESTS").is_none() {
        eprintln!("skipped: set BASTION_NET_TESTS=1 as well (loads example.com and discord.com/login)");
        return;
    }
    let root = scratch("net");
    let paths = paths(&root);
    let mut cfg = Config::default();
    cfg.browser.enabled = true;
    cfg.browser.sites = vec![
        SiteEntry {
            name: Some("example".into()),
            start: Some("https://example.com/".into()),
            hosts: Some(vec!["example.com".into()]),
            ..Default::default()
        },
        SiteEntry { preset: Some("discord".into()), ..Default::default() },
    ];
    cfg.browser.width = 1000;
    cfg.browser.height = 700;
    cfg.validate().unwrap();
    let mut phone = Phone::with(root.clone(), paths, cfg, Seen::default()).await;
    phone.ready().await;
    phone.send("browser frames on");
    phone.send("browser open example");
    phone
        .until("example.com", Duration::from_secs(60), |p| p.title() == "Example Domain" && !p.frames.is_empty())
        .await;

    phone.send("browser open discord");
    phone
        .until("Discord's page", Duration::from_secs(90), |p| {
            p.title().contains("Discord") && p.state["loading"] == false
        })
        .await;
    // Let the page settle (it loads its scripts, then draws); the newest frame is kept.
    phone.drain(Duration::from_secs(12)).await;
    let (width, height, bytes, _) = *phone.frames.last().unwrap();
    let picture = root.join("discord-login.jpg");
    std::fs::write(&picture, &phone.last_frame).unwrap();
    println!("the newest frame of Discord's login page: {}", picture.display());
    println!(
        "discord.com/login: frame {width}x{height}, {bytes} bytes; title {:?}; host {}",
        phone.title(),
        phone.state["host"]
    );
    assert_eq!((width, height), (1000, 700));
    assert!(bytes > 20_000, "a page with content, not a blank one ({bytes} bytes)");
    let log = phone.log.lock().unwrap().clone();
    let mut asked: Vec<&str> = log
        .iter()
        .filter(|l| {
            l.contains("refused request to") || l.contains("refused navigation to") || l.contains("DENY  name lookup")
        })
        .map(String::as_str)
        .collect();
    asked.sort_unstable();
    asked.dedup();
    println!("refused while showing Discord's login page:\n  {}", asked.join("\n  "));
    for line in log.iter().filter(|l| l.contains("ALLOW")) {
        println!("allowed: {line}");
    }
    phone.stop().await;
}
