//! Instances: one isolated directory per game setup.
//!
//! ```text
//! instances/<id>/
//!   instance.toml            launcher settings (typed, no arbitrary JVM arguments)
//!   profile.json(.sig)       curated instances only: the signed manifest
//!   mods/                    curated instances only: exactly the signed jars
//!   saves/ logs/ config/ resourcepacks/     the only directories the game can write to
//!   options.txt servers.dat  carried in before, and back out after, each session by the launcher —
//!                            parsed and re-encoded, never copied (see `persist`)
//! ```
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
pub const WRITABLE_DIRS: [&str; 4] = ["saves", "logs", "config", "resourcepacks"];
pub const MANIFEST_FILE: &str = "profile.json";
pub const SIGNATURE_FILE: &str = "profile.json.sig";

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
