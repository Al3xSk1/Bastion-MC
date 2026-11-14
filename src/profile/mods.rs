//! Strict verification of an instance's `mods/` directory.
//!
//! The directory must contain *exactly* the files the signed manifest lists: nothing extra
//! (no stray jar, no `.disabled` file, no sub-directory, no symlink, no hidden file), nothing
//! missing, every file a regular file with the signed size and SHA-256. The returned
//! descriptors are the ones bound into the sandbox, so a file swapped after this check is
//! simply not the file the JVM gets.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{File, OpenOptions},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::Path,
};

use super::manifest::ModEntry;
use crate::{
    error::{Context, Error, Result},
    paths::validate_file_name,
    util::{self, Expected, HashAlg},
};

pub struct VerifiedMod {
    pub file_name: String,
    pub file: File,
}

fn open_dir(dir: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(dir)
}

/// Names in a directory, read through an already-open descriptor (no path re-resolution).
fn list(dir: &File) -> Result<Vec<Vec<u8>>> {
    let dup = dir.try_clone().ctx(|| "duplicating directory descriptor")?;
    let mut handle = nix::dir::Dir::from_fd(dup.into()).ctx(|| "opening directory stream")?;
    let mut names = Vec::new();
    for entry in handle.iter() {
        let entry = entry.ctx(|| "reading directory")?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            names.push(name.to_vec());
        }
    }
    names.sort();
    Ok(names)
}

/// Verify `dir` against the manifest's mod list. An absent directory is acceptable only when
/// no mods are expected.
pub fn verify_mods_dir(dir: &Path, expected: &[ModEntry]) -> Result<Vec<VerifiedMod>> {
    verify_listed(dir, expected, "mod", "the signed manifest")
}

/// The same check for any directory that must hold exactly a recorded set of files.
/// `noun` names one file ("mod", "resource pack"), `list` the record it is checked against.
pub fn verify_listed(dir: &Path, expected: &[ModEntry], noun: &str, list_name: &str) -> Result<Vec<VerifiedMod>> {
    let handle = match open_dir(dir) {
        Ok(h) => h,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && expected.is_empty() => return Ok(Vec::new()),
        Err(e) => {
            return Err(Error::Policy(format!(
                "{noun} directory {} cannot be opened as a real directory: {e}",
                dir.display()
            )));
        }
    };

    let present = list(&handle)?;
    let wanted: BTreeSet<&[u8]> = expected.iter().map(|m| m.file.as_bytes()).collect();
    let unexpected: Vec<String> = present
        .iter()
        .filter(|name| !wanted.contains(name.as_slice()))
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect();
    if !unexpected.is_empty() {
        let shown: Vec<String> = unexpected.iter().take(10).map(|n| util::sanitize_line(n.as_bytes(), 120)).collect();
        return Err(Error::Policy(format!(
            "{} unexpected entr{} in {} not covered by {list_name}: {}{}",
            unexpected.len(),
            if unexpected.len() == 1 { "y" } else { "ies" },
            dir.display(),
            shown.join(", "),
            if unexpected.len() > shown.len() { ", …" } else { "" }
        )));
    }

    let mut out = Vec::with_capacity(expected.len());
    for entry in expected {
        validate_file_name(&entry.file)?;
        let file = util::openat_regular_nofollow(&handle, OsStr::from_bytes(entry.file.as_bytes())).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Policy(format!(
                    "{noun} {:?} is listed in {list_name} but missing from {}",
                    entry.file,
                    dir.display()
                ))
            } else {
                Error::Policy(format!("{noun} {:?} is not a plain regular file ({e})", entry.file))
            }
        })?;
        // Freeze, then hash: an in-place rewrite between the two is no longer possible.
        let _ = crate::verity::enable(&file);
        Expected::sha256(&entry.sha256)?.verify_file(&file, Some(entry.size), &format!("{noun} {}", entry.file))?;
        out.push(VerifiedMod { file_name: entry.file.clone(), file });
    }
    Ok(out)
}

/// Hash what is currently in `dir` to produce manifest entries (used by `profile lock`; the
/// result still has to be reviewed and signed by a human).
pub fn scan_mods_dir(dir: &Path) -> Result<Vec<ModEntry>> {
    scan_dir(dir, ".jar", "locking the profile")
}

/// Hash every file in `dir`, all of which must be plain files named `*<extension>`.
pub fn scan_dir(dir: &Path, extension: &str, before: &str) -> Result<Vec<ModEntry>> {
    let handle = match open_dir(dir) {
        Ok(h) => h,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(format!("opening {}", dir.display()), e)),
    };
    let mut out = Vec::new();
    for name in list(&handle)? {
        let name =
            String::from_utf8(name).map_err(|_| Error::Policy(format!("non-UTF-8 file name in {}", dir.display())))?;
        validate_file_name(&name)?;
        if !name.ends_with(extension) {
            return Err(Error::Policy(format!(
                "{} contains {name:?}, which is not a {extension}; remove it before {before}",
                dir.display()
            )));
        }
        let file = util::openat_regular_nofollow(&handle, OsStr::new(&name))
            .map_err(|e| Error::Policy(format!("{name:?} is not a plain regular file ({e})")))?;
        let (digest, size) = {
            use std::os::unix::fs::FileExt;
            struct At<'a>(&'a File, u64);
            impl std::io::Read for At<'_> {
                fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                    let n = self.0.read_at(buf, self.1)?;
                    self.1 += n as u64;
                    Ok(n)
                }
            }
            util::hash_reader(HashAlg::Sha256, At(&file, 0)).ctx(|| format!("hashing {name}"))?
        };
        out.push(ModEntry { file: name, sha256: util::hex_encode(&digest), size, source: None });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join(format!("bastion-mods-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        fs::create_dir_all(d.join("mods")).unwrap();
        d
    }

    fn entry(name: &str, content: &[u8]) -> ModEntry {
        ModEntry {
            file: name.into(),
            sha256: util::hex_encode(&util::hash_bytes(HashAlg::Sha256, content)),
            size: content.len() as u64,
            source: None,
        }
    }

    #[test]
    fn exact_match_passes_and_returns_descriptors() {
        let root = tmpdir("ok");
        let mods = root.join("mods");
        fs::write(mods.join("a.jar"), b"AAAA").unwrap();
        fs::write(mods.join("b.jar"), b"BBBBBB").unwrap();
        let v = verify_mods_dir(&mods, &[entry("a.jar", b"AAAA"), entry("b.jar", b"BBBBBB")]).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(scan_mods_dir(&mods).unwrap(), vec![entry("a.jar", b"AAAA"), entry("b.jar", b"BBBBBB")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_kind_of_deviation_is_refused() {
        let root = tmpdir("bad");
        let mods = root.join("mods");
        let want = [entry("a.jar", b"AAAA")];
        fs::write(mods.join("a.jar"), b"AAAA").unwrap();
        assert!(verify_mods_dir(&mods, &want).is_ok());

        // unexpected jar
        fs::write(mods.join("evil.jar"), b"x").unwrap();
        assert!(matches!(verify_mods_dir(&mods, &want), Err(Error::Policy(_))));
        fs::remove_file(mods.join("evil.jar")).unwrap();

        // unexpected non-jar, hidden file, sub-directory
        for extra in ["notes.txt", ".hidden", "a.jar.disabled"] {
            fs::write(mods.join(extra), b"x").unwrap();
            assert!(verify_mods_dir(&mods, &want).is_err(), "{extra}");
            fs::remove_file(mods.join(extra)).unwrap();
        }
        fs::create_dir(mods.join("sub")).unwrap();
        assert!(verify_mods_dir(&mods, &want).is_err());
        fs::remove_dir(mods.join("sub")).unwrap();

        // modified content, same size
        fs::write(mods.join("a.jar"), b"AAAB").unwrap();
        assert!(matches!(verify_mods_dir(&mods, &want), Err(Error::Integrity(_))));
        // different size
        fs::write(mods.join("a.jar"), b"AAAAA").unwrap();
        assert!(matches!(verify_mods_dir(&mods, &want), Err(Error::Integrity(_))));

        // symlink to a file with the right content
        fs::remove_file(mods.join("a.jar")).unwrap();
        fs::write(root.join("outside.jar"), b"AAAA").unwrap();
        std::os::unix::fs::symlink(root.join("outside.jar"), mods.join("a.jar")).unwrap();
        assert!(matches!(verify_mods_dir(&mods, &want), Err(Error::Policy(_))), "symlinked mod must be refused");
        fs::remove_file(mods.join("a.jar")).unwrap();

        // missing
        assert!(matches!(verify_mods_dir(&mods, &want), Err(Error::Policy(_))));

        // mods dir itself is a symlink
        fs::remove_dir(&mods).unwrap();
        fs::create_dir(root.join("real")).unwrap();
        fs::write(root.join("real/a.jar"), b"AAAA").unwrap();
        std::os::unix::fs::symlink(root.join("real"), &mods).unwrap();
        assert!(verify_mods_dir(&mods, &want).is_err(), "symlinked mods directory must be refused");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vanilla_requires_empty_or_absent_mods_dir() {
        let root = tmpdir("vanilla");
        let mods = root.join("mods");
        assert!(verify_mods_dir(&mods, &[]).unwrap().is_empty());
        assert!(verify_mods_dir(&root.join("does-not-exist"), &[]).unwrap().is_empty());
        fs::write(mods.join("sneaky.jar"), b"x").unwrap();
        assert!(verify_mods_dir(&mods, &[]).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
