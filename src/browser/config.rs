//! `[browser]` in config.toml: whether the phone has a browser, which sites it may show, and
//! how much each of them may use.
//!
//! A site is a name the phone opens it by, a title, a start page and the host names its
//! pages may load from. Nothing else is reachable: the browser's only way out is the
//! launcher's relay, which answers name lookups for the hosts of the site that is open and
//! for nothing else (see `relay`). How many sites may be listed is capped ([`MAX_SITES`]),
//! and so is each site's host list ([`MAX_HOSTS`]).
//!
//! The configuration names no program and no path: which Firefox runs is not configurable
//! (see `sandbox::locate_firefox`), and where its profile lives is fixed by the launcher.

use serde::Deserialize;

use crate::{
    config::normalize_host,
    error::{Error, Result},
};

/// How many sites `[browser] sites` may list.
pub const MAX_SITES: usize = 8;
/// How many host patterns one site may list.
pub const MAX_HOSTS: usize = 32;
/// The most frames per second the phone can be sent (the contract's bound).
pub const MAX_FRAME_RATE: u32 = 15;
/// The largest page the phone may ask for, in pixels, either way.
pub const MAX_VIEWPORT: u32 = 1280;
/// The smallest.
pub const MIN_VIEWPORT: u32 = 160;
const MAX_START_URL: usize = 512;
const MAX_TITLE_CHARS: usize = 32;

/// `[browser]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    /// Start the browser for launches. Off by default: it also needs `sites`.
    pub enabled: bool,
    /// The sites the phone's browser may open, in the order it lists them.
    pub sites: Vec<SiteEntry>,
    /// Pages per second sent to the phone while it shows the browser (1 to 15). Frames are
    /// sent only while the page changes.
    pub frame_rate: u32,
    /// JPEG quality of those pages, 30 to 90.
    pub quality: u32,
    /// The page size the browser starts with; the phone may change it within 160 to 1280.
    pub width: u32,
    pub height: u32,
    /// Memory ceiling of the browser's sandbox in MiB (a systemd scope of its own, where the
    /// game's limits are in use).
    pub memory_max_mb: u32,
    /// Test support, deliberately not settable from a configuration file: extra
    /// certificate authorities (PEM) the browser trusts, so that the tests can stand up
    /// HTTPS sites of their own.
    #[serde(skip)]
    pub test_authorities: Vec<String>,
    /// Test support: where the relay connects instead of the real host (name → address).
    #[serde(skip)]
    pub test_upstreams: Vec<(String, std::net::SocketAddr)>,
    /// Test support: after the launcher has confined itself, open the first site, take one
    /// frame and log what happened (`tests/browser_game.rs`).
    #[serde(skip)]
    pub self_test: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        BrowserConfig {
            enabled: false,
            sites: Vec::new(),
            frame_rate: 10,
            quality: 60,
            width: 800,
            height: 600,
            memory_max_mb: 2048,
            test_authorities: Vec::new(),
            test_upstreams: Vec::new(),
            self_test: false,
        }
    }
}

/// One entry of `sites`: a preset (`{ preset = "discord" }`) or a site of your own.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SiteEntry {
    /// A built-in site ([`PRESETS`]): its start page and hosts are fixed.
    pub preset: Option<String>,
    /// What the phone opens it by: 1 to 16 of `a-z`, `0-9`, `-`.
    pub name: Option<String>,
    /// What the phone shows.
    pub title: Option<String>,
    /// The page `open` goes to: `https://` and one of the site's hosts.
    pub start: Option<String>,
    /// Host names the site's pages may load from: `example.com`, or `*.example.com` for
    /// every name below it (not the name itself).
    pub hosts: Option<Vec<String>>,
    /// Requests of every kind the site's pages may make per minute — what a page loads to
    /// show itself included (counted by the browser's remote control; refused beyond).
    pub requests_per_minute: Option<u32>,
    /// API calls the site's scripts may make per minute: `fetch`, `XMLHttpRequest`,
    /// WebSocket and beacon requests (refused beyond).
    pub api_calls_per_minute: Option<u32>,
    /// Connections the relay opens for the site per minute (refused beyond).
    pub connections_per_minute: Option<u32>,
    /// Megabytes the relay carries for the site per minute, both directions together
    /// (slowed down beyond).
    pub megabytes_per_minute: Option<u32>,
}

/// A host name a site may load from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPattern {
    /// Exactly this name.
    Exact(String),
    /// Every name below this one (`*.example.com`: `a.example.com`, `a.b.example.com`).
    Below(String),
}

impl HostPattern {
    pub fn parse(text: &str) -> Result<HostPattern> {
        let bad = |why: &str| Error::Config(format!("browser site host {text:?}: {why}"));
        if let Some(base) = text.strip_prefix("*.") {
            let base = normalize_host(base).map_err(|_| bad("not a host name"))?;
            // `*.com` would be a whole top-level domain. (Two labels can still be a public
            // suffix such as `*.co.uk`; the relay would then allow that much. It is your list.)
            if base.split('.').count() < 2 {
                return Err(bad("a wildcard needs at least two labels after `*.`"));
            }
            return Ok(HostPattern::Below(base));
        }
        if text.contains('*') {
            return Err(bad("`*` is only allowed as the whole first label (`*.example.com`)"));
        }
        let host = normalize_host(text).map_err(|_| bad("not a host name"))?;
        if host.parse::<std::net::Ipv4Addr>().is_ok() || host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return Err(bad("sites are reached by name; an address cannot be listed"));
        }
        Ok(HostPattern::Exact(host))
    }

    /// `host` must already be normalised (lower case, no trailing dot).
    pub fn matches(&self, host: &str) -> bool {
        match self {
            HostPattern::Exact(name) => host == name,
            HostPattern::Below(base) => host.strip_suffix(base.as_str()).is_some_and(|rest| rest.ends_with('.')),
        }
    }
}

impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostPattern::Exact(name) => f.write_str(name),
            HostPattern::Below(base) => write!(f, "*.{base}"),
        }
    }
}

/// How much one site may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteLimits {
    pub requests_per_minute: u32,
    pub api_calls_per_minute: u32,
    pub connections_per_minute: u32,
    pub megabytes_per_minute: u32,
}

impl Default for SiteLimits {
    fn default() -> Self {
        // Discord's web client loads about 700 scripts and style sheets when it starts (it is
        // split into many small parts), and makes a few dozen API calls; sending a message is one.
        SiteLimits {
            requests_per_minute: 6000,
            api_calls_per_minute: 300,
            connections_per_minute: 120,
            megabytes_per_minute: 60,
        }
    }
}

/// A site as the launcher uses it: checked, with its limits filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub name: String,
    pub title: String,
    /// `https://<host><path>`, checked.
    pub start: String,
    /// The host of `start`.
    pub start_host: String,
    pub hosts: Vec<HostPattern>,
    pub limits: SiteLimits,
}

impl Site {
    pub fn allows(&self, host: &str) -> bool {
        self.hosts.iter().any(|p| p.matches(host))
    }
}

/// A built-in site.
pub struct Preset {
    pub name: &'static str,
    pub title: &'static str,
    pub start: &'static str,
    pub hosts: &'static [&'static str],
}

/// The built-in sites.
///
/// Discord: what `https://discord.com/app` loads when nobody is signed in was read from the
/// sandboxed browser's own log (`tests/browser.rs`, the internet test): `discord.com` (the
/// page, its ~700 scripts and style sheets, its API) and `remote-auth-gateway.discord.gg` (the
/// QR-code login), nothing else. A signed-in client talks to more hosts, which Discord names
/// in the page's own configuration (`GLOBAL_ENV`) and in its content security policy: the
/// gateway (`gateway.discord.gg`, and resume gateways below `discord.gg`), the CDN
/// (`cdn.discordapp.com`), the media proxies (`media.discordapp.net`,
/// `images-ext-1/2.discordapp.net`), static files (`static.discord.com`), and hCaptcha for the
/// captchas Discord asks for. Left out on purpose: voice (`*.discord.media`; it needs UDP,
/// which is not relayed), payments, analytics, and third-party embeds and GIFs
/// (YouTube, Spotify, Tenor, …). docs/notes/browser.md has the details.
pub const PRESETS: &[Preset] = &[Preset {
    name: "discord",
    title: "Discord",
    start: "https://discord.com/app",
    hosts: &[
        "discord.com",
        "*.discord.com",
        "discord.gg",
        "*.discord.gg",
        "discordapp.com",
        "*.discordapp.com",
        "*.discordapp.net",
        "hcaptcha.com",
        "*.hcaptcha.com",
    ],
}];

fn is_site_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 16
        && !name.starts_with('-')
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn check_title(title: &str) -> Result<String> {
    let clean: String = title.chars().filter(|c| !c.is_control()).collect();
    if clean.is_empty() || clean.chars().count() > MAX_TITLE_CHARS || clean != title {
        return Err(Error::Config(format!(
            "browser site title {title:?}: 1 to {MAX_TITLE_CHARS} characters, no control characters"
        )));
    }
    Ok(clean)
}

/// The host of an `https://` URL as the browser would send it: lower case, no port other
/// than 443, no user name or password. `None` for anything else (another scheme, an
/// address, a malformed authority).
pub fn https_host(url: &str) -> Option<String> {
    let rest = url.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).map(|_| &url[8..])?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return None;
    }
    let host = match authority.rsplit_once(':') {
        Some((host, "443")) => host,
        Some(_) => return None,
        None => authority,
    };
    normalize_host(host).ok()
}

fn check_start(start: &str, hosts: &[HostPattern]) -> Result<String> {
    let bad = |why: &str| Error::Config(format!("browser site start page {start:?}: {why}"));
    if start.len() > MAX_START_URL || !start.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(bad(&format!("at most {MAX_START_URL} printable ASCII characters, no spaces")));
    }
    let host = https_host(start).ok_or_else(|| bad("must be an https:// address of a host name (port 443)"))?;
    if !hosts.iter().any(|p| p.matches(&host)) {
        return Err(bad(&format!("its host {host} is not among the site's hosts")));
    }
    Ok(host)
}

fn limit(value: Option<u32>, default: u32, max: u32, what: &str, site: &str) -> Result<u32> {
    let value = value.unwrap_or(default);
    if !(1..=max).contains(&value) {
        return Err(Error::Config(format!("browser site {site:?}: {what} must be between 1 and {max}")));
    }
    Ok(value)
}

impl SiteEntry {
    /// Check the entry and fill in what a preset or the defaults provide.
    pub fn resolve(&self) -> Result<Site> {
        let (name, title, start, hosts): (String, String, String, Vec<String>) = match &self.preset {
            Some(preset) => {
                let p = PRESETS.iter().find(|p| p.name == preset).ok_or_else(|| {
                    Error::Config(format!(
                        "browser: there is no preset {preset:?} (built in: {})",
                        PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
                    ))
                })?;
                if self.start.is_some() || self.hosts.is_some() {
                    return Err(Error::Config(format!(
                        "browser: the preset {preset:?} has a fixed start page and host list; to choose your own, \
                         list the site without `preset`"
                    )));
                }
                (
                    self.name.clone().unwrap_or_else(|| p.name.into()),
                    self.title.clone().unwrap_or_else(|| p.title.into()),
                    p.start.into(),
                    p.hosts.iter().map(|h| h.to_string()).collect(),
                )
            }
            None => {
                let missing = |what: &str| Error::Config(format!("browser: a site without `preset` needs `{what}`"));
                let name = self.name.clone().ok_or_else(|| missing("name"))?;
                (
                    name.clone(),
                    self.title.clone().unwrap_or(name),
                    self.start.clone().ok_or_else(|| missing("start"))?,
                    self.hosts.clone().ok_or_else(|| missing("hosts"))?,
                )
            }
        };
        if !is_site_name(&name) {
            return Err(Error::Config(format!("browser site name {name:?}: 1 to 16 of a-z, 0-9 and -")));
        }
        let title = check_title(&title)?;
        if hosts.is_empty() || hosts.len() > MAX_HOSTS {
            return Err(Error::Config(format!("browser site {name:?}: list 1 to {MAX_HOSTS} hosts")));
        }
        let hosts = hosts.iter().map(|h| HostPattern::parse(h)).collect::<Result<Vec<_>>>()?;
        let start_host = check_start(&start, &hosts)?;
        let defaults = SiteLimits::default();
        let limits = SiteLimits {
            requests_per_minute: limit(
                self.requests_per_minute,
                defaults.requests_per_minute,
                100_000,
                "requests_per_minute",
                &name,
            )?,
            api_calls_per_minute: limit(
                self.api_calls_per_minute,
                defaults.api_calls_per_minute,
                100_000,
                "api_calls_per_minute",
                &name,
            )?,
            connections_per_minute: limit(
                self.connections_per_minute,
                defaults.connections_per_minute,
                10_000,
                "connections_per_minute",
                &name,
            )?,
            megabytes_per_minute: limit(
                self.megabytes_per_minute,
                defaults.megabytes_per_minute,
                10_000,
                "megabytes_per_minute",
                &name,
            )?,
        };
        Ok(Site { name, title, start, start_host, hosts, limits })
    }
}

impl BrowserConfig {
    /// Every listed site, checked. Names are unique; at most [`MAX_SITES`].
    pub fn resolved_sites(&self) -> Result<Vec<Site>> {
        if self.sites.len() > MAX_SITES {
            return Err(Error::Config(format!(
                "browser: {} sites are listed; at most {MAX_SITES} may be",
                self.sites.len()
            )));
        }
        let mut sites: Vec<Site> = Vec::with_capacity(self.sites.len());
        for entry in &self.sites {
            let site = entry.resolve()?;
            if sites.iter().any(|s| s.name == site.name) {
                return Err(Error::Config(format!("browser: the site name {:?} is listed twice", site.name)));
            }
            sites.push(site);
        }
        Ok(sites)
    }

    pub fn validate(&self) -> Result<()> {
        self.resolved_sites()?;
        if !(1..=MAX_FRAME_RATE).contains(&self.frame_rate) {
            return Err(Error::Config(format!("browser.frame_rate must be between 1 and {MAX_FRAME_RATE}")));
        }
        if !(30..=90).contains(&self.quality) {
            return Err(Error::Config("browser.quality must be between 30 and 90".into()));
        }
        for (what, value) in [("width", self.width), ("height", self.height)] {
            if !(MIN_VIEWPORT..=MAX_VIEWPORT).contains(&value) {
                return Err(Error::Config(format!("browser.{what} must be between {MIN_VIEWPORT} and {MAX_VIEWPORT}")));
            }
        }
        if !(512..=65_536).contains(&self.memory_max_mb) {
            return Err(Error::Config("browser.memory_max_mb must be between 512 and 65536".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<BrowserConfig> {
        let cfg: BrowserConfig = toml::from_str(text).map_err(|e| Error::Config(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    #[test]
    fn off_by_default_and_a_preset_needs_one_line() {
        let cfg = BrowserConfig::default();
        assert!(!cfg.enabled && cfg.sites.is_empty());
        cfg.validate().unwrap();
        let cfg = parse("enabled = true\nsites = [{ preset = \"discord\" }]").unwrap();
        let sites = cfg.resolved_sites().unwrap();
        assert_eq!(sites[0].name, "discord");
        assert_eq!(sites[0].start_host, "discord.com");
        assert!(sites[0].allows("gateway.discord.gg") && sites[0].allows("cdn.discordapp.com"));
        assert!(!sites[0].allows("example.com") && !sites[0].allows("discord.gg.evil.example"));
        assert_eq!(sites[0].limits, SiteLimits::default());
    }

    #[test]
    fn a_site_of_your_own_is_checked_field_by_field() {
        let cfg = parse(
            r#"
            [[sites]]
            name = "wiki"
            title = "Minecraft Wiki"
            start = "https://minecraft.wiki/w/Redstone"
            hosts = ["minecraft.wiki", "*.minecraft.wiki"]
            requests_per_minute = 100
            api_calls_per_minute = 10
            megabytes_per_minute = 5
            "#,
        )
        .unwrap();
        let site = &cfg.resolved_sites().unwrap()[0];
        assert_eq!((site.limits.requests_per_minute, site.limits.megabytes_per_minute), (100, 5));
        assert_eq!(site.limits.api_calls_per_minute, 10);
        assert_eq!(site.limits.connections_per_minute, SiteLimits::default().connections_per_minute);

        for (entry, needle) in [
            (r#"{ name = "w", start = "https://a.example/", hosts = ["b.example"] }"#, "not among the site's hosts"),
            (r#"{ name = "w", start = "http://a.example/", hosts = ["a.example"] }"#, "https://"),
            (r#"{ name = "w", start = "https://a.example:8443/", hosts = ["a.example"] }"#, "https://"),
            (r#"{ name = "w", start = "https://u@a.example/", hosts = ["a.example"] }"#, "https://"),
            (r#"{ name = "w", start = "https://a.example/ x", hosts = ["a.example"] }"#, "printable"),
            (r#"{ name = "W", start = "https://a.example/", hosts = ["a.example"] }"#, "site name"),
            (r#"{ name = "seventeen-letters", start = "https://a.example/", hosts = ["a.example"] }"#, "site name"),
            (r#"{ name = "w", start = "https://a.example/", hosts = [] }"#, "1 to 32 hosts"),
            (r#"{ name = "w", start = "https://a.example/", hosts = ["*"] }"#, "first label"),
            (r#"{ name = "w", start = "https://a.example/", hosts = ["*.com"] }"#, "two labels"),
            (r#"{ name = "w", start = "https://a.example/", hosts = ["a.*.example"] }"#, "first label"),
            (r#"{ name = "w", start = "https://1.2.3.4/", hosts = ["1.2.3.4"] }"#, "address"),
            (r#"{ name = "w", start = "https://a.example/" }"#, "needs `hosts`"),
            (r#"{ preset = "discord", hosts = ["evil.example"] }"#, "fixed start page"),
            (r#"{ preset = "slack" }"#, "no preset"),
            (r#"{ preset = "discord", requests_per_minute = 0 }"#, "between 1 and"),
            (r#"{ preset = "discord", api_calls_per_minute = 100001 }"#, "api_calls_per_minute must be between 1 and"),
            (r#"{ preset = "discord", title = "a\u0007b" }"#, "control"),
        ] {
            let err = parse(&format!("sites = [{entry}]")).unwrap_err().to_string();
            assert!(err.contains(needle), "{entry}: {err}");
        }
        let err = parse("sites = [{ preset = \"discord\" }, { preset = \"discord\" }]").unwrap_err().to_string();
        assert!(err.contains("listed twice"), "{err}");
        // A second Discord under another name is a different site.
        parse("sites = [{ preset = \"discord\" }, { preset = \"discord\", name = \"work\" }]").unwrap();
        let err = parse("sites = [{ preset = \"discord\", hostz = [] }]").unwrap_err().to_string();
        assert!(err.contains("unknown field"), "{err}");
    }

    #[test]
    fn the_number_of_sites_is_capped() {
        let entries: Vec<String> = (0..=MAX_SITES)
            .map(|i| format!("{{ name = \"s{i}\", start = \"https://s{i}.example/\", hosts = [\"s{i}.example\"] }}"))
            .collect();
        let err = parse(&format!("sites = [{}]", entries.join(", "))).unwrap_err().to_string();
        assert!(err.contains("at most 8"), "{err}");
        parse(&format!("sites = [{}]", entries[..MAX_SITES].join(", "))).unwrap();
    }

    #[test]
    fn frame_rate_quality_and_size_are_bounded() {
        for (line, needle) in [
            ("frame_rate = 16", "frame_rate"),
            ("frame_rate = 0", "frame_rate"),
            ("quality = 95", "quality"),
            ("width = 2000", "width"),
            ("height = 100", "height"),
            ("memory_max_mb = 100", "memory_max_mb"),
            ("test_authorities = [\"x\"]", "unknown field"),
            ("self_test = true", "unknown field"),
        ] {
            let err = parse(line).unwrap_err().to_string();
            assert!(err.contains(needle), "{line}: {err}");
        }
        parse("frame_rate = 15\nquality = 90\nwidth = 1280\nheight = 160").unwrap();
    }

    #[test]
    fn host_patterns_match_whole_labels() {
        let below = HostPattern::parse("*.Example.COM.").unwrap();
        assert_eq!(below, HostPattern::Below("example.com".into()));
        assert_eq!(below.to_string(), "*.example.com");
        for yes in ["a.example.com", "a.b.example.com"] {
            assert!(below.matches(yes), "{yes}");
        }
        for no in ["example.com", "aexample.com", "example.com.evil", "a.example.co"] {
            assert!(!below.matches(no), "{no}");
        }
        let exact = HostPattern::parse("discord.com").unwrap();
        assert!(exact.matches("discord.com") && !exact.matches("a.discord.com") && !exact.matches("xdiscord.com"));
    }

    #[test]
    fn https_hosts_are_read_the_way_a_browser_sends_them() {
        assert_eq!(https_host("https://Discord.com/app").as_deref(), Some("discord.com"));
        assert_eq!(https_host("HTTPS://a.example:443?x").as_deref(), Some("a.example"));
        assert_eq!(https_host("https://a.example#frag").as_deref(), Some("a.example"));
        for no in [
            "http://a.example/",
            "https://a.example:444/",
            "https://user@a.example/",
            "https:///x",
            "https://",
            "wss://a.example/",
            "about:blank",
            "https://a..b/",
        ] {
            assert_eq!(https_host(no), None, "{no}");
        }
    }
}
