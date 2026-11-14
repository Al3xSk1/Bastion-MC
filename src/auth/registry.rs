//! Which local accounts exist, and which one is used when none is named.
//!
//! An *account* here is a local label (`--account work`): a slot in the refresh-token store
//! plus the remembered player name. Several Microsoft accounts are several labels; nothing
//! is shared between them — each has its own keyring entry or its own age vault file, and a
//! launch only ever loads the one it was asked for.
//!
//! The registry is derived, not kept: an account exists if a sign-in was recorded for it
//! (`auth::identity`) or a vault file is present. The only thing stored here is the name of
//! the default account, in the state directory. Nothing in this module is a credential.

use std::{collections::BTreeSet, path::Path};

use crate::{
    error::Result,
    paths::{Paths, validate_id},
    util,
};

/// The label used when nothing else is configured.
pub const FALLBACK: &str = "default";

fn stems(dir: &Path, extension: &str, into: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(stem) = name.strip_suffix(extension)
            && validate_id(stem).is_ok()
        {
            into.insert(stem.to_string());
        }
    }
}

/// The account used when `--account` is not given: the recorded default if it is a valid
/// label, otherwise [`FALLBACK`]. A damaged file is ignored, never an error.
pub fn default_account(paths: &Paths) -> String {
    util::read_small_file(&paths.default_account_file(), 128)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| text.trim().to_string())
        .filter(|name| validate_id(name).is_ok())
        .unwrap_or_else(|| FALLBACK.to_string())
}

pub fn set_default(paths: &Paths, account: &str) -> Result<()> {
    validate_id(account)?;
    util::write_atomic(&paths.default_account_file(), format!("{account}\n").as_bytes(), 0o600)
}

/// Forget the default if it is `account` (used at sign-out, so that the default never
/// silently points at a slot that someone else might later fill).
pub fn clear_default_if(paths: &Paths, account: &str) {
    if default_account(paths) == account {
        let _ = std::fs::remove_file(paths.default_account_file());
    }
}

/// Every known account label, sorted; always contains the default.
pub fn list(paths: &Paths) -> Vec<String> {
    let mut names = BTreeSet::new();
    stems(&paths.identities(), ".json", &mut names);
    stems(&paths.vault(), ".age", &mut names);
    names.insert(default_account(paths));
    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::identity::{self, Identity};

    fn paths() -> (std::path::PathBuf, Paths) {
        let root =
            std::env::temp_dir().join(format!("bastion-reg-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
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
    fn accounts_come_from_sign_ins_and_vault_files() {
        let (root, paths) = paths();
        assert_eq!(list(&paths), ["default"]);
        let id = Identity { player: "Alex".into(), uuid: "069a79f444e94726a5befca90e38aaf5".into() };
        identity::remember(&paths, "work", &id).unwrap();
        std::fs::create_dir_all(paths.vault()).unwrap();
        std::fs::write(paths.vault().join("alt.age"), b"x").unwrap();
        // Not labels: ignored rather than listed.
        std::fs::write(paths.vault().join("Bad Name.age"), b"x").unwrap();
        std::fs::write(paths.vault().join("..age"), b"x").unwrap();
        std::fs::write(paths.identities().join("notes.txt"), b"x").unwrap();
        assert_eq!(list(&paths), ["alt", "default", "work"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_default_is_recorded_validated_and_cleared() {
        let (root, paths) = paths();
        assert_eq!(default_account(&paths), "default");
        set_default(&paths, "work").unwrap();
        assert_eq!(default_account(&paths), "work");
        assert_eq!(list(&paths), ["work"], "the default is always listed");
        assert!(set_default(&paths, "../escape").is_err());
        clear_default_if(&paths, "other");
        assert_eq!(default_account(&paths), "work");
        clear_default_if(&paths, "work");
        assert_eq!(default_account(&paths), "default");
        // A file that is not a label is ignored.
        for junk in ["../../etc/passwd\n", "", "UPPER", "a b", &"x".repeat(200)] {
            std::fs::write(paths.default_account_file(), junk).unwrap();
            assert_eq!(default_account(&paths), "default", "{junk:?}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
