//! The policy handed from the launcher to the in-sandbox init stage (as JSON on a sealed
//! memfd). It contains no secrets: paths, ports and flags only.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::SeccompMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitPolicy {
    /// Program to become, as seen inside the sandbox (the JVM).
    pub exec: PathBuf,
    /// Complete argv, including argv[0].
    pub argv: Vec<String>,
    /// The entire environment of the program. Nothing else is inherited.
    pub env: Vec<(String, String)>,
    pub landlock: LandlockPolicy,
    pub seccomp: SeccompPolicy,
}

/// Filesystem and network rights, expressed in sandbox-internal paths. Landlock is purely
/// additive: anything not listed is denied.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LandlockPolicy {
    /// Read files, list directories.
    pub read: Vec<PathBuf>,
    /// Read plus `execve`. Only the JVM home and the dynamic loader by default, so a
    /// `Runtime.exec("/bin/sh -c …")` payload fails even though `/usr/bin` is visible.
    pub read_exec: Vec<PathBuf>,
    /// Read, write, create and remove regular files and directories. No symlinks, sockets,
    /// FIFOs or device nodes can be created anywhere.
    pub write: Vec<PathBuf>,
    /// Character devices the game legitimately drives with ioctl (GPU, sound).
    pub devices: Vec<PathBuf>,
    /// Devices that may be opened but not ioctl'd (`/dev/null`, `/dev/urandom`, …).
    pub plain_devices: Vec<PathBuf>,
    /// Pathname UNIX sockets the game may connect to (display, audio).
    pub unix_sockets: Vec<PathBuf>,
    /// Directories in which the program may create, remove and connect to UNIX sockets.
    /// Empty for the game; a display server (the nested compositor) has to listen somewhere.
    pub socket_dirs: Vec<PathBuf>,
    /// Paths that are granted only if they exist (optional mounts).
    pub read_if_present: Vec<PathBuf>,
    pub tcp: TcpPolicy,
    /// UDP ports datagrams may be sent to; `None` leaves UDP alone (open network mode).
    /// With `Some`, binding is limited to kernel-chosen ephemeral ports. Needs Landlock
    /// ABI 10; on older kernels the private network namespace is the only UDP barrier.
    #[serde(default)]
    pub udp_send: Option<Vec<u16>>,
    /// Deny connecting to abstract UNIX sockets created outside the sandbox.
    pub scope_abstract_unix: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum TcpPolicy {
    /// `connect()` only to these ports; `bind()` denied entirely.
    Ports(Vec<u16>),
    /// `connect()` unrestricted (open network mode); `bind()` still denied.
    #[default]
    AnyConnect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeccompPolicy {
    pub mode: SeccompMode,
    pub tripwire: bool,
    pub extra_allow: Vec<String>,
}

impl Default for SeccompPolicy {
    fn default() -> Self {
        SeccompPolicy { mode: SeccompMode::Enforce, tripwire: true, extra_allow: Vec::new() }
    }
}
