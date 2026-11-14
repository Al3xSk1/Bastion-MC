//! The Bedrock profile of the relay: what a Bedrock session may reach, and how it is said.
//!
//! A Bedrock session runs in the same network namespace without a route as a Java session, and
//! the same relay serves it. What differs is the policy (`bedrock::net::NetPlan`):
//!
//! * **HTTPS by name, from three lists** instead of Mojang's registrable domains: names reached
//!   exactly (`login.live.com`), suffixes with a leading dot that match sub-domains only
//!   (`.xboxlive.com` matches `user.auth.xboxlive.com`, never `xboxlive.com` or
//!   `evilxboxlive.com`), and a deny list that wins over both (the Java services; NetherNet's
//!   signalling). The TLS ClientHello must name exactly the name that was resolved, as for Java.
//!   Nothing is terminated except the two services a join comes from (`services`).
//! * **No TCP game servers.** Bedrock speaks RakNet over UDP; a TCP connection to anything but
//!   port 443 of an allowed name is refused.
//! * **RakNet per server rule.** Each `[bedrock] servers` entry is one UDP rule: its name (or
//!   literal address) and port. The datagram relay opens a flow to the address the launcher
//!   resolved for that name — there is no TCP connection to follow, so the rule itself is the
//!   anchor (`udp`) — and only with a datagram that starts a RakNet exchange (`raknet`).
//! * **Joins handed out by Realms and the gatherings service** are reached through the *pass*
//!   ([`PASS`]): an address inside the sandbox that leads, for one join (its first flow and at
//!   most three more within 30 s), to exactly the server whose join the user approved outside
//!   the game (`services`, `realms::approval`).
//! * **No private addresses.** A Bedrock server rule has no `allow_private` (the shared
//!   `BedrockServer` type has two fields, host and port): a server that is, or resolves to, a
//!   loopback, private or link-local address is refused with the reason that LAN servers are not
//!   supported in this version. LAN discovery (broadcasts to UDP 19132) and NetherNet (WebRTC:
//!   friends' worlds, NetherNet Realms) have no way out at all.

use std::net::{Ipv4Addr, SocketAddrV4};

use crate::{
    bedrock::net::{self as plan, NetPlan},
    config::{HostSpec, MIN_UDP_PORT, ServerRule, normalize_host},
    error::{Error, Result},
};

/// Where an approved Realms or event-server join is served inside the sandbox: near the top of
/// the range of synthetic addresses (the resolver hands them out from the bottom, at most 4096 of
/// them, and never gets near it; one below the Java pass), on RakNet's default port. The game is
/// given this address in place of the one Realms or the gatherings service named.
pub const PASS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(127, 127, 255, 253), crate::bedrock::DEFAULT_PORT);

/// Most servers one plan may list. Each named one is a name lookup at the start of the session.
pub const MAX_SERVERS: usize = 64;

/// The three name lists of a Bedrock session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SniRules {
    exact: Vec<String>,
    /// Each starts with a dot.
    suffixes: Vec<String>,
    /// Exact names, or suffixes with a leading dot.
    denied: Vec<String>,
}

fn bad_list(what: &str, entry: &str, why: &str) -> Error {
    Error::Config(format!("Bedrock network plan: {what} {entry:?}: {why}"))
}

/// A suffix entry: a dot, then a host name of at least two labels (".com" would open a whole
/// top-level domain).
fn check_suffix(what: &str, entry: &str) -> Result<String> {
    let rest = entry.strip_prefix('.').ok_or_else(|| bad_list(what, entry, "a suffix starts with a dot"))?;
    let rest = normalize_host(rest).map_err(|_| bad_list(what, entry, "not a host name after the dot"))?;
    if !rest.contains('.') {
        return Err(bad_list(what, entry, "a suffix needs at least two labels"));
    }
    Ok(format!(".{rest}"))
}

impl SniRules {
    pub fn new(exact: &[String], suffixes: &[String], denied: &[String]) -> Result<Self> {
        let exact = exact
            .iter()
            .map(|e| normalize_host(e).map_err(|_| bad_list("exact name", e, "not a host name")))
            .collect::<Result<Vec<_>>>()?;
        let suffixes = suffixes.iter().map(|s| check_suffix("suffix", s)).collect::<Result<Vec<_>>>()?;
        let denied = denied
            .iter()
            .map(|d| {
                if d.starts_with('.') {
                    check_suffix("denied suffix", d)
                } else {
                    normalize_host(d).map_err(|_| bad_list("denied name", d, "not a host name"))
                }
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(SniRules { exact, suffixes, denied })
    }

    /// Is this name on the deny list (exactly, or below a denied suffix)?
    pub fn denies(&self, name: &str) -> bool {
        self.denied.iter().any(|d| if d.starts_with('.') { name.ends_with(d.as_str()) } else { name == d })
    }

    /// May HTTPS reach this name? Exactly listed, or strictly below a listed suffix — and not
    /// denied. `name` is lower case without a trailing dot (the DNS and SNI readers make it so).
    pub fn allows(&self, name: &str) -> bool {
        !self.denies(name)
            && (self.exact.iter().any(|e| e == name)
                || self.suffixes.iter().any(|s| name.len() > s.len() && name.ends_with(s.as_str())))
    }

    pub fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.suffixes.is_empty()
    }
}

/// The Bedrock part of `egress::Policy`.
#[derive(Debug, Clone)]
pub struct Rules {
    pub(super) sni: SniRules,
    /// One per `[bedrock] servers` entry: the host, its UDP port as `port` and as the only entry
    /// of `udp`, never `allow_private` (the tests' stand-ins aside).
    pub(super) servers: Vec<ServerRule>,
    /// The pass, when joins from Realms and the gatherings service are brokered.
    pub(super) pass: Option<SocketAddrV4>,
    /// `[bedrock] telemetry`: whether Microsoft's telemetry hosts are on the lists (for the reason
    /// given when one is refused).
    pub(super) telemetry: bool,
}

/// Check a plan and turn it into rules. `joins`: Realms and event-server joins are brokered in
/// this session (there is a pass).
pub fn rules(plan: &NetPlan, joins: bool) -> Result<Rules> {
    let sni = SniRules::new(&plan.sni_exact, &plan.sni_suffixes, &plan.denied)?;
    if plan.udp_servers.len() > MAX_SERVERS {
        return Err(Error::Config(format!(
            "[bedrock] servers: {} servers listed (global and instance together); at most {MAX_SERVERS}",
            plan.udp_servers.len()
        )));
    }
    let mut servers: Vec<ServerRule> = Vec::new();
    for server in &plan.udp_servers {
        let bad =
            |why: &str| Error::Config(format!("[bedrock] servers: {:?} port {}: {why}", server.host, server.port));
        if server.port < MIN_UDP_PORT {
            return Err(bad(&format!("only ports from {MIN_UDP_PORT} up are relayed")));
        }
        let text = server.host.trim();
        let host = match text.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(ip)) => {
                if super::FakeIps::contains(ip) {
                    return Err(bad("the address lies in the range reserved for name mapping"));
                }
                if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
                    return Err(bad("not the address of a server"));
                }
                HostSpec::Ip(ip)
            }
            Ok(std::net::IpAddr::V6(_)) => {
                return Err(bad("IPv6 literals are not supported; use a host name (it may resolve to IPv6)"));
            }
            Err(_) => {
                let name = normalize_host(text).map_err(|_| bad("not a host name or an IPv4 address"))?;
                if sni.denies(&name) {
                    return Err(bad("this name is on the deny list of Bedrock sessions"));
                }
                HostSpec::Name(name)
            }
        };
        let rule =
            ServerRule { host, port: server.port, explicit_port: true, allow_private: false, udp: vec![server.port] };
        if !servers.contains(&rule) {
            servers.push(rule);
        }
    }
    let telemetry = plan.sni_exact.iter().any(|e| plan::TELEMETRY_EXACT.contains(&e.as_str()));
    Ok(Rules { sni, servers, pass: joins.then_some(PASS), telemetry })
}

impl Rules {
    /// Why a name lookup was refused, phrased so that the user knows what to add — once per name
    /// in the log (the responder keeps the set).
    pub fn name_refusal(&self, name: &str) -> String {
        if self.sni.denies(name) {
            return plan::deny_reason(name).to_string();
        }
        let telemetry = plan::TELEMETRY_EXACT.contains(&name)
            || plan::TELEMETRY_SUFFIXES.iter().any(|s| name.len() > s.len() && name.ends_with(s));
        if telemetry && !self.telemetry {
            return "Microsoft telemetry, switched off ([bedrock] telemetry = false)".into();
        }
        format!(
            "not allowlisted for Bedrock; if it is a server you want to join, add {{ host = \"{name}\" }} (and its \
             port, if it is not {}) to [bedrock] servers in config.toml or to the instance's [bedrock] servers",
            crate::bedrock::DEFAULT_PORT
        )
    }

    /// The launch log's description.
    pub fn describe(&self, limits: &super::udp::Limits) -> Vec<String> {
        let mut lines = vec![format!(
            "network: filtered (Bedrock) — HTTPS by name only: {} name(s) exactly, below {}{}; never: {}",
            self.sni.exact.len(),
            self.sni.suffixes.join(", "),
            if self.telemetry { "" } else { " (Microsoft telemetry switched off)" },
            self.sni.denied.join(", ")
        )];
        if self.servers.is_empty() {
            lines.push(
                "network: no Bedrock servers listed ([bedrock] servers): multiplayer reaches only an approved Realms or \
                 event-server join"
                    .into(),
            );
        }
        for rule in &self.servers {
            lines.push(format!(
                "network: Bedrock server {}:{} — RakNet over UDP, to the address this machine resolves for it; a flow \
                 opens only with a RakNet ping or connection request",
                rule.host, rule.port
            ));
        }
        lines.push(format!(
            "network: RakNet limits — {} flows at most, datagrams up to {} bytes, {} a second to a server and {} a \
             second from it, per flow; a flow the game leaves silent for {} s ends",
            limits.max_flows,
            limits.max_datagram,
            limits.to_server.per_second,
            limits.to_game.per_second,
            limits.idle.as_secs()
        ));
        lines.push(match self.pass {
            Some(pass) => format!(
                "network: Realms and event-server joins — brokered; each needs your approval outside the game, then the \
                 game may reach that one server, once, through {pass}"
            ),
            None => "network: Realms and event-server joins are refused ([realms] approve = \"off\")".into(),
        });
        lines.push(
            "network: LAN games (broadcast discovery) and NetherNet (WebRTC: friends' worlds, NetherNet Realms) are not \
             relayed in this version"
                .into(),
        );
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bedrock::{BedrockConfig, BedrockServer};

    fn names() -> SniRules {
        let plan = plan::plan(&BedrockConfig::default(), &[]);
        SniRules::new(&plan.sni_exact, &plan.sni_suffixes, &plan.denied).unwrap()
    }

    #[test]
    fn suffixes_match_sub_domains_only_and_label_exactly() {
        let sni = names();
        for ok in [
            "user.auth.xboxlive.com",
            "a.b.c.xboxlive.com",
            "client.discovery.minecraft-services.net",
            "b980a380.playfabapi.com",
            "login.live.com",
            "multiplayer.minecraft.net",
        ] {
            assert!(sni.allows(ok), "{ok}");
        }
        for no in [
            "xboxlive.com",          // the suffix itself is not below it
            "evilxboxlive.com",      // not on a label boundary
            "xboxlive.com.evil.org", // ends elsewhere
            "live.com",              // an exact name is exact
            "www.login.live.com",    // …in both directions
            "minecraft.net",         // only multiplayer. is listed
            "api.minecraftservices.com",
            "textures.minecraft.net",
            "",
        ] {
            assert!(!sni.allows(no), "{no}");
        }
    }

    #[test]
    fn the_deny_list_wins_over_every_suffix() {
        let sni = names();
        for denied in plan::DENIED {
            assert!(sni.denies(denied) && !sni.allows(denied), "{denied}");
        }
        // Under an allowed suffix, and still refused: the Java Realms hosts and NetherNet's
        // signalling.
        for name in ["java.frontendlegacy.realms.minecraft-services.net", "signal.franchise.minecraft-services.net"] {
            assert!(sni.denies(name) && !sni.allows(name), "{name}");
            assert!(name.ends_with(".minecraft-services.net"));
        }
        // A denied suffix covers what is below it.
        let rules = SniRules::new(&[], &[".example.org".into()], &[".bad.example.org".into()]).unwrap();
        assert!(rules.allows("good.example.org") && !rules.allows("x.bad.example.org"));
    }

    #[test]
    fn lists_that_would_open_too_much_are_refused() {
        for (exact, suffix, denied) in [
            (vec!["a b".to_string()], vec![], vec![]),
            (vec![], vec!["xboxlive.com".to_string()], vec![]), // no dot: would read as exact
            (vec![], vec![".com".to_string()], vec![]),         // a whole top-level domain
            (vec![], vec![".".to_string()], vec![]),
            (vec![], vec![], vec!["..x".to_string()]),
        ] {
            assert!(SniRules::new(&exact, &suffix, &denied).is_err(), "{exact:?} {suffix:?} {denied:?}");
        }
    }

    fn plan_with(servers: Vec<BedrockServer>) -> NetPlan {
        plan::plan(&BedrockConfig { servers, ..BedrockConfig::default() }, &[])
    }

    fn server(host: &str, port: u16) -> BedrockServer {
        BedrockServer { host: host.into(), port }
    }

    #[test]
    fn servers_become_one_udp_rule_each_and_bad_entries_stop_the_launch() {
        let built = rules(
            &plan_with(vec![
                server("Play.Example.org", 19132),
                server("203.0.113.9", 19133),
                server("play.example.org", 19132),
            ]),
            true,
        )
        .unwrap();
        assert_eq!(
            built.servers,
            vec![
                ServerRule {
                    host: HostSpec::Name("play.example.org".into()),
                    port: 19132,
                    explicit_port: true,
                    allow_private: false,
                    udp: vec![19132]
                },
                ServerRule {
                    host: HostSpec::Ip(Ipv4Addr::new(203, 0, 113, 9)),
                    port: 19133,
                    explicit_port: true,
                    allow_private: false,
                    udp: vec![19133]
                },
            ],
            "names normalised, duplicates merged"
        );
        assert_eq!(built.pass, Some(PASS));
        assert!(rules(&plan_with(vec![]), false).unwrap().pass.is_none());
        for (bad, why) in [
            (server("play.example.org", 53), "from 1024"),
            (server("127.64.0.7", 19132), "reserved for name mapping"),
            (server("127.127.255.253", 19132), "reserved for name mapping"),
            (server("2001:db8::1", 19132), "IPv6"),
            (server("0.0.0.0", 19132), "not the address of a server"),
            (server("255.255.255.255", 19132), "not the address of a server"),
            (server("224.0.0.1", 19132), "not the address of a server"),
            (server("a b", 19132), "not a host name"),
            (server("api.minecraftservices.com", 19132), "deny list"),
        ] {
            let message = rules(&plan_with(vec![bad.clone()]), false).unwrap_err().to_string();
            assert!(message.contains(why), "{bad:?}: {message}");
        }
        let many: Vec<BedrockServer> = (0..=MAX_SERVERS).map(|i| server(&format!("s{i}.example.org"), 19132)).collect();
        assert!(rules(&plan_with(many), false).unwrap_err().to_string().contains("at most 64"));
    }

    #[test]
    fn a_refused_name_says_what_to_add_or_why_it_never_can_be() {
        let on = rules(&plan_with(vec![]), true).unwrap();
        let line = on.name_refusal("play.example.org");
        assert!(line.contains("{ host = \"play.example.org\" }") && line.contains("[bedrock] servers"), "{line}");
        assert!(on.name_refusal("api.minecraftservices.com").contains("Java"), "a Java service");
        assert!(on.name_refusal("signal.franchise.minecraft-services.net").contains("NetherNet"));
        let off =
            rules(&plan::plan(&BedrockConfig { telemetry: false, ..BedrockConfig::default() }, &[]), true).unwrap();
        assert!(off.name_refusal("vortex.data.microsoft.com").contains("telemetry = false"));
        assert!(off.name_refusal("browser.events.data.microsoft.com").contains("telemetry = false"));
        assert!(!on.name_refusal("vortex.data.microsoft.com").contains("telemetry = false"), "listed when on");
    }

    #[test]
    fn the_pass_is_a_synthetic_address_the_resolver_never_hands_out() {
        assert!(super::super::FakeIps::contains(*PASS.ip()));
        assert_ne!(PASS.ip(), crate::realms::PASS_ADDRESS.ip(), "not the Java pass");
        let mut fake = super::super::FakeIps::new();
        let last = (0..4096).map(|i| fake.assign(&format!("h{i}.example")).unwrap()).last().unwrap();
        assert!(u32::from(last) < u32::from(*PASS.ip()));
    }
}
