//! Curated mod profiles: a user-signed manifest pins the game version, the mod loader with
//! every one of its libraries, and every mod jar. Nothing outside Mojang's own hash chain is
//! executed unless an Ed25519 key in the trust store signed for its exact SHA-256.

pub mod author;
pub mod capabilities;
pub mod manifest;
pub mod mods;
pub mod packs;
pub mod signing;
pub mod zipdir;
