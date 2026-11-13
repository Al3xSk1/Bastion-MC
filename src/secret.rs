//! Secret material that must never reach swap, a core dump, or a forked child.
//!
//! A [`Secret`] owns its own anonymous mapping:
//!   * `mlock`ed before the first secret byte is written (never swapped),
//!   * `MADV_DONTDUMP` (excluded from core dumps even if dumping were enabled),
//!   * `MADV_WIPEONFORK` (a forked child sees zero pages, so the fork/exec window of
//!     `Command::spawn` cannot leak it),
//!   * never reallocated (fixed capacity), so no stale copies are left behind,
//!   * wiped with `explicit_bzero` on [`Secret::wipe`] and on drop.
//!
//! If the pages cannot be locked the constructor fails: the launcher refuses to hold a token
//! in swappable memory rather than degrade silently.
//!
//! Scope, stated honestly: this protects the launcher's *own* long-lived copies. Bytes that
//! transit the TLS/HTTP stack live briefly in ordinary heap buffers; those are scrubbed by the
//! zero-on-free global allocator (see `hardening`), not by this type.

use std::{fmt, num::NonZeroUsize, ptr::NonNull};

use nix::sys::mman::{MapFlags, MmapAdvise, ProtFlags, madvise, mlock, mmap_anonymous, munmap};

use crate::error::{Error, Result};

pub struct Secret {
    ptr: NonNull<u8>,
    len: usize,
    cap: usize,
}

// SAFETY: `Secret` exclusively owns its mapping; access goes through `&self`/`&mut self`.
unsafe impl Send for Secret {}
// SAFETY: shared access only ever reads.
unsafe impl Sync for Secret {}

fn page_size() -> usize {
    // SAFETY: sysconf has no preconditions.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as usize } else { 4096 }
}

impl Secret {
    /// Reserve locked memory able to hold at least `capacity` bytes.
    pub fn with_capacity(capacity: usize) -> Result<Self> {
        let page = page_size();
        let cap =
            capacity.max(1).checked_next_multiple_of(page).ok_or_else(|| Error::Other("secret too large".into()))?;
        let len = NonZeroUsize::new(cap).expect("cap is at least one page");
        // SAFETY: fresh anonymous private mapping, no address hint, not shared with anything.
        let addr =
            unsafe { mmap_anonymous(None, len, ProtFlags::PROT_READ | ProtFlags::PROT_WRITE, MapFlags::MAP_PRIVATE) }
                .map_err(|e| Error::Other(format!("cannot map secret memory: {e}")))?;
        // SAFETY: `addr..addr+cap` is the mapping created above.
        if let Err(e) = unsafe { mlock(addr, cap) } {
            // SAFETY: same mapping, nothing references it yet.
            let _ = unsafe { munmap(addr, cap) };
            return Err(Error::Other(format!(
                "cannot mlock secret memory ({e}); raise RLIMIT_MEMLOCK (`ulimit -l`) — refusing to keep tokens in swappable memory"
            )));
        }
        // Best effort: both are hardening on top of the lock, absent only on ancient kernels.
        // SAFETY: same mapping.
        unsafe {
            let _ = madvise(addr, cap, MmapAdvise::MADV_DONTDUMP);
            let _ = madvise(addr, cap, MmapAdvise::MADV_WIPEONFORK);
        }
        Ok(Secret { ptr: addr.cast(), len: 0, cap })
    }

    pub fn from_slice(data: &[u8]) -> Result<Self> {
        let mut s = Self::with_capacity(data.len())?;
        s.push(data)?;
        Ok(s)
    }

    /// Append bytes. Fails instead of growing: growth would mean copying the secret.
    pub fn push(&mut self, data: &[u8]) -> Result<()> {
        let end = self.len.checked_add(data.len()).filter(|&e| e <= self.cap);
        let Some(end) = end else {
            return Err(Error::Other("secret buffer capacity exceeded".into()));
        };
        // SAFETY: `len..end` lies inside our mapping; `data` cannot alias it (we hold `&mut self`).
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr.as_ptr().add(self.len), data.len()) };
        self.len = end;
        Ok(())
    }

    pub fn expose(&self) -> &[u8] {
        // SAFETY: `0..len` is initialised and inside the mapping.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub fn expose_str(&self) -> Result<&str> {
        std::str::from_utf8(self.expose()).map_err(|_| Error::Format("secret is not valid UTF-8".into()))
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn try_clone(&self) -> Result<Self> {
        Self::from_slice(self.expose())
    }

    /// Erase the contents now, without waiting for drop.
    pub fn wipe(&mut self) {
        // SAFETY: the whole mapping is ours and writable. explicit_bzero is the libc
        // primitive the compiler is not allowed to optimise away.
        unsafe { libc::explicit_bzero(self.ptr.as_ptr().cast(), self.cap) };
        self.len = 0;
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.wipe();
        // SAFETY: unmapping our own mapping exactly once (this also drops the lock).
        let _ = unsafe { munmap(self.ptr.cast(), self.cap) };
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret(<redacted, {} bytes>)", self.len)
    }
}

/// Deserialises a JSON string straight into locked memory. serde_json hands us a borrow of
/// the input for escape-free strings, so no intermediate `String` is created.
impl<'de> serde::Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Secret;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Secret, E> {
                Secret::from_slice(v.as_bytes()).map_err(E::custom)
            }
        }
        d.deserialize_str(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_wipes() {
        let mut s = Secret::from_slice(b"hunter2").unwrap();
        assert_eq!(s.expose(), b"hunter2");
        assert_eq!(s.expose_str().unwrap(), "hunter2");
        let raw = s.ptr.as_ptr();
        s.wipe();
        assert!(s.is_empty());
        // SAFETY: mapping is still alive until drop; reading our own wiped bytes.
        let after = unsafe { std::slice::from_raw_parts(raw, 7) };
        assert_eq!(after, &[0u8; 7]);
    }

    #[test]
    fn never_grows() {
        let mut s = Secret::with_capacity(8).unwrap();
        let cap = s.cap;
        assert!(s.push(&vec![1u8; cap]).is_ok());
        assert!(s.push(&[1]).is_err());
    }

    #[test]
    fn debug_is_redacted() {
        let s = Secret::from_slice(b"top-secret-value").unwrap();
        assert!(!format!("{s:?}").contains("top-secret"));
    }

    #[test]
    fn deserialises_from_json_including_escapes() {
        #[derive(serde::Deserialize)]
        struct T {
            token: Secret,
        }
        let t: T = serde_json::from_str(r#"{"token":"a\"bA"}"#).unwrap();
        assert_eq!(t.token.expose(), b"a\"bA");
    }

    #[test]
    fn pages_are_locked() {
        let _s = Secret::from_slice(b"x").unwrap();
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let locked_kb: u64 = status
            .lines()
            .find_map(|l| l.strip_prefix("VmLck:"))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .unwrap();
        assert!(locked_kb >= 4, "expected at least one locked page, VmLck = {locked_kb} kB");
    }
}
