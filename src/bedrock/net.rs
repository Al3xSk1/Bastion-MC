//! The Bedrock network profile (owned by the network package).
//!
//! Contract: Bedrock sessions use filtered mode only. The relay passes HTTPS by server name
//! (SNI, no TLS termination) for the names below, RakNet over UDP to the servers the user
//! listed (one rule per server, names resolved to synthetic addresses as for Java), and
//! nothing else — except Realms and event servers, whose join addresses arrive as IP literals
//! inside TLS replies: those replies are terminated by the launcher (a per-launch CA in the
//! sandbox's trust store), and each join is rewritten to a one-time pass address and approved
//! outside the game (`[realms] approve`). LAN and NetherNet (WebRTC: friends' worlds,
//! NetherNet Realms) are off in this version and refused with a reason.
//!
//! The lists here are the research's starting point (`target/research/REPORT.md` §4), refined
//! by the network package (the deny list, the terminated hosts). What enforces them is the
//! relay's Bedrock profile (`egress::bedrock`, `egress::raknet`, `egress::udp`,
//! `egress::services`); the entry point for a launch is [`prepare`] and [`Prepared::start`].
//!
//! **The two calls of a launch.** [`prepare`] comes first, where the Java launch builds its
//! `egress::Policy` (step 6): it checks the plan, makes the per-launch certificate authority (its
//! certificate goes into the sandbox's trust store, so it must exist before the sandbox does)
//! and the approvals, and says which sockets the namespace helper creates and which ports
//! Landlock opens. [`Prepared::start`] comes where the Java launch calls `egress::start` — in
//! step 10, *after* the launcher has confined itself — because from then on the relay reads what
//! the game sends, and nothing the game sends is read by a launcher that is not confined yet.
//! Nothing `start` does later needs what step 9 takes away: TCP to port 443 is in the
//! confinement ([`Prepared::confinement_needs`]), UDP is not restricted by it, and name lookups
//! stay possible.

use std::{os::fd::OwnedFd, sync::Arc};

use super::{BedrockConfig, BedrockServer};
use crate::{
    confine::Confinement,
    egress::{self, Logger, Stats, broker::Authority, services::Services},
    error::Result,
    net::http::HttpClient,
    realms::{
        Approve,
        approval::{Announcer, Answer, Approvals},
    },
    sandbox::netns::NetnsSpec,
};

/// HTTPS names the game reaches by suffix (a leading dot: subdomains only).
pub const SNI_SUFFIXES: &[&str] = &[".xboxlive.com", ".minecraft-services.net", ".playfabapi.com"];

/// HTTPS names the game reaches exactly.
pub const SNI_EXACT: &[&str] = &["login.live.com", "multiplayer.minecraft.net", "pocket.realms.minecraft.net"];

/// Microsoft's telemetry, reachable only with `[bedrock] telemetry = true`.
pub const TELEMETRY_SUFFIXES: &[&str] = &[".events.data.microsoft.com"];
pub const TELEMETRY_EXACT: &[&str] = &["vortex.data.microsoft.com"];

/// Names a Bedrock session never reaches, whatever a suffix says: the Java services (a Bedrock
/// game has no business there, and the Java token never exists in a Bedrock session) — among
/// them the Java Realms hosts, two of which lie under `.minecraft-services.net` — and NetherNet's
/// signalling (friends' worlds and NetherNet Realms over WebRTC, which this version does not
/// relay: refused at the name, with a reason, rather than failing later at addresses nobody can
/// reach). A leading dot would deny a whole sub-tree; none does yet.
pub const DENIED: &[&str] = &[
    "api.minecraftservices.com",
    "textures.minecraft.net",
    "sessionserver.mojang.com",
    "pc.realms.minecraft.net",
    "java.frontendlegacy.realms.minecraft-services.net",
    "java.frontend.realms.minecraft-services.net",
    NETHERNET_SIGNALLING,
];

/// WebRTC signalling for NetherNet (research §4.1).
pub const NETHERNET_SIGNALLING: &str = "signal.franchise.minecraft-services.net";

/// Why a name on [`DENIED`] is refused, for the one log line that says so.
pub fn deny_reason(name: &str) -> &'static str {
    if name == NETHERNET_SIGNALLING {
        "NetherNet signalling: friends' worlds and NetherNet Realms (WebRTC) are not relayed in this version"
    } else {
        "a Java Edition service: a Bedrock session never reaches it (and holds no Java credential)"
    }
}

/// The Realms API hosts whose HTTPS the launcher terminates when joins are brokered: the one
/// current clients use (gophertunnel's `realmsBaseURL`), and the older one that is also the
/// relying party of the game's Realms token (in [`SNI_EXACT`]). Only to read and rewrite join
/// answers: the game's own token passes through, the launcher holds none.
pub const REALMS_HOSTS: &[&str] =
    &["bedrock.frontendlegacy.realms.minecraft-services.net", "pocket.realms.minecraft.net"];

/// The gatherings service, which hands out event servers ("experiences"): the host
/// gophertunnel's default environment names, and the one without the `-secondary` (the game
/// learns the real one from `client.discovery.minecraft-services.net`; both are terminated so
/// that either is brokered).
pub const GATHERINGS_HOSTS: &[&str] =
    &["gatherings-secondary.franchise.minecraft-services.net", "gatherings.franchise.minecraft-services.net"];

/// What the relay of a Bedrock session allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetPlan {
    pub sni_exact: Vec<String>,
    pub sni_suffixes: Vec<String>,
    pub denied: Vec<String>,
    /// RakNet servers: the global list, then the instance's own, without duplicates.
    pub udp_servers: Vec<BedrockServer>,
}

/// The plan for one launch. Validation of the servers (names, literals, ports) belongs to
/// the network package; this only assembles.
pub fn plan(cfg: &BedrockConfig, instance_servers: &[BedrockServer]) -> NetPlan {
    let mut sni_exact: Vec<String> = SNI_EXACT.iter().map(|s| s.to_string()).collect();
    let mut sni_suffixes: Vec<String> = SNI_SUFFIXES.iter().map(|s| s.to_string()).collect();
    if cfg.telemetry {
        sni_exact.extend(TELEMETRY_EXACT.iter().map(|s| s.to_string()));
        sni_suffixes.extend(TELEMETRY_SUFFIXES.iter().map(|s| s.to_string()));
    }
    let mut udp_servers: Vec<BedrockServer> = Vec::new();
    for server in cfg.servers.iter().chain(instance_servers) {
        if !udp_servers.contains(server) {
            udp_servers.push(server.clone());
        }
    }
    NetPlan { sni_exact, sni_suffixes, denied: DENIED.iter().map(|s| s.to_string()).collect(), udp_servers }
}

// ---------------------------------------------------------------- the entry point

/// The alias of the per-launch certificate authority in the sandbox's trust store.
pub const TRUST_ALIAS: &str = "bastion-mc-bedrock-broker";

/// What the launch hands the network package for one Bedrock session.
pub struct Options<'a> {
    /// `[bedrock]`: `telemetry` and the global `servers`.
    pub cfg: &'a BedrockConfig,
    /// The instance's own `[bedrock] servers`.
    pub instance_servers: &'a [BedrockServer],
    /// `[realms] approve`. `off`: Realms and event-server joins are refused (nothing is
    /// terminated, no authority is made). `code` or `totp`: joins are brokered, and each is
    /// approved with the launcher's six-digit code typed into a trusted input — `totp` included,
    /// because its secret lives in the token store, which a Bedrock session never opens.
    pub approve: Approve,
    /// `network.launcher_proxy` is set: the launcher reaches HTTPS through it, never directly.
    pub proxied: bool,
    /// Shows a join waiting for approval, and its end, outside the game. The Java launch's
    /// announcer (`launch::prepare_realms`: a log line and the `RealmsApproval` event) fits as it
    /// is.
    pub announce: Announcer,
}

/// The network part of a Bedrock session, before the sandbox exists.
pub struct Prepared {
    policy: egress::Policy,
    authority: Option<Authority>,
    approvals: Option<Arc<Approvals>>,
    notes: Vec<String>,
    loopback_for_tests: bool,
}

/// Check the plan and make what must exist before the sandbox: the relay's policy, and — when
/// joins are brokered — the per-launch certificate authority and the approvals. Fails on a
/// malformed `[bedrock] servers` entry, naming it.
pub fn prepare(options: Options<'_>) -> Result<Prepared> {
    let plan = plan(options.cfg, options.instance_servers);
    let joins = options.approve != Approve::Off;
    let policy = egress::Policy::bedrock(&plan, joins, options.proxied)?;
    let mut notes = Vec::new();
    let (authority, approvals) = if joins {
        if options.approve == Approve::Totp {
            notes.push(
                "realms: [realms] approve = \"totp\" — in a Bedrock session each join is approved with the launcher's \
                 six-digit code instead: the TOTP secret is in the token store, which a Bedrock session never opens"
                    .to_string(),
            );
        }
        let authority = Authority::generate_constrained("Bedrock service broker", &Services::domains())?;
        let approvals = Approvals::new(Approve::Code, None, egress::bedrock::PASS, options.announce);
        (Some(authority), Some(approvals))
    } else {
        (None, None)
    };
    Ok(Prepared { policy, authority, approvals, notes, loopback_for_tests: false })
}

impl Prepared {
    /// The sockets the namespace helper creates: `sandbox::Network::Filtered(this)`. The same
    /// value goes to [`Prepared::start`].
    pub fn netns_spec(&self) -> NetnsSpec {
        self.policy.netns_spec()
    }

    /// TCP ports the game may connect to (Landlock `TcpPolicy::Ports`): 443, when any name is
    /// reachable.
    pub fn sandbox_tcp_ports(&self) -> Vec<u16> {
        self.policy.tcp_ports()
    }

    /// UDP ports the game may send to (Landlock `udp_send`): DNS, the servers' ports and the
    /// join pass's. Needs Landlock ABI 10; below it the network namespace is the only barrier.
    pub fn sandbox_udp_ports(&self) -> Vec<u16> {
        self.policy.sandbox_udp_ports()
    }

    /// The per-launch authority for the sandbox's trust store, as `sandbox::trust::TrustStore::build`
    /// takes it (alias, DER), when joins are brokered.
    pub fn trust_anchor(&self) -> Option<(&'static str, &[u8])> {
        self.authority.as_ref().map(|a| (TRUST_ALIAS, a.certificate_der()))
    }

    /// The same certificate as PEM, for a bundle written as text.
    pub fn trust_pem(&self) -> Option<String> {
        self.authority.as_ref().map(Authority::certificate_pem)
    }

    /// TCP ports the relay itself connects out to, for the launcher's confinement: 443 (the
    /// services it passes through or terminates), unless a proxy carries that.
    pub fn outbound_tcp_ports(&self) -> Vec<u16> {
        self.policy.outbound_ports(&egress::SrvTargets::new())
    }

    /// Add what the relay keeps after step 9 to the launcher's confinement: its outbound TCP
    /// ports. (Nothing else: UDP is not the confinement's to restrict, and nothing is written.)
    pub fn confinement_needs(&self, keep: &mut Confinement) {
        keep.tcp_connect.extend(self.outbound_tcp_ports());
        keep.tcp_connect.sort_unstable();
        keep.tcp_connect.dedup();
    }

    /// The approvals of this launch, when joins are brokered: what a trusted input answers
    /// ([`answer_join`]). Available from here on, so that the input can be wired before the game
    /// starts.
    pub fn approvals(&self) -> Option<Arc<Approvals>> {
        self.approvals.clone()
    }

    /// The launch log's lines about the network of this session.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = self.policy.describe();
        if self.authority.is_some() {
            lines.push(format!(
                "network: Realms and the gatherings service ({} host(s)) are TLS-terminated by the launcher to read join \
                 answers — with an authority made for this launch, in the sandbox's trust store only, valid for those \
                 hosts only; the game's own token passes through, the launcher holds none",
                Services::domains().len()
            ));
        }
        lines.extend(self.notes.iter().cloned());
        lines
    }

    /// Test support, never set by a launch: servers and approved joins on the loopback address
    /// (the tests' stand-ins) are accepted where real ones must be public.
    #[doc(hidden)]
    pub fn allowing_loopback_for_tests(mut self) -> Self {
        self.policy = self.policy.allowing_private_bedrock_servers_for_tests();
        self.loopback_for_tests = true;
        self
    }

    /// Start serving the sockets the namespace helper handed over (`spec` is
    /// [`Prepared::netns_spec`], `sockets` what the helper sent, in its order). `http` is the
    /// launcher's own client (system roots, built-in pins, TLS 1.3, the proxy if one is set);
    /// `log` receives the relay's lines. Call it where the Java launch calls `egress::start`.
    pub fn start(self, spec: &NetnsSpec, sockets: Vec<OwnedFd>, http: HttpClient, log: Logger) -> Result<Running> {
        let services = match (self.authority, &self.approvals) {
            (Some(authority), Some(approvals)) => {
                let services = Services::new(authority, &http, approvals.clone())?;
                Some(if self.loopback_for_tests { services.allowing_loopback_for_tests() } else { services })
            }
            _ => None,
        };
        let egress = egress::start_bedrock(self.policy, spec, sockets, http, log, services)?;
        Ok(Running { egress, approvals: self.approvals })
    }
}

/// The network of a running Bedrock session.
pub struct Running {
    egress: egress::Egress,
    approvals: Option<Arc<Approvals>>,
}

impl Running {
    pub fn stats(&self) -> &Arc<Stats> {
        &self.egress.stats
    }

    pub fn approvals(&self) -> Option<&Arc<Approvals>> {
        self.approvals.as_ref()
    }

    /// The launch log's summary at the end of the session.
    pub fn summary(&self) -> Vec<String> {
        use std::sync::atomic::Ordering::Relaxed;
        let s = &self.egress.stats;
        let mut lines = vec![
            format!(
                "egress summary: {} connection(s) relayed, {} denied, {} name lookup(s) refused",
                s.relayed.load(Relaxed),
                s.denied.load(Relaxed),
                s.names_refused.load(Relaxed)
            ),
            format!(
                "egress summary, RakNet: {} datagram(s) relayed in {} flow(s), {} refused",
                s.udp_relayed.load(Relaxed),
                s.udp_flows.load(Relaxed),
                s.udp_refused.load(Relaxed)
            ),
        ];
        if self.approvals.is_some() {
            lines.push(format!(
                "join broker: {} request(s) forwarded to Realms or the gatherings service, {} answered here",
                s.brokered.load(Relaxed),
                s.broker_refused.load(Relaxed)
            ));
        }
        lines
    }

    pub fn shutdown(self) {
        self.egress.shutdown();
    }
}

/// What `network.launcher_proxy` carries in a Bedrock session, for the launch log (the Java
/// launch's `egress::describe_proxy` speaks of the credential broker and skin textures, which a
/// Bedrock session does not have). Exact, because "my traffic goes through Tor" is a claim people
/// rely on.
pub fn describe_proxy(proxy: std::net::SocketAddr) -> String {
    format!(
        "network: through the SOCKS5 proxy at {proxy}, which also resolves the names: the game's HTTPS that the relay \
         passes through (Xbox Live, the Minecraft services, PlayFab, the sign-in host) and the launcher's own requests \
         to Realms and the gatherings service. NOT through the proxy: RakNet (UDP) to the Bedrock servers and to an \
         approved join, and the lookup of the servers' names (your resolver sees those) — all of these come from your \
         address"
    )
}

/// What a trusted input — the terminal of a terminal launch, the front end's `answer realms
/// <text>` line in event mode, never the game — says about a waiting join: the six digits shown
/// with it, or `cancel`. Anything else counts as a wrong code (a guess is a guess). Returns the
/// sentence to show the user.
pub fn answer_join(approvals: &Approvals, text: &str) -> String {
    let text = text.trim();
    if text.eq_ignore_ascii_case("cancel") {
        return if approvals.cancel() { "the waiting join was cancelled" } else { "no join is waiting" }.into();
    }
    let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let answer = if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit()) {
        approvals.answer(&digits)
    } else {
        approvals.answer("x")
    };
    match answer {
        Answer::Approved => "approved: the game may now join that server, once".into(),
        Answer::Wrong { attempts_left } => format!("wrong code ({attempts_left} attempt(s) left)"),
        Answer::Refused => "wrong code: the join was refused".into(),
        Answer::NoPrompt => "no join is waiting for a code".into(),
        Answer::Locked => "too many wrong codes: joins are refused until the next launch".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_hosts_are_in_the_plan_only_when_telemetry_is_on() {
        let on = plan(&BedrockConfig::default(), &[]);
        assert!(on.sni_exact.iter().any(|h| h == "vortex.data.microsoft.com"));
        let off = plan(&BedrockConfig { telemetry: false, ..BedrockConfig::default() }, &[]);
        assert!(!off.sni_exact.iter().any(|h| h == "vortex.data.microsoft.com"));
        assert!(!off.sni_suffixes.iter().any(|h| h.ends_with("data.microsoft.com")));
    }

    #[test]
    fn the_java_services_are_denied_even_where_a_suffix_would_reach_them() {
        let plan = plan(&BedrockConfig::default(), &[]);
        let rules = crate::egress::bedrock::SniRules::new(&plan.sni_exact, &plan.sni_suffixes, &plan.denied).unwrap();
        for name in DENIED {
            assert!(plan.denied.iter().any(|d| d == name));
            assert!(!plan.sni_exact.iter().any(|e| e == name));
            // Two Java Realms hosts and the NetherNet signalling host lie under
            // `.minecraft-services.net`: the deny list wins (the relay checks it first).
            assert!(!rules.allows(name), "{name}");
        }
        for name in ["api.minecraftservices.com", "textures.minecraft.net", "sessionserver.mojang.com"] {
            assert!(!plan.sni_suffixes.iter().any(|s| name.ends_with(s.as_str())), "{name}");
            assert!(deny_reason(name).contains("Java"));
        }
        assert!(deny_reason(NETHERNET_SIGNALLING).contains("NetherNet"));
    }

    #[test]
    fn the_terminated_hosts_are_reachable_names() {
        let plan = plan(&BedrockConfig::default(), &[]);
        let rules = crate::egress::bedrock::SniRules::new(&plan.sni_exact, &plan.sni_suffixes, &plan.denied).unwrap();
        for host in REALMS_HOSTS.iter().chain(GATHERINGS_HOSTS) {
            assert!(rules.allows(host), "{host} must be reachable at all to be brokered");
        }
    }

    fn options<'a>(
        cfg: &'a BedrockConfig,
        approve: Approve,
        proxied: bool,
        heard: Arc<std::sync::Mutex<Vec<crate::realms::approval::Announcement>>>,
    ) -> Options<'a> {
        Options {
            cfg,
            instance_servers: &[],
            approve,
            proxied,
            announce: Arc::new(move |a: &crate::realms::approval::Announcement| heard.lock().unwrap().push(a.clone())),
        }
    }

    #[test]
    fn without_approvals_nothing_is_terminated_and_joins_are_refused_with_the_reason() {
        let cfg = BedrockConfig {
            servers: vec![BedrockServer { host: "play.example.org".into(), port: 19133 }],
            ..BedrockConfig::default()
        };
        let prepared = prepare(options(&cfg, Approve::Off, false, Arc::default())).unwrap();
        assert!(prepared.trust_anchor().is_none() && prepared.trust_pem().is_none() && prepared.approvals().is_none());
        assert_eq!(prepared.sandbox_tcp_ports(), [443]);
        assert_eq!(prepared.sandbox_udp_ports(), [53, 19133], "no pass without joins");
        assert_eq!(prepared.netns_spec().udp_ports, [19133]);
        let text = prepared.describe().join("\n");
        assert!(text.contains("joins are refused ([realms] approve = \"off\")"), "{text}");
        assert!(!text.contains("TLS-terminated"), "{text}");
        let mut keep = Confinement::default();
        prepared.confinement_needs(&mut keep);
        assert_eq!(keep.tcp_connect, [443]);
        // Behind a proxy the launcher never dials 443 itself.
        let proxied = prepare(options(&cfg, Approve::Off, true, Arc::default())).unwrap();
        let mut keep = Confinement::default();
        proxied.confinement_needs(&mut keep);
        assert!(keep.tcp_connect.is_empty());
    }

    #[test]
    fn with_approvals_an_authority_exists_for_the_trust_store_and_totp_becomes_the_code() {
        let cfg = BedrockConfig::default();
        let prepared = prepare(options(&cfg, Approve::Totp, false, Arc::default())).unwrap();
        let (alias, der) = prepared.trust_anchor().expect("an authority");
        assert_eq!(alias, TRUST_ALIAS);
        assert!(der.len() > 100 && der[0] == 0x30, "a DER certificate");
        let pem = prepared.trust_pem().unwrap();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----\n") && pem.ends_with("-----END CERTIFICATE-----\n"));
        assert!(pem.lines().all(|l| l.len() <= 64));
        assert_eq!(prepared.approvals().unwrap().mode(), Approve::Code);
        assert_eq!(prepared.approvals().unwrap().pass_address(), crate::egress::bedrock::PASS);
        assert_eq!(prepared.sandbox_udp_ports(), [53, 19132], "the pass's port");
        let text = prepared.describe().join("\n");
        assert!(text.contains("TLS-terminated") && text.contains("TOTP secret is in the token store"), "{text}");
        // Two launches never share an authority.
        let again = prepare(options(&cfg, Approve::Code, false, Arc::default())).unwrap();
        assert_ne!(again.trust_anchor().unwrap().1, der);
        assert!(!again.describe().join("\n").contains("TOTP"));
    }

    #[test]
    fn a_malformed_server_entry_stops_the_launch_with_its_name() {
        let cfg = BedrockConfig {
            servers: vec![BedrockServer { host: "bad host".into(), port: 19132 }],
            ..BedrockConfig::default()
        };
        let err = prepare(options(&cfg, Approve::Off, false, Arc::default())).err().unwrap().to_string();
        assert!(err.contains("bad host"), "{err}");
    }

    #[test]
    fn a_trusted_answer_approves_the_waiting_join_with_its_code_and_nothing_else_does() {
        let heard: Arc<std::sync::Mutex<Vec<crate::realms::approval::Announcement>>> = Arc::default();
        let cfg = BedrockConfig::default();
        let prepared = prepare(options(&cfg, Approve::Code, false, heard.clone())).unwrap();
        let approvals = prepared.approvals().unwrap();
        assert_eq!(answer_join(&approvals, "123456"), "no join is waiting for a code");
        assert_eq!(approvals.join_requested(7), crate::realms::approval::JoinStep::Forward);
        let address = crate::realms::table::Address { host: "203.0.113.9".into(), port: 19132 };
        approvals.prompt(7, address, b"{}".to_vec());
        let code = heard.lock().unwrap()[0].code.clone().expect("the code is shown outside the game");
        let wrong = if code == "000000" { "000001" } else { "000000" };
        assert_eq!(answer_join(&approvals, wrong), "wrong code (2 attempt(s) left)");
        assert_eq!(answer_join(&approvals, "12 34"), "wrong code (1 attempt(s) left)", "malformed is an attempt");
        assert_eq!(
            answer_join(&approvals, &format!(" {} {} ", &code[..3], &code[3..])),
            "approved: the game may now join that server, once"
        );
        // Until the game has been given the pass, the user may still change their mind.
        assert_eq!(answer_join(&approvals, "cancel"), "the waiting join was cancelled");
        assert_ne!(
            approvals.join_requested(7),
            crate::realms::approval::JoinStep::Deliver(b"{}".to_vec()),
            "a cancelled approval delivers nothing"
        );
        assert_eq!(answer_join(&approvals, "cancel"), "no join is waiting");
    }

    #[test]
    fn the_proxy_note_says_that_raknet_and_server_lookups_bypass_it() {
        let text = describe_proxy("127.0.0.1:9050".parse().unwrap());
        let (proxied, direct) = text.split_once("NOT through the proxy").unwrap();
        assert!(proxied.contains("127.0.0.1:9050") && proxied.contains("Realms") && !proxied.contains("RakNet"));
        assert!(direct.contains("RakNet (UDP)") && direct.contains("your resolver") && direct.contains("your address"));
    }

    #[test]
    fn an_instances_servers_follow_the_global_ones_without_duplicates() {
        let a = BedrockServer { host: "a.example.org".into(), port: 19132 };
        let b = BedrockServer { host: "b.example.org".into(), port: 19133 };
        let cfg = BedrockConfig { servers: vec![a.clone()], ..BedrockConfig::default() };
        assert_eq!(plan(&cfg, &[a.clone(), b.clone()]).udp_servers, vec![a, b]);
    }
}
