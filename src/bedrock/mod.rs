//! Minecraft: Bedrock Edition — the Android build, run by an installed `mcpelauncher-client`
//! inside bastion's sandbox.
//!
//! **Status: interfaces only.** This module was written by the integrator as the contract of
//! the fifth round (`docs/notes/round5-contract.md`); five packages fill it in. Until then a
//! Bedrock launch is refused.
//!
//! What this is, and what it is not:
//!
//! * bastion runs **the user's installed, root-owned `mcpelauncher-client`** (never a
//!   download, the same policy as the JDK) on game files the user obtained with
//!   `mcpelauncher-ui` from **their own Google Play purchase**, imported into bastion's store.
//!   bastion contains no Google Play client and no Minecraft files.
//! * Nothing here decrypts, patches or replaces a licence check. `mcpelauncher-client` answers
//!   the game's Google Play licence query itself (after `mcpelauncher-ui` checked ownership
//!   with Google); the user accepts that knowingly with
//!   `[bedrock] accept_licence_substitution = true` ([`policy::gate`]), or no Bedrock launch
//!   happens. The Windows (GDK) build is not supported: it can only be run by extracting
//!   keys and decrypting the game outside Microsoft's protected path.
//! * The game must hold the account's **Xbox identity itself** (its sign-in library lives
//!   inside the game). Unlike Java, nothing can be brokered: bastion limits what a hostile
//!   game can do with it (sandbox, allowlist, no Java token present, the game's token store
//!   wiped after every session, sign-in pages outside the game) and says so plainly.
//!
//! Packages (ownership as in the contract): `runtime` and `store` (finding the client,
//! importing versions), the launch and sandbox profile, `net` (the Bedrock network profile:
//! names by SNI, RakNet over UDP per server rule, Realms and event servers brokered),
//! `signin` (the `mcpelauncher-webview` bridge), `policy` plus scram, canary and the
//! documents.

pub mod net;
pub mod policy;
pub mod runtime;
pub mod signin;
pub mod store;

use serde::Deserialize;

/// `[bedrock]` in config.toml. Every key has a default; `examples/config.toml` lists them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BedrockConfig {
    /// The user accepts that `mcpelauncher-client` answers the game's Google Play licence
    /// query with "verified" (after `mcpelauncher-ui` checked with Google that the account
    /// owns the game). Without it every Bedrock launch is refused ([`policy::gate`]).
    pub accept_licence_substitution: bool,
    /// The installed client. `None`: look for it where packages install it
    /// ([`runtime::discover`]). Either way it must be root-owned with no writable path
    /// component, like the JDK.
    pub client: Option<std::path::PathBuf>,
    /// Microsoft's telemetry hosts reachable during Bedrock sessions. On by default: whether
    /// a challenge cape registers without them is not known.
    pub telemetry: bool,
    /// Worlds (and the game's own settings) survive a session. The game's sign-in tokens
    /// never do: its token store is wiped when the session ends, whatever this says.
    pub persist_worlds: bool,
    /// The system-call filter of the client: `enforce`, or `log` to profile the real client
    /// once (the filter then reports instead of killing; the launch says so loudly).
    pub seccomp: SeccompMode,
    /// Bedrock servers every Bedrock instance may join (RakNet over UDP); an instance adds
    /// its own in its `instance.toml`. Realms and event servers are not listed here: they are
    /// brokered, and each join is approved outside the game (`[realms] approve`).
    pub servers: Vec<BedrockServer>,
}

impl Default for BedrockConfig {
    fn default() -> Self {
        BedrockConfig {
            accept_licence_substitution: false,
            client: None,
            telemetry: true,
            persist_worlds: true,
            seccomp: SeccompMode::Enforce,
            servers: Vec::new(),
        }
    }
}

/// How the client's system-call filter acts on a call outside its allowlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeccompMode {
    Enforce,
    Log,
}

/// One Bedrock server the game may reach: a host name (resolved to a synthetic address
/// inside the sandbox, as for Java servers) or an IP literal, and its UDP port.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BedrockServer {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

/// Bedrock's default server port (IPv4).
pub const DEFAULT_PORT: u16 = 19132;

fn default_port() -> u16 {
    DEFAULT_PORT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_is_refused_until_the_licence_substitution_is_accepted() {
        let cfg = BedrockConfig::default();
        assert!(!cfg.accept_licence_substitution);
        assert!(policy::gate(&cfg).is_err());
        let cfg = BedrockConfig { accept_licence_substitution: true, ..BedrockConfig::default() };
        assert!(policy::gate(&cfg).is_ok());
    }

    #[test]
    fn a_server_without_a_port_gets_bedrocks_default_port() {
        let cfg: BedrockConfig = toml::from_str("servers = [{ host = \"play.example.org\" }]").unwrap();
        assert_eq!(cfg.servers, vec![BedrockServer { host: "play.example.org".into(), port: DEFAULT_PORT }]);
    }

    #[test]
    fn unknown_keys_and_modes_are_refused() {
        assert!(toml::from_str::<BedrockConfig>("lan = true").is_err());
        assert!(toml::from_str::<BedrockConfig>("seccomp = \"off\"").is_err());
    }
}
