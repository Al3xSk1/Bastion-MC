//! Deleting and copying directory trees that hostile code has written.
//!
//! The per-launch game root, the instance's `saves/` and the nested compositor's directory
//! are filled by the sandbox. The launcher deletes the first and the last and, for a burner
//! session, copies the second — on the host, with the user's privileges. Two things follow.
//!
//! **The game shapes the tree.** It may `chmod` (the system-call allowlist has it), so it
//! can leave a directory without any permission bits: a plain recursive delete stops there
//! with `EACCES`, and "everything the game wrote is deleted" becomes untrue without anyone
//! noticing. It can nest directories deeper than a process may hold descriptors or stack
//! frames, and create more entries than can be removed in the time somebody is prepared to
//! wait. So [`remove`] makes each directory enterable before it enters it, keeps **one**
//! directory open however deep the tree is (it climbs back with `..` and checks that it
//! arrived where it came from), takes a deadline, and returns an error for everything it
//! could not do — callers report that, they do not assume.
//!
//! **Paths are not stable.** Between looking at a name and using it, anything able to write
//! the directory can put something else there. Both functions therefore resolve every name
//! relative to an open directory descriptor, never follow a symbolic link, and check what
//! they opened (`O_NOFOLLOW`, `O_DIRECTORY`, `fstat` on the descriptor). [`copy`] is bounded
//! in entries, depth and bytes, and compares a file's size with what is left of the byte
//! budget *before* it copies, so that a sparse file of a terabyte ends the copy at once
//! instead of filling the disk.
//!
//! Not guaranteed: atomicity. A removal that fails or runs out of time leaves part of the
//! tree behind (and says so). And nothing here defends against another program running as
//! the user moving directories around during a walk beyond refusing to continue when the
//! way back up is not the way that was taken down.

use std::{
    ffi::{CStr, OsStr, OsString},
    io,
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
    time::Instant,
};

use nix::{
    dir::Dir,
    fcntl::{AtFlags, OFlag, openat},
    sys::stat::{FchmodatFlags, FileStat, Mode, SFlag, fchmodat, fstat, fstatat},
    unistd::{UnlinkatFlags, unlinkat},
};

use crate::error::{Context, Error, Result};

const DIRECTORY: OFlag = OFlag::O_RDONLY.union(OFlag::O_DIRECTORY).union(OFlag::O_NOFOLLOW).union(OFlag::O_CLOEXEC);

fn is_dir(stat: &FileStat) -> bool {
    SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR
}

fn is_regular(stat: &FileStat) -> bool {
    SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG
}

/// An error that names the entry it happened at. (Full paths are not kept: a tree may be
/// deeper than any path can be long.)
fn at(what: &str, name: &OsStr, depth: usize, e: impl Into<io::Error>) -> io::Error {
    let e: io::Error = e.into();
    let shown = crate::util::sanitize_line(name.as_bytes(), 80);
    io::Error::new(e.kind(), format!("{what} {shown:?} ({depth} level(s) down): {e}"))
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "the time allowed for deleting ran out; part of the tree is still there")
}

/// Open the directory `name` in `parent` for reading, whatever permission bits it was left
/// with. Never follows a symbolic link.
fn enter(parent: BorrowedFd<'_>, name: &OsStr, depth: usize) -> io::Result<Dir> {
    // The tree belongs to this user (the sandbox maps its one identity to it), so the mode
    // can always be set; with AT_SYMLINK_NOFOLLOW a link is refused, not followed. A failure
    // here is left to the open below to report.
    let _ = fchmodat(parent, name, Mode::S_IRWXU, FchmodatFlags::NoFollowSymlink);
    let fd = openat(parent, name, DIRECTORY, Mode::empty()).map_err(|e| at("opening directory", name, depth, e))?;
    Dir::from_fd(fd).map_err(|e| at("reading directory", name, depth, e))
}

fn identity(fd: BorrowedFd<'_>) -> io::Result<(u64, u64)> {
    let stat = fstat(fd)?;
    Ok((stat.st_dev, stat.st_ino))
}

/// Delete the directory tree `root`. `root` itself must be a directory (a symbolic link is
/// refused, its target untouched). With a `deadline`, the work stops when it has passed and
/// the result is an error of kind `TimedOut`.
///
/// On success nothing of the tree is left. On any error, part of it may be.
pub fn remove(root: &Path, deadline: Option<Instant>) -> io::Result<()> {
    let name = root
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a path to a directory entry"))?
        .to_os_string();
    let parent = root.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let top = nix::fcntl::open(parent, OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC, Mode::empty())
        .map_err(io::Error::from)?;
    let top = Dir::from_fd(top).map_err(io::Error::from)?;
    match fstatat(top.as_fd(), name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) if is_dir(&stat) => {}
        Ok(_) => return Err(io::Error::new(io::ErrorKind::NotADirectory, "not a directory; not removing it")),
        Err(e) => return Err(e.into()),
    }

    // `current` is the one directory that is open. `above` holds, for each directory between
    // `top` and it, the identity of that directory's parent and the name it has there: enough
    // to climb back and to notice if `..` leads somewhere else.
    let mut above: Vec<((u64, u64), OsString)> = vec![(identity(top.as_fd())?, name.clone())];
    let mut current = enter(top.as_fd(), &name, 0)?;
    drop(top);
    let mut steps = 0u32;

    loop {
        // Remove what can be removed here; stop at the first sub-directory.
        let mut descend: Option<OsString> = None;
        {
            // A second descriptor for the same directory: the iterator borrows the first, and
            // the `*at` calls below need one too.
            let here: OwnedFd = current.as_fd().try_clone_to_owned()?;
            let fd = here.as_fd();
            for entry in current.iter() {
                let entry = entry.map_err(io::Error::from)?;
                let entry_name: &CStr = entry.file_name();
                if matches!(entry_name.to_bytes(), b"." | b"..") {
                    continue;
                }
                steps = steps.wrapping_add(1);
                if steps.is_multiple_of(64) && deadline.is_some_and(|d| Instant::now() >= d) {
                    return Err(timed_out());
                }
                let entry_os = OsStr::from_bytes(entry_name.to_bytes());
                // Decided by asking the kernel about the entry itself, not by the directory's
                // type hint, and without following a link.
                let stat = fstatat(fd, entry_name, AtFlags::AT_SYMLINK_NOFOLLOW)
                    .map_err(|e| at("examining", entry_os, above.len(), e))?;
                if is_dir(&stat) {
                    descend = Some(entry_os.to_os_string());
                    break;
                }
                unlinkat(fd, entry_name, UnlinkatFlags::NoRemoveDir)
                    .map_err(|e| at("deleting", entry_os, above.len(), e))?;
            }
        }
        match descend {
            Some(child) => {
                let here = identity(current.as_fd())?;
                let next = enter(current.as_fd(), &child, above.len())?;
                above.push((here, child));
                current = next; // the directory above is closed here
            }
            None => {
                // Empty: go up and remove it from there.
                let (expected, name) = above.pop().expect("the walk starts one level down");
                let up = openat(
                    current.as_fd(),
                    c"..",
                    OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|e| at("leaving directory", &name, above.len(), e))?;
                if identity(up.as_fd())? != expected {
                    return Err(io::Error::other(
                        "the tree was rearranged while it was being deleted; stopped without touching anything else",
                    ));
                }
                unlinkat(up.as_fd(), name.as_os_str(), UnlinkatFlags::RemoveDir)
                    .map_err(|e| at("deleting directory", &name, above.len(), e))?;
                if above.is_empty() {
                    return Ok(());
                }
                current = Dir::from_fd(up).map_err(io::Error::from)?;
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    return Err(timed_out());
                }
            }
        }
    }
}

/// Bounds for [`copy`].
#[derive(Debug, Clone, Copy)]
pub struct CopyLimits {
    /// Files and directories, together.
    pub max_entries: u64,
    /// Sum of the files' sizes as the file system reports them (holes count).
    pub max_bytes: u64,
    pub max_depth: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Copied {
    pub files: u64,
    pub bytes: u64,
    entries: u64,
}

fn copy_dir(src: Dir, dst: &Path, depth: usize, limits: &CopyLimits, done: &mut Copied) -> Result<()> {
    if depth > limits.max_depth {
        return Err(Error::Policy(format!(
            "the directory tree is nested more than {} levels deep; not copying it",
            limits.max_depth
        )));
    }
    crate::util::create_private_dir_all(dst)?;
    let mut src = src;
    let names: Vec<OsString> = {
        let mut names = Vec::new();
        for entry in src.iter() {
            let entry = entry.ctx(|| format!("reading the directory copied to {}", dst.display()))?;
            let name = entry.file_name().to_bytes();
            if matches!(name, b"." | b"..") {
                continue;
            }
            done.entries += 1;
            if done.entries > limits.max_entries {
                return Err(Error::Policy(format!(
                    "more than {} files and directories; not copying them",
                    limits.max_entries
                )));
            }
            names.push(OsStr::from_bytes(name).to_os_string());
        }
        names
    };
    // "It is something else now" (a link or another kind of file took the name between the
    // listing and the open): skipped like any other entry that is not a file or directory.
    let replaced = |e: &io::Error| {
        matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR | libc::ENOENT))
            || e.kind() == io::ErrorKind::InvalidInput
    };
    for name in names {
        // Vanished since it was listed.
        let Ok(stat) = fstatat(src.as_fd(), name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) else { continue };
        let to = dst.join(&name);
        if is_dir(&stat) {
            // Opened as a directory, without following a link that replaced it meanwhile.
            let fd = match openat(src.as_fd(), name.as_os_str(), DIRECTORY, Mode::empty()).map_err(io::Error::from) {
                Ok(fd) => fd,
                Err(e) if replaced(&e) => continue,
                Err(e) => return Err(Error::io(format!("opening the directory copied to {}", to.display()), e)),
            };
            let child = Dir::from_fd(fd).ctx(|| format!("reading the directory copied to {}", to.display()))?;
            copy_dir(child, &to, depth + 1, limits, done)?;
        } else if is_regular(&stat) {
            let mut input = match crate::util::openat_regular_nofollow(src.as_fd(), &name) {
                Ok(input) => input,
                Err(e) if replaced(&e) => continue,
                Err(e) => return Err(Error::io(format!("opening the file copied to {}", to.display()), e)),
            };
            copy_file(&mut input, &to, limits, done)?;
        }
        // Anything else — symbolic links, sockets, FIFOs, devices — is not copied.
    }
    Ok(())
}

fn copy_file(input: &mut std::fs::File, to: &Path, limits: &CopyLimits, done: &mut Copied) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let size = input.metadata().ctx(|| format!("examining the file copied to {}", to.display()))?.len();
    let left = limits.max_bytes.saturating_sub(done.bytes);
    // Asked before a byte is written: a sparse file claims its size without occupying it,
    // and copying it would write all of it out.
    if size > left {
        return Err(Error::Policy(format!(
            "{} is {} MiB, and only {} MiB may still be copied (the limit is {} MiB in all)",
            crate::util::sanitize_line(to.file_name().unwrap_or_default().as_bytes(), 80),
            size >> 20,
            left >> 20,
            limits.max_bytes >> 20
        )));
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(to)
        .ctx(|| format!("creating {}", to.display()))?;
    // One byte more than is allowed, to see a file that grew after it was measured.
    let copied = io::copy(&mut io::Read::take(input, left.saturating_add(1)), &mut output)
        .ctx(|| format!("copying to {}", to.display()))?;
    if copied > left {
        return Err(Error::Policy("a file grew past the copy limit while it was being copied".into()));
    }
    done.bytes += copied;
    done.files += 1;
    Ok(())
}

/// Copy the regular files and directories under `src` to `dst` (created, private to the
/// user). Symbolic links, sockets, FIFOs and devices are skipped, as is anything that
/// changes its kind between being listed and being opened. Exceeding a limit is an error,
/// and what was copied until then stays in `dst` for the caller to delete.
pub fn copy(src: &Path, dst: &Path, limits: &CopyLimits) -> Result<Copied> {
    let fd = nix::fcntl::open(src, DIRECTORY, Mode::empty()).ctx(|| format!("opening {}", src.display()))?;
    let dir = Dir::from_fd(fd).ctx(|| format!("reading {}", src.display()))?;
    let mut done = Copied::default();
    copy_dir(dir, dst, 0, limits, &mut done)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bastion-tree-{tag}-{}",
            crate::util::hex_encode(&crate::util::random_bytes::<6>().unwrap())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_tree_the_game_made_unreadable_is_removed_all_the_same() {
        let base = scratch("modes");
        let root = base.join("game-root");
        std::fs::create_dir_all(root.join("saves/world/region")).unwrap();
        std::fs::create_dir_all(root.join("locked/inner/deeper")).unwrap();
        std::fs::create_dir_all(root.join("read-only/inner")).unwrap();
        std::fs::write(root.join("saves/world/region/r.0.0.mca"), b"chunk").unwrap();
        std::fs::write(root.join("locked/inner/deeper/copy-of-the-world"), b"kept from deletion").unwrap();
        std::fs::write(root.join("read-only/inner/file"), b"x").unwrap();
        std::fs::write(root.join("unreadable-file"), b"x").unwrap();
        chmod(&root.join("unreadable-file"), 0o000);
        // What a game can do with chmod(2): no permission bits at all, or no write bit.
        chmod(&root.join("locked/inner/deeper"), 0o000);
        chmod(&root.join("locked/inner"), 0o000);
        chmod(&root.join("locked"), 0o000);
        chmod(&root.join("read-only/inner"), 0o500);
        chmod(&root.join("read-only"), 0o500);

        // The reason this module exists: the standard recursive delete gives up here.
        let plain = std::fs::remove_dir_all(&root).unwrap_err();
        assert_eq!(plain.kind(), io::ErrorKind::PermissionDenied);
        assert!(root.join("locked").exists(), "the tree survived the plain delete");

        remove(&root, None).unwrap();
        assert!(!root.exists());
        assert!(base.exists(), "nothing above the tree is touched");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn links_are_removed_as_links_and_a_link_is_never_a_tree() {
        let base = scratch("links");
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(outside.join("precious")).unwrap();
        std::fs::write(outside.join("precious/file"), b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("sub/to-outside")).unwrap();
        std::os::unix::fs::symlink(outside.join("precious/file"), root.join("to-file")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/dangling", root.join("dangling")).unwrap();
        nix::unistd::mkfifo(&root.join("fifo"), Mode::S_IRWXU).unwrap();

        remove(&root, None).unwrap();
        assert!(!root.exists());
        assert_eq!(std::fs::read(outside.join("precious/file")).unwrap(), b"keep", "link targets are untouched");

        // The root itself being a link: refused, target untouched.
        let link = base.join("link-to-outside");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(remove(&link, None).is_err());
        assert!(outside.join("precious/file").exists() && link.is_symlink());
        // A file, a missing path.
        assert!(remove(&outside.join("precious/file"), None).is_err());
        assert_eq!(remove(&base.join("absent"), None).unwrap_err().kind(), io::ErrorKind::NotFound);
        std::fs::remove_dir_all(base).unwrap();
    }

    /// Deeper than a process may have descriptors open (and than a path may be long): the
    /// walk holds one directory at a time.
    #[test]
    fn a_tree_deeper_than_the_descriptor_limit_is_removed() {
        let base = scratch("deep");
        let root = base.join("root");
        std::fs::create_dir(&root).unwrap();
        let (soft, _) = nix::sys::resource::getrlimit(nix::sys::resource::Resource::RLIMIT_NOFILE).unwrap();
        let depth = (soft as usize + 500).min(6000);
        let mut fd: OwnedFd = nix::fcntl::open(&root, DIRECTORY, Mode::empty()).unwrap();
        for level in 0..depth {
            nix::sys::stat::mkdirat(fd.as_fd(), "d", Mode::S_IRWXU).unwrap();
            if level % 97 == 0 {
                let file = openat(fd.as_fd(), "f", OFlag::O_CREAT | OFlag::O_WRONLY | OFlag::O_CLOEXEC, Mode::S_IRUSR)
                    .unwrap();
                drop(file);
            }
            fd = openat(fd.as_fd(), "d", DIRECTORY, Mode::empty()).unwrap();
        }
        drop(fd);
        remove(&root, None).unwrap();
        assert!(!root.exists(), "{depth} levels");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_removal_that_runs_out_of_time_says_so_and_can_be_finished_later() {
        let base = scratch("deadline");
        let root = base.join("root");
        for dir in 0..40 {
            std::fs::create_dir_all(root.join(format!("d{dir}"))).unwrap();
            for file in 0..40 {
                std::fs::write(root.join(format!("d{dir}/f{file}")), b"x").unwrap();
            }
        }
        let already_over = Instant::now() - std::time::Duration::from_secs(1);
        let stopped = remove(&root, Some(already_over)).unwrap_err();
        assert_eq!(stopped.kind(), io::ErrorKind::TimedOut, "{stopped}");
        assert!(root.exists(), "the tree is still (partly) there, and the caller was told");
        remove(&root, Some(Instant::now() + std::time::Duration::from_secs(60))).unwrap();
        assert!(!root.exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    const ROOMY: CopyLimits = CopyLimits { max_entries: 10_000, max_bytes: 1 << 30, max_depth: 64 };

    #[test]
    fn a_copy_takes_files_and_directories_and_nothing_else() {
        let root = scratch("copy");
        let (src, dst) = (root.join("saves"), root.join("copy"));
        std::fs::create_dir_all(src.join("World/region")).unwrap();
        std::fs::write(src.join("World/level.dat"), b"level").unwrap();
        std::fs::write(src.join("World/region/r.0.0.mca"), vec![7u8; 5000]).unwrap();
        std::fs::write(root.join("outside-secret"), b"SECRET").unwrap();
        std::os::unix::fs::symlink(root.join("outside-secret"), src.join("World/link-to-outside")).unwrap();
        std::os::unix::fs::symlink("/etc", src.join("etc-link")).unwrap();
        nix::unistd::mkfifo(&src.join("World/fifo"), Mode::S_IRWXU).unwrap();

        let copied = copy(&src, &dst, &ROOMY).unwrap();
        assert_eq!((copied.files, copied.bytes), (2, 5005));
        assert_eq!(std::fs::read(dst.join("World/level.dat")).unwrap(), b"level");
        assert_eq!(std::fs::read(dst.join("World/region/r.0.0.mca")).unwrap().len(), 5000);
        for skipped in ["World/link-to-outside", "etc-link", "World/fifo"] {
            assert!(std::fs::symlink_metadata(dst.join(skipped)).is_err(), "{skipped} was copied");
        }
        assert_eq!(std::fs::metadata(dst.join("World")).unwrap().permissions().mode() & 0o777, 0o700);
        // The copy is independent of the original.
        std::fs::write(dst.join("World/level.dat"), b"changed by the game").unwrap();
        assert_eq!(std::fs::read(src.join("World/level.dat")).unwrap(), b"level");

        // The source being a link to a directory: not followed.
        std::os::unix::fs::symlink(&src, root.join("saves-link")).unwrap();
        assert!(copy(&root.join("saves-link"), &root.join("copy2"), &ROOMY).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_copy_stops_at_its_limits_before_filling_the_disk() {
        let root = scratch("limits");
        let src = root.join("saves");
        std::fs::create_dir_all(src.join("a/b/c")).unwrap();
        std::fs::write(src.join("a/one"), vec![1u8; 3000]).unwrap();
        std::fs::write(src.join("a/b/two"), vec![2u8; 3000]).unwrap();

        // Bytes.
        let tight = CopyLimits { max_bytes: 5000, ..ROOMY };
        let err = copy(&src, &root.join("bytes"), &tight).unwrap_err();
        assert!(matches!(&err, Error::Policy(m) if m.contains("may still be copied")), "{err}");
        // Entries (files and directories count alike).
        let few = CopyLimits { max_entries: 3, ..ROOMY };
        assert!(matches!(copy(&src, &root.join("entries"), &few), Err(Error::Policy(m)) if m.contains("more than 3")));
        // Depth.
        let shallow = CopyLimits { max_depth: 1, ..ROOMY };
        assert!(matches!(copy(&src, &root.join("depth"), &shallow), Err(Error::Policy(m)) if m.contains("nested")));

        // A sparse file of 1 TiB: its size is what counts, and nothing of it is written.
        let sparse = std::fs::File::create(src.join("a/sparse.mca")).unwrap();
        sparse.set_len(1 << 40).unwrap();
        let started = Instant::now();
        let err = copy(&src, &root.join("sparse"), &ROOMY).unwrap_err();
        assert!(matches!(&err, Error::Policy(m) if m.contains("sparse.mca") && m.contains("MiB")), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "refused without copying");
        assert!(!root.join("sparse/a/sparse.mca").exists(), "not even created");
        std::fs::remove_dir_all(root).unwrap();
    }
}
