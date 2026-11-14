//! What a mod jar is able to reach, read off its class files.
//!
//! When you sign a profile you vouch for every jar in it. Reading a diff of a mod's source
//! is rarely possible; knowing that version 1.4 of a shader mod suddenly references
//! `ProcessBuilder` and `java.net.Socket` is, and it is the question worth asking the
//! author. This module lists, per jar, which *sensitive* parts of the Java platform its
//! classes refer to, and `profile lock` shows what is new since the previous lock.
//!
//! It is a static scan of constant pools: every class, method and field a class file uses
//! by name appears there. It follows jars nested inside jars (Fabric's `META-INF/jars`) and
//! reports bundled native libraries.
//!
//! **A signal for review, not a verdict.** Code that builds a class name at run time
//! (`Class.forName("java.lang.Run" + "time")`), loads classes from bytes, or calls through
//! method handles shows up here only as "reflection" or "defines classes at run time" —
//! which is exactly why those two are listed: a jar with them can do anything on this
//! list. And nothing here is enforcement; the sandbox is.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Context, Error, Result},
    util,
};

/// File in the instance directory holding the capabilities seen at the last lock. Not part
/// of what is signed: it only serves the comparison.
pub const SIDE_FILE: &str = "capabilities.json";

const MAX_ENTRIES: usize = 200_000;
const MAX_CLASS_BYTES: u64 = 8 << 20;
const MAX_NESTED_JAR_BYTES: u64 = 256 << 20;
/// All class and nested-jar bytes read from one top-level jar.
const MAX_TOTAL_BYTES: u64 = 2 << 30;
const MAX_DEPTH: usize = 4;
/// Classes named per capability (the count is always complete).
const EXAMPLES: usize = 4;

/// (capability, what it means, class-name prefixes or `Class.method` names that show it)
const RULES: &[(&str, &str, &[&str])] = &[
    (
        "runs programs",
        "starts other processes",
        &["java/lang/ProcessBuilder", "java/lang/Runtime.exec", "java/lang/ProcessHandle", "java/lang/Process."],
    ),
    (
        "network",
        "opens its own network connections",
        &[
            "java/net/Socket",
            "java/net/ServerSocket",
            "java/net/DatagramSocket",
            "java/net/MulticastSocket",
            "java/net/URLConnection",
            "java/net/HttpURLConnection",
            "javax/net/ssl/HttpsURLConnection",
            "java/net/URL.openConnection",
            "java/net/URL.openStream",
            "java/net/http/",
            "java/nio/channels/SocketChannel",
            "java/nio/channels/ServerSocketChannel",
            "java/nio/channels/DatagramChannel",
            "java/nio/channels/AsynchronousSocketChannel",
            "com/sun/net/httpserver/",
            "org/apache/http/",
            "okhttp3/",
        ],
    ),
    (
        "native code",
        "loads native libraries or calls native functions",
        &[
            "java/lang/System.load",
            "java/lang/System.loadLibrary",
            "java/lang/Runtime.load",
            "java/lang/Runtime.loadLibrary",
            "java/lang/foreign/",
            "com/sun/jna/",
            "jnr/ffi/",
            "org/lwjgl/system/JNI",
            "org/lwjgl/system/MemoryUtil.memPut",
        ],
    ),
    (
        "raw memory",
        "reads and writes memory directly (sun.misc.Unsafe)",
        &["sun/misc/Unsafe", "jdk/internal/misc/Unsafe"],
    ),
    (
        "defines classes at run time",
        "can load code that is not in the jar as class files (hides everything else on this list)",
        &[
            "java/lang/ClassLoader.defineClass",
            "java/lang/invoke/MethodHandles$Lookup.defineClass",
            "java/lang/invoke/MethodHandles$Lookup.defineHiddenClass",
            "java/net/URLClassLoader",
            "java/security/SecureClassLoader",
            "javax/script/",
            "jdk/jshell/",
            "javax/tools/JavaCompiler",
        ],
    ),
    (
        "reflection",
        "calls methods chosen at run time (can hide what it calls)",
        &[
            "java/lang/reflect/Method.invoke",
            "java/lang/reflect/AccessibleObject.setAccessible",
            "java/lang/reflect/Method.setAccessible",
            "java/lang/reflect/Field.setAccessible",
            "java/lang/reflect/Constructor.newInstance",
            "java/lang/Class.forName",
            "java/lang/invoke/MethodHandles$Lookup.find",
            "java/lang/invoke/MethodHandles.privateLookupIn",
        ],
    ),
    ("deserialisation", "reads serialised Java objects", &["java/io/ObjectInputStream", "java/beans/XMLDecoder"]),
    (
        "agents and attach",
        "instruments the JVM or attaches to processes",
        &["java/lang/instrument/", "com/sun/tools/attach/", "sun/tools/attach/", "com/sun/jdi/"],
    ),
    (
        "desktop input and clipboard",
        "synthesises input, takes screenshots or uses the system clipboard through AWT",
        &["java/awt/Robot", "java/awt/datatransfer/", "java/awt/Toolkit.getSystemClipboard", "java/awt/Desktop"],
    ),
    (
        "environment",
        "reads environment variables or changes security-relevant JVM state",
        &[
            "java/lang/System.getenv",
            "java/lang/System.setSecurityManager",
            "java/lang/System.setProperty",
            "java/security/Policy",
        ],
    ),
];

/// The same APIs named in *strings*, the way reflective code refers to them.
const BY_NAME: &[(&str, &[&str])] = &[
    ("runs programs", &["java.lang.Runtime", "java.lang.ProcessBuilder"]),
    ("raw memory", &["sun.misc.Unsafe", "jdk.internal.misc.Unsafe", "theUnsafe"]),
    ("network", &["java.net.Socket", "java.net.URLConnection", "java.net.http.HttpClient"]),
    ("defines classes at run time", &["java.lang.ClassLoader", "defineClass"]),
    ("agents and attach", &["com.sun.tools.attach", "java.lang.instrument"]),
];

/// What one capability means, for display.
pub fn describe(capability: &str) -> &'static str {
    RULES.iter().find(|(name, _, _)| *name == capability).map(|(_, text, _)| *text).unwrap_or("")
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JarReport {
    /// The mod's own id (`fabric.mod.json` / `quilt.mod.json`), when it declares one.
    pub mod_id: Option<String>,
    pub classes: usize,
    /// capability → classes that show it (at most a few named; see `counts`).
    pub capabilities: BTreeMap<String, Vec<String>>,
    /// capability → number of classes that show it.
    pub counts: BTreeMap<String, usize>,
    /// Native libraries shipped inside the jar (or inside a jar in it).
    pub natives: Vec<String>,
    /// Jars found inside (paths), each scanned as part of this report.
    pub nested: Vec<String>,
    /// Things that could not be read: a truncated class, an entry over a limit.
    pub unreadable: Vec<String>,
}

impl JarReport {
    fn note(&mut self, capability: &str, class: &str) {
        let classes = self.capabilities.entry(capability.to_string()).or_default();
        if !classes.iter().any(|c| c == class) {
            *self.counts.entry(capability.to_string()).or_default() += 1;
            if classes.len() < EXAMPLES {
                classes.push(class.to_string());
            }
        }
    }

    /// The name a report is filed under across versions of the same mod.
    pub fn identity(&self, file_name: &str) -> String {
        match &self.mod_id {
            Some(id) => id.clone(),
            // "sodium-fabric-0.6.13+mc1.21.4.jar" → "sodium-fabric"
            None => {
                let stem = file_name.strip_suffix(".jar").unwrap_or(file_name);
                let end = stem
                    .char_indices()
                    .find(|(i, c)| {
                        matches!(c, '-' | '_' | '+') && stem[i + 1..].starts_with(|d: char| d.is_ascii_digit())
                    })
                    .map_or(stem.len(), |(i, _)| i);
                stem[..end].to_ascii_lowercase()
            }
        }
    }
}

/// Names a class file refers to: `java/lang/Runtime` for a class, `java/lang/Runtime.exec`
/// for a method or field, and the text of its string constants.
struct ClassRefs {
    this_class: String,
    names: BTreeSet<String>,
    strings: Vec<String>,
}

/// Read the constant pool of a class file. `None` for anything that is not a well-formed
/// class file up to the end of its constant pool.
fn class_refs(data: &[u8]) -> Option<ClassRefs> {
    struct Cursor<'a>(&'a [u8]);
    impl<'a> Cursor<'a> {
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            if self.0.len() < n {
                return None;
            }
            let (head, tail) = self.0.split_at(n);
            self.0 = tail;
            Some(head)
        }
        fn u8(&mut self) -> Option<u8> {
            self.take(1).map(|b| b[0])
        }
        fn u16(&mut self) -> Option<u16> {
            self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
        }
    }
    enum Entry<'a> {
        Utf8(&'a [u8]),
        Class(u16),
        Str(u16),
        Member(u16, u16),
        NameAndType(u16),
        Other,
    }
    let mut cursor = Cursor(data);
    if cursor.take(4)? != [0xca, 0xfe, 0xba, 0xbe] {
        return None;
    }
    cursor.take(4)?; // minor, major
    let count = cursor.u16()? as usize;
    let mut pool: Vec<Entry<'_>> = Vec::with_capacity(count);
    pool.push(Entry::Other); // index 0 is unused
    while pool.len() < count {
        let entry = match cursor.u8()? {
            1 => {
                let len = cursor.u16()? as usize;
                Entry::Utf8(cursor.take(len)?)
            }
            3 | 4 => {
                cursor.take(4)?;
                Entry::Other
            }
            5 | 6 => {
                cursor.take(8)?;
                // Long and Double take two slots.
                pool.push(Entry::Other);
                Entry::Other
            }
            7 => Entry::Class(cursor.u16()?),
            8 => Entry::Str(cursor.u16()?),
            9..=11 => Entry::Member(cursor.u16()?, cursor.u16()?),
            12 => {
                let name = cursor.u16()?;
                cursor.u16()?;
                Entry::NameAndType(name)
            }
            15 => {
                cursor.take(3)?;
                Entry::Other
            }
            16 | 19 | 20 => {
                cursor.take(2)?;
                Entry::Other
            }
            17 | 18 => {
                cursor.take(4)?;
                Entry::Other
            }
            _ => return None,
        };
        pool.push(entry);
    }
    cursor.u16()?; // access flags
    let this_index = cursor.u16()?;

    let utf8 = |index: u16| match pool.get(index as usize) {
        // Class names are ASCII in practice; anything else is shown lossily, never trusted.
        Some(Entry::Utf8(bytes)) if bytes.len() <= 512 => Some(String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    };
    let class_name = |index: u16| match pool.get(index as usize) {
        Some(Entry::Class(name)) => utf8(*name),
        _ => None,
    };
    let mut names = BTreeSet::new();
    let mut strings = Vec::new();
    for entry in &pool {
        match entry {
            Entry::Class(name) => {
                if let Some(name) = utf8(*name) {
                    names.insert(name);
                }
            }
            Entry::Member(class, name_and_type) => {
                let member = match pool.get(*name_and_type as usize) {
                    Some(Entry::NameAndType(name)) => utf8(*name),
                    _ => None,
                };
                if let (Some(class), Some(member)) = (class_name(*class), member) {
                    names.insert(format!("{class}.{member}"));
                }
            }
            Entry::Str(text) => {
                if let Some(text) = utf8(*text) {
                    strings.push(text);
                }
            }
            _ => {}
        }
    }
    Some(ClassRefs { this_class: class_name(this_index)?, names, strings })
}

fn printable(text: &str) -> String {
    util::sanitize_line(text.as_bytes(), 120)
}

fn scan_class(report: &mut JarReport, data: &[u8], entry_name: &str) {
    let Some(refs) = class_refs(data) else {
        report.unreadable.push(format!("{} (not a readable class file)", printable(entry_name)));
        return;
    };
    report.classes += 1;
    let class = printable(&refs.this_class.replace('/', "."));
    for (capability, _, patterns) in RULES {
        // A class is not evidence against itself (a library defining `okhttp3/…`).
        let hit = refs.names.iter().any(|name| {
            patterns.iter().any(|pattern| name.starts_with(pattern)) && !name.starts_with(&refs.this_class)
        });
        if hit {
            report.note(capability, &class);
        }
    }
    for (capability, needles) in BY_NAME {
        if refs.strings.iter().any(|text| needles.iter().any(|needle| text.contains(needle))) {
            report.note(&format!("{capability} (named in a string)"), &class);
        }
    }
}

fn mod_id(json: &[u8], quilt: bool) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let id = if quilt { value.get("quilt_loader")?.get("id")? } else { value.get("id")? }.as_str()?;
    let ok = (1..=64).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'));
    ok.then(|| id.to_string())
}

fn scan_archive<R: Read + Seek>(
    reader: R,
    prefix: &str,
    depth: usize,
    budget: &mut u64,
    report: &mut JarReport,
) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| Error::Format(format!("{prefix}not a valid jar (zip): {e}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(Error::Format(format!("{prefix}{} entries: not a mod jar", archive.len())));
    }
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| Error::Format(format!("{prefix}entry {index}: {e}")))?;
        if entry.is_dir() || entry.is_symlink() {
            continue;
        }
        let name = entry.name().map_err(|e| Error::Format(format!("{prefix}entry {index}: {e}")))?.into_owned();
        let shown = format!("{prefix}{}", printable(&name));
        let lower = name.to_ascii_lowercase();
        let is_class = lower.ends_with(".class");
        let is_jar = lower.ends_with(".jar");
        let is_meta = depth == 0 && (name == "fabric.mod.json" || name == "quilt.mod.json");
        if [".so", ".dll", ".dylib", ".jnilib"].iter().any(|ext| lower.ends_with(ext)) || lower.contains(".so.") {
            report.natives.push(shown);
            continue;
        }
        if !(is_class || is_jar || is_meta) {
            continue;
        }
        let limit = if is_jar { MAX_NESTED_JAR_BYTES } else { MAX_CLASS_BYTES };
        let mut data = Vec::new();
        let read = (&mut entry).take(limit + 1).read_to_end(&mut data);
        if read.is_err() || data.len() as u64 > limit || data.len() as u64 > *budget {
            report.unreadable.push(format!("{shown} (unreadable or over the size limit)"));
            continue;
        }
        *budget -= data.len() as u64;
        if is_class {
            scan_class(report, &data, &name);
        } else if is_meta {
            if report.mod_id.is_none() {
                report.mod_id = mod_id(&data, name == "quilt.mod.json");
            }
        } else if depth + 1 >= MAX_DEPTH {
            report.unreadable.push(format!("{shown} (jars nested too deeply)"));
        } else {
            report.nested.push(shown.clone());
            if let Err(e) = scan_archive(std::io::Cursor::new(data), &format!("{shown}!/"), depth + 1, budget, report) {
                report.unreadable.push(e.to_string());
            }
        }
    }
    Ok(())
}

/// Scan one jar.
pub fn scan_jar(path: &Path) -> Result<JarReport> {
    let mut file = util::open_regular_nofollow(path).ctx(|| format!("opening {}", path.display()))?;
    file.seek(SeekFrom::Start(0)).ctx(|| "rewinding the jar")?;
    let mut report = JarReport::default();
    let mut budget = MAX_TOTAL_BYTES;
    scan_archive(file, "", 0, &mut budget, &mut report)?;
    report.natives.sort();
    Ok(report)
}

/// identity → report, for every `*.jar` in `mods_dir`.
pub fn scan_mods(mods_dir: &Path) -> Result<BTreeMap<String, (String, JarReport)>> {
    let mut out = BTreeMap::new();
    let entries = match std::fs::read_dir(mods_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(Error::io(format!("reading {}", mods_dir.display()), e)),
    };
    let mut names: Vec<String> =
        entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with(".jar")).collect();
    names.sort();
    for name in names {
        crate::paths::validate_file_name(&name)?;
        let report = scan_jar(&mods_dir.join(&name))?;
        out.insert(report.identity(&name), (name, report));
    }
    Ok(out)
}

/// What was recorded at the previous lock.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recorded {
    /// identity → (capabilities, natives)
    pub mods: BTreeMap<String, RecordedMod>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedMod {
    pub file: String,
    pub capabilities: BTreeSet<String>,
    pub natives: BTreeSet<String>,
}

impl Recorded {
    pub fn load(instance_dir: &Path) -> Recorded {
        util::read_small_file(&instance_dir.join(SIDE_FILE), 4 << 20)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn of(scanned: &BTreeMap<String, (String, JarReport)>) -> Recorded {
        Recorded {
            mods: scanned
                .iter()
                .map(|(identity, (file, report))| {
                    (
                        identity.clone(),
                        RecordedMod {
                            file: file.clone(),
                            capabilities: report.capabilities.keys().cloned().collect(),
                            natives: report.natives.iter().cloned().collect(),
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn save(&self, instance_dir: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| Error::Other(format!("encoding capabilities: {e}")))?;
        util::write_atomic(&instance_dir.join(SIDE_FILE), &json, 0o600)
    }
}

/// Lines describing what is new compared with `before`: new mods, and for known mods the
/// capabilities and native libraries they did not have. Empty when nothing was gained.
pub fn changes(before: &Recorded, scanned: &BTreeMap<String, (String, JarReport)>) -> Vec<String> {
    let mut lines = Vec::new();
    for (identity, (file, report)) in scanned {
        let examples = |capability: &str| {
            let classes = report.capabilities.get(capability).cloned().unwrap_or_default();
            let total = report.counts.get(capability).copied().unwrap_or(classes.len());
            let more = if total > classes.len() { format!(", … {total} classes") } else { String::new() };
            format!("{}{more}", classes.join(", "))
        };
        match before.mods.get(identity) {
            None if before.mods.is_empty() => {}
            None => {
                let has: Vec<&str> = report.capabilities.keys().map(String::as_str).collect();
                lines.push(format!(
                    "new mod {identity} ({file}): {}",
                    if has.is_empty() { "nothing on the watch list".to_string() } else { has.join("; ") }
                ));
            }
            Some(old) => {
                for capability in report.capabilities.keys().filter(|c| !old.capabilities.contains(*c)) {
                    lines.push(format!(
                        "{identity}: NEW since the last lock ({} → {file}): {capability} — {}",
                        old.file,
                        examples(capability)
                    ));
                }
                for native in report.natives.iter().filter(|n| !old.natives.contains(*n)) {
                    lines.push(format!("{identity}: NEW bundled native library: {native}"));
                }
            }
        }
    }
    lines
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A class file whose constant pool refers to the given classes, members and strings.
    pub(crate) fn class_file(this: &str, classes: &[&str], members: &[(&str, &str)], strings: &[&str]) -> Vec<u8> {
        let mut pool: Vec<Vec<u8>> = Vec::new();
        let utf8 = |pool: &mut Vec<Vec<u8>>, text: &str| -> u16 {
            let mut e = vec![1];
            e.extend_from_slice(&(text.len() as u16).to_be_bytes());
            e.extend_from_slice(text.as_bytes());
            pool.push(e);
            pool.len() as u16
        };
        let class = |pool: &mut Vec<Vec<u8>>, name_index: u16| -> u16 {
            let mut e = vec![7];
            e.extend_from_slice(&name_index.to_be_bytes());
            pool.push(e);
            pool.len() as u16
        };
        let this_name = utf8(&mut pool, this);
        let this_index = class(&mut pool, this_name);
        // A long constant in between: it takes two slots, which an off-by-one would miss.
        pool.push(vec![5, 0, 0, 0, 0, 0, 0, 0, 1]);
        pool.push(Vec::new()); // the phantom second slot
        for name in classes {
            let n = utf8(&mut pool, name);
            class(&mut pool, n);
        }
        for (owner, member) in members {
            let n = utf8(&mut pool, owner);
            let c = class(&mut pool, n);
            let m = utf8(&mut pool, member);
            let d = utf8(&mut pool, "()V");
            let mut nat = vec![12];
            nat.extend_from_slice(&m.to_be_bytes());
            nat.extend_from_slice(&d.to_be_bytes());
            pool.push(nat);
            let nat_index = pool.len() as u16;
            let mut method = vec![10];
            method.extend_from_slice(&c.to_be_bytes());
            method.extend_from_slice(&nat_index.to_be_bytes());
            pool.push(method);
        }
        for text in strings {
            let t = utf8(&mut pool, text);
            let mut e = vec![8];
            e.extend_from_slice(&t.to_be_bytes());
            pool.push(e);
        }
        let mut out = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52];
        out.extend_from_slice(&((pool.len() + 1) as u16).to_be_bytes());
        for entry in pool {
            out.extend(entry);
        }
        out.extend_from_slice(&[0, 0x21]); // access flags
        out.extend_from_slice(&this_index.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // super, interfaces, fields, methods, attributes
        out
    }

    pub(crate) fn jar(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn scan(bytes: &[u8]) -> JarReport {
        let mut report = JarReport::default();
        let mut budget = MAX_TOTAL_BYTES;
        scan_archive(std::io::Cursor::new(bytes.to_vec()), "", 0, &mut budget, &mut report).unwrap();
        report
    }

    #[test]
    fn capabilities_are_read_from_class_references() {
        let quiet =
            class_file("mod/Renderer", &["java/util/ArrayList", "net/minecraft/client/Minecraft"], &[], &["hello"]);
        let loud = class_file(
            "mod/Updater",
            &["java/net/URLClassLoader"],
            &[("java/lang/Runtime", "exec"), ("java/net/URL", "openConnection"), ("java/lang/System", "loadLibrary")],
            &[],
        );
        let sneaky =
            class_file("mod/Sneaky", &[], &[("java/lang/Class", "forName")], &["sun.misc.Unsafe", "theUnsafe"]);
        let bytes = jar(&[
            ("fabric.mod.json", br#"{"schemaVersion":1,"id":"example-mod","version":"1.0"}"#.to_vec()),
            ("mod/Renderer.class", quiet),
            ("mod/Updater.class", loud),
            ("mod/Sneaky.class", sneaky),
            ("assets/example/icon.png", vec![1, 2, 3]),
        ]);
        let report = scan(&bytes);
        assert_eq!(report.mod_id.as_deref(), Some("example-mod"));
        assert_eq!(report.classes, 3);
        let has = |capability: &str| report.capabilities.get(capability).cloned().unwrap_or_default();
        assert_eq!(has("runs programs"), ["mod.Updater"]);
        assert_eq!(has("network"), ["mod.Updater"]);
        assert_eq!(has("native code"), ["mod.Updater"]);
        assert_eq!(has("defines classes at run time"), ["mod.Updater"]);
        assert_eq!(has("reflection"), ["mod.Sneaky"]);
        assert_eq!(has("raw memory (named in a string)"), ["mod.Sneaky"]);
        assert!(has("raw memory").is_empty(), "no direct reference to Unsafe");
        assert!(has("deserialisation").is_empty());
        assert!(report.unreadable.is_empty() && report.natives.is_empty());
        assert!(!describe("network").is_empty());
    }

    #[test]
    fn nested_jars_natives_and_damage_are_reported() {
        let inner = jar(&[
            ("lib/Net.class", class_file("lib/Net", &["java/net/Socket"], &[], &[])),
            ("natives/libhelper.so", vec![0x7f, b'E', b'L', b'F']),
        ]);
        let bytes = jar(&[
            ("META-INF/jars/helper-1.2.jar", inner),
            ("mod/Main.class", class_file("mod/Main", &[], &[], &[])),
            ("mod/Broken.class", vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52, 0xff, 0xff, 1]),
            ("mod/NotAClass.class", b"hello".to_vec()),
            ("windows/helper.dll", vec![b'M', b'Z']),
        ]);
        let report = scan(&bytes);
        assert_eq!(report.classes, 2);
        assert_eq!(report.capabilities["network"], ["lib.Net"]);
        assert_eq!(report.nested, ["META-INF/jars/helper-1.2.jar"]);
        assert_eq!(report.natives, ["META-INF/jars/helper-1.2.jar!/natives/libhelper.so", "windows/helper.dll"]);
        assert_eq!(report.unreadable.len(), 2, "{:?}", report.unreadable);
        assert!(scan_jar(Path::new("/nonexistent.jar")).is_err());
    }

    #[test]
    fn a_library_is_not_evidence_against_itself_and_names_are_cleaned() {
        // The okhttp3 classes themselves define okhttp3/…; only their users "use the network".
        let library = class_file("okhttp3/OkHttpClient", &["okhttp3/OkHttpClient$Builder"], &[], &[]);
        let hostile_name = class_file("mod/\u{1b}[31mEvil", &["java/lang/ProcessBuilder"], &[], &[]);
        let report = scan(&jar(&[("okhttp3/OkHttpClient.class", library), ("mod/Evil.class", hostile_name)]));
        assert!(!report.capabilities.contains_key("network"));
        let class = &report.capabilities["runs programs"][0];
        assert!(!class.contains('\u{1b}'), "{class:?}");
    }

    #[test]
    fn an_update_that_gains_capabilities_is_pointed_out() {
        let scan_named = |file: &str, report: JarReport| {
            let mut map = BTreeMap::new();
            map.insert(report.identity(file), (file.to_string(), report));
            map
        };
        let v1 = scan(&jar(&[
            ("fabric.mod.json", br#"{"id":"shiny"}"#.to_vec()),
            ("a/A.class", class_file("a/A", &[], &[("java/lang/Class", "forName")], &[])),
        ]));
        let v2 = scan(&jar(&[
            ("fabric.mod.json", br#"{"id":"shiny"}"#.to_vec()),
            ("a/A.class", class_file("a/A", &[], &[("java/lang/Class", "forName")], &[])),
            ("a/B.class", class_file("a/B", &["java/net/Socket", "java/lang/ProcessBuilder"], &[], &[])),
            ("a/libx.so", vec![1]),
        ]));
        let before = Recorded::of(&scan_named("shiny-1.0.jar", v1.clone()));
        assert!(changes(&before, &scan_named("shiny-1.0.jar", v1)).is_empty(), "nothing new, nothing said");
        let lines = changes(&before, &scan_named("shiny-1.1.jar", v2.clone()));
        let text = lines.join("\n");
        assert!(
            text.contains("shiny: NEW since the last lock (shiny-1.0.jar → shiny-1.1.jar): network — a.B"),
            "{text}"
        );
        assert!(text.contains("runs programs") && text.contains("NEW bundled native library: a/libx.so"), "{text}");
        assert!(!text.contains("reflection"), "it had that before: {text}");
        // A mod that was not there before is introduced with what it has.
        let mut both = scan_named("shiny-1.1.jar", v2);
        both.insert("other".into(), ("other-2.jar".into(), JarReport::default()));
        let lines = changes(&before, &both);
        assert!(
            lines.iter().any(|l| l.starts_with("new mod other (other-2.jar): nothing on the watch list")),
            "{lines:?}"
        );
        // The very first lock has nothing to compare with.
        assert!(changes(&Recorded::default(), &both).is_empty());
        // Identity without metadata: the file name up to its version.
        assert_eq!(JarReport::default().identity("sodium-fabric-0.6.13+mc1.21.4.jar"), "sodium-fabric");
        assert_eq!(JarReport::default().identity("Iris_1.8.jar"), "iris");
        assert_eq!(JarReport::default().identity("plain.jar"), "plain");
    }
}
