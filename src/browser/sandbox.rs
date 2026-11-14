//! The browser's sandbox: bubblewrap with namespaces of its own, a network namespace whose
//! only exits are the relay's two sockets, and the stage (`stage`) inside.
//!
//! What exists in it: `/usr` read-only, a fresh `/proc` and `/dev`, private `/tmp` and
//! `/dev/shm`, a synthetic `/etc` (no user name, host name, machine id or time zone of the
//! host; the name resolver pointed at the relay; Firefox's enterprise policies), and the
//! browser's profile directory under the launcher's data directory, mounted at `/profile`.
//! No home directory, no session bus, no sound server, no display (except for `browser
//! login`, which shows a window on yours), nothing of the game's sandbox. Its processes run
//! as user 1000 in a user namespace two levels away from any capability over its network.
//!
//! The profile is the browser's only state, and the launcher holds an exclusive lock on it
//! for as long as a browser uses it (a launch with the browser, or `browser login`).

use std::{
    ffi::OsString,
    fs::File,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd, RawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nix::fcntl::{FcntlArg, FdFlag, fcntl};

use super::stage::{STAGE_ARG, StagePolicy};
use crate::{
    error::{Context, Error, Result},
    paths::Paths,
    sandbox::{
        self, Args, Mount, cgroup, display, mount_args,
        netns::{self, NetnsSpec, SocketRole},
        sealed_memfd,
    },
    util,
};

/// Where Firefox may be installed. Not configurable: the configuration names no programs.
const FIREFOX_LOCATIONS: &[&str] =
    &["/usr/lib/firefox/firefox", "/usr/lib/firefox-esr/firefox-esr", "/usr/lib/firefox-developer-edition/firefox"];
/// Firefox's remote control listens here, on the sandbox's own loopback.
pub const REMOTE_PORT: u16 = 9222;
const STAGE_PATH: &str = "/bastion/stage";
const PROFILE: &str = "/profile";
const RUNTIME_DIR: &str = "/run/user/1000";
const HOME: &str = "/tmp/home";
const TMP_SIZE: u64 = 512 << 20;
const POLICIES_PATH: &str = "/etc/firefox/policies/policies.json";

/// The launcher's directory for the browser: `<data>/browser`.
pub fn browser_dir(paths: &Paths) -> PathBuf {
    paths.data.join("browser")
}

/// The browser's profile: `<data>/browser/profile`.
pub fn profile_dir(paths: &Paths) -> PathBuf {
    browser_dir(paths).join("profile")
}

/// The installed Firefox: its program, and its version from `application.ini`.
pub fn locate_firefox() -> Result<(PathBuf, String)> {
    for candidate in FIREFOX_LOCATIONS {
        let path = Path::new(candidate);
        if !path.exists() {
            continue;
        }
        let path = util::require_root_owned(path)
            .map_err(|e| Error::Sandbox(format!("browser: {candidate} is not a root-owned program: {e}")))?;
        let ini = path.with_file_name("application.ini");
        let version = util::read_small_file(&ini, 64 * 1024)
            .ok()
            .and_then(|raw| {
                String::from_utf8_lossy(&raw)
                    .lines()
                    .find_map(|l| l.strip_prefix("Version=").map(|v| v.trim().to_string()))
            })
            .filter(|v| !v.is_empty() && v.len() <= 32 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.'))
            .unwrap_or_else(|| "(unknown version)".into());
        return Ok((path, version));
    }
    Err(Error::Sandbox(format!(
        "browser: Firefox is not installed (looked for {}); `pacman -S firefox`",
        FIREFOX_LOCATIONS.join(", ")
    )))
}

/// The exclusive lock on the profile, held for as long as a browser uses it.
pub struct ProfileLock {
    _file: File,
}

impl ProfileLock {
    /// Take the lock, or say who has it. Firefox's own lock files, left behind by a browser
    /// that was killed, are removed once the lock is held (the pid in Firefox's lock means
    /// nothing in a new PID namespace).
    pub fn take(paths: &Paths) -> Result<ProfileLock> {
        let dir = browser_dir(paths);
        util::create_private_dir_all(&profile_dir(paths))?;
        let path = dir.join("profile.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .ctx(|| format!("opening {}", path.display()))?;
        // SAFETY: plain flock on a descriptor we own.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Error::Usage(format!(
                "the browser's profile ({}) is in use: by a `bastion-mc browser login` window, or by a running \
                 launch with the in-game browser. Close that first.",
                profile_dir(paths).display()
            )));
        }
        let profile = profile_dir(paths);
        for stale in ["lock", ".parentlock"] {
            let _ = std::fs::remove_file(profile.join(stale));
        }
        Ok(ProfileLock { _file: file })
    }
}

/// How the browser shows itself.
#[derive(Debug, Clone)]
pub enum Mode {
    /// For the phone: no display; remote control through the bridge.
    Headless,
    /// A window on the user's desktop (`browser login`).
    Visible(Desktop),
}

/// The user's display, for `browser login`.
#[derive(Debug, Clone)]
pub enum Desktop {
    Wayland { socket: PathBuf },
    X11 { number: u32, socket: PathBuf, cookie: Option<Vec<u8>> },
}

impl Desktop {
    /// The session's display: Wayland if there is one, X11 otherwise.
    pub fn from_session(paths: &Paths) -> Result<Desktop> {
        if let Some(socket) = display::wayland_socket(paths) {
            return Ok(Desktop::Wayland { socket });
        }
        let (number, socket) = display::x11_socket().ok_or_else(|| {
            Error::Usage("`browser login` shows a window: it needs a Wayland or X11 display (none was found)".into())
        })?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let cookie = std::env::var_os("XAUTHORITY")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".Xauthority")))
            .and_then(|p| util::read_small_file(&p, 256 * 1024).ok())
            .map(|raw| {
                let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
                display::filter_xauthority(&raw, number, host.trim())
            })
            .filter(|c| !c.is_empty());
        Ok(Desktop::X11 { number, socket, cookie })
    }

    pub fn describe(&self) -> String {
        match self {
            Desktop::Wayland { .. } => "your Wayland session".into(),
            Desktop::X11 { number, .. } => format!("X11 display :{number}"),
        }
    }
}

/// What to start.
pub struct Plan<'a> {
    pub paths: &'a Paths,
    pub mode: Mode,
    /// The page a visible window opens with.
    pub start_url: Option<String>,
    /// Expose the host's time zone (`sandbox.expose_timezone`).
    pub expose_timezone: bool,
    /// Resource limits (a systemd scope of its own).
    pub limits: Option<cgroup::Limits>,
    /// This program: the stage and the namespace helper.
    pub program: &'a Path,
}

/// A started browser. Its sockets have not been read from.
pub struct Started {
    /// bubblewrap (or systemd-run around it). Killed when dropped.
    pub child: tokio::process::Child,
    /// The launcher's end of the remote-control bridge (headless only).
    pub bridge: Option<UnixStream>,
    /// The relay's DNS socket (127.0.0.1:53 in the browser's namespace).
    pub dns: OwnedFd,
    /// The relay's TCP listener (0.0.0.0:443 there).
    pub listener: OwnedFd,
    pub version: String,
    pub lock: ProfileLock,
}

fn data(dst: &str, text: impl Into<String>) -> Mount {
    Mount::RoData { data: text.into().into_bytes(), dst: dst.into(), mode: 0o444 }
}

/// Firefox's preferences, written into the profile at every start (`user.js` wins over
/// what Firefox saved). Each line removes a background service — none of whose hosts the
/// relay would let through anyway — or a way for a page to reach beyond the open site.
pub const USER_JS: &str = r#"// Written by bastion-mc at every start of the browser. Changes here are overwritten.
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.startup.page", 0);
user_pref("browser.startup.homepage", "about:blank");
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.newtabpage.enabled", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("browser.tabs.warnOnClose", false);
user_pref("browser.warnOnQuit", false);
user_pref("browser.warnOnQuitShortcut", false);
user_pref("toolkit.startup.max_resumed_crashes", -1);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("toolkit.telemetry.enabled", false);
user_pref("toolkit.telemetry.unified", false);
user_pref("toolkit.telemetry.archive.enabled", false);
user_pref("app.normandy.enabled", false);
user_pref("app.shield.optoutstudies.enabled", false);
user_pref("messaging-system.rsexperimentloader.enabled", false);
user_pref("services.settings.server", "data:,#remote-settings-off/v1");
user_pref("browser.safebrowsing.malware.enabled", false);
user_pref("browser.safebrowsing.phishing.enabled", false);
user_pref("browser.safebrowsing.downloads.enabled", false);
user_pref("browser.safebrowsing.provider.google4.updateURL", "");
user_pref("browser.safebrowsing.provider.mozilla.updateURL", "");
user_pref("network.captive-portal-service.enabled", false);
user_pref("network.connectivity-service.enabled", false);
user_pref("network.trr.mode", 5);
user_pref("network.dns.echconfig.enabled", false);
user_pref("network.dns.http3_echconfig.enabled", false);
user_pref("network.dns.disablePrefetch", true);
user_pref("network.prefetch-next", false);
user_pref("network.predictor.enabled", false);
user_pref("network.http.speculative-parallel-limit", 0);
user_pref("browser.urlbar.speculativeConnect.enabled", false);
user_pref("browser.places.speculativeConnect.enabled", false);
user_pref("network.http.http3.enable", false);
user_pref("network.http.originextension", false);
user_pref("network.http.http2.coalesce-hostnames", false);
user_pref("network.http.spdy.coalesce-hostnames", false);
user_pref("dom.security.https_only_mode", true);
user_pref("media.peerconnection.enabled", false);
user_pref("media.navigator.enabled", false);
user_pref("permissions.default.camera", 2);
user_pref("permissions.default.microphone", 2);
user_pref("permissions.default.geo", 2);
user_pref("permissions.default.desktop-notification", 2);
user_pref("permissions.default.xr", 2);
user_pref("geo.enabled", false);
user_pref("dom.webnotifications.enabled", false);
user_pref("dom.push.enabled", false);
user_pref("extensions.update.enabled", false);
user_pref("extensions.getAddons.cache.enabled", false);
user_pref("extensions.systemAddon.update.enabled", false);
user_pref("extensions.pocket.enabled", false);
user_pref("identity.fxaccounts.enabled", false);
user_pref("signon.rememberSignons", false);
user_pref("signon.autofillForms", false);
user_pref("browser.formfill.enable", false);
user_pref("extensions.formautofill.addresses.enabled", false);
user_pref("extensions.formautofill.creditCards.enabled", false);
user_pref("browser.download.useDownloadDir", true);
user_pref("browser.download.folderList", 2);
user_pref("browser.download.dir", "/tmp/downloads");
user_pref("browser.download.always_ask_before_handling_new_types", false);
user_pref("browser.link.open_newwindow", 1);
user_pref("browser.link.open_newwindow.restriction", 0);
user_pref("dom.disable_open_during_load", true);
user_pref("browser.cache.disk.smart_size.enabled", false);
user_pref("browser.cache.disk.capacity", 262144);
user_pref("media.gmp-manager.updateEnabled", false);
user_pref("media.gmp-gmpopenh264.enabled", false);
user_pref("media.eme.enabled", false);
user_pref("browser.translations.enable", false);
user_pref("browser.ml.enable", false);
user_pref("browser.region.update.enabled", false);
user_pref("browser.region.network.url", "");
user_pref("browser.topsites.contile.enabled", false);
user_pref("browser.crashReports.unsubmittedCheck.enabled", false);
user_pref("layout.css.prefers-color-scheme.content-override", 0);
user_pref("ui.systemUsesDarkTheme", 1);
user_pref("browser.theme.toolbar-theme", 0);
user_pref("browser.theme.content-theme", 0);
"#;

/// Enterprise policies: what `user.js` cannot lock.
pub const POLICIES: &str = r#"{"policies": {
  "DisableAppUpdate": true,
  "DisableTelemetry": true,
  "DisableFirefoxStudies": true,
  "DisablePocket": true,
  "DisableFirefoxAccounts": true,
  "DisableFeedbackCommands": true,
  "DisableProfileImport": true,
  "DontCheckDefaultBrowser": true,
  "OfferToSaveLogins": false,
  "PasswordManagerEnabled": false,
  "ExtensionUpdate": false,
  "ExtensionSettings": {"*": {"installation_mode": "blocked"}},
  "DNSOverHTTPS": {"Enabled": false, "Locked": true},
  "NetworkPrediction": false,
  "CaptivePortal": false,
  "PromptForDownloadLocation": false,
  "DownloadDirectory": "/tmp/downloads",
  "Permissions": {
    "Camera": {"BlockNewRequests": true, "Locked": true},
    "Microphone": {"BlockNewRequests": true, "Locked": true},
    "Location": {"BlockNewRequests": true, "Locked": true},
    "Notifications": {"BlockNewRequests": true, "Locked": true},
    "VirtualReality": {"BlockNewRequests": true, "Locked": true}
  },
  "UserMessaging": {"ExtensionRecommendations": false, "FeatureRecommendations": false, "SkipOnboarding": true, "MoreFromMozilla": false}
}}
"#;

/// The mounts, environment and stage policy for `plan`, given Firefox at `firefox`.
fn layout(plan: &Plan<'_>, firefox: &Path) -> Result<(Vec<Mount>, StagePolicy)> {
    let profile = profile_dir(plan.paths);
    let mut mounts = vec![
        Mount::RoBind { src: "/usr".into(), dst: "/usr".into() },
        Mount::Symlink { target: "usr/lib".into(), link: "/lib".into() },
        Mount::Symlink { target: "usr/lib".into(), link: "/lib64".into() },
        Mount::Symlink { target: "usr/bin".into(), link: "/bin".into() },
        Mount::Symlink { target: "usr/bin".into(), link: "/sbin".into() },
        Mount::Proc { dst: "/proc".into() },
        Mount::Dev { dst: "/dev".into() },
        Mount::Tmpfs { dst: "/dev/shm".into(), size_bytes: Some(TMP_SIZE), mode: 0o1777 },
        Mount::Tmpfs { dst: "/tmp".into(), size_bytes: Some(TMP_SIZE), mode: 0o1777 },
        Mount::Dir { dst: HOME.into(), mode: 0o700 },
        Mount::Dir { dst: "/tmp/downloads".into(), mode: 0o700 },
        Mount::Dir { dst: "/run".into(), mode: 0o755 },
        Mount::Dir { dst: "/run/user".into(), mode: 0o755 },
        Mount::Dir { dst: RUNTIME_DIR.into(), mode: 0o700 },
        Mount::RoBindTry { src: "/etc/ld.so.cache".into(), dst: "/etc/ld.so.cache".into() },
        Mount::RoBindTry { src: "/etc/fonts".into(), dst: "/etc/fonts".into() },
        Mount::RoBindTry { src: "/var/cache/fontconfig".into(), dst: "/var/cache/fontconfig".into() },
        data("/etc/passwd", "root:x:0:0::/:/usr/bin/nologin\nbrowser:x:1000:1000::/tmp/home:/usr/bin/nologin\n"),
        data("/etc/group", "root:x:0:\nbrowser:x:1000:\n"),
        data("/etc/nsswitch.conf", "passwd: files\ngroup: files\nhosts: files dns\n"),
        data("/etc/hosts", "127.0.0.1 localhost browser\n::1 localhost\n"),
        data("/etc/machine-id", format!("{}\n", util::hex_encode(&util::random_bytes::<16>()?))),
        data("/etc/resolv.conf", "nameserver 127.0.0.1\noptions timeout:3 attempts:2\n"),
        data(POLICIES_PATH, POLICIES),
        Mount::Bind { src: profile.clone(), dst: PROFILE.into() },
    ];
    if plan.expose_timezone
        && let Ok(zone) = std::fs::canonicalize("/etc/localtime")
        && zone.starts_with("/usr/share/zoneinfo")
    {
        mounts.push(Mount::Symlink { target: zone, link: "/etc/localtime".into() });
    }
    let lang = std::env::var("LANG").ok().filter(|l| {
        !l.is_empty()
            && l.len() <= 32
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'@'))
    });
    let mut env: Vec<(String, String)> = [
        ("PATH", "/usr/bin"),
        ("HOME", HOME),
        ("USER", "browser"),
        ("LOGNAME", "browser"),
        ("TMPDIR", "/tmp"),
        ("XDG_RUNTIME_DIR", RUNTIME_DIR),
        ("XDG_CACHE_HOME", "/tmp/home/.cache"),
        ("XDG_CONFIG_HOME", "/tmp/home/.config"),
        ("DBUS_SESSION_BUS_ADDRESS", "disabled:"),
        ("MOZ_CRASHREPORTER_DISABLE", "1"),
        ("MOZ_DISABLE_AUTO_SAFE_MODE", "1"),
        ("MOZ_NO_REMOTE", "1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    env.push(("LANG".into(), lang.unwrap_or_else(|| "C.UTF-8".into())));

    let firefox_dir = firefox.parent().ok_or_else(|| Error::Sandbox("Firefox has no directory".into()))?.to_path_buf();
    let loader = std::fs::canonicalize("/usr/lib/ld-linux-x86-64.so.2").ctx(|| "resolving the dynamic loader")?;
    let mut argv =
        vec![firefox.to_string_lossy().into_owned(), "--no-remote".into(), "--profile".into(), PROFILE.into()];
    let mut policy = StagePolicy {
        firefox: firefox.to_path_buf(),
        read: ["/usr", "/etc", "/proc", "/var/cache/fontconfig"].iter().map(PathBuf::from).collect(),
        read_exec: vec![firefox_dir, loader],
        write: [PROFILE, "/tmp", "/dev/shm", RUNTIME_DIR].iter().map(PathBuf::from).collect(),
        devices: [
            "/dev/null",
            "/dev/zero",
            "/dev/full",
            "/dev/random",
            "/dev/urandom",
            "/dev/tty",
            "/dev/ptmx",
            "/dev/pts",
        ]
        .iter()
        .map(PathBuf::from)
        .collect(),
        tcp_connect: vec![443],
        ..Default::default()
    };
    match &plan.mode {
        Mode::Headless => {
            env.push(("MOZ_HEADLESS".into(), "1".into()));
            argv.extend(["--headless".into(), "--remote-debugging-port".into(), REMOTE_PORT.to_string()]);
            argv.push("about:blank".into());
            policy.tcp_connect.push(REMOTE_PORT);
            policy.tcp_bind.push(REMOTE_PORT);
            policy.remote_port = Some(REMOTE_PORT);
        }
        Mode::Visible(desktop) => {
            match desktop {
                Desktop::Wayland { socket } => {
                    let dst = PathBuf::from(RUNTIME_DIR).join("wayland-0");
                    mounts.push(Mount::RoBind { src: socket.clone(), dst: dst.clone() });
                    env.extend([
                        ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()),
                        ("MOZ_ENABLE_WAYLAND".into(), "1".into()),
                        ("GDK_BACKEND".into(), "wayland".into()),
                        ("XDG_SESSION_TYPE".into(), "wayland".into()),
                    ]);
                    policy.unix_sockets.push(dst);
                }
                Desktop::X11 { number, socket, cookie } => {
                    let dst = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
                    mounts.push(Mount::RoBind { src: socket.clone(), dst: dst.clone() });
                    env.extend([
                        ("DISPLAY".to_string(), format!(":{number}")),
                        ("MOZ_ENABLE_WAYLAND".into(), "0".into()),
                        ("GDK_BACKEND".into(), "x11".into()),
                        ("XDG_SESSION_TYPE".into(), "x11".into()),
                    ]);
                    if let Some(cookie) = cookie {
                        let file = PathBuf::from(RUNTIME_DIR).join("Xauthority");
                        mounts.push(Mount::RoData { data: cookie.clone(), dst: file.clone(), mode: 0o400 });
                        env.push(("XAUTHORITY".into(), file.to_string_lossy().into_owned()));
                    }
                    policy.unix_sockets.push(dst);
                }
            }
            argv.push("--new-instance".into());
            if let Some(url) = &plan.start_url {
                argv.push(url.clone());
            }
        }
    }
    policy.argv = argv;
    policy.env = env;
    Ok((mounts, policy))
}

/// Write `user.js` into the profile.
fn write_preferences(paths: &Paths) -> Result<()> {
    util::write_atomic(&profile_dir(paths).join("user.js"), USER_JS.as_bytes(), 0o600)
}

/// Start the browser. Before step 9 of a launch (or for `browser login`): the namespace
/// helper and bubblewrap are programs, and the confined launcher runs none.
pub fn start(plan: Plan<'_>) -> Result<Started> {
    let (firefox, version) = locate_firefox()?;
    let lock = ProfileLock::take(plan.paths)?;
    write_preferences(plan.paths)?;
    let (mounts, policy) = layout(&plan, &firefox)?;
    let bwrap = sandbox::check_bwrap()?;
    let stage_exe = File::open(plan.program).ctx(|| format!("opening {}", plan.program.display()))?;
    let helper_exe = OwnedFd::from(stage_exe.try_clone().ctx(|| "duplicating the launcher descriptor")?);

    let mut args = Args::default();
    for flag in ["--unshare-user", "--unshare-ipc", "--unshare-pid", "--unshare-uts", "--unshare-cgroup"] {
        args.push(flag);
    }
    args.push("--disable-userns")
        .push("--die-with-parent")
        .push("--new-session")
        .push("--clearenv")
        .push("--cap-drop")
        .push("ALL")
        .push("--hostname")
        .push("browser")
        .push("--uid")
        .push("1000")
        .push("--gid")
        .push("1000");
    for mount in mounts {
        mount_args(&mut args, mount)?;
    }
    args.push("--ro-bind-fd").fd(stage_exe.into()).push(STAGE_PATH);
    args.push("--chdir").push("/tmp");
    let mut blob = Vec::new();
    for a in &args.list {
        if a.as_bytes().contains(&0) {
            return Err(Error::Sandbox("browser sandbox argument contains NUL".into()));
        }
        blob.extend_from_slice(a.as_bytes());
        blob.push(0);
    }
    let args_fd = sealed_memfd("bastion-browser-bwrap", &blob)?;
    let policy_json = serde_json::to_vec(&policy).map_err(|e| Error::Sandbox(format!("browser policy: {e}")))?;
    let policy_fd = sealed_memfd("bastion-browser-policy", &policy_json)?;
    let mut bwrap_argv: Vec<OsString> = vec![bwrap.into(), "--args".into(), args_fd.as_raw_fd().to_string().into()];
    bwrap_argv.extend(["--".into(), STAGE_PATH.into(), STAGE_ARG.into(), policy_fd.as_raw_fd().to_string().into()]);
    let mut inherit = args.inherit;
    inherit.push(args_fd);
    inherit.push(policy_fd);

    // The network: the helper creates the namespace and the relay's two sockets in it,
    // hands them over, and executes bubblewrap.
    let spec = NetnsSpec { tcp_ports: vec![443], dns: true, local_addrs: Vec::new(), udp_ports: Vec::new() };
    let (ctrl_ours, ctrl_theirs) = UnixStream::pair().ctx(|| "creating the helper channel")?;
    let spec_fd = sealed_memfd(
        "bastion-browser-netns",
        &serde_json::to_vec(&spec).map_err(|e| Error::Sandbox(format!("namespace spec: {e}")))?,
    )?;
    let mut helper_args: Vec<OsString> = vec![
        netns::NETNS_ARG.into(),
        spec_fd.as_raw_fd().to_string().into(),
        ctrl_theirs.as_raw_fd().to_string().into(),
    ];
    helper_args.extend(bwrap_argv);
    inherit.push(spec_fd);
    inherit.push(ctrl_theirs.into());
    let program: OsString = match &plan.limits {
        Some(_) => format!("/proc/self/fd/{}", helper_exe.as_raw_fd()).into(),
        None => plan.program.into(),
    };
    inherit.push(helper_exe);

    let mut cmd = match &plan.limits {
        Some(limits) => {
            let systemd_run = cgroup::locate().map_err(|e| Error::Sandbox(format!("browser limits: {e}")))?;
            let mut cmd = tokio::process::Command::new(systemd_run);
            cmd.args(limits.command(&program, &helper_args));
            cmd.env_clear().envs(cgroup::manager_env());
            cmd
        }
        None => {
            let mut cmd = tokio::process::Command::new(&program);
            cmd.args(&helper_args);
            cmd.env_clear();
            cmd
        }
    };
    let (bridge_ours, bridge_theirs) = match plan.mode {
        Mode::Headless => {
            let (ours, theirs) = UnixStream::pair().ctx(|| "creating the remote-control bridge")?;
            (Some(ours), Stdio::from(OwnedFd::from(theirs)))
        }
        Mode::Visible(_) => (None, Stdio::null()),
    };
    let raw: Vec<RawFd> = inherit.iter().map(|f| f.as_raw_fd()).collect();
    cmd.stdin(bridge_theirs).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).kill_on_drop(true);
    // SAFETY: between fork and exec only fcntl(2), which is async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            for fd in &raw {
                if libc::fcntl(*fd, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let child = cmd.spawn().ctx(|| "starting the browser's sandbox")?;
    drop(inherit);
    let roles = spec.sockets();
    let mut sockets = netns::recv_fds(ctrl_ours.as_fd(), roles.len(), Duration::from_secs(20))?;
    for (socket, role) in sockets.iter().zip(&roles) {
        netns::check_socket(socket.as_fd(), *role)?;
    }
    if roles != [SocketRole::Dns, SocketRole::Tcp(443)] || sockets.len() != 2 {
        return Err(Error::Sandbox("the browser's namespace helper handed over the wrong sockets".into()));
    }
    let listener = sockets.pop().expect("two sockets");
    let dns = sockets.pop().expect("two sockets");
    for fd in [&dns, &listener] {
        let _ = fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC));
    }
    Ok(Started { child, bridge: bridge_ours, dns, listener, version, lock })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &Path) -> Paths {
        Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        }
    }

    #[test]
    fn the_profile_lock_is_exclusive_and_clears_firefoxs_stale_locks() {
        let root = tempdir();
        let paths = paths(&root);
        util::create_private_dir_all(&profile_dir(&paths)).unwrap();
        std::os::unix::fs::symlink("browser:+3", profile_dir(&paths).join("lock")).unwrap();
        std::fs::write(profile_dir(&paths).join(".parentlock"), b"").unwrap();
        let held = ProfileLock::take(&paths).unwrap();
        assert!(!profile_dir(&paths).join("lock").exists() && !profile_dir(&paths).join(".parentlock").exists());
        let err = ProfileLock::take(&paths).err().expect("taken").to_string();
        assert!(err.contains("in use"), "{err}");
        drop(held);
        ProfileLock::take(&paths).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_layout_gives_the_browser_only_its_profile_and_no_home() {
        let root = tempdir();
        let paths = paths(&root);
        let plan = Plan {
            paths: &paths,
            mode: Mode::Headless,
            start_url: None,
            expose_timezone: false,
            limits: None,
            program: Path::new("/proc/self/exe"),
        };
        let (mounts, policy) = layout(&plan, Path::new("/usr/lib/firefox/firefox")).unwrap();
        let binds: Vec<(&Path, &Path)> = mounts
            .iter()
            .filter_map(|m| match m {
                Mount::Bind { src, dst } => Some((src.as_path(), dst.as_path())),
                _ => None,
            })
            .collect();
        assert_eq!(binds, [(profile_dir(&paths).as_path(), Path::new("/profile"))], "the one writable host directory");
        for m in &mounts {
            if let Mount::RoBind { src, .. } | Mount::RoBindTry { src, .. } = m {
                assert!(
                    ["/usr", "/etc/ld.so.cache", "/etc/fonts", "/var/cache/fontconfig"]
                        .iter()
                        .any(|p| src == Path::new(p)),
                    "unexpected host path {}",
                    src.display()
                );
            }
        }
        assert!(policy.env.iter().any(|(k, v)| k == "HOME" && v == "/tmp/home"));
        assert!(policy.env.iter().any(|(k, v)| k == "MOZ_HEADLESS" && v == "1"));
        assert!(!policy.env.iter().any(|(k, _)| k == "DISPLAY" || k == "WAYLAND_DISPLAY"));
        assert_eq!(policy.tcp_connect, [443, REMOTE_PORT]);
        assert_eq!(policy.tcp_bind, [REMOTE_PORT]);
        assert_eq!(policy.remote_port, Some(REMOTE_PORT));
        assert!(policy.unix_sockets.is_empty());
        assert!(policy.argv.iter().any(|a| a == "--headless"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preferences_turn_off_what_would_reach_beyond_the_site() {
        for pref in [
            "\"network.trr.mode\", 5",
            "\"network.http.http3.enable\", false",
            "\"network.http.originextension\", false",
            "\"media.peerconnection.enabled\", false",
            "\"signon.rememberSignons\", false",
            "\"network.dns.echconfig.enabled\", false",
        ] {
            assert!(USER_JS.contains(pref), "{pref}");
        }
        let policies: serde_json::Value = serde_json::from_str(POLICIES).unwrap();
        assert_eq!(policies["policies"]["ExtensionSettings"]["*"]["installation_mode"], "blocked");
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bastion-browser-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
