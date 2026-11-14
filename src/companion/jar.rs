//! How the companion mod's jar is made. This file is compiled twice: as part of `build.rs`,
//! which embeds the result in the launcher, and as part of the tests, which make the jar
//! again and compare. It uses nothing but the standard library.
//!
//! **Hermetic.** Inputs are the sources under `java/companion`, the compile-only stand-ins
//! under `java/stubs`, the project's `LICENSE`, and the system `javac`. No network, no build
//! tool, no annotation processor — and no Minecraft: Mojang's jar is not needed (and could not
//! be shipped) to build it. The stand-ins are compiled into directories of their own that are
//! on the compiler's class path and never in the archive.
//!
//! **Several Minecraft versions.** The mod is compiled in passes (see [`build_against`]):
//!
//! 1. `java/stubs/common` — Fabric Loader, Mixin and ASM types, names and signatures only;
//! 2. `java/companion/src` — everything that does not name a game class (the features, the
//!    platform API, the phone), against (1);
//! 3. for each *family* of Minecraft versions listed in `java/stubs/tools/families.txt`: its
//!    stubs `java/stubs/minecraft/<family>` (generated offline from Mojang's and Fabric's
//!    published files by `java/stubs/tools/regenerate.py`, names and signatures only);
//! 4. each family's *group* sources, `java/companion/platform/<group>`, against (1), (2) and
//!    the stubs of **every** family of the group — which must all give the same class files
//!    (the code names only what those families have in common, byte for byte); one copy is
//!    packed;
//! 5. each family's own sources, `java/companion/platform/<family>`, against (1), (2), its
//!    group's classes and its stubs.
//!
//! Which family runs is decided in the game (`Families.java`, from a table generated here out
//! of `families.txt`).
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
/// Fabric Loader, Mixin and ASM stand-ins.
const COMMON_STUBS: &str = "java/stubs/common";
/// One directory of generated stand-ins per family of Minecraft versions.
const MINECRAFT_STUBS: &str = "java/stubs/minecraft";
/// The families: `<family> <group> <representative> <version>...` per line.
pub const FAMILIES: &str = "java/stubs/tools/families.txt";
/// Group and family sources: `java/companion/platform/<group or family>`.
const PLATFORM_SOURCES: &str = "java/companion/platform";
/// Where the mod's version is substituted in `fabric.mod.json`.
const VERSION_MARK: &str = "@VERSION@";

pub struct Built {
    pub jar: Vec<u8>,
    /// What `javac -Xlint:all` had to say, if anything.
    pub warnings: String,
}

/// One family of Minecraft versions (a line of [`FAMILIES`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub id: String,
    pub group: String,
    pub representative: String,
    pub versions: Vec<String>,
}

fn is_identifier(word: &str) -> bool {
    !word.is_empty()
        && word.as_bytes()[0].is_ascii_alphabetic()
        && word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn is_version(word: &str) -> bool {
    !word.is_empty() && word.len() <= 32 && word.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

/// The families listed in `text` (the contents of [`FAMILIES`]), in order.
pub fn parse_families(text: &str) -> Result<Vec<Family>, String> {
    let mut out: Vec<Family> = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let words: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let bad = |why: &str| format!("{FAMILIES}, line {}: {why}", number + 1);
        if words.len() < 4 {
            return Err(bad("expected <family> <group> <representative> <version>..."));
        }
        let (id, group, representative) = (words[0], words[1], words[2]);
        if !is_identifier(id) || !is_identifier(group) || id == group {
            return Err(bad("family and group are Java identifiers, and different"));
        }
        let versions: Vec<String> = words[3..].iter().map(|v| v.to_string()).collect();
        if !versions.iter().all(|v| is_version(v)) || !versions.iter().any(|v| v == representative) {
            return Err(bad("versions are version numbers, and the representative is one of them"));
        }
        for earlier in &out {
            if earlier.id == id || earlier.id == group || earlier.group == id {
                return Err(bad("a family or group name is used twice"));
            }
            if let Some(v) = versions.iter().find(|v| earlier.versions.contains(v)) {
                return Err(bad(&format!("{v} is in two families")));
            }
        }
        out.push(Family { id: id.into(), group: group.into(), representative: representative.into(), versions });
    }
    if out.is_empty() {
        return Err(format!("{FAMILIES} lists no family"));
    }
    Ok(out)
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

/// The Java source of the family table the mod picks its family from.
fn family_table(families: &[Family]) -> String {
    let mut rows = String::new();
    for family in families {
        let versions: Vec<String> = family.versions.iter().map(|v| format!("\"{v}\"")).collect();
        rows.push_str(&format!("        {{\"{}\", \"{}\", {}}},\n", family.id, family.group, versions.join(", ")));
    }
    format!(
        "package bastion.companion.platform;\n\n/** Generated from java/stubs/tools/families.txt by the build. */\nfinal class FamilyTable {{\n    \
         /** Per family: its id, its group, then every Minecraft version in it. */\n    static final String[][] ROWS = {{\n{rows}    }};\n\n    \
         private FamilyTable() {{\n    }}\n}}\n"
    )
}

/// As [`build`], but with `real` — the genuine Fabric Loader, Mixin and ASM jars — on the
/// class path in place of the common stand-ins. The result must be the same bytes: that is the
/// test that those stand-ins are faithful. (The Minecraft stand-ins are always used: the
/// game's jar is never an input.)
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
    let families_text = fs::read_to_string(root.join(FAMILIES)).map_err(|e| format!("reading {FAMILIES}: {e}"))?;
    let families = parse_families(&families_text)?;
    let _ = fs::remove_dir_all(work);
    let stubs = work.join("stubs");
    let (common_stubs, classes, generated) = (stubs.join("common"), work.join("classes"), work.join("generated"));
    // An empty directory to name as the source path, so that javac looks for sources nowhere.
    let nowhere = work.join("empty");

    // The mod's generated sources: its version, and the family table.
    let build_info = generated.join("bastion/companion/BuildInfo.java");
    let table = generated.join("bastion/companion/platform/FamilyTable.java");
    for (path, text) in [
        (
            &build_info,
            format!(
                "package bastion.companion;\n\nfinal class BuildInfo {{\n    static final String VERSION = \"{version}\";\n\n    \
                 private BuildInfo() {{\n    }}\n}}\n"
            ),
        ),
        (&table, family_table(&families)),
    ] {
        fs::create_dir_all(path.parent().expect("has a parent")).map_err(|e| format!("creating {work:?}: {e}"))?;
        fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))?;
    }

    let below = |dir: &Path| -> Result<Vec<PathBuf>, String> {
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        Ok(files_below(dir)?.into_iter().filter(|f| f.ends_with(".java")).map(|f| dir.join(f)).collect())
    };
    let path_of = |dirs: &[&Path]| -> Result<std::ffi::OsString, String> {
        std::env::join_paths(dirs).map_err(|e| format!("class path: {e}"))
    };
    let mut warnings = String::new();
    // 1. The common stand-ins, or the real jars.
    let common: Vec<PathBuf> = match real {
        Some(jars) => jars.to_vec(),
        None => {
            warnings =
                javac(compiler, nowhere.as_os_str(), &nowhere, &common_stubs, &below(&root.join(COMMON_STUBS))?)?;
            vec![common_stubs.clone()]
        }
    };
    let common_refs: Vec<&Path> = common.iter().map(PathBuf::as_path).collect();
    // 2. The version-independent part of the mod.
    let mut sources = below(&root.join(SOURCES))?;
    sources.push(build_info);
    sources.push(table);
    warnings.push_str(&javac(compiler, &path_of(&common_refs)?, &nowhere, &classes, &sources)?);

    // 3. Every family's stand-ins.
    for family in &families {
        let dir = root.join(MINECRAFT_STUBS).join(&family.id);
        let family_sources = below(&dir)?;
        if family_sources.is_empty() {
            return Err(format!("{} holds no stand-ins (java/stubs/tools/regenerate.py writes them)", dir.display()));
        }
        javac(compiler, nowhere.as_os_str(), &nowhere, &stubs.join(&family.id), &family_sources)?;
    }

    // 4. Each group, against every family of the group: the same class files each time.
    let mut packed = vec![classes.clone()];
    let mut groups: Vec<&str> = Vec::new();
    for family in &families {
        if !groups.contains(&family.group.as_str()) {
            groups.push(&family.group);
        }
    }
    for group in &groups {
        let group_sources = below(&root.join(PLATFORM_SOURCES).join(group))?;
        let out = work.join("group").join(group);
        if group_sources.is_empty() {
            fs::create_dir_all(&out).map_err(|e| format!("creating {}: {e}", out.display()))?;
            continue;
        }
        // The family whose class files were taken first, and those files.
        type Compiled = (String, Vec<(String, Vec<u8>)>);
        let mut first: Option<Compiled> = None;
        for family in families.iter().filter(|f| f.group == *group) {
            let each = work.join("group-check").join(group).join(&family.id);
            let family_stubs = stubs.join(&family.id);
            let mut path: Vec<&Path> = common_refs.clone();
            path.push(&classes);
            path.push(&family_stubs);
            let said = javac(compiler, &path_of(&path)?, &nowhere, &each, &group_sources)
                .map_err(|e| format!("the {group} sources against the stand-ins of {}: {e}", family.id))?;
            let mut files = Vec::new();
            for class in files_below(&each)? {
                let bytes = fs::read(each.join(&class)).map_err(|e| format!("reading {class}: {e}"))?;
                files.push((class, bytes));
            }
            match &first {
                None => {
                    warnings.push_str(&said);
                    first = Some((family.id.clone(), files));
                }
                Some((id, expected)) => {
                    if *expected != files {
                        return Err(format!(
                            "the {group} sources compile differently against the stand-ins of {} and of {id}: they \
                             name something the two families do not have in common (move it into the families' own \
                             sources)",
                            family.id
                        ));
                    }
                }
            }
        }
        fs::create_dir_all(work.join("group")).map_err(|e| format!("creating {}: {e}", out.display()))?;
        fs::rename(work.join("group-check").join(group).join(&first.expect("a group has families").0), &out)
            .map_err(|e| format!("moving the {group} classes: {e}"))?;
        packed.push(out);
    }

    // 5. Each family's own sources.
    for family in &families {
        let family_sources = below(&root.join(PLATFORM_SOURCES).join(&family.id))?;
        if family_sources.is_empty() {
            continue;
        }
        let out = work.join("family").join(&family.id);
        let (group_classes, family_stubs) = (work.join("group").join(&family.group), stubs.join(&family.id));
        let mut path: Vec<&Path> = common_refs.clone();
        path.push(&classes);
        path.push(&group_classes);
        path.push(&family_stubs);
        warnings.push_str(
            &javac(compiler, &path_of(&path)?, &nowhere, &out, &family_sources)
                .map_err(|e| format!("the sources of {}: {e}", family.id))?,
        );
        packed.push(out);
    }

    let read = |path: PathBuf| fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()));
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for dir in &packed {
        for class in files_below(dir)? {
            let bytes = read(dir.join(&class))?;
            entries.push((class, bytes));
        }
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
