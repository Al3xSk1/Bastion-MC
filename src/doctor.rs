//! `bastion-mc doctor`: report what this machine can and cannot enforce, and prove the
//! sandbox works by actually starting it. Nothing is skipped silently.

use std::{path::Path, time::Duration};

use tokio::io::AsyncReadExt;

use crate::{
    config::{Config, NetworkMode, TokenStore as StoreKind},
    hardening::Hardening,
    java,
    paths::Paths,
    profile::signing::TrustStore,
    sandbox::{
        self, BaseOptions, Network, Resolv, SandboxSpec, display, landlock,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
};

#[derive(Default)]
struct Report {
    failures: usize,
    warnings: usize,
}

impl Report {
    fn ok(&mut self, what: &str, detail: impl AsRef<str>) {
        println!("  ok    {what}: {}", detail.as_ref());
    }
    fn warn(&mut self, what: &str, detail: impl AsRef<str>) {
        self.warnings += 1;
        println!("  WARN  {what}: {}", detail.as_ref());
    }
    fn fail(&mut self, what: &str, detail: impl AsRef<str>) {
        self.failures += 1;
        println!("  FAIL  {what}: {}", detail.as_ref());
    }
    fn info(&mut self, what: &str, detail: impl AsRef<str>) {
        println!("  info  {what}: {}", detail.as_ref());
    }
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

async fn sandbox_self_test(jvm: &java::Jvm) -> Result<String, String> {
    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: Some(jvm),
        exec_allow: &[],
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })
    .map_err(|e| e.to_string())?;
    base.landlock.tcp = TcpPolicy::Ports(vec![]);
    base.landlock.udp_send = Some(Vec::new());
    let mut argv = vec!["java".to_string(), "-Xmx64m".into()];
    argv.extend(java::hardening_flags(jvm.major, false));
    argv.push("-version".into());
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::None,
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: jvm.java.clone(),
            argv,
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let mut running = sandbox::spawn(spec, Path::new("/proc/self/exe")).map_err(|e| e.to_string())?;
    let mut err = running.child.stderr.take().expect("piped");
    let mut text = String::new();
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        let _ = err.read_to_string(&mut text).await;
        running.child.wait().await
    })
    .await
    .map_err(|_| "timed out".to_string())?
    .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(text.lines().next().unwrap_or_default().to_string())
    } else {
        Err(format!("exit {status}: {}", crate::util::sanitize_line(text.as_bytes(), 300)))
    }
}

/// What each display protection can do on this machine, independent of which one the
/// current session would pick.
fn display_isolation(r: &mut Report, cfg: &Config, paths: &Paths) {
    use crate::config::IsolationMode;
    let wayland_session = display::wayland_socket(paths);
    match (&wayland_session, cfg.sandbox.wayland_security_context) {
        (None, _) => r.info("Wayland security context", "no Wayland session socket in this session"),
        (Some(_), IsolationMode::Off) => r.warn(
            "Wayland security context",
            "sandbox.wayland_security_context = \"off\": the game would get the session's own socket",
        ),
        (Some(socket), mode) => match sandbox::wayland::globals(socket) {
            Ok(globals) if globals.iter().any(|g| g.interface == sandbox::wayland::MANAGER) => r.ok(
                "Wayland security context",
                "supported: the compositor withholds privileged interfaces (screen capture, input injection, …) \
                 from the game's socket",
            ),
            Ok(_) if mode == IsolationMode::Require => r.fail(
                "Wayland security context",
                "not supported by this compositor, and sandbox.wayland_security_context = \"require\"",
            ),
            Ok(_) => r.warn(
                "Wayland security context",
                "not supported by this compositor: the game would get the session's socket, with every interface \
                 the compositor offers ordinary clients",
            ),
            Err(e) => r.warn("Wayland security context", format!("could not ask the compositor: {e}")),
        },
    }
    let nested = sandbox::nested::locate(&cfg.sandbox.nested_compositor_layers).and_then(|()| {
        if Path::new("/dev/dri").is_dir() {
            Ok(())
        } else {
            Err("gamescope renders with Vulkan and this machine exposes no GPU (/dev/dri)".to_string())
        }
    });
    let x11_session = wayland_session.is_none() && display::x11_socket().is_some();
    match (nested, cfg.sandbox.nested_compositor) {
        (_, IsolationMode::Off) if x11_session => r.warn(
            "nested compositor",
            "sandbox.nested_compositor = \"off\": on this X11 session the game would share the session's display",
        ),
        (_, IsolationMode::Off) => r.info("nested compositor", "off (sandbox.nested_compositor = \"off\")"),
        (Ok(()), _) => r.ok("nested compositor", "available: gamescope can give the game an X display of its own"),
        (Err(why), IsolationMode::Require) => r.fail("nested compositor", why),
        (Err(why), _) if x11_session => r.warn("nested compositor", format!("not available: {why}")),
        (Err(why), _) => r.info("nested compositor", format!("not available ({why}); not needed on Wayland")),
    }
}

pub async fn run(paths: &Paths, cfg: &Config, hardening: &Hardening) -> i32 {
    let mut r = Report::default();

    println!("kernel");
    r.info("release", read("/proc/sys/kernel/osrelease").unwrap_or_default());
    let abi = landlock::kernel_abi();
    if abi >= landlock::MIN_KERNEL_ABI {
        r.ok("Landlock", format!("ABI {abi} (minimum {})", landlock::MIN_KERNEL_ABI));
        if abi >= landlock::UDP_ABI {
            r.ok("Landlock UDP rules", "the sandbox can send datagrams only to the launcher's DNS port");
        } else {
            r.info(
                "Landlock UDP rules",
                format!(
                    "need ABI {} (Linux 7.2); until then the private network namespace alone contains UDP",
                    landlock::UDP_ABI
                ),
            );
        }
    } else {
        r.fail(
            "Landlock",
            format!("ABI {abi}; need at least {} (Linux 6.12, `landlock` in lsm=)", landlock::MIN_KERNEL_ABI),
        );
    }
    match read("/proc/sys/kernel/unprivileged_userns_clone").as_deref() {
        Some("0") => {
            r.fail("user namespaces", "kernel.unprivileged_userns_clone = 0; bubblewrap cannot work unprivileged")
        }
        _ => r.ok("user namespaces", "unprivileged user namespaces available"),
    }
    // SAFETY: getpid through the x32 entry point; no side effects.
    if unsafe { libc::syscall(0x4000_0000 | 39) } > 0 {
        r.info(
            "x32 ABI",
            "enabled in this kernel — syscall denylists would be bypassable; the allowlist filter is not",
        );
    }
    match read("/proc/sys/kernel/yama/ptrace_scope").as_deref() {
        Some("0") => r.warn(
            "ptrace_scope",
            "0: any process running as you can read this launcher's memory while it holds tokens",
        ),
        Some("3") => r.ok("ptrace_scope", "3: no process can attach to another, including to the sandboxed game"),
        Some(v) => r.warn(
            "ptrace_scope",
            format!(
                "{v}: the launcher is protected, but a sandboxed process lives in a user namespace you own, so any \
                 other program running as you can still read the game's memory (and its session token). \
                 Only kernel.yama.ptrace_scope=3 closes that."
            ),
        ),
        None => r.warn("ptrace_scope", "Yama is not available"),
    }
    if let Some(pattern) = read("/proc/sys/kernel/core_pattern") {
        r.info(
            "core dumps",
            format!(
                "handler {:?}; launcher and game run with RLIMIT_CORE=1 (kernel aborts the dump)",
                pattern.chars().take(40).collect::<String>()
            ),
        );
    }

    println!("launcher process");
    for (name, ok) in [
        ("non-dumpable", hardening.non_dumpable),
        ("core dumps blocked", hardening.core_dumps_blocked),
        ("no_new_privs", hardening.no_new_privs),
    ] {
        if ok { r.ok(name, "yes") } else { r.fail(name, "could not be set") }
    }
    if hardening.whole_process_locked {
        r.ok("memory locking", "entire process locked (mlockall)");
    } else {
        let limit = hardening.memlock_limit.map(|l| format!("{} KiB", l / 1024)).unwrap_or_else(|| "unlimited".into());
        r.info(
            "memory locking",
            format!("token buffers are mlock'd individually; RLIMIT_MEMLOCK ({limit}) is too small to lock the whole process"),
        );
    }
    match crate::secret::backing() {
        crate::secret::Backing::SecretMem => r.ok(
            "token memory",
            "memfd_secret: tokens are locked in RAM and removed from the kernel's direct map (not readable \
             through /proc/kcore, ptrace or process_vm_readv)",
        ),
        crate::secret::Backing::Locked => r.info(
            "token memory",
            "locked anonymous memory (never swapped or dumped). memfd_secret is not available: this kernel has \
             it disabled (`secretmem.enable=0`) or does not provide it",
        ),
    }
    match (cfg.launcher.confine, crate::confine::supported()) {
        (crate::config::ConfineMode::Off, _) => r.warn(
            "self-confinement",
            "launcher.confine = \"off\": the launcher keeps all of your access while the game runs",
        ),
        (_, Ok(())) => r.ok(
            "self-confinement",
            "once the game runs, the launcher drops file access outside the instance, program execution and most \
             of the network (Landlock on all threads)",
        ),
        (crate::config::ConfineMode::Require, Err(why)) => r.fail("self-confinement", why),
        (_, Err(why)) => r.warn("self-confinement", format!("not available: {why}")),
    }
    let swaps: Vec<String> = read("/proc/swaps")
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    if swaps.iter().any(|s| !s.contains("zram")) {
        r.warn(
            "swap",
            format!(
                "disk-backed swap in use ({}): transient TLS buffers are wiped on free but not locked",
                swaps.join(", ")
            ),
        );
    } else {
        r.ok(
            "swap",
            if swaps.is_empty() { "none".to_string() } else { format!("RAM-backed only ({})", swaps.join(", ")) },
        );
    }
    let groups: Vec<String> = nix::unistd::getgroups()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|g| nix::unistd::Group::from_gid(g).ok().flatten().map(|g| g.name))
        .collect();
    for risky in ["docker", "lxd", "incus-admin", "libvirt"] {
        if groups.iter().any(|g| g == risky) {
            r.warn(
                "account",
                format!("you are in group `{risky}`, which grants root-equivalent access: any program running as you can take over the machine, sandbox or not"),
            );
        }
    }

    println!("sandbox");
    match sandbox::check_bwrap() {
        Ok(p) => r.ok("bubblewrap", format!("{} (root-owned, not setuid)", p.display())),
        Err(e) => r.fail("bubblewrap", e.to_string()),
    }
    let jvms = match java::discover() {
        Ok(j) if !j.is_empty() => {
            let list: Vec<String> = j.iter().map(|j| format!("Java {} ({})", j.major, j.home.display())).collect();
            r.ok("system JVMs", list.join(", "));
            j
        }
        Ok(_) => {
            r.fail("system JVMs", format!("none under {}; install e.g. jre21-openjdk", java::JVM_ROOT));
            Vec::new()
        }
        Err(e) => {
            r.fail("system JVMs", e.to_string());
            Vec::new()
        }
    };
    if let Some(jvm) = jvms.first() {
        match sandbox_self_test(jvm).await {
            Ok(line) => r.ok("live self-test", format!("JVM started under bubblewrap + Landlock + seccomp: {line}")),
            Err(e) => r.fail("live self-test", e),
        }
    }
    match cfg.sandbox.cgroup {
        crate::config::CgroupMode::Off => {
            r.warn("resource limits", "sandbox.cgroup = \"off\": the game is not limited in memory or process count")
        }
        mode => match sandbox::cgroup::preflight().await {
            Ok(program) => r.ok(
                "resource limits",
                format!("transient systemd scope via {} (memory ceiling, no swap, task limit)", program.display()),
            ),
            Err(why) if mode == crate::config::CgroupMode::Require => r.fail("resource limits", why),
            Err(why) => r.warn("resource limits", format!("not available: {why}")),
        },
    }
    match display::plan(&cfg.sandbox, paths, "doctor") {
        Ok(d) => {
            for note in d.notes {
                if note.contains("WARNING") { r.warn("desktop", note) } else { r.ok("desktop", note) }
            }
        }
        Err(e) => r.warn("desktop", e.to_string()),
    }
    display_isolation(&mut r, cfg, paths);
    match cfg.network.mode {
        NetworkMode::Filtered => r.ok("network", "filtered (private namespace, allowlisting relay)"),
        NetworkMode::None => r.ok("network", "none"),
        NetworkMode::Open => r.warn("network", "open: the game shares the host network, no egress control"),
    }
    if cfg.sandbox.seccomp == crate::config::SeccompMode::Log {
        r.warn("seccomp", "mode = \"log\": violations are only logged. Diagnostics only.");
    }

    println!("trust");
    match crate::net::tls::load_system_roots() {
        Ok(roots) => r.ok(
            "TLS",
            format!(
                "TLS 1.3 only; {} system trust anchors; {} pinned host pattern(s)",
                roots.len(),
                cfg.tls.pins.len()
            ),
        ),
        Err(e) => r.fail("TLS", e.to_string()),
    }
    match TrustStore::load(paths) {
        Ok(t) if t.keys.is_empty() => r.info("profile signing keys", format!("none trusted yet ({})", t.dir.display())),
        Ok(t) => r.ok(
            "profile signing keys",
            format!(
                "{} from {:?} store {}: {}",
                t.keys.len(),
                t.source,
                t.dir.display(),
                t.keys.iter().map(|(n, k)| format!("{n} [{}]", k.id())).collect::<Vec<_>>().join(", ")
            ),
        ),
        Err(e) => r.fail("profile signing keys", e.to_string()),
    }
    match &cfg.auth.client_id {
        Some(_) => r.ok("OAuth client", "auth.client_id is set (your own Azure application)"),
        None => r.info("OAuth client", "auth.client_id not set — required for signing in; there is no built-in ID"),
    }
    match cfg.auth.token_store {
        StoreKind::Keyutils => r.ok(
            "refresh token store",
            "kernel keyring: RAM only; readable by other programs in your login session, not by the sandbox",
        ),
        StoreKind::Age => {
            match crate::auth::store::check_age_program(&cfg.auth.age.program) {
                Ok(p) => r.ok("refresh token store", format!("age vault via {}", p.display())),
                Err(e) => r.fail("refresh token store", e.to_string()),
            }
            use crate::auth::store::{IdentityKind, identity_kind};
            let mut identities: Vec<(String, &std::path::Path)> = Vec::new();
            if let Some(common) = &cfg.auth.age.identity_file {
                identities.push(("vault key".into(), common));
            }
            for (label, keys) in &cfg.auth.age.accounts {
                identities.push((format!("vault key of account {label}"), &keys.identity_file));
            }
            for (what, path) in identities {
                match identity_kind(path) {
                    IdentityKind::Plugin(name) => r.ok(
                        &what,
                        format!(
                            "hardware-backed (age-plugin-{name}): every decryption needs the device, so the stored \
                             token cannot be copied and used elsewhere"
                        ),
                    ),
                    IdentityKind::Software => r.warn(
                        &what,
                        format!(
                            "a software key in {}: it protects backups and other users, but any program running as \
                             you can read the key and decrypt the vault. Use an age plugin identity (YubiKey, TPM, \
                             FIDO2) to bind the vault to hardware",
                            path.display()
                        ),
                    ),
                    IdentityKind::PassphraseProtected => r.warn(
                        &what,
                        "a passphrase-protected identity: age will ask for the passphrase on the terminal at every \
                         launch; a front end cannot answer that",
                    ),
                    IdentityKind::Unreadable(why) => r.fail(&what, format!("{}: {why}", path.display())),
                }
            }
        }
        StoreKind::None => r.ok("refresh token store", "none (sign in on every launch)"),
    }

    println!("\n{} failure(s), {} warning(s)", r.failures, r.warnings);
    if r.failures > 0 { 1 } else { 0 }
}
