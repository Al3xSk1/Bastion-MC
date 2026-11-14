//! Instances: one isolated directory per game setup.
//!
//! ```text
//! instances/<id>/
//!   instance.toml            launcher settings (typed, no arbitrary JVM arguments)
//!   profile.json(.sig)       curated instances only: the signed manifest
//!   mods/                    curated instances only: exactly the signed jars
//!   saves/ logs/ config/     the only directories the game can write to
//!   resourcepacks/ shaderpacks/   read-only for the game; see `profile::packs`
//!   persist/                 options.txt, servers.dat: carried in before, and back out after,
//!                            each session by the launcher — parsed and re-encoded, never
//!                            copied (see `persist`)
//! ```
//!
//! `persist/` is a directory of its own for one reason: once the game runs, the launcher
//! confines itself and keeps write access to exactly that directory of the instance (see
//! `confine`). Everything that decides how the *next* launch is protected — `instance.toml`
//! (network mode, server allowlist), the manifest and its signature, `packs.lock.json` — lies
//! outside it, so a launcher that was compromised through something the game sent cannot
//! weaken the launch after this one. What it could still do is rewrite the settings files in
//! `persist/`; those are parsed and re-encoded again before the game is given them.
//!
//! A *vanilla* instance runs an unmodified Mojang version and needs no signature: its whole
//! content is covered by Mojang's hash chain. A *curated* instance exists only by virtue of
//! a manifest signed by a trusted key.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    config::{InstanceConfig, validate_version_id},
    error::{Error, Result},
    paths::Paths,
    profile::{
        manifest::{MAX_MANIFEST_BYTES, ProfileManifest},
        signing::{self, SignatureFile, TrustStore},
    },
    util,
};

/// The game may write here and nowhere else in the instance (kernel-enforced).
/// `resourcepacks/` and `shaderpacks/` are deliberately not in this list: packs are added
/// by the user (`bastion-mc pack import`) and are read-only to the game; see `profile::packs`.
pub const WRITABLE_DIRS: [&str; 3] = ["saves", "logs", "config"];
pub const MANIFEST_FILE: &str = "profile.json";
pub const SIGNATURE_FILE: &str = "profile.json.sig";
/// Where the settings files the launcher carries (`sandbox.persist_files`) rest between
/// sessions: the one part of an instance the confined launcher can still write.
pub const PERSIST_DIR: &str = "persist";
/// The launcher's own files in an instance directory: what the instance is and how the next
/// launch is protected. None of them can be a "settings file": such files are carried to and
/// from the game, and those of an older instance are *moved* into `persist/`
/// ([`adopt_persisted`]) — which must never happen to one of these.
pub const OWN_FILES: [&str; 4] = ["instance.toml", MANIFEST_FILE, SIGNATURE_FILE, crate::profile::packs::LOCK_FILE];

#[derive(Debug)]
pub enum Kind {
    Vanilla,
    Curated { manifest: Box<ProfileManifest>, signed_by: String },
}

#[derive(Debug)]
pub struct Instance {
    pub id: String,
    pub dir: PathBuf,
    pub config: InstanceConfig,
    pub minecraft: String,
    pub kind: Kind,
}

/// A directory the sandbox will get read-write. It must be a real directory of ours: a
/// symlink here would silently redirect the bind mount to wherever it points.
pub fn ensure_plain_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => util::require_private(path),
        Ok(_) => Err(Error::Policy(format!("{} must be a plain directory, not a symlink or file", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => util::create_private_dir_all(path),
        Err(e) => Err(Error::io(format!("inspecting {}", path.display()), e)),
    }
}

/// Make sure the instance has its `persist/` directory, and move into it the settings files
/// that launchers before this layout kept in the instance directory itself. Returns notes
/// for the user (what was moved, what was left alone and why).
///
/// A file is moved only if it is a regular file and `persist/` has none of that name; it is
/// moved as it is, because it is parsed and re-encoded anyway before the game gets it. If
/// both places hold one, the one in `persist/` is the one in use — the other is left where
/// it is and reported, never deleted and never silently preferred.
pub fn adopt_persisted(dir: &Path, names: &[String]) -> Result<Vec<String>> {
    let persist = dir.join(PERSIST_DIR);
    ensure_plain_dir(&persist)?;
    let mut notes = Vec::new();
    for name in names {
        // (Checked by the configuration already; a name is never joined onto a path unchecked,
        // and the instance's own files are never moved, whatever was configured.)
        crate::paths::validate_file_name(name)?;
        if OWN_FILES.contains(&name.as_str()) {
            return Err(Error::Policy(format!(
                "{name} is the launcher's own file in an instance; it cannot be a settings file (sandbox.persist_files)"
            )));
        }
        let (old, new) = (dir.join(name), persist.join(name));
        match fs::symlink_metadata(&old) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::io(format!("inspecting {}", old.display()), e)),
            Ok(meta) if !meta.file_type().is_file() => {
                notes.push(format!(
                    "{name} in the instance directory is not a regular file; it is not used (settings now live in \
                     {PERSIST_DIR}/)"
                ));
                continue;
            }
            Ok(_) => {}
        }
        if fs::symlink_metadata(&new).is_ok() {
            notes.push(format!(
                "{name} exists both in the instance directory and in {PERSIST_DIR}/; the one in {PERSIST_DIR}/ is \
                 used, the other one is left untouched (delete it, or move it over the one in {PERSIST_DIR}/)"
            ));
            continue;
        }
        fs::rename(&old, &new).map_err(|e| Error::io(format!("moving {} to {}", old.display(), new.display()), e))?;
        notes.push(format!("{name} moved into the instance's {PERSIST_DIR}/ directory, where settings files now live"));
    }
    Ok(notes)
}

/// The directory `name` is to be read from when nothing may be created or moved (a burner
/// session): `persist/` if it is a real directory that holds the file, otherwise the
/// instance directory itself, where it was kept before `persist/` existed.
pub fn persisted_in(dir: &Path, name: &str) -> PathBuf {
    let persist = dir.join(PERSIST_DIR);
    let plain = fs::symlink_metadata(&persist).is_ok_and(|m| m.file_type().is_dir());
    if plain && fs::symlink_metadata(persist.join(name)).is_ok() { persist } else { dir.to_path_buf() }
}

pub fn list(paths: &Paths) -> Result<Vec<String>> {
    let dir = paths.instances();
    let mut ids = Vec::new();
    match fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir())
                    && let Ok(name) = entry.file_name().into_string()
                    && crate::paths::validate_id(&name).is_ok()
                {
                    ids.push(name);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(format!("reading {}", dir.display()), e)),
    }
    ids.sort();
    Ok(ids)
}

/// Create the directory skeleton. `minecraft` is written to instance.toml for vanilla
/// instances; curated ones take the version from their manifest.
pub fn create(paths: &Paths, id: &str, minecraft: Option<&str>) -> Result<PathBuf> {
    let dir = paths.instance(id)?;
    if dir.exists() {
        return Err(Error::Usage(format!("instance {id:?} already exists at {}", dir.display())));
    }
    util::create_private_dir_all(&dir)?;
    for sub in WRITABLE_DIRS {
        util::create_private_dir_all(&dir.join(sub))?;
    }
    let mut text = String::from("# bastion-mc instance settings. Unknown keys are errors.\n");
    if let Some(v) = minecraft {
        validate_version_id(v)?;
        text.push_str(&format!("minecraft = \"{v}\"\n"));
    }
    text.push_str("# memory_mb = 4096\n# width = 1280\n# height = 720\n\n[network]\n# mode = \"filtered\"\n# servers = [\"play.example.org\", \"203.0.113.7:25570\"]\n");
    util::write_atomic(&dir.join("instance.toml"), text.as_bytes(), 0o600)?;
    Ok(dir)
}

/// Read the raw manifest and signature of a curated instance (`None` for vanilla).
pub fn read_signed_manifest(dir: &Path) -> Result<Option<(Vec<u8>, SignatureFile)>> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let sig_path = dir.join(SIGNATURE_FILE);
    if !manifest_path.exists() {
        if sig_path.exists() {
            return Err(Error::Signature(format!("{} exists without {MANIFEST_FILE}", sig_path.display())));
        }
        return Ok(None);
    }
    let bytes = util::read_small_file(&manifest_path, MAX_MANIFEST_BYTES)?;
    if !sig_path.exists() {
        return Err(Error::Signature(format!(
            "{} is not signed. Review it, then run `bastion-mc profile sign`.",
            manifest_path.display()
        )));
    }
    let sig_text = String::from_utf8(util::read_small_file(&sig_path, 4096)?)
        .map_err(|_| Error::Format("signature file is not UTF-8".into()))?;
    Ok(Some((bytes, SignatureFile::parse(&sig_text)?)))
}

/// Load an instance for launching. For curated instances this verifies the signature against
/// the trust store, validates the manifest, and enforces the anti-rollback serial — before
/// anything from the manifest is acted upon.
pub fn load(paths: &Paths, id: &str) -> Result<Instance> {
    let dir = paths.instance(id)?;
    if !dir.is_dir() {
        return Err(Error::Usage(format!(
            "no instance {id:?} (expected {}). Create one with `bastion-mc instance new`.",
            dir.display()
        )));
    }
    util::require_private(&dir)?;
    let config = InstanceConfig::load(&dir)?;

    match read_signed_manifest(&dir)? {
        None => {
            let minecraft = config.minecraft.clone().ok_or_else(|| {
                Error::Config(format!("{}/instance.toml must set `minecraft = \"<version>\"`", dir.display()))
            })?;
            Ok(Instance { id: id.to_string(), dir, config, minecraft, kind: Kind::Vanilla })
        }
        Some((bytes, sig)) => {
            let trust = TrustStore::load(paths)?;
            if trust.keys.is_empty() {
                return Err(Error::Signature(format!(
                    "no trusted signing keys in {}. Add your public key with `bastion-mc key trust`.",
                    trust.dir.display()
                )));
            }
            let signed_by = trust.verify(&bytes, &sig)?.to_string();
            let manifest = ProfileManifest::parse_verified(&bytes, id)?;
            signing::check_and_record_serial(paths, id, manifest.serial, &bytes)?;
            if let Some(v) = &config.minecraft
                && v != &manifest.minecraft.version
            {
                return Err(Error::Config(format!(
                    "instance.toml says Minecraft {v} but the signed profile says {}",
                    manifest.minecraft.version
                )));
            }
            let minecraft = manifest.minecraft.version.clone();
            Ok(Instance {
                id: id.to_string(),
                dir,
                config,
                minecraft,
                kind: Kind::Curated { manifest: Box::new(manifest), signed_by },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{manifest::tests::sample, signing::SecretKey};
    use std::os::unix::fs::PermissionsExt;

    fn sandbox_paths(tag: &str) -> (PathBuf, Paths) {
        let root = std::env::temp_dir()
            .join(format!("bastion-inst-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
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

    fn trust(paths: &Paths, key: &SecretKey) {
        let dir = paths.user_trusted_keys();
        util::create_private_dir_all(&dir).unwrap();
        util::write_atomic(&dir.join("me.pub"), key.public().encode().as_bytes(), 0o600).unwrap();
    }

    #[test]
    fn vanilla_instance_roundtrip() {
        let (root, paths) = sandbox_paths("vanilla");
        let dir = create(&paths, "vanilla-1.21.4", Some("1.21.4")).unwrap();
        for sub in WRITABLE_DIRS {
            assert!(dir.join(sub).is_dir());
        }
        let inst = load(&paths, "vanilla-1.21.4").unwrap();
        assert_eq!(inst.minecraft, "1.21.4");
        assert!(matches!(inst.kind, Kind::Vanilla));
        assert_eq!(list(&paths).unwrap(), vec!["vanilla-1.21.4"]);
        assert!(create(&paths, "vanilla-1.21.4", Some("1.21.4")).is_err(), "no overwrite");
        assert!(load(&paths, "missing").is_err());
        assert!(load(&paths, "../escape").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn curated_instance_requires_a_valid_trusted_signature() {
        let (root, paths) = sandbox_paths("curated");
        let dir = create(&paths, "perf", None).unwrap();
        let key = SecretKey::generate().unwrap();
        let manifest = sample("perf");
        let bytes = manifest.to_bytes().unwrap();
        fs::write(dir.join(MANIFEST_FILE), &bytes).unwrap();

        // unsigned
        assert!(matches!(load(&paths, "perf"), Err(Error::Signature(_))));
        // signed, but nobody is trusted yet
        fs::write(dir.join(SIGNATURE_FILE), key.sign(&bytes).render()).unwrap();
        assert!(matches!(load(&paths, "perf"), Err(Error::Signature(_))));
        // trusted: loads
        trust(&paths, &key);
        let inst = load(&paths, "perf").unwrap();
        assert!(matches!(&inst.kind, Kind::Curated { signed_by, .. } if signed_by == "me"));
        assert_eq!(inst.minecraft, "1.21.4");

        // one byte of the manifest changed after signing
        let mut tampered = bytes.clone();
        let pos = tampered.windows(6).position(|w| w == b"sodium").unwrap();
        tampered[pos] = b'S';
        fs::write(dir.join(MANIFEST_FILE), &tampered).unwrap();
        assert!(matches!(load(&paths, "perf"), Err(Error::Signature(_))));

        // signed by someone else
        let other = SecretKey::generate().unwrap();
        fs::write(dir.join(MANIFEST_FILE), &bytes).unwrap();
        fs::write(dir.join(SIGNATURE_FILE), other.sign(&bytes).render()).unwrap();
        assert!(matches!(load(&paths, "perf"), Err(Error::Signature(_))));

        // a manifest validly signed for a different instance
        let foreign = sample("other").to_bytes().unwrap();
        fs::write(dir.join(MANIFEST_FILE), &foreign).unwrap();
        fs::write(dir.join(SIGNATURE_FILE), key.sign(&foreign).render()).unwrap();
        assert!(matches!(load(&paths, "perf"), Err(Error::Policy(_))));

        // rollback: serial 2 accepted, then serial 1 offered again
        let mut v2 = sample("perf");
        v2.serial = 2;
        let v2b = v2.to_bytes().unwrap();
        fs::write(dir.join(MANIFEST_FILE), &v2b).unwrap();
        fs::write(dir.join(SIGNATURE_FILE), key.sign(&v2b).render()).unwrap();
        load(&paths, "perf").unwrap();
        fs::write(dir.join(MANIFEST_FILE), &bytes).unwrap();
        fs::write(dir.join(SIGNATURE_FILE), key.sign(&bytes).render()).unwrap();
        assert!(
            matches!(load(&paths, "perf"), Err(Error::Policy(_))),
            "an older, validly signed manifest must be refused"
        );

        // a stray signature without a manifest is suspicious, not "vanilla"
        fs::remove_file(dir.join(MANIFEST_FILE)).unwrap();
        assert!(matches!(load(&paths, "perf"), Err(Error::Signature(_))));
        fs::remove_dir_all(root).unwrap();
    }

    /// Settings files of an instance from before `persist/` existed are moved there once;
    /// nothing is overwritten, nothing is followed, and a burner session finds the files
    /// wherever they are without moving anything.
    #[test]
    fn settings_files_of_an_existing_instance_move_into_the_persist_directory() {
        let (root, paths) = sandbox_paths("persist");
        let dir = create(&paths, "old", Some("1.21.4")).unwrap();
        let names = vec!["options.txt".to_string(), "servers.dat".to_string(), "hotbar.nbt".to_string()];
        fs::write(dir.join("options.txt"), b"fov:70\n").unwrap();
        fs::write(dir.join("servers.dat"), b"old-servers").unwrap();
        assert!(!dir.join(PERSIST_DIR).exists());

        // A burner session moves and creates nothing, and reads from where the file is.
        assert_eq!(persisted_in(&dir, "options.txt"), dir);
        assert!(!dir.join(PERSIST_DIR).exists());

        let notes = adopt_persisted(&dir, &names).unwrap();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes.iter().all(|n| n.contains("moved into")), "{notes:?}");
        let persist = dir.join(PERSIST_DIR);
        assert_eq!(fs::read(persist.join("options.txt")).unwrap(), b"fov:70\n");
        assert_eq!(fs::read(persist.join("servers.dat")).unwrap(), b"old-servers");
        assert!(!dir.join("options.txt").exists() && !dir.join("servers.dat").exists());
        assert_eq!(fs::metadata(&persist).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(persisted_in(&dir, "options.txt"), persist);
        assert_eq!(persisted_in(&dir, "hotbar.nbt"), dir, "not there: the old place is where to look");
        // What decides the next launch's protection is not in the directory that stays writable.
        assert!(dir.join("instance.toml").exists() && !persist.join("instance.toml").exists());

        // Again: nothing to do, nothing said.
        assert!(adopt_persisted(&dir, &names).unwrap().is_empty());

        // A file that appears in the old place later does not replace the one in use.
        fs::write(dir.join("options.txt"), b"fov:110\n").unwrap();
        let notes = adopt_persisted(&dir, &names).unwrap();
        assert!(notes.len() == 1 && notes[0].contains("exists both"), "{notes:?}");
        assert_eq!(fs::read(persist.join("options.txt")).unwrap(), b"fov:70\n");
        assert_eq!(fs::read(dir.join("options.txt")).unwrap(), b"fov:110\n", "left untouched");
        fs::remove_file(dir.join("options.txt")).unwrap();

        // A link in the old place is not moved and not followed.
        fs::write(root.join("elsewhere"), b"secret").unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), dir.join("hotbar.nbt")).unwrap();
        let notes = adopt_persisted(&dir, &names).unwrap();
        assert!(notes.len() == 1 && notes[0].contains("not a regular file"), "{notes:?}");
        assert!(!persist.join("hotbar.nbt").exists() && dir.join("hotbar.nbt").is_symlink());

        // The instance's own files are never moved, whatever a configuration names.
        for own in OWN_FILES {
            fs::write(dir.join(own), b"{}").unwrap();
            assert!(matches!(adopt_persisted(&dir, &[own.to_string()]), Err(Error::Policy(_))), "{own}");
            assert!(dir.join(own).exists() && !persist.join(own).exists(), "{own} was moved");
        }

        // `persist` itself being a link (or a file) is refused: the launcher would otherwise
        // keep write access to wherever it points.
        fs::remove_dir_all(&persist).unwrap();
        std::os::unix::fs::symlink(&root, &persist).unwrap();
        assert!(matches!(adopt_persisted(&dir, &names), Err(Error::Policy(_))));
        assert_eq!(persisted_in(&dir, "elsewhere"), dir, "a linked persist/ is not read through either");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writable_directories_must_not_be_symlinks() {
        let (root, paths) = sandbox_paths("symlink");
        let dir = create(&paths, "x", Some("1.21.4")).unwrap();
        fs::remove_dir(dir.join("saves")).unwrap();
        std::os::unix::fs::symlink("/home", dir.join("saves")).unwrap();
        assert!(matches!(ensure_plain_dir(&dir.join("saves")), Err(Error::Policy(_))));
        assert!(ensure_plain_dir(&dir.join("logs")).is_ok());
        assert!(ensure_plain_dir(&dir.join("screenshots")).is_ok(), "missing directories are created");
        fs::set_permissions(dir.join("config"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(ensure_plain_dir(&dir.join("config")).is_err(), "world-writable directory");
        fs::remove_dir_all(root).unwrap();
    }
}
