//! Authoring side of curated profiles: draft a manifest, lock the mods directory into it,
//! sign it. None of this is trusted at launch time — only the resulting signature is.

use std::{collections::BTreeMap, path::PathBuf};

use serde::Deserialize;

use super::{
    manifest::{FORMAT, Loader, LoaderKind, LoaderLibrary, MinecraftPin, ProfileManifest, ProfileOptions},
    mods,
    signing::SecretKey,
};
use crate::{
    error::{Error, Result},
    instance::{self, MANIFEST_FILE, SIGNATURE_FILE},
    mojang::{maven::Coordinate, meta::VERSION_MANIFEST_URL},
    net::http::HttpClient,
    paths::Paths,
    util::{self, Expected, HashAlg},
};

const META_LIMIT: u64 = 4 << 20;
const LIBRARY_LIMIT: u64 = 128 << 20;

#[derive(Deserialize)]
struct LoaderProfile {
    #[serde(rename = "mainClass")]
    main_class: String,
    #[serde(default)]
    arguments: LoaderArguments,
    libraries: Vec<MetaLibrary>,
}

#[derive(Deserialize, Default)]
struct LoaderArguments {
    #[serde(default)]
    game: Vec<String>,
    #[serde(default)]
    jvm: Vec<String>,
}

#[derive(Deserialize)]
struct MetaLibrary {
    name: String,
    url: String,
    sha256: Option<String>,
}

#[derive(Deserialize)]
struct LoaderListEntry {
    loader: LoaderVersion,
}

#[derive(Deserialize)]
struct LoaderVersion {
    version: String,
    #[serde(default)]
    stable: Option<bool>,
}

/// Version strings are interpolated into URLs.
fn url_safe(what: &str, value: &str) -> Result<()> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && !value.contains("..")
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'));
    if ok {
        Ok(())
    } else {
        Err(Error::Usage(format!("{what} {value:?} contains characters that cannot be used here")))
    }
}

fn meta_base(kind: LoaderKind) -> &'static str {
    match kind {
        LoaderKind::Fabric => "https://meta.fabricmc.net/v2/versions/loader",
        LoaderKind::Quilt => "https://meta.quiltmc.org/v3/versions/loader",
    }
}

/// Create a curated instance with a draft manifest for `minecraft` + a mod loader.
///
/// Trust note, printed for the user as well: hashes the loader's metadata server publishes
/// are cross-checked; libraries it publishes no hash for are pinned to what was downloaded
/// just now over TLS (trust on first use). Your signature freezes the result either way.
pub async fn new_profile(
    paths: &Paths,
    http: &HttpClient,
    id: &str,
    minecraft: &str,
    kind: LoaderKind,
    loader_version: Option<&str>,
) -> Result<PathBuf> {
    url_safe("Minecraft version", minecraft)?;
    let base = meta_base(kind);
    let loader_version = match loader_version {
        Some(v) => v.to_string(),
        None => {
            let list: Vec<LoaderListEntry> =
                serde_json::from_slice(&http.get(&format!("{base}/{minecraft}"), META_LIMIT).await?)
                    .map_err(|e| Error::Format(format!("loader version list: {e}")))?;
            list.iter()
                .find(|e| e.loader.stable.unwrap_or(false))
                .or(list.first())
                .map(|e| e.loader.version.clone())
                .ok_or_else(|| Error::Usage(format!("no {kind:?} loader is published for Minecraft {minecraft}")))?
        }
    };
    url_safe("loader version", &loader_version)?;

    let profile: LoaderProfile = serde_json::from_slice(
        &http.get(&format!("{base}/{minecraft}/{loader_version}/profile/json"), META_LIMIT).await?,
    )
    .map_err(|e| Error::Format(format!("loader profile: {e}")))?;

    let mut libraries = Vec::new();
    let (mut cross_checked, mut first_use) = (0, 0);
    for lib in &profile.libraries {
        let coord = Coordinate::parse(&lib.name)?;
        let repo = if lib.url.ends_with('/') { lib.url.clone() } else { format!("{}/", lib.url) };
        let bytes = http.get(&format!("{repo}{}", coord.path()), LIBRARY_LIMIT).await?;
        let digest = util::hex_encode(&util::hash_bytes(HashAlg::Sha256, &bytes));
        match &lib.sha256 {
            Some(published) => {
                if Expected::sha256(published)?.hex() != digest {
                    return Err(Error::Integrity(format!(
                        "{}: downloaded SHA-256 {digest} differs from the one its metadata publishes ({published})",
                        lib.name
                    )));
                }
                cross_checked += 1;
            }
            None => first_use += 1,
        }
        libraries.push(LoaderLibrary { name: lib.name.clone(), url: repo, sha256: digest, size: bytes.len() as u64 });
    }

    // Pin Mojang's version file too, so the signature spans the whole game.
    #[derive(Deserialize)]
    struct Manifest {
        versions: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        sha1: String,
    }
    let mojang: Manifest = serde_json::from_slice(&http.get(VERSION_MANIFEST_URL, 16 << 20).await?)
        .map_err(|e| Error::Format(format!("version manifest: {e}")))?;
    let version_json_sha1 =
        mojang.versions.iter().find(|v| v.id == minecraft).map(|v| v.sha1.clone()).ok_or_else(|| {
            Error::Usage(format!("Minecraft version {minecraft:?} is not in Mojang's version manifest"))
        })?;

    let dir = instance::create(paths, id, None)?;
    util::create_private_dir_all(&dir.join("mods"))?;
    let manifest = ProfileManifest {
        format: FORMAT.into(),
        id: id.to_string(),
        serial: 1,
        minecraft: MinecraftPin { version: minecraft.to_string(), version_json_sha1: Some(version_json_sha1) },
        loader: Some(Loader {
            kind,
            version: loader_version.clone(),
            main_class: profile.main_class,
            jvm_args: profile.arguments.jvm,
            game_args: profile.arguments.game,
            libraries,
        }),
        mods: Vec::new(),
        options: ProfileOptions::default(),
    };
    manifest.validate(id)?;
    util::write_atomic(&dir.join(MANIFEST_FILE), &manifest.to_bytes()?, 0o600)?;
    eprintln!(
        "drafted {kind:?} {loader_version} profile for Minecraft {minecraft}: {} libraries \
         ({cross_checked} matched published SHA-256, {first_use} pinned on first use).",
        manifest.loader.as_ref().map(|l| l.libraries.len()).unwrap_or(0)
    );
    Ok(dir)
}

pub(super) fn read_draft(paths: &Paths, id: &str) -> Result<(PathBuf, ProfileManifest)> {
    let dir = paths.instance(id)?;
    let path = dir.join(MANIFEST_FILE);
    let bytes = util::read_small_file(&path, super::manifest::MAX_MANIFEST_BYTES)?;
    let manifest: ProfileManifest =
        serde_json::from_slice(&bytes).map_err(|e| Error::Format(format!("{}: {e}", path.display())))?;
    manifest.validate(id)?;
    Ok((dir, manifest))
}

/// Replace the manifest's mod list with what is in `mods/` right now, bump the serial if the
/// list changed, and discard the old signature (always). Returns the new mod list for the user to review.
pub fn lock(paths: &Paths, id: &str) -> Result<ProfileManifest> {
    lock_with(paths, id, &BTreeMap::new())
}

/// [`lock`], recording where files came from. `sources` (file name → address) names the
/// files that were just put into `mods/` by `profile add`. Every other entry keeps the
/// source it had for as long as the file is byte for byte the one that was recorded: a
/// source says where *these* bytes came from, so it does not survive a change of content.
pub fn lock_with(paths: &Paths, id: &str, sources: &BTreeMap<String, String>) -> Result<ProfileManifest> {
    let (dir, mut manifest) = read_draft(paths, id)?;
    let mut scanned = mods::scan_mods_dir(&dir.join("mods"))?;
    for entry in &mut scanned {
        let recorded = || {
            manifest
                .mods
                .iter()
                .find(|old| old.file == entry.file && old.sha256 == entry.sha256 && old.size == entry.size)
                .and_then(|old| old.source.clone())
        };
        entry.source = sources.get(&entry.file).cloned().or_else(recorded);
    }
    if scanned != manifest.mods {
        manifest.mods = scanned;
        manifest.serial += 1;
    }
    manifest.validate(id)?;
    util::write_atomic(&dir.join(MANIFEST_FILE), &manifest.to_bytes()?, 0o600)?;
    let _ = std::fs::remove_file(dir.join(SIGNATURE_FILE));
    Ok(manifest)
}

/// Sign the manifest as it is on disk. Refuses if `mods/` does not match it: a signature
/// should describe a state you can actually launch.
pub fn sign(paths: &Paths, id: &str, key: &SecretKey) -> Result<(ProfileManifest, String)> {
    let dir = paths.instance(id)?;
    let path = dir.join(MANIFEST_FILE);
    let bytes = util::read_small_file(&path, super::manifest::MAX_MANIFEST_BYTES)?;
    let manifest = ProfileManifest::parse_verified(&bytes, id)?;
    mods::verify_mods_dir(&dir.join("mods"), &manifest.mods)?;
    let signature = key.sign(&bytes);
    util::write_atomic(&dir.join(SIGNATURE_FILE), signature.render().as_bytes(), 0o600)?;
    Ok((manifest, key.public().id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::manifest::tests::sample;

    fn paths(tag: &str) -> (PathBuf, Paths) {
        let root = std::env::temp_dir()
            .join(format!("bastion-author-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let p = Paths {
            config: root.join("c"),
            data: root.join("d"),
            cache: root.join("k"),
            state: root.join("s"),
            runtime: None,
        };
        p.ensure().unwrap();
        (root, p)
    }

    #[test]
    fn lock_then_sign_then_launchable() {
        let (root, p) = paths("flow");
        let dir = instance::create(&p, "perf", None).unwrap();
        std::fs::create_dir(dir.join("mods")).unwrap();
        let mut draft = sample("perf");
        draft.mods.clear();
        std::fs::write(dir.join(MANIFEST_FILE), draft.to_bytes().unwrap()).unwrap();

        std::fs::write(dir.join("mods/sodium.jar"), b"SODIUM").unwrap();
        let locked = lock(&p, "perf").unwrap();
        assert_eq!(locked.serial, 2, "a changed mod list bumps the serial");
        assert_eq!(locked.mods.len(), 1);
        assert_eq!(locked.mods[0].sha256, util::hex_encode(&util::hash_bytes(HashAlg::Sha256, b"SODIUM")));
        assert_eq!(lock(&p, "perf").unwrap().serial, 2, "locking again without changes is idempotent");

        let key = SecretKey::generate().unwrap();
        let (_, key_id) = sign(&p, "perf", &key).unwrap();
        assert_eq!(key_id, key.public().id());
        util::create_private_dir_all(&p.user_trusted_keys()).unwrap();
        util::write_atomic(&p.user_trusted_keys().join("me.pub"), key.public().encode().as_bytes(), 0o600).unwrap();
        instance::load(&p, "perf").unwrap();

        // A jar dropped in after signing: signing again is refused until it is locked,
        // and locking discards the now-stale signature.
        std::fs::write(dir.join("mods/extra.jar"), b"EXTRA").unwrap();
        assert!(sign(&p, "perf", &key).is_err());
        assert_eq!(lock(&p, "perf").unwrap().serial, 3);
        assert!(!dir.join(SIGNATURE_FILE).exists());
        assert!(instance::load(&p, "perf").is_err(), "unsigned after re-lock");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_recorded_source_stays_with_the_bytes_it_describes() {
        let (root, p) = paths("source");
        let dir = instance::create(&p, "perf", None).unwrap();
        std::fs::create_dir(dir.join("mods")).unwrap();
        let mut draft = sample("perf");
        draft.mods.clear();
        std::fs::write(dir.join(MANIFEST_FILE), draft.to_bytes().unwrap()).unwrap();
        std::fs::write(dir.join("mods/a.jar"), b"AAAA").unwrap();
        std::fs::write(dir.join("mods/b.jar"), b"BBBB").unwrap();

        let from = "https://cdn.modrinth.com/data/AABBCCDD/versions/EEFFGGHH/a.jar";
        let sources = BTreeMap::from([("a.jar".to_string(), from.to_string())]);
        let locked = lock_with(&p, "perf", &sources).unwrap();
        assert_eq!(locked.serial, 2);
        let source = |m: &ProfileManifest, file: &str| m.mods.iter().find(|e| e.file == file).unwrap().source.clone();
        assert_eq!(source(&locked, "a.jar").as_deref(), Some(from));
        assert_eq!(source(&locked, "b.jar"), None);

        // A plain lock afterwards changes nothing: same serial, and the source is still there.
        let again = lock(&p, "perf").unwrap();
        assert_eq!(again.serial, 2, "a recorded source must not look like a changed mod list");
        assert_eq!(source(&again, "a.jar").as_deref(), Some(from));

        // Other bytes under the same name are not what the source described.
        std::fs::write(dir.join("mods/a.jar"), b"AAAB").unwrap();
        let changed = lock(&p, "perf").unwrap();
        assert_eq!(changed.serial, 3);
        assert_eq!(source(&changed, "a.jar"), None);
        // A source that is not an https address never gets into a manifest.
        let bad = BTreeMap::from([("b.jar".to_string(), "http://example.org/b.jar".to_string())]);
        assert!(lock_with(&p, "perf", &bad).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn version_strings_are_restricted_before_reaching_a_url() {
        assert!(url_safe("v", "1.21.4").is_ok());
        assert!(url_safe("v", "0.16.10+build.1").is_ok());
        for bad in ["", "../x", "1.21/../../", "a b", "x?y=z", "1.21#frag", "%2e%2e"] {
            assert!(url_safe("v", bad).is_err(), "{bad:?}");
        }
    }
}
