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
//! Where the kernel offers it, the mapping is `memfd_secret` memory instead of anonymous
//! memory: pages that are additionally removed from the kernel's own direct map. That
//! closes the read paths that go *through the kernel* — `/proc/kcore`, `/dev/mem`,
//! `process_vm_readv`, `ptrace(PEEKDATA)`, a kernel bug that reads arbitrary direct-map
//! addresses — and the pages can never be swapped or dumped. It is not magic, and the
//! limits are worth stating: the bytes are still ordinary cleartext RAM, so a kernel
//! module or anyone who can run code in the kernel can map them again, and so can a
//! cold-boot or DMA attack; and code running *inside this process* reads them like any
//! other memory. `secretmem` can be switched off by the administrator
//! (`secretmem.enable=0`) and is unavailable while hibernation is possible on some
//! kernels; then the anonymous, locked mapping described above is used, as before.
//! [`backing`] says which one this process got.
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

/// What kind of memory secrets live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backing {
    /// `memfd_secret`: locked, and absent from the kernel's direct map.
    SecretMem,
    /// Anonymous memory, `mlock`ed, excluded from dumps, wiped on fork.
    Locked,
}

/// Map `cap` bytes of `memfd_secret` memory, or `None` if this kernel will not.
fn map_secretmem(cap: usize) -> Option<NonNull<u8>> {
    // SAFETY: plain system call; flags 0 (the descriptor is close-on-exec'd below).
    let fd = unsafe { libc::syscall(libc::SYS_memfd_secret, libc::O_CLOEXEC as libc::c_uint) };
    if fd < 0 {
        return None;
    }
    // SAFETY: a descriptor just returned by the kernel, owned by nothing else.
    let fd = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd as i32) };
    let raw = std::os::fd::AsRawFd::as_raw_fd(&fd);
    // SAFETY: sizing our own fresh descriptor.
    if unsafe { libc::ftruncate(raw, cap as libc::off_t) } != 0 {
        return None;
    }
    // secretmem only supports shared mappings. Nobody else has the descriptor, and it is
    // closed as soon as the mapping exists, so "shared" is shared with no one.
    // SAFETY: a new mapping of our own descriptor, no address hint.
    let addr =
        unsafe { libc::mmap(std::ptr::null_mut(), cap, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, raw, 0) };
    if addr == libc::MAP_FAILED {
        return None;
    }
    // A shared mapping would be shared with a forked child (and "wipe on fork" exists only
    // for private memory): the child must not get the mapping at all. Without that
    // guarantee this kind of memory is not used.
    // SAFETY: the mapping created above.
    if unsafe { libc::madvise(addr, cap, libc::MADV_DONTFORK) } != 0 {
        // SAFETY: unmapping our own, unused mapping.
        unsafe { libc::munmap(addr, cap) };
        return None;
    }
    // SAFETY: same mapping; best effort (secretmem is never dumped anyway).
    unsafe { libc::madvise(addr, cap, libc::MADV_DONTDUMP) };
    NonNull::new(addr.cast())
}

/// Which kind of memory a secret created now would get.
pub fn backing() -> Backing {
    static KIND: std::sync::OnceLock<Backing> = std::sync::OnceLock::new();
    *KIND.get_or_init(|| match map_secretmem(page_size()) {
        Some(ptr) => {
            // SAFETY: unmapping the probe mapping, which nothing references.
            unsafe { libc::munmap(ptr.as_ptr().cast(), page_size()) };
            Backing::SecretMem
        }
        None => Backing::Locked,
    })
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
        if backing() == Backing::SecretMem
            && let Some(ptr) = map_secretmem(cap)
        {
            return Ok(Secret { ptr, len: 0, cap });
        }
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

    /// Whatever the backing: a forked child must not be able to read the secret, and the
    /// parent's copy must be unaffected.
    #[test]
    fn a_forked_child_does_not_see_the_secret() {
        let s = Secret::from_slice(b"fork-must-not-copy-this").unwrap();
        let ptr = s.ptr.as_ptr();
        // SAFETY: the child only reads memory (or faults) and exits without running any
        // Rust runtime code; the parent waits for it.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            // Anonymous backing: the page reads as zeros (wipe on fork). secretmem backing:
            // the mapping does not exist and the read faults (SIGSEGV).
            // SAFETY: deliberate read of a possibly unmapped address in a throw-away child.
            let first = unsafe { std::ptr::read_volatile(ptr) };
            // SAFETY: immediate exit, no destructors.
            unsafe { libc::_exit(if first == 0 { 0 } else { 42 }) };
        }
        let mut status = 0;
        // SAFETY: waiting for our own child.
        unsafe { libc::waitpid(pid, &mut status, 0) };
        let leaked = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 42;
        assert!(!leaked, "the child read the secret");
        match backing() {
            Backing::SecretMem => {
                assert!(libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGSEGV, "status {status}")
            }
            Backing::Locked => assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0, "status {status}"),
        }
        assert_eq!(s.expose(), b"fork-must-not-copy-this");
    }

    /// With secretmem the kernel refuses to read the pages on another process's behalf —
    /// here on our own behalf, through the same interface a debugger would use.
    #[test]
    fn secretmem_pages_cannot_be_read_through_the_kernel() {
        if backing() != Backing::SecretMem {
            eprintln!("skipped: memfd_secret is not available here (secretmem.enable=0 or an old kernel)");
            return;
        }
        let s = Secret::from_slice(b"not-through-the-kernel").unwrap();
        let mut out = [0u8; 22];
        let local = libc::iovec { iov_base: out.as_mut_ptr().cast(), iov_len: out.len() };
        let remote = libc::iovec { iov_base: s.ptr.as_ptr().cast(), iov_len: out.len() };
        // SAFETY: both iovecs describe memory of this process.
        let n = unsafe { libc::process_vm_readv(libc::getpid(), &local, 1, &remote, 1, 0) };
        assert_eq!(n, -1, "process_vm_readv returned the secret: {:?}", &out[..]);
        assert_eq!(out, [0u8; 22]);
        // An ordinary buffer can be read this way: the refusal is about the secret pages.
        let plain = *b"an-ordinary-buffer-22b";
        let remote = libc::iovec { iov_base: plain.as_ptr() as *mut libc::c_void, iov_len: plain.len() };
        // SAFETY: as above.
        assert_eq!(unsafe { libc::process_vm_readv(libc::getpid(), &local, 1, &remote, 1, 0) }, 22);
        // And the ordinary uses still work: a system call that copies from the pages.
        let (r, w) = nix::unistd::pipe().unwrap();
        assert_eq!(nix::unistd::write(&w, s.expose()).unwrap(), 22);
        let mut back = [0u8; 22];
        assert_eq!(nix::unistd::read(&r, &mut back).unwrap(), 22);
        assert_eq!(&back, b"not-through-the-kernel");
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
        // secretmem pages are unswappable by construction and are not counted as VmLck.
        if backing() == Backing::Locked {
            assert!(locked_kb >= 4, "expected at least one locked page, VmLck = {locked_kb} kB");
        }
    }
}
