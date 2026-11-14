//! What bastion reads from an imported version's `AndroidManifest.xml`: the package name, the
//! version code and the version name of the root `<manifest>` element — the same three
//! attributes (and `split`) that `mcpelauncher-client` and `mcpelauncher-ui` read from it
//! (`ApkInfo::fromXml` in mcpelauncher-apkinfo).
//!
//! The file is Android's compiled ("binary") XML: a chunk header, a string pool, a resource
//! map, then one chunk per XML node. The file comes from the user's download, so it is
//! untrusted input: every offset and length is checked against the chunk it lies in, string
//! pools are bounded in count and in the length of a decoded string, chunks are bounded in
//! number, and only the first element is looked at. The layout follows AOSP's
//! `ResourceTypes.h` (`ResChunk_header`, `ResStringPool_header`, `ResXMLTree_attrExt`,
//! `ResXMLTree_attribute`, `Res_value`), with its validity rules where they matter here: chunk
//! sizes 4-byte aligned, strings terminated, UTF-8 and UTF-16 pools.
//!
//! Not guaranteed: that a manifest which parses is Mojang's. Anybody can write
//! `package="com.mojang.minecraftpe"`. The check keeps out what is plainly not Minecraft (a
//! split's manifest, another app, a mangled file); the files themselves are as trustworthy as
//! the download they came from (see `store`).

use crate::error::{Error, Result};

/// The largest `AndroidManifest.xml` read (Minecraft's is a few tens of kilobytes).
pub const MAX_BYTES: usize = 1 << 20;
/// Strings in the pool, chunks in the file, and characters in one decoded string.
const MAX_STRINGS: usize = 1 << 16;
const MAX_CHUNKS: usize = 1 << 16;
const MAX_STRING_CHARS: usize = 4096;

const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_XML_TYPE: u16 = 0x0003;
const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
const RES_XML_END_ELEMENT_TYPE: u16 = 0x0103;
const RES_XML_RESOURCE_MAP_TYPE: u16 = 0x0180;
const UTF8_FLAG: u32 = 1 << 8;
const NO_ENTRY: u32 = 0xFFFF_FFFF;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;
const TYPE_INT_HEX: u8 = 0x11;
/// `android:versionCode` and `android:versionName` by resource id, for manifests whose
/// attribute names were stripped.
const ATTR_VERSION_CODE: u32 = 0x0101_021b;
const ATTR_VERSION_NAME: u32 = 0x0101_021c;

/// The root element's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApkInfo {
    pub package: String,
    pub version_code: u32,
    pub version_name: String,
    /// Set in the manifest of a split APK; the base APK's has none.
    pub split: Option<String>,
}

fn bad(why: impl Into<String>) -> Error {
    Error::Format(format!("AndroidManifest.xml: {}", why.into()))
}

fn u16_at(data: &[u8], at: usize) -> Result<u16> {
    data.get(at..at.checked_add(2).ok_or_else(|| bad("offset overflow"))?)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| bad("truncated"))
}

fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    data.get(at..at.checked_add(4).ok_or_else(|| bad("offset overflow"))?)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| bad("truncated"))
}

/// A chunk header, checked against the bytes it claims.
struct Chunk<'a> {
    kind: u16,
    header_size: usize,
    bytes: &'a [u8],
}

fn chunk_at(data: &[u8], at: usize) -> Result<Chunk<'_>> {
    let kind = u16_at(data, at)?;
    let header_size = usize::from(u16_at(data, at + 2)?);
    let size = u32_at(data, at + 4)? as usize;
    if header_size < 8 || size < header_size {
        return Err(bad("a chunk smaller than its header"));
    }
    if (header_size | size) & 3 != 0 {
        return Err(bad("a chunk that is not 4-byte aligned"));
    }
    let end = at.checked_add(size).filter(|end| *end <= data.len()).ok_or_else(|| bad("a chunk runs past the file"))?;
    Ok(Chunk { kind, header_size, bytes: &data[at..end] })
}

struct StringPool<'a> {
    count: usize,
    utf8: bool,
    offsets: &'a [u8],
    strings: &'a [u8],
}

impl<'a> StringPool<'a> {
    fn parse(chunk: &Chunk<'a>) -> Result<Self> {
        if chunk.header_size < 28 {
            return Err(bad("a string pool header that is too small"));
        }
        let bytes = chunk.bytes;
        let count = u32_at(bytes, 8)? as usize;
        let styles = u32_at(bytes, 12)? as usize;
        let flags = u32_at(bytes, 16)?;
        let strings_start = u32_at(bytes, 20)? as usize;
        let styles_start = u32_at(bytes, 24)? as usize;
        if count > MAX_STRINGS || styles > MAX_STRINGS {
            return Err(bad("too many strings"));
        }
        let offsets_end = chunk.header_size + 4 * (count + styles);
        if offsets_end > bytes.len() {
            return Err(bad("string offsets run past the pool"));
        }
        let strings = if count == 0 {
            &bytes[..0]
        } else {
            let end = if styles > 0 { styles_start } else { bytes.len() };
            if strings_start < offsets_end || strings_start > end || end > bytes.len() {
                return Err(bad("string data outside the pool"));
            }
            &bytes[strings_start..end]
        };
        Ok(StringPool {
            count,
            utf8: flags & UTF8_FLAG != 0,
            offsets: &bytes[chunk.header_size..chunk.header_size + 4 * count],
            strings,
        })
    }

    /// Length prefix of a UTF-8 pool string: one byte, or two with the high bit set.
    fn len8(data: &[u8], at: usize) -> Result<(usize, usize)> {
        let first = *data.get(at).ok_or_else(|| bad("truncated string"))?;
        if first & 0x80 == 0 {
            return Ok((usize::from(first), 1));
        }
        let second = *data.get(at + 1).ok_or_else(|| bad("truncated string"))?;
        Ok(((usize::from(first & 0x7f) << 8) | usize::from(second), 2))
    }

    /// Length prefix of a UTF-16 pool string: one unit, or two with the high bit set.
    fn len16(data: &[u8], at: usize) -> Result<(usize, usize)> {
        let first = u16_at(data, at)?;
        if first & 0x8000 == 0 {
            return Ok((usize::from(first), 2));
        }
        let second = u16_at(data, at + 2)?;
        Ok(((usize::from(first & 0x7fff) << 16) | usize::from(second), 4))
    }

    fn get(&self, index: u32) -> Result<String> {
        let index = index as usize;
        if index >= self.count {
            return Err(bad("a string index outside the pool"));
        }
        let at = u32_at(self.offsets, 4 * index)? as usize;
        let data = self.strings;
        if self.utf8 {
            let (chars, a) = Self::len8(data, at)?;
            let (len, b) = Self::len8(data, at + a)?;
            let start = at + a + b;
            let end = start.checked_add(len).ok_or_else(|| bad("string length overflow"))?;
            if chars > MAX_STRING_CHARS || len > 4 * MAX_STRING_CHARS {
                return Err(bad("a string that is too long"));
            }
            if data.get(end) != Some(&0) {
                return Err(bad("an unterminated string"));
            }
            let text = std::str::from_utf8(&data[start..end]).map_err(|_| bad("a string that is not UTF-8"))?;
            Ok(text.to_string())
        } else {
            let (units, a) = Self::len16(data, at)?;
            if units > MAX_STRING_CHARS {
                return Err(bad("a string that is too long"));
            }
            let start = at + a;
            let end = start + 2 * units;
            if u16_at(data, end)? != 0 {
                return Err(bad("an unterminated string"));
            }
            let raw = &data[start..end];
            let units = raw.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b));
            char::decode_utf16(units)
                .collect::<std::result::Result<String, _>>()
                .map_err(|_| bad("a string that is not UTF-16"))
        }
    }
}

/// Reads the root element. Everything after it is not looked at.
pub fn parse(data: &[u8]) -> Result<ApkInfo> {
    if data.len() > MAX_BYTES {
        return Err(bad(format!("{} bytes; the limit is {MAX_BYTES}", data.len())));
    }
    let file = chunk_at(data, 0)?;
    if file.kind != RES_XML_TYPE {
        return Err(bad("not a compiled XML file"));
    }
    let doc = file.bytes;
    let mut at = file.header_size;
    let mut pool: Option<StringPool<'_>> = None;
    let mut resource_ids: &[u8] = &[];
    for _ in 0..MAX_CHUNKS {
        if at >= doc.len() {
            return Err(bad("no root element"));
        }
        let chunk = chunk_at(doc, at)?;
        match chunk.kind {
            RES_STRING_POOL_TYPE if pool.is_some() => return Err(bad("two string pools")),
            RES_STRING_POOL_TYPE => pool = Some(StringPool::parse(&chunk)?),
            RES_XML_RESOURCE_MAP_TYPE => resource_ids = &chunk.bytes[chunk.header_size..],
            RES_XML_START_ELEMENT_TYPE => {
                let pool = pool.as_ref().ok_or_else(|| bad("an element before the string pool"))?;
                return root_element(&chunk, pool, resource_ids);
            }
            RES_XML_END_ELEMENT_TYPE => return Err(bad("an element ends before one starts")),
            // Namespaces, CDATA and chunk types this reader does not know: skipped, as
            // Android's own parser does.
            _ => {}
        }
        at += chunk.bytes.len();
    }
    Err(bad("too many chunks"))
}

fn root_element(chunk: &Chunk<'_>, pool: &StringPool<'_>, resource_ids: &[u8]) -> Result<ApkInfo> {
    // A node header is the chunk header plus a line number and a comment reference.
    if chunk.header_size < 16 {
        return Err(bad("an element header that is too small"));
    }
    let ext = &chunk.bytes[chunk.header_size..];
    let name = u32_at(ext, 4)?;
    let attribute_start = usize::from(u16_at(ext, 8)?);
    let attribute_size = usize::from(u16_at(ext, 10)?);
    let attribute_count = usize::from(u16_at(ext, 12)?);
    if pool.get(name)? != "manifest" {
        return Err(bad("the root element is not <manifest>"));
    }
    if attribute_size < 20 || attribute_start < 20 {
        return Err(bad("malformed attributes"));
    }
    let end = attribute_start + attribute_count * attribute_size;
    if end > ext.len() {
        return Err(bad("attributes run past the element"));
    }
    let (mut package, mut code, mut version, mut split) = (None, None, None, None);
    for i in 0..attribute_count {
        let attr = &ext[attribute_start + i * attribute_size..attribute_start + (i + 1) * attribute_size];
        let name_ref = u32_at(attr, 4)?;
        let raw = u32_at(attr, 8)?;
        let data_type = attr[15];
        let data = u32_at(attr, 16)?;
        let name = if name_ref == NO_ENTRY { String::new() } else { pool.get(name_ref)? };
        let resource_id =
            if (name_ref as usize) < resource_ids.len() / 4 { u32_at(resource_ids, 4 * name_ref as usize)? } else { 0 };
        let string_value = || -> Result<String> {
            if raw != NO_ENTRY {
                pool.get(raw)
            } else if data_type == TYPE_STRING {
                pool.get(data)
            } else {
                Err(bad(format!("attribute {name:?} is not a string")))
            }
        };
        let slot = if name == "versionCode" || resource_id == ATTR_VERSION_CODE {
            if !matches!(data_type, TYPE_INT_DEC | TYPE_INT_HEX) {
                return Err(bad("versionCode is not an integer"));
            }
            set(&mut code, data, "versionCode")?;
            continue;
        } else if name == "versionName" || resource_id == ATTR_VERSION_NAME {
            &mut version
        } else if name == "package" {
            &mut package
        } else if name == "split" {
            &mut split
        } else {
            continue;
        };
        let value = string_value()?;
        set(slot, value, &name)?;
    }
    let package = package.ok_or_else(|| bad("no package attribute"))?;
    let version_code = code.ok_or_else(|| bad("no versionCode"))?;
    let version_name = version.ok_or_else(|| bad("no versionName"))?;
    if package.is_empty()
        || package.len() > 255
        || !package.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_'))
    {
        return Err(bad("a package name that is not one"));
    }
    if version_name.is_empty() || version_name.chars().count() > 128 || version_name.chars().any(char::is_control) {
        return Err(bad("a version name that is empty, too long or has control characters"));
    }
    if version_code == 0 {
        return Err(bad("versionCode 0"));
    }
    Ok(ApkInfo { package, version_code, version_name, split })
}

fn set<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(bad(format!("attribute {name} given twice")));
    }
    Ok(())
}

/// A compiled manifest as `aapt2` lays it out (string pool, resource map, namespace, the root
/// element), for tests and fuzz seeds. `utf8` selects the pool's encoding; `strip_names`
/// leaves `versionCode` and `versionName` with empty names, known only by resource id.
#[doc(hidden)]
pub fn synthetic(package: &str, code: u32, name: &str, split: Option<&str>, utf8: bool, strip_names: bool) -> Vec<u8> {
    // String pool: attribute names first (the resource map covers them), then the rest.
    let mut strings: Vec<String> = vec![
        if strip_names { String::new() } else { "versionCode".into() },
        if strip_names { String::new() } else { "versionName".into() },
        "package".into(),
        "split".into(),
        "android".into(),
        "http://schemas.android.com/apk/res/android".into(),
        "manifest".into(),
        package.into(),
        name.into(),
    ];
    if let Some(split) = split {
        strings.push(split.into());
    }
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    for s in &strings {
        offsets.push(data.len() as u32);
        if utf8 {
            let units = s.encode_utf16().count();
            assert!(units < 0x80 && s.len() < 0x80, "short strings only");
            data.push(units as u8);
            data.push(s.len() as u8);
            data.extend_from_slice(s.as_bytes());
            data.push(0);
        } else {
            let units: Vec<u16> = s.encode_utf16().collect();
            data.extend_from_slice(&(units.len() as u16).to_le_bytes());
            for unit in units {
                data.extend_from_slice(&unit.to_le_bytes());
            }
            data.extend_from_slice(&[0, 0]);
        }
    }
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let header_size = 28u16;
    let strings_start = u32::from(header_size) + 4 * strings.len() as u32;
    let mut pool = Vec::new();
    pool.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
    pool.extend_from_slice(&header_size.to_le_bytes());
    pool.extend_from_slice(&(strings_start + data.len() as u32).to_le_bytes());
    pool.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    pool.extend_from_slice(&0u32.to_le_bytes());
    pool.extend_from_slice(&(if utf8 { UTF8_FLAG } else { 0 }).to_le_bytes());
    pool.extend_from_slice(&strings_start.to_le_bytes());
    pool.extend_from_slice(&0u32.to_le_bytes());
    for offset in offsets {
        pool.extend_from_slice(&offset.to_le_bytes());
    }
    pool.extend_from_slice(&data);

    let mut map = Vec::new();
    map.extend_from_slice(&RES_XML_RESOURCE_MAP_TYPE.to_le_bytes());
    map.extend_from_slice(&8u16.to_le_bytes());
    map.extend_from_slice(&16u32.to_le_bytes());
    map.extend_from_slice(&ATTR_VERSION_CODE.to_le_bytes());
    map.extend_from_slice(&ATTR_VERSION_NAME.to_le_bytes());

    let node = |kind: u16, body: &[u8]| {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&kind.to_le_bytes());
        chunk.extend_from_slice(&16u16.to_le_bytes());
        chunk.extend_from_slice(&(16 + body.len() as u32).to_le_bytes());
        chunk.extend_from_slice(&1u32.to_le_bytes());
        chunk.extend_from_slice(&NO_ENTRY.to_le_bytes());
        chunk.extend_from_slice(body);
        chunk
    };
    let namespace = [4u32.to_le_bytes(), 5u32.to_le_bytes()].concat();
    let attribute = |ns: u32, name: u32, raw: u32, data_type: u8, data: u32| {
        let mut a = Vec::new();
        for word in [ns, name, raw] {
            a.extend_from_slice(&word.to_le_bytes());
        }
        a.extend_from_slice(&8u16.to_le_bytes());
        a.push(0);
        a.push(data_type);
        a.extend_from_slice(&data.to_le_bytes());
        a
    };
    let mut attributes = vec![
        attribute(5, 0, NO_ENTRY, TYPE_INT_DEC, code),
        attribute(5, 1, 8, TYPE_STRING, 8),
        attribute(NO_ENTRY, 2, 7, TYPE_STRING, 7),
    ];
    if split.is_some() {
        attributes.push(attribute(NO_ENTRY, 3, 9, TYPE_STRING, 9));
    }
    let mut element = Vec::new();
    for word in [NO_ENTRY, 6] {
        element.extend_from_slice(&word.to_le_bytes());
    }
    for half in [20u16, 20, attributes.len() as u16, 0, 0, 0] {
        element.extend_from_slice(&half.to_le_bytes());
    }
    for a in &attributes {
        element.extend_from_slice(a);
    }
    let end_element = [NO_ENTRY.to_le_bytes(), 6u32.to_le_bytes()].concat();

    let body = [
        pool,
        map,
        node(0x0100, &namespace),
        node(RES_XML_START_ELEMENT_TYPE, &element),
        node(RES_XML_END_ELEMENT_TYPE, &end_element),
        node(0x0101, &namespace),
    ]
    .concat();
    let mut file = Vec::new();
    file.extend_from_slice(&RES_XML_TYPE.to_le_bytes());
    file.extend_from_slice(&8u16.to_le_bytes());
    file.extend_from_slice(&(8 + body.len() as u32).to_le_bytes());
    file.extend_from_slice(&body);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minecraft(utf8: bool, strip: bool) -> Vec<u8> {
        synthetic("com.mojang.minecraftpe", 982606201, "1.26.62.1", None, utf8, strip)
    }

    #[test]
    fn the_root_elements_package_version_code_and_name_are_read_from_either_pool_encoding() {
        for utf8 in [false, true] {
            let info = parse(&minecraft(utf8, false)).unwrap();
            assert_eq!(
                info,
                ApkInfo {
                    package: "com.mojang.minecraftpe".into(),
                    version_code: 982606201,
                    version_name: "1.26.62.1".into(),
                    split: None
                }
            );
        }
    }

    #[test]
    fn stripped_attribute_names_are_recognised_by_resource_id() {
        assert_eq!(parse(&minecraft(false, true)).unwrap().version_code, 982606201);
    }

    #[test]
    fn a_splits_manifest_says_so() {
        let split = synthetic("com.mojang.minecraftpe", 1, "1.0", Some("config.x86_64"), false, false);
        assert_eq!(parse(&split).unwrap().split.as_deref(), Some("config.x86_64"));
    }

    #[test]
    fn hostile_lengths_offsets_and_counts_are_refused_not_followed() {
        let good = minecraft(false, false);
        // The file claims to be longer than it is.
        let mut long = good.clone();
        long[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&long).is_err());
        // A string pool with an absurd string count.
        let mut many = good.clone();
        many[16..20].copy_from_slice(&0x00ff_ffffu32.to_le_bytes());
        assert!(parse(&many).is_err());
        // A misaligned chunk size.
        let mut odd = good.clone();
        let size = u32::from_le_bytes(odd[12..16].try_into().unwrap()) + 1;
        odd[12..16].copy_from_slice(&size.to_le_bytes());
        assert!(parse(&odd).is_err());
        // Every prefix: an error, never a panic.
        for cut in 0..good.len() {
            assert!(parse(&good[..cut]).is_err(), "{cut}");
        }
        assert!(parse(&vec![0u8; MAX_BYTES + 1]).is_err());
        assert!(parse(b"<?xml version=\"1.0\"?><manifest/>").is_err());
    }

    #[test]
    fn an_unterminated_string_is_refused() {
        let mut data = minecraft(true, false);
        // The first string ("versionCode") ends with a NUL; overwrite it.
        let pool_start = 8;
        let strings_start = u32::from_le_bytes(data[pool_start + 20..pool_start + 24].try_into().unwrap()) as usize;
        let terminator = pool_start + strings_start + 2 + "versionCode".len();
        assert_eq!(data[terminator], 0);
        data[terminator] = b'x';
        assert!(parse(&data).is_err());
    }
}
