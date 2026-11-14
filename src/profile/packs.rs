//! Resource packs and shader packs: added by the user, never by the game.
//!
//! A resource pack is code-adjacent data: it is parsed by the game (images, JSON models,
//! fonts, sounds, shaders written in GLSL), and parser bugs in exactly these paths have
//! been used against players. So the game does not get to put one on disk:
//!
//! * `resourcepacks/` and `shaderpacks/` are **read-only** inside the sandbox. A pack the
//!   game downloads from a server lives in the per-launch scratch root and is gone
//!   afterwards; it cannot install itself for the next session.
//! * A pack gets into an instance through `bastion-mc pack import`, an explicit act by the
//!   user outside the game. The file is checked to be a plain zip archive of the right
//!   shape, hashed, and recorded in `packs.lock.json` in the instance directory (which the
//!   game cannot write).
//! * At every launch the two directories must contain exactly the recorded files with the
//!   recorded hashes — the same rule as for `mods/` — and the game is given the verified
//!   descriptors, not paths.
//!
//! The lock file is not signed: it protects against the *game* changing packs, not against
//! other programs running as the user (nothing stored in the user's own files can).

use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    manifest::ModEntry,
    mods::{self, VerifiedMod},
};
use crate::{
    error::{Context, Error, Result},
    paths::validate_file_name,
    util::{self, HashAlg},
};

pub const LOCK_FILE: &str = "packs.lock.json";
const MAX_LOCK_BYTES: u64 = 1 << 20;
/// Largest pack accepted. (Big "HD" packs reach a few hundred MiB.)
const MAX_PACK_BYTES: u64 = 2 << 30;
const MAX_ENTRIES: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackKind {
    Resource,
    Shader,
}

impl PackKind {
    pub const ALL: [PackKind; 2] = [PackKind::Resource, PackKind::Shader];

    /// Directory in the instance and in the game root.
    pub fn dir(self) -> &'static str {
        match self {
            PackKind::Resource => "resourcepacks",
            PackKind::Shader => "shaderpacks",
        }
    }
    pub fn noun(self) -> &'static str {
        match self {
            PackKind::Resource => "resource pack",
            PackKind::Shader => "shader pack",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    #[serde(default)]
    pub resourcepacks: Vec<ModEntry>,
    #[serde(default)]
    pub shaderpacks: Vec<ModEntry>,
}

impl Lock {
    pub fn entries(&self, kind: PackKind) -> &Vec<ModEntry> {
        match kind {
            PackKind::Resource => &self.resourcepacks,
            PackKind::Shader => &self.shaderpacks,
        }
    }
    fn entries_mut(&mut self, kind: PackKind) -> &mut Vec<ModEntry> {
        match kind {
            PackKind::Resource => &mut self.resourcepacks,
            PackKind::Shader => &mut self.shaderpacks,
        }
    }

    /// `None` when the instance has no lock file yet.
    pub fn load(instance_dir: &Path) -> Result<Option<Lock>> {
        let path = instance_dir.join(LOCK_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = util::read_small_file(&path, MAX_LOCK_BYTES)?;
        let lock: Lock = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Format(format!("{} is not a pack lock file: {e}", path.display())))?;
        for kind in PackKind::ALL {
            let mut seen = std::collections::BTreeSet::new();
            for entry in lock.entries(kind) {
                validate_file_name(&entry.file)?;
                if !entry.file.ends_with(".zip") || !seen.insert(&entry.file) {
                    return Err(Error::Format(format!("{}: bad or repeated entry {:?}", path.display(), entry.file)));
                }
            }
        }
        Ok(Some(lock))
    }

    fn save(&self, instance_dir: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| Error::Other(format!("encoding the pack lock: {e}")))?;
        util::write_atomic(&instance_dir.join(LOCK_FILE), &json, 0o600)
    }
}

/// Is `file` a zip archive that looks like a pack of this kind? Only the directory of the
/// archive is read; nothing is decompressed.
fn check_archive(file: &std::fs::File, kind: PackKind) -> Result<()> {
    let mut reader = file.try_clone().ctx(|| "duplicating the pack descriptor")?;
    reader.seek(SeekFrom::Start(0)).ctx(|| "rewinding the pack")?;
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).map_err(|_| Error::Format("not a zip archive (too short)".into()))?;
    if magic != *b"PK\x03\x04" {
        // A pack that starts with something else is a polyglot at best.
        return Err(Error::Format("not a zip archive (it does not start with a zip entry)".into()));
    }
    reader.seek(SeekFrom::Start(0)).ctx(|| "rewinding the pack")?;
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| Error::Format(format!("not a valid zip archive: {e}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(Error::Format(format!("the archive has {} entries; that is not a pack", archive.len())));
    }
    let mut marker = false;
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(|e| Error::Format(format!("archive entry {i}: {e}")))?;
        let name = entry.name().map_err(|e| Error::Format(format!("archive entry {i}: {e}")))?.into_owned();
        let unsafe_name = name.starts_with('/')
            || name.contains('\\')
            || name.contains('\0')
            || name.split('/').any(|part| part == "..")
            || name.len() > 512;
        if unsafe_name || entry.is_symlink() {
            return Err(Error::Policy(format!(
                "the archive contains an entry that points outside itself or is a link ({}); refusing it",
                util::sanitize_line(name.as_bytes(), 80)
            )));
        }
        marker |= match kind {
            PackKind::Resource => name == "pack.mcmeta",
            PackKind::Shader => name.starts_with("shaders/"),
        };
    }
    if !marker {
        return Err(Error::Format(match kind {
            PackKind::Resource => "no pack.mcmeta at the top of the archive: not a resource pack (packs zipped \
                                   with an extra folder level do not load in the game either)"
                .into(),
            PackKind::Shader => "no shaders/ folder at the top of the archive: not a shader pack".into(),
        }));
    }
    Ok(())
}

fn hash(file: &std::fs::File) -> Result<(String, u64)> {
    let mut reader = file.try_clone().ctx(|| "duplicating the pack descriptor")?;
    reader.seek(SeekFrom::Start(0)).ctx(|| "rewinding the pack")?;
    let (digest, size) = util::hash_reader(HashAlg::Sha256, reader).ctx(|| "hashing the pack")?;
    Ok((util::hex_encode(&digest), size))
}

/// Check `source`, copy it into the instance under `name` (default: its own file name) and
/// record it. Replaces an entry of the same name.
pub fn import(instance_dir: &Path, kind: PackKind, source: &Path, name: Option<&str>) -> Result<ModEntry> {
    let name =
        match name {
            Some(n) => n.to_string(),
            None => source.file_name().and_then(|n| n.to_str()).map(|n| n.replace(' ', "_")).ok_or_else(|| {
                Error::Usage("the pack's file name is not valid UTF-8; give a name with --name".into())
            })?,
        };
    validate_file_name(&name).map_err(|_| {
        Error::Usage(format!(
            "{name:?} cannot be used as the pack's name (allowed: [A-Za-z0-9._+-]); give one with --name"
        ))
    })?;
    if !name.ends_with(".zip") {
        return Err(Error::Usage("packs are .zip files (a pack in a folder: zip its contents first)".into()));
    }
    let file = util::open_regular_nofollow(source).ctx(|| format!("opening {}", source.display()))?;
    let length = file.metadata().ctx(|| "examining the pack")?.len();
    if length > MAX_PACK_BYTES {
        return Err(Error::Policy(format!(
            "the pack is {} MiB; the limit is {} MiB",
            length >> 20,
            MAX_PACK_BYTES >> 20
        )));
    }
    check_archive(&file, kind)?;
    let (sha256, size) = hash(&file)?;

    let dir = instance_dir.join(kind.dir());
    crate::instance::ensure_plain_dir(&dir)?;
    // Write the bytes that were hashed (same descriptor), then make them the pack.
    let staged = dir.join(format!(".import-{}", util::hex_encode(&util::random_bytes::<6>()?)));
    let result = (|| -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut reader = file.try_clone().ctx(|| "duplicating the pack descriptor")?;
        reader.seek(SeekFrom::Start(0)).ctx(|| "rewinding the pack")?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&staged)
            .ctx(|| format!("creating {}", staged.display()))?;
        let copied = std::io::copy(&mut reader.take(MAX_PACK_BYTES + 1), &mut out).ctx(|| "copying the pack")?;
        out.sync_all().ctx(|| "writing the pack")?;
        if copied != size {
            return Err(Error::Integrity("the pack changed while it was being imported".into()));
        }
        let (copied_hash, _) = hash(&std::fs::File::open(&staged).ctx(|| "re-reading the copy")?)?;
        if copied_hash != sha256 {
            return Err(Error::Integrity("the pack changed while it was being imported".into()));
        }
        std::fs::rename(&staged, dir.join(&name)).ctx(|| format!("placing {name}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result?;

    let entry = ModEntry { file: name.clone(), sha256, size, source: None };
    let mut lock = Lock::load(instance_dir)?.unwrap_or_default();
    let entries = lock.entries_mut(kind);
    entries.retain(|e| e.file != name);
    entries.push(entry.clone());
    entries.sort_by(|a, b| a.file.cmp(&b.file));
    lock.save(instance_dir)?;
    Ok(entry)
}

/// Remove a pack and its record.
pub fn remove(instance_dir: &Path, kind: PackKind, name: &str) -> Result<()> {
    validate_file_name(name)?;
    let mut lock = Lock::load(instance_dir)?.unwrap_or_default();
    let before = lock.entries(kind).len();
    lock.entries_mut(kind).retain(|e| e.file != name);
    if lock.entries(kind).len() == before {
        return Err(Error::Usage(format!("this instance has no {} called {name:?}", kind.noun())));
    }
    match std::fs::remove_file(instance_dir.join(kind.dir()).join(name)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(format!("removing {name}"), e)),
    }
    lock.save(instance_dir)
}

/// Record whatever is in the two directories now (after the user has looked at it): for
/// instances from before packs were locked, and for packs copied in by hand.
pub fn adopt(instance_dir: &Path) -> Result<Lock> {
    let mut lock = Lock::default();
    for kind in PackKind::ALL {
        let dir = instance_dir.join(kind.dir());
        let entries = mods::scan_dir(&dir, ".zip", "recording the packs")?;
        for entry in &entries {
            let file = util::open_regular_nofollow(&dir.join(&entry.file)).ctx(|| format!("opening {}", entry.file))?;
            check_archive(&file, kind).map_err(|e| Error::Policy(format!("{}: {e}", entry.file)))?;
        }
        *lock.entries_mut(kind) = entries;
    }
    lock.save(instance_dir)?;
    Ok(lock)
}

pub struct Verified {
    pub kind: PackKind,
    pub files: Vec<VerifiedMod>,
}

/// What the game is given: for each kind, exactly the recorded files, verified. An
/// instance without a lock file must have both directories empty.
pub fn verify(instance_dir: &Path) -> Result<Vec<Verified>> {
    let lock = Lock::load(instance_dir)?;
    let mut out = Vec::new();
    for kind in PackKind::ALL {
        let dir: PathBuf = instance_dir.join(kind.dir());
        let expected: &[ModEntry] = lock.as_ref().map(|l| l.entries(kind).as_slice()).unwrap_or(&[]);
        let files = mods::verify_listed(&dir, expected, kind.noun(), "the instance's pack list (packs.lock.json)")
            .map_err(|e| match e {
                Error::Policy(text) => Error::Policy(format!(
                    "{text}. Packs are added with `bastion-mc pack import`; if you put these files there yourself \
                     and have looked at them, record them with `bastion-mc pack adopt <instance>`."
                )),
                other => other,
            })?;
        out.push(Verified { kind, files });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bastion-packs-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        dir
    }

    fn zip_with(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn import_verify_remove() {
        let root = scratch("flow");
        let inst = root.join("inst");
        std::fs::create_dir_all(&inst).unwrap();
        std::fs::set_permissions(&inst, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        // No lock, nothing there: fine.
        assert!(verify(&inst).unwrap().iter().all(|v| v.files.is_empty()));

        let pack = root.join("My Pack.zip");
        zip_with(&pack, &[("pack.mcmeta", b"{}"), ("assets/minecraft/textures/block/stone.png", b"png")]);
        let entry = import(&inst, PackKind::Resource, &pack, None).unwrap();
        assert_eq!(entry.file, "My_Pack.zip");
        assert_eq!(entry.size, std::fs::metadata(&pack).unwrap().len());
        let shader = root.join("shader.zip");
        zip_with(&shader, &[("shaders/final.fsh", b"void main(){}")]);
        import(&inst, PackKind::Shader, &shader, Some("nice-shader.zip")).unwrap();

        let verified = verify(&inst).unwrap();
        assert_eq!(verified[0].files.len(), 1);
        assert_eq!(verified[1].files[0].file_name, "nice-shader.zip");

        // The game (or anything else) changed a pack, added one, or removed one: refused.
        std::fs::write(inst.join("resourcepacks/My_Pack.zip"), b"PK\x03\x04 tampered").unwrap();
        let err = verify(&inst).err().unwrap();
        assert!(matches!(err, Error::Integrity(_)), "{err}");
        import(&inst, PackKind::Resource, &pack, None).unwrap(); // importing again repairs it
        std::fs::write(inst.join("resourcepacks/dropped-by-something.zip"), b"x").unwrap();
        let err = verify(&inst).err().unwrap().to_string();
        assert!(err.contains("dropped-by-something.zip") && err.contains("pack import"), "{err}");
        std::fs::remove_file(inst.join("resourcepacks/dropped-by-something.zip")).unwrap();
        std::fs::remove_file(inst.join("shaderpacks/nice-shader.zip")).unwrap();
        assert!(verify(&inst).err().unwrap().to_string().contains("missing"));

        remove(&inst, PackKind::Shader, "nice-shader.zip").unwrap();
        assert!(remove(&inst, PackKind::Shader, "nice-shader.zip").is_err());
        remove(&inst, PackKind::Resource, "My_Pack.zip").unwrap();
        assert!(!inst.join("resourcepacks/My_Pack.zip").exists());
        assert!(verify(&inst).unwrap().iter().all(|v| v.files.is_empty()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn files_that_are_not_packs_are_refused_at_import() {
        let root = scratch("refuse");
        let inst = root.join("inst");
        std::fs::create_dir_all(&inst).unwrap();
        std::fs::set_permissions(&inst, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let good = root.join("good.zip");
        zip_with(&good, &[("pack.mcmeta", b"{}")]);

        let not_zip = root.join("notes.zip");
        std::fs::write(&not_zip, b"just text").unwrap();
        let nested = root.join("nested.zip");
        zip_with(&nested, &[("MyPack/pack.mcmeta", b"{}")]);
        let escaping = root.join("escaping.zip");
        zip_with(&escaping, &[("pack.mcmeta", b"{}"), ("../../.bashrc", b"x")]);
        let absolute = root.join("absolute.zip");
        zip_with(&absolute, &[("pack.mcmeta", b"{}"), ("/etc/cron.d/x", b"x")]);
        // A polyglot: something else first, a valid zip appended.
        let polyglot = root.join("polyglot.zip");
        let mut bytes = b"#!/bin/sh\necho hi\n".to_vec();
        bytes.extend(std::fs::read(&good).unwrap());
        std::fs::write(&polyglot, bytes).unwrap();
        let link = root.join("link.zip");
        std::os::unix::fs::symlink(&good, &link).unwrap();

        for bad in [&not_zip, &nested, &escaping, &absolute, &polyglot, &link] {
            assert!(import(&inst, PackKind::Resource, bad, None).is_err(), "{}", bad.display());
        }
        assert!(import(&inst, PackKind::Shader, &good, None).is_err(), "a resource pack is not a shader pack");
        assert!(import(&inst, PackKind::Resource, &good, Some("../evil.zip")).is_err());
        assert!(import(&inst, PackKind::Resource, &good, Some("pack.jar")).is_err());
        // Nothing was left behind by the refusals.
        let left: Vec<_> =
            std::fs::read_dir(inst.join("resourcepacks")).map(|d| d.flatten().collect()).unwrap_or_default();
        assert!(left.is_empty(), "{left:?}");
        assert!(Lock::load(&inst).unwrap().is_none());

        // Adopting records what is there — and applies the same checks.
        std::fs::create_dir_all(inst.join("resourcepacks")).unwrap();
        std::fs::set_permissions(inst.join("resourcepacks"), std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        std::fs::copy(&good, inst.join("resourcepacks/by-hand.zip")).unwrap();
        assert!(verify(&inst).is_err(), "unrecorded packs stop a launch");
        let lock = adopt(&inst).unwrap();
        assert_eq!(lock.resourcepacks.len(), 1);
        assert_eq!(verify(&inst).unwrap()[0].files.len(), 1);
        std::fs::copy(&escaping, inst.join("resourcepacks/escaping.zip")).unwrap();
        assert!(adopt(&inst).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
