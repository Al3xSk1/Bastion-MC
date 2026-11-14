//! Imported Bedrock versions (owned by the store package).
//!
//! `bastion-mc bedrock import <dir>` copies one version that `mcpelauncher-ui` downloaded
//! into bastion's store; a launch opens it by descriptor ([`open`]) after checking every file
//! against the manifest the import wrote, and the sandbox binds exactly that directory
//! read-only at `/bastion/game`.
//!
//! **Where mcpelauncher-ui keeps versions** (`versionmanager.cpp`:
//! `QStandardPaths::GenericDataLocation` + `mcpelauncher/versions`; `apkextractiontask.cpp`
//! names the directory after the version name): `$XDG_DATA_HOME/mcpelauncher/versions/<name>/`
//! (by default `~/.local/share/…`) for the native package, and
//! `~/.var/app/io.mrarm.mcpelauncher/data/mcpelauncher/versions/<name>/` for the Flatpak, whose
//! `XDG_DATA_HOME` is that directory. Both are found by [`ui_version_dirs`].
//!
//! **What is copied, and why only that.** The UI extracts each APK through
//! `MinecraftExtractUtils::filterMinecraftFile` (mcpelauncher-extract): `assets/…` (with
//! `res/raw/…` and the launcher icon folded into it), `lib/<abi>/…` for every ABI it
//! downloaded, and the base APK's `AndroidManifest.xml`. The client reads exactly
//! `AndroidManifest.xml` (package and version code, `main.cpp`), `lib/<abi>/` (the game's
//! libraries, mapped by its own linker) and `assets/` (the game's asset manager and its
//! relative paths) — so the import copies `AndroidManifest.xml`, `lib/x86_64/` and `assets/`,
//! nothing else, and says what it left out. **No APK file is needed at run time and none is
//! ever accepted**: the import takes the directory the UI extracted, never an `.apk`.
//! Only x86_64 builds: `lib/x86_64/libminecraftpe.so` must exist, and every file in
//! `lib/x86_64/` must be an x86-64 shared library named `lib*.so` (the files Android itself
//! would install).
//!
//! **What the copy refuses**: symbolic links, hard links, devices, FIFOs and sockets anywhere
//! in the copied part, names that are not UTF-8 or contain control characters, and trees
//! beyond its [`LIMITS`]. A link or a hard link in a download directory is the way to make a
//! private file of the user's appear inside the game's sandbox (everything in the version is
//! readable by the game), so none is followed or copied as content. Every name is resolved
//! relative to an open directory with `O_NOFOLLOW`, every file is checked to be the same file
//! (device, inode, size, modification and change time) when it is copied as when it was looked
//! at, and a file that changes while it is copied ends the import. The copy goes to a hidden
//! staging directory, is flushed, gets its manifest **last**, is made read-only (files `0444`,
//! directories `0555`) and appears under its id by one `rename` that never replaces anything.
//!
//! **Layout**: `versions/<id>/manifest` (see `manifest`) and `versions/<id>/game/` (what the
//! sandbox mounts). [`open`] checks every file's size and SHA-256 against the manifest at every
//! launch (the hash alone ran at 2.1 GiB/s on the machine this was written on, which has SHA
//! extensions; reading the files from disk comes on top when they are not cached), refuses
//! anything not listed, and returns the directory with a shared lock on it that lives as long
//! as the descriptor: [`remove`] takes the lock exclusively and refuses while any launch holds
//! it, and also refuses while a running launch, by its marker in `running/`, belongs to a
//! Bedrock instance of that version.
//!
//! Not guaranteed: that the files are Mojang's. They are what the user's download wrote; the
//! store makes them checkable, not authentic. And the check happens at launch: a program
//! running as the user can change the files afterwards (as it can change anything else of the
//! user's) — the read-only modes stop accidents, not the owner.

mod axml;
pub(crate) mod elf;
pub mod manifest;

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::FileExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use nix::{
    dir::Dir,
    errno::Errno,
    fcntl::{AtFlags, Flock, FlockArg, OFlag, RenameFlags},
    sys::stat::{FileStat, Mode, SFlag, fstat, fstatat},
};

use self::manifest::{Entry, Kind, Manifest};
use crate::{
    error::{Context, Error, Result},
    events::{Progress, Reporter, Unit},
    paths::{Paths, validate_id},
    util,
};

/// Where imported versions live.
pub fn versions_dir(paths: &Paths) -> PathBuf {
    paths.data.join("bedrock").join("versions")
}

const MANIFEST_FILE: &str = "manifest";
const GAME_DIR: &str = "game";
const STAGING_PREFIX: &str = ".import-";
const TRASH_PREFIX: &str = ".remove-";
const ANDROID_MANIFEST: &str = "AndroidManifest.xml";
const MINECRAFT_LIBRARY: &str = "libminecraftpe.so";
/// What `lib/` may hold besides `x86_64/`: the other ABIs mcpelauncher-ui downloads.
const OTHER_ABIS: &[&str] = &["x86", "arm64-v8a", "armeabi-v7a", "armeabi", "mips", "mips64"];

/// Bounds of an import.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Files and directories in the copied part.
    pub entries: usize,
    /// All files together.
    pub bytes: u64,
    /// Any one file.
    pub file_bytes: u64,
    /// Directory levels below `assets/`.
    pub depth: usize,
    /// Files in `lib/x86_64/`.
    pub libraries: usize,
}

/// The bounds `bastion-mc bedrock import` uses. None of them could be checked against a real
/// version: there are no game files on the machine this was written on. They are deliberately
/// generous guesses (8 GiB in all, 2 GiB for one file, 200,000 entries, 24 levels below
/// `assets/`), so that only a runaway or hostile tree meets them; should a real version ever
/// exceed one, the refusal names the limit. Such a tree would otherwise fill the disk (bytes),
/// the file system's inodes and the import's time (entries), or the walk's descriptors and
/// the length of the paths the manifest has to carry (depth).
pub const LIMITS: Limits =
    Limits { entries: manifest::MAX_ENTRIES, bytes: 8 << 30, file_bytes: 2 << 30, depth: 24, libraries: 64 };

/// A version directory opened for a launch.
#[derive(Debug)]
pub struct OpenedVersion {
    /// The version's id in the store (validated like an instance id).
    pub id: String,
    /// Its path, for messages and for bubblewrap's `--ro-bind` (the launch prefers `dir`).
    pub path: PathBuf,
    /// The directory (`versions/<id>/game`), opened `O_RDONLY | O_DIRECTORY | O_NOFOLLOW` by
    /// [`open`] (`O_PATH` by [`open_dir_unverified`]): what was checked is what gets mounted,
    /// even if the path is replaced in between. From [`open`] it also carries a shared `flock`
    /// that lasts as long as this descriptor and its duplicates stay open: keep the
    /// `OpenedVersion` until the game has exited and give bubblewrap a duplicate, so that
    /// [`remove`] refuses for the whole session. (`flock` does not work on `O_PATH`
    /// descriptors; for a bind mount, a read-only directory descriptor serves the same.)
    pub dir: OwnedFd,
}

/// What `versions` shows about one imported version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub version_name: String,
    pub version_code: u32,
    pub files: u64,
    pub bytes: u64,
    /// Seconds since the epoch.
    pub imported: u64,
}

impl Summary {
    fn new(id: &str, manifest: &Manifest) -> Summary {
        Summary {
            id: id.to_string(),
            version_name: manifest.version_name.clone(),
            version_code: manifest.version_code,
            files: manifest.files(),
            bytes: manifest.bytes(),
            imported: manifest.imported,
        }
    }
}

/// The result of an import.
#[derive(Debug, Clone)]
pub struct Imported {
    pub summary: Summary,
    /// What was found and deliberately not copied.
    pub notes: Vec<String>,
}

/// Everything in the store.
#[derive(Debug, Default)]
pub struct Listing {
    pub versions: Vec<Summary>,
    /// Entries that are not usable versions, with the reason.
    pub damaged: Vec<(String, String)>,
    /// Staging and removal directories of interrupted imports and removals.
    pub leftovers: Vec<String>,
}

const DIR_FLAGS: OFlag = OFlag::O_RDONLY.union(OFlag::O_DIRECTORY).union(OFlag::O_NOFOLLOW).union(OFlag::O_CLOEXEC);
const FILE_FLAGS: OFlag = OFlag::O_RDONLY.union(OFlag::O_NOFOLLOW).union(OFlag::O_NONBLOCK).union(OFlag::O_CLOEXEC);
const NEW_FILE: OFlag =
    OFlag::O_WRONLY.union(OFlag::O_CREAT).union(OFlag::O_EXCL).union(OFlag::O_NOFOLLOW).union(OFlag::O_CLOEXEC);

fn read_only_file() -> Mode {
    Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH
}

fn read_only_dir() -> Mode {
    Mode::S_IRUSR | Mode::S_IXUSR | Mode::S_IRGRP | Mode::S_IXGRP | Mode::S_IROTH | Mode::S_IXOTH
}

fn file_type(stat: &FileStat) -> SFlag {
    SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT
}

fn shown(text: &str) -> String {
    util::sanitize_line(text.as_bytes(), 160)
}

/// What a non-file, non-directory entry is, for a refusal.
fn what_it_is(kind: SFlag) -> &'static str {
    match kind {
        SFlag::S_IFLNK => "a symbolic link",
        SFlag::S_IFIFO => "a FIFO",
        SFlag::S_IFSOCK => "a socket",
        SFlag::S_IFCHR | SFlag::S_IFBLK => "a device",
        SFlag::S_IFDIR => "a directory",
        SFlag::S_IFREG => "a file",
        _ => "neither a file nor a directory",
    }
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") }
}

fn too_many(limit: usize) -> Error {
    Error::Policy(format!("more than {limit} files and directories; not importing a tree this large"))
}

/// The names in a directory, sorted, without `.` and `..`, each taken from `budget` (which
/// runs out at `limit`). A name that is not UTF-8 is an error naming the directory.
fn names(dir: &mut Dir, rel: &str, budget: &mut usize, limit: usize) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in dir.iter() {
        let entry = entry.ctx(|| format!("reading {}", shown(rel)))?;
        let raw = entry.file_name().to_bytes();
        if matches!(raw, b"." | b"..") {
            continue;
        }
        if *budget == 0 {
            return Err(too_many(limit));
        }
        *budget -= 1;
        let name = std::str::from_utf8(raw).map_err(|_| {
            Error::Policy(format!(
                "{} holds a name that is not UTF-8; not importing it",
                shown(if rel.is_empty() { "the directory" } else { rel })
            ))
        })?;
        out.push(name.to_string());
    }
    out.sort_unstable();
    Ok(out)
}

// ---------------------------------------------------------------- scan (nothing copied yet)

/// The identity of a file at the time it was looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Facts {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Facts {
    fn of(stat: &FileStat) -> Facts {
        Facts {
            dev: stat.st_dev,
            ino: stat.st_ino,
            size: stat.st_size as u64,
            mtime: (stat.st_mtime, stat.st_mtime_nsec),
            ctime: (stat.st_ctime, stat.st_ctime_nsec),
        }
    }
}

#[derive(Debug, Clone)]
enum Planned {
    /// A directory, and the identity it had.
    Dir {
        dev: u64,
        ino: u64,
    },
    File(Facts),
}

/// What the scan found: entries in walk order (each directory before what it holds), and the
/// facts read from the version.
struct Scan {
    entries: Vec<(String, Planned)>,
    apk: axml::ApkInfo,
    bytes: u64,
    files: usize,
    manifest_bytes: usize,
    notes: Vec<String>,
}

struct Walk<'a> {
    limits: &'a Limits,
    budget: usize,
    bytes: u64,
    files: usize,
    manifest_bytes: usize,
    entries: Vec<(String, Planned)>,
}

impl Walk<'_> {
    fn add(&mut self, rel: String, planned: Planned) -> Result<()> {
        manifest::validate_path(&rel).map_err(|why| Error::Policy(format!("{}: {why}", shown(&rel))))?;
        // The manifest line this entry will need: kind, digest, size, path.
        self.manifest_bytes += rel.len() + 90;
        if self.manifest_bytes > manifest::MAX_BYTES {
            return Err(Error::Policy("the version's file list would exceed the manifest's size limit".into()));
        }
        if let Planned::File(facts) = &planned {
            if facts.size > self.limits.file_bytes {
                return Err(Error::Policy(format!(
                    "{} is {} MiB; no file of a version may be larger than {} MiB",
                    shown(&rel),
                    facts.size >> 20,
                    self.limits.file_bytes >> 20
                )));
            }
            self.bytes += facts.size;
            self.files += 1;
            if self.bytes > self.limits.bytes {
                return Err(Error::Policy(format!(
                    "the version is larger than {} MiB; not importing a tree this large",
                    self.limits.bytes >> 20
                )));
            }
        }
        if self.entries.len() >= self.limits.entries {
            return Err(too_many(self.limits.entries));
        }
        self.entries.push((rel, planned));
        Ok(())
    }

    /// A regular file with one link, or a refusal that says what it is instead.
    fn file_facts(stat: &FileStat, rel: &str) -> Result<Facts> {
        let kind = file_type(stat);
        if kind != SFlag::S_IFREG {
            return Err(Error::Policy(format!(
                "{} is {}: an imported version holds only files and directories (a link could pull a file of \
                 yours into the game's view); not importing it",
                shown(rel),
                what_it_is(kind)
            )));
        }
        if stat.st_nlink != 1 {
            return Err(Error::Policy(format!(
                "{} has {} hard links: a hard-linked file is the same file as one somewhere else, which could \
                 put a file of yours into the game's view; not importing it",
                shown(rel),
                stat.st_nlink
            )));
        }
        Ok(Facts::of(stat))
    }

    fn list(&mut self, dir: &mut Dir, rel: &str) -> Result<Vec<String>> {
        let mut budget = self.budget;
        let names = names(dir, rel, &mut budget, self.limits.entries)?;
        self.budget = budget;
        Ok(names)
    }

    /// Everything under the directory `dir` (whose own entry the caller added).
    fn tree(&mut self, dir: BorrowedFd<'_>, rel: &str, depth: usize) -> Result<()> {
        if depth > self.limits.depth {
            return Err(Error::Policy(format!(
                "{} is nested more than {} levels deep; not importing it",
                shown(rel),
                self.limits.depth
            )));
        }
        let fd = nix::fcntl::openat(dir, ".", DIR_FLAGS, Mode::empty()).ctx(|| format!("opening {}", shown(rel)))?;
        let mut listing = Dir::from_fd(fd).ctx(|| format!("reading {}", shown(rel)))?;
        for name in self.list(&mut listing, rel)? {
            let child = join(rel, &name);
            manifest::validate_name(&name).map_err(|why| Error::Policy(format!("{}: {why}", shown(&child))))?;
            let stat = fstatat(listing.as_fd(), name.as_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
                .ctx(|| format!("examining {}", shown(&child)))?;
            if file_type(&stat) == SFlag::S_IFDIR {
                self.add(child.clone(), Planned::Dir { dev: stat.st_dev, ino: stat.st_ino })?;
                let sub = nix::fcntl::openat(listing.as_fd(), name.as_str(), DIR_FLAGS, Mode::empty())
                    .ctx(|| format!("opening {}", shown(&child)))?;
                self.tree(sub.as_fd(), &child, depth + 1)?;
            } else {
                let facts = Self::file_facts(&stat, &child)?;
                self.add(child, Planned::File(facts))?;
            }
        }
        Ok(())
    }

    /// A directory entry of `parent` that must be a directory: added, opened and returned.
    fn dir(&mut self, parent: BorrowedFd<'_>, name: &str, rel: &str) -> Result<OwnedFd> {
        let stat = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).ctx(|| format!("examining {rel}"))?;
        if file_type(&stat) != SFlag::S_IFDIR {
            return Err(Error::Policy(format!(
                "{rel} is {}, not a directory; not importing it",
                what_it_is(file_type(&stat))
            )));
        }
        self.add(rel.to_string(), Planned::Dir { dev: stat.st_dev, ino: stat.st_ino })?;
        nix::fcntl::openat(parent, name, DIR_FLAGS, Mode::empty()).ctx(|| format!("opening {rel}"))
    }
}

fn read_all_bounded(file: &File, max: usize, what: &str) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let read = file.take(max as u64 + 1).read_to_end(&mut data).ctx(|| format!("reading {what}"))?;
    if read > max {
        return Err(Error::Policy(format!("{what} is larger than {max} bytes; not importing it")));
    }
    Ok(data)
}

/// The facts of the version in an extracted directory, without copying anything.
fn scan(root: &OwnedFd, source: &Path, limits: &Limits) -> Result<Scan> {
    let top_fd =
        nix::fcntl::openat(root, ".", DIR_FLAGS, Mode::empty()).ctx(|| format!("opening {}", source.display()))?;
    let mut top = Dir::from_fd(top_fd).ctx(|| format!("reading {}", source.display()))?;
    let mut walk = Walk { limits, budget: limits.entries, bytes: 0, files: 0, manifest_bytes: 0, entries: Vec::new() };
    let top_names = walk.list(&mut top, "")?;
    let mut notes = Vec::new();

    if !top_names.iter().any(|n| n == ANDROID_MANIFEST) {
        let nested: Vec<String> = top_names.iter().filter(|n| !n.starts_with('.')).take(5).map(|n| shown(n)).collect();
        return Err(Error::Policy(format!(
            "{} is not a version directory of mcpelauncher-ui: it has no {ANDROID_MANIFEST}{}",
            source.display(),
            if nested.is_empty() {
                String::new()
            } else {
                format!(
                    " (if it is mcpelauncher-ui's versions directory, import one of the versions in it: {})",
                    nested.join(", ")
                )
            }
        )));
    }

    // The manifest: read, checked, and its facts kept for the copy.
    let stat = fstatat(top.as_fd(), ANDROID_MANIFEST, AtFlags::AT_SYMLINK_NOFOLLOW)
        .ctx(|| format!("examining {ANDROID_MANIFEST}"))?;
    let facts = Walk::file_facts(&stat, ANDROID_MANIFEST)?;
    let file = File::from(
        nix::fcntl::openat(top.as_fd(), ANDROID_MANIFEST, FILE_FLAGS, Mode::empty())
            .ctx(|| format!("opening {ANDROID_MANIFEST}"))?,
    );
    if Facts::of(&fstat(&file).ctx(|| format!("examining {ANDROID_MANIFEST}"))?) != facts {
        return Err(changed(ANDROID_MANIFEST));
    }
    let apk = axml::parse(&read_all_bounded(&file, axml::MAX_BYTES, ANDROID_MANIFEST)?)?;
    check_apk(&apk)?;
    walk.add(ANDROID_MANIFEST.into(), Planned::File(facts))?;

    // assets/
    if !top_names.iter().any(|n| n == "assets") {
        return Err(Error::Policy(format!(
            "{} has no assets/ directory: the game cannot run without it",
            source.display()
        )));
    }
    let assets = walk.dir(top.as_fd(), "assets", "assets")?;
    walk.tree(assets.as_fd(), "assets", 1)?;

    // lib/x86_64/
    if !top_names.iter().any(|n| n == "lib") {
        return Err(Error::Policy(format!(
            "{} has no lib/ directory: there is no game library to run",
            source.display()
        )));
    }
    let lib_fd = walk.dir(top.as_fd(), "lib", "lib")?;
    let mut lib = Dir::from_fd(lib_fd).ctx(|| "reading lib")?;
    let abis = walk.list(&mut lib, "lib")?;
    if !abis.iter().any(|n| n == manifest::ABI) {
        let others: Vec<String> = abis.iter().map(|n| format!("lib/{}", shown(n))).collect();
        return Err(Error::Policy(format!(
            "this version has no x86_64 libraries ({}): bastion runs only x86_64 builds, the ones \
             mcpelauncher-client runs natively on this machine. Download the x86_64 version with mcpelauncher-ui",
            if others.is_empty() { "lib/ is empty".to_string() } else { format!("it has {}", others.join(", ")) }
        )));
    }
    for other in abis.iter().filter(|n| n.as_str() != manifest::ABI) {
        notes.push(if OTHER_ABIS.contains(&other.as_str()) {
            format!("not copied: lib/{} (libraries for another machine; only x86_64 is run)", shown(other))
        } else {
            format!("not copied: lib/{} (not an ABI the client loads)", shown(other))
        });
    }
    let abi_fd = walk.dir(lib.as_fd(), manifest::ABI, "lib/x86_64")?;
    let mut abi = Dir::from_fd(abi_fd).ctx(|| "reading lib/x86_64")?;
    let libraries = walk.list(&mut abi, "lib/x86_64")?;
    if libraries.len() > limits.libraries {
        return Err(Error::Policy(format!("lib/x86_64 holds more than {} files; not importing it", limits.libraries)));
    }
    if !libraries.iter().any(|n| n == MINECRAFT_LIBRARY) {
        return Err(Error::Policy(format!(
            "lib/x86_64 has no {MINECRAFT_LIBRARY}: this is not a Minecraft version (or its download is incomplete)"
        )));
    }
    for name in &libraries {
        let rel = format!("lib/x86_64/{name}");
        if !manifest::is_library_name(name) {
            return Err(Error::Policy(format!(
                "{}: lib/x86_64 may hold only libraries named lib*.so (what Android installs from an APK); not importing it",
                shown(&rel)
            )));
        }
        let stat = fstatat(abi.as_fd(), name.as_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
            .ctx(|| format!("examining {}", shown(&rel)))?;
        let facts = Walk::file_facts(&stat, &rel)?;
        let file = File::from(
            nix::fcntl::openat(abi.as_fd(), name.as_str(), FILE_FLAGS, Mode::empty())
                .ctx(|| format!("opening {}", shown(&rel)))?,
        );
        check_library(&file, &rel)?;
        walk.add(rel, Planned::File(facts))?;
    }

    for name in top_names.iter().filter(|n| !matches!(n.as_str(), ANDROID_MANIFEST | "assets" | "lib")) {
        notes.push(if name.to_ascii_lowercase().ends_with(".apk") {
            format!(
                "not copied: {} (APK files are never imported; the client needs only the extracted files)",
                shown(name)
            )
        } else {
            format!("not copied: {} (the client does not read it)", shown(name))
        });
    }
    Ok(Scan {
        bytes: walk.bytes,
        files: walk.files,
        manifest_bytes: walk.manifest_bytes,
        entries: walk.entries,
        apk,
        notes,
    })
}

fn check_apk(apk: &axml::ApkInfo) -> Result<()> {
    if apk.package != manifest::PACKAGE {
        return Err(Error::Policy(format!(
            "{ANDROID_MANIFEST} is the manifest of {}, not of Minecraft ({}): bastion imports Minecraft only",
            shown(&apk.package),
            manifest::PACKAGE
        )));
    }
    if let Some(split) = &apk.split {
        return Err(Error::Policy(format!(
            "{ANDROID_MANIFEST} is the manifest of the split APK {}, not of the base APK: the download is incomplete",
            shown(split)
        )));
    }
    Ok(())
}

fn check_library(file: &File, rel: &str) -> Result<()> {
    let mut head = [0u8; elf::HEADER_BYTES];
    let mut got = 0;
    while got < head.len() {
        match file.read_at(&mut head[got..], got as u64) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::io(format!("reading {}", shown(rel)), e)),
        }
    }
    elf::check_x86_64_library(&head[..got])
        .map_err(|why| Error::Policy(format!("{}: {why}; bastion runs only x86_64 builds", shown(rel))))
}

fn changed(rel: &str) -> Error {
    Error::Policy(format!(
        "{} changed while it was being imported; import again when nothing (mcpelauncher-ui) is writing to the directory",
        shown(rel)
    ))
}

// ---------------------------------------------------------------- copy

/// Copies one file, hashing what it writes; refuses a file that is not the one the scan saw
/// or that changes while it is read.
fn copy_file(
    src_dir: BorrowedFd<'_>,
    dst_dir: BorrowedFd<'_>,
    name: &str,
    rel: &str,
    facts: &Facts,
    buf: &mut [u8],
    progress: &Progress,
) -> Result<[u8; 32]> {
    let mut input = File::from(
        nix::fcntl::openat(src_dir, name, FILE_FLAGS, Mode::empty()).ctx(|| format!("opening {}", shown(rel)))?,
    );
    let before = fstat(&input).ctx(|| format!("examining {}", shown(rel)))?;
    if Facts::of(&before) != *facts || file_type(&before) != SFlag::S_IFREG || before.st_nlink != 1 {
        return Err(changed(rel));
    }
    if rel.starts_with("lib/") {
        check_library(&input, rel)?;
    }
    let mut output = File::from(
        nix::fcntl::openat(dst_dir, name, NEW_FILE, Mode::S_IRUSR | Mode::S_IWUSR)
            .ctx(|| format!("creating the copy of {}", shown(rel)))?,
    );
    let mut hasher = util::Hasher::new(util::HashAlg::Sha256);
    let mut total: u64 = 0;
    loop {
        let n = match input.read(buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::io(format!("reading {}", shown(rel)), e)),
        };
        total += n as u64;
        if total > facts.size {
            return Err(changed(rel));
        }
        hasher.update(&buf[..n]);
        output.write_all(&buf[..n]).ctx(|| format!("writing the copy of {}", shown(rel)))?;
        progress.add(n as u64);
    }
    let after = fstat(&input).ctx(|| format!("examining {}", shown(rel)))?;
    if total != facts.size || Facts::of(&after) != *facts {
        return Err(changed(rel));
    }
    nix::sys::stat::fchmod(&output, read_only_file()).ctx(|| format!("making the copy of {} read-only", shown(rel)))?;
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&hasher.finish());
    Ok(digest)
}

/// Copies what the scan planned from `src` into the empty directory `dst`, in the scan's
/// order, and returns the manifest's entries (sorted). Each copied directory is made
/// read-only once everything in it is there.
fn copy_planned(src: &OwnedFd, dst: &OwnedFd, scan: &Scan, reporter: &Reporter) -> Result<Vec<Entry>> {
    let progress = reporter.progress("copying the version", scan.bytes, Unit::Bytes);
    let mut buf = vec![0u8; 1 << 20];
    // The chain of directories from the top down to the one being filled: its path, the
    // source directory and the copy.
    let mut stack: Vec<(String, OwnedFd, OwnedFd)> = vec![(
        String::new(),
        nix::fcntl::openat(src, ".", DIR_FLAGS, Mode::empty()).ctx(|| "opening the version directory")?,
        nix::fcntl::openat(dst, ".", DIR_FLAGS, Mode::empty()).ctx(|| "opening the copy")?,
    )];
    let mut entries = Vec::with_capacity(scan.entries.len());
    for (rel, planned) in &scan.entries {
        let (parent, name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
        while stack.last().is_some_and(|(path, _, _)| path != parent) {
            let (path, _, copy) = stack.pop().expect("not empty");
            nix::sys::stat::fchmod(&copy, read_only_dir()).ctx(|| format!("making {} read-only", shown(&path)))?;
        }
        let (_, src_dir, dst_dir) =
            stack.last().ok_or_else(|| Error::Other(format!("{}: out of walk order", shown(rel))))?;
        match planned {
            Planned::Dir { dev, ino } => {
                let from = nix::fcntl::openat(src_dir, name, DIR_FLAGS, Mode::empty())
                    .ctx(|| format!("opening {}", shown(rel)))?;
                let now = fstat(&from).ctx(|| format!("examining {}", shown(rel)))?;
                if (now.st_dev, now.st_ino) != (*dev, *ino) {
                    return Err(changed(rel));
                }
                nix::sys::stat::mkdirat(dst_dir, name, Mode::S_IRWXU)
                    .ctx(|| format!("creating the copy of {}", shown(rel)))?;
                let to = nix::fcntl::openat(dst_dir, name, DIR_FLAGS, Mode::empty())
                    .ctx(|| format!("opening the copy of {}", shown(rel)))?;
                entries.push(Entry { path: rel.clone(), kind: Kind::Dir });
                stack.push((rel.clone(), from, to));
            }
            Planned::File(facts) => {
                let sha256 = copy_file(src_dir.as_fd(), dst_dir.as_fd(), name, rel, facts, &mut buf, &progress)?;
                entries.push(Entry { path: rel.clone(), kind: Kind::File { size: facts.size, sha256 } });
            }
        }
    }
    while let Some((path, _, copy)) = stack.pop() {
        nix::sys::stat::fchmod(&copy, read_only_dir()).ctx(|| format!("making {} read-only", shown(&path)))?;
    }
    progress.complete();
    entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Writes the manifest into the staging directory (the last file of an import).
fn write_manifest(stage: BorrowedFd<'_>, manifest: &Manifest) -> Result<()> {
    let mut file = File::from(
        nix::fcntl::openat(stage, MANIFEST_FILE, NEW_FILE, Mode::S_IRUSR | Mode::S_IWUSR)
            .ctx(|| "creating the manifest")?,
    );
    file.write_all(&manifest::encode(manifest)).ctx(|| "writing the manifest")?;
    nix::sys::stat::fchmod(&file, read_only_file()).ctx(|| "making the manifest read-only")?;
    file.sync_all().ctx(|| "writing the manifest")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn open_versions_dir(paths: &Paths, create: bool) -> Result<Option<OwnedFd>> {
    let dir = versions_dir(paths);
    if create {
        util::create_private_dir_all(&dir)?;
    }
    match nix::fcntl::open(&dir, DIR_FLAGS, Mode::empty()) {
        Ok(fd) => Ok(Some(fd)),
        Err(Errno::ENOENT) if !create => Ok(None),
        Err(e) => Err(Error::io(format!("opening {}", dir.display()), e.into())),
    }
}

/// Copies the version in `source` (a directory mcpelauncher-ui extracted) into the store
/// under `id` (by default its version name, or else the directory's name).
pub fn import(paths: &Paths, source: &Path, id: Option<&str>, reporter: &Reporter) -> Result<Imported> {
    import_with(paths, source, id, reporter, &LIMITS)
}

fn import_with(
    paths: &Paths,
    source: &Path,
    id: Option<&str>,
    reporter: &Reporter,
    limits: &Limits,
) -> Result<Imported> {
    let meta = std::fs::symlink_metadata(source).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::Usage(format!("{} does not exist", source.display())),
        _ => Error::io(format!("examining {}", source.display()), e),
    })?;
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(format!("{} is a symbolic link; give the directory itself", source.display())));
    }
    if !meta.is_dir() {
        return Err(Error::Policy(format!(
            "{} is not a directory. bastion imports the directory mcpelauncher-ui extracted a version into \
             (~/.local/share/mcpelauncher/versions/<version>/, or the Flatpak's \
             ~/.var/app/io.mrarm.mcpelauncher/data/mcpelauncher/versions/<version>/), never an APK",
            source.display()
        )));
    }
    if let Some(id) = id {
        validate_id(id)?;
    }
    let root = nix::fcntl::open(source, DIR_FLAGS, Mode::empty()).ctx(|| format!("opening {}", source.display()))?;
    reporter.status(format!("checking {}", source.display()));
    let scan = scan(&root, source, limits)?;

    let id = match id {
        Some(id) => id.to_string(),
        None => [Some(scan.apk.version_name.as_str()), source.file_name().and_then(OsStr::to_str)]
            .into_iter()
            .flatten()
            .find(|candidate| validate_id(candidate).is_ok())
            .map(str::to_string)
            .ok_or_else(|| {
                Error::Usage(format!(
                    "neither the version name {:?} nor the directory's name is a valid id; choose one with --id",
                    shown(&scan.apk.version_name)
                ))
            })?,
    };

    let versions_path = versions_dir(paths);
    let versions = open_versions_dir(paths, true)?.ok_or_else(|| Error::Other("the version store vanished".into()))?;
    sweep(&versions, &versions_path, Duration::from_secs(60));
    if fstatat(&versions, id.as_str(), AtFlags::AT_SYMLINK_NOFOLLOW).is_ok() {
        return Err(already(&id));
    }
    let space = nix::sys::statvfs::fstatvfs(&versions).ctx(|| format!("examining {}", versions_path.display()))?;
    let free = space.blocks_available().saturating_mul(space.fragment_size());
    let needed = scan.bytes + scan.manifest_bytes as u64 + (64 << 20);
    if free < needed {
        return Err(Error::Policy(format!(
            "the store ({}) has {} MiB free; this version needs {} MiB",
            versions_path.display(),
            free >> 20,
            needed >> 20
        )));
    }

    let staging = format!("{STAGING_PREFIX}{}", util::hex_encode(&util::random_bytes::<8>()?));
    nix::sys::stat::mkdirat(&versions, staging.as_str(), Mode::S_IRWXU).ctx(|| "creating the staging directory")?;
    let staged = (|| -> Result<Manifest> {
        let stage = nix::fcntl::openat(&versions, staging.as_str(), DIR_FLAGS, Mode::empty())
            .ctx(|| "opening the staging directory")?;
        // Held for the whole import: a sweep by another process leaves the directory alone.
        let stage = Flock::lock(stage, FlockArg::LockExclusiveNonblock)
            .map_err(|(_, e)| Error::io("locking the staging directory", e.into()))?;
        nix::sys::stat::mkdirat(&*stage, GAME_DIR, Mode::S_IRWXU).ctx(|| "creating the staging directory")?;
        let game =
            nix::fcntl::openat(&*stage, GAME_DIR, DIR_FLAGS, Mode::empty()).ctx(|| "opening the staging directory")?;
        reporter.status(format!("copying {} files ({} MiB)", scan.files, scan.bytes >> 20));
        let entries = copy_planned(&root, &game, &scan, reporter)?;
        // Again, from the copy: what was parsed is what was stored.
        let copied = File::from(
            nix::fcntl::openat(&game, ANDROID_MANIFEST, FILE_FLAGS, Mode::empty())
                .ctx(|| "opening the copied manifest")?,
        );
        if axml::parse(&read_all_bounded(&copied, axml::MAX_BYTES, ANDROID_MANIFEST)?)? != scan.apk {
            return Err(changed(ANDROID_MANIFEST));
        }
        nix::unistd::syncfs(&*stage).ctx(|| "flushing the copy to disk")?;
        let manifest = Manifest {
            package: scan.apk.package.clone(),
            version_name: scan.apk.version_name.clone(),
            version_code: scan.apk.version_code,
            imported: now_secs(),
            entries,
        };
        write_manifest(stage.as_fd(), &manifest)?;
        nix::sys::stat::fchmod(&*stage, read_only_dir()).ctx(|| "making the version read-only")?;
        nix::unistd::fsync(&*stage).ctx(|| "flushing the copy to disk")?;
        match nix::fcntl::renameat2(&versions, staging.as_str(), &versions, id.as_str(), RenameFlags::RENAME_NOREPLACE)
        {
            Ok(()) => {}
            Err(Errno::EEXIST) => return Err(already(&id)),
            Err(e) => return Err(Error::io("moving the copy into place", e.into())),
        }
        nix::unistd::fsync(&versions).ctx(|| format!("flushing {}", versions_path.display()))?;
        Ok(manifest)
    })();
    match staged {
        Ok(manifest) => {
            let summary = Summary::new(&id, &manifest);
            reporter.status(format!(
                "imported Minecraft {} (version code {}) as {id}: {} files, {} MiB, read-only, with a SHA-256 manifest",
                shown(&summary.version_name),
                summary.version_code,
                summary.files,
                summary.bytes >> 20
            ));
            Ok(Imported { summary, notes: scan.notes })
        }
        Err(e) => {
            let _ = crate::tree::remove(&versions_path.join(&staging), Some(Instant::now() + Duration::from_secs(300)));
            Err(e)
        }
    }
}

fn already(id: &str) -> Error {
    Error::Policy(format!(
        "a version called {id} is already imported; remove it first (`bastion-mc bedrock remove {id}`) or choose \
         another id with --id"
    ))
}

/// Removes staging and removal directories that nobody holds (an import or a removal that was
/// interrupted) and that are older than `min_age` (one that was created a moment ago may not
/// be locked yet). Best effort: what cannot be removed stays, and [`list`] counts it.
fn sweep(versions: &OwnedFd, versions_path: &Path, min_age: Duration) {
    let Ok(fd) = nix::fcntl::openat(versions, ".", DIR_FLAGS, Mode::empty()) else { return };
    let Ok(mut dir) = Dir::from_fd(fd) else { return };
    let leftovers: Vec<OsString> = dir
        .iter()
        .flatten()
        .map(|e| OsStr::from_bytes(e.file_name().to_bytes()).to_os_string())
        .filter(|n| {
            n.as_bytes().starts_with(STAGING_PREFIX.as_bytes()) || n.as_bytes().starts_with(TRASH_PREFIX.as_bytes())
        })
        .take(1000)
        .collect();
    for name in leftovers {
        let path = versions_path.join(&name);
        let old = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= min_age);
        if !old {
            continue;
        }
        let Ok(fd) = nix::fcntl::openat(versions, name.as_os_str(), DIR_FLAGS, Mode::empty()) else { continue };
        // Held by a running import or removal: not a leftover.
        let Ok(_lock) = Flock::lock(fd, FlockArg::LockExclusiveNonblock) else { continue };
        let _ = crate::tree::remove(&path, Some(Instant::now() + Duration::from_secs(120)));
    }
}

// ---------------------------------------------------------------- open, verify, list

fn read_manifest(root: BorrowedFd<'_>, id: &str) -> Result<Manifest> {
    let file = match nix::fcntl::openat(root, MANIFEST_FILE, FILE_FLAGS, Mode::empty()) {
        Ok(fd) => File::from(fd),
        Err(Errno::ENOENT) => return Err(Error::Integrity(format!("Bedrock version {id} has no manifest"))),
        Err(Errno::ELOOP) => {
            return Err(Error::Integrity(format!("the manifest of Bedrock version {id} is a symbolic link")));
        }
        Err(e) => return Err(Error::io(format!("opening the manifest of {id}"), e.into())),
    };
    let stat = fstat(&file).ctx(|| format!("examining the manifest of {id}"))?;
    if file_type(&stat) != SFlag::S_IFREG {
        return Err(Error::Integrity(format!("the manifest of Bedrock version {id} is not a regular file")));
    }
    let mut data = Vec::new();
    (&file)
        .take(manifest::MAX_BYTES as u64 + 1)
        .read_to_end(&mut data)
        .ctx(|| format!("reading the manifest of {id}"))?;
    manifest::parse(&data).map_err(|e| match e {
        Error::Integrity(why) => Error::Integrity(format!("Bedrock version {id}: {why}")),
        other => other,
    })
}

fn open_root(paths: &Paths, id: &str) -> Result<(OwnedFd, OwnedFd)> {
    validate_id(id)?;
    let missing =
        || Error::Usage(format!("Bedrock version {id} is not imported (`bastion-mc bedrock versions` lists what is)"));
    let versions = open_versions_dir(paths, false)?.ok_or_else(missing)?;
    match nix::fcntl::openat(&versions, id, DIR_FLAGS, Mode::empty()) {
        Ok(root) => Ok((versions, root)),
        Err(Errno::ENOENT) => Err(missing()),
        Err(Errno::ELOOP | Errno::ENOTDIR) => {
            Err(Error::Integrity(format!("{} is not a directory", versions_dir(paths).join(id).display())))
        }
        Err(e) => Err(Error::io(format!("opening {}", versions_dir(paths).join(id).display()), e.into())),
    }
}

/// Checks one stored file against its size and digest.
fn verify_file(
    dir: BorrowedFd<'_>,
    name: &str,
    path: &str,
    stat: &FileStat,
    size: u64,
    sha256: &[u8; 32],
    buf: &mut [u8],
) -> std::result::Result<(), String> {
    let file = File::from(
        nix::fcntl::openat(dir, name, FILE_FLAGS, Mode::empty())
            .map_err(|e| format!("{} cannot be opened ({e})", shown(path)))?,
    );
    let now = fstat(&file).map_err(|e| format!("{} cannot be examined ({e})", shown(path)))?;
    if (now.st_dev, now.st_ino) != (stat.st_dev, stat.st_ino) || file_type(&now) != SFlag::S_IFREG {
        return Err(format!("{} was replaced while it was checked", shown(path)));
    }
    if now.st_nlink != 1 {
        return Err(format!("{} has {} hard links", shown(path), now.st_nlink));
    }
    if now.st_size as u64 != size {
        return Err(format!("{} is {} bytes, its manifest says {size}", shown(path), now.st_size));
    }
    let mut hasher = util::Hasher::new(util::HashAlg::Sha256);
    let mut total = 0u64;
    let mut reader = &file;
    loop {
        let n = match reader.read(buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("{} cannot be read ({e})", shown(path))),
        };
        total += n as u64;
        if total > size {
            return Err(format!("{} grew while it was checked", shown(path)));
        }
        hasher.update(&buf[..n]);
    }
    if total != size || hasher.finish().as_slice() != sha256.as_slice() {
        return Err(format!("{} does not have the SHA-256 its manifest lists", shown(path)));
    }
    Ok(())
}

/// Compares the tree under `game` with the manifest: the same directories and files, nothing
/// else, every file of the listed size and SHA-256. Directories are only entered when the
/// manifest lists them, so the walk is as deep as the manifest allows and no deeper.
fn verify_tree(game: BorrowedFd<'_>, manifest: &Manifest, id: &str) -> Result<()> {
    struct Check<'m> {
        expected: HashMap<&'m str, &'m Kind>,
        seen: usize,
        buf: Vec<u8>,
    }
    fn walk(check: &mut Check<'_>, dir: BorrowedFd<'_>, rel: &str) -> std::result::Result<(), String> {
        let fd = nix::fcntl::openat(dir, ".", DIR_FLAGS, Mode::empty())
            .map_err(|e| format!("{} cannot be opened ({e})", shown(rel)))?;
        let mut listing = Dir::from_fd(fd).map_err(|e| format!("{} cannot be read ({e})", shown(rel)))?;
        let mut budget = check.expected.len() + 1;
        let names = names(&mut listing, rel, &mut budget, check.expected.len())
            .map_err(|_| "more entries than its manifest lists, or a name that is not UTF-8".to_string())?;
        for name in names {
            let path = join(rel, &name);
            let Some(&kind) = check.expected.get(path.as_str()) else {
                return Err(format!("{} is in the store but not in its manifest", shown(&path)));
            };
            let stat = fstatat(listing.as_fd(), name.as_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(|e| format!("{} cannot be examined ({e})", shown(&path)))?;
            match (file_type(&stat), kind) {
                (SFlag::S_IFDIR, Kind::Dir) => {
                    let sub = nix::fcntl::openat(listing.as_fd(), name.as_str(), DIR_FLAGS, Mode::empty())
                        .map_err(|e| format!("{} cannot be opened ({e})", shown(&path)))?;
                    check.seen += 1;
                    walk(check, sub.as_fd(), &path)?;
                }
                (SFlag::S_IFREG, Kind::File { size, sha256 }) => {
                    let mut buf = std::mem::take(&mut check.buf);
                    let outcome = verify_file(listing.as_fd(), &name, &path, &stat, *size, sha256, &mut buf);
                    check.buf = buf;
                    outcome?;
                    check.seen += 1;
                }
                (actual, expected) => {
                    let listed = if *expected == Kind::Dir { "a directory" } else { "a file" };
                    return Err(format!("{} is {}, its manifest lists {listed}", shown(&path), what_it_is(actual)));
                }
            }
        }
        Ok(())
    }

    let fail = |what: String| Error::Integrity(format!("Bedrock version {id}: {what}; import it again"));
    let mut check = Check {
        expected: manifest.entries.iter().map(|e| (e.path.as_str(), &e.kind)).collect(),
        seen: 0,
        buf: vec![0u8; 1 << 20],
    };
    walk(&mut check, game, "").map_err(fail)?;
    if check.seen != manifest.entries.len() {
        // Name the first one missing, for the message.
        let missing = manifest
            .entries
            .iter()
            .map(|e| e.path.as_str())
            .find(|path| fstatat(game, *path, AtFlags::AT_SYMLINK_NOFOLLOW).is_err())
            .unwrap_or("an entry");
        return Err(fail(format!("{} is missing from the store", shown(missing))));
    }
    Ok(())
}

fn open_verified(paths: &Paths, id: &str) -> Result<(OpenedVersion, Manifest)> {
    let (_versions, root) = open_root(paths, id)?;
    let manifest = read_manifest(root.as_fd(), id)?;
    let game = match nix::fcntl::openat(&root, GAME_DIR, DIR_FLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ENOENT | Errno::ELOOP | Errno::ENOTDIR) => {
            return Err(Error::Integrity(format!("Bedrock version {id} has no game directory; import it again")));
        }
        Err(e) => return Err(Error::io(format!("opening Bedrock version {id}"), e.into())),
    };
    // A shared lock for as long as the launch keeps the descriptor, and nothing longer. The
    // RAII wrapper would tie it to a value the launch does not keep, so the descriptor itself
    // carries it: closing the last copy of it is what ends it.
    #[allow(deprecated)]
    match nix::fcntl::flock(game.as_raw_fd(), FlockArg::LockSharedNonblock) {
        Ok(()) => {}
        Err(Errno::EWOULDBLOCK) => return Err(Error::Policy(format!("Bedrock version {id} is being removed"))),
        Err(e) => return Err(Error::io(format!("locking Bedrock version {id}"), e.into())),
    }
    verify_tree(game.as_fd(), &manifest, id)?;
    let path = versions_dir(paths).join(id).join(GAME_DIR);
    Ok((OpenedVersion { id: id.to_string(), path, dir: game }, manifest))
}

/// Opens and verifies an imported version: every file's size and SHA-256 against the
/// manifest, nothing unlisted. The returned descriptor holds the version against removal for
/// as long as it is open.
pub fn open(paths: &Paths, id: &str) -> Result<OpenedVersion> {
    open_verified(paths, id).map(|(opened, _)| opened)
}

/// The same check as [`open`], for `bastion-mc bedrock versions --verify`; nothing is held.
pub fn verify(paths: &Paths, id: &str) -> Result<Summary> {
    open_verified(paths, id).map(|(_, manifest)| Summary::new(id, &manifest))
}

/// Opens a directory as a version **without** any check: for tests of the launch, which
/// need a version before the store exists.
pub fn open_dir_unverified(id: &str, path: &Path) -> Result<OpenedVersion> {
    let fd = nix::fcntl::open(
        path,
        OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| Error::io(format!("opening {}", path.display()), std::io::Error::from(e)))?;
    Ok(OpenedVersion { id: id.to_string(), path: path.to_path_buf(), dir: fd })
}

/// Writes a stand-in for a version directory as mcpelauncher-ui extracts one (Minecraft
/// 1.26.62.1, x86_64), for tests that need something [`import`] accepts — the launch's too, to
/// go through [`open`] instead of [`open_dir_unverified`]. Its libraries are ELF headers and
/// nothing else: the client cannot run them.
#[doc(hidden)]
pub fn write_stand_in_version(dir: &Path) -> Result<()> {
    let library = |extra: usize| {
        let mut bytes = elf::synthetic(3, elf::EM_X86_64);
        bytes.extend((0..extra).map(|i| (i % 251) as u8));
        bytes
    };
    let files: [(&str, Vec<u8>); 6] = [
        (ANDROID_MANIFEST, axml::synthetic(manifest::PACKAGE, 982606201, "1.26.62.1", None, false, false)),
        ("lib/x86_64/libminecraftpe.so", library(40_000)),
        ("lib/x86_64/libc++_shared.so", library(500)),
        ("assets/resource_packs/vanilla/manifest.json", br#"{"format_version":2}"#.to_vec()),
        ("assets/fonts/Mojangles.ttf", vec![7u8; 3000]),
        ("assets/xboxservices.config", br#"{"TitleId":0}"#.to_vec()),
    ];
    for (name, bytes) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap_or(dir)).ctx(|| format!("creating {}", dir.display()))?;
        std::fs::write(&path, bytes).ctx(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Every imported version, from its manifest (no file is hashed: see [`verify`]).
pub fn list(paths: &Paths) -> Result<Listing> {
    let mut listing = Listing::default();
    let Some(versions) = open_versions_dir(paths, false)? else { return Ok(listing) };
    let fd = nix::fcntl::openat(&versions, ".", DIR_FLAGS, Mode::empty()).ctx(|| "opening the version store")?;
    let mut dir = Dir::from_fd(fd).ctx(|| "reading the version store")?;
    let names: Vec<Vec<u8>> = dir
        .iter()
        .flatten()
        .map(|e| e.file_name().to_bytes().to_vec())
        .filter(|n| !matches!(n.as_slice(), b"." | b".."))
        .take(10_000)
        .collect();
    for raw in names {
        let name = String::from_utf8_lossy(&raw).into_owned();
        if name.starts_with(STAGING_PREFIX) || name.starts_with(TRASH_PREFIX) {
            listing.leftovers.push(shown(&name));
            continue;
        }
        if validate_id(&name).is_err() {
            listing.damaged.push((shown(&name), "not a version id; bastion did not put it here".into()));
            continue;
        }
        let summary = match nix::fcntl::openat(&versions, name.as_str(), DIR_FLAGS, Mode::empty()) {
            Ok(root) => read_manifest(root.as_fd(), &name).map(|m| Summary::new(&name, &m)),
            Err(e) => Err(Error::io("not a directory that can be opened", e.into())),
        };
        match summary {
            Ok(summary) => listing.versions.push(summary),
            Err(e) => listing.damaged.push((name, e.to_string())),
        }
    }
    listing.versions.sort_by(|a, b| a.id.cmp(&b.id));
    listing.damaged.sort();
    listing.leftovers.sort();
    Ok(listing)
}

// ---------------------------------------------------------------- remove

/// The Bedrock version an instance names (`kind = "bedrock"` and `[bedrock] version` in its
/// `instance.toml`, the shape the round-5 contract gives), or `None` for any other instance.
pub fn instance_version(paths: &Paths, instance: &str) -> Result<Option<String>> {
    let file = paths.instance(instance)?.join("instance.toml");
    let raw = util::read_small_file(&file, 64 * 1024)?;
    let text = String::from_utf8(raw).map_err(|_| Error::Format(format!("{} is not UTF-8", file.display())))?;
    let table: toml::Table = toml::from_str(&text).map_err(|e| Error::Format(format!("{}: {e}", file.display())))?;
    if table.get("kind").and_then(toml::Value::as_str) != Some("bedrock") {
        return Ok(None);
    }
    Ok(table.get("bedrock").and_then(|b| b.get("version")).and_then(toml::Value::as_str).map(str::to_string))
}

/// Running launches (by their markers) of Bedrock instances that name `id`, and launches whose
/// instance cannot be read (they might). A Bedrock launch marks itself as one of its own kind
/// (`running::Marker::bedrock`); a launch marker of the Java kind is looked at too, since only
/// the instance's file can tell what it runs.
fn launches_using(paths: &Paths, id: &str) -> Vec<String> {
    use crate::running::Activity;
    let mut found = Vec::new();
    for running in crate::running::list(paths) {
        let (Activity::Bedrock(instance) | Activity::Launch(instance)) = &running.activity else { continue };
        match instance_version(paths, instance) {
            Ok(Some(version)) if version == id => found.push(format!("instance {instance} (process {})", running.pid)),
            Ok(_) => {}
            Err(e) => found.push(format!(
                "instance {instance} (process {}), whose settings cannot be read to tell: {e}",
                running.pid
            )),
        }
    }
    found
}

/// Deletes an imported version. Refused while a launch holds it ([`open`]'s lock) or while a
/// marked launch of a Bedrock instance names it. Returns notes (instances that name it).
pub fn remove(paths: &Paths, id: &str) -> Result<Vec<String>> {
    let (versions, root) = open_root(paths, id)?;
    let users = launches_using(paths, id);
    if !users.is_empty() {
        return Err(Error::Policy(format!(
            "Bedrock version {id} is in use by the running launch of {}; stop it first",
            users.join(", ")
        )));
    }
    let lock = match nix::fcntl::openat(&root, GAME_DIR, DIR_FLAGS, Mode::empty()) {
        Ok(game) => match Flock::lock(game, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => Some(lock),
            Err((_, Errno::EWOULDBLOCK)) => {
                return Err(Error::Policy(format!(
                    "Bedrock version {id} is in use by a running launch (it holds the version open); stop it first"
                )));
            }
            Err((_, e)) => return Err(Error::io(format!("locking Bedrock version {id}"), e.into())),
        },
        // No game directory: nothing can be using it (opening it would fail).
        Err(_) => None,
    };
    let trash = format!("{TRASH_PREFIX}{id}-{}", util::hex_encode(&util::random_bytes::<4>()?));
    nix::fcntl::renameat2(&versions, id, &versions, trash.as_str(), RenameFlags::RENAME_NOREPLACE)
        .ctx(|| format!("moving Bedrock version {id} out of the way"))?;
    let _ = nix::unistd::fsync(&versions);
    // The trash directory, locked so that a sweep elsewhere leaves it to this removal.
    let trash_lock = nix::fcntl::openat(&versions, trash.as_str(), DIR_FLAGS, Mode::empty())
        .ok()
        .and_then(|fd| Flock::lock(fd, FlockArg::LockExclusiveNonblock).ok());
    let path = versions_dir(paths).join(&trash);
    let removed = crate::tree::remove(&path, Some(Instant::now() + Duration::from_secs(600)));
    drop((lock, trash_lock));
    removed.map_err(|e| {
        Error::io(
            format!(
                "removing Bedrock version {id} (what is left is in {}; the next import removes it)",
                path.display()
            ),
            e,
        )
    })?;
    let mut notes = Vec::new();
    if let Ok(instances) = crate::instance::list(paths) {
        for instance in instances.iter().take(10_000) {
            if instance_version(paths, instance).ok().flatten().as_deref() == Some(id) {
                notes.push(format!(
                    "instance {instance} names Bedrock version {id}: it cannot be launched until that version is imported again"
                ));
            }
        }
    }
    Ok(notes)
}

// ---------------------------------------------------------------- mcpelauncher-ui's versions

/// Where a version found in mcpelauncher-ui's directories comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Native,
    Flatpak,
}

/// A version directory of mcpelauncher-ui.
#[derive(Debug, Clone)]
pub struct UiVersion {
    pub path: PathBuf,
    pub origin: Origin,
    /// Version name and code from its `AndroidManifest.xml`, if that reads.
    pub version_name: Option<String>,
    pub version_code: Option<u32>,
    /// It holds `lib/x86_64/libminecraftpe.so`.
    pub x86_64: bool,
}

/// The Flatpak's application id.
pub const UI_FLATPAK_ID: &str = "io.mrarm.mcpelauncher";

/// The version directories mcpelauncher-ui keeps: `<data_home or ~/.local/share>/mcpelauncher/
/// versions/*` (the native package) and `~/.var/app/io.mrarm.mcpelauncher/data/mcpelauncher/
/// versions/*` (the Flatpak). Its own temporary extraction directories (`temp-*`) and links
/// are left out. Nothing is copied or hashed here.
pub fn ui_version_dirs(home: &Path, data_home: Option<&Path>) -> Vec<UiVersion> {
    let native =
        data_home.map(Path::to_path_buf).unwrap_or_else(|| home.join(".local/share")).join("mcpelauncher/versions");
    let flatpak = home.join(".var/app").join(UI_FLATPAK_ID).join("data/mcpelauncher/versions");
    let mut found = Vec::new();
    for (base, origin) in [(native, Origin::Native), (flatpak, Origin::Flatpak)] {
        let Ok(entries) = std::fs::read_dir(&base) else { continue };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .take(1000)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|p| {
                p.file_name().and_then(OsStr::to_str).is_some_and(|n| !n.starts_with("temp-") && !n.starts_with('.'))
            })
            .collect();
        dirs.sort();
        for path in dirs {
            let manifest_path = path.join(ANDROID_MANIFEST);
            if std::fs::symlink_metadata(&manifest_path).is_err() {
                continue;
            }
            let info = util::read_small_file(&manifest_path, axml::MAX_BYTES as u64)
                .ok()
                .and_then(|data| axml::parse(&data).ok());
            let x86_64 =
                std::fs::symlink_metadata(path.join("lib/x86_64").join(MINECRAFT_LIBRARY)).is_ok_and(|m| m.is_file());
            found.push(UiVersion {
                path,
                origin,
                version_name: info.as_ref().map(|i| i.version_name.clone()),
                version_code: info.as_ref().map(|i| i.version_code),
                x86_64,
            });
        }
    }
    found
}

/// [`ui_version_dirs`] for this user: `$HOME` (or the password database) and `$XDG_DATA_HOME`.
pub fn ui_version_dirs_for_user() -> Vec<UiVersion> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| nix::unistd::User::from_uid(nix::unistd::getuid()).ok().flatten().map(|u| u.dir));
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
    home.map(|home| ui_version_dirs(&home, data_home.as_deref())).unwrap_or_default()
}

/// What `bastion-mc bedrock versions` prints.
pub fn listing_lines(listing: &Listing, ui: &[UiVersion]) -> Vec<String> {
    let mut lines = Vec::new();
    if listing.versions.is_empty() {
        lines.push("no Bedrock versions imported yet".to_string());
    }
    for v in &listing.versions {
        lines.push(format!(
            "{}\tMinecraft {} (version code {})\t{} files, {} MiB",
            v.id,
            shown(&v.version_name),
            v.version_code,
            v.files,
            v.bytes >> 20
        ));
    }
    for (name, why) in &listing.damaged {
        lines.push(format!("{}\tNOT USABLE: {}", shown(name), util::sanitize_line(why.as_bytes(), 300)));
    }
    if !listing.leftovers.is_empty() {
        lines.push(format!(
            "{} unfinished import(s) or removal(s) in the store; the next import or removal cleans them up",
            listing.leftovers.len()
        ));
    }
    let new: Vec<&UiVersion> =
        ui.iter().filter(|u| !listing.versions.iter().any(|v| Some(v.version_code) == u.version_code)).collect();
    if !new.is_empty() {
        lines.push(String::new());
        lines.push(
            "mcpelauncher-ui has these versions, not imported (bastion-mc bedrock import <directory>):".to_string(),
        );
        for u in new {
            let what = match (&u.version_name, u.version_code) {
                (Some(name), Some(code)) => format!("Minecraft {} (version code {code})", shown(name)),
                _ => "an AndroidManifest.xml that does not read".to_string(),
            };
            let note = match (u.x86_64, u.origin) {
                (false, _) => ", no x86_64 libraries: cannot be imported",
                (true, Origin::Flatpak) => ", from the Flatpak",
                (true, Origin::Native) => "",
            };
            lines.push(format!("  {}\t{what}{note}", shown(&u.path.to_string_lossy())));
        }
    }
    lines
}

// ---------------------------------------------------------------- fuzzing

/// The parsers of imported bytes: the version manifest (strict, and its own output reads back
/// as itself, byte for byte), the `AndroidManifest.xml` reader (bounded, printable results) and
/// the ELF header checks, plus the pacman records `runtime` reads — whatever the input, an
/// answer and no panic.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    if let Ok(m) = manifest::parse(data) {
        assert!(m.entries.len() <= manifest::MAX_ENTRIES);
        let again = manifest::encode(&m);
        assert_eq!(manifest::parse(&again).expect("own output"), m, "not a fixed point");
        assert_eq!(again.as_slice(), data, "two spellings of one manifest");
    }
    if let Ok(info) = axml::parse(data) {
        assert!(!info.package.is_empty() && info.package.len() <= 255);
        assert!(!info.version_name.chars().any(char::is_control));
        assert!(info.version_code > 0);
    }
    let _ = elf::check_x86_64_library(data);
    let _ = elf::check_x86_64_program(data);
    crate::bedrock::runtime::fuzz_pacman(data);
}

/// Valid inputs for [`fuzz`]: a manifest, compiled `AndroidManifest.xml` files in both string
/// encodings (one with stripped attribute names, one of a split), an ELF header, and pacman
/// database records.
#[doc(hidden)]
pub fn fuzz_seeds() -> Vec<Vec<u8>> {
    let sha = |byte: u8| [byte; 32];
    let manifest = Manifest {
        package: manifest::PACKAGE.into(),
        version_name: "1.26.62.1".into(),
        version_code: 982606201,
        imported: 1_794_617_499,
        entries: vec![
            Entry { path: "AndroidManifest.xml".into(), kind: Kind::File { size: 2048, sha256: sha(1) } },
            Entry { path: "assets".into(), kind: Kind::Dir },
            Entry { path: "assets/a b.json".into(), kind: Kind::File { size: 7, sha256: sha(2) } },
            Entry { path: "lib".into(), kind: Kind::Dir },
            Entry { path: "lib/x86_64".into(), kind: Kind::Dir },
            Entry { path: "lib/x86_64/libminecraftpe.so".into(), kind: Kind::File { size: 99, sha256: sha(3) } },
        ],
    };
    vec![
        manifest::encode(&manifest),
        axml::synthetic(manifest::PACKAGE, 982606201, "1.26.62.1", None, false, false),
        axml::synthetic(manifest::PACKAGE, 982607024, "1.26.70.24", None, true, true),
        axml::synthetic(manifest::PACKAGE, 1, "1.0", Some("config.x86_64"), false, false),
        elf::synthetic(3, elf::EM_X86_64),
        crate::bedrock::runtime::fuzz_seed_pacman(),
    ]
}

#[cfg(test)]
mod tests;
