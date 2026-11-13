//! Mojang's metadata chain and the verified local store.
//!
//! Trust chain: TLS 1.3 → `version_manifest_v2.json` → SHA-1 of the version JSON → SHA-1 and
//! size of the client jar, every library, the asset index, the logging configuration → SHA-1
//! and size of every asset object. Nothing is written to its final path, and nothing is ever
//! handed to the JVM, without matching the digest published one level up.

pub mod args;
pub mod install;
pub mod maven;
pub mod meta;
