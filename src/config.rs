//! Launcher configuration (`$XDG_CONFIG_HOME/bastion-mc/config.toml`) and per-instance
//! settings (`instances/<id>/instance.toml`).
//!
//! Every table uses `deny_unknown_fields`: a mistyped security option is a hard error, never a
//! silently ignored line. Neither file can name code, JVM arguments or commands. Code reaches
//! the game through a *signed* profile manifest — and, for Fabric profiles, through the
//! launcher's own embedded companion mod, which `[companion]` can switch off or tune (see
//! `companion`) but cannot replace or point at anything else.

use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    error::{Error, Result},
    paths::Paths,
    util,
};

pub const DEFAULT_GAME_PORT: u16 = 25565;
const MAX_CONFIG_BYTES: u64 = 256 * 1024;

// ---------------------------------------------------------------- top level

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub auth: AuthConfig,
    pub tls: TlsConfig,
    pub java: JavaConfig,
    pub sandbox: SandboxConfig,
    pub network: NetworkConfig,
    pub downloads: DownloadsConfig,
    pub launcher: LauncherConfig,
    /// The companion mod for Fabric instances (see `companion`).
    pub companion: crate::companion::CompanionConfig,
    /// Settings sync across instances (see `sync`).
    pub sync: crate::sync::SyncConfig,
    /// Realms joins and how they are approved (see `realms`).
    pub realms: crate::realms::RealmsConfig,
    /// Spotify on the in-game phone: the desktop app over MPRIS, the Web API (see `spotify`).
    pub spotify: crate::spotify::SpotifyConfig,
    /// Clips of the game, saved from the in-game phone (see `clips`).
    pub clips: crate::clips::ClipsConfig,
    /// The phone's web browser (see `browser`). Off by default.
    pub browser: crate::browser::BrowserConfig,
    /// Minecraft: Bedrock Edition, the Android build in bastion's sandbox (see `bedrock`).
    /// Refused until `accept_licence_substitution` is set.
    pub bedrock: crate::bedrock::BedrockConfig,
}

impl Config {
    /// A missing file means defaults; an unreadable, foreign-owned or malformed file is fatal.
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.config_file();
        if !file.exists() {
            let cfg = Config::default();
            cfg.validate()?;
            return Ok(cfg);
        }
        util::require_private(&file)?;
        let raw = util::read_small_file(&file, MAX_CONFIG_BYTES)?;
        let text = String::from_utf8(raw).map_err(|_| Error::Config(format!("{} is not UTF-8", file.display())))?;
        let cfg: Config = toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", file.display())))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        self.browser.validate()?;
        if let Some(id) = &self.auth.client_id {
            validate_client_id(id)?;
        }
        for host in self.tls.pins.keys() {
            normalize_host(host.trim_start_matches('.'))?;
        }
        for pins in self.tls.pins.values() {
            for pin in pins {
                crate::net::tls::parse_pin(pin)?;
            }
        }
        if !(512..=262_144).contains(&self.java.memory_mb) {
            return Err(Error::Config("java.memory_mb must be between 512 and 262144".into()));
        }
        if let Some(home) = &self.java.home
            && !home.is_absolute()
        {
            return Err(Error::Config("java.home must be an absolute path".into()));
        }
        for d in &self.network.service_domains {
            normalize_host(d)?;
        }
        for s in &self.network.servers {
            s.to_rule()?;
        }
        for endpoint in &self.network.broker.extra_token_endpoints {
            crate::egress::broker::parse_endpoint(endpoint)?;
        }
        for h in &self.downloads.extra_hosts {
            normalize_host(h)?;
        }
        if !(1..=32).contains(&self.downloads.parallel) {
            return Err(Error::Config("downloads.parallel must be between 1 and 32".into()));
        }
        for name in self.sandbox.persist_files.iter().chain(&self.sandbox.extra_rw_dirs) {
            crate::paths::validate_file_name(name)
                .map_err(|e| Error::Config(format!("sandbox persist/rw entry: {e}")))?;
        }
        for name in &self.sandbox.extra_rw_dirs {
            if matches!(name.as_str(), "mods" | "resourcepacks" | "shaderpacks") {
                return Err(Error::Config(format!(
                    "sandbox.extra_rw_dirs: {name:?} cannot be made writable for the game. Mods come from the signed \
                     profile; resource and shader packs are added with `bastion-mc pack import`."
                )));
            }
            // Where the launcher keeps the settings files it carries (`persist_files`): what
            // lies there has been through a sanitiser, which a game writing into the
            // directory itself would bypass.
            if name == crate::instance::PERSIST_DIR {
                return Err(Error::Config(format!(
                    "sandbox.extra_rw_dirs: {name:?} is the launcher's own directory for the settings files it \
                     carries (persist_files); the game cannot be given it."
                )));
            }
        }
        for name in &self.sandbox.persist_files {
            // Settings files are carried to and from the game, and moved into `persist/` for
            // an instance from before that directory existed: never the files that say what
            // the instance is (its manifest, its pack list).
            if crate::instance::OWN_FILES.contains(&name.as_str()) {
                return Err(Error::Config(format!(
                    "sandbox.persist_files: {name:?} is the launcher's own file in an instance, not a settings file \
                     of the game."
                )));
            }
            // Nothing crosses from the game to the instance as raw bytes: a persisted file
            // is parsed and written out again, so its format has to be one the launcher knows.
            if crate::persist::kind_of(name).is_none() {
                return Err(Error::Config(format!(
                    "sandbox.persist_files: {name:?} has no known format (supported: servers.dat, options*.txt, \
                     other *.txt, *.dat / *.nbt as NBT, *.json). Files the game should keep as they are belong \
                     in a directory listed under extra_rw_dirs."
                )));
            }
        }
        for p in &self.sandbox.exec_allow {
            if !p.is_absolute() {
                return Err(Error::Config("sandbox.exec_allow entries must be absolute paths".into()));
            }
        }
        if !(64..=65_536).contains(&self.sandbox.tmp_size_mb) {
            return Err(Error::Config("sandbox.tmp_size_mb must be between 64 and 65536".into()));
        }
        if !(64..=1_000_000).contains(&self.sandbox.tasks_max) {
            return Err(Error::Config("sandbox.tasks_max must be between 64 and 1000000".into()));
        }
        if let Some(q) = self.sandbox.cpu_quota_percent
            && !(10..=100_000).contains(&q)
        {
            return Err(Error::Config("sandbox.cpu_quota_percent must be between 10 and 100000".into()));
        }
        // These become gamescope's command line. `--` is where the launcher puts the one
        // program gamescope starts; an entry must not be able to move that.
        let args = &self.sandbox.nested_compositor_args;
        if args.len() > 64
            || args.iter().any(|a| a.is_empty() || a.len() > 256 || a == "--" || a.chars().any(char::is_control))
        {
            return Err(Error::Config(
                "sandbox.nested_compositor_args: at most 64 non-empty options of at most 256 characters, without \
                 control characters, and none of them `--`"
                    .into(),
            ));
        }
        if let Some(limit) = self.sandbox.memory_max_mb
            && u64::from(limit) < u64::from(self.java.memory_mb) + 512
        {
            return Err(Error::Config(
                "sandbox.memory_max_mb must leave at least 512 MiB above java.memory_mb (the JVM needs memory \
                 beyond its heap)"
                    .into(),
            ));
        }
        if let Some(proxy) = &self.network.launcher_proxy {
            parse_socks5(proxy)?;
        }
        self.companion.validate()?;
        self.spotify.validate()?;
        self.clips.validate()?;
        if self.auth.token_store == TokenStore::Age {
            let age = &self.auth.age;
            if age.recipients_file.is_none() || age.identity_file.is_none() {
                return Err(Error::Config(
                    "auth.token_store = \"age\" requires auth.age.recipients_file and auth.age.identity_file".into(),
                ));
            }
            for (label, keys) in &age.accounts {
                crate::paths::validate_id(label)
                    .map_err(|_| Error::Config(format!("auth.age.accounts: {label:?} is not an account label")))?;
                if !keys.recipients_file.is_absolute() || !keys.identity_file.is_absolute() {
                    return Err(Error::Config("auth.age paths must be absolute".into()));
                }
            }
            for p in
                [Some(&age.program), age.recipients_file.as_ref(), age.identity_file.as_ref()].into_iter().flatten()
            {
                if !p.is_absolute() {
                    return Err(Error::Config("auth.age paths must be absolute".into()));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- auth

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    /// Application (client) ID of YOUR OWN Azure app registration. There is no built-in ID.
    pub client_id: Option<String>,
    pub flow: AuthFlow,
    pub token_store: TokenStore,
    /// Accept accounts whose licence comes from Xbox Game Pass (not listed by the store
    /// entitlement endpoint). Off by default: the entitlement check is then strict.
    pub allow_game_pass: bool,
    /// What to do when the account's profile (name, skin, cape) differs from what was
    /// pinned at the last sign-in and the change was not made through this launcher.
    pub canary: CanaryMode,
    /// On a canary mismatch, also delete the stored refresh token of that account. Off by
    /// default: it costs a new sign-in every time you change your skin elsewhere.
    pub canary_wipe: bool,
    pub age: AgeConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanaryMode {
    /// Refuse to launch until the change is accepted (`bastion-mc canary accept`).
    Enforce,
    /// Say so, launch anyway, keep the old pin.
    Warn,
    Off,
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig {
            client_id: None,
            flow: AuthFlow::Pkce,
            token_store: TokenStore::Keyutils,
            allow_game_pass: false,
            canary: CanaryMode::Enforce,
            canary_wipe: false,
            age: AgeConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthFlow {
    /// Authorization Code + PKCE with a loopback redirect on an ephemeral port.
    Pkce,
    /// Device Code flow (no local listener; needs "Allow public client flows" on the app).
    Device,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenStore {
    /// Kernel keyring: RAM only, gone at reboot.
    Keyutils,
    /// File encrypted by the external `age` tool (hardware keys via age plugins).
    Age,
    /// Never persist a refresh token; log in on every launch.
    None,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgeConfig {
    pub program: PathBuf,
    /// To whom vault files are encrypted. Not used — nor are the per-account ones below —
    /// when the root-owned `/etc/bastion-mc/age-recipients` exists (see `auth::store`).
    pub recipients_file: Option<PathBuf>,
    pub identity_file: Option<PathBuf>,
    /// Keys of their own for single accounts (`[auth.age.accounts.<label>]`): that
    /// account's vault file is encrypted to, and decrypted with, these instead — a
    /// different hardware key per account, for instance. Accounts not listed use the two
    /// files above.
    pub accounts: std::collections::BTreeMap<String, AgeAccountKeys>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgeAccountKeys {
    pub recipients_file: PathBuf,
    pub identity_file: PathBuf,
}

impl Default for AgeConfig {
    fn default() -> Self {
        AgeConfig {
            program: PathBuf::from("/usr/bin/age"),
            recipients_file: None,
            identity_file: None,
            accounts: Default::default(),
        }
    }
}

fn validate_client_id(id: &str) -> Result<()> {
    let b = id.as_bytes();
    let shape_ok = b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
    if shape_ok {
        Ok(())
    } else {
        Err(Error::Config("auth.client_id must be an Azure application (client) ID in UUID form".into()))
    }
}

// ---------------------------------------------------------------- tls

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsConfig {
    /// Pin Microsoft/Mojang hosts to the root and intermediate keys they are known to use
    /// (see `net::tls::MICROSOFT_PINS`). On by default.
    pub builtin_pins: bool,
    /// host (exact) or ".suffix" -> list of "sha256/<base64 SPKI hash>". A connection to a
    /// pinned host succeeds only if a certificate in the *validated* chain matches. An entry
    /// here replaces the built-in pins for the same pattern.
    pub pins: BTreeMap<String, Vec<String>>,
}

impl Default for TlsConfig {
    fn default() -> Self {
        TlsConfig { builtin_pins: true, pins: BTreeMap::new() }
    }
}

// ---------------------------------------------------------------- java

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JavaConfig {
    #[serde(rename = "match")]
    pub matching: JavaMatch,
    /// Explicit JVM home. Must still be a root-owned installation under /usr/lib/jvm.
    pub home: Option<PathBuf>,
    pub memory_mb: u32,
    /// Leave `false` unless a mod genuinely needs java.io.ObjectInputStream.
    pub allow_serialization: bool,
}

impl Default for JavaConfig {
    fn default() -> Self {
        JavaConfig { matching: JavaMatch::Exact, home: None, memory_mb: 4096, allow_serialization: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JavaMatch {
    /// Use exactly the Java feature release the game version declares.
    Exact,
    /// Allow the lowest installed release that is at least the declared one.
    Newer,
}

// ---------------------------------------------------------------- sandbox

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    pub display: DisplayPref,
    /// X11 has no client isolation: a compromised game can log keys and inject input into
    /// every other X client, which is a sandbox escape. Must be opted into explicitly.
    pub allow_x11: bool,
    pub audio: AudioPref,
    /// Expose the audio socket even when the sound server lets clients load modules into it
    /// (classic PulseAudio, or pipewire-pulse with `pulse.allow-module-loading`). That is
    /// code execution outside the sandbox; leave this off.
    pub allow_pulse_module_loading: bool,
    pub gpu: bool,
    pub seccomp: SeccompMode,
    /// Kill the JVM (SIGSYS) on syscalls that only an exploit would issue.
    pub tripwire: bool,
    pub expose_timezone: bool,
    /// Extra executables the JVM may `execve` (default: only the JVM itself).
    pub exec_allow: Vec<PathBuf>,
    /// Extra syscall names to allow, for drivers the built-in allowlist does not cover.
    pub extra_syscalls: Vec<String>,
    /// Files in the game root that the launcher carries into the session and back out again.
    /// They are never copied: each is parsed against a strict grammar for its kind and
    /// re-encoded (see `persist`), in both directions.
    pub persist_files: Vec<String>,
    /// Additional instance sub-directories mounted read-write.
    pub extra_rw_dirs: Vec<String>,
    pub tmp_size_mb: u32,
    /// Resource limits through a transient systemd scope (memory, no swap, task count).
    pub cgroup: CgroupMode,
    /// Memory ceiling for the whole sandbox, in MiB. Default: twice the JVM heap, plus the
    /// sandbox's tmpfs, plus 1 GiB — a bound on runaway use, not a tuning knob.
    pub memory_max_mb: Option<u32>,
    /// Most threads and processes the sandbox may have at once.
    pub tasks_max: u32,
    /// CPU time ceiling for the sandbox in percent of one CPU (`400` = four CPUs).
    /// Default: unbounded.
    pub cpu_quota_percent: Option<u32>,
    /// On Wayland: give the game a socket of its own that the compositor knows to be a
    /// sandbox (`wp_security_context_v1`), so that it withholds screen capture, input
    /// injection and the other interfaces meant for trusted desktop components.
    pub wayland_security_context: IsolationMode,
    /// On X11: run the game inside a nested compositor (gamescope), so that it talks to a
    /// display server of its own instead of the session's, which has no client isolation.
    pub nested_compositor: IsolationMode,
    /// Extra options for gamescope, e.g. `["-W", "1920", "-H", "1080", "-f"]`.
    pub nested_compositor_args: Vec<String>,
    /// Test support, deliberately not settable from a configuration file: directories
    /// layered over `/usr` in the nested compositor's sandbox, so that the tests can run a
    /// gamescope that is not installed. Programs found in a layer skip the root-ownership
    /// check that installed ones must pass.
    #[serde(skip)]
    pub nested_compositor_layers: Vec<PathBuf>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            display: DisplayPref::Auto,
            allow_x11: false,
            audio: AudioPref::Pulse,
            allow_pulse_module_loading: false,
            gpu: true,
            seccomp: SeccompMode::Enforce,
            tripwire: true,
            expose_timezone: false,
            exec_allow: Vec::new(),
            extra_syscalls: Vec::new(),
            persist_files: vec!["options.txt".into(), "servers.dat".into()],
            extra_rw_dirs: Vec::new(),
            tmp_size_mb: 1024,
            cgroup: CgroupMode::Auto,
            memory_max_mb: None,
            tasks_max: 4096,
            cpu_quota_percent: None,
            wayland_security_context: IsolationMode::Auto,
            nested_compositor: IsolationMode::Auto,
            nested_compositor_args: Vec::new(),
            nested_compositor_layers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CgroupMode {
    /// Use a systemd scope when the user manager can provide one; say so when it cannot.
    Auto,
    /// Refuse to launch without limits.
    Require,
    Off,
}

/// How to treat a protection that depends on what the desktop session offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IsolationMode {
    /// Use it where the session supports it; say so plainly where it does not.
    Auto,
    /// Refuse to launch without it.
    Require,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayPref {
    Auto,
    Wayland,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioPref {
    Pulse,
    Alsa,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SeccompMode {
    Enforce,
    /// Diagnostics only: forbidden syscalls are logged by the kernel and then ALLOWED.
    Log,
}

// ---------------------------------------------------------------- network

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub mode: NetworkMode,
    /// Registrable domains of Mojang services; any sub-domain is reachable on 443 only.
    pub service_domains: Vec<String>,
    pub servers: Vec<ServerEntry>,
    /// Serve `http://textures.minecraft.net/texture/<hash>` by fetching it over HTTPS.
    pub texture_gateway: bool,
    /// Keep the access token in the launcher and authenticate requests on the game's behalf.
    pub broker: BrokerConfig,
    /// `socks5h://IP:PORT`: a SOCKS5 proxy (Tor, for instance) that carries, and resolves
    /// the names for, everything that goes to Microsoft or Mojang: sign-in, downloads, and
    /// while the game runs what the credential broker forwards, the texture gateway's
    /// fetches, and the game's own HTTPS connections to Mojang service domains that the
    /// relay passes through.
    ///
    /// Not through the proxy, by design: connections to multiplayer servers, the lookups of
    /// their names and SRV records (the local resolver sees them), and relayed UDP (voice
    /// chat). Those come from your address. And with `mode = "open"` nothing the game sends
    /// is the launcher's to route: the game then reaches Mojang directly as well.
    pub launcher_proxy: Option<String>,
}

/// `socks5h://<literal IP>:<port>`. A literal address, so that finding the proxy needs no
/// name lookup; `socks5h` (not `socks5`) because the proxy does the resolving.
pub fn parse_socks5(text: &str) -> Result<std::net::SocketAddr> {
    let bad = || {
        Error::Config(format!(
            "network.launcher_proxy: {text:?} is not of the form socks5h://IP:PORT (for example \
             socks5h://127.0.0.1:9050; IPv6 as socks5h://[::1]:9050)"
        ))
    };
    let rest = text.strip_prefix("socks5h://").ok_or_else(bad)?;
    let addr: std::net::SocketAddr = rest.parse().map_err(|_| bad())?;
    if addr.port() == 0 { Err(bad()) } else { Ok(addr) }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            mode: NetworkMode::Filtered,
            service_domains: vec!["minecraft.net".into(), "mojang.com".into(), "minecraftservices.com".into()],
            servers: Vec::new(),
            texture_gateway: true,
            broker: BrokerConfig::default(),
            launcher_proxy: None,
        }
    }
}

/// The credential broker (filtered network mode). The game is handed a placeholder instead
/// of the access token; each switch below decides whether the launcher will attach the real
/// one to a class of requests. Profile, skin, cape and name changes are never allowed from
/// the game, whatever is set here.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrokerConfig {
    pub enabled: bool,
    /// Prove your identity to a game server you connect to (online-mode multiplayer).
    pub join: bool,
    /// Fetch the chat-signing key pair (servers that enforce secure chat need it).
    pub chat_signing: bool,
    /// Read your privileges, ban status and block list.
    pub account_read: bool,
    /// Friends list, friend requests, presence.
    pub friends: bool,
    /// Change account preferences (profanity filter, who may send friend requests).
    pub settings: bool,
    /// Send player reports.
    pub reports: bool,
    /// Send telemetry.
    pub telemetry: bool,
    /// Further `"METHOD host/path"` endpoints that get the credential (for game versions
    /// newer than this launcher). Hosts under mojang.com / minecraftservices.com only.
    pub extra_token_endpoints: Vec<String>,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        BrokerConfig {
            enabled: true,
            join: true,
            chat_signing: true,
            account_read: true,
            friends: false,
            settings: false,
            reports: false,
            telemetry: false,
            extra_token_endpoints: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    /// Private network namespace; the only way out is the launcher's allowlisting relay.
    Filtered,
    /// Host network namespace (no egress control, host loopback services reachable).
    Open,
    /// Private, empty network namespace.
    None,
}

/// `"host"`, `"host:port"`, `"203.0.113.7:25570"`, or a table:
/// `{ host = "play.example.org", port = 25565, allow_private = false, voice = 24454 }`.
///
/// `voice` (one port) and `udp` (a list) name UDP ports **of that server** to which the
/// launcher relays datagrams — what voice-chat mods need. Without them a rule allows no UDP
/// at all, and there is no way to allow UDP globally. See `egress::udp` for the conditions
/// under which such a datagram is relayed.
///
/// A table with a key that is not listed here is an error, like everywhere else in this
/// file: `Deserialize` is written out below because the derived untagged form would
/// silently ignore a mistyped key.
#[derive(Debug, Clone)]
pub enum ServerEntry {
    Short(String),
    Full {
        host: String,
        port: Option<u16>,
        allow_private: bool,
        /// One UDP port of this server (Simple Voice Chat's default is 24454).
        voice: Option<u16>,
        /// UDP ports of this server; `voice = N` is the same as `udp = [N]`.
        udp: Vec<u16>,
    },
}

/// The table form of a server entry, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerTable {
    host: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    allow_private: bool,
    #[serde(default)]
    voice: Option<u16>,
    #[serde(default)]
    udp: Vec<u16>,
}

impl<'de> Deserialize<'de> for ServerEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Entry;
        impl<'de> serde::de::Visitor<'de> for Entry {
            type Value = ServerEntry;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "a server: \"host\", \"host:port\", or a table { host = \"…\", port = …, allow_private = …, \
                     voice = …, udp = […] }",
                )
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> std::result::Result<ServerEntry, E> {
                Ok(ServerEntry::Short(text.to_string()))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> std::result::Result<ServerEntry, A::Error> {
                let ServerTable { host, port, allow_private, voice, udp } =
                    ServerTable::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(ServerEntry::Full { host, port, allow_private, voice, udp })
            }
        }
        deserializer.deserialize_any(Entry)
    }
}

/// Most UDP ports one server rule may name. A voice-chat mod uses one.
pub const MAX_UDP_PORTS_PER_SERVER: usize = 4;
/// Lowest UDP port a server rule may name. Below it lie the well-known services (DNS, NTP,
/// SNMP, …): no voice server listens there, the sandbox's own DNS port is among them, and a
/// relay that could be pointed at them would be a way to talk to infrastructure on the
/// server's address rather than to a game.
pub const MIN_UDP_PORT: u16 = 1024;

/// The UDP ports of one rule: sorted, without duplicates, few, and none of them privileged.
fn udp_ports(host: &str, ports: impl Iterator<Item = u16>) -> Result<Vec<u16>> {
    let mut ports: Vec<u16> = ports.collect();
    ports.sort_unstable();
    ports.dedup();
    if let Some(low) = ports.iter().find(|p| **p < MIN_UDP_PORT) {
        return Err(Error::Config(format!(
            "server {host:?}: UDP port {low} cannot be allowlisted; only ports from {MIN_UDP_PORT} up are relayed"
        )));
    }
    if ports.len() > MAX_UDP_PORTS_PER_SERVER {
        return Err(Error::Config(format!(
            "server {host:?}: {} UDP ports named; at most {MAX_UDP_PORTS_PER_SERVER} per server",
            ports.len()
        )));
    }
    Ok(ports)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HostSpec {
    Name(String),
    Ip(Ipv4Addr),
}

impl std::fmt::Display for HostSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostSpec::Name(n) => f.write_str(n),
            HostSpec::Ip(ip) => write!(f, "{ip}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRule {
    pub host: HostSpec,
    pub port: u16,
    /// `true` when the port was written out; an implicit port means "25565, or whatever the
    /// server's `_minecraft._tcp` SRV record says".
    pub explicit_port: bool,
    /// Permit destinations in private/loopback/link-local ranges (LAN servers).
    pub allow_private: bool,
    /// UDP ports of this server to which datagrams are relayed (sorted, no duplicates).
    /// Empty — the default, and the only possibility for the short forms — means no UDP.
    pub udp: Vec<u16>,
}

impl ServerEntry {
    pub fn to_rule(&self) -> Result<ServerRule> {
        match self {
            ServerEntry::Short(s) => parse_server(s, false),
            ServerEntry::Full { host, port, allow_private, voice, udp } => {
                let mut rule = parse_server(host, *allow_private)?;
                if let Some(p) = port {
                    if *p == 0 {
                        return Err(Error::Config(format!("server {host:?}: bad port")));
                    }
                    if rule.explicit_port && rule.port != *p {
                        return Err(Error::Config(format!("server {host:?}: port given twice with different values")));
                    }
                    rule.port = *p;
                    rule.explicit_port = true;
                }
                rule.udp = udp_ports(host, voice.iter().chain(udp).copied())?;
                Ok(rule)
            }
        }
    }
}

/// Parse `host[:port]`. IPv6 literals are rejected: the filtered network only speaks IPv4
/// inside the sandbox (names may still resolve to IPv6 on the host side).
pub fn parse_server(spec: &str, allow_private: bool) -> Result<ServerRule> {
    let spec = spec.trim();
    if spec.starts_with('[') || spec.matches(':').count() > 1 {
        return Err(Error::Config(format!(
            "server {spec:?}: IPv6 literals are not supported; use a host name (it may resolve to IPv6)"
        )));
    }
    let (host, port, explicit_port) = match spec.rsplit_once(':') {
        Some((h, p)) => {
            let port: u16 = p
                .parse()
                .ok()
                .filter(|p| *p != 0)
                .ok_or_else(|| Error::Config(format!("server {spec:?}: bad port")))?;
            (h, port, true)
        }
        None => (spec, DEFAULT_GAME_PORT, false),
    };
    let host = match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => HostSpec::Ip(ip),
        Ok(IpAddr::V6(_)) => return Err(Error::Config(format!("server {spec:?}: IPv6 literals are not supported"))),
        Err(_) => HostSpec::Name(normalize_host(host)?),
    };
    Ok(ServerRule { host, port, explicit_port, allow_private, udp: Vec::new() })
}

/// Lower-case, strip one trailing dot, and insist on plain LDH host-name syntax.
pub fn normalize_host(host: &str) -> Result<String> {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    if h.is_empty() || h.len() > 253 || !h.split('.').all(label_ok) {
        return Err(Error::Config(format!("invalid host name {host:?}")));
    }
    Ok(h)
}

// ---------------------------------------------------------------- downloads

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DownloadsConfig {
    /// Additional HTTPS hosts the launcher itself may download from (e.g. a private maven).
    pub extra_hosts: Vec<String>,
    /// Re-hash every asset object on every launch (a few seconds).
    pub verify_assets_on_launch: bool,
    pub parallel: usize,
}

impl Default for DownloadsConfig {
    fn default() -> Self {
        DownloadsConfig { extra_hosts: Vec::new(), verify_assets_on_launch: true, parallel: 8 }
    }
}

// ---------------------------------------------------------------- the launcher itself

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherConfig {
    /// Confine the launcher process itself (Landlock and a system-call filter, on all
    /// threads) once the game is running: "auto" (as far as the kernel can; what is missing
    /// is reported), "require" (the whole of it, or no launch), "off".
    pub confine: ConfineMode,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        LauncherConfig { confine: ConfineMode::Auto }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfineMode {
    Auto,
    Require,
    Off,
}

// ---------------------------------------------------------------- per-instance

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstanceConfig {
    /// Game version of a vanilla instance. Curated instances take it from the signed manifest.
    pub minecraft: Option<String>,
    pub memory_mb: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub network: InstanceNetwork,
    /// This instance's own companion settings, on top of the global `[companion]`.
    pub companion: crate::companion::CompanionOverride,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstanceNetwork {
    pub mode: Option<NetworkMode>,
    /// Added to the global server allowlist for this instance only.
    pub servers: Vec<ServerEntry>,
}

impl InstanceConfig {
    pub fn load(instance_dir: &Path) -> Result<Self> {
        let file = instance_dir.join("instance.toml");
        if !file.exists() {
            return Ok(InstanceConfig::default());
        }
        util::require_private(&file)?;
        let raw = util::read_small_file(&file, MAX_CONFIG_BYTES)?;
        let text = String::from_utf8(raw).map_err(|_| Error::Config(format!("{} is not UTF-8", file.display())))?;
        let cfg: InstanceConfig =
            toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", file.display())))?;
        if let Some(m) = cfg.memory_mb
            && !(512..=262_144).contains(&m)
        {
            return Err(Error::Config("instance memory_mb must be between 512 and 262144".into()));
        }
        if let Some(v) = &cfg.minecraft {
            validate_version_id(v)?;
        }
        for s in &cfg.network.servers {
            s.to_rule()?;
        }
        cfg.companion.validate()?;
        Ok(cfg)
    }
}

/// Mojang version ids become directory names; keep them to a boring alphabet.
pub fn validate_version_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && !id.contains("..")
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+' | b' '));
    if ok { Ok(()) } else { Err(Error::Config(format!("invalid Minecraft version id {id:?}"))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_strict_ones() {
        let c = Config::default();
        assert_eq!(c.network.mode, NetworkMode::Filtered);
        assert!(!c.sandbox.allow_x11);
        assert_eq!(c.sandbox.seccomp, SeccompMode::Enforce);
        assert!(c.sandbox.tripwire);
        assert!(!c.java.allow_serialization);
        assert_eq!(c.java.matching, JavaMatch::Exact);
        assert!(c.auth.client_id.is_none(), "there must be no built-in client id");
        assert!(c.downloads.verify_assets_on_launch);
        assert_eq!(c.sandbox.cgroup, CgroupMode::Auto);
        assert_eq!(c.launcher.confine, ConfineMode::Auto);
        let b = &c.network.broker;
        assert!(b.enabled && b.join && b.chat_signing && b.account_read, "what playing needs");
        assert!(!b.friends && !b.settings && !b.reports && !b.telemetry, "what it does not");
        c.validate().unwrap();
    }

    /// `examples/config.toml` says "every value shown is the default": it must parse, be
    /// valid, and describe exactly the built-in defaults — so that the file a user copies
    /// changes nothing until they change it, and no key in it is stale.
    #[test]
    fn the_example_configuration_is_the_defaults() {
        let text = include_str!("../examples/config.toml");
        let example: Config = toml::from_str(text).expect("examples/config.toml must parse");
        example.validate().unwrap();
        assert_eq!(format!("{example:#?}"), format!("{:#?}", Config::default()));
        // Every section and every documented switch appears in it (commented or not).
        for key in [
            "[auth]",
            "[launcher]",
            "[tls]",
            "[java]",
            "[sandbox]",
            "[network]",
            "[network.broker]",
            "[downloads]",
            "canary",
            "canary_wipe",
            "confine",
            "nested_compositor",
            "nested_compositor_args",
            "wayland_security_context",
            "cgroup",
            "memory_max_mb",
            "tasks_max",
            "cpu_quota_percent",
            "launcher_proxy",
            "voice = ",
            "extra_token_endpoints",
            "[auth.age.accounts.",
            "[companion]",
            "zoom_key",
            "zoom_factor",
            "zoom_wheel",
            "hud",
            "fullbright_gamma",
            "[realms]",
            "approve",
            "[sync]",
            "options",
            "[spotify]",
            "album_art",
            "client_id = ",
            "redirect_port",
            "[clips]",
            "buffer_seconds",
            "saves_per_minute",
            "max_clips",
            "max_total_mb",
            "folder = ",
            "[browser]",
            "preset = \"discord\"",
            "api_calls_per_minute",
            "megabytes_per_minute",
            "frame_rate",
        ] {
            assert!(text.contains(key), "examples/config.toml does not mention {key}");
        }
    }

    #[test]
    fn unknown_keys_are_errors() {
        assert!(toml::from_str::<Config>("[sandbox]\nallow_x11 = true\n").is_ok());
        assert!(toml::from_str::<Config>("[sandbox]\nalow_x11 = true\n").is_err());
        assert!(toml::from_str::<Config>("[sandbx]\n").is_err());
        assert!(toml::from_str::<InstanceConfig>("jvm_args = [\"-javaagent:x\"]\n").is_err());
    }

    #[test]
    fn the_launchers_own_instance_directories_cannot_be_handed_to_the_game() {
        let mut c = Config::default();
        c.sandbox.extra_rw_dirs = vec!["screenshots".into()];
        c.validate().unwrap();
        for own in ["mods", "resourcepacks", "shaderpacks", "persist"] {
            c.sandbox.extra_rw_dirs = vec!["screenshots".into(), own.into()];
            assert!(matches!(c.validate(), Err(Error::Config(m)) if m.contains(own)), "{own}");
        }
    }

    /// A settings file is carried between the game and `persist/`, and an instance from
    /// before that layout has such files moved there from its own directory. Neither may
    /// ever apply to the files that decide what the instance is and how it is protected.
    #[test]
    fn the_launchers_own_instance_files_cannot_be_carried_as_settings() {
        let mut c = Config::default();
        for own in ["profile.json", "packs.lock.json"] {
            c.sandbox.persist_files = vec!["options.txt".into(), own.into()];
            let refusal = c.validate().unwrap_err();
            assert!(
                matches!(&refusal, Error::Config(m) if m.contains(own) && m.contains("not a settings file of the game")),
                "{refusal}"
            );
        }
        c.sandbox.persist_files = vec!["options.txt".into(), "hotbar.nbt".into(), "command_history.txt".into()];
        c.validate().unwrap();
    }

    #[test]
    fn persisted_files_need_a_known_format() {
        let mut c = Config::default();
        c.sandbox.persist_files = vec!["options.txt".into(), "servers.dat".into(), "hotbar.nbt".into()];
        c.validate().unwrap();
        for bad in ["launcher_profiles", "mod.jar", "state.bin"] {
            c.sandbox.persist_files = vec![bad.into()];
            assert!(matches!(c.validate(), Err(Error::Config(m)) if m.contains("no known format")), "{bad}");
        }
    }

    #[test]
    fn client_id_shape() {
        assert!(validate_client_id("01234567-89ab-cdef-0123-456789abcdef").is_ok());
        assert!(validate_client_id("not-a-uuid").is_err());
        assert!(validate_client_id("0123456789abcdef0123456789abcdef0123").is_err());
    }

    #[test]
    fn server_specs() {
        let r = parse_server("Play.Example.COM.", false).unwrap();
        assert_eq!(r.host, HostSpec::Name("play.example.com".into()));
        assert_eq!((r.port, r.explicit_port), (25565, false));
        let r = parse_server("203.0.113.7:25570", false).unwrap();
        assert_eq!(r.host, HostSpec::Ip(Ipv4Addr::new(203, 0, 113, 7)));
        assert_eq!((r.port, r.explicit_port), (25570, true));
        for bad in ["[::1]:25565", "::1", "host:0", "host:99999", "bad host", "", "a..b", "-x.example"] {
            assert!(parse_server(bad, false).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn server_entry_forms() {
        let c: NetworkConfig = toml::from_str(
            "servers = [\"mc.example.org\", { host = \"lan.box\", port = 25570, allow_private = true }]\n",
        )
        .unwrap();
        let rules: Vec<_> = c.servers.iter().map(|s| s.to_rule().unwrap()).collect();
        assert_eq!(rules[0].port, 25565);
        assert!(rules[1].allow_private && rules[1].port == 25570 && rules[1].explicit_port);
    }

    fn rules(text: &str) -> Result<Vec<ServerRule>> {
        let net: NetworkConfig = toml::from_str(text).map_err(|e| Error::Config(e.to_string()))?;
        net.servers.iter().map(ServerEntry::to_rule).collect()
    }

    #[test]
    fn a_server_rule_can_name_udp_ports() {
        let r = rules(
            r#"servers = [
                "plain.example.org",
                { host = "play.example.org", voice = 24454 },
                { host = "lan.box", port = 25570, allow_private = true, udp = [24455, 24454, 24455] },
                { host = "both.example.org", voice = 24454, udp = [30000] },
                { host = "none.example.org", udp = [] },
                { host = "edges.example.org", udp = [1024, 65535] },
                { host = "four.example.org", voice = 1024, udp = [1024, 1025, 1026, 1027] },
            ]"#,
        )
        .unwrap();
        assert!(r[0].udp.is_empty(), "the short form allows no UDP");
        assert_eq!(r[1].udp, [24454]);
        assert_eq!((r[1].port, r[1].explicit_port, r[1].allow_private), (25565, false, false), "nothing else changes");
        assert_eq!(r[2].udp, [24454, 24455], "sorted, each port once");
        assert!(r[2].allow_private && r[2].port == 25570);
        assert_eq!(r[3].udp, [24454, 30000], "`voice` is one more entry of `udp`");
        assert!(r[4].udp.is_empty());
        assert_eq!(r[5].udp, [1024, 65535], "the lowest and the highest port that can be named");
        assert_eq!(r[6].udp.len(), MAX_UDP_PORTS_PER_SERVER, "a port named twice counts once");
        // What `--join` produces, and every short form: never any UDP.
        assert!(parse_server("play.example.org:25565", true).unwrap().udp.is_empty());
        assert!(ServerEntry::Short("203.0.113.7:25570".into()).to_rule().unwrap().udp.is_empty());
    }

    #[test]
    fn hostile_and_mistaken_udp_rules_are_errors() {
        for bad in [
            r#"servers = [{ host = "a.example", voice = 0 }]"#,
            r#"servers = [{ host = "a.example", voice = 53 }]"#, // the sandbox's own DNS port
            r#"servers = [{ host = "a.example", voice = 123 }]"#,
            r#"servers = [{ host = "a.example", voice = 1023 }]"#,
            r#"servers = [{ host = "a.example", udp = [24454, 161] }]"#,
            r#"servers = [{ host = "a.example", voice = 65536 }]"#,
            r#"servers = [{ host = "a.example", voice = -1 }]"#,
            r#"servers = [{ host = "a.example", voice = 99999999999999999999 }]"#,
            r#"servers = [{ host = "a.example", voice = "24454" }]"#,
            r#"servers = [{ host = "a.example", voice = 24454.0 }]"#,
            r#"servers = [{ host = "a.example", voice = [24454] }]"#,
            r#"servers = [{ host = "a.example", voice = true }]"#,
            r#"servers = [{ host = "a.example", udp = 24454 }]"#,
            r#"servers = [{ host = "a.example", udp = "all" }]"#,
            r#"servers = [{ host = "a.example", udp = ["*"] }]"#,
            r#"servers = [{ host = "a.example", udp = ["24454"] }]"#,
            r#"servers = [{ host = "a.example", udp = [[24454]] }]"#,
            r#"servers = [{ host = "a.example", udp = [{ port = 24454 }] }]"#,
            r#"servers = [{ host = "a.example", udp = [1024, 1025, 1026, 1027, 1028] }]"#, // more than four
            r#"servers = [{ host = "a.example", voice = 1028, udp = [1024, 1025, 1026, 1027] }]"#,
            // A mistyped key is an error. It used to be dropped without a word, which for
            // these keys would have meant "no voice" and for `allow_private` a silent refusal.
            r#"servers = [{ host = "a.example", vioce = 24454 }]"#,
            r#"servers = [{ host = "a.example", UDP = [24454] }]"#,
            r#"servers = [{ host = "a.example", alow_private = true }]"#,
            r#"servers = [{ host = "a.example", voice = 24454, voice = 24455 }]"#,
            r#"servers = [{ voice = 24454 }]"#, // no host
            r#"servers = [{ host = "", voice = 24454 }]"#,
            r#"servers = [{ host = "a.example", port = 0, voice = 24454 }]"#,
            r#"servers = [{ host = "a.example:25565", port = 25570 }]"#,
            r#"servers = [{ host = "127.64.0.5 ", voice = 24454, port = 70000 }]"#,
            r#"servers = [24454]"#,
            r#"servers = [["a.example"]]"#,
            r#"servers = [true]"#,
            // There is no way to allow UDP other than in a server's own rule.
            r#"udp = [24454]"#,
            r#"voice = 24454"#,
            r#"servers = ["a.example"]
               udp = true"#,
        ] {
            assert!(rules(bad).is_err(), "{bad}");
        }
        // The message says what is wrong.
        let typo = rules(r#"servers = [{ host = "a.example", vioce = 24454 }]"#).unwrap_err().to_string();
        assert!(typo.contains("vioce") && typo.contains("voice"), "{typo}");
        let low = rules(r#"servers = [{ host = "a.example", voice = 53 }]"#).unwrap_err().to_string();
        assert!(low.contains("UDP port 53") && low.contains("1024"), "{low}");
        let many = rules(r#"servers = [{ host = "a.example", udp = [1024, 1025, 1026, 1027, 1028] }]"#).unwrap_err();
        assert!(many.to_string().contains("at most 4"), "{many}");
        let shape = rules("servers = [24454]").unwrap_err().to_string();
        assert!(shape.contains("a server"), "{shape}");
    }

    #[test]
    fn an_instance_takes_the_same_server_rules_and_checks_them_when_it_is_loaded() {
        let dir = std::env::temp_dir()
            .join(format!("bastion-inst-{}", crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())));
        crate::util::create_private_dir_all(&dir).unwrap();
        let write = |text: &str| crate::util::write_atomic(&dir.join("instance.toml"), text.as_bytes(), 0o600).unwrap();

        write("[network]\nservers = [\"plain.example.org\", { host = \"play.example.org\", voice = 24454 }]\n");
        let cfg = InstanceConfig::load(&dir).unwrap();
        let rules: Vec<_> = cfg.network.servers.iter().map(|s| s.to_rule().unwrap()).collect();
        assert!(rules[0].udp.is_empty());
        assert_eq!(rules[1].udp, [24454]);

        for bad in [
            "[network]\nservers = [{ host = \"play.example.org\", voice = 53 }]\n",
            "[network]\nservers = [{ host = \"play.example.org\", voic = 24454 }]\n",
            "[network]\nservers = [{ host = \"play.example.org\", udp = [1, 2, 3, 4, 5] }]\n",
            "[network]\nudp = [24454]\n",
        ] {
            write(bad);
            assert!(matches!(InstanceConfig::load(&dir), Err(Error::Config(_))), "{bad}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
