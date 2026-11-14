//! A Bedrock launch, end to end, with a stand-in for `mcpelauncher-client`
//! (`tests/support/bedrock_standin.rs`, compiled here with `rustc`): the real bubblewrap →
//! init (Landlock + the client's seccomp profile) chain, the relay, the data split, the stub.
//!
//! Three sessions of one instance, in order:
//!
//! 1. filtered network, on a version imported into the store and opened by `store::open` (as
//!    a real launch does): the stand-in writes a world, settings and sign-in tokens, fetches
//!    HTTPS from a stand-in host through the relay (libcurl, the sandbox's CA bundle with the
//!    stand-in's CA appended after the network package's), runs the webview the way the client
//!    does, and tries what must fail; after the session the version can be removed again (the
//!    session held it);
//! 2. `[network] mode = "none"`, after a crashed session was simulated: the leftover is
//!    deleted before the start, the world and settings are still there, the tokens are not;
//! 3. filtered again, with the launcher confining itself — which confines this test process
//!    too, so this is the only test in the file and the last thing it does.
//!
//! Needs bubblewrap, unprivileged user namespaces and Landlock (ABI ≥ 6), like
//! `tests/sandbox.rs`. No display (the desktop is given as empty), no systemd scope
//! (`cgroup = "off"`), no network beyond loopback. `BASTION_TEST_RUNTIME` names the directory
//! for the bridge socket's short path (default: `$XDG_RUNTIME_DIR`).

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use bastion_mc::{
    bedrock::{
        launch::{Setup, launch_with, profile},
        policy,
        runtime::Client,
        store::{self, OpenedVersion, open_dir_unverified},
    },
    config::{CgroupMode, Config, ConfineMode},
    events::{Reporter, StopSignal},
    instance,
    launch::LaunchOptions,
    net::tls,
    paths::Paths,
    sandbox::display::Desktop,
    util,
};
use bastion_mc_proto::{Event, Shield, Source};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    ServerConfig,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;

const INSTANCE: &str = "bedrock-test";
const HOST: &str = "client.discovery.minecraft-services.net";
const BODY: &str = "standin-ok";

/// The stand-in client, compiled once per source text.
fn standin(tmp: &Path) -> PathBuf {
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/bedrock_standin.rs");
    let text = std::fs::read(source).expect("stand-in source");
    let tag = util::hex_encode(&util::hash_bytes(util::HashAlg::Sha256, &text)[..8]);
    let out = tmp.join(format!("bedrock-standin-{tag}"));
    if !out.exists() {
        let rustc = std::env::var_os("RUSTC")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO")).with_file_name("rustc"));
        let rustc = if rustc.exists() { rustc } else { PathBuf::from("rustc") };
        let partial = out.with_extension("partial");
        let status = std::process::Command::new(rustc)
            .args(["--edition", "2024", "-C", "opt-level=1", "-C", "debuginfo=0", "-o"])
            .arg(&partial)
            .arg(source)
            .status()
            .expect("running rustc");
        assert!(status.success(), "compiling the stand-in client failed");
        std::fs::rename(&partial, &out).unwrap();
    }
    out
}

/// A CA and a server certificate for [`HOST`], signed by it.
fn authority() -> (String, ServerConfig) {
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name.push(DnType::CommonName, "bastion-mc stand-in CA");
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    let ca_key = KeyPair::generate().unwrap();
    let ca_der = ca.self_signed(&ca_key).unwrap().der().to_vec();
    let body: Vec<String> =
        util::b64_encode(&ca_der).as_bytes().chunks(64).map(|l| String::from_utf8(l.to_vec()).unwrap()).collect();
    let ca_pem = format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", body.join("\n"));
    let mut leaf = CertificateParams::new(vec![HOST.to_string()]).unwrap();
    leaf.distinguished_name.push(DnType::CommonName, HOST);
    let leaf_key = KeyPair::generate().unwrap();
    let leaf_der = leaf.signed_by(&leaf_key, &Issuer::from_params(&ca, &ca_key)).unwrap().der().clone();
    let config = ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf_der], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())))
        .unwrap();
    (ca_pem, config)
}

/// A SOCKS5 endpoint (what `network.launcher_proxy` names) that serves [`HOST`] itself: it
/// takes the CONNECT, then terminates TLS on the same stream and answers one HTTP request.
/// No outbound connection — so it keeps working after the launcher, which is this process,
/// has confined itself.
async fn socks5_host(config: ServerConfig) -> (SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let asked: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let log = asked.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else { break };
            let (acceptor, log) = (acceptor.clone(), log.clone());
            tokio::spawn(async move {
                let mut greeting = [0u8; 3];
                if client.read_exact(&mut greeting).await.is_err() || greeting != [5, 1, 0] {
                    return;
                }
                let _ = client.write_all(&[5, 0]).await;
                let mut head = [0u8; 5];
                if client.read_exact(&mut head).await.is_err() || head[..4] != [5, 1, 0, 3] {
                    return;
                }
                let mut name = vec![0u8; head[4] as usize + 2];
                if client.read_exact(&mut name).await.is_err() {
                    return;
                }
                name.truncate(name.len() - 2);
                log.lock().unwrap().push(String::from_utf8_lossy(&name).into_owned());
                let _ = client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 1, 187]).await;
                let Ok(mut tls) = acceptor.accept(client).await else { return };
                let mut request = [0u8; 4096];
                let _ = tls.read(&mut request).await;
                let reply =
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}", BODY.len());
                let _ = tls.write_all(reply.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, asked)
}

struct World {
    root: PathBuf,
    paths: Paths,
    client: Client,
    version: PathBuf,
    ca_pem: String,
    proxy: SocketAddr,
}

impl World {
    fn config(&self, confine: ConfineMode) -> Config {
        let mut cfg = Config::default();
        cfg.bedrock.accept_licence_substitution = true;
        cfg.sandbox.cgroup = CgroupMode::Off;
        cfg.launcher.confine = confine;
        cfg.network.launcher_proxy = Some(format!("socks5h://{}", self.proxy));
        cfg.validate().unwrap();
        cfg
    }

    /// What the stand-in is to do and look at in session `number`.
    fn probes(&self, number: u32) -> String {
        [
            format!("session={number}"),
            format!("host_path={}", self.paths.data.display()),
            format!("host_path={}", self.paths.config.display()),
            format!("host_path={}", self.paths.state.display()),
            format!("host_path={}", self.paths.data.join("vault").display()),
            format!("host_path={}", self.root.join("instance-dir-marker").display()),
            format!("https=https://{HOST}/standin"),
            "denied=https://api.minecraftservices.com/minecraft/profile".to_string(),
        ]
        .join("\n")
    }

    /// Run one session on the test's own version directory (opened without the store's check).
    async fn session(&self, number: u32, confine: ConfineMode) -> Session {
        std::fs::write(self.version.join("standin-probes.txt"), self.probes(number)).unwrap();
        let version = open_dir_unverified("1.26.62.1-standin", &self.version).unwrap();
        self.session_with(version, confine).await
    }

    /// Run one session on `version`; the game's `key=value` lines, the status lines, every event.
    async fn session_with(&self, version: OpenedVersion, confine: ConfineMode) -> Session {
        let reporter = Reporter::capture();
        let opts = LaunchOptions {
            instance: INSTANCE.into(),
            account: "default".into(),
            demo: false,
            burner: false,
            join: None,
            repair: false,
            stop: StopSignal::never(),
            program: env!("CARGO_BIN_EXE_bastion-mc").into(),
        };
        let inst = instance::load(&self.paths, INSTANCE).expect("instance");
        let setup = Setup {
            client: self.client.clone(),
            version,
            desktop: Some(Desktop::default()),
            extra_trust_pem: self.ca_pem.clone().into_bytes(),
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(120),
            launch_with(&self.paths, &self.config(confine), &opts, inst, setup, &reporter),
        )
        .await
        .expect("the session did not end");
        let events = reporter.captured();
        let mut game = BTreeMap::new();
        let mut lines = Vec::new();
        for event in &events {
            match event {
                Event::Log { source: Source::Game, line } => {
                    if let Some((k, v)) = line.split_once('=') {
                        game.insert(k.to_string(), v.to_string());
                    }
                    lines.push(format!("game     | {line}"));
                }
                Event::Log { source, line } => lines.push(format!("{:<8} | {line}", source.tag())),
                Event::Status { text } => lines.push(format!("status   | {text}")),
                _ => {}
            }
        }
        Session { outcome: outcome.map_err(|e| e.to_string()), game, lines, events }
    }
}

struct Session {
    outcome: Result<i32, String>,
    game: BTreeMap<String, String>,
    lines: Vec<String>,
    events: Vec<Event>,
}

impl Session {
    fn get(&self, key: &str) -> &str {
        self.game
            .get(key)
            .map(String::as_str)
            .unwrap_or_else(|| panic!("no `{key}=` from the stand-in\n{}", self.dump()))
    }
    fn expect(&self, key: &str, want: &str) {
        assert_eq!(self.get(key), want, "{key}\n{}", self.dump());
    }
    fn refused(&self, key: &str) {
        let got = self.get(key);
        assert_ne!(got, "ok", "{key} must fail\n{}", self.dump());
    }
    fn said(&self, needle: &str) -> bool {
        self.lines.iter().any(|l| l.contains(needle))
    }
    fn dump(&self) -> String {
        format!("outcome: {:?}\n{}", self.outcome, self.lines.join("\n"))
    }
    fn protection(&self) -> BTreeMap<String, (Shield, String)> {
        self.events
            .iter()
            .find_map(|e| match e {
                Event::Protection { items } => {
                    Some(items.iter().map(|i| (i.aspect.clone(), (i.shield, i.detail.clone()))).collect())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no protection event\n{}", self.dump()))
    }
}

fn files_named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_name() == name {
                found.push(path.clone());
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            }
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bedrock_session_keeps_its_worlds_loses_its_tokens_and_reaches_only_what_it_may() {
    if bastion_mc::sandbox::check_bwrap().is_err() || bastion_mc::sandbox::landlock::kernel_abi() < 6 {
        eprintln!("skipped: needs bubblewrap and Landlock ABI >= 6");
        return;
    }
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    // Earlier runs end confined (session 3) and cannot clean up after themselves.
    for entry in std::fs::read_dir(&tmp).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with("bedrock-launch-") {
            let _ = bastion_mc::tree::remove(&entry.path(), None);
        }
    }
    let runtime_base = std::env::var_os("BASTION_TEST_RUNTIME")
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    for entry in std::fs::read_dir(&runtime_base).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with("bmc-bedrock-") {
            let _ = bastion_mc::tree::remove(&entry.path(), None);
        }
    }
    let tag = util::hex_encode(&util::random_bytes::<6>().unwrap());
    let root = tmp.join(format!("bedrock-launch-{tag}"));
    let runtime = runtime_base.join(format!("bmc-bedrock-{tag}"));
    util::create_private_dir_all(&runtime).unwrap();
    let paths = Paths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        state: root.join("state"),
        runtime: Some(runtime.clone()),
    };
    paths.ensure().unwrap();
    // Something of the launcher's own that the game must not see.
    util::create_private_dir_all(&paths.data.join("vault")).unwrap();
    std::fs::write(paths.data.join("vault").join("default.age"), b"not for the game").unwrap();

    // The client: the stand-in at <client>/bin/mcpelauncher-client, an "installed webview" next
    // to it (the same program, which under that name says so loudly), a library directory.
    let program = standin(&tmp);
    let client_dir = root.join("client");
    std::fs::create_dir_all(client_dir.join("bin")).unwrap();
    std::fs::create_dir_all(client_dir.join("lib")).unwrap();
    std::fs::copy(&program, client_dir.join("bin/mcpelauncher-client")).unwrap();
    std::fs::copy(&program, client_dir.join("bin/mcpelauncher-webview")).unwrap();
    std::fs::write(client_dir.join("lib/libstandin.txt"), b"lib").unwrap();
    let client = Client {
        program: client_dir.join("bin/mcpelauncher-client"),
        lib_dirs: vec![client_dir.join("lib")],
        webview_paths: vec![
            client_dir.join("bin/mcpelauncher-webview"),
            PathBuf::from("/nonexistent/mcpelauncher-webview"),
        ],
        version: Some("stand-in".into()),
    };
    // The version: what an imported game directory has where the client looks.
    let version = root.join("version");
    std::fs::create_dir_all(version.join("lib/x86_64")).unwrap();
    std::fs::write(version.join("lib/x86_64/libminecraftpe.so"), vec![0x7f; 4096]).unwrap();
    std::fs::write(version.join("AndroidManifest.xml"), b"stand-in").unwrap();

    let (ca_pem, server) = authority();
    let (proxy, asked) = socks5_host(server).await;
    let world = World { root: root.clone(), paths: paths.clone(), client, version, ca_pem, proxy };
    instance::create_bedrock(&paths, INSTANCE, "1.26.62.1").unwrap();
    let inst_dir = paths.instance(INSTANCE).unwrap();
    let kept = inst_dir.join(bastion_mc::bedrock::launch::instance::DATA_DIR);
    let curl = Path::new("/usr/lib/libcurl.so.4").exists();

    // ---------------------------------------------------------------- session 1
    // A version as the store keeps it: written as mcpelauncher-ui extracts one (with the
    // stand-in's instructions among its assets), imported, and opened with its check — the
    // instance names it ("1.26.62.1").
    let source = root.join("download");
    store::write_stand_in_version(&source).unwrap();
    std::fs::write(source.join("assets/standin-probes.txt"), world.probes(1)).unwrap();
    store::import(&paths, &source, None, &Reporter::capture()).expect("importing the stand-in version");
    let library = std::fs::metadata(source.join("lib/x86_64/libminecraftpe.so")).unwrap().len();
    let one =
        world.session_with(store::open(&paths, "1.26.62.1").expect("the imported version"), ConfineMode::Off).await;
    if std::env::var_os("BASTION_TEST_VERBOSE").is_some() {
        println!("{}", one.dump());
    }
    assert_eq!(one.outcome, Ok(0), "{}", one.dump());
    one.expect("argv", "-dg /bastion/game -dd /bastion/data -dc /bastion/cache");
    one.expect("game.read", &library.to_string());
    one.refused("game.write");
    // The data root is writable; the mod directory is not; nothing written can be executed.
    for key in ["write.world", "write.options", "write.settings", "write.token.external", "write.token.root"] {
        one.expect(key, "ok");
    }
    one.expect("write.planted_lib", "ok");
    one.refused("write.planted_mod");
    one.expect("mods.entries", "0");
    one.expect("written_exe.made", "ok");
    one.expect("written_exe.exec", "errno13");
    // The webview: found where the client looks first (its own directory), and it is the stub.
    one.expect("webview.bin_same_as_init", "true");
    one.expect("webview.installed_present", "true");
    one.expect("webview.installed_same_as_init", "true");
    one.expect("webview.path", &client_dir.join("bin/mcpelauncher-webview").display().to_string());
    one.expect("webview.same_as_init", "true");
    one.expect("webview.fake_ran", "false");
    // Nothing of the host, no keyring, no namespaces.
    one.refused("home");
    for path in [&paths.data, &paths.config, &paths.state, &paths.data.join("vault"), &root.join("instance-dir-marker")]
    {
        one.expect(&format!("host_path[{}]", path.display()), "errno2");
    }
    one.refused("session_bus");
    one.refused("x11.path");
    one.refused("x11.abstract");
    for key in ["keyctl", "add_key", "unshare.user", "unshare.net", "clone.newuser"] {
        one.expect(key, "errno38");
    }
    // The network: the bridge is there; a name that is not allowlisted does not resolve; HTTPS
    // to an allowlisted name goes through the relay and verifies against the appended CA.
    one.expect("bridge.present", "true");
    let bridge = one.get("bridge.connect");
    assert!(bridge == "ok" || bridge == "errno111", "the bridge socket is reachable from the sandbox: {bridge}");
    one.refused("dns.unlisted");
    if curl {
        one.expect("https.code", "200");
        one.expect("https.body", BODY);
        assert!(one.get("denied.error").contains("resolve"), "{}", one.dump());
        assert!(asked.lock().unwrap().iter().any(|n| n == HOST), "the relay went through the proxy for {HOST}");
    } else {
        one.expect("https.error", "no libcurl");
    }
    assert!(one.said("auth: none by the launcher"), "{}", one.dump());
    assert!(one.said("this session's data root has been deleted"), "{}", one.dump());
    // On the host: the world and the settings are kept; no sign-in material is; no session
    // directory and no bridge socket name are left.
    let world_file = kept.join("games/com.mojang/minecraftWorlds/standin-world/level.dat");
    assert_eq!(std::fs::read_to_string(&world_file).unwrap(), "world-from-session-1");
    assert_eq!(
        std::fs::read_to_string(kept.join("games/com.mojang/minecraftpe/options.txt")).unwrap(),
        "gfx_viewdistance:8"
    );
    assert_eq!(std::fs::read_to_string(kept.join(profile::CLIENT_SETTINGS)).unwrap(), "fullscreen=false");
    assert!(files_named(&kept, "xal").is_empty() && files_named(&kept, "token.json").is_empty());
    assert!(files_named(&kept, "libjnivmsupport.so").is_empty());
    let sessions = || std::fs::read_dir(policy::sessions_dir(&paths)).map(|d| d.count()).unwrap_or(0);
    assert_eq!(sessions(), 0, "no session directory is left");
    assert!(files_named(&runtime, "signin.sock").is_empty(), "the bridge socket's name is gone");
    // The session held the version until it ended; now it can be removed (and the instance
    // that names it is named).
    let notes = store::remove(&paths, "1.26.62.1").expect("the version is free once the session is over");
    assert!(notes.iter().any(|n| n.contains(INSTANCE)), "{notes:?}");

    // ---------------------------------------------------------------- session 2
    // A session that crashed a moment ago left its data root (and its sign-in) behind, where
    // sessions keep it (`policy::sessions_dir`).
    let leftover = policy::sessions_dir(&paths).join(format!("{INSTANCE}-0123456789abcdef"));
    std::fs::create_dir_all(leftover.join("xal")).unwrap();
    std::fs::write(leftover.join("xal/token.json"), b"{}").unwrap();
    let toml = inst_dir.join("instance.toml");
    let original = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, original.replace("# mode = \"filtered\"", "mode = \"none\"")).unwrap();
    let two = world.session(2, ConfineMode::Off).await;
    if std::env::var_os("BASTION_TEST_VERBOSE").is_some() {
        println!("{}", two.dump());
    }
    assert_eq!(two.outcome, Ok(0), "{}", two.dump());
    assert!(two.said("deleted the data of 1 earlier Bedrock session(s)"), "{}", two.dump());
    assert!(!leftover.exists());
    two.expect("world.present", "true");
    two.expect("options.present", "true");
    two.expect("settings.content", "fullscreen=false");
    two.expect("token.external.present", "false");
    two.expect("token.root.present", "false");
    two.expect("planted_lib.present", "false");
    two.expect("mods.entries", "0");
    two.expect("bridge.present", "false");
    two.refused("dns.unlisted");
    assert!(two.game.contains_key("https.error"), "no network in mode none\n{}", two.dump());
    let shields = two.protection();
    assert_eq!(shields["network"], (Shield::On, "offline: no network at all".to_string()));
    assert_eq!(shields["credential"].0, Shield::On, "{shields:?}");
    std::fs::write(&toml, original).unwrap();

    // ---------------------------------------------------------------- session 3 (confines this process)
    let three = world.session(3, ConfineMode::Auto).await;
    assert_eq!(three.outcome, Ok(0), "{}", three.dump());
    three.expect("world.present", "true");
    three.expect("token.root.present", "false");
    if curl {
        three.expect("https.code", "200");
    }
    let shields = three.protection();
    let (shield, detail) = &shields["credential"];
    assert_eq!(*shield, Shield::Partial, "the game holds an Xbox sign-in of its own: {detail}");
    assert!(detail.contains("Xbox") && detail.contains("no Java token"), "{detail}");
    assert_eq!(shields["network"].0, Shield::On, "{shields:?}");
    assert!(shields["network"].1.starts_with("filtered"), "{shields:?}");
    assert_eq!(shields["sign-in"].0, Shield::Off, "no bridge server in this build: {shields:?}");
    assert_ne!(shields["launcher"].0, Shield::Off, "the launcher confined itself: {shields:?}");
    assert!(three.said("launcher: confined itself") || three.said("only PARTLY confined"), "{}", three.dump());
    for line in three.lines.iter().filter(|l| l.contains("launcher:") || l.contains("data:")) {
        println!("{line}");
    }
}
