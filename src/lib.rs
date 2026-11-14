//! bastion-mc: a zero-trust Minecraft launcher for Arch Linux.
//!
//! See `docs/THREAT_MODEL.md` for what each module defends against and, just as importantly,
//! what it does not.

pub mod auth;
pub mod cli;
pub mod config;
pub mod confine;
pub mod doctor;
pub mod egress;
pub mod error;
pub mod events;
pub mod fuzzing;
pub mod hardening;
pub mod instance;
pub mod java;
pub mod launch;
pub mod mojang;
pub mod net;
pub mod paths;
pub mod persist;
pub mod profile;
pub mod proofs;
pub mod running;
pub mod sandbox;
pub mod scram;
pub mod secret;
pub mod util;
pub mod verity;

pub use error::{Error, Result};

include!(concat!(env!("OUT_DIR"), "/bootstrap_meta.rs"));

/// The compiled `BastionBootstrap.class`, built from `java/BastionBootstrap.java` by build.rs.
pub const BOOTSTRAP_CLASS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/BastionBootstrap.class"));
pub const BOOTSTRAP_CLASS_NAME: &str = "BastionBootstrap";
