//! fs-verity: kernel-enforced immutability for verified files.
//!
//! Hash verification alone leaves a window: another process running as the same user could
//! rewrite a jar *in place* between the moment it is hashed and the moment the sandbox maps
//! it. Binding by descriptor does not help against in-place writes, because the descriptor
//! refers to the same inode.
//!
//! With fs-verity enabled on a file the kernel refuses every write, append and truncate on
//! it, for everyone including its owner, and verifies each page against a Merkle tree on
//! read. The launcher therefore enables verity *before* it hashes: from that point the bytes
//! cannot change, so the digest it then computes is the digest of what the JVM will read.
//!
//! Available where the filesystem supports it (btrfs out of the box; ext4 and f2fs with the
//! `verity` feature). Elsewhere this is a no-op and the residual in the threat model stands.
//! A verity file can still be unlinked or renamed, so `--repair` and normal updates work.

use std::{
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
};

const FS_VERITY_FL: libc::c_long = 0x0010_0000;
const FS_VERITY_HASH_ALG_SHA256: u32 = 1;

/// `struct fsverity_enable_arg` from `<linux/fsverity.h>`.
#[repr(C)]
#[derive(Default)]
struct EnableArg {
    version: u32,
    hash_algorithm: u32,
    block_size: u32,
    salt_size: u32,
    salt_ptr: u64,
    sig_size: u32,
    reserved1: u32,
    sig_ptr: u64,
    reserved2: [u64; 11],
}

// _IOW('f', 133, struct fsverity_enable_arg) and _IOR('f', 1, long)
const FS_IOC_ENABLE_VERITY: libc::c_ulong = 0x4080_6685;
const FS_IOC_GETFLAGS: libc::c_ulong = 0x8008_6601;

const _: () = assert!(std::mem::size_of::<EnableArg>() == 128);

/// Is the file already protected by fs-verity?
pub fn is_enabled(file: &File) -> bool {
    let mut flags: libc::c_long = 0;
    // SAFETY: FS_IOC_GETFLAGS writes one long through the pointer.
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_GETFLAGS as _, &mut flags as *mut libc::c_long) };
    rc == 0 && flags & FS_VERITY_FL != 0
}

/// Make `file` immutable through fs-verity if the filesystem can do it.
///
/// Returns `Ok(true)` when the file is verity-protected afterwards, `Ok(false)` when that is
/// not possible here (unsupported filesystem, not our file, or someone holds it open for
/// writing). `file` must be open read-only.
pub fn enable(file: &File) -> io::Result<bool> {
    if is_enabled(file) {
        return Ok(true);
    }
    let meta = file.metadata()?;
    if !meta.file_type().is_file() || meta.uid() != nix::unistd::geteuid().as_raw() {
        return Ok(false);
    }
    // The ioctl demands write permission on the inode (not a writable descriptor). Store
    // files are kept 0444, so lend ourselves the bit for the duration of the call.
    let mode = meta.permissions().mode() & 0o7777;
    let lend = mode & 0o200 == 0;
    if lend {
        file.set_permissions(std::fs::Permissions::from_mode(mode | 0o200))?;
    }
    let arg =
        EnableArg { version: 1, hash_algorithm: FS_VERITY_HASH_ALG_SHA256, block_size: 4096, ..Default::default() };
    // SAFETY: the kernel reads a fully initialised fsverity_enable_arg of the size encoded
    // in the request number.
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_ENABLE_VERITY as _, &arg as *const EnableArg) };
    let err = io::Error::last_os_error();
    if lend {
        let _ = file.set_permissions(std::fs::Permissions::from_mode(mode));
    }
    if rc == 0 {
        return Ok(true);
    }
    match err.raw_os_error() {
        Some(libc::EEXIST) => Ok(true),
        // Filesystem without verity, verity feature not enabled, or a writer exists.
        Some(
            libc::EOPNOTSUPP | libc::ENOTTY | libc::EINVAL | libc::ETXTBSY | libc::EPERM | libc::EACCES | libc::EROFS,
        ) => Ok(false),
        _ => Err(err),
    }
}

/// Does the filesystem holding `dir` support fs-verity for us? (Used by `doctor`.)
pub fn supported_in(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".verity-probe-{}", std::process::id()));
    let result = (|| -> io::Result<bool> {
        std::fs::write(&probe, [0u8; 4096])?;
        enable(&File::open(&probe)?)
    })();
    let _ = std::fs::remove_file(&probe);
    result.unwrap_or(false)
}
