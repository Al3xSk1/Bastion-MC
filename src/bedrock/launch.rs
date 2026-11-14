//! The launch of a Bedrock instance: the user's installed `mcpelauncher-client` running the
//! Android build of the game in bastion-mc's sandbox (contract: `docs/notes/round5-contract.md`).
//!
//! `launch::launch` hands a Bedrock instance to [`launch`] right after loading it. What follows
//! keeps the order of the Java launch where the two share a reason — above all: everything the
//! session needs is made before the launcher confines itself (`launch`'s step 9), and nothing
//! the game sends is read before that.
//!
//!  1. the gate first (`policy::gate`: the licence substitution accepted), then the installed
//!     client (`runtime::discover`, the store package's) and the imported version opened by
//!     descriptor and checked against its manifest (`store::open`; the `OpenedVersion` is
//!     held for the whole session, so `bedrock remove` refuses meanwhile); the session's marker
//!     (`running::Marker::bedrock`)
//!  2. the instance's data: its lock; what killed or crashed sessions left in the directory of
//!     Bedrock sessions (deleted — it holds their sign-in: `policy::remove_leftovers`); a fresh
//!     data root for this session (`policy::SessionDir`) with the kept worlds and settings bound
//!     into it (`profile`)
//!  3. network (`filtered`, the default, or `none`; never `open`): the network package's part
//!     prepared ([`net::prepare`]: the namespace's sockets, Landlock's ports, the per-launch
//!     authority for the trust store); display, audio, GPU, limits
//!  4. **no sign-in by the launcher.** No Microsoft or Minecraft token is fetched, read or held
//!     in a Bedrock session; neither the token store nor the vault is opened. The game signs
//!     in to Xbox itself, through the sign-in bridge, outside its sandbox
//!  5. the bridge's socket (`signin::Bridge`), the nested compositor — before step 9
//!  8. the sandbox starts (the client, its arguments, an environment built from scratch); the
//!     bridge socket's name on the host is removed once the sandbox holds it
//!  9. the limits are read back from the kernel, the launcher confines itself
//! 10. the sign-in bridge is served (`signin::serve`), the protections this session has are
//!     reported, the relay starts (`Prepared::start`) and with it the Realms answers from the
//!     trusted input (`realms::answers`); the log until the game exits
//! 11. the session's data root is deleted — with it the game's sign-in — and that is said only
//!     if it is true; then what to know about capes and the canary, when the game was signed in
//!     (`policy::report_after_session`)
//!
//! What is deliberately not done: no Java credential of any kind, no `--burner`, `--join` or
//! `--repair` (each refused with what to do instead; `--demo` changes nothing and says so), no
//! native mods, no `open` network.

pub mod instance;
pub mod profile;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use rustls::pki_types::{CertificateDer, pem::PemObject};

use crate::{
    bedrock::{
        SeccompMode as BedrockSeccomp, net, policy, runtime,
        runtime::Client,
        signin,
        store::{self, OpenedVersion},
    },
    config::{CgroupMode, Config, ConfineMode, NetworkMode, SeccompMode},
    confine::{self, Confinement},
    egress,
    error::{Context, Error, Result},
    events::{AnswerKind, Event, Phase, Protection, Reporter, Shield, Source},
    instance::{Instance, Kind},
    launch::{LaunchOptions, Log, realms_announcer, relay_output, shields},
    net::tls,
    paths::Paths,
    realms::{self, Approve},
    sandbox::{
        self, BaseOptions, Network, Resolv, SandboxSpec, cgroup,
        display::{self, Desktop},
        nested,
        policy::{InitPolicy, SeccompPolicy, SeccompProfile, TcpPolicy},
        trust::TrustStore,
    },
};

const SIGSYS_EXIT: i32 = 128 + libc::SIGSYS;
/// The memory ceiling of a Bedrock session when `[sandbox] memory_max_mb` is not set, before
/// the tmpfs sizes are added: a bound on runaway use, not a tuning knob. (The Android build
/// typically stays well below it; it has never been measured on this machine.)
const MEMORY_MAX_MB: u64 = 6144;
/// The nested compositor's own scope, as for Java (`launch`).
const NESTED_MEMORY_MAX_MB: u64 = 2048;
const NESTED_TASKS_MAX: u32 = 1024;
const NESTED_CPU_QUOTA_PERCENT: u32 = 200;
/// Most CA certificates a test may add to the sandbox's trust store ([`Setup::extra_trust_pem`]).
const MAX_EXTRA_CAS: usize = 8;

/// Why a Bedrock session never gets the host's network.
const OPEN_REFUSED: &str = "network mode \"open\" is refused for a Bedrock instance: the game holds the account's Xbox \
     sign-in, and only the filtered relay keeps it from reaching anything but the Bedrock hosts and the servers you \
     listed. Set `[network] mode = \"filtered\"` (the default) or `\"none\"` (offline single-player) in the \
     instance's instance.toml";

/// What a Bedrock launch takes from the store and runtime packages, and what a test can put
/// in their place. Built only by code — never from a configuration file.
#[doc(hidden)]
pub struct Setup {
    pub client: Client,
    pub version: OpenedVersion,
    /// The desktop the game gets. `None`: decided as for Java (`display::plan`).
    pub desktop: Option<Desktop>,
    /// CA certificates (PEM) a test's stand-in hosts are signed with, put into the sandbox's
    /// trust store after the network package's per-launch authority. Empty in every real
    /// launch ([`launch`]); parsed strictly and bounded all the same.
    pub extra_trust_pem: Vec<u8>,
}

/// Launch a Bedrock instance (called by `launch::launch` once it has loaded `inst`).
pub async fn launch(
    paths: &Paths,
    cfg: &Config,
    opts: &LaunchOptions,
    inst: Instance,
    reporter: &Reporter,
) -> Result<i32> {
    // The gate first: without the licence substitution accepted, nothing of Bedrock is looked at.
    policy::gate(&cfg.bedrock)?;
    let Kind::Bedrock(settings) = &inst.kind else {
        return Err(Error::Usage(format!("{} is not a Bedrock instance", inst.id)));
    };
    refuse_java_options(opts, &inst.id)?;
    let client = runtime::discover(&cfg.bedrock)?;
    let version = store::open(paths, &settings.version)?;
    launch_with(paths, cfg, opts, inst, Setup { client, version, desktop: None, extra_trust_pem: Vec::new() }, reporter)
        .await
}

/// The options of `bastion-mc launch` that are Java's. `--demo` asks for what a Bedrock
/// launch is anyway — nobody signed in by the launcher, no token — so it is accepted (the
/// front end passes it whenever its demo switch is on) and said to change nothing; the others
/// would change what happens and are refused with what to do instead.
fn refuse_java_options(opts: &LaunchOptions, id: &str) -> Result<()> {
    let refuse = |what: &str| Err(Error::Usage(format!("{id} is a Bedrock instance: {what}")));
    if opts.burner {
        return refuse(
            "--burner is a Java feature. For an offline Bedrock session that keeps nothing, set `[network] mode = \
             \"none\"` in the instance and `[bedrock] persist_worlds = false` in config.toml",
        );
    }
    if opts.join.is_some() {
        return refuse(
            "--join is not available for Bedrock. List the server under [bedrock] servers (config.toml, or the \
             instance's instance.toml) and pick it in the game's server list",
        );
    }
    if opts.repair {
        return refuse(
            "--repair downloads Java game files again. A Bedrock version is checked against its manifest at every \
             launch; `bastion-mc bedrock import` imports it again",
        );
    }
    Ok(())
}

/// What `--demo` means for a Bedrock launch: nothing that is not so anyway.
const DEMO_NOTE: &str = "--demo changes nothing for Bedrock: the launcher signs nobody in and no Java token exists in \
                         any Bedrock session; the game itself is started as it is (no trial mode)";

/// [`launch`] with the client, the version and (for tests) the desktop and the network
/// package's part already in hand.
#[doc(hidden)]
pub async fn launch_with(
    paths: &Paths,
    cfg: &Config,
    opts: &LaunchOptions,
    inst: Instance,
    mut setup: Setup,
    reporter: &Reporter,
) -> Result<i32> {
    let say = |m: &str| reporter.status(m);
    reporter.phase(Phase::Preparing);
    policy::gate(&cfg.bedrock)?;
    let Kind::Bedrock(settings) = &inst.kind else {
        return Err(Error::Usage(format!("{} is not a Bedrock instance", inst.id)));
    };
    refuse_java_options(opts, &inst.id)?;
    if opts.demo {
        say(DEMO_NOTE);
    }
    let client = setup.client.clone();
    say(&format!(
        "instance {:?}: Minecraft Bedrock {} (the Android build), run by mcpelauncher-client{} at {}",
        inst.id,
        setup.version.id,
        client.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default(),
        client.program.display()
    ));
    // So that `bastion-mc scram` can find and stop this launch — as a Bedrock session, whose
    // game holds the account's Xbox sign-in (scram says so when it cannot stop one). Made before
    // anything of the session exists, dropped after its data directory is gone.
    let _running = crate::running::Marker::bedrock(paths, &inst.id)?;
    let swept =
        crate::running::sweep_leftovers(paths, Duration::from_secs(60), Some(Instant::now() + Duration::from_secs(20)));
    if swept.removed > 0 {
        say(&format!("removed {} leftover director(ies) of earlier launches that did not end properly", swept.removed));
    }
    for failure in &swept.failed {
        reporter.warn(format!(
            "WARNING: a directory an earlier launch left in {} could not be removed ({failure}). The next launch \
             tries again.",
            paths.scratch().display()
        ));
    }
    if cfg.launcher.confine == ConfineMode::Require
        && let confine::Support::Partly(gaps) = confine::support()
    {
        return Err(Error::Sandbox(format!("launcher.confine = \"require\", but {}", gaps.join("; "))));
    }

    // 3 (decided before anything is created). The network mode.
    let mode = network_mode(cfg.network.mode, inst.config.network.mode)?;
    // The system-call filter, built once here: a bad `[bedrock] extra_syscalls` ends the
    // launch now, with the configuration's words, rather than inside the sandbox.
    let seccomp = seccomp_policy(cfg);
    sandbox::seccomp::build(&seccomp)?;
    // The desktop: a refusal (X11 without consent, a sound server that loads modules) comes
    // before anything is created.
    let mut desktop = match setup.desktop.take() {
        Some(desktop) => desktop,
        None => display::plan(&cfg.sandbox, paths, &inst.id)?,
    };
    let wayland_context = desktop.wayland_context.take();
    let mut display_protection: (Shield, String) = if desktop.nested.is_some() {
        (Shield::Partial, "nested compositor: not started yet".into())
    } else if desktop.mounts.is_empty() {
        (Shield::On, "no display".into())
    } else {
        let seen = desktop.wayland_isolation.as_ref().filter(|_| wayland_context.is_some());
        shields::planned_display(seen, desktop.uses_x11)
    };

    // 2. The instance's data. Its lock first (one session per instance: the worlds are one
    // database), held to the very end: released only after this session's data root is gone.
    let mut kept = profile::Kept::open(&inst.dir, &inst.id, cfg.bedrock.persist_worlds)?;
    // What sessions that were killed or crashed left in the directory of Bedrock sessions —
    // their data roots, each holding that session's sign-in — is deleted first (the risk
    // package's `policy::remove_leftovers`: the one function scram and doctor look through too;
    // directories a running session holds locked are left alone). A directory of sessions that
    // is a link or open to others stops the launch: no sign-in goes into it.
    let leftovers = policy::remove_leftovers(paths, Some(Instant::now() + Duration::from_secs(20))).map_err(|why| {
        Error::Policy(format!(
            "the directory of Bedrock sessions cannot be used ({why}); nothing was deleted and nothing was started"
        ))
    })?;
    if !leftovers.removed.is_empty() {
        say(&format!(
            "deleted the data of {} earlier Bedrock session(s) that did not end properly — the game's sign-in with it",
            leftovers.removed.len()
        ));
    }
    for failure in &leftovers.failed {
        reporter.warn(format!(
            "WARNING: the data of an earlier Bedrock session could not be deleted ({failure}); it may hold that \
             session's Xbox sign-in. The next Bedrock start tries again, and `bastion-mc scram` deletes it."
        ));
    }
    // This session's data root: the client's -dd, and with it the token store of the game's
    // sign-in library (`policy::SessionDir`, under `policy::sessions_dir`, 0700, locked).
    let mut session = policy::SessionDir::create(paths, &inst.id)?;
    say(&if kept.is_empty() {
        "data: nothing is kept ([bedrock] persist_worlds = false): this session starts without worlds, and everything \
         it writes is deleted at its end"
            .to_string()
    } else {
        format!(
            "data: worlds and the game's settings are kept in {}; everything else the session writes — the game's \
             sign-in included — is deleted at its end",
            inst.dir.join(instance::DATA_DIR).display()
        )
    });

    // The launch log, opened before the network: the Realms announcer writes to it.
    let (log, log_path) = Log::open(paths, &inst.id, reporter)?;

    // 3. Network: the network package's part (`bedrock::net`), prepared now — its per-launch
    // authority must be in the sandbox's trust store, so it exists before the sandbox does — and
    // started in step 10, after the launcher has confined itself, where the Java launch starts
    // its relay. Offline (`none`): neither.
    let http = crate::launch::http_client(cfg)?;
    let mut realms_answers = None;
    let (network, tcp, udp_send, resolv, prepared) = match mode {
        NetworkMode::Filtered => {
            // A Realms or event-server join waits for the user's approval outside the game. A
            // Bedrock session has no phone to type the code into: the code comes from the
            // trusted input instead (this terminal, or the front end's `answer realms` line;
            // never the game). Taken only when joins are brokered at all.
            let announce = realms_announcer(reporter, &log);
            let announce = if cfg.realms.approve != Approve::Off {
                let answers = reporter.answers(AnswerKind::Realms)?;
                let announce = realms::answers::tracking(announce, answers.pending_handle());
                realms_answers = Some(answers);
                announce
            } else {
                announce
            };
            let prepared = net::prepare(net::Options {
                cfg: &cfg.bedrock,
                instance_servers: &settings.servers,
                approve: cfg.realms.approve,
                proxied: cfg.network.launcher_proxy.is_some(),
                announce,
            })?;
            let mut lines = prepared.describe();
            lines.extend(http.socks5().map(net::describe_proxy));
            for line in &lines {
                say(line);
                log.record(Source::Launcher, line);
            }
            (
                Network::Filtered(prepared.netns_spec()),
                TcpPolicy::Ports(prepared.sandbox_tcp_ports()),
                prepared.sandbox_udp_ports(),
                Resolv::Loopback,
                Some(prepared),
            )
        }
        _ => {
            say("network: none (offline): nothing is reachable; the game cannot sign in");
            (Network::None, TcpPolicy::Ports(Vec::new()), Vec::new(), Resolv::None, None)
        }
    };
    for note in &desktop.notes {
        say(note);
    }

    // 4. No sign-in here — said, because it is the difference from every Java launch.
    say("auth: none by the launcher. The game signs in to Xbox itself (its pages are shown outside the game); no \
         Microsoft or Minecraft token of yours exists in this session, and the token store is not opened");

    // 5. Everything the sandbox is made of, before step 9.
    let trust = trust_store(prepared.as_ref().and_then(net::Prepared::trust_anchor), &setup.extra_trust_pem)?;
    let mut signin = SignIn::prepare(paths, mode)?;
    let bridge_socket = signin.socket();
    // Every installed webview the client could find (the store package's discovery: its own
    // directory, the sandbox's PATH, pacman's records, a manual install's tree; links resolved
    // to the files they lead to) is covered by the stub.
    let stub = profile::Stub::open(&opts.program, &client.installed_webviews())?;
    if !stub.covers.is_empty() {
        say(&format!(
            "sign-in: the installed webview at {} is covered by bastion-mc's bridge stub",
            stub.covers.iter().map(|(p, _)| p.display().to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    let base = sandbox::base(&BaseOptions {
        tmp_size_mb: cfg.sandbox.tmp_size_mb,
        jvm: None,
        exec_allow: &[],
        resolv,
        expose_timezone: cfg.sandbox.expose_timezone,
        trust: Some(&trust),
    })?;
    let client_file = crate::util::open_regular_nofollow(&client.program)
        .ctx(|| format!("opening the client {}", client.program.display()))?;
    let window = inst.config.width.zip(inst.config.height);
    let profile::Profile { exec, argv, mut env, mut mounts, mut landlock, cwd } = profile::build(
        base,
        profile::Inputs {
            client: &client,
            client_file,
            // A duplicate: the version itself (`setup.version`, with the shared lock on its
            // directory that makes `bedrock remove` refuse) is kept until the session is over.
            version_dir: setup.version.dir.try_clone().ctx(|| "duplicating the version's descriptor")?,
            session: &session,
            kept: &mut kept,
            stub,
            bridge_socket: bridge_socket.clone(),
            window,
            telemetry: cfg.bedrock.telemetry,
        },
    )?;
    if !cfg.bedrock.telemetry {
        say(&format!(
            "telemetry: off ([bedrock] telemetry = false): Microsoft's telemetry hosts are not reachable, and the client \
             is told to keep the game's telemetry from starting ({}=1)",
            profile::TELEMETRY_SWITCH
        ));
    }
    mounts.extend(desktop.mounts);
    env.extend(desktop.env);
    landlock.devices.extend(desktop.devices);
    landlock.unix_sockets.extend(desktop.sockets);
    landlock.tcp = tcp;
    landlock.udp_send = Some(udp_send);

    // Resource limits, asked of the user manager now.
    let limits = match cfg.sandbox.cgroup {
        CgroupMode::Off => {
            say("limits: none (sandbox.cgroup = \"off\"): the game can use all memory and process slots you can");
            None
        }
        cgroup_mode => match cgroup::preflight(cfg.sandbox.cpu_quota_percent.is_some()).await {
            Ok(_) => {
                let memory_max =
                    cfg.sandbox.memory_max_mb.map(u64::from).unwrap_or(
                        MEMORY_MAX_MB + u64::from(cfg.sandbox.tmp_size_mb) + u64::from(profile::CACHE_SIZE_MB),
                    );
                say(&format!(
                    "limits: {memory_max} MiB of memory, no swap, {} tasks{} (systemd scope; read back once the game \
                     runs)",
                    cfg.sandbox.tasks_max,
                    cfg.sandbox.cpu_quota_percent.map(|q| format!(", CPU {q}%")).unwrap_or_default()
                ));
                Some(
                    cgroup::Limits::new(&inst.id, memory_max, cfg.sandbox.tasks_max)?
                        .with_cpu_quota(cfg.sandbox.cpu_quota_percent),
                )
            }
            Err(why) if cgroup_mode == CgroupMode::Require => {
                return Err(Error::Sandbox(format!("sandbox.cgroup = \"require\", but {why}")));
            }
            Err(why) => {
                say(&format!("limits: none — {why}"));
                None
            }
        },
    };

    log.line(Source::Launcher, &format!("client argv: {}", argv.join(" ")));
    log.line(
        Source::Launcher,
        &format!(
            "sandbox: the game at {} (read-only), this session's data root at {} (deleted at the end), the bridge \
             stub at {}{}",
            profile::SANDBOX_GAME,
            profile::SANDBOX_DATA,
            signin::SANDBOX_STUB,
            if bridge_socket.is_some() { format!(", the bridge at {}", signin::SANDBOX_SOCKET) } else { String::new() }
        ),
    );
    match cfg.bedrock.seccomp {
        BedrockSeccomp::Enforce => log.line(
            Source::Launcher,
            if cfg.sandbox.tripwire {
                "seccomp: the client's own allowlist, enforced; the tripwire is on; namespaces and keyrings are refused"
            } else {
                "seccomp: the client's own allowlist, enforced; the tripwire is OFF (sandbox.tripwire = false); \
                 namespaces and keyrings are refused"
            },
        ),
        BedrockSeccomp::Log => {
            let warning = "seccomp: LOG MODE ([bedrock] seccomp = \"log\") — system calls of the client outside its \
                           allowlist are NOT refused: the kernel logs each (`journalctl -k -g 'type=1326'`) and lets \
                           it through. The tripwire and the refusal of namespaces, keyrings, io_uring, userfaultfd \
                           and perf events still hold. Profile one session, then set it back to \"enforce\".";
            log.record(Source::Launcher, warning);
            reporter.warn(warning);
        }
    }
    log.line(Source::Launcher, &signin.describe());

    // The nested compositor (X11 sessions), started last, as for Java.
    let mut nested_limits = None;
    let mut nested = match desktop.nested.take() {
        Some(mut plan) => {
            say("starting the nested compositor…");
            if limits.is_some() {
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
            if let nested::ClipboardCut::Unconfirmed(why) = &session.clipboard {
                let warning = format!(
                    "display: WARNING: the nested compositor's clipboard bridge is NOT known to be cut — {why}. The \
                     game may be able to read what you copy in other windows, and to set the clipboard."
                );
                log.record(Source::Launcher, &warning);
                reporter.warn(&warning);
            }
            display_protection = shields::nested_display(&session.clipboard);
            Some(session)
        }
        None => None,
    };

    let game_limits = limits.clone();
    let spec = SandboxSpec { mounts, network, limits, cwd, policy: InitPolicy { exec, argv, env, landlock, seccomp } };
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .ctx(|| "installing SIGINT handler")?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .ctx(|| "installing SIGTERM handler")?;

    // 8. Start. Nothing the game sends is read before step 9.
    opts.stop.supervise();
    reporter.phase(Phase::Starting);
    let display_output_held = match &nested {
        Some(session) => Some(session.hold_output().await),
        None => None,
    };
    let mut running = sandbox::spawn(spec, &opts.program)?;
    // The client's standard input carries nothing (unlike the JVM's launch frame).
    let _ = running.channel.shutdown(std::net::Shutdown::Both);
    let payload = running.payload_pid().await;
    if let (Some(context), Some(_)) = (&wayland_context, payload) {
        context.unlink();
    }
    // The bridge socket's name on the host: once the program in the sandbox exists, bubblewrap
    // has bound the socket, and the name is not needed any more — without it no other program
    // of yours can connect to the bridge. Now, while this process may still remove it.
    if bridge_socket.is_some() {
        if payload.is_some() {
            signin.release_name();
        } else {
            let warning = "sign-in: WARNING: the program in the sandbox could not be identified in time, so the \
                           bridge socket keeps its name on the host for this session: other programs of yours could \
                           send it sign-in requests (each is checked like the game's).";
            log.record(Source::Launcher, warning);
            reporter.warn(warning);
        }
    }

    // 9. Looked at, not assumed: the limits, read back from the kernel…
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
            log.line(Source::Launcher, "limits: in force — read back from the kernel for the sandbox's control group");
            limits_protection = shields::limits(shields::Limits::InForce);
        } else if matches!(running.child.try_wait(), Ok(Some(_))) {
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
            let warning = format!("limits: WARNING: asked for, but NOT in force — {why}.");
            log.record(Source::Launcher, &warning);
            reporter.warn(&warning);
            limits_protection = shields::limits(shields::Limits::NotInForce);
        }
    }

    // …then this process confines itself: from here on it is a supervisor and a relay. It
    // keeps write access to the cache (this session's data root is there, deleted at the end)
    // and nothing of the instance: the kept worlds are written by the game, not by it.
    let launcher_protection: (Shield, String) = match cfg.launcher.confine {
        ConfineMode::Off => {
            log.line(Source::Launcher, "launcher: not confined (launcher.confine = \"off\")");
            shields::launcher_off()
        }
        confine_mode => {
            let keep = confinement(paths, prepared.as_ref(), http.socks5());
            let demand =
                if confine_mode == ConfineMode::Require { confine::Demand::Whole } else { confine::Demand::Possible };
            let outcome = match confine::apply(&keep, demand) {
                Err(e) if confine_mode == ConfineMode::Require => {
                    let _ = running.stop(Duration::from_secs(5)).await;
                    return Err(e);
                }
                outcome => outcome.map_err(|e| e.to_string()),
            };
            match &outcome {
                Ok(applied) if applied.is_whole() => log.line(
                    Source::Launcher,
                    &format!(
                        "launcher: confined itself, on all threads — no program execution, writable only under the \
                         cache, TCP only to port(s) {}; no kernel keyring, no new namespaces",
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
    // 10 begins: from here on what the game sends is read. The sign-in bridge's server first —
    // it only accepts on the listener made before step 9 — so that the read-out says whether it
    // runs.
    signin.serve(reporter, &log);
    reporter.emit(Event::Protection {
        items: protection(display_protection, mode, limits_protection, launcher_protection, signin.shield()),
    });
    say(&format!("sandbox started; log: {}", log_path.display()));
    reporter.phase(Phase::Running);

    // 10. Only now is anything the game sends looked at.
    if let Some(hold) = display_output_held {
        hold.release();
    }
    let mut realms_answering = None;
    let network_running = match prepared {
        Some(prepared) => {
            let spec = prepared.netns_spec();
            let approvals = prepared.approvals();
            let sink = log.clone();
            let logger: egress::Logger = Arc::new(move |line: String| sink.line(Source::Egress, &line));
            let started = prepared.start(&spec, std::mem::take(&mut running.egress), http.clone(), logger)?;
            // From now on, a Realms code from the trusted input answers this session's join prompt.
            if let (Some(answers), Some(approvals)) = (realms_answers.take(), approvals) {
                realms_answering = Some(realms::answers::serve(answers, approvals, reporter.clone()));
            }
            Some(started)
        }
        None => None,
    };
    let out = tokio::spawn(relay_output(running.child.stdout.take().expect("piped"), Source::Game, log.clone()));
    let err = tokio::spawn(relay_output(running.child.stderr.take().expect("piped"), Source::Game, log.clone()));

    let mut interrupted = false;
    let status = tokio::select! {
        s = running.child.wait() => s.ctx(|| "waiting for the sandbox")?,
        _ = async { tokio::select! { _ = sigint.recv() => {}, _ = sigterm.recv() => {}, _ = opts.stop.wait() => {} } } => {
            say("stopping the game…");
            reporter.phase(Phase::Stopping);
            interrupted = true;
            running.stop(Duration::from_secs(15)).await.ctx(|| "stopping the sandbox")?
        }
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
    let signed_in = signin.finish().await;
    if let Some(task) = realms_answering.take() {
        task.abort();
    }
    if let Some(started) = network_running {
        for line in started.summary() {
            log.line(Source::Launcher, &line);
        }
        started.shutdown();
    }

    // 11. The session's data root — the game's sign-in in it — is deleted (bounded in time by
    // `SessionDir::remove`), and that is said only if it is true.
    match session.remove() {
        Ok(()) => log.line(
            Source::Launcher,
            "data: this session's data root has been deleted — the game's Xbox sign-in with it (the kept worlds and \
             settings are not in it)",
        ),
        Err(why) => {
            let warning = format!(
                "data: WARNING: this session's data root could NOT be deleted completely ({why}). It may hold the \
                 game's Xbox sign-in. The next Bedrock start deletes it, and so does `bastion-mc scram`."
            );
            log.record(Source::Launcher, &warning);
            reporter.warn(&warning);
        }
    }
    drop(session);
    drop(kept);
    // Only now may the version be removed (`bedrock remove` takes its lock exclusively).
    drop(setup.version);
    // Capes and the profile canary, after a session in which the game was signed in (the risk
    // package's text; nothing without a sign-in through the bridge).
    policy::report_after_session(reporter, signed_in);

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
            "SECURITY: the Bedrock client was killed by the seccomp tripwire (SIGSYS): it issued a system call that \
             exploit code uses (ptrace, bpf, mount, kexec, process_vm_*, …). Treat the server you were connected to, \
             and the content you loaded, as hostile. The client's profile has not been taken on a real game yet: if \
             this happened while the game was crashing, its crash reporter is the other explanation. Kernel log: \
             `journalctl -k -g 'type=1326'`.",
        );
        return Err(Error::Sandbox("the Bedrock client was killed by the seccomp tripwire; see the log".into()));
    }
    match std::os::unix::process::ExitStatusExt::signal(&status) {
        Some(signal) => {
            log.line(Source::Launcher, &format!("the sandbox was ended by signal {signal} (reported as status {code})"))
        }
        None => log.line(Source::Launcher, &format!("the client exited with status {code}")),
    }
    reporter.emit(Event::Exited { code });
    Ok(code)
}

/// What the launcher keeps after it confines itself in step 9: the supervisor's baseline, write
/// access to the cache — this session's data root lies there (`policy::sessions_dir`), and
/// removing it at the end is the point of the session's data split; the nested compositor's
/// directories too — and the network package's outbound ports, plus the proxy's.
fn confinement(paths: &Paths, prepared: Option<&net::Prepared>, proxy: Option<std::net::SocketAddr>) -> Confinement {
    let mut keep = Confinement::supervisor();
    keep.write.push(paths.cache.clone());
    if let Some(prepared) = prepared {
        prepared.confinement_needs(&mut keep);
    }
    keep.tcp_connect.extend(proxy.map(|proxy| proxy.port()));
    keep.tcp_connect.sort_unstable();
    keep.tcp_connect.dedup();
    keep
}

/// A Bedrock session's network mode: the instance's, else the global one — and never the
/// host's network.
fn network_mode(global: NetworkMode, instance: Option<NetworkMode>) -> Result<NetworkMode> {
    match instance.unwrap_or(global) {
        NetworkMode::Open => Err(Error::Policy(OPEN_REFUSED.into())),
        mode => Ok(mode),
    }
}

/// The client's system-call filter: its own allowlist (`seccomp::BEDROCK_ALLOW`), the
/// tripwire as configured for every sandbox, and `[bedrock] seccomp` / `extra_syscalls`.
fn seccomp_policy(cfg: &Config) -> SeccompPolicy {
    SeccompPolicy {
        mode: match cfg.bedrock.seccomp {
            BedrockSeccomp::Enforce => SeccompMode::Enforce,
            BedrockSeccomp::Log => SeccompMode::Log,
        },
        tripwire: cfg.sandbox.tripwire,
        extra_allow: cfg.bedrock.extra_syscalls.clone(),
        profile: SeccompProfile::BedrockClient,
    }
}

/// What the `Protection` event says about a Bedrock session. The `credential` item is the
/// honest difference from Java: the game holds an Xbox sign-in of its own, which nothing can
/// broker; what the launcher guarantees is that no Java token exists in the session.
fn protection(
    display: (Shield, String),
    mode: NetworkMode,
    limits: (Shield, String),
    launcher: (Shield, String),
    signin: (Shield, String),
) -> Vec<Protection> {
    let item = |aspect: &str, (shield, detail): (Shield, String)| Protection { aspect: aspect.into(), shield, detail };
    let (network, credential) = match mode {
        NetworkMode::Filtered => (
            (Shield::On, "filtered: only allowlisted Bedrock hosts, by name, through the launcher".to_string()),
            (
                Shield::Partial,
                "the game signs in to Xbox itself and holds that sign-in while it runs (deleted with the session's \
                 data at the end); no Java token exists in this session"
                    .to_string(),
            ),
        ),
        _ => (
            (Shield::On, "offline: no network at all".to_string()),
            (Shield::On, "offline: the game cannot sign in, and no Java token exists in this session".to_string()),
        ),
    };
    vec![
        item("display", display),
        item("network", network),
        item("credential", credential),
        item("sign-in", signin),
        item("limits", limits),
        item("launcher", launcher),
    ]
}

/// The sandbox's TLS trust store: the network package's per-launch authority first (when Realms
/// and event-server joins are brokered: `net::Prepared::trust_anchor`), then a test's stand-in
/// CAs ([`Setup::extra_trust_pem`], empty in a real launch), then the distribution's roots.
/// Everything here comes from this process, never from the game; the PEM is parsed strictly
/// all the same, and bounded.
fn trust_store(anchor: Option<(&str, &[u8])>, extra_pem: &[u8]) -> Result<TrustStore> {
    let mut certificates: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some((alias, der)) = anchor {
        certificates.push((alias.to_string(), der.to_vec()));
    }
    if !extra_pem.is_empty() {
        if extra_pem.len() > 64 * 1024 {
            return Err(Error::Tls("the extra CA certificates for the sandbox exceed 64 KiB".into()));
        }
        let before = certificates.len();
        for (index, der) in CertificateDer::pem_slice_iter(extra_pem).enumerate() {
            let der = der.map_err(|e| Error::Tls(format!("an extra CA certificate for the sandbox: {e:?}")))?;
            if index >= MAX_EXTRA_CAS {
                return Err(Error::Tls(format!("more than {MAX_EXTRA_CAS} extra CA certificates for the sandbox")));
            }
            certificates.push((format!("bastion-mc-bedrock-{index}"), der.as_ref().to_vec()));
        }
        if certificates.len() == before {
            return Err(Error::Tls("the extra CA certificates for the sandbox contain no certificate".into()));
        }
    }
    for (index, der) in tls::load_system_certificates()?.into_iter().enumerate() {
        certificates.push((format!("system-{index:04}"), der.as_ref().to_vec()));
    }
    Ok(TrustStore::from_certificates(&certificates))
}

// ---------------------------------------------------------------- the sign-in bridge

/// The sign-in bridge of one session (the sign-in package's `bedrock::signin`): the listener,
/// bound before the sandbox exists ([`signin::Bridge::prepare`], in a private directory of
/// this launch) and mounted into the sandbox as a file at `signin::SANDBOX_SOCKET`; its name on
/// the host, removed as soon as the sandbox holds it, so that no other program of the user can
/// reach the bridge; and from step 10, after the launcher confined itself, the server
/// ([`signin::serve`]), which only accepts on that listener. Offline there is no bridge.
struct SignIn {
    bridge: Option<signin::Bridge>,
    dir: Option<PathBuf>,
    /// The socket's name is still on the host (the program in the sandbox was not identified).
    name_kept: bool,
    serving: Option<signin::Serving>,
    /// Why the server could not be started, if it could not.
    failed: Option<String>,
}

impl SignIn {
    fn prepare(paths: &Paths, mode: NetworkMode) -> Result<SignIn> {
        let mut signin = SignIn { bridge: None, dir: None, name_kept: false, serving: None, failed: None };
        if mode == NetworkMode::Filtered {
            let dir = profile::bridge_dir(paths)?;
            signin.bridge = Some(signin::Bridge::prepare(&dir)?);
            signin.dir = Some(dir);
            signin.name_kept = true;
        }
        Ok(signin)
    }

    /// The socket on the host, to bind into the sandbox (none offline).
    fn socket(&self) -> Option<PathBuf> {
        self.bridge.as_ref().map(|b| b.socket_dir().join(signin::SOCKET_NAME))
    }

    /// Remove the socket's name and its directory on the host (the sandbox holds its mount).
    fn release_name(&mut self) {
        if let Some(socket) = self.socket() {
            let _ = std::fs::remove_file(socket);
        }
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir(dir);
        }
        self.name_kept = false;
    }

    /// The launch log's line about the bridge, before the sandbox starts.
    fn describe(&self) -> String {
        if self.bridge.is_some() {
            format!(
                "sign-in: the game's sign-in window is bastion-mc's stub; its requests come over the bridge at {} and \
                 are served once the launcher has confined itself: Microsoft's page is shown outside the game, and \
                 only this terminal or the front end answers it",
                signin::SANDBOX_SOCKET
            )
        } else {
            "sign-in: none (offline): the stub has no bridge to reach".into()
        }
    }

    /// Step 10: serve the bridge for the rest of the session.
    fn serve(&mut self, reporter: &Reporter, log: &Arc<Log>) {
        let Some(bridge) = self.bridge.take() else { return };
        let sink = log.clone();
        let line: signin::server::Log = Arc::new(move |text: &str| sink.line(Source::Launcher, text));
        match signin::serve(bridge, reporter, line) {
            Ok(serving) => self.serving = Some(serving),
            Err(e) => {
                let warning = format!(
                    "sign-in: WARNING: the bridge could not be served ({e}); a sign-in attempt in the game fails at once"
                );
                log.record(Source::Launcher, &warning);
                reporter.warn(&warning);
                self.failed = Some(e.to_string());
            }
        }
    }

    /// The `sign-in` item of the protection read-out.
    fn shield(&self) -> (Shield, String) {
        match (&self.serving, &self.failed) {
            (_, Some(why)) => {
                (Shield::Off, format!("the bridge could not be served ({why}): a sign-in attempt in the game fails"))
            }
            (Some(_), None) if self.name_kept => (
                Shield::Partial,
                "Microsoft's page is shown outside the game, but the bridge kept its name on the host: other \
                 programs of yours could send it sign-in requests (each is checked like the game's)"
                    .into(),
            ),
            (Some(_), None) => (
                Shield::On,
                "Microsoft's page (login.live.com, Minecraft's own client) is shown outside the game; only this \
                 terminal or the front end answers it"
                    .into(),
            ),
            (None, None) => (Shield::On, "offline: no sign-in pages at all".into()),
        }
    }

    /// The end of the session: the server stops (a waiting prompt is withdrawn). Whether a
    /// sign-in was handed to the game.
    async fn finish(self) -> bool {
        match self.serving {
            Some(serving) => serving.shutdown().await,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bedrock::net;

    #[test]
    fn the_network_mode_is_the_instances_else_the_global_one_and_never_open() {
        let (filtered, none, open) = (NetworkMode::Filtered, NetworkMode::None, NetworkMode::Open);
        assert_eq!(network_mode(filtered, None).unwrap(), filtered);
        assert_eq!(network_mode(none, None).unwrap(), none);
        assert_eq!(network_mode(filtered, Some(none)).unwrap(), none);
        assert_eq!(network_mode(open, Some(filtered)).unwrap(), filtered, "the instance's choice wins");
        for refused in [network_mode(open, None), network_mode(filtered, Some(open))] {
            let why = refused.unwrap_err().to_string();
            assert!(why.contains("\"open\" is refused for a Bedrock instance") && why.contains("Xbox"), "{why}");
        }
    }

    #[test]
    fn java_options_are_refused_with_what_to_do_instead() {
        let base = || LaunchOptions {
            instance: "bedrock".into(),
            account: "default".into(),
            demo: false,
            burner: false,
            join: None,
            repair: false,
            stop: crate::events::StopSignal::never(),
            program: "/proc/self/exe".into(),
        };
        assert!(refuse_java_options(&base(), "bedrock").is_ok());
        assert!(refuse_java_options(&LaunchOptions { demo: true, ..base() }, "bedrock").is_ok(), "nothing to refuse");
        for (opts, says) in [
            (LaunchOptions { burner: true, ..base() }, "mode = \"none\""),
            (LaunchOptions { join: Some("play.example.org".into()), ..base() }, "[bedrock] servers"),
            (LaunchOptions { repair: true, ..base() }, "bedrock import"),
        ] {
            match refuse_java_options(&opts, "bedrock") {
                Err(Error::Usage(why)) => {
                    assert!(why.starts_with("bedrock is a Bedrock instance") && why.contains(says), "{why}")
                }
                other => panic!("{says}: {other:?}"),
            }
        }
    }

    fn pem(der: &[u8]) -> String {
        let body: Vec<String> = crate::util::b64_encode(der)
            .as_bytes()
            .chunks(64)
            .map(|l| String::from_utf8(l.to_vec()).unwrap())
            .collect();
        format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", body.join("\n"))
    }

    fn a_ca() -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.self_signed(&rcgen::KeyPair::generate().unwrap()).unwrap().der().to_vec()
    }

    #[test]
    fn the_brokers_authority_comes_first_in_the_trust_store_and_garbage_is_refused() {
        let system = trust_store(None, b"").unwrap();
        let (broker, der) = (a_ca(), a_ca());
        let store = trust_store(Some((net::TRUST_ALIAS, &broker)), pem(&der).as_bytes()).unwrap();
        assert_eq!(store.anchors, system.anchors + 2);
        let text = String::from_utf8_lossy(store.pem()).into_owned();
        assert!(text.starts_with(&format!("# {}\n", net::TRUST_ALIAS)), "the broker's authority comes first");
        let (at_broker, at_extra) = (
            text.find(&crate::util::b64_encode(&broker)[..60]).unwrap(),
            text.find(&crate::util::b64_encode(&der)[..60]).unwrap(),
        );
        assert!(at_broker < at_extra && text.contains("# bastion-mc-bedrock-0\n"));
        // Not a certificate, too many, too large: refused, never silently dropped.
        assert!(trust_store(None, b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n").is_err());
        assert!(trust_store(None, b"hello").is_err());
        let many: String = (0..=MAX_EXTRA_CAS).map(|_| pem(&der)).collect();
        assert!(trust_store(None, many.as_bytes()).is_err());
        assert!(trust_store(None, &vec![b'A'; 65 * 1024]).is_err());
    }

    /// What the launch takes from the network package, for the filtered mode: the namespace's
    /// sockets, the ports Landlock opens, the authority for the trust store when joins are
    /// brokered, and what the launcher keeps after step 9 (443, unless a proxy carries it).
    #[test]
    fn the_network_packages_part_is_what_the_launch_puts_into_the_sandbox_and_the_confinement() {
        let announce: crate::realms::approval::Announcer = Arc::new(|_: &crate::realms::approval::Announcement| {});
        let cfg = crate::bedrock::BedrockConfig {
            servers: vec![crate::bedrock::BedrockServer { host: "play.example.org".into(), port: 19133 }],
            ..crate::bedrock::BedrockConfig::default()
        };
        let options = |approve, proxied| net::Options {
            cfg: &cfg,
            instance_servers: &[],
            approve,
            proxied,
            announce: announce.clone(),
        };
        let off = net::prepare(options(Approve::Off, false)).unwrap();
        assert_eq!(off.sandbox_tcp_ports(), [443], "HTTPS only: no game server over TCP");
        assert_eq!(off.sandbox_udp_ports(), [53, 19133], "DNS and the listed server");
        assert!(off.trust_anchor().is_none() && off.approvals().is_none(), "nothing terminated without approvals");
        let mut keep = Confinement::supervisor();
        off.confinement_needs(&mut keep);
        assert_eq!(keep.tcp_connect, [53, 443], "name lookups (the supervisor's) and HTTPS");
        let code = net::prepare(options(Approve::Code, false)).unwrap();
        let (alias, _) = code.trust_anchor().expect("an authority when joins are brokered");
        assert_eq!(alias, net::TRUST_ALIAS);
        assert!(code.approvals().is_some());
        // `totp` acts as `code` in a Bedrock session, and the launch log says so.
        let totp = net::prepare(options(Approve::Totp, false)).unwrap();
        assert!(totp.describe().iter().any(|l| l.contains("approve = \"totp\"") && l.contains("six-digit code")));
        let proxied = net::prepare(options(Approve::Off, true)).unwrap();
        let mut keep = Confinement::supervisor();
        proxied.confinement_needs(&mut keep);
        assert_eq!(keep.tcp_connect, [53], "behind a proxy the launcher never dials 443 itself");
    }

    /// The confined launcher must still be able to delete the session's data root at the end:
    /// its confinement lets it write where the risk package puts the directories of sessions.
    #[test]
    fn the_confined_launcher_can_still_delete_the_sessions_data_root() {
        let paths = Paths {
            config: "/x/config".into(),
            data: "/x/data".into(),
            cache: "/x/cache".into(),
            state: "/x/state".into(),
            runtime: None,
        };
        let keep = confinement(&paths, None, Some("127.0.0.1:9050".parse().unwrap()));
        let sessions = policy::sessions_dir(&paths);
        assert!(
            keep.write.iter().any(|w| sessions.starts_with(w)),
            "{:?} must cover {}",
            keep.write,
            sessions.display()
        );
        assert!(!keep.write.iter().any(|w| paths.data.starts_with(w) || w.starts_with(&paths.data)), "not the data");
        assert_eq!(keep.tcp_connect, [53, 9050], "name lookups and the proxy; nothing else offline");
    }

    #[test]
    fn the_protection_read_out_says_the_game_holds_its_own_sign_in_and_no_java_token_exists() {
        let some = |d: &str| (Shield::On, d.to_string());
        let items = protection(some("d"), NetworkMode::Filtered, some("l"), some("c"), (Shield::Off, "s".into()));
        let aspects: Vec<&str> = items.iter().map(|i| i.aspect.as_str()).collect();
        assert_eq!(aspects, ["display", "network", "credential", "sign-in", "limits", "launcher"]);
        let credential = &items[2];
        assert_eq!(
            credential.shield,
            Shield::Partial,
            "an Xbox sign-in in the game is not protected like a brokered token"
        );
        assert!(credential.detail.contains("signs in to Xbox itself") && credential.detail.contains("no Java token"));
        let offline = protection(some("d"), NetworkMode::None, some("l"), some("c"), (Shield::On, "s".into()));
        assert_eq!(offline[1].detail, "offline: no network at all");
        assert_eq!(offline[2].shield, Shield::On);
        assert!(offline[2].detail.contains("cannot sign in") && offline[2].detail.contains("no Java token"));
    }

    #[test]
    fn the_client_gets_its_own_seccomp_profile_with_the_configured_mode_and_additions() {
        let mut cfg = Config::default();
        let policy = seccomp_policy(&cfg);
        assert_eq!(policy.profile, SeccompProfile::BedrockClient);
        assert_eq!(policy.mode, SeccompMode::Enforce);
        assert!(policy.tripwire);
        cfg.bedrock.seccomp = BedrockSeccomp::Log;
        cfg.bedrock.extra_syscalls = vec!["splice".into()];
        let policy = seccomp_policy(&cfg);
        assert_eq!(policy.mode, SeccompMode::Log);
        assert_eq!(policy.extra_allow, ["splice"]);
        assert!(crate::sandbox::seccomp::build(&policy).is_ok());
        cfg.bedrock.extra_syscalls = vec!["keyctl".into()];
        assert!(crate::sandbox::seccomp::build(&seccomp_policy(&cfg)).is_err(), "checked before anything starts");
    }
}
