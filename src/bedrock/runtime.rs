//! Finding the installed `mcpelauncher-client` (owned by the store package).
//!
//! **Where packages install it.** The client's CMake rules install the program into
//! `<prefix>/bin` (`install(TARGETS mcpelauncher-client … DESTINATION bin)`), its prebuilt
//! Android-side libraries into `<prefix>/share/mcpelauncher/lib/<abi>/` (the manifest installs
//! `mcpelauncher-linux-bin/lib`: `libsqliteX.so` for each ABI) and the gamepad table into
//! `<prefix>/share/mcpelauncher/gamecontrollerdb.txt`. curl is linked statically, and SDL3 is
//! either vendored and static or the system's (the AUR package uses the system's), so there is
//! no private ELF library directory. The AUR package
//! `mcpelauncher-linux` (1.8.7) builds with `-DCMAKE_INSTALL_PREFIX=/usr -DENABLE_DEV_PATHS=OFF`
//! and pacman installs it as root: `/usr/bin/mcpelauncher-client`. That is where [`discover`]
//! looks first, then `/usr/local/bin` (a `make install` with the default prefix), unless
//! `[bedrock] client` names it. bastion never downloads or builds a client.
//!
//! **What is trusted.** The same rule as for the JVM (`java.rs`), applied to every step of the
//! way: each directory from `/` down, every symbolic link on the way (followed one hop at a
//! time, relative to a directory already checked, never by a path string) and the program
//! itself must be owned by root and writable by nobody else, on a file system that is not
//! FUSE (a FUSE file system is served by a program of a user, who decides what it contains and
//! whom it says owns it). The program must be an x86-64 ELF executable, not set-user-ID, not a
//! script. Refused, each with its reason: anything under the user's home (whoever owns it,
//! the user can change it); Flatpak installs, system or per-user, found by location and by the
//! layout of a Flatpak deployment (the client runs on the Flatpak runtime's libraries, from
//! files outside the system's package manager, and Flatpak's own sandbox needs new namespaces,
//! which bastion's sandbox forbids); AppImages and their mounts (they mount themselves through
//! FUSE and bring their own libraries).
//!
//! **Where the client finds a sign-in window.** `XalWebViewQt::findWebView`
//! (`src/xal_webview_qt.cpp`) looks for a program called `mcpelauncher-webview` first in the
//! directories compiled in as `MCPELAUNCHER_WEBVIEW_PATH` (CMake `XAL_WEBVIEW_QT_PATH`; relative
//! entries are taken relative to the client's own directory — the upstream AppImage builds use
//! `.`), then in `$PATH` (`EnvPathUtil::findInPath`; `/bin:/usr/bin` when `PATH` is unset or
//! empty, and an empty entry means the current directory). The compiled-in value of a given
//! binary cannot be read back reliably, so [`Client::webview_paths`] lists every place an
//! installed webview can be: beside the client, in the sandbox's `PATH` directories after the
//! stub's own (`/usr/bin`), every `mcpelauncher-webview` the pacman database lists, and every
//! one in the client's install tree when that is not `/usr` (a manual install); links among
//! them are listed with the file they lead to. The launch covers each that exists with
//! bastion's stub ([`Client::installed_webviews`]).
//!
//! The version comes from the package that owns the program in pacman's database; the
//! client is never run outside the sandbox to ask it.
//!
//! Not guaranteed: that a root-owned client is benign — it is unaudited native code that
//! patches the game, trusted as much as any other program root installed. The checks keep
//! out what a user (or a program running as the user) could have put in its place.

use std::{
    ffi::OsString,
    fs::File,
    os::{
        fd::{AsFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::FileExt},
    },
    path::{Component, Path, PathBuf},
};

use nix::{
    dir::Dir,
    fcntl::{AtFlags, OFlag},
    sys::{
        stat::{FileStat, Mode, SFlag, fstat, fstatat},
        statfs::{FUSE_SUPER_MAGIC, fstatfs},
    },
};

use super::{BedrockConfig, signin::STUB_NAME, store::elf};
use crate::{
    error::{Context, Error, Result},
    util,
};

/// The program's file name.
pub const CLIENT_NAME: &str = "mcpelauncher-client";
/// Where [`discover`] looks when `[bedrock] client` is not set, in this order (relative to
/// `/`).
pub const SEARCHED: &[&str] = &["usr/bin/mcpelauncher-client", "usr/local/bin/mcpelauncher-client"];
/// The directories of the sandbox's `PATH` after the stub's own `/bastion/bin` (relative to
/// `/`). A launch that puts another directory into the client's `PATH` must add it here.
pub const SANDBOX_PATH_DIRS: &[&str] = &["usr/bin"];
/// The data directories the client searches (`PathHelper::findDataFile`: `$XDG_DATA_DIRS`,
/// by default `/usr/local/share:/usr/share`, plus `mcpelauncher/`), relative to `/`; its own
/// `<prefix>/share/mcpelauncher` is added in front.
const SYSTEM_DATA_DIRS: &[&str] = &["usr/local/share/mcpelauncher", "usr/share/mcpelauncher"];
const PACMAN_LOCAL: &str = "var/lib/pacman/local";
const FLATPAK_SYSTEM: &str = "var/lib/flatpak";
const FLATPAK_INSTALLATIONS: &str = "etc/flatpak/installations.d";
const FLATPAK_ID: &str = super::store::UI_FLATPAK_ID;
/// Symbolic links followed on one path, and directory levels (the kernel's own limits are 40
/// links; 64 levels is far beyond any install).
const MAX_HOPS: usize = 40;
const MAX_DEPTH: usize = 64;
/// Entries examined in a data directory and in a manual install's tree.
const MAX_TREE_ENTRIES: usize = 200_000;

/// An installed client, as the launch needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    /// The client's executable (its real path: every link resolved).
    pub program: PathBuf,
    /// Directories with the client's own shared libraries, mounted read-only and executable
    /// beside it (empty when it uses only system libraries). For the packaged client these are
    /// its data directories (`/usr/share/mcpelauncher`: the Android-side `libsqliteX.so` that
    /// its own linker maps, and `gamecontrollerdb.txt`), each checked like the program.
    pub lib_dirs: Vec<PathBuf>,
    /// Every path where an installed `mcpelauncher-webview` (the client's Qt sign-in window)
    /// could be found by the client: the launch covers each with bastion's bridge stub, so
    /// that no sign-in page ever runs inside the game's sandbox. Absolute paths as the sandbox
    /// sees them (its `/usr` is the system's); see [`Client::installed_webviews`] for the ones
    /// that exist.
    pub webview_paths: Vec<PathBuf>,
    /// The client's version, when the package says it ("1.8.7-1 (mcpelauncher-linux)").
    pub version: Option<String>,
}

impl Client {
    /// The [`webview_paths`](Client::webview_paths) that exist now as regular files, links
    /// resolved to the file they lead to: what the launch must cover with the stub. A path
    /// that does not exist on the system does not exist in the sandbox either (its `/usr` is
    /// the system's, read-only).
    pub fn installed_webviews(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for path in &self.webview_paths {
            let Ok(meta) = std::fs::symlink_metadata(path) else { continue };
            let real = if meta.file_type().is_symlink() {
                match std::fs::canonicalize(path) {
                    Ok(real) => real,
                    Err(_) => continue,
                }
            } else {
                path.clone()
            };
            if std::fs::symlink_metadata(&real).is_ok_and(|m| m.is_file()) && !out.contains(&real) {
                out.push(real);
            }
        }
        out
    }
}

/// What may be trusted, and where the system is. Production: [`Policy::system`], with `/` and
/// root; this module's tests build trees of their own under a directory owned by the test's
/// user (nothing outside the tests can construct another policy).
#[derive(Debug, Clone)]
struct Policy {
    /// Where the system starts (`/`).
    root: PathBuf,
    /// The only owner a trusted file or directory may have (root).
    owner: u32,
    /// The user's home directories, as configured and with links resolved: nothing under them
    /// is trusted.
    homes: Vec<PathBuf>,
    /// Flatpak installations: system, per-user and configured ones.
    flatpak_roots: Vec<PathBuf>,
}

impl Policy {
    fn system() -> Policy {
        let root = PathBuf::from("/");
        let mut homes: Vec<PathBuf> = Vec::new();
        let env_home = std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute());
        let passwd_home = nix::unistd::User::from_uid(nix::unistd::getuid()).ok().flatten().map(|u| u.dir);
        for home in [env_home, passwd_home].into_iter().flatten() {
            for form in [Some(home.clone()), std::fs::canonicalize(&home).ok()].into_iter().flatten() {
                if form != root && !homes.contains(&form) {
                    homes.push(form);
                }
            }
        }
        let flatpak_roots = flatpak_roots(&root, &homes);
        Policy { root, owner: 0, homes, flatpak_roots }
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

/// The system installation, each home's per-user installation, and the `Path=` of every
/// installation configured in `/etc/flatpak/installations.d/*.conf`.
fn flatpak_roots(root: &Path, homes: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = vec![root.join(FLATPAK_SYSTEM)];
    roots.extend(homes.iter().map(|h| h.join(".local/share/flatpak")));
    if let Ok(entries) = std::fs::read_dir(root.join(FLATPAK_INSTALLATIONS)) {
        for entry in entries.flatten().take(256) {
            let Ok(data) = util::read_small_file(&entry.path(), 64 * 1024) else { continue };
            for line in String::from_utf8_lossy(&data).lines() {
                if let Some(path) = line.trim().strip_prefix("Path=").map(str::trim).filter(|p| p.starts_with('/')) {
                    roots.push(PathBuf::from(path));
                }
            }
        }
    }
    let resolved: Vec<PathBuf> = roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).collect();
    roots.extend(resolved);
    roots.sort();
    roots.dedup();
    roots
}

const FLATPAK_REASON: &str = "a Flatpak's client runs on the Flatpak runtime's libraries, not this system's, from files \
     Flatpak manages outside the system's package manager (a per-user Flatpak even under your home), and Flatpak's own \
     sandbox needs new namespaces, which bastion's sandbox forbids";

fn under_home(path: &Path, policy: &Policy) -> Result<()> {
    if let Some(home) = policy.homes.iter().find(|home| path.starts_with(home)) {
        return Err(Error::Policy(format!(
            "{} is under your home directory ({}): you, and every program running as you, can change what is \
             there, so bastion does not run it as the Bedrock client. Install the client with a system package \
             (on Arch: mcpelauncher-linux from the AUR)",
            path.display(),
            home.display()
        )));
    }
    Ok(())
}

/// Refusals by location alone: before anything is opened (the configured path) and after
/// (where its links led).
fn refuse_location(path: &Path, policy: &Policy) -> Result<()> {
    if let Some(root) = policy.flatpak_roots.iter().find(|root| path.starts_with(root)) {
        return Err(Error::Policy(format!(
            "{} is a Flatpak install (in {}); bastion does not run it: {FLATPAK_REASON}",
            path.display(),
            root.display()
        )));
    }
    under_home(path, policy)?;
    if path.components().any(|c| c.as_os_str().as_bytes().starts_with(b".mount_")) {
        return Err(Error::Policy(format!(
            "{} is inside an AppImage's mount: an AppImage mounts itself through FUSE, served by a program of \
             yours, and brings its own libraries; bastion runs only a client installed by a system package",
            path.display()
        )));
    }
    Ok(())
}

/// A Flatpak deployment found by its layout: `…/<commit>/files/…` beside a `metadata` file
/// that declares an application or a runtime (wherever the installation lives).
fn refuse_flatpak_layout(path: &Path, policy: &Policy) -> Result<()> {
    for dir in path.ancestors().skip(1) {
        if !dir.starts_with(&policy.root) || dir == policy.root {
            break;
        }
        if dir.file_name().map(|n| n.as_bytes()) != Some(b"files") {
            continue;
        }
        let Some(deploy) = dir.parent() else { continue };
        let Ok(metadata) = util::read_small_file(&deploy.join("metadata"), 64 * 1024) else { continue };
        let text = String::from_utf8_lossy(&metadata);
        if text.lines().any(|l| matches!(l.trim(), "[Application]" | "[Runtime]")) {
            return Err(Error::Policy(format!(
                "{} is part of a Flatpak deployment ({}); bastion does not run it: {FLATPAK_REASON}",
                path.display(),
                deploy.display()
            )));
        }
    }
    Ok(())
}

fn fuse_check(fd: impl AsFd, path: &Path) -> Result<()> {
    let fs = fstatfs(fd).ctx(|| format!("examining the file system of {}", path.display()))?;
    refuse_file_system(fs.filesystem_type(), path)
}

fn refuse_file_system(kind: nix::sys::statfs::FsType, path: &Path) -> Result<()> {
    if kind == FUSE_SUPER_MAGIC {
        return Err(Error::Policy(format!(
            "{} is on a FUSE file system (an AppImage's mount, or another file system served by a program of a \
             user): what it contains can change at any moment, and whom it says owns it is up to that program",
            path.display()
        )));
    }
    Ok(())
}

fn check_dir(fd: &OwnedFd, path: &Path, policy: &Policy) -> Result<()> {
    let st = fstat(fd).ctx(|| format!("examining {}", path.display()))?;
    if st.st_uid != policy.owner {
        return Err(Error::Policy(format!(
            "{} is owned by uid {}, not by root: every directory on the way to the Bedrock client must be root's \
             (as for the JVM), or whoever owns it could put another program in the client's place",
            path.display(),
            st.st_uid
        )));
    }
    if st.st_mode & 0o022 != 0 {
        return Err(Error::Policy(format!(
            "{} is writable by group or others (mode {:o}): anyone who can write it could put another program in \
             the Bedrock client's place",
            path.display(),
            st.st_mode & 0o7777
        )));
    }
    fuse_check(fd, path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    File,
    Dir,
}

/// Where a walk ended: the real path, its parent directory and its name there (both for
/// opening it again), and what `fstat` said about it.
struct Found {
    path: PathBuf,
    parent: OwnedFd,
    name: OsString,
    stat: FileStat,
    fd: OwnedFd,
}

const PATH_FLAGS: OFlag = OFlag::O_PATH.union(OFlag::O_NOFOLLOW).union(OFlag::O_CLOEXEC);

/// Resolves `path` from the policy's root one component at a time: every directory on the
/// way is checked before anything below it is looked at, every symbolic link is read from a
/// checked directory and followed by its components (an absolute target starts again at the
/// root), and the walk ends on a file or a directory, as `want` says.
fn walk(path: &Path, policy: &Policy, want: Want) -> Result<Found> {
    let rest = path.strip_prefix(&policy.root).map_err(|_| {
        Error::Config(format!("{} is not an absolute path (the Bedrock client must be given as one)", path.display()))
    })?;
    let root_fd = nix::fcntl::open(&policy.root, OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC, Mode::empty())
        .ctx(|| format!("opening {}", policy.root.display()))?;
    check_dir(&root_fd, &policy.root, policy)?;
    let mut dirs: Vec<(PathBuf, OwnedFd)> = vec![(policy.root.clone(), root_fd)];
    let mut pending: Vec<OsString> = rest.components().rev().map(|c| c.as_os_str().to_os_string()).collect();
    let mut hops = 0usize;
    while let Some(name) = pending.pop() {
        match name.as_bytes() {
            b"" | b"." => continue,
            b".." => {
                if dirs.len() > 1 {
                    dirs.pop();
                }
                continue;
            }
            _ => {}
        }
        let (here, dir) = dirs.last().expect("the root stays");
        let here = here.join(&name);
        let st = fstatat(dir, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
            .ctx(|| format!("examining {}", here.display()))?;
        match SFlag::from_bits_truncate(st.st_mode) & SFlag::S_IFMT {
            SFlag::S_IFLNK => {
                hops += 1;
                if hops > MAX_HOPS {
                    return Err(Error::Policy(format!("{}: too many symbolic links (a loop?)", path.display())));
                }
                let target = PathBuf::from(
                    nix::fcntl::readlinkat(dir, name.as_os_str())
                        .ctx(|| format!("reading the link {}", here.display()))?,
                );
                if target.as_os_str().is_empty() {
                    return Err(Error::Policy(format!("{} is an empty symbolic link", here.display())));
                }
                if target.is_absolute() {
                    let inside = target.strip_prefix(&policy.root).map_err(|_| {
                        Error::Policy(format!("{} leads outside the system ({})", here.display(), target.display()))
                    })?;
                    dirs.truncate(1);
                    pending.extend(inside.components().rev().map(|c| c.as_os_str().to_os_string()));
                } else {
                    pending.extend(target.components().rev().map(|c| match c {
                        Component::ParentDir => OsString::from(".."),
                        other => other.as_os_str().to_os_string(),
                    }));
                }
            }
            SFlag::S_IFDIR => {
                let fd = nix::fcntl::openat(dir, name.as_os_str(), PATH_FLAGS | OFlag::O_DIRECTORY, Mode::empty())
                    .ctx(|| format!("opening {}", here.display()))?;
                check_dir(&fd, &here, policy)?;
                if dirs.len() >= MAX_DEPTH {
                    return Err(Error::Policy(format!("{}: nested too deeply", path.display())));
                }
                dirs.push((here, fd));
            }
            SFlag::S_IFREG if pending.is_empty() && want == Want::File => {
                let fd = nix::fcntl::openat(dir, name.as_os_str(), PATH_FLAGS, Mode::empty())
                    .ctx(|| format!("opening {}", here.display()))?;
                let stat = fstat(&fd).ctx(|| format!("examining {}", here.display()))?;
                if (stat.st_dev, stat.st_ino) != (st.st_dev, st.st_ino) {
                    return Err(Error::Policy(format!("{} changed while it was checked", here.display())));
                }
                let (_, parent) = dirs.pop().expect("the root stays");
                return Ok(Found { path: here, parent, name, stat, fd });
            }
            SFlag::S_IFREG => {
                return Err(Error::Policy(format!(
                    "{} is a file, but {} must be a directory",
                    here.display(),
                    if pending.is_empty() { path.display().to_string() } else { "it".to_string() }
                )));
            }
            _ => {
                return Err(Error::Policy(format!("{} is neither a file nor a directory", here.display())));
            }
        }
    }
    match want {
        Want::Dir => {
            let (path, fd) = dirs.pop().expect("the root stays");
            let stat = fstat(&fd).ctx(|| format!("examining {}", path.display()))?;
            let name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
            let parent =
                nix::fcntl::openat(&fd, "..", OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC, Mode::empty())
                    .ctx(|| format!("opening the directory above {}", path.display()))?;
            Ok(Found { path, parent, name, stat, fd })
        }
        Want::File => Err(Error::Policy(format!("{} is a directory, not a program", path.display()))),
    }
}

/// The program at `path`, checked: location, every step of the way, the file itself and its
/// header. Returns its real path.
fn trusted_program(path: &Path, policy: &Policy) -> Result<PathBuf> {
    refuse_location(path, policy)?;
    let found = walk(path, policy, Want::File)?;
    refuse_location(&found.path, policy)?;
    refuse_flatpak_layout(&found.path, policy)?;
    let st = &found.stat;
    let shown = found.path.display();
    if st.st_uid != policy.owner || st.st_mode & 0o022 != 0 {
        return Err(Error::Policy(format!(
            "{shown} must be owned by root and writable by nobody else (it is uid {}, mode {:o}): bastion runs \
             only a client installed by a system package",
            st.st_uid,
            st.st_mode & 0o7777
        )));
    }
    if st.st_mode & 0o6000 != 0 {
        return Err(Error::Policy(format!("{shown} is set-user-ID or set-group-ID; no Bedrock client needs that")));
    }
    if st.st_mode & 0o001 == 0 {
        return Err(Error::Policy(format!("{shown} is not executable by you (mode {:o})", st.st_mode & 0o7777)));
    }
    fuse_check(&found.fd, &found.path)?;
    let file = File::from(
        nix::fcntl::openat(
            &found.parent,
            found.name.as_os_str(),
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .ctx(|| format!("opening {shown}"))?,
    );
    let now = fstat(&file).ctx(|| format!("examining {shown}"))?;
    if (now.st_dev, now.st_ino) != (st.st_dev, st.st_ino) {
        return Err(Error::Policy(format!("{shown} changed while it was checked")));
    }
    let mut head = [0u8; elf::HEADER_BYTES];
    let got = file.read_at(&mut head, 0).ctx(|| format!("reading {shown}"))?;
    elf::check_x86_64_program(&head[..got]).map_err(|why| {
        Error::Policy(match why.as_str() {
            "an AppImage" => format!(
                "{shown} is an AppImage: when it runs it mounts itself through FUSE, as a file system served by a \
                 program of yours, and brings its own libraries; bastion runs only a client installed by a system package"
            ),
            "a script, not a program" => format!(
                "{shown} is a script (a wrapper, such as a Flatpak's exported launcher): bastion runs the client \
                 program itself"
            ),
            other => format!("{shown} is {other}: not an x86-64 Bedrock client"),
        })
    })?;
    Ok(found.path)
}

/// A directory the client reads at run time, and everything in it: each step of the way as
/// for the program, each entry root's and writable by nobody else, no links, nothing but
/// files and directories. Returns its real path.
fn trusted_tree(dir: &Path, policy: &Policy) -> Result<PathBuf> {
    refuse_location(dir, policy)?;
    let found = walk(dir, policy, Want::Dir)?;
    refuse_location(&found.path, policy)?;
    let mut budget = MAX_TREE_ENTRIES;
    fn inside(fd: &OwnedFd, at: &Path, policy: &Policy, depth: usize, budget: &mut usize) -> Result<()> {
        if depth > 16 {
            return Err(Error::Policy(format!("{} is nested too deeply", at.display())));
        }
        let listing_fd =
            nix::fcntl::openat(fd, ".", OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC, Mode::empty())
                .ctx(|| format!("opening {}", at.display()))?;
        let mut listing = Dir::from_fd(listing_fd).ctx(|| format!("reading {}", at.display()))?;
        let names: Vec<OsString> = listing
            .iter()
            .flatten()
            .map(|e| std::ffi::OsStr::from_bytes(e.file_name().to_bytes()).to_os_string())
            .filter(|n| n.as_bytes() != b"." && n.as_bytes() != b"..")
            .collect();
        for name in names {
            *budget =
                budget.checked_sub(1).ok_or_else(|| Error::Policy(format!("{} holds too many files", at.display())))?;
            let path = at.join(&name);
            let st = fstatat(listing.as_fd(), name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
                .ctx(|| format!("examining {}", path.display()))?;
            match SFlag::from_bits_truncate(st.st_mode) & SFlag::S_IFMT {
                SFlag::S_IFDIR => {
                    let sub = nix::fcntl::openat(
                        listing.as_fd(),
                        name.as_os_str(),
                        PATH_FLAGS | OFlag::O_DIRECTORY,
                        Mode::empty(),
                    )
                    .ctx(|| format!("opening {}", path.display()))?;
                    check_dir(&sub, &path, policy)?;
                    inside(&sub, &path, policy, depth + 1, budget)?;
                }
                SFlag::S_IFREG => {
                    if st.st_uid != policy.owner || st.st_mode & 0o022 != 0 {
                        return Err(Error::Policy(format!(
                            "{} must be owned by root and writable by nobody else (uid {}, mode {:o}): the Bedrock client \
                             loads its files",
                            path.display(),
                            st.st_uid,
                            st.st_mode & 0o7777
                        )));
                    }
                }
                SFlag::S_IFLNK => {
                    return Err(Error::Policy(format!(
                        "{} is a symbolic link inside the Bedrock client's data directory; bastion accepts only plain files there",
                        path.display()
                    )));
                }
                _ => return Err(Error::Policy(format!("{} is neither a file nor a directory", path.display()))),
            }
        }
        Ok(())
    }
    inside(&found.fd, &found.path, policy, 0, &mut budget)?;
    Ok(found.path)
}

/// The client's data directories that exist, in the client's own search order.
fn data_dirs(program: &Path, policy: &Policy) -> Result<Vec<PathBuf>> {
    let prefix = program.parent().and_then(Path::parent);
    let mut candidates: Vec<PathBuf> = prefix.map(|p| p.join("share/mcpelauncher")).into_iter().collect();
    candidates.extend(SYSTEM_DATA_DIRS.iter().map(|d| policy.at(d)));
    let mut out = Vec::new();
    for candidate in candidates {
        if std::fs::symlink_metadata(&candidate).is_err() {
            continue;
        }
        let real = trusted_tree(&candidate, policy)?;
        if !out.contains(&real) {
            out.push(real);
        }
    }
    Ok(out)
}

/// What pacman's database says.
#[derive(Debug, Default, PartialEq, Eq)]
struct Pacman {
    /// Name and version of the package that owns the program.
    owner: Option<(String, String)>,
    /// Every installed file called `mcpelauncher-webview`, of any package.
    webviews: Vec<PathBuf>,
}

/// The entries of a `files` record (`%FILES%` up to the next blank line or section).
fn files_entries(text: &str) -> impl Iterator<Item = &str> {
    text.lines().skip_while(|l| *l != "%FILES%").skip(1).take_while(|l| !l.is_empty() && !l.starts_with('%'))
}

/// A field of a `desc` record (`%NAME%`, then the value on the next line), if it looks like
/// a package name or version.
fn desc_field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut lines = text.lines();
    lines.find(|l| *l == key)?;
    lines.next().filter(|v| !v.is_empty() && v.len() <= 128).filter(|v| {
        v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-' | b':' | b'~' | b'@'))
    })
}

/// Reads pacman's local database: the owner of `program` and every `mcpelauncher-webview`.
/// Records larger than the bound are skipped; an absent database (not Arch) is no error.
fn scan_pacman(policy: &Policy, program: &Path) -> Pacman {
    let mut found = Pacman::default();
    let Ok(rel) = program.strip_prefix(&policy.root) else { return found };
    let rel = rel.to_string_lossy();
    let Ok(entries) = std::fs::read_dir(policy.at(PACMAN_LOCAL)) else { return found };
    for entry in entries.flatten().take(50_000) {
        let Ok(data) = util::read_small_file(&entry.path().join("files"), 64 << 20) else { continue };
        let text = String::from_utf8_lossy(&data);
        let mut owns = false;
        for file in files_entries(&text) {
            if file == rel {
                owns = true;
            }
            if !file.ends_with('/') && Path::new(file).file_name().is_some_and(|n| n.as_bytes() == STUB_NAME.as_bytes())
            {
                let path = policy.root.join(file);
                if !found.webviews.contains(&path) {
                    found.webviews.push(path);
                }
            }
        }
        if owns && found.owner.is_none() {
            let desc = util::read_small_file(&entry.path().join("desc"), 1 << 20).unwrap_or_default();
            let desc = String::from_utf8_lossy(&desc);
            if let (Some(name), Some(version)) = (desc_field(&desc, "%NAME%"), desc_field(&desc, "%VERSION%")) {
                found.owner = Some((name.to_string(), version.to_string()));
            }
        }
    }
    found
}

/// Files called `mcpelauncher-webview` in the tree of a manual install (`prefix` other than
/// `/usr` and `/`). Refused when the tree is larger than can be searched: the webview could
/// then be anywhere in it.
fn webviews_in_tree(prefix: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut budget = MAX_TREE_ENTRIES;
    let mut stack = vec![(prefix.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(fd) = nix::fcntl::open(
            &dir,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) else {
            continue;
        };
        let Ok(mut listing) = Dir::from_fd(fd) else { continue };
        let names: Vec<OsString> = listing
            .iter()
            .flatten()
            .map(|e| std::ffi::OsStr::from_bytes(e.file_name().to_bytes()).to_os_string())
            .filter(|n| n.as_bytes() != b"." && n.as_bytes() != b"..")
            .collect();
        for name in names {
            budget = budget.checked_sub(1).ok_or_else(|| {
                Error::Policy(format!(
                    "{} holds more than {MAX_TREE_ENTRIES} entries: too many to search for an installed \
                     mcpelauncher-webview, which the sandbox must cover. Install the client with a package, or into a \
                     prefix of its own",
                    prefix.display()
                ))
            })?;
            let path = dir.join(&name);
            let Ok(st) = fstatat(listing.as_fd(), name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) else { continue };
            let kind = SFlag::from_bits_truncate(st.st_mode) & SFlag::S_IFMT;
            if name.as_bytes() == STUB_NAME.as_bytes() && kind != SFlag::S_IFDIR {
                found.push(path.clone());
            }
            if kind == SFlag::S_IFDIR && depth < 12 {
                stack.push((path, depth + 1));
            }
        }
    }
    Ok(found)
}

/// Every place an installed webview can be, in the order the client would try them, with
/// links followed to what they lead to.
fn webview_paths(program: &Path, policy: &Policy, pacman: &Pacman) -> Result<Vec<PathBuf>> {
    let appdir = program.parent().unwrap_or(&policy.root);
    let mut paths = vec![appdir.join(STUB_NAME)];
    paths.extend(SANDBOX_PATH_DIRS.iter().map(|d| policy.at(d).join(STUB_NAME)));
    paths.extend(pacman.webviews.iter().cloned());
    if let Some(prefix) = appdir.parent()
        && prefix != policy.root
        && prefix != policy.at("usr")
    {
        paths.extend(webviews_in_tree(prefix)?);
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for path in paths {
        let target = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|m| m.file_type().is_symlink())
            .and_then(|_| std::fs::canonicalize(&path).ok());
        for p in [Some(path), target].into_iter().flatten() {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Existing Flatpak installs of mcpelauncher (for the messages).
fn flatpak_installs(policy: &Policy) -> Vec<PathBuf> {
    policy
        .flatpak_roots
        .iter()
        .map(|root| root.join("app").join(FLATPAK_ID))
        .filter(|p| std::fs::symlink_metadata(p).is_ok())
        .collect()
}

fn not_installed(policy: &Policy) -> Error {
    let searched: Vec<String> = SEARCHED.iter().map(|r| policy.at(r).display().to_string()).collect();
    let flatpaks = flatpak_installs(policy);
    let flatpak_note = if flatpaks.is_empty() {
        String::new()
    } else {
        format!(
            " The Flatpak install at {} is not used: {FLATPAK_REASON}.",
            flatpaks.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        )
    };
    Error::Config(format!(
        "mcpelauncher-client is not installed (looked for {}). bastion runs the client a system package installs — \
         on Arch the AUR package mcpelauncher-linux, installed with pacman — and never downloads or builds one. If \
         yours is elsewhere, set [bedrock] client to it (it must be owned by root, with no writable directory on its \
         way).{flatpak_note}",
        searched.join(" and ")
    ))
}

fn discover_with(cfg: &BedrockConfig, policy: &Policy) -> Result<Client> {
    let program = match &cfg.client {
        Some(path) => {
            if !path.is_absolute() {
                return Err(Error::Config(format!("[bedrock] client = {:?} must be an absolute path", path.display())));
            }
            // Refused by location before anything at the path is looked at.
            refuse_location(path, policy)?;
            if std::fs::symlink_metadata(path).is_err() {
                return Err(Error::Config(format!("[bedrock] client = {:?} does not exist", path.display())));
            }
            trusted_program(path, policy)?
        }
        None => {
            let candidate = SEARCHED.iter().map(|r| policy.at(r)).find(|p| std::fs::symlink_metadata(p).is_ok());
            // An untrustworthy client where packages put it is a finding, not a reason to look
            // further: it is refused with its reason.
            match candidate {
                Some(path) => trusted_program(&path, policy)?,
                None => return Err(not_installed(policy)),
            }
        }
    };
    let lib_dirs = data_dirs(&program, policy)?;
    let pacman = scan_pacman(policy, &program);
    let webview_paths = webview_paths(&program, policy, &pacman)?;
    let version = pacman.owner.map(|(name, version)| format!("{version} ({name})"));
    Ok(Client { program, lib_dirs, webview_paths, version })
}

/// Finds and checks the client (see the module's documentation for where and how).
pub fn discover(cfg: &BedrockConfig) -> Result<Client> {
    discover_with(cfg, &Policy::system())
}

/// What `doctor` reports about the client: never fails.
#[derive(Debug)]
pub struct Description {
    /// The client a launch would run, or why there is none.
    pub client: Result<Client>,
    /// Where it was looked for: the configured path, or [`SEARCHED`].
    pub searched: Vec<PathBuf>,
    /// Things worth a line: installed webviews the launch covers, Flatpak installs that are
    /// not used and why.
    pub notes: Vec<String>,
}

/// [`discover`], and what else `doctor` should say about it.
pub fn describe(cfg: &BedrockConfig) -> Description {
    let policy = Policy::system();
    let searched = match &cfg.client {
        Some(path) => vec![path.clone()],
        None => SEARCHED.iter().map(|r| policy.at(r)).collect(),
    };
    let client = discover_with(cfg, &policy);
    let mut notes = Vec::new();
    if let Ok(client) = &client {
        for webview in client.installed_webviews() {
            notes.push(format!(
                "installed sign-in window {}: covered with bastion's stub in the sandbox, so sign-in pages open outside the game",
                webview.display()
            ));
        }
    }
    for flatpak in flatpak_installs(&policy) {
        notes.push(format!("Flatpak install at {} not used: {FLATPAK_REASON}", flatpak.display()));
    }
    Description { client, searched, notes }
}

// ---------------------------------------------------------------- fuzzing

/// pacman database records, read as `files` and as `desc` (root writes them; bounded and
/// checked all the same): whatever the input, the entries are lines of it and the fields are
/// short and printable.
#[doc(hidden)]
pub fn fuzz_pacman(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    for file in files_entries(&text) {
        assert!(!file.is_empty() && !file.contains('\n'));
    }
    for key in ["%NAME%", "%VERSION%"] {
        if let Some(value) = desc_field(&text, key) {
            assert!(value.len() <= 128 && value.bytes().all(|b| b.is_ascii_graphic()), "{value:?}");
        }
    }
}

/// A `desc` and a `files` record of the client's package, as one input.
#[doc(hidden)]
pub fn fuzz_seed_pacman() -> Vec<u8> {
    b"%NAME%\nmcpelauncher-linux\n\n%VERSION%\n1.8.7-1\n\n%BASE%\nmcpelauncher-linux\n\n%FILES%\nusr/\nusr/bin/\n\
      usr/bin/mcpelauncher-client\nusr/bin/mcpelauncher-webview\nusr/share/mcpelauncher/\n\
      usr/share/mcpelauncher/lib/x86_64/libsqliteX.so\n\n"
        .to_vec()
}

#[cfg(test)]
mod tests;
