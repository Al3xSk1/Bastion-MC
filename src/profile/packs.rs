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
//!   descriptors, not paths. The archive check is repeated on those descriptors, so a pack
//!   recorded under an older, laxer check does not ride along unexamined.
//!
//! **What "checked" means.** The archive's directory is read by [`super::zipdir`], which
//! accepts one unambiguous shape of zip archive and refuses everything a second reader
//! could take for something else (see there). On the names it yields: nothing outside the
//! archive, no links or special files, and the marker of the pack's kind at the top. The
//! check is about the *directory* — what a program that opens the pack as a zip file sees.
//! Nothing is decompressed, and what the members contain is not looked at: a pack is still
//! data that the game's parsers read, inside the sandbox.
//!
//! **One set of bytes.** The source of an import may be a place the game can write (a pack
//! it downloaded into a world's folder). It is therefore read exactly once: the bytes go
//! into a copy in the instance and into the hash in the same pass, and everything after
//! that — the check, the record — is about the copy, through the descriptor it was written
//! with. A source that changes while it is read yields a copy that is checked as it is.
//!
//! The lock file is not signed: it protects against the *game* changing packs, not against
//! other programs running as the user (nothing stored in the user's own files can). Such a
//! program could also rewrite the copy between its check and its recording; the hash taken
//! again after the check only catches a change that is still there.

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    manifest::ModEntry,
    mods::{self, VerifiedMod},
    zipdir,
};
use crate::{
    error::{Context, Error, Result},
    paths::validate_file_name,
    util::{self, Expected, HashAlg},
};

pub const LOCK_FILE: &str = "packs.lock.json";
const MAX_LOCK_BYTES: u64 = 1 << 20;
/// Largest pack accepted. (Big "HD" packs reach a few hundred MiB.)
const MAX_PACK_BYTES: u64 = 2 << 30;
/// What the archive's directory may be: as many members as a classic zip archive can
/// declare (zip64 is refused, see `zipdir`), in a directory that is read in one piece.
/// (Large packs have some ten thousand members and a directory of a megabyte or two.)
const ARCHIVE_LIMITS: zipdir::Limits =
    zipdir::Limits { max_entries: 65_534, max_directory_bytes: 32 << 20, max_name_bytes: 512 };

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
/// archive is read (by position, through this descriptor); nothing is decompressed.
fn check_archive(file: &std::fs::File, kind: PackKind) -> Result<()> {
    let length = file.metadata().ctx(|| "examining the pack")?.len();
    if length > MAX_PACK_BYTES {
        return Err(Error::Policy(format!(
            "the pack is {} MiB; the limit is {} MiB",
            length >> 20,
            MAX_PACK_BYTES >> 20
        )));
    }
    let entries = zipdir::read(file, length, &ARCHIVE_LIMITS)?;
    let marker = entries.iter().any(|entry| match kind {
        PackKind::Resource => entry.name == "pack.mcmeta",
        PackKind::Shader => entry.name.starts_with("shaders/"),
    });
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
    let (digest, size) = util::hash_file(HashAlg::Sha256, file).ctx(|| "hashing the pack")?;
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
    // (Said early when it is already true; the copy below is bounded whatever this says.)
    let length = file.metadata().ctx(|| "examining the pack")?.len();
    if length > MAX_PACK_BYTES {
        return Err(Error::Policy(format!(
            "the pack is {} MiB; the limit is {} MiB",
            length >> 20,
            MAX_PACK_BYTES >> 20
        )));
    }
    import_from(instance_dir, kind, file, &name)
}

/// The part of [`import`] that handles the pack's bytes. `source` can be read once, from
/// front to back, and that is all this function is able to do with it: what it checks and
/// records afterwards is its own copy.
fn import_from(instance_dir: &Path, kind: PackKind, mut source: impl Read, name: &str) -> Result<ModEntry> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let dir = instance_dir.join(kind.dir());
    crate::instance::ensure_plain_dir(&dir)?;
    let staged = dir.join(format!(".import-{}", util::hex_encode(&util::random_bytes::<6>()?)));
    let result = (|| -> Result<ModEntry> {
        let mut copy = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&staged)
            .ctx(|| format!("creating {}", staged.display()))?;
        // The one pass over the source: every block goes into the copy and into the hash.
        let mut hasher = util::Hasher::new(HashAlg::Sha256);
        let mut size = 0u64;
        let mut block = vec![0u8; 128 * 1024];
        loop {
            let n = match source.read(&mut block) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::io("reading the pack", e)),
            };
            size += n as u64;
            if size > MAX_PACK_BYTES {
                return Err(Error::Policy(format!("the pack is larger than {} MiB", MAX_PACK_BYTES >> 20)));
            }
            hasher.update(&block[..n]);
            copy.write_all(&block[..n]).ctx(|| "copying the pack")?;
        }
        copy.sync_all().ctx(|| "writing the pack")?;
        let sha256 = util::hex_encode(&hasher.finish());

        // From here on: the copy, through the descriptor it was written with.
        check_archive(&copy, kind)?;
        if hash(&copy)? != (sha256.clone(), size) {
            return Err(Error::Integrity("the copy of the pack changed while it was being checked".into()));
        }
        std::fs::rename(&staged, dir.join(name)).ctx(|| format!("placing {name}"))?;
        Ok(ModEntry { file: name.to_string(), sha256, size, source: None })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    let entry = result?;

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
            // What is recorded must be what was checked: the archive check and the recorded
            // hash are both taken from this one descriptor (a file that was exchanged since
            // the directory was scanned fails the second).
            let file = util::open_regular_nofollow(&dir.join(&entry.file)).ctx(|| format!("opening {}", entry.file))?;
            check_archive(&file, kind).map_err(|e| Error::Policy(format!("{}: {e}", entry.file)))?;
            Expected::sha256(&entry.sha256)?.verify_file(
                &file,
                Some(entry.size),
                &format!("{} {}", kind.noun(), entry.file),
            )?;
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
        // The same archive check as at import, on the descriptor the game will be given.
        for pack in &files {
            check_archive(&pack.file, kind).map_err(|e| {
                Error::Policy(format!(
                    "{} {:?} is recorded in the pack list but does not pass the archive check: {e}. Remove it \
                     (`bastion-mc pack remove`), or re-create the zip and import it again.",
                    kind.noun(),
                    pack.file_name
                ))
            })?;
        }
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

    /// The source of an import is read once. A source that hands out something else when it
    /// is read a second time — the game swapping the file between a check and a copy — has
    /// no second time: what was checked, what is recorded and what lies in the instance are
    /// the same bytes.
    #[test]
    fn an_import_reads_its_source_once_and_records_what_it_checked() {
        /// Yields `first` once; any read after its end is the mistake this test is about.
        struct Once {
            first: std::io::Cursor<Vec<u8>>,
            done: bool,
        }
        impl Read for Once {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                assert!(!self.done, "the source was read again after it had been read to its end");
                let n = self.first.read(buf)?;
                self.done = n == 0;
                Ok(n)
            }
        }
        let root = scratch("once");
        let inst = root.join("inst");
        std::fs::create_dir_all(&inst).unwrap();
        std::fs::set_permissions(&inst, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let good = zipdir::tests::zip_of(&[("pack.mcmeta", b"{}"), ("assets/a.txt", b"a")]);
        let source = Once { first: std::io::Cursor::new(good.clone()), done: false };
        let entry = import_from(&inst, PackKind::Resource, source, "swapped.zip").unwrap();
        assert_eq!(entry.sha256, util::hex_encode(&util::hash_bytes(HashAlg::Sha256, &good)));
        assert_eq!(entry.size, good.len() as u64);
        assert_eq!(std::fs::read(inst.join("resourcepacks/swapped.zip")).unwrap(), good, "the copy is what was read");
        assert_eq!(verify(&inst).unwrap()[0].files.len(), 1);

        // What is refused leaves neither a file nor a record.
        let hostile = zipdir::tests::zip_of(&[("pack.mcmeta", b"{}"), ("../../.bashrc", b"x")]);
        let source = Once { first: std::io::Cursor::new(hostile), done: false };
        assert!(import_from(&inst, PackKind::Resource, source, "hostile.zip").is_err());
        let left: Vec<String> = std::fs::read_dir(inst.join("resourcepacks"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        assert_eq!(left, ["swapped.zip"]);
        assert_eq!(Lock::load(&inst).unwrap().unwrap().resourcepacks.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Archives that mean different things to different readers do not become packs — at
    /// import, when adopting, or at a launch if one is already recorded.
    #[test]
    fn archives_with_an_ambiguous_directory_are_refused_everywhere() {
        let root = scratch("ambiguous");
        let inst = root.join("inst");
        std::fs::create_dir_all(inst.join("resourcepacks")).unwrap();
        for dir in [inst.clone(), inst.join("resourcepacks")] {
            std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        }
        // Declares one member (`pack.mcmeta`); its directory holds three.
        let mut hidden = zipdir::tests::zip_of(&[("pack.mcmeta", b"{}"), ("../../.bashrc", b"x"), ("b", b"y")]);
        let end = hidden.len() - 22;
        hidden[end + 8..end + 12].copy_from_slice(&[1, 0, 1, 0]);
        let file = root.join("hidden.zip");
        std::fs::write(&file, &hidden).unwrap();
        let err = import(&inst, PackKind::Resource, &file, None).unwrap_err().to_string();
        assert!(err.contains("beyond the 1 members"), "{err}");
        assert!(std::fs::read_dir(inst.join("resourcepacks")).unwrap().next().is_none(), "nothing was left behind");

        // Copied in by hand: not adopted.
        std::fs::write(inst.join("resourcepacks/hidden.zip"), &hidden).unwrap();
        assert!(adopt(&inst).unwrap_err().to_string().contains("beyond the 1 members"));
        // Already recorded (by a launcher whose check was laxer): not handed to the game.
        let lock = Lock {
            resourcepacks: vec![ModEntry {
                file: "hidden.zip".into(),
                sha256: util::hex_encode(&util::hash_bytes(HashAlg::Sha256, &hidden)),
                size: hidden.len() as u64,
                source: None,
            }],
            shaderpacks: Vec::new(),
        };
        lock.save(&inst).unwrap();
        let err = verify(&inst).err().unwrap().to_string();
        assert!(err.contains("does not pass the archive check") && err.contains("pack remove"), "{err}");
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
