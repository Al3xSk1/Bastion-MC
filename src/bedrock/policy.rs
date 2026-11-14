//! The gate in front of every Bedrock launch, and (owned by the risk package) the account-risk
//! controls around a Bedrock session: scram, the canary prompt, doctor lines.

use super::BedrockConfig;
use crate::error::{Error, Result};

/// Refuses a Bedrock launch unless the user accepted the licence substitution.
///
/// The text names what is being accepted, so that a user who never read the documents
/// learns it here rather than not at all.
pub fn gate(cfg: &BedrockConfig) -> Result<()> {
    if cfg.accept_licence_substitution {
        return Ok(());
    }
    Err(Error::Policy(
        "a Bedrock launch needs `[bedrock] accept_licence_substitution = true` in config.toml: \
         mcpelauncher-client answers the game's Google Play licence query itself, after \
         mcpelauncher-ui has checked with Google that your account owns Minecraft. bastion-mc \
         decrypts and patches nothing, but it runs that client; see docs/BEDROCK.md"
            .into(),
    ))
}
