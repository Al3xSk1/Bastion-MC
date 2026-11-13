//! Who an account was the last time it signed in.
//!
//! A front end wants to say "signed in as Alex" without a network round trip and without
//! touching a token. The player name and profile id are public information (every server
//! you join sees them), so they are simply remembered in the state directory when a sign-in
//! succeeds and forgotten at sign-out. Nothing here is a credential, and nothing trusts the
//! file: a launch always uses the identity Mojang returns for the token.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    paths::{Paths, validate_id},
    util,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub player: String,
    /// Undashed profile UUID.
    pub uuid: String,
}

impl Identity {
    fn is_well_formed(&self) -> bool {
        (1..=25).contains(&self.player.len())
            && self.player.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && self.uuid.len() == 32
            && self.uuid.bytes().all(|b| b.is_ascii_hexdigit())
    }
}

fn file(paths: &Paths, account: &str) -> Result<PathBuf> {
    validate_id(account)?;
    Ok(paths.identities().join(format!("{account}.json")))
}

pub fn remember(paths: &Paths, account: &str, identity: &Identity) -> Result<()> {
    if !identity.is_well_formed() {
        return Err(Error::Format("refusing to record a malformed player identity".into()));
    }
    let path = file(paths, account)?;
    util::create_private_dir_all(&paths.identities())?;
    let json = serde_json::to_vec(identity).map_err(|e| Error::Other(format!("encoding identity: {e}")))?;
    util::write_atomic(&path, &json, 0o600)
}

/// The remembered identity, or `None` if there is none or the file is not exactly what
/// [`remember`] writes (it is only ever displayed, so a damaged file is just ignored).
pub fn recall(paths: &Paths, account: &str) -> Option<Identity> {
    let path = file(paths, account).ok()?;
    let bytes = util::read_small_file(&path, 1024).ok()?;
    serde_json::from_slice::<Identity>(&bytes).ok().filter(Identity::is_well_formed)
}

pub fn forget(paths: &Paths, account: &str) -> Result<()> {
    match std::fs::remove_file(file(paths, account)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io("removing the remembered identity", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (PathBuf, Paths) {
        let root =
            std::env::temp_dir().join(format!("bastion-id-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        };
        paths.ensure().unwrap();
        (root, paths)
    }

    #[test]
    fn remember_recall_forget() {
        let (root, paths) = paths();
        let id = Identity { player: "Notch".into(), uuid: "069a79f444e94726a5befca90e38aaf5".into() };
        assert_eq!(recall(&paths, "default"), None);
        remember(&paths, "default", &id).unwrap();
        assert_eq!(recall(&paths, "default"), Some(id.clone()));
        assert_eq!(recall(&paths, "other"), None, "accounts do not share an identity");
        forget(&paths, "default").unwrap();
        forget(&paths, "default").unwrap();
        assert_eq!(recall(&paths, "default"), None);
        assert!(remember(&paths, "../escape", &id).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_well_formed_identities_are_stored_or_shown() {
        let (root, paths) = paths();
        let bad = Identity { player: "\u{1b}[31mEvil".into(), uuid: "069a79f444e94726a5befca90e38aaf5".into() };
        assert!(remember(&paths, "default", &bad).is_err());
        std::fs::create_dir_all(paths.identities()).unwrap();
        for text in [
            r#"{"player":"\u001b[2J","uuid":"069a79f444e94726a5befca90e38aaf5"}"#,
            r#"{"player":"Notch","uuid":"not-a-uuid"}"#,
            r#"{"player":"Notch","uuid":"069a79f444e94726a5befca90e38aaf5","extra":1}"#,
            "not json",
        ] {
            std::fs::write(paths.identities().join("default.json"), text).unwrap();
            assert_eq!(recall(&paths, "default"), None, "{text}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
