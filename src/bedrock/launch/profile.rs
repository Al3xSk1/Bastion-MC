//! The sandbox of a Bedrock session: what `mcpelauncher-client` sees, may write, may run,
//! and where the game's data lives (contract: `docs/notes/round5-contract.md`, "The sandbox").
//!
//! ```text
//! /usr, /etc (synthetic), /proc, /dev, /tmp, /run/user/1000   as for Java (`sandbox::base`)
//! /bastion/game        the imported version, read-only, bound by the descriptor the store checked
//! /bastion/data        THIS SESSION's data root: a fresh directory, deleted when the session ends
//!   games/com.mojang/minecraftWorlds   ┐ the instance's kept part (`persist_worlds`):
//!   games/com.mojang/minecraftpe       │ bound read-write from instances/<id>/bedrock-data/
//!   mcpelauncher-client-settings.txt   ┘
//!   mods/              empty and read-only: the client's mod loader finds nothing, ever
//! /bastion/cache       tmpfs (the client's -dc)
//! /bastion/bin/mcpelauncher-webview    the sign-in bridge's stub (the launcher's own executable)
//! <every installed webview path>       the same stub, over the installed program
//! /bastion/run/signin.sock             the bridge's socket
//! <client program>, <its library dirs> read-only at their own paths
//! ```
//!
//! **Why the data root is per session and only named parts are kept.** The client hands the
//! game a single directory for everything Android would keep apart — files, cache, external
//! storage, and the storage of the Xbox sign-in library (`Interop.GetLocalStoragePath`,
//! `LocalStorage.getPath`, `Context.getFilesDir`, `/data/data/com.mojang.minecraftpe`: all
//! the `-dd` directory, see `src/jni/` of the client). Where in that directory the game's
//! sign-in library keeps its tokens is the game's choice, not the client's: older versions
//! used `games/com.mojang/xal/`, newer ones stopped writing that folder (Mojang's report
//! MCPE-178354) and keep it elsewhere in internal storage. A list of token paths to wipe
//! would fail open the day the game moves them again. So the whole data root is the token
//! store: everything the session writes is deleted at its end, except the two folders and
//! one file the user wants kept. The same rule closes a second door: the client looks for
//! its own helper libraries (`lib/<abi>/*.so`, through `PathHelper::findDataFile`) and for
//! native mods (`mods/`) in the data directory *first* — a session that could leave a
//! library there would have it loaded into the next one. With a fresh root it cannot.
//!
//! **What is not guaranteed.** That nothing in the kept folders is sign-in material: the
//! game's layout there is known from reports, not from a game file (there is none on this
//! machine). A sign-in that the next session does not ask for again would be the symptom.
//! And `mmap(PROT_EXEC)` of a file the session wrote cannot be stopped (Landlock governs
//! `execve`, not mappings; the client's own linker maps the game's libraries that way): code
//! already running in the client can map what it wrote. What Landlock does stop is any
//! `execve` of it, and the fresh root keeps it from being loaded at the next start.

use std::{
    fs::File,
    os::{
        fd::OwnedFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use nix::fcntl::{Flock, FlockArg};

use crate::{
    bedrock::{runtime::Client, signin},
    error::{Context, Error, Result},
    instance,
    paths::Paths,
    running::{self, DirLock},
    sandbox::{Base, Mount, policy::LandlockPolicy},
    util,
};

pub const SANDBOX_GAME: &str = "/bastion/game";
pub const SANDBOX_DATA: &str = "/bastion/data";
pub const SANDBOX_CACHE: &str = "/bastion/cache";
pub const SANDBOX_BIN: &str = "/bastion/bin";
/// The client's mod directory (`PathHelper::getPrimaryDataDirectory() + "mods/"`).
pub const MODS_DIR: &str = "mods";
/// The client's own settings (window, scale, overlay), kept with the worlds.
pub const CLIENT_SETTINGS: &str = "mcpelauncher-client-settings.txt";
/// What is kept from one session to the next, relative to the data root: the worlds and
/// the game's own settings (`options.txt` and its neighbours). Nothing else survives.
pub const KEPT_DIRS: [&str; 2] = ["games/com.mojang/minecraftWorlds", "games/com.mojang/minecraftpe"];
/// The tmpfs at `/bastion/cache` (the client's telemetry cache, Mesa's shader cache).
pub const CACHE_SIZE_MB: u32 = 512;
/// Separates the instance id from the rest of a session directory's name. Not in the id
/// alphabet (`paths::validate_id`), so a Java launch's game root (`<id>-<hex>`) can never
/// look like a Bedrock session of another instance.
const SESSION_MARK: &str = "+bedrock-";

// ---------------------------------------------------------------- the data split

/// The instance's kept data, opened for one session.
pub struct Kept {
    /// `instances/<id>/bedrock-data/`, locked for the session: two sessions of one instance
    /// would write the same worlds (a LevelDB database is not made for that).
    _lock: Flock<File>,
    /// Descriptors of the kept directories and of the client's settings file, in the order
    /// of [`KEPT_DIRS`] then [`CLIENT_SETTINGS`]: what was checked is what gets mounted.
    dirs: Vec<(String, OwnedFd)>,
}

impl Kept {
    /// Lock the instance's data directory (created if missing) and open what is kept;
    /// `persist` false keeps nothing (the lock is still taken).
    pub fn open(inst_dir: &Path, id: &str, persist: bool) -> Result<Kept> {
        let root = inst_dir.join(super::instance::DATA_DIR);
        instance::ensure_plain_dir(&root)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .ctx(|| format!("opening {}", root.display()))?;
        let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_, errno)| match errno {
            nix::errno::Errno::EWOULDBLOCK => Error::Usage(format!(
                "the Bedrock instance {id:?} is already running (another launch holds {})",
                root.display()
            )),
            other => Error::io(format!("locking {}", root.display()), std::io::Error::from(other)),
        })?;
        let mut dirs = Vec::new();
        if persist {
            for rel in KEPT_DIRS {
                // Each component a plain, private directory of ours: a link anywhere on the
                // way would point the game's bind mount somewhere else.
                let mut path = root.clone();
                for part in rel.split('/') {
                    path.push(part);
                    instance::ensure_plain_dir(&path)?;
                }
                dirs.push((rel.to_string(), open_path(&path, true)?));
            }
            let settings = root.join(CLIENT_SETTINGS);
            match std::fs::symlink_metadata(&settings) {
                Ok(meta) if meta.file_type().is_file() => {}
                Ok(_) => {
                    return Err(Error::Policy(format!("{} must be a regular file", settings.display())));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(&settings)
                        .ctx(|| format!("creating {}", settings.display()))?;
                }
                Err(e) => return Err(Error::io(format!("inspecting {}", settings.display()), e)),
            }
            dirs.push((CLIENT_SETTINGS.to_string(), open_path(&settings, false)?));
        }
        Ok(Kept { _lock: lock, dirs })
    }

    /// Whether anything is kept.
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }

    fn mounts(&mut self) -> Vec<Mount> {
        std::mem::take(&mut self.dirs)
            .into_iter()
            .map(|(rel, dir)| Mount::BindFd { dir, dst: Path::new(SANDBOX_DATA).join(rel) })
            .collect()
    }
}

/// `O_PATH | O_NOFOLLOW` (and `O_DIRECTORY` for a directory): a handle to bind, nothing more.
fn open_path(path: &Path, directory: bool) -> Result<OwnedFd> {
    use nix::{fcntl::OFlag, sys::stat::Mode};
    let mut flags = OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    if directory {
        flags |= OFlag::O_DIRECTORY;
    }
    nix::fcntl::open(path, flags, Mode::empty())
        .map_err(|e| Error::io(format!("opening {}", path.display()), std::io::Error::from(e)))
}

/// One session's data root on the host: `<cache>/scratch/<id>+bedrock-<hex>`, the token store
/// of the game (see the module header). Locked while it exists, so that no sweep takes it
/// from under a running session; deleted at the end ([`Session::remove`]), and if a session
/// could not do that (it was killed, the machine lost power), by the next start of the same
/// instance whatever its age ([`sweep_instance`]), by any later launch after a minute
/// (`running::sweep_leftovers`), and by `bastion-mc scram`.
pub struct Session {
    dir: PathBuf,
    lock: Option<DirLock>,
    removed: bool,
}

impl Session {
    pub fn create(paths: &Paths, id: &str) -> Result<Session> {
        let base = paths.scratch();
        util::create_private_dir_all(&base)?;
        let dir = base.join(format!("{id}{SESSION_MARK}{}", util::hex_encode(&util::random_bytes::<8>()?)));
        util::create_private_dir_all(&dir)?;
        let lock = running::lock_dir(&dir).ctx(|| format!("locking {}", dir.display()))?;
        Ok(Session { dir, lock: Some(lock), removed: false })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// The data root as a mount (by descriptor: the directory created is the one bound).
    pub fn mount(&self) -> Result<Mount> {
        Ok(Mount::BindFd { dir: open_path(&self.dir, true)?, dst: SANDBOX_DATA.into() })
    }

    /// Delete the session's data — the game's sign-in tokens with it. The tree was written
    /// by the sandbox: removed the careful way (`tree::remove`, bounded in time), and what
    /// could not be removed is said, never passed over.
    pub fn remove(&mut self, deadline: std::time::Instant) -> std::result::Result<(), String> {
        self.removed = true;
        self.lock = None;
        match crate::tree::remove(&self.dir, Some(deadline)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", self.dir.display())),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A launch that ends some other way (an error, a cancelled start): best effort; what
        // stays is removed by the next start of this instance.
        if !self.removed {
            let _ = self.remove(std::time::Instant::now() + std::time::Duration::from_secs(30));
        }
    }
}

/// Is `name` the directory name of a session of instance `id`?
fn is_session_of(name: &str, id: &str) -> bool {
    name.strip_prefix(id)
        .and_then(|rest| rest.strip_prefix(SESSION_MARK))
        .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// What [`sweep_instance`] did.
#[derive(Debug, Default)]
pub struct Swept {
    pub removed: usize,
    pub failed: Vec<String>,
}

/// Delete what earlier sessions of instance `id` left (sessions that were killed or
/// crashed): their data roots hold the game's sign-in. Whatever their age — the caller holds
/// the instance's lock ([`Kept::open`]), so none of them belongs to a running session. A
/// directory that some process still holds locked is left alone and reported.
pub fn sweep_instance(paths: &Paths, id: &str, deadline: std::time::Instant) -> Swept {
    let mut swept = Swept::default();
    let Ok(entries) = std::fs::read_dir(paths.scratch()) else { return swept };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        if !is_session_of(&name, id) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = entry.path();
        // Taking the lock is the test for "nobody holds it"; it is dropped right after.
        let free = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&path)
            .ok()
            .and_then(|file| Flock::lock(file, FlockArg::LockExclusiveNonblock).ok());
        if free.is_none() {
            swept.failed.push(format!("{name}: held by a running process"));
            continue;
        }
        drop(free);
        match crate::tree::remove(&path, Some(deadline)) {
            Ok(()) => swept.removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => swept.failed.push(format!("{name}: {e}")),
        }
    }
    swept
}

/// Every session data root present now, of any instance (for `scram`, which deletes them).
pub fn session_dirs(paths: &Paths) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(paths.scratch()) else { return Vec::new() };
    entries
        .flatten()
        .filter(|entry| {
            entry.file_name().to_str().and_then(|name| name.split_once(SESSION_MARK)).is_some_and(|(id, hex)| {
                crate::paths::validate_id(id).is_ok() && is_session_of(&format!("{id}{SESSION_MARK}{hex}"), id)
            })
        })
        .map(|entry| entry.path())
        .collect()
}

// ---------------------------------------------------------------- the profile

/// The sign-in stub: the launcher's own executable, opened once per place it is mounted
/// (each mount consumes its descriptor).
pub struct Stub {
    /// The file for `/bastion/bin/mcpelauncher-webview`.
    pub bin: File,
    /// One per installed webview path to cover, with that path.
    pub covers: Vec<(PathBuf, File)>,
}

impl Stub {
    /// Open `program` (this executable) for the stub's own path and for every path in
    /// `webview_paths` — the installed webviews (`Client::installed_webviews`: the places that
    /// exist, links resolved to the files they lead to, since a bind mount follows a link in
    /// its destination). A path that does not exist on the host does not exist in the sandbox
    /// either (everything outside the generated directories is a read-only view of the host),
    /// so there is nothing there for the client to find.
    pub fn open(program: &Path, webview_paths: &[PathBuf]) -> Result<Stub> {
        let open = || File::open(program).ctx(|| format!("opening {}", program.display()));
        let mut covers = Vec::new();
        for path in webview_paths {
            if !path.is_absolute() {
                return Err(Error::Policy(format!("webview path {} is not absolute", path.display())));
            }
            if std::fs::symlink_metadata(path).is_ok() {
                covers.push((path.clone(), open()?));
            }
        }
        Ok(Stub { bin: open()?, covers })
    }
}

/// What the profile is built from.
pub struct Inputs<'a> {
    pub client: &'a Client,
    /// The client's executable, opened (bound by this descriptor at its own path).
    pub client_file: File,
    /// The imported version's directory (`OpenedVersion::dir`).
    pub version_dir: OwnedFd,
    pub session: &'a Session,
    /// Borrowed, not taken: its lock must outlive the session, not the building of the profile.
    pub kept: &'a mut Kept,
    pub stub: Stub,
    /// The bridge's socket on the host, bound at `signin::SANDBOX_SOCKET`.
    pub bridge_socket: Option<PathBuf>,
    /// `-ww` / `-wh`, from the instance's `width` and `height`.
    pub window: Option<(u32, u32)>,
}

/// The program the sandbox runs, and everything that goes with it.
pub struct Profile {
    pub exec: PathBuf,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub mounts: Vec<Mount>,
    pub landlock: LandlockPolicy,
    pub cwd: PathBuf,
}

/// Directories the sandbox makes itself (`/tmp`, `/dev/shm` and the runtime directory are
/// the game's to write; `/bastion` holds the data root). Nothing the client executes may lie
/// below them: the game could put files beside it, where the client looks for its own (its
/// data files are found relative to its directory). The runtime package's checks already
/// keep an installed client out of such places on the host; this keeps it so in here.
const SANDBOX_MADE: [&str; 6] = ["/tmp", "/dev", "/proc", "/run", "/bastion", "/etc"];

fn outside_sandbox_made(what: &str, path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::Policy(format!("{what} {} is not an absolute path", path.display())));
    }
    if let Some(dir) = SANDBOX_MADE.iter().find(|dir| path.starts_with(dir)) {
        return Err(Error::Policy(format!(
            "{what} {} lies below {dir}, which the sandbox makes itself: it must be installed elsewhere",
            path.display()
        )));
    }
    Ok(())
}

/// Put the Bedrock profile on top of the base system view (`sandbox::base`, built without a
/// JVM and without `exec_allow`). Display, audio and network are added by the launch.
pub fn build(base: Base, mut input: Inputs<'_>) -> Result<Profile> {
    let Base { mut mounts, mut landlock, mut env } = base;
    let program = &input.client.program;
    outside_sandbox_made("the client", program)?;
    for dir in &input.client.lib_dirs {
        outside_sandbox_made("the client's library directory", dir)?;
    }
    for (path, _) in &input.stub.covers {
        outside_sandbox_made("the webview", path)?;
    }

    // The game, read-only; the data root, then what is kept inside it, then the empty and
    // sealed mod directory (after the kept binds, so nothing can come to lie over it).
    mounts.push(Mount::RoBindFd { file: File::from(input.version_dir), dst: SANDBOX_GAME.into() });
    mounts.push(input.session.mount()?);
    mounts.extend(input.kept.mounts());
    let mods = Path::new(SANDBOX_DATA).join(MODS_DIR);
    mounts.push(Mount::Tmpfs { dst: mods.clone(), size_bytes: Some(4096), mode: 0o555 });
    mounts.push(Mount::RemountRo { dst: mods });
    mounts.push(Mount::Tmpfs {
        dst: SANDBOX_CACHE.into(),
        size_bytes: Some(u64::from(CACHE_SIZE_MB) << 20),
        mode: 0o700,
    });

    // The client at its own path (it finds its data files relative to its directory), then
    // its libraries; the stub last, so that no client mount can come to lie over it.
    mounts.push(Mount::RoBindFd { file: input.client_file, dst: program.clone() });
    for dir in &input.client.lib_dirs {
        mounts.push(Mount::RoBind { src: dir.clone(), dst: dir.clone() });
        landlock.read_exec.push(dir.clone());
    }
    landlock.read_exec.push(program.clone());
    let stub_path = PathBuf::from(signin::SANDBOX_STUB);
    mounts.push(Mount::RoBindFd { file: input.stub.bin, dst: stub_path.clone() });
    landlock.read_exec.push(stub_path);
    for (path, file) in std::mem::take(&mut input.stub.covers) {
        mounts.push(Mount::RoBindFd { file, dst: path.clone() });
        landlock.read_exec.push(path);
    }
    if let Some(socket) = &input.bridge_socket {
        // The socket file itself, not its directory: once the sandbox holds the mount, the
        // name on the host can go (see `bedrock::launch`), and nothing else can connect.
        let dst = PathBuf::from(signin::SANDBOX_SOCKET);
        mounts.push(Mount::RoBind { src: socket.clone(), dst: dst.clone() });
        landlock.unix_sockets.push(dst);
    }

    // Writable: the data root and the cache (plus the base's /tmp, /dev/shm and runtime
    // directory). Never executable: no write grant carries Landlock's execute right.
    landlock.write.push(SANDBOX_DATA.into());
    landlock.write.push(SANDBOX_CACHE.into());
    landlock.scope_abstract_unix = true;

    // The environment, from scratch (the base's), with the stub's directory first in PATH and
    // HOME inside the data root, so that whatever writes below HOME is part of the session.
    // PATH is exactly the stub's directory and the store package's `SANDBOX_PATH_DIRS` — the
    // directories its discovery searched for installed webviews — with no empty entry (the
    // client's PATH search takes an empty entry for the working directory). XDG_DATA_DIRS
    // stays unset: the client then searches only /usr/local/share and /usr/share (read-only).
    set(&mut env, "PATH", &sandbox_path());
    set(&mut env, "HOME", SANDBOX_DATA);
    set(&mut env, "XDG_CACHE_HOME", SANDBOX_CACHE);

    // The client's arguments, as its argument parser names them (`main.cpp`: --game-dir -dg,
    // --data-dir -dd, --cache-dir -dc, --width -ww, --height -wh). Not --free-only (it refuses
    // the full game) and never -m (mod directories).
    let mut argv = vec![
        program.display().to_string(),
        "-dg".into(),
        SANDBOX_GAME.into(),
        "-dd".into(),
        SANDBOX_DATA.into(),
        "-dc".into(),
        SANDBOX_CACHE.into(),
    ];
    if let Some((width, height)) = input.window {
        argv.extend(["-ww".into(), width.to_string(), "-wh".into(), height.to_string()]);
    }
    // The working directory is read-only and holds nothing the client could take for its own
    // (`PathHelper` takes a working directory that holds `lib/<abi>/libminecraftpe.so` for its
    // data directory): `/`, as on Android, where an app's working directory is `/` as well.
    Ok(Profile { exec: program.clone(), argv, env, mounts, landlock, cwd: SANDBOX_CWD.into() })
}

/// The client's working directory (see [`build`]).
pub const SANDBOX_CWD: &str = "/";

/// The client's `PATH`: the stub's directory, then the store package's
/// `runtime::SANDBOX_PATH_DIRS`.
pub fn sandbox_path() -> String {
    std::iter::once(SANDBOX_BIN.to_string())
        .chain(crate::bedrock::runtime::SANDBOX_PATH_DIRS.iter().map(|dir| format!("/{dir}")))
        .collect::<Vec<_>>()
        .join(":")
}

fn set(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    env.retain(|(k, _)| k != key);
    env.push((key.to_string(), value.to_string()));
}

/// The prefix of a bridge socket's directory (`signin-<12 hex digits>`).
const BRIDGE_PREFIX: &str = "signin-";
/// How old a leftover bridge socket must be before a later launch removes it.
const BRIDGE_LEFTOVER_AGE: std::time::Duration = std::time::Duration::from_secs(60);

/// A private directory for the bridge socket, with a path short enough for a socket (under
/// the session's runtime directory when there is one). The scheme is the companion's.
pub fn bridge_dir(paths: &Paths) -> Result<PathBuf> {
    const MAX_SOCKET_PATH: usize = 107;
    let base = paths.runtime.as_deref().map(|r| r.join(crate::paths::APP)).unwrap_or_else(|| paths.cache.clone());
    sweep_bridge_dirs(&base);
    let dir = base.join(format!("{BRIDGE_PREFIX}{}", util::hex_encode(&util::random_bytes::<6>()?)));
    let socket = dir.join(signin::SOCKET_NAME);
    if socket.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(Error::Sandbox(format!(
            "{} is too long for a socket (more than {MAX_SOCKET_PATH} bytes); set XDG_RUNTIME_DIR",
            socket.display()
        )));
    }
    util::create_private_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .ctx(|| format!("restricting {}", dir.display()))?;
    Ok(dir)
}

/// Remove what launches that were killed left of their bridge: a launch removes the socket's
/// name itself once the sandbox holds it, so a name that is still there belongs to a launch
/// that never got that far. Only `signin-<12 hex>` directories, only a socket older than a
/// minute, and only one nobody listens on any more (connecting is refused); the directory
/// goes only if it is then empty.
fn sweep_bridge_dirs(base: &Path) {
    use std::os::unix::fs::FileTypeExt;
    const MAX_SWEPT: usize = 256;
    let Ok(entries) = std::fs::read_dir(base) else { return };
    for entry in entries.flatten().take(MAX_SWEPT) {
        let ours = entry.file_name().to_str().and_then(|name| name.strip_prefix(BRIDGE_PREFIX)).is_some_and(|rest| {
            rest.len() == 12 && rest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        let (dir, socket) = (entry.path(), entry.path().join(signin::SOCKET_NAME));
        let is_dir = std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_dir());
        let old_socket = std::fs::symlink_metadata(&socket).is_ok_and(|meta| {
            meta.file_type().is_socket()
                && meta.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= BRIDGE_LEFTOVER_AGE)
        });
        if !ours || !is_dir || !old_socket {
            continue;
        }
        let refused = matches!(
            std::os::unix::net::UnixStream::connect(&socket),
            Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED)
        );
        if refused && std::fs::remove_file(&socket).is_ok() {
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::{BaseOptions, Resolv};

    struct Fixture {
        root: PathBuf,
        paths: Paths,
        inst_dir: PathBuf,
        client: Client,
        version: PathBuf,
        webview: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            // Not below /tmp: inside the sandbox that is the game's, and the profile refuses a
            // client there (see the last test).
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("bastion-bprof-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
            let paths = Paths {
                config: root.join("config"),
                data: root.join("data"),
                cache: root.join("cache"),
                state: root.join("state"),
                runtime: None,
            };
            paths.ensure().unwrap();
            let inst_dir = paths.instance("bedrock").unwrap();
            util::create_private_dir_all(&inst_dir).unwrap();
            let bin = root.join("client/bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::create_dir_all(root.join("client/lib")).unwrap();
            std::fs::write(bin.join("mcpelauncher-client"), b"client").unwrap();
            let webview = bin.join("mcpelauncher-webview");
            std::fs::write(&webview, b"installed webview").unwrap();
            let version = root.join("version");
            std::fs::create_dir_all(version.join("lib/x86_64")).unwrap();
            let client = Client {
                program: bin.join("mcpelauncher-client"),
                lib_dirs: vec![root.join("client/lib")],
                webview_paths: vec![webview.clone(), root.join("not-installed/mcpelauncher-webview")],
                version: None,
            };
            Fixture { root, paths, inst_dir, client, version, webview }
        }

        fn build(&self, session: &Session, kept: &mut Kept, window: Option<(u32, u32)>) -> Profile {
            let base = crate::sandbox::base(&BaseOptions {
                tmp_size_mb: 64,
                jvm: None,
                exec_allow: &[],
                resolv: Resolv::None,
                expose_timezone: false,
                trust: None,
            })
            .unwrap();
            let program = self.root.join("launcher");
            std::fs::write(&program, b"launcher").unwrap();
            build(
                base,
                Inputs {
                    client: &self.client,
                    client_file: File::open(&self.client.program).unwrap(),
                    version_dir: open_path(&self.version, true).unwrap(),
                    session,
                    kept,
                    stub: Stub::open(&program, &self.client.installed_webviews()).unwrap(),
                    bridge_socket: Some(self.root.join("signin.sock")),
                    window,
                },
            )
            .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn dst(mount: &Mount) -> (&'static str, PathBuf) {
        match mount {
            Mount::RoBind { dst, .. } => ("ro-bind", dst.clone()),
            Mount::RoBindTry { dst, .. } => ("ro-bind-try", dst.clone()),
            Mount::Bind { dst, .. } => ("bind", dst.clone()),
            Mount::DevBind { dst, .. } => ("dev-bind", dst.clone()),
            Mount::RoBindFd { dst, .. } => ("ro-bind-fd", dst.clone()),
            Mount::BindFd { dst, .. } => ("bind-fd", dst.clone()),
            Mount::RoData { dst, .. } => ("ro-data", dst.clone()),
            Mount::Tmpfs { dst, .. } => ("tmpfs", dst.clone()),
            Mount::Dir { dst, .. } => ("dir", dst.clone()),
            Mount::Symlink { link, .. } => ("symlink", link.clone()),
            Mount::RemountRo { dst } => ("remount-ro", dst.clone()),
            Mount::Proc { dst } => ("proc", dst.clone()),
            Mount::Dev { dst } => ("dev", dst.clone()),
            Mount::RoOverlay { dst, .. } => ("ro-overlay", dst.clone()),
        }
    }

    #[test]
    fn the_game_is_read_only_the_data_root_is_the_sessions_and_the_mod_directory_is_sealed() {
        let f = Fixture::new("mounts");
        let session = Session::create(&f.paths, "bedrock").unwrap();
        let mut kept = Kept::open(&f.inst_dir, "bedrock", true).unwrap();
        let profile = f.build(&session, &mut kept, None);
        // The instance stays locked for as long as the session holds `kept`, not just while
        // the profile is built.
        assert!(matches!(Kept::open(&f.inst_dir, "bedrock", true), Err(Error::Usage(_))), "the lock went early");
        drop(kept);
        let mounts: Vec<(&str, PathBuf)> = profile.mounts.iter().map(dst).collect();
        let at = |kind: &str, path: &str| {
            mounts.iter().position(|(k, d)| *k == kind && d == Path::new(path)).unwrap_or_else(|| {
                panic!("no {kind} at {path}: {mounts:?}");
            })
        };
        at("ro-bind-fd", SANDBOX_GAME);
        let data = at("bind-fd", SANDBOX_DATA);
        let worlds = at("bind-fd", "/bastion/data/games/com.mojang/minecraftWorlds");
        let options = at("bind-fd", "/bastion/data/games/com.mojang/minecraftpe");
        let settings = at("bind-fd", "/bastion/data/mcpelauncher-client-settings.txt");
        let mods = at("tmpfs", "/bastion/data/mods");
        let sealed = at("remount-ro", "/bastion/data/mods");
        assert!(data < worlds && worlds < options && options < settings && settings < mods && mods < sealed);
        at("tmpfs", SANDBOX_CACHE);
        // The client at its own path, then its libraries, then the stub — over the installed
        // webview too, and after the client's mounts so that none of them lies over it.
        let client = at("ro-bind-fd", &f.client.program.display().to_string());
        let lib = at("ro-bind", &f.client.lib_dirs[0].display().to_string());
        let stub = at("ro-bind-fd", signin::SANDBOX_STUB);
        let cover = at("ro-bind-fd", &f.webview.display().to_string());
        assert!(client < lib && lib < stub && stub < cover);
        assert!(!mounts.iter().any(|(_, d)| d.starts_with(f.root.join("not-installed"))), "nothing to cover there");
        at("ro-bind", signin::SANDBOX_SOCKET);
        // Nothing of the host's home, of the instance, or of the launcher's data is mounted.
        for (_, d) in &mounts {
            assert!(!d.starts_with(&f.paths.data) && !d.starts_with(&f.inst_dir), "{d:?}");
        }
    }

    #[test]
    fn nothing_writable_is_executable_and_the_stub_and_client_are() {
        let f = Fixture::new("landlock");
        let session = Session::create(&f.paths, "bedrock").unwrap();
        let profile = f.build(&session, &mut Kept::open(&f.inst_dir, "bedrock", true).unwrap(), None);
        let l = &profile.landlock;
        for writable in [SANDBOX_DATA, SANDBOX_CACHE, "/tmp", "/dev/shm", crate::sandbox::RUNTIME_DIR] {
            assert!(l.write.iter().any(|w| w == Path::new(writable)), "{writable} writable");
            assert!(
                !l.read_exec.iter().any(|x| x.starts_with(writable) || Path::new(writable).starts_with(x)),
                "{writable} must not be executable: {:?}",
                l.read_exec
            );
        }
        for exec in
            [f.client.program.clone(), f.client.lib_dirs[0].clone(), signin::SANDBOX_STUB.into(), f.webview.clone()]
        {
            assert!(l.read_exec.contains(&exec), "{exec:?}: {:?}", l.read_exec);
        }
        assert!(l.unix_sockets.contains(&PathBuf::from(signin::SANDBOX_SOCKET)));
        assert!(l.scope_abstract_unix);
    }

    #[test]
    fn the_client_gets_its_arguments_and_an_environment_from_scratch() {
        let f = Fixture::new("argv");
        let session = Session::create(&f.paths, "bedrock").unwrap();
        let profile = f.build(&session, &mut Kept::open(&f.inst_dir, "bedrock", false).unwrap(), Some((1280, 720)));
        let program = f.client.program.display().to_string();
        assert_eq!(profile.exec, f.client.program);
        assert_eq!(
            profile.argv,
            [
                &program,
                "-dg",
                "/bastion/game",
                "-dd",
                "/bastion/data",
                "-dc",
                "/bastion/cache",
                "-ww",
                "1280",
                "-wh",
                "720"
            ]
        );
        assert_eq!(profile.cwd, Path::new("/"), "read-only, and nothing the client takes for its own");
        let get = |k: &str| {
            let all: Vec<&String> = profile.env.iter().filter(|(key, _)| key == k).map(|(_, v)| v).collect();
            assert!(all.len() <= 1, "{k} set twice: {all:?}");
            all.first().map(|v| v.as_str())
        };
        assert_eq!(get("PATH"), Some("/bastion/bin:/usr/bin"), "the stub's directory first");
        assert!(sandbox_path().split(':').all(|d| d.starts_with('/') && d.len() > 1), "no empty or relative entry");
        assert_eq!(get("XDG_DATA_DIRS"), None, "the client's data search stays on /usr");
        assert_eq!(get("XDG_DATA_HOME"), None);
        assert_eq!(get("HOME"), Some(SANDBOX_DATA));
        assert_eq!(get("XDG_CACHE_HOME"), Some(SANDBOX_CACHE));
        assert_eq!(get("DBUS_SESSION_BUS_ADDRESS"), None);
        assert_eq!(get("DISPLAY"), None, "the display comes from the desktop plan, not from here");
        // Without persistence nothing of the instance is mounted, but the mod directory is
        // still sealed.
        let mounts: Vec<(&str, PathBuf)> = profile.mounts.iter().map(dst).collect();
        assert!(!mounts.iter().any(|(_, d)| d.starts_with("/bastion/data/games")));
        assert!(mounts.iter().any(|(k, d)| *k == "remount-ro" && d == Path::new("/bastion/data/mods")));
    }

    #[test]
    fn one_instance_runs_once_at_a_time_and_its_kept_directories_must_be_plain() {
        let f = Fixture::new("lock");
        let first = Kept::open(&f.inst_dir, "bedrock", true).unwrap();
        match Kept::open(&f.inst_dir, "bedrock", true) {
            Err(Error::Usage(why)) => assert!(why.contains("already running"), "{why}"),
            other => panic!("a second session was not refused: {}", other.is_ok()),
        }
        drop(first);
        assert!(Kept::open(&f.inst_dir, "bedrock", false).unwrap().is_empty(), "persist_worlds = false keeps nothing");
        // A link on the way to the worlds would point the game's bind mount elsewhere.
        let games = f.inst_dir.join(super::super::instance::DATA_DIR).join("games");
        std::fs::remove_dir_all(&games).unwrap();
        std::os::unix::fs::symlink(&f.root, &games).unwrap();
        assert!(matches!(Kept::open(&f.inst_dir, "bedrock", true), Err(Error::Policy(_))));
    }

    #[test]
    fn earlier_sessions_of_this_instance_are_deleted_and_nothing_else_is() {
        let f = Fixture::new("sweep");
        let scratch = f.paths.scratch();
        util::create_private_dir_all(&scratch).unwrap();
        let crashed = scratch.join("bedrock+bedrock-0123456789abcdef");
        std::fs::create_dir_all(crashed.join("xal")).unwrap();
        std::fs::write(crashed.join("xal/token.json"), b"{}").unwrap();
        // Not this instance's, not a session at all, a name that only looks similar.
        let keep = [
            "other+bedrock-0123456789abcdef",
            "bedrock-0123456789abcdef",
            "bedrock+bedrock-0123",
            "bedrock+bedrock-0123456789abcdeg",
        ];
        for name in keep {
            std::fs::create_dir_all(scratch.join(name)).unwrap();
        }
        // A running session holds its lock: never swept.
        let running = Session::create(&f.paths, "bedrock").unwrap();
        let swept = sweep_instance(&f.paths, "bedrock", std::time::Instant::now() + std::time::Duration::from_secs(5));
        assert_eq!(swept.removed, 1, "{swept:?}");
        assert!(!crashed.exists());
        assert!(running.path().exists() && swept.failed.len() == 1 && swept.failed[0].contains("held"), "{swept:?}");
        for name in keep {
            assert!(scratch.join(name).exists(), "{name} was removed");
        }
        let all = session_dirs(&f.paths);
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(all.contains(&running.path().to_path_buf()) && all.contains(&scratch.join(keep[0])));
    }

    #[test]
    fn a_client_below_a_directory_the_sandbox_makes_is_refused() {
        for path in ["/tmp/x/mcpelauncher-client", "/run/user/1000/c", "/bastion/data/c", "/dev/shm/c", "relative/c"] {
            assert!(matches!(outside_sandbox_made("the client", Path::new(path)), Err(Error::Policy(_))), "{path}");
        }
        for path in ["/usr/bin/mcpelauncher-client", "/opt/mcpelauncher/bin/mcpelauncher-client"] {
            assert!(outside_sandbox_made("the client", Path::new(path)).is_ok(), "{path}");
        }
    }

    #[test]
    fn a_killed_launchs_bridge_socket_is_swept_and_a_live_one_is_not() {
        // A short path: a socket's is limited to 107 bytes.
        let base =
            std::env::temp_dir().join(format!("bmc-bs-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
        std::fs::create_dir_all(&base).unwrap();
        let old = |dir: &Path| {
            // Backdate the socket past the leftover age.
            let socket = dir.join(signin::SOCKET_NAME);
            let then = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
            let times = [libc::timespec {
                tv_sec: then.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                tv_nsec: 0,
            }; 2];
            let path = std::ffi::CString::new(socket.as_os_str().as_encoded_bytes()).unwrap();
            // SAFETY: valid C string and a two-element timespec array; no flags.
            assert_eq!(
                unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) },
                0
            );
        };
        // Killed: bound, then the listener closed (the process is gone), name left behind.
        let dead = base.join("signin-0123456789ab");
        std::fs::create_dir_all(&dead).unwrap();
        drop(std::os::unix::net::UnixListener::bind(dead.join(signin::SOCKET_NAME)).unwrap());
        old(&dead);
        // Live: still listening.
        let live = base.join("signin-ba9876543210");
        std::fs::create_dir_all(&live).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(live.join(signin::SOCKET_NAME)).unwrap();
        old(&live);
        // Not ours.
        let other = base.join("companion-0123456789ab");
        std::fs::create_dir_all(&other).unwrap();
        drop(std::os::unix::net::UnixListener::bind(other.join(signin::SOCKET_NAME)).unwrap());
        old(&other);
        sweep_bridge_dirs(&base);
        assert!(!dead.exists(), "the killed launch's socket and directory are gone");
        assert!(live.join(signin::SOCKET_NAME).exists(), "a socket somebody listens on stays");
        assert!(other.exists(), "another component's directory stays");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// What the launch says about deleting the data root rests on `Session::remove`: nothing
    /// when the tree is gone, also one the game left without permission bits.
    #[test]
    fn a_sessions_data_root_is_deleted_even_with_its_permission_bits_taken() {
        let f = Fixture::new("remove");
        let mut session = Session::create(&f.paths, "bedrock").unwrap();
        let dir = session.path().to_path_buf();
        std::fs::create_dir_all(dir.join("xal")).unwrap();
        std::fs::write(dir.join("xal/token.json"), b"{}").unwrap();
        std::fs::set_permissions(dir.join("xal"), std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(session.remove(std::time::Instant::now() + std::time::Duration::from_secs(5)), Ok(()));
        assert!(!dir.exists());
        // Dropping a session that was never removed removes it too.
        let dropped = Session::create(&f.paths, "bedrock").unwrap();
        let path = dropped.path().to_path_buf();
        drop(dropped);
        assert!(!path.exists());
    }
}
