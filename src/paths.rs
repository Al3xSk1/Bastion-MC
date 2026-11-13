//! XDG Base Directory layout. Nothing is ever written outside these four roots.
//!
//! ```text
//! $XDG_CONFIG_HOME/bastion-mc/   config.toml, trusted-keys.d/
//! $XDG_DATA_HOME/bastion-mc/     instances/<id>/, libraries/, versions/, assets/, vault/
//! $XDG_CACHE_HOME/bastion-mc/    scratch/ (per-launch game root), natives/
//! $XDG_STATE_HOME/bastion-mc/    logs/, profile-serials/
//! ```

use std::{
    env,
    path::{Path, PathBuf},
};

use crate::{
    error::{Error, Result},
    util,
};

pub const APP: &str = "bastion-mc";

/// Root-owned trust anchors. When this directory contains keys, user-level keys are ignored,
/// so a compromise of the user account cannot silently add a signing key.
pub const SYSTEM_TRUSTED_KEYS: &str = "/etc/bastion-mc/trusted-keys.d";

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
    /// `$XDG_RUNTIME_DIR` of the host session, if any (display and audio sockets live there).
    pub runtime: Option<PathBuf>,
}

/// Per the XDG spec a relative path in these variables is invalid and must be ignored.
fn xdg(var: &str) -> Option<PathBuf> {
    env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute())
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| nix::unistd::User::from_uid(nix::unistd::getuid()).ok().flatten().map(|u| u.dir))
            .ok_or_else(|| Error::Config("cannot determine home directory (HOME unset and no passwd entry)".into()))?;
        Ok(Paths {
            config: xdg("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join(APP),
            data: xdg("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")).join(APP),
            cache: xdg("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")).join(APP),
            state: xdg("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")).join(APP),
            runtime: xdg("XDG_RUNTIME_DIR"),
        })
    }

    /// Create the roots with mode 0700 and verify nobody else can write to them.
    pub fn ensure(&self) -> Result<()> {
        for dir in [&self.config, &self.data, &self.cache, &self.state] {
            util::create_private_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }
    pub fn user_trusted_keys(&self) -> PathBuf {
        self.config.join("trusted-keys.d")
    }
    pub fn instances(&self) -> PathBuf {
        self.data.join("instances")
    }
    pub fn instance(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.instances().join(id))
    }
    pub fn libraries(&self) -> PathBuf {
        self.data.join("libraries")
    }
    pub fn versions(&self) -> PathBuf {
        self.data.join("versions")
    }
    pub fn assets(&self) -> PathBuf {
        self.data.join("assets")
    }
    pub fn vault(&self) -> PathBuf {
        self.data.join("vault")
    }
    pub fn scratch(&self) -> PathBuf {
        self.cache.join("scratch")
    }
    pub fn logs(&self) -> PathBuf {
        self.state.join("logs")
    }
    pub fn serials(&self) -> PathBuf {
        self.state.join("profile-serials")
    }
    /// Who each local account was at its last sign-in (player name only; no credentials).
    pub fn identities(&self) -> PathBuf {
        self.state.join("accounts")
    }
}

/// Instance and account identifiers become path components and keyring descriptions, so the
/// alphabet is deliberately tiny: no separators, no leading dot, no `..`.
pub fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && !id.contains("..");
    if ok {
        Ok(())
    } else {
        Err(Error::Usage(format!(
            "invalid identifier {id:?}: use 1-64 characters from [a-z0-9._-], starting with a letter or digit"
        )))
    }
}

/// A file name that is safe to use as a single path component inside the sandbox.
pub fn validate_file_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains("..")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'));
    if ok {
        Ok(())
    } else {
        Err(Error::Policy(format!("unsafe file name {name:?} (allowed: [A-Za-z0-9._+-], no leading dot)")))
    }
}

/// Reject anything that could step outside its root when joined onto it.
pub fn validate_relative(path: &Path) -> Result<()> {
    use std::path::Component;
    let clean = path.components().all(|c| matches!(c, Component::Normal(_)));
    if clean && !path.as_os_str().is_empty() {
        Ok(())
    } else {
        Err(Error::Policy(format!("unsafe relative path {:?}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        for good in ["vanilla-1.21.4", "a", "fabric_perf.2", "0abc"] {
            assert!(validate_id(good).is_ok(), "{good}");
        }
        for bad in ["", ".hidden", "../x", "a/b", "UPPER", "a..b", "-lead", &"x".repeat(65), "sp ace"] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_names() {
        assert!(validate_file_name("sodium-fabric-0.6.5+mc1.21.4.jar").is_ok());
        for bad in ["", ".x.jar", "a/b.jar", "..", "a b.jar", "x\0.jar", "évil.jar"] {
            assert!(validate_file_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn relative_paths() {
        assert!(validate_relative(Path::new("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar")).is_ok());
        for bad in ["", "/abs", "../up", "a/../b", "./a"] {
            assert!(validate_relative(Path::new(bad)).is_err(), "{bad:?}");
        }
    }
}
