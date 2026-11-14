//! Launch pipeline. Nothing below runs out of order, and every step that can fail does so
//! before the JVM exists:
//!
//!  1. load the instance (signature, manifest validation, anti-rollback)
//!  2. make the game files present; verify digests
//!  3. curated only: fetch + verify loader libraries (SHA-256), verify `mods/` exactly
//!  4. pick a root-owned system JVM
//!  5. re-verify every file from the descriptor that will be bound into the sandbox
//!  6. decide network, display, audio, GPU exposure
//!  7. authenticate (as late as possible, so tokens live as briefly as possible)
//!  8. start the sandbox; write the launch frame to its stdin socket; wipe frame and tokens
//!  9. look at what was started instead of assuming it: read the resource limits back from
//!     the kernel; then the launcher confines itself — and only after that reports which
//!     protections this launch really has
//! 10. relay (filtered network) and log (sanitised) until the game exits. Nothing the game
//!     sends — a DNS question, a TLS hello, a line of output, or what it makes its display
//!     server print — is read before step 9 is done
//! 11. carry the allowlisted settings files back (parsed and re-encoded, never copied);
//!     delete the scratch game root, and say so only if that worked

use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::Write,
    net::Shutdown,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use crate::{
    BOOTSTRAP_CLASS, BOOTSTRAP_CLASS_NAME,
    auth::{self, store::TokenStore},
    companion,
    config::{CgroupMode, Config, ConfineMode, NetworkMode, ServerRule, parse_server},
    confine::{self, Confinement},
    egress,
    error::{Context, Error, Result},
    events::{Event, Phase, Protection, Reporter, Shield, Source, StopSignal},
    instance::{self, Instance, Kind, WRITABLE_DIRS},
    java,
    mojang::{
        args::{Piece, Vars, build_frame, render_plain, render_redacted},
        install::{Installed, Store, Stored, extract_natives},
        maven::Coordinate,
        meta::{Artifact, Features},
    },
    net::http::HttpClient,
    paths::{Paths, validate_file_name},
    persist,
    profile::{manifest::ProfileManifest, mods, packs},
    sandbox::{
        self, BaseOptions, GAME_DIR, Mount, Network, Resolv, SUPPORT_DIR, SandboxSpec, cgroup, display, nested,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
        trust::TrustStore,
    },
    secret::Secret,
    util::{self, Expected},
};

const MAX_LOG_LINE: usize = 4096;
/// The game controls how much it prints; the log must not be a way to fill the disk. Each
/// kind of line has a share of its own (256 MiB in all), because a single budget is the
/// game's to spend: one that prints until the file is full would keep everything said
/// afterwards — the relay's refusals, the broker's `[SECURITY]` lines, the tripwire report —
/// out of the file. What a share guarantees is that *other* kinds of line cannot use it up.
/// The relay's and the display server's shares are still driven by what the game does (a
/// flood of refused lookups fills the first); only the launcher's own share is out of its
/// reach, and that is where the summary of refusals and the tripwire report go.
const LOG_SHARES: [u64; 4] = [8 << 20, 24 << 20, 24 << 20, 200 << 20];
const SIGSYS_EXIT: i32 = 128 + libc::SIGSYS;
/// How long deleting the game root at exit may take. A game can leave a tree that takes
/// minutes to delete; what is left is removed by a later launch (and that is said).
const SCRATCH_REMOVAL_TIME: Duration = Duration::from_secs(30);
/// Most files and directories a burner session copies.
const BURNER_MAX_ENTRIES: u64 = 1_000_000;
/// Ceilings for the nested compositor's own scope. gamescope and Xwayland together use a
/// few hundred MiB and a few dozen threads; what a game renders lives in GPU memory.
const NESTED_MEMORY_MAX_MB: u64 = 2048;
const NESTED_TASKS_MAX: u32 = 1024;
/// gamescope composites on the GPU; two CPUs are ample, and a game that makes its display
/// server spin cannot take the rest of the machine with it.
const NESTED_CPU_QUOTA_PERCENT: u32 = 200;

pub struct LaunchOptions {
    pub instance: String,
    pub account: String,
    /// Start the client in Mojang's built-in demo mode without signing in (no token exists).
    pub demo: bool,
    /// A throw-away session for content you do not trust (a downloaded map, a resource
    /// pack, a replay): no network at all, no sign-in and no token, and the instance's
    /// worlds and settings are *copies* that are deleted afterwards — nothing the game
    /// writes survives. The full game is started only for an account that passed the
    /// ownership check at its last sign-in on this machine; otherwise Mojang's demo mode.
    pub burner: bool,
    /// `host[:port]` to connect to on start; also allowlisted for this session.
    pub join: Option<String>,
    pub repair: bool,
    /// Raised by whoever drives the launcher when the game should be shut down.
    pub stop: StopSignal,
    /// This program: it is re-executed as the sandbox's init stage and as the launcher's
    /// helpers. `/proc/self/exe`, unless the caller is some other executable (the tests).
    pub program: PathBuf,
}

pub fn http_client(cfg: &Config) -> Result<HttpClient> {
    let roots = crate::net::tls::load_system_roots()?;
    let pins = crate::net::tls::PinSet::from_config(&cfg.tls.pins, cfg.tls.builtin_pins)?;
    let proxy = cfg.network.launcher_proxy.as_deref().map(crate::config::parse_socks5).transpose()?;
    Ok(HttpClient::new(crate::net::tls::client_config(roots, pins)?, &cfg.downloads.extra_hosts).with_socks5(proxy))
}

pub struct Prepared {
    pub installed: Installed,
    /// Loader libraries of a curated profile, pinned by SHA-256 in the signed manifest.
    pub loader_libs: Vec<Stored>,
}

fn loader_artifacts(paths: &Paths, manifest: &ProfileManifest) -> Result<Vec<Stored>> {
    let Some(loader) = &manifest.loader else { return Ok(Vec::new()) };
    let root = paths.libraries();
    loader
        .libraries
        .iter()
        .map(|lib| {
            let coord = Coordinate::parse(&lib.name)?;
            let base = if lib.url.ends_with('/') { lib.url.clone() } else { format!("{}/", lib.url) };
            let rel_path = coord.path();
            Ok(Stored {
                path: root.join(&rel_path),
                artifact: Artifact {
                    name: lib.name.clone(),
                    url: format!("{base}{rel_path}"),
                    rel_path,
                    expect: Expected::sha256(&lib.sha256)?,
                    size: Some(lib.size),
                },
            })
        })
        .collect()
}

/// Steps 2–3: everything on disk and verified once.
pub async fn prepare(
    paths: &Paths,
    cfg: &Config,
    http: &HttpClient,
    inst: &Instance,
    repair: bool,
    reporter: &Reporter,
) -> Result<Prepared> {
    let store = Store { paths, http, parallel: cfg.downloads.parallel, repair, reporter };
    let (pin, loader_libs) = match &inst.kind {
        Kind::Vanilla => (None, Vec::new()),
        Kind::Curated { manifest, .. } => (
            manifest.minecraft.version_json_sha1.as_deref().map(Expected::sha1).transpose()?,
            loader_artifacts(paths, manifest)?,
        ),
    };
    let installed = store.install(&inst.minecraft, pin.as_ref()).await?;
    if !loader_libs.is_empty() {
        store.ensure(loader_libs.clone(), 256 << 20, "mod loader libraries").await?;
    }
    Ok(Prepared { installed, loader_libs })
}

// ---------------------------------------------------------------- logging

/// Launch logs kept per instance. Each is capped in size (`MAX_LOG_BYTES`); this caps
/// their number, so an instance's log directory is bounded however often it is started.
const KEEP_LAUNCH_LOGS: usize = 20;

/// Delete the oldest `launch-<unix time>.log` files in `dir` until at most `keep` remain.
/// Only files with exactly that name shape are counted or touched.
fn prune_logs(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut logs: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let stamp = name.to_str()?.strip_prefix("launch-")?.strip_suffix(".log")?;
            let regular = entry.file_type().ok()?.is_file();
            (regular && !stamp.is_empty() && stamp.len() <= 20 && stamp.bytes().all(|b| b.is_ascii_digit()))
                .then(|| Some((stamp.parse().ok()?, entry.path())))
                .flatten()
        })
        .collect();
    logs.sort();
    let excess = logs.len().saturating_sub(keep);
    for (_, path) in logs.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

/// Whose line it is, for the log's size limit (see [`LOG_SHARES`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Share {
    /// What the launcher itself has to say. Nothing the game does adds to it.
    Launcher = 0,
    /// The relay and the credential broker: one line per connection, lookup or request.
    Egress = 1,
    /// Output of the nested compositor (shown as the launcher's, written by a program that
    /// the game talks to).
    Display = 2,
    Game = 3,
}

impl Share {
    fn of(source: Source) -> Self {
        match source {
            Source::Launcher => Share::Launcher,
            Source::Egress => Share::Egress,
            Source::Game => Share::Game,
        }
    }
    fn what(self) -> &'static str {
        match self {
            Share::Launcher => "launcher",
            Share::Egress => "relay",
            Share::Display => "display server",
            Share::Game => "game",
        }
    }
}

struct Log {
    /// The file, and per share the bytes written so far (`None` once its limit was reached).
    file: Mutex<(File, [Option<u64>; 4])>,
    limits: [u64; 4],
    /// Everything recorded with [`Log::line`] is also shown live (terminal or front end).
    reporter: Reporter,
}

impl Log {
    fn open(paths: &Paths, instance: &str, reporter: &Reporter) -> Result<(Arc<Self>, PathBuf)> {
        let dir = paths.logs().join(instance);
        util::create_private_dir_all(&dir)?;
        prune_logs(&dir, KEEP_LAUNCH_LOGS - 1);
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let path = dir.join(format!("launch-{stamp}.log"));
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .ctx(|| format!("opening {}", path.display()))?;
        Ok((Arc::new(Log::over(file, LOG_SHARES, reporter)), path))
    }

    fn over(file: File, limits: [u64; 4], reporter: &Reporter) -> Self {
        Log { file: Mutex::new((file, [Some(0); 4])), limits, reporter: reporter.clone() }
    }

    /// Write to the file only. `text` must already be sanitised if it came from the sandbox.
    fn record_as(&self, share: Share, source: Source, text: &str) {
        let tag = source.tag();
        let Ok(mut guard) = self.file.lock() else { return };
        let (file, written) = &mut *guard;
        let size = (tag.len() + text.len() + 4) as u64;
        let (slot, limit) = (&mut written[share as usize], self.limits[share as usize]);
        match *slot {
            Some(so_far) if so_far + size <= limit => {
                let _ = writeln!(file, "[{tag}] {text}");
                *slot = Some(so_far + size);
            }
            // Said once per share, and not counted against anybody: four short lines at most.
            Some(_) => {
                let _ = writeln!(
                    file,
                    "[launcher] log size limit for {} output reached; further {} output is not recorded",
                    share.what(),
                    share.what()
                );
                *slot = None;
            }
            None => {}
        }
    }

    fn record(&self, source: Source, text: &str) {
        self.record_as(Share::of(source), source, text);
    }

    /// Record and show live. `text` must already be sanitised if it came from the sandbox.
    fn line(&self, source: Source, text: &str) {
        self.record(source, text);
        self.reporter.log(source, text);
    }

    /// A line of the nested compositor: the launcher's by name, but with a share of its own,
    /// because how much that program prints is not the launcher's doing.
    fn display_line(&self, text: &str) {
        self.record_as(Share::Display, Source::Launcher, text);
        self.reporter.log(Source::Launcher, text);
    }
}

/// Forward one output stream of the sandbox, line by line. Everything is sanitised: the
/// game's output is attacker-influenced text that must not reach a terminal raw.
async fn relay_output(stream: impl AsyncRead + Unpin, source: Source, log: Arc<Log>) {
    let mut reader = BufReader::with_capacity(64 * 1024, stream);
    let mut buf = Vec::with_capacity(1024);
    loop {
        buf.clear();
        // Bound memory even if the game never emits a newline.
        let mut limited = (&mut reader).take(256 * 1024);
        match limited.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                log.line(source, &util::sanitize_line(&buf, MAX_LOG_LINE));
            }
        }
    }
}

// ---------------------------------------------------------------- per-launch scratch

/// How much a burner session may copy into the cache: what is free there now, less a tenth
/// (at least 1 GiB) that stays free. The instance's worlds were written by earlier game
/// sessions; a file that claims a terabyte must end the launch, not fill the disk.
fn burner_copy_budget(cache: &std::path::Path) -> Result<u64> {
    let stat = nix::sys::statvfs::statvfs(cache).ctx(|| format!("examining the file system of {}", cache.display()))?;
    let free = (stat.blocks_available() as u64).saturating_mul(stat.fragment_size() as u64);
    Ok(free.saturating_sub((free / 10).max(1 << 30)))
}

/// The game root for one session, plus extracted natives. Removed on drop, whatever happens —
/// as far as that is possible: [`Scratch::remove`] is the way to find out whether it was.
struct Scratch {
    root: PathBuf,
    natives: Option<PathBuf>,
    removed: bool,
    /// Tell a sweep by another launch (or by `scram`) that these directories are in use.
    locks: Vec<crate::running::DirLock>,
}

impl Scratch {
    fn create(paths: &Paths, instance: &str) -> Result<Self> {
        let base = paths.scratch();
        util::create_private_dir_all(&base)?;
        let root = base.join(format!("{instance}-{}", util::hex_encode(&util::random_bytes::<8>()?)));
        util::create_private_dir_all(&root)?;
        let lock = crate::running::lock_dir(&root).ctx(|| format!("locking {}", root.display()))?;
        Ok(Scratch { root, natives: None, removed: false, locks: vec![lock] })
    }

    /// A directory for natives unpacked by the launcher, next to the game root.
    fn natives_dir(&mut self, paths: &Paths) -> Result<PathBuf> {
        let dir = paths.cache.join(format!("natives-{}", util::hex_encode(&util::random_bytes::<8>()?)));
        util::create_private_dir_all(&dir)?;
        self.locks.push(crate::running::lock_dir(&dir).ctx(|| format!("locking {}", dir.display()))?);
        self.natives = Some(dir.clone());
        Ok(dir)
    }

    /// Delete everything the session left. The tree was written by the sandbox: it is
    /// removed the careful way (`tree::remove`), for a bounded time, and what could not be
    /// removed is returned, one message per directory — never passed over.
    fn remove(&mut self) -> Vec<String> {
        self.removed = true;
        self.locks.clear();
        let deadline = std::time::Instant::now() + SCRATCH_REMOVAL_TIME;
        let mut failures = Vec::new();
        for dir in std::iter::once(&self.root).chain(&self.natives) {
            match crate::tree::remove(dir, Some(deadline)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => failures.push(format!("{}: {e}", dir.display())),
            }
        }
        failures
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // The launch is ending some other way (an error, a cancelled start): best effort,
        // and whatever stays is swept — and reported — by a later launch.
        if !self.removed {
            let _ = self.remove();
        }
    }
}

// ---------------------------------------------------------------- the launch itself

struct ClasspathEntry {
    key: String,
    sandbox_path: String,
    file: File,
}

fn classpath_entry(index: usize, stored: &Stored, file: File) -> ClasspathEntry {
    let name = stored.artifact.rel_path.rsplit('/').next().unwrap_or("lib.jar");
    let name = if validate_file_name(name).is_ok() { name } else { "lib.jar" };
    let key =
        Coordinate::parse(&stored.artifact.name).map(|c| c.key()).unwrap_or_else(|_| stored.artifact.rel_path.clone());
    ClasspathEntry { key, sandbox_path: format!("{SUPPORT_DIR}/lib/{index:04}-{name}"), file }
}

fn dashed_uuid(id: &str) -> String {
    if id.len() == 32 {
        format!("{}-{}-{}-{}-{}", &id[..8], &id[8..12], &id[12..16], &id[16..20], &id[20..])
    } else {
        id.to_string()
    }
}

pub async fn launch(paths: &Paths, cfg: &Config, opts: &LaunchOptions, reporter: &Reporter) -> Result<i32> {
    let say = |m: &str| reporter.status(m);
    reporter.phase(Phase::Preparing);

    // 1. Instance (signature verified here for curated profiles).
    let inst = instance::load(paths, &opts.instance)?;
    match &inst.kind {
        Kind::Vanilla => say(&format!("instance {:?}: vanilla Minecraft {}", inst.id, inst.minecraft)),
        Kind::Curated { manifest, signed_by } => say(&format!(
            "instance {:?}: curated profile serial {} for Minecraft {} — signature by {signed_by:?} verified",
            inst.id, manifest.serial, inst.minecraft
        )),
    }
    // So that `bastion-mc scram` can find and stop this launch.
    let _running = crate::running::Marker::create(paths, &inst.id)?;
    // Bounded: how much an earlier game left behind was that game's choice.
    let swept = crate::running::sweep_leftovers(
        paths,
        Duration::from_secs(60),
        Some(std::time::Instant::now() + Duration::from_secs(20)),
    );
    if swept.removed > 0 {
        say(&format!("removed {} leftover director(ies) of earlier launches that did not end properly", swept.removed));
    }
    for failure in &swept.failed {
        reporter.warn(format!(
            "WARNING: a directory an earlier launch left in {} could not be removed ({failure}); it holds what that \
             game wrote. The next launch tries again.",
            paths.scratch().display()
        ));
    }
    if cfg.launcher.confine == ConfineMode::Require
        && let confine::Support::Partly(gaps) = confine::support()
    {
        // Known before anything is started: "require" means the whole confinement, and this
        // kernel cannot enforce all of it.
        return Err(Error::Sandbox(format!("launcher.confine = \"require\", but {}", gaps.join("; "))));
    }
    if opts.burner && opts.join.is_some() {
        return Err(Error::Usage("--burner has no network; it cannot be combined with --join".into()));
    }
    // A burner session of an owned account is the full game, offline; of anything else, the demo.
    let burner_identity = if opts.burner && !opts.demo { auth::identity::recall(paths, &opts.account) } else { None };
    let demo = opts.demo || (opts.burner && burner_identity.is_none());
    if opts.burner {
        say(&match &burner_identity {
            Some(identity) => format!(
                "burner session: offline, nothing is kept; full game for {} (ownership was verified at the last \
                 sign-in of account {:?})",
                identity.player, opts.account
            ),
            None => "burner session: offline, nothing is kept; demo mode (no signed-in account on this machine to \
                     vouch for ownership)"
                .to_string(),
        });
    }
    if !demo && !opts.burner && cfg.auth.client_id.is_none() {
        return Err(Error::Config(
            "auth.client_id is not set. This launcher ships no client ID: register your own Azure application \
             and put its ID in config.toml (see README), or use --demo to test the sandbox without an account."
                .into(),
        ));
    }

    // What the desktop will be asked for (display, audio, GPU) is decided here, long before
    // it is used in step 6: a refusal — X11 without consent, a sound server that would load
    // modules for the game — should not come after a long download.
    let mut desktop = display::plan(&cfg.sandbox, paths, &inst.id)?;
    // Held until the game has exited: the compositor accepts on the game's socket only
    // while it is.
    let wayland_context = desktop.wayland_context.take();
    // What is said about the display is what was seen, not what was asked for: for a
    // security context, the list of what its socket offers (`display::plan` looked); for a
    // nested compositor, the report of its clipboard cut-off — which does not exist before
    // the compositor runs, so that case is settled where it is started.
    let mut display_protection: (Shield, String) = if desktop.nested.is_some() {
        (Shield::Partial, "nested compositor: not started yet".into())
    } else {
        let seen = desktop.wayland_isolation.as_ref().filter(|_| wayland_context.is_some());
        shields::planned_display(seen, desktop.uses_x11)
    };

    // 2–3. Files present and verified.
    let http = http_client(cfg)?;
    if let Some(proxy) = http.socks5() {
        // What the proxy carries depends on what stands between the game and the network.
        let mode = if opts.burner { NetworkMode::None } else { inst.config.network.mode.unwrap_or(cfg.network.mode) };
        say(&egress::describe_proxy(proxy, mode));
    }
    let prepared = prepare(paths, cfg, &http, &inst, opts.repair, reporter).await?;
    let installed = &prepared.installed;

    // The mods directory must match the signed list exactly (or be empty for vanilla).
    let mods_dir = inst.dir.join("mods");
    let (verified_mods, manifest) = match &inst.kind {
        Kind::Vanilla => (mods::verify_mods_dir(&mods_dir, &[])?, None),
        Kind::Curated { manifest, .. } => (mods::verify_mods_dir(&mods_dir, &manifest.mods)?, Some(manifest.as_ref())),
    };
    if !verified_mods.is_empty() {
        say(&format!("mods: {} jar(s) match the signed manifest", verified_mods.len()));
    }
    // Resource and shader packs: exactly the ones the user imported, unchanged.
    let verified_packs = packs::verify(&inst.dir)?;
    for group in verified_packs.iter().filter(|g| !g.files.is_empty()) {
        say(&format!("packs: {} {}(s) match the instance's pack list", group.files.len(), group.kind.noun()));
    }

    // 4. System JVM.
    let jvm =
        java::select(&java::discover()?, installed.json.java_major(), cfg.java.matching, cfg.java.home.as_deref())?;
    say(&format!(
        "jvm: Java {} at {} (game declares Java {})",
        jvm.version,
        jvm.home.display(),
        installed.json.java_major()
    ));

    // 5. Re-verify from the descriptors that will be bound.
    reporter.phase(Phase::Verifying);
    let threads = cfg.downloads.parallel;
    let (verify_assets, inst2) = (cfg.downloads.verify_assets_on_launch, installed.clone());
    let files = tokio::task::spawn_blocking(move || inst2.verify_for_launch(verify_assets, threads))
        .await
        .map_err(|e| Error::Other(format!("verification task failed: {e}")))??;
    let mut loader_files = Vec::new();
    for stored in &prepared.loader_libs {
        loader_files.push((stored.clone(), stored.open_verified()?));
    }
    say(&format!(
        "verified: client jar, {} libraries, asset index{}{}",
        files.libraries.len() + loader_files.len(),
        if verify_assets { format!(", {} asset objects", installed.asset_objects.len()) } else { String::new() },
        if files.logging.is_some() { ", logging configuration" } else { "" }
    ));

    // Class path: loader libraries first; a loader library replaces the vanilla one of the
    // same group:artifact (the loader ships the newer ASM, for example).
    let mut classpath: Vec<ClasspathEntry> = Vec::new();
    for (stored, file) in loader_files {
        let entry = classpath_entry(classpath.len(), &stored, file);
        classpath.push(entry);
    }
    let loader_keys: BTreeSet<String> = classpath.iter().map(|e| e.key.clone()).collect();
    for (stored, file) in files.libraries {
        let entry = classpath_entry(classpath.len(), &stored, file);
        if !loader_keys.contains(&entry.key) {
            classpath.push(entry);
        }
    }
    let client_path = format!("{SUPPORT_DIR}/client.jar");
    let classpath_string = std::iter::once(format!("{SUPPORT_DIR}/boot"))
        .chain(classpath.iter().map(|e| e.sandbox_path.clone()))
        .chain(std::iter::once(client_path.clone()))
        .collect::<Vec<_>>()
        .join(":");

    // 6. Network, display, audio, GPU.
    let join: Option<ServerRule> = opts.join.as_deref().map(|j| parse_server(j, false)).transpose()?;
    let mode = if opts.burner { NetworkMode::None } else { inst.config.network.mode.unwrap_or(cfg.network.mode) };
    let (network, tcp, resolv, policy) = match mode {
        NetworkMode::Filtered => {
            let policy = egress::Policy::new(&cfg.network, &inst.config.network, join.clone())?;
            for line in policy.describe() {
                say(&line);
            }
            (
                Network::Filtered(policy.netns_spec()),
                TcpPolicy::Ports(policy.tcp_ports()),
                Resolv::Loopback,
                Some(policy),
            )
        }
        NetworkMode::Open => {
            say(
                "network: OPEN — the game shares the host network: no egress control, host loopback services reachable",
            );
            (Network::Open, TcpPolicy::AnyConnect, Resolv::Host, None)
        }
        NetworkMode::None => {
            say("network: none (offline)");
            (Network::None, TcpPolicy::Ports(vec![]), Resolv::None, None)
        }
    };
    for note in &desktop.notes {
        say(note);
    }

    // Scratch game root and the instance's writable directories.
    let mut scratch = Scratch::create(paths, &inst.id)?;
    // The settings files the launcher carries live in a directory of their own inside the
    // instance: it is the only part of the instance the launcher can still write once it
    // has confined itself — not `instance.toml`, not the manifest, not the pack list. (A
    // burner session writes nothing back, and so creates and moves nothing.)
    let persist_dir = inst.dir.join(instance::PERSIST_DIR);
    if !opts.burner {
        for note in instance::adopt_persisted(&inst.dir, &cfg.sandbox.persist_files)? {
            say(&format!("settings: {note}"));
        }
    }
    for name in &cfg.sandbox.persist_files {
        // Into the session: re-encoded, so a file damaged on the host does not reach the
        // game's parsers either. A file that does not parse is left where it is and the
        // game starts with its defaults.
        let from = if opts.burner { instance::persisted_in(&inst.dir, name) } else { persist_dir.clone() };
        match persist::carry(&from, &scratch.root, name) {
            Ok(Some(report)) => {
                if let Some(note) = report.describe(name) {
                    say(&format!("settings: {note}"));
                }
            }
            Ok(None) => {}
            Err(e) => say(&format!("settings: not handing the game the stored {name}: {e}")),
        }
    }
    let mut rw_dirs: Vec<String> = WRITABLE_DIRS.iter().map(|s| s.to_string()).collect();
    rw_dirs.extend(cfg.sandbox.extra_rw_dirs.iter().cloned());
    rw_dirs.dedup();
    for d in &rw_dirs {
        instance::ensure_plain_dir(&inst.dir.join(d))?;
    }
    if opts.burner {
        // Copies inside the per-launch scratch root, which is deleted when the launch ends.
        // Bounded: these directories were written by earlier game sessions.
        let budget = burner_copy_budget(&paths.cache)?;
        let (mut files, mut bytes) = (0u64, 0u64);
        for d in &rw_dirs {
            let limits = crate::tree::CopyLimits {
                max_entries: BURNER_MAX_ENTRIES,
                max_bytes: budget.saturating_sub(bytes),
                max_depth: 64,
            };
            let copied = crate::tree::copy(&inst.dir.join(d), &scratch.root.join(d), &limits).map_err(|e| match e {
                Error::Policy(why) => Error::Policy(format!(
                    "a burner session copies the instance's {d}/ into the cache, and that copy was stopped: {why}. \
                     (Free space in {} allows {} MiB.)",
                    paths.cache.display(),
                    budget >> 20
                )),
                other => other,
            })?;
            files += copied.files;
            bytes += copied.bytes;
        }
        say(&format!(
            "burner session: the game works on copies of {} ({files} files, {} MiB); they are deleted afterwards",
            rw_dirs.join(", "),
            bytes >> 20
        ));
    }

    // The credential broker (filtered network only: it lives in the relay). Its certificate
    // authority exists from here on, in this process's memory and nowhere else; the one
    // place its certificate goes is the trust store generated for this sandbox.
    let broker_on = matches!(mode, NetworkMode::Filtered) && cfg.network.broker.enabled;
    let authority = if broker_on { Some(egress::broker::Authority::generate()?) } else { None };

    // Mounts.
    let trust = TrustStore::build(authority.as_ref().map(|a| ("bastion-mc-credential-broker", a.certificate_der())))?;
    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: cfg.sandbox.tmp_size_mb,
        jvm: Some(&jvm),
        exec_allow: &cfg.sandbox.exec_allow,
        resolv,
        expose_timezone: cfg.sandbox.expose_timezone,
        trust: Some(&trust),
    })?;
    let mut mounts = std::mem::take(&mut base.mounts);
    mounts.extend(desktop.mounts);
    mounts.push(Mount::RoData {
        data: BOOTSTRAP_CLASS.to_vec(),
        dst: format!("{SUPPORT_DIR}/boot/{BOOTSTRAP_CLASS_NAME}.class").into(),
        mode: 0o444,
    });
    for entry in classpath {
        mounts.push(Mount::RoBindFd { file: entry.file, dst: entry.sandbox_path.into() });
    }
    mounts.push(Mount::RoBindFd { file: files.client, dst: client_path.into() });

    let assets_root = format!("{SUPPORT_DIR}/assets");
    let index_id = installed.json.asset_index.id.clone();
    mounts.push(Mount::RoBind { src: paths.assets().join("objects"), dst: format!("{assets_root}/objects").into() });
    mounts.push(Mount::RoBindFd {
        file: files.asset_index,
        dst: format!("{assets_root}/indexes/{index_id}.json").into(),
    });
    let mut game_assets = assets_root.clone();
    let mut resources_bind = None;
    if let Some(tree) = &installed.logical_assets {
        if installed.assets.map_to_resources {
            game_assets = format!("{GAME_DIR}/resources");
            resources_bind = Some(tree.clone());
        } else {
            game_assets = format!("{assets_root}/virtual/{index_id}");
            mounts.push(Mount::RoBind { src: tree.clone(), dst: game_assets.clone().into() });
        }
    }

    let mut jvm_extra: Vec<String> = Vec::new();
    if let Some((argument, _stored, file)) = files.logging {
        let dst = format!("{SUPPORT_DIR}/log4j2.xml");
        mounts.push(Mount::RoBindFd { file, dst: dst.clone().into() });
        jvm_extra.push(argument.replace("${path}", &dst));
    }

    // The companion mod, for Fabric profiles of tested versions (see `companion`): its jar
    // and settings as read-only files outside the signed mods directory, the one property
    // that makes Fabric Loader pick the jar up, and a socket back to the launcher.
    let companion = companion::plan(&cfg.companion, &inst, jvm.major, paths)?;
    companion.attach(&mut mounts, &mut jvm_extra, &mut base.landlock.unix_sockets);

    // Natives: old versions ship them in jars the launcher must unpack (from the verified
    // descriptors, into a directory the game cannot write); new versions unpack their own
    // into a scratch directory.
    let natives_dir = if files.natives.is_empty() {
        let dir = "/tmp/natives".to_string();
        for sub in ["", "/java", "/jna", "/lwjgl", "/netty"] {
            mounts.push(Mount::Dir { dst: format!("{dir}{sub}").into(), mode: 0o700 });
        }
        dir
    } else {
        let host = scratch.natives_dir(paths)?;
        let mut count = 0;
        for (_stored, exclude, file) in &files.natives {
            count += extract_natives(file, exclude, &host)?;
        }
        say(&format!("natives: {count} shared objects unpacked from {} verified archives", files.natives.len()));
        let dir = format!("{SUPPORT_DIR}/natives");
        mounts.push(Mount::RoBind { src: host, dst: dir.clone().into() });
        dir
    };

    mounts.push(Mount::Bind { src: scratch.root.clone(), dst: GAME_DIR.into() });
    if !opts.burner {
        for d in &rw_dirs {
            mounts.push(Mount::Bind { src: inst.dir.join(d), dst: format!("{GAME_DIR}/{d}").into() });
        }
    }
    if !verified_mods.is_empty() {
        // A tmpfs holding only the verified jars, then sealed read-only.
        let mods_dst = format!("{GAME_DIR}/mods");
        mounts.push(Mount::Tmpfs { dst: mods_dst.clone().into(), size_bytes: Some(1 << 20), mode: 0o755 });
        for m in verified_mods {
            mounts.push(Mount::RoBindFd { file: m.file, dst: format!("{mods_dst}/{}", m.file_name).into() });
        }
        mounts.push(Mount::RemountRo { dst: mods_dst.into() });
    }
    // Packs: always a sealed directory, also when it is empty — the game can read packs
    // but never put one where the next session would load it.
    for group in verified_packs {
        let dst = format!("{GAME_DIR}/{}", group.kind.dir());
        mounts.push(Mount::Tmpfs { dst: dst.clone().into(), size_bytes: Some(1 << 20), mode: 0o755 });
        for pack in group.files {
            mounts.push(Mount::RoBindFd { file: pack.file, dst: format!("{dst}/{}", pack.file_name).into() });
        }
        mounts.push(Mount::RemountRo { dst: dst.into() });
    }
    if let Some(tree) = resources_bind {
        mounts.push(Mount::RoBind { src: tree, dst: format!("{GAME_DIR}/resources").into() });
    }

    // Landlock policy.
    let mut landlock = base.landlock;
    landlock.write.push(GAME_DIR.into());
    landlock.devices.extend(desktop.devices);
    landlock.unix_sockets.extend(desktop.sockets);
    landlock.tcp = tcp;
    // In open mode on X11, libxcb tries the server's abstract socket first and treats a
    // permission error as fatal instead of falling back to the socket file.
    // Always: an X11 client is given its display as a socket path (never an abstract name),
    // so even with the host's network namespace there is no reason to reach an abstract
    // socket outside the sandbox.
    landlock.scope_abstract_unix = true;
    // UDP: in filtered mode a datagram can reach the launcher's DNS responder and, only if
    // a server rule names UDP ports (voice chat), the launcher's relay sockets on exactly
    // those ports; nothing at all in offline mode. (Landlock ABI 10; see `restrict_udp`.)
    landlock.udp_send = match mode {
        NetworkMode::Filtered => Some(policy.as_ref().map_or_else(|| vec![53], egress::Policy::sandbox_udp_ports)),
        NetworkMode::None => Some(Vec::new()),
        NetworkMode::Open => None,
    };

    // Template variables.
    let mut features = Features::default();
    if demo {
        features.set("is_demo_user");
    }
    let resolution = inst.config.width.zip(inst.config.height);
    if resolution.is_some() {
        features.set("has_custom_resolution");
    }
    let quick_play = join.is_some() && installed.json.has_feature_rule("is_quick_play_multiplayer");
    if quick_play {
        features.set("is_quick_play_multiplayer");
    }

    // 7. Authenticate — last thing before the sandbox starts.
    let mut canary_note = None;
    let session = if opts.burner {
        say("auth: burner session — nobody is signed in, no token exists");
        None
    } else if demo {
        say("auth: demo mode — no account, no token");
        None
    } else {
        reporter.phase(Phase::Authenticating);
        let client_id = cfg.auth.client_id.as_deref().expect("checked above");
        let store = TokenStore::open(&cfg.auth, paths)?;
        let refresh = store.load(&opts.account)?.ok_or_else(|| {
            Error::Auth(format!("not signed in (account {:?}). Run `bastion-mc login` first.", opts.account))
        })?;
        let (session, next_refresh) = auth::authenticate(&http, client_id, &refresh, cfg.auth.allow_game_pass).await?;
        drop(refresh);
        store.save(&opts.account, &next_refresh)?;
        // From here on no refresh token is in this process's memory. The stored one is still
        // within its reach — until step 9, where the launcher confines itself and loses the
        // kernel keyring (and, by Landlock, the vault directory and the `age` program).
        drop(next_refresh);
        say(&format!("auth: signed in as {} ({:?} licence)", session.profile_name, session.license));
        // Display-only memory of who this account is; failing to record it changes nothing.
        let identity =
            auth::identity::Identity { player: session.profile_name.clone(), uuid: session.profile_id.clone() };
        let _ = auth::identity::remember(paths, &opts.account, &identity);
        let _ = auth::activity::record(paths, &opts.account, "launch", &inst.id);
        // Did the account change behind our back? Checked before the token goes anywhere.
        match canary_verdict(paths, cfg, &opts.account, &session.profile)? {
            Ok(note) => {
                // What is not routine is shown whatever else is silenced (`--quiet`), and —
                // like every note of the canary — goes into the launch log once that is open.
                if note.notable {
                    reporter.warn(&note.text)
                } else {
                    say(&note.text)
                }
                canary_note = Some(note);
            }
            Err(refusal) => {
                if cfg.auth.canary_wipe {
                    store.delete(&opts.account)?;
                    say("canary: the stored refresh token for this account was deleted (auth.canary_wipe)");
                }
                return Err(refusal);
            }
        }
        Some(session)
    };
    // What the game is given as its "access token". With the broker (or with no network at
    // all) that is a placeholder that is worth nothing outside this launch; the real token
    // reaches the JVM only where nothing stands between the game and the network that could
    // act for it.
    let demo_token = Secret::from_slice(b"0")?;
    let placeholder = egress::broker::new_placeholder()?;
    let placeholder_token = Secret::from_slice(placeholder.as_bytes())?;
    let token_in_jvm = session.is_some() && !broker_on && !matches!(mode, NetworkMode::None);
    let (player, uuid, token): (String, String, &Secret) = match &session {
        Some(s) if token_in_jvm => (s.profile_name.clone(), s.profile_id.clone(), &s.access_token),
        Some(s) => (s.profile_name.clone(), s.profile_id.clone(), &placeholder_token),
        None => match &burner_identity {
            // Name and id are public; the "token" is the same worthless constant as in demo mode.
            Some(identity) => (identity.player.clone(), identity.uuid.clone(), &demo_token),
            None => ("Player".into(), "0".repeat(32), &demo_token),
        },
    };
    if token_in_jvm {
        say(match mode {
            NetworkMode::Open => {
                "auth: WARNING — with the open network the access token itself is handed to the game; code \
                 running in the game can read and use it. The credential broker needs `mode = \"filtered\"`."
            }
            _ => {
                "auth: WARNING — network.broker.enabled = false: the access token itself is handed to the game; \
                 code running in the game can read and use it."
            }
        });
    }

    let mut vars = Vars::default();
    vars.set("natives_directory", natives_dir)
        .set("launcher_name", "bastion-mc")
        .set("launcher_version", env!("CARGO_PKG_VERSION"))
        .set("classpath", classpath_string)
        .set("classpath_separator", ":")
        .set("library_directory", format!("{SUPPORT_DIR}/lib"))
        .set("version_name", installed.json.id.clone())
        .set("version_type", installed.json.kind.clone())
        .set("game_directory", GAME_DIR)
        .set("assets_root", assets_root)
        .set("assets_index_name", index_id)
        .set("game_assets", game_assets)
        .set("auth_player_name", player)
        .set("auth_uuid", uuid.clone())
        .set("user_type", "msa")
        .set("user_properties", "{}")
        // No launcher/installation identifiers are passed on: nothing for telemetry to key on.
        .set("clientid", "")
        .set("auth_xuid", "");
    if let Some((w, h)) = resolution {
        vars.set("resolution_width", w.to_string()).set("resolution_height", h.to_string());
    }
    if let Some(rule) = &join {
        vars.set("quickPlayMultiplayer", format!("{}:{}", rule.host, rule.port));
    }
    vars.set_secret("auth_access_token", vec![Piece::Sec(token)]);
    vars.set_secret(
        "auth_session",
        vec![Piece::Lit("token:".into()), Piece::Sec(token), Piece::Lit(format!(":{}", dashed_uuid(&uuid)))],
    );

    // JVM command line: version-provided options, then ours, which therefore win.
    let memory = inst.config.memory_mb.unwrap_or(cfg.java.memory_mb);
    let mut argv = vec!["java".to_string(), format!("-Xmx{memory}M")];
    let mut unknown = BTreeSet::new();
    for template in installed.json.jvm_arg_templates(&features) {
        let arg = render_plain(&template, &vars, &mut unknown)?;
        let lowered = arg.to_ascii_lowercase();
        if ["-javaagent", "-agentlib", "-agentpath", "-xbootclasspath"].iter().any(|p| lowered.starts_with(p)) {
            return Err(Error::Policy(format!("version file requests a forbidden JVM option: {arg:?}")));
        }
        argv.push(arg);
    }
    argv.extend(jvm_extra);
    let mut main_class = installed.json.main_class.clone();
    let mut game_templates = installed.json.game_arg_templates(&features);
    let mut allow_serialization = cfg.java.allow_serialization;
    if let Some(m) = manifest {
        allow_serialization |= m.options.allow_java_serialization;
        if let Some(loader) = &m.loader {
            argv.extend(loader.jvm_args.iter().cloned());
            main_class = loader.main_class.clone();
            game_templates.extend(loader.game_args.iter().cloned());
        }
    }
    argv.extend(java::hardening_flags(jvm.major, allow_serialization));
    if broker_on && jvm.major < 11 {
        // The broker speaks TLS 1.3 only; Java 8 has it but does not offer it by default.
        argv.push("-Djdk.tls.client.protocols=TLSv1.3,TLSv1.2".into());
    }
    argv.push(BOOTSTRAP_CLASS_NAME.into());
    if let (Some(rule), false) = (&join, quick_play) {
        game_templates.extend([
            "--server".to_string(),
            rule.host.to_string(),
            "--port".to_string(),
            rule.port.to_string(),
        ]);
    }
    if !unknown.is_empty() {
        say(&format!(
            "note: unknown placeholders expanded to nothing: {}",
            unknown.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    let (log, log_path) = Log::open(paths, &inst.id, reporter)?;
    // Said before the log existed; it belongs in the record of this launch all the same.
    if let Some(note) = &canary_note {
        log.record(Source::Launcher, &note.text);
    }
    log.line(Source::Launcher, &format!("jvm argv: {}", argv.join(" ")));
    log.line(
        Source::Launcher,
        &format!(
            "game argv (via launch channel, never on a command line): {main_class} {}",
            game_templates.iter().map(|t| render_redacted(t, &vars)).collect::<Vec<_>>().join(" ")
        ),
    );
    log.line(Source::Launcher, &companion.describe());

    // Resource limits. Asked of the user manager now, so that "it cannot" is known before
    // the game starts rather than discovered as a sandbox that fails to come up.
    let limits = match cfg.sandbox.cgroup {
        CgroupMode::Off => {
            say("limits: none (sandbox.cgroup = \"off\"): the game can use all memory and process slots you can");
            None
        }
        mode => match cgroup::preflight(cfg.sandbox.cpu_quota_percent.is_some()).await {
            Ok(_) => {
                let memory_max = cfg
                    .sandbox
                    .memory_max_mb
                    .map(u64::from)
                    .unwrap_or_else(|| 2 * u64::from(memory) + u64::from(cfg.sandbox.tmp_size_mb) + 1024);
                let cpu = match cfg.sandbox.cpu_quota_percent {
                    Some(q) => format!(", CPU {q}%"),
                    None => String::new(),
                };
                say(&format!(
                    "limits: {memory_max} MiB of memory, no swap, {} tasks{cpu} (systemd scope; a probe scope held \
                     its limits, the game's own are read back once it runs)",
                    cfg.sandbox.tasks_max
                ));
                Some(
                    cgroup::Limits::new(&inst.id, memory_max, cfg.sandbox.tasks_max)?
                        .with_cpu_quota(cfg.sandbox.cpu_quota_percent),
                )
            }
            Err(why) if mode == CgroupMode::Require => {
                return Err(Error::Sandbox(format!("sandbox.cgroup = \"require\", but {why}")));
            }
            Err(why) => {
                say(&format!("limits: none — {why}"));
                None
            }
        },
    };

    let mut env = base.env;
    env.extend(desktop.env);

    // A display server of the game's own, where the session's has no isolation (X11). It
    // is started only now — everything that could still fail without it has been done —
    // and the game is given its inner display, which did not exist until this moment.
    let mut nested_limits = None;
    let mut nested = match desktop.nested.take() {
        Some(mut plan) => {
            say("starting the nested compositor…");
            if limits.is_some() {
                // Its own scope: the game cannot starve the session by way of its display
                // server (X clients can make a server allocate on their behalf).
                plan.limits = Some(
                    cgroup::Limits::new(&format!("{}-display", inst.id), NESTED_MEMORY_MAX_MB, NESTED_TASKS_MAX)?
                        .with_cpu_quota(Some(NESTED_CPU_QUOTA_PERCENT)),
                );
                nested_limits = plan.limits.clone();
            }
            let sink = log.clone();
            let session =
                nested::start(plan, &opts.program, Arc::new(move |line: String| sink.display_line(&line))).await?;
            session.attach(&mut mounts, &mut landlock.unix_sockets, &mut env);
            log.line(
                Source::Launcher,
                &format!("display: the game's display is :{} inside the nested compositor", session.display),
            );
            // The clipboard: known from the compositor that is now running, or not claimed.
            match &session.clipboard {
                nested::ClipboardCut::Active => log.line(
                    Source::Launcher,
                    "display: the nested compositor's clipboard bridge is cut — the cut-off reported from inside the \
                     compositor that the bridge's calls land in it",
                ),
                nested::ClipboardCut::Unconfirmed(why) => {
                    let warning = format!(
                        "display: WARNING: the nested compositor's clipboard bridge is NOT known to be cut — {why}. \
                         The game may be able to read what you copy in other windows, and to set the clipboard."
                    );
                    log.record(Source::Launcher, &warning);
                    reporter.warn(&warning);
                }
            }
            display_protection = shields::nested_display(&session.clipboard);
            Some(session)
        }
        None => None,
    };
    let game_limits = limits.clone();
    let spec = SandboxSpec {
        mounts,
        network,
        limits,
        cwd: GAME_DIR.into(),
        policy: InitPolicy {
            exec: jvm.java.clone(),
            argv,
            env,
            landlock,
            seccomp: SeccompPolicy {
                mode: cfg.sandbox.seccomp,
                tripwire: cfg.sandbox.tripwire,
                extra_allow: cfg.sandbox.extra_syscalls.clone(),
            },
        },
    };

    // What the game will be told about the network is fixed before the game exists: SRV
    // records are looked up now, once. Afterwards the set of ports the relay connects to is
    // closed, and the launcher can hold itself to it.
    let srv = match &policy {
        Some(policy) => Some(policy.resolve_srv().await),
        None => None,
    };
    let outbound_ports = match (&policy, &srv) {
        (Some(policy), Some(srv)) => policy.outbound_ports(srv),
        _ => Vec::new(),
    };
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .ctx(|| "installing SIGINT handler")?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .ctx(|| "installing SIGTERM handler")?;
    let credential_protection: (Shield, &str) = if session.is_none() {
        (Shield::On, "no account and no token in this session")
    } else if broker_on {
        (Shield::On, "broker: the game holds a placeholder, the token stays in the launcher")
    } else if token_in_jvm {
        (Shield::Partial, "the session token is inside the game process")
    } else {
        (Shield::On, "offline: the token is not given to the game")
    };

    // 8. Start; deliver the frame; forget every secret.
    // From here a stop request means "shut the game down cleanly", not "abandon the launch".
    opts.stop.supervise();
    reporter.phase(Phase::Starting);
    let frame = build_frame(&main_class, &game_templates, &vars)?;
    drop(vars);
    // From the moment the game exists until the launcher has confined itself, nothing the
    // game can influence is read: its own output and its network sockets are picked up in
    // step 10, and the output of its display server — which the game talks to — is held
    // back here.
    let display_output_held = match &nested {
        Some(session) => Some(session.hold_output().await),
        None => None,
    };
    let mut running = sandbox::spawn(spec, &opts.program)?;
    let delivered = running.channel.write_all(frame.expose()).and_then(|_| running.channel.shutdown(Shutdown::Write));
    drop(frame); // wiped
    drop(demo_token);
    drop(placeholder_token);
    // The access token: with the broker it stays here, in locked memory, for the session —
    // the game never had it. Without the broker it has just gone to the game and this
    // process forgets it.
    let credential = match (session, broker_on) {
        (Some(s), true) => Some(egress::broker::Credential { token: s.access_token, profile_id: s.profile_id }),
        (session, _) => {
            drop(session); // wiped
            None
        }
    };
    delivered.ctx(|| "delivering the launch frame")?;
    // The game's Wayland socket needs its name only until bubblewrap has bound it, which it
    // has once the program inside exists. (If that cannot be established, the name stays
    // until the launcher exits.)
    let payload = running.payload_pid().await;
    if let (Some(context), Some(_)) = (&wayland_context, payload) {
        context.unlink();
    }
    // The companion channel's socket likewise — and now, while this process can still
    // remove a name in the runtime directory (step 9 takes that away). Reading from the
    // socket starts in step 10.
    if payload.is_some() {
        companion.release_name();
    }

    // 9. Not assumed: looked at. First the resource limits, read back from the kernel for the
    // program in the sandbox (and for its display server); a scope that was created is not
    // yet a limit that is in force.
    let mut limits_protection = shields::limits(shields::Limits::NotAsked);
    if let Some(limits) = &game_limits {
        let mut checks = vec![("the game", payload.map(|pid| pid as u32).or(running.child.id()), limits)];
        if let (Some(session), Some(limits)) = (&nested, &nested_limits) {
            checks.push(("the game's display server", session.process_id(), limits));
        }
        let mut problems = Vec::new();
        for (what, pid, limits) in checks {
            let verified = match pid {
                Some(pid) => {
                    cgroup::verify_started(pid, limits, Duration::from_secs(3)).await.map_err(|e| e.to_string())
                }
                None => Err("its process could not be identified".to_string()),
            };
            if let Err(why) = verified {
                problems.push(format!("{what}: {why}"));
            }
        }
        if problems.is_empty() {
            log.line(
                Source::Launcher,
                "limits: in force — memory.max, memory.swap.max, pids.max (and cpu.max, where a quota is set) of \
                 the sandbox's control group were read back from the kernel",
            );
            limits_protection = shields::limits(shields::Limits::InForce);
        } else if matches!(running.child.try_wait(), Ok(Some(_))) {
            // Nothing to read back from: the sandbox is already over (a game that failed to
            // start). That is reported as what it is, below — not as limits that failed.
            log.line(Source::Launcher, "limits: the sandbox ended before its limits could be read back");
            limits_protection = shields::limits(shields::Limits::SandboxEnded);
        } else {
            let why = problems.join("; ");
            if cfg.sandbox.cgroup == CgroupMode::Require {
                let _ = running.stop(Duration::from_secs(5)).await;
                return Err(Error::Sandbox(format!(
                    "sandbox.cgroup = \"require\", but the limits that were asked for are not in force: {why}"
                )));
            }
            let warning = format!(
                "limits: WARNING: asked for, but NOT in force — {why}. The game can use all the memory and process \
                 slots that you can."
            );
            log.record(Source::Launcher, &warning);
            reporter.warn(&warning);
            limits_protection = shields::limits(shields::Limits::NotInForce);
        }
    }

    // Then this process: from here on it is a supervisor and a relay. Whatever else a user
    // can do — read the home directory, run programs, read the kernel keyring, connect
    // anywhere — it gives up, on every thread, so that a bug in handling what the game sends
    // cannot be turned into that. This happens *before* anything the game sends is read.
    let launcher_protection: (Shield, String) = match cfg.launcher.confine {
        ConfineMode::Off => {
            log.line(Source::Launcher, "launcher: not confined (launcher.confine = \"off\")");
            shields::launcher_off()
        }
        mode => {
            let mut keep = Confinement::supervisor();
            // Settings carried back at exit: their directory, and nothing else of the
            // instance — not its configuration, its manifest or its pack list. A burner
            // session carries nothing back and keeps no part of the instance writable.
            if !opts.burner {
                keep.write.push(persist_dir.clone());
            }
            keep.write.push(paths.cache.clone()); // scratch and unpacked natives, removed at exit
            keep.tcp_connect.extend(outbound_ports);
            // What the broker forwards to Mojang goes through the proxy, if one is set.
            keep.tcp_connect.extend(http.socks5().map(|proxy| proxy.port()));
            let demand = if mode == ConfineMode::Require { confine::Demand::Whole } else { confine::Demand::Possible };
            let outcome = match confine::apply(&keep, demand) {
                Err(e) if mode == ConfineMode::Require => {
                    let _ = running.stop(Duration::from_secs(5)).await;
                    return Err(e);
                }
                outcome => outcome.map_err(|e| e.to_string()),
            };
            match &outcome {
                Ok(applied) if applied.is_whole() => log.line(
                    Source::Launcher,
                    &format!(
                        "launcher: confined itself, on all threads — Landlock: no program execution, files only under \
                         {}, TCP only to port(s) {}; system-call filter: no kernel keyring, no new namespaces",
                        if opts.burner {
                            "the cache (nothing of the instance)"
                        } else {
                            "the instance's persist/ and the cache"
                        },
                        keep.tcp_connect.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
                    ),
                ),
                Ok(applied) => {
                    let warning =
                        format!("launcher: WARNING: only PARTLY confined — not in force: {}", applied.gaps.join("; "));
                    log.record(Source::Launcher, &warning);
                    reporter.warn(&warning);
                }
                Err(e) => {
                    let warning = format!("launcher: WARNING: NOT confined — {e}");
                    log.record(Source::Launcher, &warning);
                    reporter.warn(&warning);
                }
            }
            shields::launcher(&outcome)
        }
    };

    // Which protections this launch runs with — said now, when each of them is a fact.
    {
        let item = |aspect: &str, shield: Shield, detail: &str| Protection {
            aspect: aspect.into(),
            shield,
            detail: detail.into(),
        };
        let network: (Shield, &str) = match mode {
            NetworkMode::Filtered => (Shield::On, "filtered: only allowlisted hosts, through the launcher"),
            NetworkMode::None => (Shield::On, "offline: no network at all"),
            NetworkMode::Open => (Shield::Off, "open: the host's network, no egress control"),
        };
        reporter.emit(Event::Protection {
            items: vec![
                item("display", display_protection.0, &display_protection.1),
                item("network", network.0, network.1),
                item("credential", credential_protection.0, credential_protection.1),
                item("limits", limits_protection.0, &limits_protection.1),
                item("launcher", launcher_protection.0, &launcher_protection.1),
            ],
        });
    }
    say(&format!("sandbox started; log: {}", log_path.display()));
    reporter.phase(Phase::Running);

    // 10. Supervise. Only now is anything the game sends looked at: its display server's
    // output, its network sockets, its own output.
    drop(display_output_held);
    let egress = match policy {
        Some(policy) => {
            let spec = policy.netns_spec();
            let sink = log.clone();
            let logger: egress::Logger = Arc::new(move |line: String| sink.line(Source::Egress, &line));
            let broker = match authority {
                Some(authority) => {
                    let broker =
                        egress::broker::Broker::new(authority, &cfg.network.broker, &http, placeholder, credential)?;
                    log.line(Source::Launcher, &broker.describe());
                    Some(broker)
                }
                None => None,
            };
            let srv = srv.expect("looked up with the policy");
            Some(egress::start(policy, &spec, std::mem::take(&mut running.egress), http.clone(), logger, srv, broker)?)
        }
        None => None,
    };
    let out = tokio::spawn(relay_output(running.child.stdout.take().expect("piped"), Source::Game, log.clone()));
    let err = tokio::spawn(relay_output(running.child.stderr.take().expect("piped"), Source::Game, log.clone()));
    // The companion's channel (its socket's name on the host went in step 8). What arrives
    // on it was written by something inside the game and is logged as such.
    let companion_channel = companion.serve({
        let sink = log.clone();
        Arc::new(move |voice, line: String| match voice {
            companion::channel::Voice::Launcher => sink.line(Source::Launcher, &line),
            companion::channel::Voice::Claimed => sink.line(Source::Game, &line),
        })
    });

    let mut interrupted = false;
    let status = tokio::select! {
        s = running.child.wait() => s.ctx(|| "waiting for the sandbox")?,
        _ = async { tokio::select! { _ = sigint.recv() => {}, _ = sigterm.recv() => {}, _ = opts.stop.wait() => {} } } => {
            say("stopping the game…");
            reporter.phase(Phase::Stopping);
            interrupted = true;
            running.stop(Duration::from_secs(15)).await.ctx(|| "stopping the sandbox")?
        }
        // The game's display server is gone — its window was closed, or it crashed. Without
        // a display the game is a process that cannot be seen or quit; stop it.
        gone = async {
            match nested.as_mut() {
                Some(session) => session.exited().await,
                None => std::future::pending().await,
            }
        } => {
            let closed = gone.as_ref().is_ok_and(|status| status.success());
            log.line(Source::Launcher, &match gone {
                Ok(status) if closed => format!("the nested compositor's window was closed ({status}); stopping the game"),
                Ok(status) => format!("the nested compositor ended unexpectedly ({status}); stopping the game"),
                Err(e) => format!("lost track of the nested compositor ({e}); stopping the game"),
            });
            reporter.phase(Phase::Stopping);
            interrupted = closed;
            running.stop(Duration::from_secs(15)).await.ctx(|| "stopping the sandbox")?
        }
    };
    if let Some(session) = nested.take() {
        session.stop().await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = out.await;
        let _ = err.await;
    })
    .await;

    if let Some(e) = egress {
        let (relayed, denied, refused) = (
            e.stats.relayed.load(std::sync::atomic::Ordering::Relaxed),
            e.stats.denied.load(std::sync::atomic::Ordering::Relaxed),
            e.stats.names_refused.load(std::sync::atomic::Ordering::Relaxed),
        );
        let count = |n: &std::sync::atomic::AtomicU64| n.load(std::sync::atomic::Ordering::Relaxed);
        let (brokered, credentialed, answered) =
            (count(&e.stats.brokered), count(&e.stats.credentialed), count(&e.stats.broker_refused));
        let (udp_flows, udp_relayed, udp_refused) =
            (count(&e.stats.udp_flows), count(&e.stats.udp_relayed), count(&e.stats.udp_refused));
        e.shutdown();
        log.line(
            Source::Launcher,
            &format!(
                "egress summary: {relayed} connection(s) relayed, {denied} denied, {refused} name lookup(s) refused"
            ),
        );
        if udp_flows + udp_relayed + udp_refused > 0 {
            log.line(
                Source::Launcher,
                &format!(
                    "egress summary, UDP: {udp_relayed} datagram(s) relayed in {udp_flows} flow(s), {udp_refused} refused"
                ),
            );
        }
        if broker_on {
            log.line(
                Source::Launcher,
                &format!(
                    "credential broker: {brokered} request(s) forwarded to Mojang, {credentialed} of them with your \
                     credential; {answered} answered by the launcher instead"
                ),
            );
        }
    }

    if let Some(channel) = companion_channel {
        log.line(Source::Launcher, &channel.finish());
    }

    // 11. Settings the launcher carries back on the game's behalf, into the one directory of
    // the instance it can still write. (Not from a burner session: nothing it wrote is kept.)
    for name in cfg.sandbox.persist_files.iter().filter(|_| !opts.burner) {
        // Back out: what the game left is untrusted. It is parsed and written out again;
        // what does not parse stays in the scratch directory and is deleted with it.
        match persist::carry(&scratch.root, &persist_dir, name) {
            Ok(Some(report)) => {
                if let Some(note) = report.describe(name) {
                    log.line(Source::Launcher, &format!("settings: {note}"));
                }
            }
            Ok(None) => {}
            Err(e) => log.line(Source::Launcher, &format!("not persisting {name}: {e}")),
        }
    }
    // What the game wrote into its root is deleted — and that is only said if it is true. The
    // game shapes that tree (it may leave directories without permission bits, or more files
    // than can be deleted in the time allowed); what could not be removed is named.
    let left_behind = scratch.remove();
    if left_behind.is_empty() {
        if opts.burner {
            log.line(
                Source::Launcher,
                "burner session: worlds, settings and everything else the game wrote have been deleted",
            );
        }
    } else {
        let warning = format!(
            "{}WARNING: NOT everything the game wrote could be deleted — {}. What is left holds only what that game \
             wrote{}; the next launch tries again and says whether it succeeded.",
            if opts.burner { "burner session: " } else { "" },
            left_behind.join("; "),
            if opts.burner { " (including its copies of the instance's worlds)" } else { "" }
        );
        log.record(Source::Launcher, &warning);
        reporter.warn(&warning);
    }
    drop(scratch);

    let code = status.code().unwrap_or(1);
    if interrupted {
        log.line(Source::Launcher, "game stopped on request");
        reporter.emit(Event::Exited { code: 130 });
        return Ok(130);
    }
    if code == SIGSYS_EXIT {
        reporter.emit(Event::Exited { code });
        log.line(
            Source::Launcher,
            "SECURITY: the game process was killed by the seccomp tripwire (SIGSYS). It issued a system call that \
             only exploit code uses (ptrace, bpf, mount, kexec, process_vm_*, …). Treat this instance's mods and the \
             server you were connected to as hostile. Kernel log: `journalctl -k -g 'type=1326'`.",
        );
        return Err(Error::Sandbox("the game was killed by the seccomp tripwire; see the log".into()));
    }
    log.line(Source::Launcher, &format!("game exited with status {code}"));
    reporter.emit(Event::Exited { code });
    Ok(code)
}

/// What the `Protection` event says about each thing this launch looked at.
///
/// The event is what a front end draws its shields from, so the rule is in one place and
/// has a test: a shield is `On` only for what was seen to be whole. Asked for, set up,
/// started — none of that is "on" by itself.
mod shields {
    use super::{Shield, confine, nested};

    /// The display, where nothing has to be started for it. `context_seen`: with a Wayland
    /// security context, what the launcher saw when it listed the game's socket (`Ok`: none
    /// of the known privileged interfaces; `Err`: one of them, or no listing).
    pub fn planned_display(
        context_seen: Option<&std::result::Result<String, String>>,
        uses_x11: bool,
    ) -> (Shield, String) {
        match context_seen {
            Some(Ok(seen)) => (Shield::On, format!("Wayland security context: {seen}")),
            Some(Err(problem)) => (
                Shield::Partial,
                format!("Wayland security context in place, but not shown to withhold anything: {problem}"),
            ),
            None if uses_x11 => {
                (Shield::Off, "the session's X11 display: the game can watch and type into other windows".into())
            }
            None => (Shield::Partial, "the session's Wayland socket, without a security context".into()),
        }
    }

    /// The display behind a nested compositor that is running.
    pub fn nested_display(clipboard: &nested::ClipboardCut) -> (Shield, String) {
        match clipboard {
            nested::ClipboardCut::Active => (
                Shield::On,
                "nested compositor: a display server of the game's own; its clipboard bridge is cut (reported from \
                 inside the compositor)"
                    .into(),
            ),
            nested::ClipboardCut::Unconfirmed(_) => (
                Shield::Partial,
                "nested compositor: a display server of the game's own, but its clipboard bridge is not known to be \
                 cut"
                .into(),
            ),
        }
    }

    /// What was found out about the resource limits.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Limits {
        /// None were asked for, or none could be had.
        NotAsked,
        /// Read back from the kernel, for the game and for its display server.
        InForce,
        /// The sandbox was over before anything could be read back.
        SandboxEnded,
        /// Asked for, and the kernel does not hold them.
        NotInForce,
    }

    pub fn limits(seen: Limits) -> (Shield, String) {
        match seen {
            Limits::NotAsked => (Shield::Off, "no resource limits".into()),
            Limits::InForce => {
                (Shield::On, "memory, swap and task limits, read back from the kernel (systemd scope)".into())
            }
            Limits::SandboxEnded => (Shield::Off, "the sandbox ended before its limits could be read back".into()),
            Limits::NotInForce => (Shield::Off, "resource limits were asked for but are not in force".into()),
        }
    }

    /// The launcher's own confinement, for what confining returned.
    pub fn launcher(outcome: &std::result::Result<confine::Applied, String>) -> (Shield, String) {
        match outcome {
            Ok(applied) if applied.is_whole() => {
                (Shield::On, "confined itself: Landlock and a system-call filter on all threads".into())
            }
            Ok(applied) => (Shield::Partial, format!("partly confined; not in force: {}", applied.gaps.join("; "))),
            Err(_) => (Shield::Off, "not self-confined: confining failed".into()),
        }
    }

    pub fn launcher_off() -> (Shield, String) {
        (Shield::Off, "not self-confined (launcher.confine = \"off\")".into())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The finding behind this module: "confined" was announced for a confinement that
        /// had not happened, or had happened in part. Every shield is `On` for exactly one
        /// outcome — the one in which the launcher saw the whole of what the shield stands
        /// for — and what is missing is in the text.
        #[test]
        fn a_shield_is_on_only_for_what_was_seen_to_be_whole() {
            // The launcher's own confinement.
            let whole = Ok(confine::Applied::default());
            assert_eq!(launcher(&whole).0, Shield::On);
            let gap = "this kernel's Landlock (ABI 8) cannot restrict connections to pathname UNIX sockets";
            let partly = Ok(confine::Applied { gaps: vec![gap.to_string()] });
            let (shield, detail) = launcher(&partly);
            assert_eq!(shield, Shield::Partial, "a confinement with a gap is not shown as on");
            assert!(detail.contains(gap), "and the gap is named: {detail}");
            assert_eq!(launcher(&Err("restrict_self: EPERM".into())).0, Shield::Off);
            assert_eq!(launcher_off().0, Shield::Off);

            // Resource limits: only what was read back from the kernel.
            for (seen, shield) in [
                (Limits::InForce, Shield::On),
                (Limits::NotInForce, Shield::Off),
                (Limits::SandboxEnded, Shield::Off),
                (Limits::NotAsked, Shield::Off),
            ] {
                assert_eq!(limits(seen).0, shield, "{seen:?}");
            }

            // The display. A security context is "on" when its socket was listed and found
            // free of privileged interfaces; one that offers them, or could not be listed,
            // is not — and neither is a context that was planned but is not in place.
            let listed =
                Ok("37 interface(s) offered on the game's socket, none of the 16 known privileged ones".into());
            assert_eq!(planned_display(Some(&listed), false).0, Shield::On);
            let leaking = Err("the compositor still offers the game's socket: zwlr_screencopy_manager_v1".to_string());
            let (shield, detail) = planned_display(Some(&leaking), false);
            assert_eq!(shield, Shield::Partial);
            assert!(detail.contains("zwlr_screencopy_manager_v1"), "{detail}");
            assert_eq!(planned_display(None, false).0, Shield::Partial, "the session's Wayland socket");
            assert_eq!(planned_display(None, true).0, Shield::Off, "the session's X11 display");

            // A nested compositor: only the cut-off's own report counts.
            assert_eq!(nested_display(&nested::ClipboardCut::Active).0, Shield::On);
            let unconfirmed = nested::ClipboardCut::Unconfirmed("the cut-off library did not report".into());
            let (shield, detail) = nested_display(&unconfirmed);
            assert_eq!(shield, Shield::Partial);
            assert!(detail.contains("not known to be cut"), "{detail}");
        }
    }
}

/// What the canary has to say about a profile that may be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryNote {
    pub text: String,
    /// Not routine — a difference that the configuration only warns about, a damaged record,
    /// or a first pinning (which is also what a deleted record looks like). Every caller shows
    /// such a note whatever else is silenced; none may drop it.
    pub notable: bool,
}

impl CanaryNote {
    fn routine(text: impl Into<String>) -> Self {
        CanaryNote { text: text.into(), notable: false }
    }
    fn notable(text: impl Into<String>) -> Self {
        CanaryNote { text: text.into(), notable: true }
    }
}

/// Compare the profile just returned by Mojang with the pinned one.
///
/// The outer `Result` is a failure to check at all. Inside: a note that the caller must
/// show (and record, where it keeps a log), or the refusal that stops the command.
///
/// Nothing is pinned here except a first sight, and capes gained by an unchanged profile. In
/// `warn` mode a difference and a damaged record leave the pin exactly as it was.
pub fn canary_verdict(
    paths: &Paths,
    cfg: &Config,
    account: &str,
    profile: &auth::minecraft::Profile,
) -> Result<std::result::Result<CanaryNote, Error>> {
    use crate::{auth::canary, config::CanaryMode};
    if cfg.auth.canary == CanaryMode::Off {
        return Ok(Ok(CanaryNote::routine("canary: off (auth.canary = \"off\")")));
    }
    let checked = match canary::check(paths, account, profile) {
        Ok(checked) => checked,
        Err(damaged) if cfg.auth.canary == CanaryMode::Enforce => return Ok(Err(damaged)),
        Err(damaged) => {
            return Ok(Ok(CanaryNote::notable(format!(
                "canary: WARNING: {damaged} (auth.canary = \"warn\", so this continues; the damaged record is left \
                 as it is)"
            ))));
        }
    };
    Ok(match checked {
        canary::Check::First => {
            canary::accept(paths, account, profile)?;
            Ok(CanaryNote::notable(format!(
                "canary: pinned this account's profile for the first time (player {}). If this account was pinned \
                 on this machine before, its record has been removed.",
                profile.name
            )))
        }
        canary::Check::Unchanged => {
            // Records capes gained since; a no-op otherwise.
            canary::accept(paths, account, profile)?;
            Ok(CanaryNote::routine("canary: profile unchanged since it was pinned (name, skin, cape)"))
        }
        canary::Check::Changed(differences) if cfg.auth.canary == CanaryMode::Enforce => {
            Err(canary::refusal(account, &differences))
        }
        canary::Check::Changed(differences) => Ok(CanaryNote::notable(format!(
            "canary: WARNING: the profile changed outside this launcher ({}); auth.canary = \"warn\", so this \
             continues and the old pin is kept. `bastion-mc canary accept` pins the new state.",
            differences.join("; ")
        ))),
    })
}

#[cfg(test)]
mod canary_tests {
    use super::*;
    use crate::{
        auth::minecraft::{Profile, Skin, Variant},
        config::CanaryMode,
    };

    fn profile(name: &str) -> Profile {
        Profile {
            id: "069a79f444e94726a5befca90e38aaf5".into(),
            name: name.into(),
            skins: vec![Skin {
                id: "6a6e65e576dd4c3ca625162924514568".into(),
                active: true,
                variant: Variant::Classic,
                texture: "1a4af718455d4aab".into(),
            }],
            capes: Vec::new(),
        }
    }

    #[test]
    fn the_canary_pins_first_then_enforces_warns_or_stays_out_of_it() {
        let root = std::env::temp_dir()
            .join(format!("bastion-cv-{}", crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        };
        paths.ensure().unwrap();
        let mut cfg = Config::default();
        assert_eq!(cfg.auth.canary, CanaryMode::Enforce, "enforcing is the default");

        let first = canary_verdict(&paths, &cfg, "default", &profile("Notch")).unwrap().unwrap();
        assert!(first.text.contains("first time") && first.notable, "a first pinning is never silent: {first:?}");
        let same = canary_verdict(&paths, &cfg, "default", &profile("Notch")).unwrap().unwrap();
        assert!(same.text.contains("unchanged") && !same.notable, "{same:?}");

        let refusal = canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().unwrap_err();
        assert!(matches!(refusal, Error::Policy(_)));
        let text = refusal.to_string();
        assert!(text.contains("Notch → Stolen") && text.contains("canary accept --account default"), "{text}");
        // Refusing does not move the pin: the next launch is refused as well.
        assert!(canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().is_err());

        cfg.auth.canary = CanaryMode::Warn;
        let warned = canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().unwrap();
        assert!(warned.text.contains("WARNING") && warned.text.contains("Notch → Stolen"), "{warned:?}");
        assert!(warned.notable, "a warning is shown whatever else is silenced");
        cfg.auth.canary = CanaryMode::Enforce;
        assert!(
            canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().is_err(),
            "a warning pins nothing"
        );

        cfg.auth.canary = CanaryMode::Off;
        let off = canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().unwrap();
        assert!(off.text.contains("off") && !off.notable);

        // A damaged record stops an enforcing launch too…
        cfg.auth.canary = CanaryMode::Enforce;
        let pin = paths.canaries().join("default.json");
        std::fs::write(&pin, "{}").unwrap();
        assert!(canary_verdict(&paths, &cfg, "default", &profile("Notch")).unwrap().is_err());
        // …and in warn mode it is said and left as it is: a warning replaces no record.
        cfg.auth.canary = CanaryMode::Warn;
        let damaged = canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().unwrap();
        assert!(damaged.notable && damaged.text.contains("damaged"), "{damaged:?}");
        assert_eq!(std::fs::read(&pin).unwrap(), b"{}", "the damaged record was replaced by a warning");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod scratch_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// What the launch says about deleting the game root rests on what `Scratch::remove`
    /// returns: nothing when the tree is gone — also one the game left without permission
    /// bits — and the name of every directory that is still there otherwise.
    #[test]
    fn a_game_root_that_could_not_be_deleted_is_named_and_one_without_permission_bits_is_deleted() {
        let root = std::env::temp_dir()
            .join(format!("bastion-scratch-{}", crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        };
        paths.ensure().unwrap();
        let chmod = |path: &std::path::Path, mode: u32| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
        };

        // What a game can leave: a directory it took every permission bit from.
        let mut scratch = Scratch::create(&paths, "vanilla").unwrap();
        let natives = scratch.natives_dir(&paths).unwrap();
        std::fs::create_dir_all(scratch.root.join("saves/copy-of-the-world")).unwrap();
        std::fs::write(scratch.root.join("saves/copy-of-the-world/level.dat"), b"world").unwrap();
        chmod(&scratch.root.join("saves"), 0o000);
        let game_root = scratch.root.clone();
        assert!(scratch.remove().is_empty(), "permission bits do not keep the tree");
        assert!(!game_root.exists() && !natives.exists());
        drop(scratch);

        // What cannot be deleted (here: its parent directory does not allow it) is named.
        let mut scratch = Scratch::create(&paths, "vanilla").unwrap();
        std::fs::write(scratch.root.join("options.txt"), b"fov:70\n").unwrap();
        let game_root = scratch.root.clone();
        chmod(&paths.scratch(), 0o500);
        let left = scratch.remove();
        chmod(&paths.scratch(), 0o700);
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].starts_with(&game_root.display().to_string()), "the directory is named: {left:?}");
        assert!(game_root.exists(), "and it is in fact still there");
        // Trying again (as the next launch's sweep does) finishes the job.
        assert!(scratch.remove().is_empty());
        assert!(!game_root.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;

    /// A game that prints until its share of the log is used up does not keep what is said
    /// afterwards — by the relay, by the display server, by the launcher — out of the file.
    #[test]
    fn a_source_that_fills_its_share_of_the_log_does_not_silence_the_others() {
        let dir = std::env::temp_dir()
            .join(format!("bastion-logshare-{}", crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launch.log");
        let reporter = Reporter::capture();
        // Shares in the order of `Share`: launcher, relay, display server, game.
        let log = Log::over(File::create(&path).unwrap(), [400, 300, 300, 1000], &reporter);

        log.line(Source::Launcher, "sandbox started");
        for i in 0..200 {
            log.line(Source::Game, &format!("flood line {i:03} ................................"));
        }
        // After the flood: everything a reader of the log needs to know about what the game did.
        log.line(Source::Egress, "egress: DENY  name lookup \"exfil.example\" (not allowlisted)");
        log.line(
            Source::Egress,
            "broker: api.minecraftservices.com POST /minecraft/profile/skins → 403 here [SECURITY]",
        );
        log.display_line("display server: (EE) something the game provoked");
        log.line(Source::Launcher, "SECURITY: the game process was killed by the seccomp tripwire (SIGSYS).");

        let text = std::fs::read_to_string(&path).unwrap();
        let count = |needle: &str| text.matches(needle).count();
        assert!((10..30).contains(&count("[game] flood line")), "the game's share is what it could fill: {text}");
        assert_eq!(count("log size limit for game output reached"), 1, "said once");
        assert_eq!(count("[egress] egress: DENY"), 1, "{text}");
        assert_eq!(count("[SECURITY]"), 1, "{text}");
        assert_eq!(count("[launcher] display server: (EE)"), 1, "{text}");
        assert_eq!(count("killed by the seccomp tripwire"), 1, "the launcher's own share is out of the game's reach");
        assert!(text.len() < 2400, "the file stays within the sum of the shares (plus the notices): {}", text.len());

        // The relay's share is its own too: a flood of refusals ends there, and the launcher's
        // summary — which counts them — is still written.
        for i in 0..50 {
            log.line(Source::Egress, &format!("egress: DENY  name lookup \"n{i}.example\" (not allowlisted)"));
        }
        log.line(Source::Launcher, "egress summary: 0 connection(s) relayed, 0 denied, 51 name lookup(s) refused");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("log size limit for relay output reached").count(), 1);
        assert!(text.contains("egress summary: 0 connection(s) relayed, 0 denied, 51 name lookup(s) refused"));
        // Everything was still shown live, whatever the file holds.
        let live = reporter.captured().iter().filter(|e| matches!(e, Event::Log { .. })).count();
        assert_eq!(live, 1 + 200 + 4 + 50 + 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn old_launch_logs_are_pruned_and_nothing_else_is_touched() {
        let dir = std::env::temp_dir()
            .join(format!("bastion-logs-{}", crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        for stamp in [5u64, 1, 9, 3, 7, 100, 20] {
            std::fs::write(dir.join(format!("launch-{stamp}.log")), b"x").unwrap();
        }
        for other in ["launch-.log", "launch-12x.log", "notes.txt", "launch-4.log.bak", "crash-1.log"] {
            std::fs::write(dir.join(other), b"keep").unwrap();
        }
        std::fs::create_dir(dir.join("launch-2.log")).unwrap(); // a directory with a log's name
        prune_logs(&dir, 3);
        let mut left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(
            left,
            [
                "crash-1.log",
                "launch-.log",
                "launch-100.log",
                "launch-12x.log",
                "launch-2.log",
                "launch-20.log",
                "launch-4.log.bak",
                "launch-9.log",
                "notes.txt"
            ],
            "the three newest by time stamp (numeric, not lexical) remain"
        );
        prune_logs(&dir, 3);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 9, "idempotent");
        prune_logs(std::path::Path::new("/nonexistent/bastion"), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
