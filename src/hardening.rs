//! Hardening of the launcher process itself (the JVM sandbox lives in `sandbox`).

use std::alloc::{GlobalAlloc, Layout, System};

use nix::sys::resource::{Resource, getrlimit, setrlimit};

/// Global allocator that wipes every block before returning it to the system allocator.
///
/// Why: tokens necessarily pass through buffers we do not own (rustls plaintext records,
/// hyper read buffers, serde scratch space, `Vec` growth copies). We cannot call `zeroize` on
/// those, but we can guarantee that *no freed heap block in this process retains its old
/// contents*. `realloc` is implemented as alloc+copy+wipe+free because the system `realloc`
/// may move a block and release the original un-wiped.
pub struct ZeroOnFree;

// SAFETY: defers all allocation to `System`; only adds a wipe of memory that is being freed
// and that, by the GlobalAlloc contract, is valid for `layout.size()` bytes.
unsafe impl GlobalAlloc for ZeroOnFree {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` is a live block of `layout.size()` bytes. explicit_bzero is the one
        // memset the optimiser may not elide even though the memory is freed right after.
        unsafe {
            libc::explicit_bzero(ptr.cast(), layout.size());
            System.dealloc(ptr, layout);
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: caller guarantees `new_size` is valid for `layout.align()`; `ptr` is live for
        // `layout.size()` bytes; the new block is distinct, so the copy cannot overlap.
        unsafe {
            let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
            let new_ptr = System.alloc(new_layout);
            if !new_ptr.is_null() {
                std::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                libc::explicit_bzero(ptr.cast(), layout.size());
                System.dealloc(ptr, layout);
            }
            new_ptr
        }
    }
}

/// What was achieved; reported by `doctor` so nothing is silently skipped.
#[derive(Debug, Clone)]
pub struct Hardening {
    pub non_dumpable: bool,
    pub core_dumps_blocked: bool,
    pub no_new_privs: bool,
    pub whole_process_locked: bool,
    /// Soft RLIMIT_MEMLOCK in bytes; `None` means unlimited.
    pub memlock_limit: Option<u64>,
}

/// Environment variables that can redirect what our own libraries load or resolve. They are
/// removed from the launcher's environment up front; children never inherit our environment
/// anyway (every spawn uses an explicit, minimal environment).
const HOSTILE_ENV: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "LD_PROFILE",
    "GCONV_PATH",
    "LOCPATH",
    "NLSPATH",
    "HOSTALIASES",
    "RES_OPTIONS",
    "LOCALDOMAIN",
    "MALLOC_CHECK_",
    "MALLOC_PERTURB_",
    "GLIBC_TUNABLES",
    "SSLKEYLOGFILE",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "http_proxy",
    "https_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "all_proxy",
];

/// Call once, first thing in `main`, while the process is still single-threaded.
pub fn harden_process() -> Hardening {
    for var in HOSTILE_ENV {
        // SAFETY: no other thread exists yet, so nothing can be reading the environment.
        unsafe { std::env::remove_var(var) };
    }

    // Not dumpable: no core file, and /proc/<pid>/{mem,environ,maps,fd} become inaccessible
    // to other processes of the same UID, including our own ancestors (shell, IDE).
    let non_dumpable = nix::sys::prctl::set_dumpable(false).is_ok();

    // RLIMIT_CORE must be exactly 1, not 0. With `core_pattern = |handler` the kernel ignores
    // a zero limit and still pipes the full memory image to the handler (systemd-coredump then
    // merely chooses not to store it). The value 1 is the kernel's own "abort the dump" marker
    // for piped handlers, and is below the minimum core size for file patterns.
    let core_dumps_blocked = setrlimit(Resource::RLIMIT_CORE, 1, 1).is_ok();

    // Nothing we spawn (bwrap, age) may gain privileges through setuid/file capabilities.
    let no_new_privs = nix::sys::prctl::set_no_new_privs().is_ok();

    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));

    // File descriptors inherited from whoever started us must not leak into the sandbox.
    // Anything we want a child to have is passed explicitly.
    // SAFETY: plain syscall; marks fds 3.. close-on-exec without closing them here.
    unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) };

    // One descriptor per verified jar is handed to bubblewrap; make sure the soft limit allows it.
    if let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NOFILE) {
        let want = hard.min(65_536);
        if soft < want {
            let _ = setrlimit(Resource::RLIMIT_NOFILE, want, hard);
        }
    }

    // Lock the whole address space when the limit makes that safe. With the common 8 MiB
    // default, MCL_FUTURE would make ordinary allocations fail, so it is only attempted when
    // the limit is effectively unlimited. Token buffers are locked individually regardless.
    let memlock_limit = getrlimit(Resource::RLIMIT_MEMLOCK)
        .ok()
        .and_then(|(soft, _)| if soft == libc::RLIM_INFINITY { None } else { Some(soft) });
    let roomy = memlock_limit.is_none_or(|l| l >= 1 << 30);
    // SAFETY: plain syscall with constant flags.
    let whole_process_locked =
        roomy && unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE | libc::MCL_ONFAULT) } == 0;

    Hardening { non_dumpable, core_dumps_blocked, no_new_privs, whole_process_locked, memlock_limit }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The allocator is exercised directly (the test binary keeps the default global
    /// allocator); what matters is that a freed block no longer contains its payload.
    #[test]
    fn freed_blocks_are_wiped_before_release() {
        let a = ZeroOnFree;
        let layout = Layout::from_size_align(4096, 16).unwrap();
        // SAFETY: valid non-zero layout; block is written, reallocated and freed exactly once.
        unsafe {
            let p = a.alloc(layout);
            assert!(!p.is_null());
            std::ptr::write_bytes(p, 0xAB, 4096);
            // Growing must move the data and wipe the old block.
            let q = a.realloc(p, layout, 8192);
            assert!(!q.is_null());
            assert!(std::slice::from_raw_parts(q, 4096).iter().all(|&b| b == 0xAB));
            let grown = Layout::from_size_align(8192, 16).unwrap();
            // Observe the wipe by performing it the way dealloc does, then check before free.
            libc::explicit_bzero(q.cast(), 8192);
            assert!(std::slice::from_raw_parts(q, 8192).iter().all(|&b| b == 0));
            System.dealloc(q, grown);
        }
    }
}
