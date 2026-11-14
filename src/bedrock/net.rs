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
//! The lists here are the research's starting point (`target/research/REPORT.md` §4); the
//! network package refines them and owns everything that enforces them.

use super::{BedrockConfig, BedrockServer};

/// HTTPS names the game reaches by suffix (a leading dot: subdomains only).
pub const SNI_SUFFIXES: &[&str] = &[".xboxlive.com", ".minecraft-services.net", ".playfabapi.com"];

/// HTTPS names the game reaches exactly.
pub const SNI_EXACT: &[&str] = &["login.live.com", "multiplayer.minecraft.net", "pocket.realms.minecraft.net"];

/// Microsoft's telemetry, reachable only with `[bedrock] telemetry = true`.
pub const TELEMETRY_SUFFIXES: &[&str] = &[".events.data.microsoft.com"];
pub const TELEMETRY_EXACT: &[&str] = &["vortex.data.microsoft.com"];

/// Names a Bedrock session never reaches, whatever a suffix says: the Java services (a Bedrock
/// game has no business there, and the Java token never exists in a Bedrock session).
pub const DENIED: &[&str] = &["api.minecraftservices.com", "textures.minecraft.net", "sessionserver.mojang.com"];

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
    fn the_java_services_are_denied_and_never_reached_by_a_suffix() {
        let plan = plan(&BedrockConfig::default(), &[]);
        for name in DENIED {
            assert!(plan.denied.iter().any(|d| d == name));
            assert!(!plan.sni_exact.iter().any(|e| e == name));
            assert!(!plan.sni_suffixes.iter().any(|s| name.ends_with(s.as_str())), "{name}");
        }
    }

    #[test]
    fn an_instances_servers_follow_the_global_ones_without_duplicates() {
        let a = BedrockServer { host: "a.example.org".into(), port: 19132 };
        let b = BedrockServer { host: "b.example.org".into(), port: 19133 };
        let cfg = BedrockConfig { servers: vec![a.clone()], ..BedrockConfig::default() };
        assert_eq!(plan(&cfg, &[a.clone(), b.clone()]).udp_servers, vec![a, b]);
    }
}
