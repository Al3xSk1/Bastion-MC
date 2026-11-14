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
//!  9. relay (filtered network) and log (sanitised) until the game exits
//! 10. carry the allowlisted settings files back (parsed and re-encoded, never copied);
//!     delete the scratch game root

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
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
/// The game controls how much it prints; the log must not be a way to fill the disk.
const MAX_LOG_BYTES: u64 = 256 << 20;
const SIGSYS_EXIT: i32 = 128 + libc::SIGSYS;
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

struct Log {
    /// The file and the number of bytes written to it so far.
    file: Mutex<(File, u64)>,
    /// Everything recorded is also shown live (terminal or front end).
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
        Ok((Arc::new(Log { file: Mutex::new((file, 0)), reporter: reporter.clone() }), path))
    }

    /// `text` must already be sanitised if it came from the sandbox.
    fn line(&self, source: Source, text: &str) {
        let tag = source.tag();
        if let Ok(mut guard) = self.file.lock() {
            let (file, written) = &mut *guard;
            let size = (tag.len() + text.len() + 4) as u64;
            if *written <= MAX_LOG_BYTES {
                if *written + size > MAX_LOG_BYTES {
                    let _ = writeln!(file, "[launcher] log size limit reached; further output is not recorded");
                    *written = MAX_LOG_BYTES + 1;
                } else {
                    let _ = writeln!(file, "[{tag}] {text}");
                    *written += size;
                }
            }
        }
        self.reporter.log(source, text);
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

/// Copy the regular files and directories under `src` to `dst`. Anything else — symbolic
/// links, sockets, devices — is skipped: the source is a directory the game could write to.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path, files: &mut u64, bytes: &mut u64) -> Result<()> {
    const MAX_DEPTH: usize = 64;
    fn walk(
        src: &std::path::Path,
        dst: &std::path::Path,
        depth: usize,
        files: &mut u64,
        bytes: &mut u64,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Error::Other(format!("{} is nested too deeply to copy", src.display())));
        }
        util::create_private_dir_all(dst)?;
        for entry in std::fs::read_dir(src).ctx(|| format!("reading {}", src.display()))? {
            let entry = entry.ctx(|| format!("reading {}", src.display()))?;
            let kind = entry.file_type().ctx(|| format!("examining {}", entry.path().display()))?;
            let (from, to) = (entry.path(), dst.join(entry.file_name()));
            if kind.is_dir() {
                walk(&from, &to, depth + 1, files, bytes)?;
            } else if kind.is_file() {
                // Opened without following a link that replaced the file after the check.
                let mut input = util::open_regular_nofollow(&from).ctx(|| format!("opening {}", from.display()))?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&to)
                    .ctx(|| format!("creating {}", to.display()))?;
                *bytes += std::io::copy(&mut input, &mut output).ctx(|| format!("copying {}", from.display()))?;
                *files += 1;
            }
        }
        Ok(())
    }
    walk(src, dst, 0, files, bytes)
}

/// The game root for one session, plus extracted natives. Removed on drop, whatever happens.
struct Scratch {
    root: PathBuf,
    natives: Option<PathBuf>,
}

impl Scratch {
    fn create(paths: &Paths, instance: &str) -> Result<Self> {
        let base = paths.scratch();
        util::create_private_dir_all(&base)?;
        let root = base.join(format!("{instance}-{}", util::hex_encode(&util::random_bytes::<8>()?)));
        util::create_private_dir_all(&root)?;
        Ok(Scratch { root, natives: None })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // remove_dir_all never follows symlinks; the tree was written by the sandbox.
        let _ = fs::remove_dir_all(&self.root);
        if let Some(n) = &self.natives {
            let _ = fs::remove_dir_all(n);
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
    let swept = crate::running::sweep_leftovers(paths, Duration::from_secs(60));
    if swept > 0 {
        say(&format!("removed {swept} leftover director(ies) of earlier launches that did not end properly"));
    }
    if cfg.launcher.confine == ConfineMode::Require {
        // Known before anything is started: whether the kernel can confine a running,
        // multi-threaded process at all.
        confine::supported().map_err(|why| Error::Sandbox(format!("launcher.confine = \"require\", but {why}")))?;
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
    let display_protection = if desktop.nested.is_some() {
        (Shield::On, "nested compositor: a display server of the game's own, clipboard cut")
    } else if wayland_context.is_some() {
        (Shield::On, "Wayland security context: capture and input injection withheld")
    } else if desktop.uses_x11 {
        (Shield::Off, "the session's X11 display: the game can watch and type into other windows")
    } else {
        (Shield::Partial, "the session's Wayland socket, without a security context")
    };

    // 2–3. Files present and verified.
    let http = http_client(cfg)?;
    if let Some(proxy) = http.socks5() {
        say(&format!(
            "network: the launcher's own connections (sign-in, downloads, credential broker) go through the SOCKS5 \
             proxy at {proxy}; the game's connections to multiplayer servers do not"
        ));
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
    for name in &cfg.sandbox.persist_files {
        // Into the session: re-encoded, so a file damaged on the host does not reach the
        // game's parsers either. A file that does not parse is left where it is and the
        // game starts with its defaults.
        match persist::carry(&inst.dir, &scratch.root, name) {
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
        let (mut files, mut bytes) = (0u64, 0u64);
        for d in &rw_dirs {
            copy_tree(&inst.dir.join(d), &scratch.root.join(d), &mut files, &mut bytes)?;
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
        let host = paths.cache.join(format!("natives-{}", util::hex_encode(&util::random_bytes::<8>()?)));
        util::create_private_dir_all(&host)?;
        scratch.natives = Some(host.clone());
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
    // UDP: the launcher's DNS responder is the only thing a datagram can reach in filtered
    // mode, and nothing at all in offline mode. (Landlock ABI 10; see `restrict_udp`.)
    landlock.udp_send = match mode {
        NetworkMode::Filtered => Some(vec![53]),
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
        drop(next_refresh); // from here on the launcher holds no refresh token
        say(&format!("auth: signed in as {} ({:?} licence)", session.profile_name, session.license));
        // Display-only memory of who this account is; failing to record it changes nothing.
        let identity =
            auth::identity::Identity { player: session.profile_name.clone(), uuid: session.profile_id.clone() };
        let _ = auth::identity::remember(paths, &opts.account, &identity);
        let _ = auth::activity::record(paths, &opts.account, "launch", &inst.id);
        // Did the account change behind our back? Checked before the token goes anywhere.
        match canary_verdict(paths, cfg, &opts.account, &session.profile)? {
            Ok(note) => say(&note),
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
    log.line(Source::Launcher, &format!("jvm argv: {}", argv.join(" ")));
    log.line(
        Source::Launcher,
        &format!(
            "game argv (via launch channel, never on a command line): {main_class} {}",
            game_templates.iter().map(|t| render_redacted(t, &vars)).collect::<Vec<_>>().join(" ")
        ),
    );

    // Resource limits. Asked of the user manager now, so that "it cannot" is known before
    // the game starts rather than discovered as a sandbox that fails to come up.
    let limits = match cfg.sandbox.cgroup {
        CgroupMode::Off => {
            say("limits: none (sandbox.cgroup = \"off\"): the game can use all memory and process slots you can");
            None
        }
        mode => match cgroup::preflight().await {
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
                    "limits: {memory_max} MiB of memory, no swap, {} tasks{cpu} (systemd scope)",
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
            }
            let sink = log.clone();
            let session =
                nested::start(plan, &opts.program, Arc::new(move |line: String| sink.line(Source::Launcher, &line)))
                    .await?;
            session.attach(&mut mounts, &mut landlock.unix_sockets, &mut env);
            log.line(
                Source::Launcher,
                &format!("display: the game's display is :{} inside the nested compositor", session.display),
            );
            Some(session)
        }
        None => None,
    };
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

    // 8. Start; deliver the frame; forget every secret.
    // From here a stop request means "shut the game down cleanly", not "abandon the launch".
    opts.stop.supervise();
    reporter.phase(Phase::Starting);
    {
        let item = |aspect: &str, (shield, detail): (Shield, &str)| Protection {
            aspect: aspect.into(),
            shield,
            detail: detail.into(),
        };
        let confined = cfg.launcher.confine != ConfineMode::Off && confine::supported().is_ok();
        reporter.emit(Event::Protection {
            items: vec![
                item("display", display_protection),
                item(
                    "network",
                    match mode {
                        NetworkMode::Filtered => (Shield::On, "filtered: only allowlisted hosts, through the launcher"),
                        NetworkMode::None => (Shield::On, "offline: no network at all"),
                        NetworkMode::Open => (Shield::Off, "open: the host's network, no egress control"),
                    },
                ),
                item(
                    "credential",
                    if session.is_none() {
                        (Shield::On, "no account and no token in this session")
                    } else if broker_on {
                        (Shield::On, "broker: the game holds a placeholder, the token stays in the launcher")
                    } else if token_in_jvm {
                        (Shield::Partial, "the session token is inside the game process")
                    } else {
                        (Shield::On, "offline: the token is not given to the game")
                    },
                ),
                item(
                    "limits",
                    if spec.limits.is_some() {
                        (Shield::On, "memory, swap and task limits (systemd scope)")
                    } else {
                        (Shield::Off, "no resource limits")
                    },
                ),
                item(
                    "launcher",
                    if confined {
                        (Shield::On, "confines itself while the game runs")
                    } else {
                        (Shield::Partial, "not self-confined")
                    },
                ),
            ],
        });
    }
    let frame = build_frame(&main_class, &game_templates, &vars)?;
    drop(vars);
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
    if let Some(context) = &wayland_context
        && running.payload_pid().await.is_some()
    {
        context.unlink();
    }
    say(&format!("sandbox started; log: {}", log_path.display()));
    reporter.phase(Phase::Running);

    // 9. Supervise.
    let mut outbound_ports = Vec::new();
    let egress = match policy {
        Some(policy) => {
            let spec = policy.netns_spec();
            let sink = log.clone();
            let logger: egress::Logger = Arc::new(move |line: String| sink.line(Source::Egress, &line));
            // SRV records are looked up now, once: afterwards the set of ports the relay
            // connects to is closed, and the launcher can hold itself to it.
            let srv = policy.resolve_srv().await;
            outbound_ports = policy.outbound_ports(&srv);
            let broker = match authority {
                Some(authority) => {
                    let broker =
                        egress::broker::Broker::new(authority, &cfg.network.broker, &http, placeholder, credential)?;
                    log.line(Source::Launcher, &broker.describe());
                    Some(broker)
                }
                None => None,
            };
            Some(egress::start(policy, &spec, std::mem::take(&mut running.egress), http.clone(), logger, srv, broker)?)
        }
        None => None,
    };
    let out = tokio::spawn(relay_output(running.child.stdout.take().expect("piped"), Source::Game, log.clone()));
    let err = tokio::spawn(relay_output(running.child.stderr.take().expect("piped"), Source::Game, log.clone()));

    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .ctx(|| "installing SIGINT handler")?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .ctx(|| "installing SIGTERM handler")?;
    // From here on this process is a supervisor and a relay. Whatever else a user can do —
    // read the home directory, run programs, connect anywhere — it gives up, on every
    // thread, so that a bug in handling the game's traffic cannot be turned into that.
    match cfg.launcher.confine {
        ConfineMode::Off => log.line(Source::Launcher, "launcher: not confined (launcher.confine = \"off\")"),
        mode => {
            let mut keep = Confinement::supervisor();
            keep.write.push(inst.dir.clone()); // settings carried back at exit
            keep.write.push(paths.cache.clone()); // scratch and unpacked natives, removed at exit
            keep.tcp_connect.extend(outbound_ports);
            // What the broker forwards to Mojang goes through the proxy, if one is set.
            keep.tcp_connect.extend(http.socks5().map(|proxy| proxy.port()));
            match confine::apply(&keep) {
                Ok(()) => log.line(
                    Source::Launcher,
                    &format!(
                        "launcher: confined itself (Landlock, all threads): no program execution, files only under \
                         the instance and the cache, TCP only to port(s) {}",
                        keep.tcp_connect.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
                    ),
                ),
                Err(e) if mode == ConfineMode::Require => {
                    let _ = running.stop(Duration::from_secs(5)).await;
                    return Err(e);
                }
                Err(e) => log.line(Source::Launcher, &format!("launcher: not confined — {e}")),
            }
        }
    }

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
        e.shutdown();
        log.line(
            Source::Launcher,
            &format!(
                "egress summary: {relayed} connection(s) relayed, {denied} denied, {refused} name lookup(s) refused"
            ),
        );
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

    // 10. Settings the launcher carries back on the game's behalf. (Not from a burner
    // session: nothing it wrote is kept.)
    if opts.burner {
        log.line(
            Source::Launcher,
            "burner session: worlds, settings and everything else the game wrote are deleted now",
        );
    }
    for name in cfg.sandbox.persist_files.iter().filter(|_| !opts.burner) {
        // Back out: what the game left is untrusted. It is parsed and written out again;
        // what does not parse stays in the scratch directory and is deleted with it.
        match persist::carry(&scratch.root, &inst.dir, name) {
            Ok(Some(report)) => {
                if let Some(note) = report.describe(name) {
                    log.line(Source::Launcher, &format!("settings: {note}"));
                }
            }
            Ok(None) => {}
            Err(e) => log.line(Source::Launcher, &format!("not persisting {name}: {e}")),
        }
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

/// Compare the profile just returned by Mojang with the pinned one.
///
/// The outer `Result` is a failure to check at all. Inside: a note for the log, or the
/// refusal that stops the launch.
pub fn canary_verdict(
    paths: &Paths,
    cfg: &Config,
    account: &str,
    profile: &auth::minecraft::Profile,
) -> Result<std::result::Result<String, Error>> {
    use crate::{auth::canary, config::CanaryMode};
    if cfg.auth.canary == CanaryMode::Off {
        return Ok(Ok("canary: off (auth.canary = \"off\")".into()));
    }
    let checked = match canary::check(paths, account, profile) {
        Ok(checked) => checked,
        Err(damaged) if cfg.auth.canary == CanaryMode::Enforce => return Ok(Err(damaged)),
        Err(damaged) => return Ok(Ok(format!("canary: WARNING: {damaged}"))),
    };
    Ok(match checked {
        canary::Check::First => {
            canary::accept(paths, account, profile)?;
            Ok(format!("canary: pinned this account's profile for the first time (player {})", profile.name))
        }
        canary::Check::Unchanged => {
            // Records capes gained since; a no-op otherwise.
            canary::accept(paths, account, profile)?;
            Ok("canary: profile unchanged since it was pinned (name, skin, cape)".into())
        }
        canary::Check::Changed(differences) if cfg.auth.canary == CanaryMode::Enforce => {
            Err(canary::refusal(account, &differences))
        }
        canary::Check::Changed(differences) => Ok(format!(
            "canary: WARNING: the profile changed outside this launcher ({}); auth.canary = \"warn\", so the \
             launch continues. `bastion-mc canary accept` pins the new state.",
            differences.join("; ")
        )),
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
        assert!(first.contains("first time"), "{first}");
        let same = canary_verdict(&paths, &cfg, "default", &profile("Notch")).unwrap().unwrap();
        assert!(same.contains("unchanged"), "{same}");

        let refusal = canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().unwrap_err();
        assert!(matches!(refusal, Error::Policy(_)));
        let text = refusal.to_string();
        assert!(text.contains("Notch → Stolen") && text.contains("canary accept --account default"), "{text}");
        // Refusing does not move the pin: the next launch is refused as well.
        assert!(canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().is_err());

        cfg.auth.canary = CanaryMode::Warn;
        let warned = canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().unwrap();
        assert!(warned.contains("WARNING") && warned.contains("Notch → Stolen"), "{warned}");
        cfg.auth.canary = CanaryMode::Enforce;
        assert!(
            canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().is_err(),
            "a warning pins nothing"
        );

        cfg.auth.canary = CanaryMode::Off;
        assert!(canary_verdict(&paths, &cfg, "default", &profile("Stolen")).unwrap().unwrap().contains("off"));

        // A damaged record stops an enforcing launch too.
        cfg.auth.canary = CanaryMode::Enforce;
        std::fs::write(paths.canaries().join("default.json"), "{}").unwrap();
        assert!(canary_verdict(&paths, &cfg, "default", &profile("Notch")).unwrap().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;

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

#[cfg(test)]
mod burner_tests {
    use super::*;

    #[test]
    fn a_burner_copy_takes_files_and_directories_and_nothing_else() {
        let root = std::env::temp_dir()
            .join(format!("bastion-burn-{}", crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())));
        let (src, dst) = (root.join("saves"), root.join("copy"));
        std::fs::create_dir_all(src.join("World/region")).unwrap();
        std::fs::write(src.join("World/level.dat"), b"level").unwrap();
        std::fs::write(src.join("World/region/r.0.0.mca"), vec![7u8; 5000]).unwrap();
        std::fs::write(root.join("outside-secret"), b"SECRET").unwrap();
        std::os::unix::fs::symlink(root.join("outside-secret"), src.join("World/link-to-outside")).unwrap();
        std::os::unix::fs::symlink("/etc", src.join("etc-link")).unwrap();
        nix::unistd::mkfifo(&src.join("World/fifo"), nix::sys::stat::Mode::S_IRWXU).unwrap();

        let (mut files, mut bytes) = (0, 0);
        copy_tree(&src, &dst, &mut files, &mut bytes).unwrap();
        assert_eq!((files, bytes), (2, 5005));
        assert_eq!(std::fs::read(dst.join("World/level.dat")).unwrap(), b"level");
        assert_eq!(std::fs::read(dst.join("World/region/r.0.0.mca")).unwrap().len(), 5000);
        for skipped in ["World/link-to-outside", "etc-link", "World/fifo"] {
            assert!(std::fs::symlink_metadata(dst.join(skipped)).is_err(), "{skipped} was copied");
        }
        // The copy is independent of the original.
        std::fs::write(dst.join("World/level.dat"), b"changed by the game").unwrap();
        assert_eq!(std::fs::read(src.join("World/level.dat")).unwrap(), b"level");
        std::fs::remove_dir_all(root).unwrap();
    }
}
