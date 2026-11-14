//! How the companion mod's jar is made. This file is compiled twice: as part of `build.rs`,
//! which embeds the result in the launcher, and as part of the tests, which make the jar
//! again and compare. It uses nothing but the standard library.
//!
//! **Hermetic.** Inputs are the sources under `java/companion`, the compile-only stand-ins
//! under `java/stubs`, the project's `LICENSE`, and the system `javac`. No network, no build
//! tool, no annotation processor — and no Minecraft: the mod reaches the game by name at run
//! time, so Mojang's jar is not needed (and could not be shipped) to build it. The stand-ins
//! are compiled into a directory of their own that is on the compiler's class path and never
//! in the archive.
//!
//! **Reproducible.** The same sources and the same `javac` give the same bytes, wherever
//! and whenever they are built: sources are passed in sorted order, classes carry no debug
//! information (`-g:none`: no source file names, line or variable tables) and a fixed
//! class-file version ([`CLASS_RELEASE`]); the archive is written here, entry by entry in
//! sorted order, uncompressed, with a fixed time stamp and no optional fields.
//! A different `javac` release may emit different bytecode for the same source; nothing
//! here can make two compilers agree.

use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// `javac --release`: the class-file version of the mod, and the oldest JDK that can build
/// it. Minecraft versions that ship unobfuscated need Java 25 anyway; 17 only keeps the
/// requirement on the *build* machine low.
pub const CLASS_RELEASE: u32 = 17;

const MANIFEST: &str = "META-INF/MANIFEST.MF";
const SOURCES: &str = "java/companion/src";
const RESOURCES: &str = "java/companion/resources";
const STUBS: &str = "java/stubs";
/// Where the mod's version is substituted in `fabric.mod.json`.
const VERSION_MARK: &str = "@VERSION@";

pub struct Built {
    pub jar: Vec<u8>,
    /// What `javac -Xlint:all` had to say, if anything.
    pub warnings: String,
}

/// Every regular file below `dir`, as paths relative to it with `/` separators, sorted.
fn files_below(dir: &Path) -> Result<Vec<String>, String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
        let entries = fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
            let name = entry.file_name().into_string().map_err(|n| format!("non-UTF-8 file name {n:?}"))?;
            let kind = entry.file_type().map_err(|e| format!("examining {}: {e}", entry.path().display()))?;
            let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            if kind.is_dir() {
                walk(&entry.path(), &relative, out)?;
            } else if kind.is_file() {
                out.push(relative);
            } else {
                return Err(format!("{} is neither a file nor a directory", entry.path().display()));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    out.sort();
    Ok(out)
}

/// One compiler run. Everything it may read is named here: the sources given, and classes
/// on `class_path`. Options the environment could add are removed.
fn javac(
    compiler: &OsStr,
    class_path: &OsStr,
    nowhere: &Path,
    out: &Path,
    sources: &[PathBuf],
) -> Result<String, String> {
    for dir in [nowhere, out] {
        fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    let output = Command::new(compiler)
        .arg("--release")
        .arg(CLASS_RELEASE.to_string())
        .args(["-proc:none", "-implicit:none", "-g:none", "-encoding", "UTF-8", "-Xlint:all"])
        .arg("-classpath")
        .arg(class_path)
        .arg("-sourcepath")
        .arg(nowhere)
        .arg("-d")
        .arg(out)
        .args(sources)
        .env_remove("JDK_JAVAC_OPTIONS")
        .env_remove("JAVA_TOOL_OPTIONS")
        .env_remove("_JAVA_OPTIONS")
        .env_remove("CLASSPATH")
        .output()
        .map_err(|e| {
            let name = compiler.to_string_lossy();
            format!("cannot run javac ({name}): {e}. Install a JDK of version 17 or newer (Arch: `pacman -S jdk25-openjdk`, or jdk-openjdk) or set JAVAC.")
        })?;
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        Ok(said)
    } else {
        Err(format!(
            "javac --release {CLASS_RELEASE} failed (the companion mod needs JDK {CLASS_RELEASE} or newer to build):\n{said}"
        ))
    }
}

/// Compile and pack the mod. `root` is the repository, `work` a scratch directory that is
/// emptied first; neither path ends up in the result.
pub fn build(root: &Path, compiler: &OsStr, version: &str, work: &Path) -> Result<Built, String> {
    build_against(root, compiler, version, work, None)
}

/// As [`build`], but with `real` — the genuine Fabric Loader and Mixin jars — on the class
/// path in place of the stand-ins. The result must be the same bytes: that is the test
/// that the stand-ins are faithful.
pub fn build_against(
    root: &Path,
    compiler: &OsStr,
    version: &str,
    work: &Path,
    real: Option<&[PathBuf]>,
) -> Result<Built, String> {
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+')) {
        return Err(format!("{version:?} is not usable as the mod's version"));
    }
    let _ = fs::remove_dir_all(work);
    let (stub_classes, classes, generated) = (work.join("stubs"), work.join("classes"), work.join("generated"));
    // An empty directory to name as the source path, so that javac looks for sources nowhere.
    let nowhere = work.join("empty");

    // The mod's one generated source: its version.
    let build_info = generated.join("bastion/companion/BuildInfo.java");
    fs::create_dir_all(build_info.parent().expect("has a parent")).map_err(|e| format!("creating {work:?}: {e}"))?;
    fs::write(
        &build_info,
        format!(
            "package bastion.companion;\n\nfinal class BuildInfo {{\n    static final String VERSION = \"{version}\";\n\n    \
             private BuildInfo() {{\n    }}\n}}\n"
        ),
    )
    .map_err(|e| format!("writing {}: {e}", build_info.display()))?;

    let below = |dir: &str| -> Result<Vec<PathBuf>, String> {
        let base = root.join(dir);
        Ok(files_below(&base)?.into_iter().filter(|f| f.ends_with(".java")).map(|f| base.join(f)).collect())
    };
    let mut warnings = String::new();
    let class_path = match real {
        Some(jars) => std::env::join_paths(jars).map_err(|e| format!("class path: {e}"))?,
        None => {
            warnings = javac(compiler, nowhere.as_os_str(), &nowhere, &stub_classes, &below(STUBS)?)?;
            stub_classes.into_os_string()
        }
    };
    let mut sources = below(SOURCES)?;
    sources.push(build_info);
    warnings.push_str(&javac(compiler, &class_path, &nowhere, &classes, &sources)?);

    let read = |path: PathBuf| fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()));
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for class in files_below(&classes)? {
        let bytes = read(classes.join(&class))?;
        entries.push((class, bytes));
    }
    for resource in files_below(&root.join(RESOURCES))? {
        let mut bytes = read(root.join(RESOURCES).join(&resource))?;
        if resource == "fabric.mod.json" {
            let text = String::from_utf8(bytes).map_err(|_| "fabric.mod.json is not UTF-8".to_string())?;
            if !text.contains(VERSION_MARK) {
                return Err(format!("fabric.mod.json has no {VERSION_MARK}"));
            }
            bytes = text.replace(VERSION_MARK, version).into_bytes();
        }
        entries.push((resource, bytes));
    }
    entries.push(("META-INF/LICENSE".into(), read(root.join("LICENSE"))?));
    entries.push((MANIFEST.into(), b"Manifest-Version: 1.0\r\n\r\n".to_vec()));
    Ok(Built { jar: archive(entries)?, warnings })
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// A zip archive of stored (uncompressed) entries: the manifest first, as jar readers
/// expect, then the rest by name. Every field that could vary between two runs is fixed.
///
/// Layout per entry: a local header followed by the bytes; then one central-directory
/// record per entry and the end-of-directory record. No directory entries, extra fields,
/// comments or data descriptors.
pub fn archive(mut entries: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
    /// 1980-01-01 00:00:00, the earliest time the format can express.
    const DOS_TIME: u16 = 0;
    const DOS_DATE: u16 = (1 << 5) | 1;
    /// "Version needed to extract": 1.0, the plainest kind of entry.
    const VERSION: u16 = 10;

    entries.sort_by(|a, b| (a.0 != MANIFEST, &a.0).cmp(&(b.0 != MANIFEST, &b.0)));
    if entries.len() > usize::from(u16::MAX) {
        return Err("too many entries for a plain zip archive".into());
    }
    let short = |n: usize| u16::try_from(n).map_err(|_| "a name is too long for a zip archive".to_string());
    let long = |n: usize| u32::try_from(n).map_err(|_| "too large for a plain zip archive".to_string());

    let mut out = Vec::new();
    let mut directory = Vec::new();
    for (index, (name, data)) in entries.iter().enumerate() {
        let plain = !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'$'))
            && !name.starts_with('/')
            && !name.split('/').any(|part| part.is_empty() || part == "." || part == "..");
        if !plain {
            return Err(format!("{name:?} is not usable as an entry name"));
        }
        if index > 0 && entries[index - 1].0 == *name {
            return Err(format!("{name:?} appears twice"));
        }
        let offset = long(out.len())?;
        // Shared by both headers: version, flags, method (0 = stored), time, date, CRC-32,
        // compressed size, size, name length, extra-field length.
        let mut common = Vec::with_capacity(26);
        common.extend_from_slice(&VERSION.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        common.extend_from_slice(&DOS_TIME.to_le_bytes());
        common.extend_from_slice(&DOS_DATE.to_le_bytes());
        common.extend_from_slice(&crc32(data).to_le_bytes());
        common.extend_from_slice(&long(data.len())?.to_le_bytes());
        common.extend_from_slice(&long(data.len())?.to_le_bytes());
        common.extend_from_slice(&short(name.len())?.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());

        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&common);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        directory.extend_from_slice(&VERSION.to_le_bytes()); // made by: same, host system 0
        directory.extend_from_slice(&common);
        // Comment length, disk number, internal attributes; external attributes; offset.
        directory.extend_from_slice(&[0u8; 6]);
        directory.extend_from_slice(&0u32.to_le_bytes());
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name.as_bytes());
    }
    let directory_offset = long(out.len())?;
    let count = short(entries.len())?;
    out.extend_from_slice(&directory);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]); // this disk, disk with the directory
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&long(directory.len())?.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    Ok(out)
}
