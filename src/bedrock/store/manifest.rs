//! The per-file manifest of an imported version (`versions/<id>/manifest`).
//!
//! Written last by an import, read by every launch: it lists every directory and file of the
//! version's `game/` tree with each file's size and SHA-256, and what the version is. The
//! store is a directory of the user's, so the manifest is input like any other: the reader is
//! strict (one exact line grammar, known keys in a fixed order, entries sorted and unique,
//! bounded counts and lengths, totals that must add up) and refuses anything else as an
//! integrity failure. It does not make the files trustworthy — anybody who can write the
//! store can write a matching manifest — it makes them *checkable*: what a launch runs is
//! exactly what the import copied, unless something with the user's rights changed both.
//!
//! ```text
//! bastion-mc bedrock version 1
//! package com.mojang.minecraftpe
//! version-name 1.26.62.1
//! version-code 982606201
//! abi x86_64
//! imported 1794617499
//! entries 3
//! bytes 1234
//! f <sha256> <size> AndroidManifest.xml
//! d assets
//! ...
//! end
//! ```
//!
//! Paths are relative to `game/`, `/`-separated, UTF-8 without control characters; the path
//! is the last field of its line, so it may contain spaces.

use crate::{
    error::{Error, Result},
    util,
};

pub const MAGIC: &str = "bastion-mc bedrock version 1";
/// The only ABI a version may be imported for.
pub const ABI: &str = "x86_64";
pub const PACKAGE: &str = "com.mojang.minecraftpe";

/// Bounds shared with the import (see `store` for why each is what it is).
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_NAME_BYTES: usize = 255;
pub const MAX_PATH_BYTES: usize = 1024;
/// Components of a path (the import's depth limit is below this).
pub const MAX_COMPONENTS: usize = 32;
pub const MAX_BYTES: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File { size: u64, sha256: [u8; 32] },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub package: String,
    pub version_name: String,
    pub version_code: u32,
    /// Seconds since the epoch.
    pub imported: u64,
    /// Sorted by path (bytes), unique, every parent listed before its children.
    pub entries: Vec<Entry>,
}

impl Manifest {
    pub fn files(&self) -> u64 {
        self.entries.iter().filter(|e| matches!(e.kind, Kind::File { .. })).count() as u64
    }

    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| if let Kind::File { size, .. } = e.kind { size } else { 0 }).sum()
    }
}

fn bad(why: impl Into<String>) -> Error {
    Error::Integrity(format!("the version's manifest is damaged: {}", why.into()))
}

/// One component of a path inside a version: what a directory entry may be called.
pub fn validate_name(name: &str) -> std::result::Result<(), &'static str> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("an empty, \".\" or \"..\" name");
    }
    if name.len() > MAX_NAME_BYTES {
        return Err("a name longer than 255 bytes");
    }
    if name.contains('/') {
        return Err("a name with a slash");
    }
    if name.chars().any(char::is_control) {
        return Err("a name with a control character");
    }
    Ok(())
}

pub fn validate_path(path: &str) -> std::result::Result<(), &'static str> {
    if path.len() > MAX_PATH_BYTES {
        return Err("a path longer than 1024 bytes");
    }
    if path.split('/').count() > MAX_COMPONENTS {
        return Err("a path nested more than 32 levels deep");
    }
    path.split('/').try_for_each(validate_name)
}

/// Where an entry may be in a version, and as what: `AndroidManifest.xml`, `assets/…`,
/// `lib/`, `lib/x86_64/` and the `lib*.so` files directly in it. Nothing else.
pub fn allowed(path: &str, dir: bool) -> bool {
    match path.split_once('/') {
        None => matches!((path, dir), ("AndroidManifest.xml", false) | ("assets", true) | ("lib", true)),
        Some(("assets", _)) => true,
        Some(("lib", rest)) => match rest.split_once('/') {
            None => rest == ABI && dir,
            Some((abi, name)) => abi == ABI && !dir && is_library_name(name),
        },
        _ => false,
    }
}

/// A library name as Android installs them: `lib*.so`, from a small alphabet.
pub fn is_library_name(name: &str) -> bool {
    name.starts_with("lib") && name.ends_with(".so") && name.len() > 6 && crate::paths::validate_file_name(name).is_ok()
}

fn number(text: &str) -> Option<u64> {
    let canonical =
        !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) && (text == "0" || !text.starts_with('0'));
    if canonical { text.parse().ok() } else { None }
}

pub fn encode(manifest: &Manifest) -> Vec<u8> {
    let mut out = String::new();
    out.push_str(MAGIC);
    out.push('\n');
    out.push_str(&format!("package {}\n", manifest.package));
    out.push_str(&format!("version-name {}\n", manifest.version_name));
    out.push_str(&format!("version-code {}\n", manifest.version_code));
    out.push_str(&format!("abi {ABI}\n"));
    out.push_str(&format!("imported {}\n", manifest.imported));
    out.push_str(&format!("entries {}\n", manifest.entries.len()));
    out.push_str(&format!("bytes {}\n", manifest.bytes()));
    for entry in &manifest.entries {
        match &entry.kind {
            Kind::Dir => out.push_str(&format!("d {}\n", entry.path)),
            Kind::File { size, sha256 } => {
                out.push_str(&format!("f {} {size} {}\n", util::hex_encode(sha256), entry.path))
            }
        }
    }
    out.push_str("end\n");
    out.into_bytes()
}

/// Reads a manifest. Anything that is not exactly what [`encode`] writes is an integrity
/// failure.
pub fn parse(data: &[u8]) -> Result<Manifest> {
    if data.len() > MAX_BYTES {
        return Err(bad(format!("{} bytes; the limit is {MAX_BYTES}", data.len())));
    }
    let text = std::str::from_utf8(data).map_err(|_| bad("not UTF-8"))?;
    let body = text.strip_suffix('\n').ok_or_else(|| bad("does not end with a line break"))?;
    let mut lines = body.split('\n');
    let mut field = |key: &str| -> Result<&str> {
        let line = lines.next().ok_or_else(|| bad(format!("ends before {key}")))?;
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(' '))
            .ok_or_else(|| bad(format!("expected {key}, found {:?}", util::sanitize_line(line.as_bytes(), 60))))
    };
    let magic = field("bastion-mc")?;
    if magic != "bedrock version 1" {
        return Err(bad("an unknown format"));
    }
    let package = field("package")?.to_string();
    let version_name = field("version-name")?.to_string();
    let version_code = number(field("version-code")?)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| bad("version-code"))?;
    if field("abi")? != ABI {
        return Err(bad("an ABI other than x86_64"));
    }
    let imported = number(field("imported")?).ok_or_else(|| bad("imported"))?;
    let count =
        number(field("entries")?).map(|n| n as usize).filter(|n| *n <= MAX_ENTRIES).ok_or_else(|| bad("entries"))?;
    let bytes = number(field("bytes")?).ok_or_else(|| bad("bytes"))?;
    if package != PACKAGE {
        return Err(bad("a package other than Minecraft's"));
    }
    if version_name.is_empty() || version_name.chars().count() > 128 || version_name.chars().any(char::is_control) {
        return Err(bad("version-name"));
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(count.min(4096));
    let mut dirs: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut total: u64 = 0;
    for _ in 0..count {
        let line = lines.next().ok_or_else(|| bad("fewer entries than it says"))?;
        let (kind, path) = if let Some(path) = line.strip_prefix("d ") {
            (Kind::Dir, path)
        } else if let Some(rest) = line.strip_prefix("f ") {
            let mut parts = rest.splitn(3, ' ');
            let (hex, size, path) = (parts.next(), parts.next(), parts.next());
            let (Some(hex), Some(size), Some(path)) = (hex, size, path) else { return Err(bad("a short file line")) };
            if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                return Err(bad("a digest that is not 64 lowercase hex digits"));
            }
            let digest = util::hex_decode(hex).map_err(|_| bad("a digest"))?;
            let size = number(size).ok_or_else(|| bad("a size"))?;
            total = total.checked_add(size).ok_or_else(|| bad("sizes overflow"))?;
            let mut sha256 = [0u8; 32];
            sha256.copy_from_slice(&digest);
            (Kind::File { size, sha256 }, path)
        } else {
            return Err(bad(format!("an unknown line {:?}", util::sanitize_line(line.as_bytes(), 60))));
        };
        validate_path(path).map_err(|why| bad(format!("{why} in an entry")))?;
        if !allowed(path, kind == Kind::Dir) {
            return Err(bad(format!(
                "an entry outside the version layout: {:?}",
                util::sanitize_line(path.as_bytes(), 80)
            )));
        }
        if let Some(last) = entries.last()
            && last.path.as_str() >= path
        {
            return Err(bad("entries out of order or repeated"));
        }
        if let Some((parent, _)) = path.rsplit_once('/')
            && !dirs.contains(parent)
        {
            return Err(bad("an entry whose directory is not listed"));
        }
        if kind == Kind::Dir {
            dirs.insert(path);
        }
        entries.push(Entry { path: path.to_string(), kind });
    }
    if lines.next() != Some("end") || lines.next().is_some() {
        return Err(bad("more entries than it says, or no end line"));
    }
    if total != bytes {
        return Err(bad("the sizes do not add up to its byte count"));
    }
    for required in ["AndroidManifest.xml", "assets", "lib", "lib/x86_64", "lib/x86_64/libminecraftpe.so"] {
        if entries.binary_search_by(|e| e.path.as_str().cmp(required)).is_err() {
            return Err(bad(format!("{required} is not listed")));
        }
    }
    Ok(Manifest { package, version_name, version_code, imported, entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Manifest {
        let file = |path: &str, size: u64, byte: u8| Entry {
            path: path.into(),
            kind: Kind::File { size, sha256: [byte; 32] },
        };
        let dir = |path: &str| Entry { path: path.into(), kind: Kind::Dir };
        Manifest {
            package: PACKAGE.into(),
            version_name: "1.26.62.1".into(),
            version_code: 982606201,
            imported: 1_794_617_499,
            entries: vec![
                file("AndroidManifest.xml", 2048, 1),
                dir("assets"),
                dir("assets/fonts"),
                file("assets/fonts/Mojangles.ttf", 30_000, 2),
                file("assets/sounds and music.txt", 0, 3),
                dir("lib"),
                dir("lib/x86_64"),
                file("lib/x86_64/libc++_shared.so", 1_000_000, 4),
                file("lib/x86_64/libminecraftpe.so", 150_000_000, 5),
            ],
        }
    }

    #[test]
    fn a_manifest_reads_back_as_it_was_written() {
        let manifest = sample();
        let text = encode(&manifest);
        assert_eq!(parse(&text).unwrap(), manifest);
        assert_eq!(manifest.files(), 5);
        assert_eq!(manifest.bytes(), 151_032_048);
        assert!(String::from_utf8(text).unwrap().contains("\nf 0505050505050505050505050505050505050505050505050505050505050505 150000000 lib/x86_64/libminecraftpe.so\n"));
    }

    #[test]
    fn anything_but_the_exact_grammar_is_an_integrity_failure() {
        let good = String::from_utf8(encode(&sample())).unwrap();
        let cases: Vec<(String, &str)> = vec![
            (good.replace("version-code 982606201", "version-code 0982606201"), "leading zero"),
            (good.replace("abi x86_64", "abi arm64-v8a"), "another ABI"),
            (good.replace("package com.mojang.minecraftpe", "package com.example.app"), "another package"),
            (good.replace("entries 9", "entries 8"), "fewer entries than listed"),
            (good.replace("entries 9", "entries 10"), "more entries than listed"),
            (good.replace("bytes 151032048", "bytes 151032049"), "totals"),
            (good.replace("d assets/fonts\n", ""), "a parent missing (and the count)"),
            (good.replace("assets/fonts/Mojangles.ttf", "assets/fonts/../../etc/passwd"), "an escape"),
            (good.replace("assets/fonts/Mojangles.ttf", "assets/fonts//x"), "an empty component"),
            (good.replace("lib/x86_64/libc++_shared.so", "lib/x86_64/run.sh"), "a non-library in lib"),
            (good.replace("d lib/x86_64\n", "d lib/x86\n"), "another ABI directory"),
            (good.replace("0505050505", "05050505ZZ"), "a bad digest"),
            (good.replace("0505050505", "05050505AB"), "upper-case hex"),
            (good.replace("\nend\n", "\n"), "no end line"),
            (good.replace("\nend\n", "\nend\nextra\n"), "trailing data"),
            (good.replace("Mojangles.ttf", "Mojangles\u{7}.ttf"), "a control character"),
            (good.clone() + "\n", "a blank line after the end"),
            (good.trim_end().to_string(), "no final line break"),
        ];
        for (text, what) in cases {
            let err = parse(text.as_bytes()).expect_err(what);
            assert!(matches!(err, Error::Integrity(_)), "{what}: {err}");
        }
        // Out of order: swap two file lines.
        let swapped = good.replace(
            "lib/x86_64/libc++_shared.so\nf 0505050505050505050505050505050505050505050505050505050505050505 150000000 lib/x86_64/libminecraftpe.so",
            "lib/x86_64/libminecraftpe.so\nf 0505050505050505050505050505050505050505050505050505050505050505 150000000 lib/x86_64/libc++_shared.so",
        );
        assert_ne!(swapped, good);
        assert!(parse(swapped.as_bytes()).is_err());
    }

    #[test]
    fn the_layout_admits_the_three_things_the_client_reads_and_nothing_else() {
        assert!(allowed("AndroidManifest.xml", false));
        assert!(allowed("assets/resource_packs/vanilla/manifest.json", false));
        assert!(allowed("assets/x", true));
        assert!(allowed("lib/x86_64", true));
        assert!(allowed("lib/x86_64/libminecraftpe.so", false));
        for (path, dir) in [
            ("AndroidManifest.xml", true),
            ("classes.dex", false),
            ("lib/arm64-v8a", true),
            ("lib/x86_64/libfoo.so", true),
            ("lib/x86_64/sub/libfoo.so", false),
            ("lib/x86_64/gdbserver", false),
            ("lib/libfoo.so", false),
            ("res/raw/x.ogg", false),
        ] {
            assert!(!allowed(path, dir), "{path}");
        }
    }
}
