//! One error type for the whole launcher.
//!
//! The variants are coarse on purpose: callers never branch on *why* a security check failed
//! in order to retry more leniently. Every integrity, signature or policy failure is terminal.

use std::io;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("configuration error: {0}")]
    Config(String),
    /// A hash or size did not match. Never recoverable, never downgraded to a warning.
    #[error("INTEGRITY VIOLATION: {0}")]
    Integrity(String),
    #[error("signature verification failed: {0}")]
    Signature(String),
    #[error("policy violation: {0}")]
    Policy(String),
    #[error("network error: {0}")]
    Net(String),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("sandbox error: {0}")]
    Sandbox(String),
    #[error("malformed data: {0}")]
    Format(String),
    #[error("{0}")]
    Usage(String),
    /// The command was stopped on request before it finished.
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Error::Io { context: context.into(), source }
    }

    /// Process exit code. Distinct codes let wrappers tell "you mistyped" from "something
    /// tampered with your files".
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => 2,
            Error::Cancelled => 130,
            Error::Config(_) => 3,
            Error::Integrity(_) | Error::Signature(_) => 4,
            Error::Policy(_) => 5,
            Error::Auth(_) => 6,
            Error::Net(_) | Error::Tls(_) => 7,
            Error::Sandbox(_) => 8,
            Error::Io { .. } | Error::Format(_) | Error::Other(_) => 1,
        }
    }
}

/// Attach a human-readable context to low-level errors.
pub trait Context<T> {
    fn ctx<S: Into<String>>(self, f: impl FnOnce() -> S) -> Result<T>;
}

impl<T> Context<T> for io::Result<T> {
    fn ctx<S: Into<String>>(self, f: impl FnOnce() -> S) -> Result<T> {
        self.map_err(|e| Error::io(f(), e))
    }
}

impl<T> Context<T> for nix::Result<T> {
    fn ctx<S: Into<String>>(self, f: impl FnOnce() -> S) -> Result<T> {
        self.map_err(|e| Error::io(f(), io::Error::from(e)))
    }
}
