//! Files the launcher carries between an instance and the game's scratch root.
//!
//! The game cannot write into the instance directory except in four sub-directories. A few
//! settings files that live next to them (`options.txt`, `servers.dat`, …) are instead
//! copied into the scratch root before a session and copied back afterwards — by the
//! launcher, on the host, with the user's privileges. Copying the bytes would make the
//! launcher a courier for whatever a compromised game chose to leave there: a 2 GiB
//! "options" file, an NBT document nested ten thousand levels deep, terminal escape
//! sequences waiting for the next `cat`. Whatever reads those files next on the host —
//! another launcher, a backup tool, a future version of this one — would be parsing
//! attacker-controlled input.
//!
//! So nothing is copied. Each file is **parsed** against a strict, bounded grammar for its
//! kind and **written out again from the parsed form**; what does not parse is not carried.
//! The output is always small, well-formed and canonical, whatever the input was.
//!
//! | name | kind | what survives |
//! |---|---|---|
//! | `servers.dat` | server list (NBT) | per server: name, address, icon (base64), three flags — nothing else |
//! | `options*.txt` | `key:value` lines | well-formed keys, printable values, first occurrence of each key |
//! | other `*.txt` | text lines | printable UTF-8 lines |
//! | other `*.dat`, `*.nbt` | NBT | the same tree, re-encoded; depth, size and node count bounded |
//! | `*.json` | JSON | the same value, re-encoded |
//!
//! The same pass runs in both directions: a file damaged on the host does not reach the
//! game either.

use std::{collections::BTreeSet, path::Path};

use crate::{
    error::{Error, Result},
    util,
};

/// Largest file that is even read.
pub const MAX_INPUT_BYTES: u64 = 8 << 20;

const MAX_TEXT_LINES: usize = 20_000;
const MAX_LINE_BYTES: usize = 16 * 1024;
const MAX_OPTION_KEY: usize = 128;
const MAX_SERVERS: usize = 1024;
const MAX_SERVER_NAME_CHARS: usize = 256;
const MAX_SERVER_ADDRESS_CHARS: usize = 255;
const MAX_ICON_BYTES: usize = 256 * 1024;
const MAX_NBT_DEPTH: usize = 32;
const MAX_NBT_NODES: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Servers,
    Options,
    Text,
    Nbt,
    Json,
}

/// Which grammar a persisted file name is held to; `None` if there is none for it (such a
/// name is rejected in the configuration: there is no "copy as is").
pub fn kind_of(name: &str) -> Option<Kind> {
    let lower = name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, e)| e)?;
    Some(match extension {
        "dat" if lower == "servers.dat" => Kind::Servers,
        "dat" | "nbt" => Kind::Nbt,
        "txt" if lower.starts_with("options") => Kind::Options,
        "txt" => Kind::Text,
        "json" => Kind::Json,
        _ => return None,
    })
}

/// What a pass kept and what it left behind, for the log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub kept: usize,
    pub dropped: usize,
}

impl Report {
    pub fn describe(&self, name: &str) -> Option<String> {
        (self.dropped > 0).then(|| {
            format!(
                "{name}: {} item(s) did not fit the expected format and were left out ({} kept)",
                self.dropped, self.kept
            )
        })
    }
}

/// Parse `input` as `kind` and return its canonical re-encoding.
pub fn sanitize(kind: Kind, input: &[u8]) -> Result<(Vec<u8>, Report)> {
    if input.len() as u64 > MAX_INPUT_BYTES {
        return Err(Error::Format("file is larger than a settings file can be".into()));
    }
    match kind {
        Kind::Options => Ok(options(input)),
        Kind::Text => Ok(text(input)),
        Kind::Servers => servers(input),
        Kind::Nbt => nbt::canonical(input).map(|out| (out, Report { kept: 1, dropped: 0 })),
        Kind::Json => json(input),
    }
}

/// Carry `name` from `src_dir` to `dst_dir` through its sanitiser. Returns `None` when there
/// is no such file. Symlinks and special files are refused at both ends.
pub fn carry(src_dir: &Path, dst_dir: &Path, name: &str) -> Result<Option<Report>> {
    let kind = kind_of(name).ok_or_else(|| Error::Policy(format!("{name}: no format is known for this file")))?;
    let src = src_dir.join(name);
    match std::fs::symlink_metadata(&src) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(format!("inspecting {}", src.display()), e)),
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(Error::Policy(format!("{} is not a regular file; not carrying it", src.display())));
        }
        Ok(_) => {}
    }
    let input = util::read_small_file(&src, MAX_INPUT_BYTES)?;
    let (output, report) = sanitize(kind, &input).map_err(|e| Error::Format(format!("{name}: {e}")))?;
    util::write_atomic(&dst_dir.join(name), &output, 0o600)?;
    Ok(Some(report))
}

// ---------------------------------------------------------------- text

/// Characters that may appear in a carried line: anything printable. Control characters
/// (including ESC, the C1 range and the line/paragraph separators) and the bidirectional
/// overrides that make text render as something else are not.
fn printable(c: char) -> bool {
    !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn clean_lines(input: &[u8]) -> impl Iterator<Item = Option<&str>> {
    input.split(|b| *b == b'\n').take(MAX_TEXT_LINES + 1).map(|raw| {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let line = std::str::from_utf8(raw).ok()?;
        (line.len() <= MAX_LINE_BYTES && line.chars().all(printable)).then_some(line)
    })
}

fn text(input: &[u8]) -> (Vec<u8>, Report) {
    let mut out = Vec::with_capacity(input.len().min(1 << 20));
    let mut report = Report::default();
    for (index, line) in clean_lines(input).enumerate() {
        match line {
            Some(line) if index < MAX_TEXT_LINES => {
                out.extend_from_slice(line.as_bytes());
                out.push(b'\n');
                report.kept += 1;
            }
            _ => report.dropped += 1,
        }
    }
    // A trailing newline in the input is not a line of its own.
    if input.is_empty() || input.ends_with(b"\n") {
        out.truncate(out.len().saturating_sub(1));
        report.kept = report.kept.saturating_sub(1);
    }
    (out, report)
}

fn options(input: &[u8]) -> (Vec<u8>, Report) {
    let mut out = Vec::with_capacity(input.len().min(1 << 20));
    let mut report = Report::default();
    let mut seen = BTreeSet::new();
    for (index, line) in clean_lines(input).enumerate() {
        let Some(line) = line else {
            report.dropped += 1;
            continue;
        };
        if line.is_empty() {
            continue;
        }
        let entry = line.split_once(':').filter(|(key, _)| {
            (1..=MAX_OPTION_KEY).contains(&key.len())
                && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        });
        match entry {
            Some((key, value)) if index < MAX_TEXT_LINES && seen.insert(key) => {
                out.extend_from_slice(key.as_bytes());
                out.push(b':');
                out.extend_from_slice(value.as_bytes());
                out.push(b'\n');
                report.kept += 1;
            }
            _ => report.dropped += 1,
        }
    }
    (out, report)
}

fn json(input: &[u8]) -> Result<(Vec<u8>, Report)> {
    // serde_json bounds nesting itself (128 levels) and rejects trailing garbage.
    let value: serde_json::Value =
        serde_json::from_slice(input).map_err(|e| Error::Format(format!("not valid JSON ({e})")))?;
    let out = serde_json::to_vec(&value).map_err(|e| Error::Format(format!("cannot re-encode JSON ({e})")))?;
    Ok((out, Report { kept: 1, dropped: 0 }))
}

// ---------------------------------------------------------------- servers.dat

/// One multiplayer-list entry: exactly the fields the game itself stores.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Server {
    name: String,
    address: String,
    icon: Option<String>,
    accept_textures: Option<bool>,
    hidden: bool,
    prevents_chat_reports: Option<bool>,
}

fn clean_text(value: &str, max_chars: usize) -> Option<String> {
    (value.chars().count() <= max_chars && value.chars().all(printable)).then(|| value.to_string())
}

fn server_from(fields: &[(String, nbt::Tag)]) -> Option<Server> {
    let string = |key: &str| fields.iter().find(|(k, _)| k == key).and_then(|(_, t)| t.as_str());
    let flag = |key: &str| fields.iter().find(|(k, _)| k == key).and_then(|(_, t)| t.as_byte()).map(|b| b != 0);
    let address = clean_text(string("ip")?, MAX_SERVER_ADDRESS_CHARS).filter(|a| !a.is_empty())?;
    Some(Server {
        name: string("name").and_then(|n| clean_text(n, MAX_SERVER_NAME_CHARS)).unwrap_or_default(),
        address,
        // A PNG in base64. It is never decoded here; only its alphabet and size are held.
        icon: string("icon")
            .filter(|icon| {
                !icon.is_empty()
                    && icon.len() <= MAX_ICON_BYTES
                    && icon.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
            })
            .map(str::to_string),
        accept_textures: flag("acceptTextures"),
        hidden: flag("hidden").unwrap_or(false),
        prevents_chat_reports: flag("preventsChatReports"),
    })
}

fn servers(input: &[u8]) -> Result<(Vec<u8>, Report)> {
    let root = nbt::parse(input)?;
    let mut report = Report::default();
    let mut list = Vec::new();
    if let Some(nbt::Tag::List(_, entries)) = root.get("servers") {
        for entry in entries {
            match entry.as_compound().and_then(server_from) {
                Some(server) if list.len() < MAX_SERVERS => list.push(server),
                _ => report.dropped += 1,
            }
        }
    }
    report.kept = list.len();

    let entries: Vec<nbt::Tag> = list
        .into_iter()
        .map(|s| {
            let mut fields = Vec::new();
            if let Some(icon) = s.icon {
                fields.push(("icon".to_string(), nbt::Tag::String(icon)));
            }
            fields.push(("ip".to_string(), nbt::Tag::String(s.address)));
            fields.push(("name".to_string(), nbt::Tag::String(s.name)));
            if let Some(accept) = s.accept_textures {
                fields.push(("acceptTextures".to_string(), nbt::Tag::Byte(accept as i8)));
            }
            if s.hidden {
                fields.push(("hidden".to_string(), nbt::Tag::Byte(1)));
            }
            if let Some(prevents) = s.prevents_chat_reports {
                fields.push(("preventsChatReports".to_string(), nbt::Tag::Byte(prevents as i8)));
            }
            nbt::Tag::Compound(fields)
        })
        .collect();
    let element = if entries.is_empty() { nbt::TAG_END } else { nbt::TAG_COMPOUND };
    let root = nbt::Tag::Compound(vec![("servers".to_string(), nbt::Tag::List(element, entries))]);
    Ok((nbt::write(&root), report))
}

// ---------------------------------------------------------------- NBT

/// Uncompressed, big-endian NBT ("Named Binary Tag"), as Java Edition writes `servers.dat`
/// and `hotbar.nbt`. The parser allocates in proportion to the input it has actually seen,
/// refuses to recurse past [`MAX_NBT_DEPTH`], and counts every node.
pub mod nbt {
    use super::{Error, MAX_NBT_DEPTH, MAX_NBT_NODES, Result};

    pub const TAG_END: u8 = 0;
    pub const TAG_COMPOUND: u8 = 10;

    #[derive(Debug, Clone, PartialEq)]
    pub enum Tag {
        Byte(i8),
        Short(i16),
        Int(i32),
        Long(i64),
        Float(f32),
        Double(f64),
        ByteArray(Vec<u8>),
        String(String),
        /// Element type and elements (all of that type).
        List(u8, Vec<Tag>),
        /// Fields in file order.
        Compound(Vec<(String, Tag)>),
        IntArray(Vec<i32>),
        LongArray(Vec<i64>),
    }

    impl Tag {
        pub fn id(&self) -> u8 {
            match self {
                Tag::Byte(_) => 1,
                Tag::Short(_) => 2,
                Tag::Int(_) => 3,
                Tag::Long(_) => 4,
                Tag::Float(_) => 5,
                Tag::Double(_) => 6,
                Tag::ByteArray(_) => 7,
                Tag::String(_) => 8,
                Tag::List(..) => 9,
                Tag::Compound(_) => TAG_COMPOUND,
                Tag::IntArray(_) => 11,
                Tag::LongArray(_) => 12,
            }
        }
        pub fn as_str(&self) -> Option<&str> {
            if let Tag::String(s) = self { Some(s) } else { None }
        }
        pub fn as_byte(&self) -> Option<i8> {
            if let Tag::Byte(b) = self { Some(*b) } else { None }
        }
        pub fn as_compound(&self) -> Option<&[(String, Tag)]> {
            if let Tag::Compound(fields) = self { Some(fields) } else { None }
        }
        /// First field of that name, if this is a compound.
        pub fn get(&self, name: &str) -> Option<&Tag> {
            self.as_compound()?.iter().find(|(key, _)| key == name).map(|(_, tag)| tag)
        }
    }

    fn bad(what: &str) -> Error {
        Error::Format(format!("malformed NBT: {what}"))
    }

    struct Reader<'a> {
        data: &'a [u8],
        at: usize,
        nodes: usize,
    }

    impl<'a> Reader<'a> {
        fn take(&mut self, len: usize) -> Result<&'a [u8]> {
            let end = self.at.checked_add(len).filter(|end| *end <= self.data.len()).ok_or_else(|| bad("truncated"))?;
            let slice = &self.data[self.at..end];
            self.at = end;
            Ok(slice)
        }
        fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
            Ok(self.take(N)?.try_into().expect("length checked"))
        }
        fn u8(&mut self) -> Result<u8> {
            Ok(self.array::<1>()?[0])
        }
        fn i32(&mut self) -> Result<i32> {
            Ok(i32::from_be_bytes(self.array()?))
        }
        /// A length prefix for `width`-byte elements. It is checked against the bytes that
        /// are actually left, so a forged count can never reserve memory.
        fn count(&mut self, width: usize) -> Result<usize> {
            let n = usize::try_from(self.i32()?).map_err(|_| bad("negative length"))?;
            let need = n.checked_mul(width).ok_or_else(|| bad("length overflow"))?;
            if need > self.data.len() - self.at {
                return Err(bad("length exceeds the file"));
            }
            Ok(n)
        }
        fn string(&mut self) -> Result<String> {
            let len = u16::from_be_bytes(self.array()?) as usize;
            decode_mutf8(self.take(len)?)
        }
        fn node(&mut self) -> Result<()> {
            self.nodes += 1;
            if self.nodes > MAX_NBT_NODES { Err(bad("too many tags")) } else { Ok(()) }
        }

        fn payload(&mut self, id: u8, depth: usize) -> Result<Tag> {
            self.node()?;
            if depth > MAX_NBT_DEPTH {
                return Err(bad("nested too deeply"));
            }
            Ok(match id {
                1 => Tag::Byte(self.u8()? as i8),
                2 => Tag::Short(i16::from_be_bytes(self.array()?)),
                3 => Tag::Int(self.i32()?),
                4 => Tag::Long(i64::from_be_bytes(self.array()?)),
                5 => Tag::Float(f32::from_be_bytes(self.array()?)),
                6 => Tag::Double(f64::from_be_bytes(self.array()?)),
                7 => {
                    let n = self.count(1)?;
                    Tag::ByteArray(self.take(n)?.to_vec())
                }
                8 => Tag::String(self.string()?),
                9 => {
                    let element = self.u8()?;
                    if element > 12 {
                        return Err(bad("unknown list element type"));
                    }
                    // Every element except "end" occupies at least one byte, so the count is
                    // bounded by the input; a list *of* "end" tags must be empty.
                    let n = self.count(if element == TAG_END { 0 } else { 1 })?;
                    if element == TAG_END && n != 0 {
                        return Err(bad("non-empty list of end tags"));
                    }
                    let mut items = Vec::with_capacity(n.min(1024));
                    for _ in 0..n {
                        items.push(self.payload(element, depth + 1)?);
                    }
                    Tag::List(element, items)
                }
                TAG_COMPOUND => {
                    let mut fields = Vec::new();
                    loop {
                        let id = self.u8()?;
                        if id == TAG_END {
                            break;
                        }
                        let name = self.string()?;
                        fields.push((name, self.payload(id, depth + 1)?));
                    }
                    Tag::Compound(fields)
                }
                11 => {
                    let n = self.count(4)?;
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        items.push(self.i32()?);
                    }
                    Tag::IntArray(items)
                }
                12 => {
                    let n = self.count(8)?;
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        items.push(i64::from_be_bytes(self.array()?));
                    }
                    Tag::LongArray(items)
                }
                _ => return Err(bad("unknown tag type")),
            })
        }
    }

    /// Parse a whole file: one named compound, nothing after it. The root's name is ignored
    /// (the game writes an empty one).
    pub fn parse(data: &[u8]) -> Result<Tag> {
        if data.starts_with(&[0x1f, 0x8b]) {
            return Err(bad("compressed NBT is not accepted here"));
        }
        let mut reader = Reader { data, at: 0, nodes: 0 };
        if reader.u8()? != TAG_COMPOUND {
            return Err(bad("the root is not a compound"));
        }
        let _name = reader.string()?;
        let root = reader.payload(TAG_COMPOUND, 0)?;
        if reader.at != data.len() {
            return Err(bad("data after the root tag"));
        }
        Ok(root)
    }

    fn put(out: &mut Vec<u8>, tag: &Tag) {
        match tag {
            Tag::Byte(v) => out.push(*v as u8),
            Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::ByteArray(v) => {
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                out.extend_from_slice(v);
            }
            Tag::String(v) => put_string(out, v),
            Tag::List(element, items) => {
                out.push(*element);
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for item in items {
                    put(out, item);
                }
            }
            Tag::Compound(fields) => {
                for (name, value) in fields {
                    out.push(value.id());
                    put_string(out, name);
                    put(out, value);
                }
                out.push(TAG_END);
            }
            Tag::IntArray(v) => {
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                v.iter().for_each(|i| out.extend_from_slice(&i.to_be_bytes()));
            }
            Tag::LongArray(v) => {
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                v.iter().for_each(|i| out.extend_from_slice(&i.to_be_bytes()));
            }
        }
    }

    fn put_string(out: &mut Vec<u8>, value: &str) {
        let encoded = encode_mutf8(value);
        // Strings come from `decode_mutf8` (at most 65535 encoded bytes, and re-encoding is
        // length-preserving) or from this crate's own short literals, so the length always
        // fits; if that ever stops being true, stop rather than write a corrupt file.
        assert!(encoded.len() <= u16::MAX as usize, "NBT string longer than the format allows");
        out.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
        out.extend_from_slice(&encoded);
    }

    /// Encode a tree as a file: an unnamed root compound.
    pub fn write(root: &Tag) -> Vec<u8> {
        let mut out = vec![TAG_COMPOUND, 0, 0];
        put(&mut out, root);
        out
    }

    /// Parse and re-encode. Lists whose elements do not all have the declared type cannot be
    /// produced by `parse`, so the output is well-formed by construction.
    pub fn canonical(data: &[u8]) -> Result<Vec<u8>> {
        Ok(write(&parse(data)?))
    }

    /// Java's "modified UTF-8": NUL is `C0 80`, and characters outside the BMP are two
    /// three-byte surrogates. Anything else that standard UTF-8 would reject is rejected.
    pub fn decode_mutf8(bytes: &[u8]) -> Result<String> {
        let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let b0 = bytes[i];
            let continuation = |offset: usize| -> Result<u16> {
                match bytes.get(i + offset) {
                    Some(b) if b & 0xC0 == 0x80 => Ok(u16::from(b & 0x3F)),
                    _ => Err(bad("invalid string encoding")),
                }
            };
            match b0 {
                0x01..=0x7F => {
                    units.push(u16::from(b0));
                    i += 1;
                }
                0xC0..=0xDF => {
                    let unit = (u16::from(b0 & 0x1F) << 6) | continuation(1)?;
                    // Overlong forms are invalid, except the one Java uses for NUL.
                    if unit < 0x80 && unit != 0 {
                        return Err(bad("overlong string encoding"));
                    }
                    units.push(unit);
                    i += 2;
                }
                0xE0..=0xEF => {
                    let unit = (u16::from(b0 & 0x0F) << 12) | (continuation(1)? << 6) | continuation(2)?;
                    if unit < 0x800 {
                        return Err(bad("overlong string encoding"));
                    }
                    units.push(unit);
                    i += 3;
                }
                _ => return Err(bad("invalid string encoding")),
            }
        }
        // Unpaired surrogates have no place in text.
        String::from_utf16(&units).map_err(|_| bad("unpaired surrogate in string"))
    }

    pub fn encode_mutf8(value: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(value.len());
        for unit in value.encode_utf16() {
            match unit {
                0x0001..=0x007F => out.push(unit as u8),
                0x0000 | 0x0080..=0x07FF => {
                    out.push(0xC0 | (unit >> 6) as u8);
                    out.push(0x80 | (unit & 0x3F) as u8);
                }
                _ => {
                    out.push(0xE0 | (unit >> 12) as u8);
                    out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                    out.push(0x80 | (unit & 0x3F) as u8);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{nbt::Tag, *};

    fn named(id: u8, name: &str) -> Vec<u8> {
        let mut v = vec![id];
        v.extend_from_slice(&(name.len() as u16).to_be_bytes());
        v.extend_from_slice(name.as_bytes());
        v
    }

    fn string_field(name: &str, value: &str) -> Vec<u8> {
        let mut v = named(8, name);
        let encoded = nbt::encode_mutf8(value);
        v.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
        v.extend_from_slice(&encoded);
        v
    }

    fn byte_field(name: &str, value: u8) -> Vec<u8> {
        let mut v = named(1, name);
        v.push(value);
        v
    }

    /// A `servers.dat` the way the game writes it, from raw per-server field encodings.
    fn servers_file(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut v = named(10, "");
        v.extend(named(9, "servers"));
        v.push(if entries.is_empty() { 0 } else { 10 });
        v.extend_from_slice(&(entries.len() as i32).to_be_bytes());
        for entry in entries {
            v.extend_from_slice(entry);
            v.push(0);
        }
        v.push(0);
        v
    }

    fn parsed_servers(file: &[u8]) -> Vec<Vec<(String, Tag)>> {
        let root = nbt::parse(file).unwrap();
        let Some(Tag::List(_, entries)) = root.get("servers") else { panic!("no server list") };
        entries.iter().map(|e| e.as_compound().unwrap().to_vec()).collect()
    }

    #[test]
    fn file_names_map_to_grammars() {
        assert_eq!(kind_of("servers.dat"), Some(Kind::Servers));
        assert_eq!(kind_of("options.txt"), Some(Kind::Options));
        assert_eq!(kind_of("optionsof.txt"), Some(Kind::Options));
        assert_eq!(kind_of("command_history.txt"), Some(Kind::Text));
        assert_eq!(kind_of("hotbar.nbt"), Some(Kind::Nbt));
        assert_eq!(kind_of("usercache.json"), Some(Kind::Json));
        for none in ["servers", "launcher_profiles", "mod.jar", "run.sh", "options.txt.bak", ""] {
            assert_eq!(kind_of(none), None, "{none:?}");
        }
    }

    #[test]
    fn a_real_server_list_survives_with_exactly_its_fields() {
        let icon = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAAA";
        let file = servers_file(&[
            [
                string_field("name", "§aSurvival"),
                string_field("ip", "play.example.org:25570"),
                string_field("icon", icon),
                byte_field("acceptTextures", 1),
            ]
            .concat(),
            [string_field("ip", "203.0.113.7"), string_field("name", "LAN box"), byte_field("hidden", 1)].concat(),
        ]);
        let (out, report) = sanitize(Kind::Servers, &file).unwrap();
        assert_eq!(report, Report { kept: 2, dropped: 0 });
        let servers = parsed_servers(&out);
        assert_eq!(
            servers[0],
            vec![
                ("icon".to_string(), Tag::String(icon.into())),
                ("ip".to_string(), Tag::String("play.example.org:25570".into())),
                ("name".to_string(), Tag::String("§aSurvival".into())),
                ("acceptTextures".to_string(), Tag::Byte(1)),
            ]
        );
        assert_eq!(
            servers[1],
            vec![
                ("ip".to_string(), Tag::String("203.0.113.7".into())),
                ("name".to_string(), Tag::String("LAN box".into())),
                ("hidden".to_string(), Tag::Byte(1)),
            ]
        );
        // Sanitising is idempotent: the output is already canonical.
        assert_eq!(sanitize(Kind::Servers, &out).unwrap().0, out);
    }

    #[test]
    fn everything_the_schema_does_not_name_is_left_behind() {
        let mut smuggled = named(7, "payload"); // a byte array the game never writes here
        smuggled.extend_from_slice(&5i32.to_be_bytes());
        smuggled.extend_from_slice(b"\x7fELF\x02");
        let file = servers_file(&[
            [string_field("ip", "ok.example.org"), smuggled, string_field("x-extra", "1")].concat(),
            [string_field("name", "no address")].concat(), // no ip: dropped
            [string_field("ip", "esc\u{1b}[2J.example.org")].concat(), // control characters
            [string_field("ip", "a.example.org"), string_field("icon", "not base64!")].concat(), // icon dropped
            [string_field("ip", "b.example.org"), named(3, "acceptTextures"), 1i32.to_be_bytes().to_vec()].concat(), // wrong type
            [string_field("ip", "c.example.org"), string_field("name", "x\u{202e}gnp.exe")].concat(), // bidi override
        ]);
        let (out, report) = sanitize(Kind::Servers, &file).unwrap();
        assert_eq!(report, Report { kept: 4, dropped: 2 });
        let servers = parsed_servers(&out);
        let fields = |i: usize| servers[i].iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>();
        assert_eq!(fields(0), ["ip", "name"], "unknown fields and binary payloads are gone");
        assert_eq!(fields(1), ["ip", "name"], "an icon outside the base64 alphabet is dropped, the entry kept");
        assert_eq!(fields(2), ["ip", "name"], "a flag of the wrong type is dropped");
        assert_eq!(servers[3][1].1, Tag::String(String::new()), "a name with a bidi override is blanked");
        assert!(!out.windows(4).any(|w| w == b"\x7fELF"));
    }

    #[test]
    fn empty_and_missing_lists_become_an_empty_list() {
        let expected = sanitize(Kind::Servers, &servers_file(&[])).unwrap().0;
        assert_eq!(parsed_servers(&expected).len(), 0);
        // A root without any server list at all.
        let bare = [named(10, ""), vec![0]].concat();
        assert_eq!(sanitize(Kind::Servers, &bare).unwrap().0, expected);
        // `servers` of the wrong type.
        let wrong = [named(10, ""), string_field("servers", "nope"), vec![0]].concat();
        assert_eq!(sanitize(Kind::Servers, &wrong).unwrap().0, expected);
    }

    #[test]
    fn hostile_nbt_is_rejected_without_work_proportional_to_its_claims() {
        let reject = |data: &[u8], why: &str| {
            let err = sanitize(Kind::Nbt, data).unwrap_err().to_string();
            assert!(err.contains(why), "{err:?} should mention {why:?}");
        };
        reject(b"", "truncated");
        reject(&[8, 0, 0, 0, 0], "root is not a compound");
        reject(&[0x1f, 0x8b, 8, 0], "compressed");
        reject(&[named(10, ""), vec![0, 0xff]].concat(), "data after the root");
        reject(&[named(10, ""), named(13, "x")].concat(), "unknown tag type");
        // A list claiming two billion elements in a 20-byte file.
        reject(
            &[named(10, ""), named(9, "l"), vec![1], i32::MAX.to_be_bytes().to_vec(), vec![0]].concat(),
            "exceeds the file",
        );
        reject(&[named(10, ""), named(9, "l"), vec![1], (-1i32).to_be_bytes().to_vec()].concat(), "negative length");
        reject(&[named(10, ""), named(12, "a"), 0x2000_0000i32.to_be_bytes().to_vec()].concat(), "exceeds the file");
        // A list of "end" tags with a length: zero-size elements would make the count free.
        reject(&[named(10, ""), named(9, "l"), vec![0], 1000i32.to_be_bytes().to_vec(), vec![0]].concat(), "end tags");
        // Nesting: compounds inside compounds, far past the limit, in a tiny file.
        let mut deep = named(10, "");
        for _ in 0..500 {
            deep.extend(named(10, "d"));
        }
        deep.extend(std::iter::repeat_n(0u8, 501));
        reject(&deep, "nested too deeply");
        // Many tags.
        let mut wide = named(10, "");
        for _ in 0..(MAX_NBT_NODES + 10) {
            wide.extend(byte_field("b", 0));
        }
        wide.push(0);
        reject(&wide, "too many tags");
        // Strings: truncated, invalid, overlong, unpaired surrogate.
        reject(&[named(10, ""), vec![8, 0, 1, b'k', 0, 9, b'x']].concat(), "truncated");
        reject(&[named(10, ""), vec![8, 0, 1, b'k', 0, 1, 0xff], vec![0]].concat(), "string encoding");
        reject(&[named(10, ""), vec![8, 0, 1, b'k', 0, 2, 0xc1, 0x81], vec![0]].concat(), "overlong");
        reject(&[named(10, ""), vec![8, 0, 1, b'k', 0, 3, 0xed, 0xa0, 0x80], vec![0]].concat(), "surrogate");
        reject(&[named(10, ""), vec![8, 0, 1, b'k', 0, 1, 0x00], vec![0]].concat(), "string encoding");
    }

    #[test]
    fn generic_nbt_round_trips_every_tag_type() {
        let tree = Tag::Compound(vec![
            ("b".into(), Tag::Byte(-1)),
            ("s".into(), Tag::Short(-2)),
            ("i".into(), Tag::Int(i32::MIN)),
            ("l".into(), Tag::Long(i64::MAX)),
            ("f".into(), Tag::Float(1.5)),
            ("d".into(), Tag::Double(-0.25)),
            ("ba".into(), Tag::ByteArray(vec![0, 255, 7])),
            ("str".into(), Tag::String("naïve \u{0} 𝄞 §6".into())),
            ("list".into(), Tag::List(8, vec![Tag::String("a".into()), Tag::String("b".into())])),
            ("empty".into(), Tag::List(0, vec![])),
            ("nested".into(), Tag::Compound(vec![("ia".into(), Tag::IntArray(vec![1, -1]))])),
            ("la".into(), Tag::LongArray(vec![i64::MIN, 0])),
        ]);
        let bytes = nbt::write(&tree);
        assert_eq!(nbt::parse(&bytes).unwrap(), tree);
        assert_eq!(sanitize(Kind::Nbt, &bytes).unwrap().0, bytes, "canonical input is a fixed point");
    }

    #[test]
    fn modified_utf8_matches_java() {
        // DataOutputStream.writeUTF: NUL as C0 80, U+1D11E as a surrogate pair.
        assert_eq!(nbt::encode_mutf8("\u{0}"), [0xC0, 0x80]);
        assert_eq!(nbt::encode_mutf8("é"), [0xC3, 0xA9]);
        assert_eq!(nbt::encode_mutf8("€"), [0xE2, 0x82, 0xAC]);
        assert_eq!(nbt::encode_mutf8("𝄞"), [0xED, 0xA0, 0xB4, 0xED, 0xB4, 0x9E]);
        for text in ["", "plain", "\u{0}\u{7f}\u{80}\u{7ff}\u{800}\u{ffff}", "𝄞 and 😀", "§aGreen"] {
            assert_eq!(nbt::decode_mutf8(&nbt::encode_mutf8(text)).unwrap(), text);
        }
        // Standard four-byte UTF-8 is *not* modified UTF-8.
        assert!(nbt::decode_mutf8("𝄞".as_bytes()).is_err());
    }

    #[test]
    fn options_keep_well_formed_settings_and_nothing_else() {
        let input = "version:4189\r\nfov:0.5\nkey_key.attack:key.mouse.left\nresourcePacks:[\"vanilla\",\"file/x.zip\"]\n\
                     lastServer:\nlang:en_us\nfov:1.0\nno separator here\n:empty key\nbad key!:1\n\
                     esc:\u{1b}[31mred\nbidi:\u{202e}abc\n\nsoundCategory_master:0.0";
        let (out, report) = sanitize(Kind::Options, input.as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(out.clone()).unwrap(),
            "version:4189\nfov:0.5\nkey_key.attack:key.mouse.left\nresourcePacks:[\"vanilla\",\"file/x.zip\"]\n\
             lastServer:\nlang:en_us\nsoundCategory_master:0.0\n"
        );
        assert_eq!(report, Report { kept: 7, dropped: 6 }, "duplicate, malformed and unprintable lines are dropped");
        assert_eq!(sanitize(Kind::Options, &out).unwrap().0, out, "idempotent");

        let (out, report) = sanitize(Kind::Options, b"ok:1\n\xff\xfe:2\nalso:3\n").unwrap();
        assert_eq!(out, b"ok:1\nalso:3\n");
        assert_eq!(report.dropped, 1, "a line that is not UTF-8 is dropped, the rest kept");

        let long = format!("a:{}\nb:1\n", "x".repeat(MAX_LINE_BYTES));
        assert_eq!(sanitize(Kind::Options, long.as_bytes()).unwrap().0, b"b:1\n");
        let many: String = (0..MAX_TEXT_LINES + 50).map(|i| format!("k{i}:v\n")).collect();
        let (out, report) = sanitize(Kind::Options, many.as_bytes()).unwrap();
        assert_eq!(out.iter().filter(|b| **b == b'\n').count(), MAX_TEXT_LINES);
        assert!(report.dropped >= 1);
    }

    #[test]
    fn plain_text_keeps_printable_lines() {
        let (out, report) = sanitize(Kind::Text, b"/gamemode creative\n\n/say \x07bell\nlast").unwrap();
        assert_eq!(out, b"/gamemode creative\n\nlast\n");
        assert_eq!(report, Report { kept: 3, dropped: 1 });
        assert_eq!(sanitize(Kind::Text, b"").unwrap().0, b"");
        assert_eq!(sanitize(Kind::Text, b"one\n").unwrap().0, b"one\n");
    }

    #[test]
    fn json_is_reencoded_or_refused() {
        let (out, _) = sanitize(Kind::Json, b" { \"a\" : [1, 2.5, \"x\\u001b\"], \"b\": null }\n").unwrap();
        assert_eq!(out, br#"{"a":[1,2.5,"x\u001b"],"b":null}"#, "escapes stay escapes; whitespace goes");
        for bad in [&b"{\"a\":1} trailing"[..], b"{", b"", b"\xff", &"[".repeat(1000).into_bytes()] {
            assert!(sanitize(Kind::Json, bad).is_err());
        }
    }

    #[test]
    fn oversized_input_is_refused_before_parsing() {
        let big = vec![b'a'; MAX_INPUT_BYTES as usize + 1];
        for kind in [Kind::Options, Kind::Text, Kind::Servers, Kind::Nbt, Kind::Json] {
            assert!(sanitize(kind, &big).is_err(), "{kind:?}");
        }
    }

    #[test]
    fn carrying_goes_through_the_sanitiser_and_refuses_odd_files() {
        let root = std::env::temp_dir()
            .join(format!("bastion-persist-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let (src, dst) = (root.join("src"), root.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        assert_eq!(carry(&src, &dst, "options.txt").unwrap(), None, "a missing file is not an error");
        std::fs::write(src.join("options.txt"), b"fov:70\n\x1b]0;pwned\x07:1\n").unwrap();
        let report = carry(&src, &dst, "options.txt").unwrap().unwrap();
        assert_eq!(report, Report { kept: 1, dropped: 1 });
        assert_eq!(std::fs::read(dst.join("options.txt")).unwrap(), b"fov:70\n");
        assert!(report.describe("options.txt").unwrap().contains("1 item(s)"));

        // Malformed: nothing is written, an existing destination is left alone.
        std::fs::write(dst.join("servers.dat"), b"previous").unwrap();
        std::fs::write(src.join("servers.dat"), b"\x0a\x00\x00\x09").unwrap();
        assert!(carry(&src, &dst, "servers.dat").is_err());
        assert_eq!(std::fs::read(dst.join("servers.dat")).unwrap(), b"previous");

        // A symlink where the file should be (planted by the game) is never followed.
        std::fs::remove_file(src.join("servers.dat")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.join("servers.dat")).unwrap();
        assert!(carry(&src, &dst, "servers.dat").is_err());
        assert_eq!(std::fs::read(dst.join("servers.dat")).unwrap(), b"previous");

        assert!(carry(&src, &dst, "payload.bin").is_err(), "no grammar, no carrying");
        std::fs::remove_dir_all(root).unwrap();
    }
}
