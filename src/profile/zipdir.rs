//! A strict reader for the directory of a zip archive.
//!
//! A zip archive is read from its end: a small record there says where the *central
//! directory* is and how many members it lists, and the directory names the members. The
//! format leaves room for an archive to mean different things to different readers — a
//! trailing comment that contains a second end record, a member count that does not match
//! the directory, the same name twice, a second set of sizes and offsets ("zip64"), a name
//! in an extra field that replaces the one in the header. A check that reads the archive
//! one way says nothing about a program that reads it another way.
//!
//! This reader does not guess. It accepts the one shape that every reader takes for the
//! same thing, and refuses the rest:
//!
//! * the archive starts with a member and **ends with the end record** — no archive
//!   comment, nothing after it;
//! * the end record is the classic one: **no zip64**, one "disk";
//! * the directory lies exactly between the members and the end record, and consists of
//!   **exactly as many headers as the end record declares** — not one byte more or less;
//! * every header names a member that starts inside the archive, at a local header;
//! * names are UTF-8 (pure ASCII unless the header says UTF-8), free of control characters
//!   and backslashes, relative, without `..`, and **each occurs once**;
//! * no header carries a "Unicode path" or zip64 extra field (either could mean another
//!   name, or other offsets, to a reader that honours it);
//! * no member is a symbolic link, device, socket or FIFO, and none is encrypted.
//!
//! It is bounded before it reads: the member count is a 16-bit field (and checked against
//! [`Limits::max_entries`]), the directory is read in one piece only after its size has
//! been checked against [`Limits::max_directory_bytes`] and against the archive's own
//! length, and nothing is ever allocated on the strength of a number in the file alone.
//!
//! Not checked, deliberately: the members' *local* headers beyond their signature, and
//! whether members overlap or share data. A reader that walks local headers instead of the
//! directory (a streaming unzip) may therefore see other names than these. What this module
//! answers is what a directory-based reader is given — `java.util.zip.ZipFile`, which is
//! what Minecraft 26.3 opens a resource pack with (`FilePackResources`, seen in its
//! bytecode; how a shader mod opens a shader pack is that mod's business). Nothing is
//! decompressed.
//!
//! Why the count matters (tried with OpenJDK 27): given an archive whose end record says
//! "1 member" in front of a directory with three headers, a reader that takes the count
//! lists one member, and `java.util.zip.ZipFile` lists all three; given two headers with
//! the same name, the first kind keeps one and Java lists both.

use std::{collections::BTreeSet, fs::File, io};

use crate::error::{Error, Result};

const LOCAL_HEADER: [u8; 4] = *b"PK\x03\x04";
const CENTRAL_HEADER: [u8; 4] = *b"PK\x01\x02";
const END_RECORD: [u8; 4] = *b"PK\x05\x06";
const ZIP64_LOCATOR: [u8; 4] = *b"PK\x06\x07";
const END_RECORD_LEN: u64 = 22;
const ZIP64_LOCATOR_LEN: u64 = 20;
const CENTRAL_HEADER_LEN: usize = 46;
const LOCAL_HEADER_LEN: u64 = 30;

/// General-purpose flag bits: member encrypted (0), strong encryption (6), directory
/// values masked (13).
const FLAGS_ENCRYPTED: u16 = 1 | 1 << 6 | 1 << 13;
/// General-purpose flag bit 11: name and comment are UTF-8.
const FLAG_UTF8: u16 = 1 << 11;
const EXTRA_ZIP64: u16 = 0x0001;
const EXTRA_UNICODE_PATH: u16 = 0x7075;

/// Bounds for [`read`].
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Most members. (The format itself ends at 65,534 without zip64.)
    pub max_entries: usize,
    /// Largest directory that is read into memory.
    pub max_directory_bytes: u64,
    pub max_name_bytes: usize,
}

/// One member, as the directory describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    /// Where the member's local header starts.
    pub offset: u32,
}

/// Something that can be read at a position: a file, or bytes in memory.
pub trait ReadAt {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;
}

impl ReadAt for File {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(self, buf, offset)
    }
}

impl ReadAt for [u8] {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let start = usize::try_from(offset).ok().filter(|start| *start <= self.len());
        match start.and_then(|start| self.get(start..start.checked_add(buf.len())?)) {
            Some(bytes) => {
                buf.copy_from_slice(bytes);
                Ok(())
            }
            None => Err(io::ErrorKind::UnexpectedEof.into()),
        }
    }
}

fn malformed(what: impl AsRef<str>) -> Error {
    Error::Format(format!("not acceptable as a zip archive: {}", what.as_ref()))
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Is `name` one that means the same path to everything that handles it, inside the
/// archive's own tree?
fn check_name(raw: &[u8], utf8_flag: bool, limits: &Limits) -> Result<String> {
    let shown = || crate::util::sanitize_line(raw, 80);
    if raw.is_empty() || raw.len() > limits.max_name_bytes {
        return Err(malformed(format!("a member's name is empty or longer than {} bytes", limits.max_name_bytes)));
    }
    let name = std::str::from_utf8(raw)
        .ok()
        // Without the UTF-8 flag the bytes are in some code page; only ASCII is the same in all.
        .filter(|name| utf8_flag || name.is_ascii())
        .ok_or_else(|| malformed(format!("a member's name is not UTF-8 text ({})", shown())))?;
    let escapes = name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || name.split('/').any(|part| part == "..");
    if escapes {
        return Err(Error::Policy(format!(
            "the archive contains a member whose name points outside the archive or is not a plain path ({})",
            shown()
        )));
    }
    Ok(name.to_string())
}

/// The records of an extra field: none of them may be one that changes what the header
/// says, and they must fill the field exactly.
fn check_extra(extra: &[u8], name: &str) -> Result<()> {
    let shown = || crate::util::sanitize_line(name.as_bytes(), 80);
    let mut rest = extra;
    while !rest.is_empty() {
        if rest.len() < 4 {
            return Err(malformed(format!("the extra data of member {} is cut short", shown())));
        }
        let (id, size) = (u16_at(rest, 0), usize::from(u16_at(rest, 2)));
        if size > rest.len() - 4 {
            return Err(malformed(format!("the extra data of member {} is cut short", shown())));
        }
        match id {
            EXTRA_ZIP64 => return Err(malformed("it uses the zip64 format, which is not accepted here")),
            EXTRA_UNICODE_PATH => {
                return Err(malformed(format!(
                    "member {} carries a second name in an extra field; which of the two a reader uses differs",
                    shown()
                )));
            }
            _ => {}
        }
        rest = &rest[4 + size..];
    }
    Ok(())
}

/// Read the directory of the archive in `source`, which is `length` bytes long. Returns
/// the members in directory order, or the reason the archive is not one of the shape
/// described in the module documentation.
pub fn read(source: &(impl ReadAt + ?Sized), length: u64, limits: &Limits) -> Result<Vec<Entry>> {
    let io = |e: io::Error| Error::io("reading the archive", e);
    if length < LOCAL_HEADER_LEN + END_RECORD_LEN {
        return Err(malformed("it is too short to hold a member"));
    }
    let mut magic = [0u8; 4];
    source.read_exact_at(&mut magic, 0).map_err(io)?;
    if magic != LOCAL_HEADER {
        // An archive that starts with something else is a polyglot at best.
        return Err(malformed("it does not start with a zip entry"));
    }

    // The end record: the last 22 bytes, with no comment after it.
    let end_at = length - END_RECORD_LEN;
    let mut end = [0u8; END_RECORD_LEN as usize];
    source.read_exact_at(&mut end, end_at).map_err(io)?;
    if end[..4] != END_RECORD || u16_at(&end, 20) != 0 {
        return Err(malformed(
            "it does not end in a plain end-of-archive record (an archive comment or data after the archive is \
             not accepted: re-create the zip without them)",
        ));
    }
    let (disk, directory_disk) = (u16_at(&end, 4), u16_at(&end, 6));
    let (here, total) = (u16_at(&end, 8), u16_at(&end, 10));
    let (directory_size, directory_at) = (u32_at(&end, 12), u32_at(&end, 16));
    if disk != 0 || directory_disk != 0 || here != total {
        return Err(malformed("it claims to be one part of a multi-part archive"));
    }
    // zip64: sentinel values here, or a zip64 locator in front of the end record. A second
    // set of counts and offsets is a second opinion about what the archive contains.
    let mut locator = [0u8; 4];
    if end_at >= ZIP64_LOCATOR_LEN {
        source.read_exact_at(&mut locator, end_at - ZIP64_LOCATOR_LEN).map_err(io)?;
    }
    if total == u16::MAX || directory_size == u32::MAX || directory_at == u32::MAX || locator == ZIP64_LOCATOR {
        return Err(malformed(
            "it uses the zip64 format (more than 65,534 members, or 4 GiB and more), which is not accepted here",
        ));
    }
    let total = usize::from(total);
    if total == 0 {
        return Err(malformed("it has no members"));
    }
    if total > limits.max_entries {
        return Err(malformed(format!("it has {total} members; the limit is {}", limits.max_entries)));
    }
    // The directory ends exactly where the end record begins, and starts after the first
    // member's header. (Both numbers are 32 bits wide: the sum cannot overflow 64.)
    let (directory_at, directory_size) = (u64::from(directory_at), u64::from(directory_size));
    if directory_at + directory_size != end_at || directory_at < LOCAL_HEADER_LEN {
        return Err(malformed(
            "its directory does not lie directly in front of the end-of-archive record (data was put in front \
             of, behind or into the archive)",
        ));
    }
    if directory_size > limits.max_directory_bytes {
        return Err(malformed(format!(
            "its directory is {} MiB; the limit is {} MiB",
            directory_size >> 20,
            limits.max_directory_bytes >> 20
        )));
    }
    if directory_size < (total * CENTRAL_HEADER_LEN) as u64 {
        return Err(malformed("its directory is too small for the number of members it declares"));
    }

    // Bounded above by `max_directory_bytes`, and known to lie inside the file.
    let mut directory = vec![0u8; directory_size as usize];
    source.read_exact_at(&mut directory, directory_at).map_err(io)?;

    let mut entries = Vec::with_capacity(total);
    let mut seen = BTreeSet::new();
    let mut rest = directory.as_slice();
    for index in 0..total {
        if rest.len() < CENTRAL_HEADER_LEN || rest[..4] != CENTRAL_HEADER {
            return Err(malformed(format!(
                "its directory ends after {index} of the {total} members the end-of-archive record declares"
            )));
        }
        let flags = u16_at(rest, 8);
        let (compressed_size, uncompressed_size) = (u32_at(rest, 20), u32_at(rest, 24));
        let (name_len, extra_len, comment_len) =
            (usize::from(u16_at(rest, 28)), usize::from(u16_at(rest, 30)), usize::from(u16_at(rest, 32)));
        let (disk_start, external, offset) = (u16_at(rest, 34), u32_at(rest, 38), u32_at(rest, 42));
        let header_len = CENTRAL_HEADER_LEN + name_len + extra_len + comment_len;
        if header_len > rest.len() {
            return Err(malformed(format!("the directory entry of member {} is cut short", index + 1)));
        }
        let name =
            check_name(&rest[CENTRAL_HEADER_LEN..CENTRAL_HEADER_LEN + name_len], flags & FLAG_UTF8 != 0, limits)?;
        let shown = || crate::util::sanitize_line(name.as_bytes(), 80);
        check_extra(&rest[CENTRAL_HEADER_LEN + name_len..CENTRAL_HEADER_LEN + name_len + extra_len], &name)?;
        if compressed_size == u32::MAX || uncompressed_size == u32::MAX || offset == u32::MAX || disk_start == u16::MAX
        {
            return Err(malformed("it uses the zip64 format, which is not accepted here"));
        }
        if disk_start != 0 {
            return Err(malformed("it claims to be one part of a multi-part archive"));
        }
        if flags & FLAGS_ENCRYPTED != 0 {
            return Err(malformed(format!("member {} is encrypted", shown())));
        }
        // The file type in the Unix mode bits, wherever the archive says it was made: an
        // extractor that believes them would create that kind of file.
        if !matches!((external >> 16) & 0o170000, 0 | 0o100000 | 0o040000) {
            return Err(Error::Policy(format!(
                "the archive contains a member that is a link or a special file ({}); refusing it",
                shown()
            )));
        }
        // The member starts inside the archive, in front of the directory, at a local header.
        let mut local = [0u8; 4];
        if u64::from(offset) + LOCAL_HEADER_LEN > directory_at {
            return Err(malformed(format!("member {} is said to start outside the archive's data", shown())));
        }
        source.read_exact_at(&mut local, u64::from(offset)).map_err(io)?;
        if local != LOCAL_HEADER {
            return Err(malformed(format!("member {} does not start at a zip entry", shown())));
        }
        if !seen.insert(name.clone()) {
            return Err(malformed(format!(
                "the name {} occurs more than once; which member a reader uses for it differs",
                shown()
            )));
        }
        entries.push(Entry { name, compressed_size, uncompressed_size, offset });
        rest = &rest[header_len..];
    }
    if !rest.is_empty() {
        // The case this module exists for: a directory that holds more than the count says.
        // A reader that walks the directory to its end would see members this check did not.
        return Err(malformed(format!(
            "its directory holds {} byte(s) beyond the {total} members the end-of-archive record declares",
            rest.len()
        )));
    }
    Ok(entries)
}

/// For the robustness tests and fuzz targets. Whatever the bytes are, [`read`] returns; and
/// an archive it accepts has the properties it promises — including that another
/// implementation of the format, where it can read the archive at all, finds the very same
/// names in it.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    let limits = Limits { max_entries: 1000, max_directory_bytes: 1 << 20, max_name_bytes: 512 };
    let Ok(entries) = read(data, data.len() as u64, &limits) else { return };
    assert!(!entries.is_empty() && entries.len() <= limits.max_entries);
    let mut names = BTreeSet::new();
    for entry in &entries {
        let name = &entry.name;
        assert!(names.insert(name.as_str()), "{name:?} twice");
        assert!(!name.is_empty() && name.len() <= limits.max_name_bytes, "{name:?}");
        assert!(!name.starts_with('/') && !name.contains('\\') && !name.chars().any(char::is_control), "{name:?}");
        assert!(name.split('/').all(|part| part != ".."), "{name:?}");
        assert!(data[entry.offset as usize..].starts_with(&LOCAL_HEADER), "{name:?} does not start at a member");
    }
    if let Ok(other) = zip::ZipArchive::new(io::Cursor::new(data)) {
        let theirs: Vec<Vec<u8>> = other.file_names_raw().map(<[u8]>::to_vec).collect();
        let ours: Vec<Vec<u8>> = entries.iter().map(|e| e.name.clone().into_bytes()).collect();
        assert_eq!(theirs, ours, "another zip reader lists other members than the strict one accepted");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub(crate) const LIMITS: Limits = Limits { max_entries: 1000, max_directory_bytes: 1 << 20, max_name_bytes: 512 };

    /// An archive as an ordinary zip writer produces it (stored members).
    pub(crate) fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// Where the end record and the directory of a well-formed archive are.
    fn layout(archive: &[u8]) -> (usize, usize, usize) {
        let end = archive.len() - 22;
        assert_eq!(&archive[end..end + 4], b"PK\x05\x06");
        (end, u32_at(archive, end + 16) as usize, u32_at(archive, end + 12) as usize)
    }

    fn names(archive: &[u8]) -> Result<Vec<String>> {
        read(archive, archive.len() as u64, &LIMITS).map(|entries| entries.into_iter().map(|e| e.name).collect())
    }

    fn refused(archive: &[u8], why: &str) {
        match names(archive) {
            Ok(names) => panic!("accepted ({names:?}); expected a refusal mentioning {why:?}"),
            Err(e) => assert!(e.to_string().contains(why), "{e} — expected it to mention {why:?}"),
        }
    }

    #[test]
    fn an_ordinary_archive_is_listed_in_directory_order() {
        let archive = zip_of(&[("pack.mcmeta", b"{}"), ("assets/minecraft/lang/en_us.json", b"{}"), ("assets/", b"")]);
        assert_eq!(names(&archive).unwrap(), ["pack.mcmeta", "assets/minecraft/lang/en_us.json", "assets/"]);
        let entries = read(archive.as_slice(), archive.len() as u64, &LIMITS).unwrap();
        assert_eq!((entries[0].offset, entries[0].uncompressed_size), (0, 2));
        // The same through a file.
        let path = std::env::temp_dir()
            .join(format!("bastion-zipdir-{}", crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())));
        std::fs::write(&path, &archive).unwrap();
        let file = File::open(&path).unwrap();
        assert_eq!(read(&file, archive.len() as u64, &LIMITS).unwrap(), entries);
        // A length that is not the file's is not believed.
        assert!(read(&file, archive.len() as u64 + 1, &LIMITS).is_err());
        assert!(read(&file, archive.len() as u64 - 1, &LIMITS).is_err());
        std::fs::remove_file(path).unwrap();
        // UTF-8 names with the flag that says so.
        assert_eq!(names(&zip_of(&[("pack.mcmeta", b"{}"), ("assets/é.txt", b"x")])).unwrap()[1], "assets/é.txt");
    }

    /// The finding: an end record that declares one member in front of a directory that
    /// holds more. A reader that takes the count (as the zip crate does) sees `pack.mcmeta`;
    /// a reader that walks the directory to its end sees the rest too.
    #[test]
    fn a_directory_that_holds_more_than_the_declared_count_is_refused() {
        let mut archive = zip_of(&[("pack.mcmeta", b"{}"), ("../../.bashrc", b"x"), ("assets/a.txt", b"y")]);
        let (end, ..) = layout(&archive);
        archive[end + 8..end + 12].copy_from_slice(&[1, 0, 1, 0]); // "one member"
        // What the general-purpose reader makes of it: a one-member archive, nothing wrong.
        let lenient = zip::ZipArchive::new(io::Cursor::new(archive.clone())).expect("the zip crate reads it");
        assert_eq!(lenient.len(), 1, "it stops at the declared count");
        refused(&archive, "beyond the 1 members");

        // The other way round: more declared than there are.
        let mut archive = zip_of(&[("pack.mcmeta", b"{}")]);
        let (end, ..) = layout(&archive);
        archive[end + 8..end + 12].copy_from_slice(&[2, 0, 2, 0]);
        refused(&archive, "too small for the number of members");
    }

    #[test]
    fn a_name_that_occurs_twice_is_refused() {
        // Two members of the same length of name; the second is renamed to the first's.
        let mut archive = zip_of(&[("pack.mcmeta", b"{}"), ("pack.mcmetb", b"other")]);
        let (_, directory, _) = layout(&archive);
        let second = directory + archive[directory..].windows(11).rposition(|w| w == b"pack.mcmetb").unwrap();
        archive[second + 10] = b'a';
        let lenient = zip::ZipArchive::new(io::Cursor::new(archive.clone())).expect("the zip crate reads it");
        assert_eq!(lenient.len(), 1, "the general-purpose reader keeps one of the two, silently");
        refused(&archive, "occurs more than once");
    }

    #[test]
    fn archives_that_readers_could_take_for_different_things_are_refused() {
        let good = zip_of(&[("pack.mcmeta", b"{}"), ("assets/a.txt", b"y")]);
        let (end, directory, size) = layout(&good);

        // An archive comment (which could hold a second end record).
        let mut commented = good.clone();
        commented[end + 20] = 4;
        commented.extend_from_slice(b"hey!");
        refused(&commented, "does not end in a plain end-of-archive record");
        // Data after the archive, data in front of it.
        refused(&[good.as_slice(), b"trailing"].concat(), "does not end in a plain end-of-archive record");
        refused(&[b"#!/bin/sh\n".as_slice(), &good].concat(), "does not start with a zip entry");
        // A gap between the directory and the end record (a second directory could sit there).
        let mut gap = good[..end].to_vec();
        gap.extend_from_slice(&[0u8; 46]);
        gap.extend_from_slice(&good[end..]);
        refused(&gap, "does not lie directly in front");
        // zip64: sentinel values, or a locator in front of the end record.
        let mut sentinel = good.clone();
        sentinel[end + 16..end + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        refused(&sentinel, "zip64");
        let mut located = good[..end].to_vec();
        located.extend_from_slice(b"PK\x06\x07");
        located.extend_from_slice(&[0u8; 16]);
        let mut record = good[end..].to_vec();
        record[12..16].copy_from_slice(&((size + 20) as u32).to_le_bytes());
        located.extend_from_slice(&record);
        refused(&located, "zip64");
        // Multi-part.
        let mut parts = good.clone();
        parts[end + 4] = 1;
        refused(&parts, "multi-part");
        // A member that starts in the directory, or not at a member.
        let mut outside = good.clone();
        outside[directory + 42..directory + 46].copy_from_slice(&(directory as u32).to_le_bytes());
        refused(&outside, "start outside the archive's data");
        let mut elsewhere = good.clone();
        elsewhere[directory + 42] = 5;
        refused(&elsewhere, "does not start at a zip entry");
        // An extra field that carries another name for the member (Info-ZIP "Unicode path").
        let extra = [0x75, 0x70, 5, 0, 1, 0, 0, 0, 0];
        let name_len = u16_at(&good, directory + 28) as usize;
        let mut renamed = good[..directory + 46 + name_len].to_vec();
        renamed.extend_from_slice(&extra);
        renamed.extend_from_slice(&good[directory + 46 + name_len..end]);
        renamed[directory + 30] = extra.len() as u8;
        let mut record = good[end..].to_vec();
        record[12..16].copy_from_slice(&((size + extra.len()) as u32).to_le_bytes());
        renamed.extend_from_slice(&record);
        refused(&renamed, "second name in an extra field");
        // Encrypted, by the flag.
        let mut encrypted = good.clone();
        encrypted[directory + 8] |= 1;
        refused(&encrypted, "is encrypted");
        // Too many members, too large a directory, for the caller's limits.
        let tight = Limits { max_entries: 1, ..LIMITS };
        assert!(read(good.as_slice(), good.len() as u64, &tight).unwrap_err().to_string().contains("the limit is 1"));
        let small = Limits { max_directory_bytes: 16, ..LIMITS };
        assert!(read(good.as_slice(), good.len() as u64, &small).is_err());
        // Not an archive at all.
        refused(b"just text", "too short");
        refused(&[b'x'; 200], "does not start with a zip entry");
        refused(&[LOCAL_HEADER.as_slice(), &[0u8; 200]].concat(), "does not end in a plain end-of-archive record");
    }

    #[test]
    fn names_that_leave_the_archive_and_links_are_refused() {
        for bad in ["../../.bashrc", "/etc/cron.d/x", "a/../../b", "assets\\x.txt", "bell\u{7}.txt"] {
            let err = names(&zip_of(&[("pack.mcmeta", b"{}"), (bad, b"x")])).unwrap_err();
            assert!(matches!(err, Error::Policy(_)), "{bad:?}: {err}");
        }
        // A name that is not UTF-8, and one that is but is not declared to be.
        let mut archive = zip_of(&[("pack.mcmeta", b"{}"), ("assets/x.txt", b"x")]);
        let (_, directory, _) = layout(&archive);
        let at = directory + archive[directory..].windows(12).rposition(|w| w == b"assets/x.txt").unwrap();
        archive[at + 7] = 0xff;
        refused(&archive, "not UTF-8 text");
        let mut undeclared = zip_of(&[("pack.mcmeta", b"{}"), ("assets/é.txt", b"x")]);
        let (_, directory, _) = layout(&undeclared);
        let second = directory + 46 + 11; // after the first header and its 11-byte name
        assert_eq!(&undeclared[second..second + 4], b"PK\x01\x02");
        undeclared[second + 9] &= !0x08; // clear the UTF-8 flag (bit 11)
        refused(&undeclared, "not UTF-8 text");

        // A symbolic link, by its Unix mode — whatever system the archive claims to be from.
        let mut link = zip_of(&[("pack.mcmeta", b"{}"), ("assets/link", b"/etc/passwd")]);
        let (_, directory, _) = layout(&link);
        let second = directory + 46 + 11;
        link[second + 38..second + 42].copy_from_slice(&((0o120777u32) << 16).to_le_bytes());
        let err = names(&link).unwrap_err();
        assert!(matches!(&err, Error::Policy(m) if m.contains("link or a special file")), "{err}");
        // A FIFO likewise; a directory and a regular file are what an archive consists of.
        link[second + 38..second + 42].copy_from_slice(&((0o010644u32) << 16).to_le_bytes());
        assert!(names(&link).is_err());
        link[second + 38..second + 42].copy_from_slice(&((0o100644u32) << 16).to_le_bytes());
        assert_eq!(names(&link).unwrap().len(), 2);
    }

    /// A count that would cost gigabytes if it were believed costs nothing here: the
    /// directory's size is checked before a byte of it is read, and the count is 16 bits.
    #[test]
    fn nothing_is_allocated_on_the_strength_of_a_number_in_the_file() {
        let mut archive = zip_of(&[("pack.mcmeta", b"{}")]);
        let (end, ..) = layout(&archive);
        // "65,534 members in a 3 GiB directory", in a file of a few hundred bytes.
        archive[end + 8..end + 12].copy_from_slice(&[0xfe, 0xff, 0xfe, 0xff]);
        archive[end + 12..end + 16].copy_from_slice(&(3u32 << 30).to_le_bytes());
        let roomy = Limits { max_entries: 100_000, max_directory_bytes: 8 << 30, max_name_bytes: 512 };
        let started = std::time::Instant::now();
        let err = read(archive.as_slice(), archive.len() as u64, &roomy).unwrap_err().to_string();
        assert!(err.contains("does not lie directly in front"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn the_harness_accepts_what_the_reader_accepts() {
        fuzz(&zip_of(&[("pack.mcmeta", b"{}"), ("assets/minecraft/textures/block/stone.png", b"png")]));
        fuzz(&zip_of(&[("shaders/final.fsh", b"void main(){}")]));
        fuzz(b"");
        fuzz(b"PK\x03\x04");
    }
}
