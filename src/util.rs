//! Small shared helpers: hex, hashing, careful file handling, output sanitising.

use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use base64::Engine;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::{Context, Error, Result};

// ---------------------------------------------------------------- hex / base64

pub fn hex_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(TABLE[(b >> 4) as usize] as char);
        out.push(TABLE[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn hex_decode(s: &str) -> Result<Vec<u8>> {
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let raw = s.as_bytes();
    if !raw.len().is_multiple_of(2) {
        return Err(Error::Format("hex string has odd length".into()));
    }
    raw.as_chunks::<2>()
        .0
        .iter()
        .map(|p| match (nibble(p[0]), nibble(p[1])) {
            (Some(h), Some(l)) => Ok((h << 4) | l),
            _ => Err(Error::Format("invalid hex digit".into())),
        })
        .collect()
}

pub fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|_| Error::Format("invalid base64".into()))
}

pub fn b64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Constant-time equality for values an attacker may probe (OAuth `state`, pins).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| Error::Other(format!("kernel CSPRNG unavailable: {e}")))?;
    Ok(buf)
}

// ---------------------------------------------------------------- hashing

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlg {
    /// Only because Mojang publishes nothing stronger. Second-preimage resistance is what the
    /// launcher relies on, and that still holds for SHA-1.
    Sha1,
    Sha256,
}

impl HashAlg {
    pub fn hex_len(self) -> usize {
        match self {
            HashAlg::Sha1 => 40,
            HashAlg::Sha256 => 64,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            HashAlg::Sha1 => "sha1",
            HashAlg::Sha256 => "sha256",
        }
    }
}

pub enum Hasher {
    Sha1(Sha1),
    Sha256(Sha256),
}

impl Hasher {
    pub fn new(alg: HashAlg) -> Self {
        match alg {
            HashAlg::Sha1 => Hasher::Sha1(Sha1::new()),
            HashAlg::Sha256 => Hasher::Sha256(Sha256::new()),
        }
    }
    pub fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Sha1(h) => h.update(data),
            Hasher::Sha256(h) => h.update(data),
        }
    }
    pub fn finish(self) -> Vec<u8> {
        match self {
            Hasher::Sha1(h) => h.finalize().to_vec(),
            Hasher::Sha256(h) => h.finalize().to_vec(),
        }
    }
}

pub fn hash_bytes(alg: HashAlg, data: &[u8]) -> Vec<u8> {
    let mut h = Hasher::new(alg);
    h.update(data);
    h.finish()
}

/// Hash everything a reader yields; returns the digest and the number of bytes consumed.
pub fn hash_reader<R: Read>(alg: HashAlg, mut reader: R) -> io::Result<(Vec<u8>, u64)> {
    let mut h = Hasher::new(alg);
    let mut buf = vec![0u8; 128 * 1024];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..n]);
        total += n as u64;
    }
    Ok((h.finish(), total))
}

/// A digest we expect a file to have. Hex is validated and normalised once, here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    pub alg: HashAlg,
    hex: String,
}

impl Expected {
    pub fn new(alg: HashAlg, hex: &str) -> Result<Self> {
        let hex = hex.trim().to_ascii_lowercase();
        if hex.len() != alg.hex_len() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Format(format!("not a valid {} digest: {hex:?}", alg.name())));
        }
        Ok(Expected { alg, hex })
    }
    pub fn sha1(hex: &str) -> Result<Self> {
        Self::new(HashAlg::Sha1, hex)
    }
    pub fn sha256(hex: &str) -> Result<Self> {
        Self::new(HashAlg::Sha256, hex)
    }
    pub fn hex(&self) -> &str {
        &self.hex
    }
    pub fn matches(&self, digest: &[u8]) -> bool {
        hex_encode(digest) == self.hex
    }

    /// Hash an already-open file from offset 0 and compare digest (and size, when known).
    /// The caller keeps using this very file descriptor afterwards, which is what makes the
    /// check immune to path swaps.
    pub fn verify_file(&self, file: &File, size: Option<u64>, what: &str) -> Result<()> {
        use std::os::unix::fs::FileExt;
        struct At<'a>(&'a File, u64);
        impl Read for At<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.0.read_at(buf, self.1)?;
                self.1 += n as u64;
                Ok(n)
            }
        }
        let (digest, len) = hash_reader(self.alg, At(file, 0)).ctx(|| format!("hashing {what}"))?;
        if let Some(size) = size
            && size != len
        {
            return Err(Error::Integrity(format!("{what}: size is {len} bytes, expected {size}")));
        }
        if !self.matches(&digest) {
            return Err(Error::Integrity(format!(
                "{what}: {} is {}, expected {}",
                self.alg.name(),
                hex_encode(&digest),
                self.hex
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- files

/// Open for reading without following a final symlink, and insist on a regular file.
/// `O_NONBLOCK` keeps a planted FIFO from hanging us before the type check.
pub fn open_regular_nofollow(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    require_regular(&file)?;
    Ok(file)
}

/// `openat` variant: the name is resolved relative to an already-open directory.
pub fn openat_regular_nofollow(dir: impl AsFd, name: &OsStr) -> io::Result<File> {
    use nix::{fcntl::OFlag, sys::stat::Mode};
    if name.is_empty() || name.as_encoded_bytes().contains(&b'/') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a plain file name"));
    }
    let fd = nix::fcntl::openat(
        dir,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?;
    let file = File::from(fd);
    require_regular(&file)?;
    Ok(file)
}

fn require_regular(file: &File) -> io::Result<()> {
    if file.metadata()?.file_type().is_file() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"))
    }
}

/// Create a directory tree that only we can enter.
pub fn create_private_dir_all(path: &Path) -> Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path).ctx(|| format!("creating {}", path.display()))?;
    require_private(path)
}

/// The path must be ours and must not be writable by anyone else.
pub fn require_private(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).ctx(|| format!("inspecting {}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(format!("{} is a symlink; refusing to trust it", path.display())));
    }
    let euid = nix::unistd::geteuid().as_raw();
    if meta.uid() != euid {
        return Err(Error::Policy(format!("{} is owned by uid {}, not by you", path.display(), meta.uid())));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(Error::Policy(format!(
            "{} is writable by group or others (mode {:o}); fix with chmod go-w",
            path.display(),
            meta.mode() & 0o7777
        )));
    }
    Ok(())
}

/// System files we execute or trust (JVM, bwrap, CA bundle) must be root-owned and not
/// writable by anyone else, after resolving symlinks. Returns the canonical path.
pub fn require_root_owned(path: &Path) -> Result<PathBuf> {
    let real = fs::canonicalize(path).ctx(|| format!("resolving {}", path.display()))?;
    let meta = fs::metadata(&real).ctx(|| format!("inspecting {}", real.display()))?;
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err(Error::Policy(format!(
            "{} must be owned by root and not group/world-writable (uid {}, mode {:o})",
            real.display(),
            meta.uid(),
            meta.mode() & 0o7777
        )));
    }
    Ok(real)
}

/// Write-then-rename so readers never observe a half-written file. The temporary file is
/// created with `O_EXCL | O_NOFOLLOW`, so a pre-planted symlink cannot redirect the write.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    let dir = path.parent().ok_or_else(|| Error::Other(format!("{} has no parent", path.display())))?;
    let name = path.file_name().ok_or_else(|| Error::Other(format!("{} has no file name", path.display())))?;
    let tmp = dir.join(format!(".{}.tmp-{}", name.to_string_lossy(), hex_encode(&random_bytes::<8>()?)));
    let result = (|| -> io::Result<()> {
        let mut f =
            OpenOptions::new().write(true).create_new(true).mode(mode).custom_flags(libc::O_NOFOLLOW).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        // Persist the rename itself.
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.ctx(|| format!("writing {}", path.display()))
}

/// Read a small file fully, refusing symlinks and oversized content.
pub fn read_small_file(path: &Path, max: u64) -> Result<Vec<u8>> {
    let file = open_regular_nofollow(path).ctx(|| format!("opening {}", path.display()))?;
    let len = file.metadata().ctx(|| format!("inspecting {}", path.display()))?.len();
    if len > max {
        return Err(Error::Format(format!("{} is {len} bytes; limit is {max}", path.display())));
    }
    let mut buf = Vec::with_capacity(len as usize);
    file.take(max + 1).read_to_end(&mut buf).ctx(|| format!("reading {}", path.display()))?;
    if buf.len() as u64 > max {
        return Err(Error::Format(format!("{} grew past the {max} byte limit", path.display())));
    }
    Ok(buf)
}

// ---------------------------------------------------------------- output hygiene

/// Make one line of untrusted process output safe to print on a terminal and to store in a
/// log: control characters (including ESC, so no ANSI/OSC sequences survive) are replaced,
/// invalid UTF-8 is replaced, and the line is capped.
pub fn sanitize_line(raw: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut out = String::with_capacity(text.len().min(max_chars) + 16);
    for (count, ch) in text.chars().enumerate() {
        if count >= max_chars {
            out.push_str(" …[truncated]");
            break;
        }
        match ch {
            '\t' => out.push(' '),
            c if c.is_control() => out.push('\u{FFFD}'),
            // Bidi overrides and other invisible format characters can spoof log content.
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' => {
                out.push('\u{FFFD}')
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_rejects_garbage() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0x1a]), "00ff1a");
        assert_eq!(hex_decode("00FF1a").unwrap(), vec![0x00, 0xff, 0x1a]);
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn known_digests() {
        assert_eq!(hex_encode(&hash_bytes(HashAlg::Sha1, b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex_encode(&hash_bytes(HashAlg::Sha256, b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn expected_rejects_wrong_shape() {
        assert!(Expected::sha1("abc").is_err());
        assert!(Expected::sha256(&"g".repeat(64)).is_err());
        let e = Expected::sha1("A9993E364706816ABA3E25717850C26C9CD0D89D").unwrap();
        assert!(e.matches(&hash_bytes(HashAlg::Sha1, b"abc")));
        assert!(!e.matches(&hash_bytes(HashAlg::Sha1, b"abd")));
    }

    #[test]
    fn sanitize_strips_escape_sequences() {
        let s = sanitize_line(b"ok \x1b[31mred\x1b[0m \x07bell \xff", 200);
        assert!(!s.contains('\x1b') && !s.contains('\x07'));
        assert!(s.starts_with("ok "));
        assert!(sanitize_line(&[b'a'; 500], 10).ends_with("[truncated]"));
    }

    #[test]
    fn ct_eq_basic() {
        assert!(ct_eq(b"same", b"same"));
        assert!(!ct_eq(b"same", b"diff"));
        assert!(!ct_eq(b"short", b"longer"));
    }
}
