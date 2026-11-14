//! The phone's web browser: the installed Firefox, headless, in a sandbox of its own, able to
//! show only the sites listed in `[browser] sites`, each within its budgets.
//!
//! ```text
//!   phone (in the game) ──companion channel──▶ dispatcher ──▶ BrowserApp ──▶ driver
//!                                                                             │ WebDriver BiDi
//!   launcher ◀─────────── bridge (a socket pair; the stage copies bytes) ─────┘
//!      │                                                    ┌──────────────────────────────┐
//!      │ relay: DNS for the open site's hosts,             │ browser sandbox (bubblewrap) │
//!      │ HTTPS by SNI, budgets       ◀── 127.0.0.1:53/443 ──│  stage ─▶ Firefox --headless │
//!      ▼                                                    │  /profile = <data>/browser/… │
//!   the site                                                └──────────────────────────────┘
//! ```
//!
//! **When.** Everything is started before the launcher confines itself (step 9 of a launch):
//! the namespace helper, bubblewrap, Firefox. Afterwards the launcher only talks to it: the
//! relay serves the two sockets the helper created inside the browser's network namespace,
//! and the driver speaks BiDi over the bridge, a socket pair that existed before step 9.
//! The confined launcher keeps nothing for the browser but TCP port 443 (or the SOCKS5
//! proxy's port), to reach the sites.
//!
//! **What the game can do through it** (the phone is code in the game): show the configured
//! sites, see what they show (frames), click, scroll and type in them — in a signed-in page
//! that means acting as you on that site, within the site's budgets. It cannot name an
//! address (only a configured site, by name), reach any host the open site does not list,
//! read the profile, cookies or tokens (no command returns anything but frames, the host,
//! the page title and notices), open developer tools, download into anything it can see,
//! or upload a file (the sandbox has none). A site that can be made to display its own
//! secrets — an XSS in it, a page that runs code you type — is outside what the launcher
//! can stop.
//!
//! **Signing in** happens outside the game: `bastion-mc browser login <site>` shows the same
//! sandbox and profile as a window on your desktop, so a password is never typed through
//! the game (see `login`).

pub mod app;
pub mod bidi;
pub mod budget;
pub mod config;
pub mod driver;
pub mod login;
pub mod relay;
pub mod sandbox;
pub mod stage;
pub mod ws;

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{mpsc, oneshot, watch},
};

pub use config::BrowserConfig;

use self::{
    app::BrowserApp,
    config::Site,
    driver::{DriverMsg, Setup},
    relay::{SiteState, Upstream},
    sandbox::{Mode, Plan, Started},
};
use crate::{
    config::Config,
    confine::Confinement,
    egress::Logger,
    paths::Paths,
    phone::{Command, Outbound, Outbox, PhoneApp, Service},
    sandbox::cgroup,
    util,
};

/// Lines of Firefox's own output that go into the launch log, at most.
const MAX_OUTPUT_LINES: u64 = 400;
const LOGINS_FILE: &str = "logins.json";

/// When `browser login <site>` was last completed, per site name (Unix time).
pub fn read_logins(paths: &Paths) -> BTreeMap<String, u64> {
    util::read_small_file(&sandbox::browser_dir(paths).join(LOGINS_FILE), 64 * 1024)
        .ok()
        .and_then(|raw| serde_json::from_slice::<BTreeMap<String, u64>>(&raw).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, _)| name.len() <= 16)
        .take(64)
        .collect()
}

/// Record that `browser login <site>` was completed now.
pub fn record_login(paths: &Paths, site: &str) -> crate::Result<()> {
    let mut logins = read_logins(paths);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    logins.insert(site.to_string(), now);
    let json = serde_json::to_vec_pretty(&logins).map_err(|e| crate::Error::Other(format!("logins: {e}")))?;
    util::write_atomic(&sandbox::browser_dir(paths).join(LOGINS_FILE), &json, 0o600)
}

/// The browser as a launch prepares it.
pub struct BrowserService {
    sites: Vec<Site>,
    state: Result<Box<Prepared>, String>,
}

struct Prepared {
    started: Started,
    /// What was read back about the browser's resource limits (`None`: none were asked for).
    limits_note: Option<String>,
    profile: std::path::PathBuf,
    signed_in: Vec<bool>,
    upstream: Upstream,
    cfg: BrowserConfig,
    log: Logger,
    tx: mpsc::Sender<DriverMsg>,
    rx: mpsc::Receiver<DriverMsg>,
    kill_tx: oneshot::Sender<()>,
    kill_rx: oneshot::Receiver<()>,
    done_tx: oneshot::Sender<()>,
    done_rx: Option<oneshot::Receiver<()>>,
}

/// Ends the browser at the end of a launch: Firefox is asked to close (and writes its
/// profile), then the sandbox is made to end.
pub struct Stopper {
    driver: mpsc::Sender<DriverMsg>,
    kill: Option<oneshot::Sender<()>>,
    done: Option<oneshot::Receiver<()>>,
}

impl Stopper {
    pub async fn stop(mut self) {
        let (ack, acked) = oneshot::channel();
        if self.driver.try_send(DriverMsg::Stop(ack)).is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(4), acked).await;
        }
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
        if let Some(done) = self.done.take() {
            let _ = tokio::time::timeout(Duration::from_secs(8), done).await;
        }
    }
}

/// Start the browser for a launch, if `[browser]` asks for it. `None`: not enabled.
/// `scope`: the launch runs its sandboxes in systemd scopes (the browser gets one of its
/// own, `[browser] memory_max_mb`).
pub fn prepare(
    paths: &Paths,
    cfg: &Config,
    instance: &str,
    program: &Path,
    scope: bool,
    log: Logger,
) -> Option<BrowserService> {
    if !cfg.browser.enabled {
        return None;
    }
    let sites = match cfg.browser.resolved_sites() {
        Ok(sites) => sites,
        Err(e) => return Some(BrowserService { sites: Vec::new(), state: Err(e.to_string()) }),
    };
    if sites.is_empty() {
        return Some(BrowserService {
            sites,
            state: Err("[browser] lists no sites: there is nothing it could show".into()),
        });
    }
    let state = start(paths, cfg, instance, program, scope, log, &sites).map(Box::new).map_err(|e| e.to_string());
    Some(BrowserService { sites, state })
}

fn start(
    paths: &Paths,
    cfg: &Config,
    instance: &str,
    program: &Path,
    scope: bool,
    log: Logger,
    sites: &[Site],
) -> crate::Result<Prepared> {
    let limits = if scope {
        Some(cgroup::Limits::new(&format!("{instance}-browser"), u64::from(cfg.browser.memory_max_mb), 1024)?)
    } else {
        None
    };
    let started = sandbox::start(Plan {
        paths,
        mode: Mode::Headless,
        start_url: None,
        expose_timezone: cfg.sandbox.expose_timezone,
        limits: limits.clone(),
        program,
    })?;
    // Like the game's: a scope that was asked for is not yet a limit in force. Read back.
    let limits_note = limits.map(|limits| {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let verdict = match started.child.id() {
                Some(pid) => cgroup::verify(pid, &limits).map_err(|e| e.to_string()),
                None => Err("its process could not be identified".to_string()),
            };
            match verdict {
                Ok(()) => {
                    break format!(
                        "its own limits are in force ({} MiB of memory, no swap, {} tasks; read back from the kernel)",
                        limits.memory_max_bytes >> 20,
                        limits.tasks_max
                    );
                }
                Err(_) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Err(why) => break format!("WARNING: its limits are NOT in force — {why}"),
            }
        }
    });
    let logins = read_logins(paths);
    let upstream = if !cfg.browser.test_upstreams.is_empty() {
        Upstream::Fixed(cfg.browser.test_upstreams.clone())
    } else {
        match cfg.network.launcher_proxy.as_deref().map(crate::config::parse_socks5).transpose()? {
            Some(proxy) => Upstream::Socks5(proxy),
            None => Upstream::Direct,
        }
    };
    let (tx, rx) = mpsc::channel(64);
    let (kill_tx, kill_rx) = oneshot::channel();
    let (done_tx, done_rx) = oneshot::channel();
    Ok(Prepared {
        limits_note,
        profile: sandbox::profile_dir(paths),
        signed_in: sites.iter().map(|s| logins.contains_key(&s.name)).collect(),
        started,
        upstream,
        cfg: cfg.browser.clone(),
        log,
        tx,
        rx,
        kill_tx,
        kill_rx,
        done_tx,
        done_rx: Some(done_rx),
    })
}

impl BrowserService {
    /// Whether the browser is running (otherwise the phone is told why not).
    pub fn is_running(&self) -> bool {
        self.state.is_ok()
    }

    /// Something to end the browser with at the end of the launch. Take it before
    /// [`Service::into_app`]; dropping the service (or the launch) also ends the browser,
    /// only less politely.
    pub fn stopper(&mut self) -> Option<Stopper> {
        let prepared = self.state.as_mut().ok()?;
        let kill = std::mem::replace(&mut prepared.kill_tx, oneshot::channel().0);
        Some(Stopper { driver: prepared.tx.clone(), kill: Some(kill), done: prepared.done_rx.take() })
    }
}

impl Service for BrowserService {
    fn name(&self) -> &'static str {
        driver::APP
    }

    fn describe(&self) -> String {
        match &self.state {
            Ok(prepared) => format!(
                "browser: Firefox {} started in a sandbox of its own (profile {}, no home directory, no display, nothing \
                 of the game's sandbox; network only through the launcher's relay, HTTPS to the open site's hosts); {}. \
                 Sites: {}",
                prepared.started.version,
                prepared.profile.display(),
                prepared.limits_note.as_deref().unwrap_or("no resource limits of its own (sandbox.cgroup)"),
                self.sites
                    .iter()
                    .map(|s| format!("{} ({} hosts)", s.name, s.hosts.len()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(why) => format!("browser: NOT started — {why}"),
        }
    }

    fn confinement_needs(&self, keep: &mut Confinement) {
        if let Ok(prepared) = &self.state {
            match &prepared.upstream {
                Upstream::Direct => keep.tcp_connect.push(443),
                Upstream::Socks5(proxy) => keep.tcp_connect.push(proxy.port()),
                Upstream::Fixed(table) => keep.tcp_connect.extend(table.iter().map(|(_, a)| a.port())),
            }
        }
    }

    fn into_app(self: Box<Self>) -> Box<dyn PhoneApp> {
        let sites = self.sites;
        let prepared = match self.state {
            Ok(prepared) => *prepared,
            Err(why) => {
                // An app that says why there is no browser.
                let (tx, rx) = mpsc::channel(8);
                tokio::spawn(dead_driver(rx, sites.clone(), why));
                return Box::new(BrowserApp::new(tx, &sites));
            }
        };
        let Prepared { mut started, signed_in, upstream, cfg, log, tx, rx, kill_rx, done_tx, .. } = prepared;
        let (open_tx, open_rx) = watch::channel(None);
        let states: Vec<SiteState> = sites.iter().cloned().map(SiteState::new).collect();
        let relay = match relay::start(states, open_rx, started.dns, started.listener, upstream, log.clone()) {
            Ok(relay) => Some(relay),
            Err(e) => {
                log(format!("browser: the relay could not start: {e}"));
                None
            }
        };
        // Firefox's own output, bounded, into the launch log.
        let lines = Arc::new(std::sync::atomic::AtomicU64::new(0));
        if let Some(out) = started.child.stdout.take() {
            tokio::spawn(log_output(out, log.clone(), lines.clone()));
        }
        if let Some(err) = started.child.stderr.take() {
            tokio::spawn(log_output(err, log.clone(), lines));
        }
        // The sandbox: when it ends, the driver hears of it; at the end of the launch it is
        // made to end.
        let mut child = started.child;
        let lock = started.lock;
        let exited_tx = tx.clone();
        let supervisor_log = log.clone();
        tokio::spawn(async move {
            let _lock = lock;
            tokio::select! {
                status = child.wait() => {
                    let what = status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
                    supervisor_log(format!("browser: its sandbox ended ({what})"));
                    let _ = exited_tx.send(DriverMsg::Exited(what)).await;
                }
                Ok(()) = kill_rx => {
                    if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
                        let _ = child.kill().await;
                    }
                }
            }
            if let Some(relay) = relay {
                for line in relay.summary() {
                    supervisor_log(line);
                }
                relay.shutdown();
            }
            let _ = done_tx.send(());
        });
        let bridge = match started.bridge {
            Some(bridge) => bridge,
            None => {
                let (tx, rx) = mpsc::channel(8);
                tokio::spawn(dead_driver(rx, sites.clone(), "the browser has no remote control".into()));
                return Box::new(BrowserApp::new(tx, &sites));
            }
        };
        let setup = Setup {
            sites: sites.clone(),
            signed_in,
            open_tx,
            bridge,
            frame_rate: cfg.frame_rate,
            quality: cfg.quality,
            width: cfg.width,
            height: cfg.height,
            log: log.clone(),
        };
        tokio::spawn(driver::run(setup, rx));
        let mut app = BrowserApp::new(tx, &sites);
        if cfg.self_test {
            self_test(&mut app, &sites, log);
        }
        Box::new(app)
    }
}

/// What the phone hears when the browser could not be started: why.
async fn dead_driver(mut rx: mpsc::Receiver<DriverMsg>, sites: Vec<Site>, why: String) {
    let state = |out: &Outbox| {
        out.state(
            driver::APP,
            &serde_json::json!({
                "sites": sites.iter().map(|s| serde_json::json!({"name": s.name, "title": s.title})).collect::<Vec<_>>(),
                "open": null, "host": null, "loading": false, "title": "", "ready": false,
                "error": format!("the browser is not available: {why}"), "signed_in_hint": false,
            }),
        )
    };
    while let Some(msg) = rx.recv().await {
        match msg {
            DriverMsg::Hello(out) | DriverMsg::Command(_, out) => {
                state(&out);
            }
            DriverMsg::Stop(ack) => {
                let _ = ack.send(());
                return;
            }
            DriverMsg::Exited(_) => {}
        }
    }
}

async fn log_output(stream: impl tokio::io::AsyncRead + Unpin, log: Logger, count: Arc<std::sync::atomic::AtomicU64>) {
    let mut lines = BufReader::new(stream).split(b'\n');
    while let Ok(Some(line)) = lines.next_segment().await {
        let n = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < MAX_OUTPUT_LINES {
            log(format!("browser output: {}", util::sanitize_line(&line, 300)));
        } else if n == MAX_OUTPUT_LINES {
            log("browser output: further lines are not logged".into());
        }
    }
}

/// `[browser] self_test` (tests only): open the first site with frames on, as a phone would,
/// and log what the phone would have been sent.
fn self_test(app: &mut BrowserApp, sites: &[Site], log: Logger) {
    let (out, mut rx) = Outbox::new(256);
    app.hello(&out);
    for line in [format!("browser open {}", sites[0].name), "browser frames on".to_string()] {
        if let Ok(command) = Command::parse(line.as_bytes()) {
            app.command(command, &out);
        }
    }
    tokio::spawn(async move {
        let _out = out;
        let (mut frames, mut last_state) = (0u64, String::new());
        while let Some(message) = rx.recv().await {
            match message {
                Outbound::Frame { width, height, bytes, .. } => {
                    frames += 1;
                    if frames == 1 {
                        log(format!("browser self-test: first frame {width}x{height}, {} bytes of JPEG", bytes.len()));
                    }
                }
                Outbound::State { json, .. } if json != last_state => {
                    log(format!("browser self-test: state {}", util::sanitize_line(json.as_bytes(), 600)));
                    last_state = json;
                }
                Outbound::Notice { text, .. } => log(format!("browser self-test: notice {text}")),
                _ => {}
            }
        }
    });
}
