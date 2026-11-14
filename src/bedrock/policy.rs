//! The gate in front of every Bedrock launch, and the account-risk controls around a Bedrock
//! session: what `doctor` says about Bedrock, where a session keeps what must not outlive it,
//! how `scram` and the next start find what a killed session left there, and what the user is
//! told about capes and the profile canary when a session ends.
//!
//! **Why a session's whole data directory counts as a credential store.** The game's sign-in
//! library keeps its tokens (a refresh token of the Microsoft account, the device key, Xbox
//! tokens) in its "local storage", and `mcpelauncher-client` answers the library's question
//! for that storage's path with the root of the client's data directory
//! (`XboxLocalStorage.getPath` → `PathHelper::getPrimaryDataDirectory()` in
//! `src/jni/xbox_live.cpp` of the client, at the research's commit). The file names inside are
//! the game's own and are not known here: the real game never ran on this machine. So the data
//! directory a session gets is per-session scratch ([`SessionDir`]). Only the worlds and the
//! game's own settings come from the instance (that is the launch's part), and everything else
//! the game writes there ends with the session. A session that is killed, or crashes, cannot
//! remove its directory. [`remove_leftovers`] does that, at the next start and in `scram`.
//! `scram` treats the directory as a credential: it is removed with the other credentials,
//! before what Java games leave, and its removal is confirmed.
//!
//! **Where.** `$XDG_CACHE_HOME/bastion-mc/bedrock-sessions/<instance>-<16 hex>`, mode 0700,
//! locked by its launch ([`running::lock_dir`]) for as long as that launch lives. The lock,
//! not the marker, keeps a sweep away from the directory of a running session: a lock ends with
//! its process however that process ends, and a file cannot forge it. The directories are not
//! under `scratch/`, because what is there is swept last, for a bounded time, as no more than
//! what a game wrote.
//!
//! **What this does not do.** Deleting a file is not a secure wipe (SSDs and copy-on-write file
//! systems keep old blocks). A token the game copied elsewhere while it ran, through any
//! connection it was allowed, is out of reach of everything here: only "sign out everywhere" at
//! Microsoft ends it. During the session the game holds the sign-in in memory; nothing can
//! broker it, unlike Java's access token. And none of this ran with the real client: where the
//! real game keeps its tokens is read from the client's source, not observed.

use std::{
    os::unix::{ffi::OsStrExt, fs::DirBuilderExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use super::BedrockConfig;
use crate::{
    config::Config,
    error::{Context, Error, Result},
    events::Reporter,
    paths::{Paths, validate_id},
    running, tree, util,
};

/// Refuses a Bedrock launch unless the user accepted the licence substitution.
///
/// The text names what is being accepted, so that a user who never read the documents learns
/// it here rather than not at all, and it says what bastion-mc itself does not do.
pub fn gate(cfg: &BedrockConfig) -> Result<()> {
    if cfg.accept_licence_substitution {
        return Ok(());
    }
    Err(Error::Policy(
        "Bedrock is off until you accept how its licence check works. mcpelauncher-client, the program \
         bastion-mc runs for Bedrock, answers the game's own Google Play licence query with \"licensed\" by \
         itself; the real check is the one mcpelauncher-ui made when it downloaded the game, by asking Google \
         whether your Google account bought Minecraft. bastion-mc decrypts nothing, patches nothing and answers \
         no licence query, and it cannot check that a version you import was bought. If you accept that, set \
         `accept_licence_substitution = true` under `[bedrock]` in config.toml (`bastion-mc config path` shows \
         where it is). docs/BEDROCK.md explains it."
            .into(),
    ))
}

// ---------------------------------------------------------------- the session's directory

/// How long removing one session's directory may take at the end of the session. The game
/// decides how much it writes there; what is left after this is removed by the next start or
/// by `scram`, which report it if they cannot.
const REMOVAL_TIME: Duration = Duration::from_secs(30);

/// Where Bedrock sessions keep their data directories (see the module documentation).
pub fn sessions_dir(paths: &Paths) -> PathBuf {
    paths.cache.join("bedrock-sessions")
}

/// The data directory of one Bedrock session: the client's `-dd`, and with it the token store
/// of the game's sign-in library. Created empty, locked for the life of the value (at most the
/// life of this process), removed by [`SessionDir::remove`] or, failing that, on drop.
///
/// Create it after the session's marker ([`running::Marker::bedrock`]) and drop it before the
/// marker: then a `scram` that runs in between finds the launch by its marker, and anything
/// that sweeps finds a directory that is either locked or abandoned.
pub struct SessionDir {
    path: PathBuf,
    lock: Option<running::DirLock>,
    removed: bool,
}

impl std::fmt::Debug for SessionDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionDir")
            .field("path", &self.path)
            .field("locked", &self.lock.is_some())
            .field("removed", &self.removed)
            .finish()
    }
}

impl SessionDir {
    /// `<sessions_dir>/<instance>-<16 random hex digits>`, new (an existing directory of that
    /// name is an error, never reused), mode 0700, locked before it is returned.
    pub fn create(paths: &Paths, instance: &str) -> Result<SessionDir> {
        validate_id(instance)?;
        let base = sessions_dir(paths);
        util::create_private_dir_all(&base)?;
        let path = base.join(format!("{instance}-{}", util::hex_encode(&util::random_bytes::<8>()?)));
        std::fs::DirBuilder::new().mode(0o700).create(&path).ctx(|| format!("creating {}", path.display()))?;
        // Between the two calls the directory is new, empty and unlocked: a sweep that runs at
        // that moment may delete it, and then this lock fails and the launch stops. Nothing of
        // the game is in it yet.
        let lock = running::lock_dir(&path).ctx(|| format!("locking {}", path.display()))?;
        Ok(SessionDir { path, lock: Some(lock), removed: false })
    }

    /// The directory, for the launch to mount at the client's data directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the directory and everything the game wrote into it, the careful way
    /// (`tree::remove`: the game may have taken permissions away, nested deeply, written many
    /// files) and for a bounded time, and only then release the lock. `Err` says what is left;
    /// the caller reports it, and the next start or `scram` removes it.
    pub fn remove(&mut self) -> std::result::Result<(), String> {
        self.removed = true;
        let result = tree::remove(&self.path, Some(Instant::now() + REMOVAL_TIME));
        self.lock = None;
        match result {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", self.path.display())),
        }
    }
}

impl Drop for SessionDir {
    fn drop(&mut self) {
        // The session is ending some other way (an error, a cancelled start): best effort.
        if !self.removed {
            let _ = self.remove();
        }
    }
}

/// What [`remove_leftovers`] did, by directory name (shown sanitised).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Leftovers {
    /// Removed completely.
    pub removed: Vec<String>,
    /// Not removed, or not completely, each with the reason.
    pub failed: Vec<String>,
    /// Left alone: a live process holds the directory's lock, so its session is running.
    pub in_use: Vec<String>,
}

/// Remove the data directories of Bedrock sessions that ended without removing their own: a
/// launcher that was killed (by `scram`, by the kernel, by a power cut) or crashed. Every
/// directory a live process holds locked is left alone and listed in [`Leftovers::in_use`]. A
/// file or link found there (bastion-mc puts none there) is removed, a link as a link.
///
/// The work stops at `deadline`; what is left then is in [`Leftovers::failed`]. `Err`: the
/// directory of sessions could not be read, or is not a private directory of the user's (a
/// link, for one, would make this delete what it points to), so nothing was deleted and
/// nothing is known about what it holds.
pub fn remove_leftovers(paths: &Paths, deadline: Option<Instant>) -> std::result::Result<Leftovers, String> {
    let base = sessions_dir(paths);
    let mut done = Leftovers::default();
    let Some(entries) = open_sessions(&base)? else { return Ok(done) };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            // What the listing passed over is found by whoever looks again (scram does).
            Err(e) => {
                done.failed.push(format!("listing {} stopped: {e}", base.display()));
                break;
            }
        };
        let path = entry.path();
        let name = util::sanitize_line(entry.file_name().as_bytes(), 120);
        let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        if is_dir && running::in_use(&path) {
            done.in_use.push(name);
            continue;
        }
        let result = if is_dir { tree::remove(&path, deadline) } else { std::fs::remove_file(&path) };
        match result {
            Ok(()) => done.removed.push(name),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => done.failed.push(format!("{name}: {e}")),
        }
    }
    Ok(done)
}

/// The directory of sessions, listed, if it exists — and only if it is a private directory of
/// the user's, as everything here that is created with `util::create_private_dir_all` is.
fn open_sessions(base: &Path) -> std::result::Result<Option<std::fs::ReadDir>, String> {
    match std::fs::symlink_metadata(base) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", base.display())),
        Ok(_) => {}
    }
    util::require_private(base).map_err(|e| e.to_string())?;
    std::fs::read_dir(base).map(Some).map_err(|e| format!("{}: {e}", base.display()))
}

/// What is in the directory of sessions now, without changing anything: how many directories
/// a live process holds (running sessions), and how many other entries there are (leftovers).
/// For `doctor`, which reports and never deletes.
fn look_at_sessions(paths: &Paths) -> std::result::Result<(usize, usize), String> {
    let base = sessions_dir(paths);
    let Some(entries) = open_sessions(&base)? else { return Ok((0, 0)) };
    let (mut running, mut left) = (0, 0);
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", base.display()))?.path();
        let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        if is_dir && running::in_use(&path) {
            running += 1;
        } else {
            left += 1;
        }
    }
    Ok((running, left))
}

/// Was Bedrock set up on this machine (a version was imported, or a session ran)? `scram` then
/// says what it does and does not do about Bedrock.
pub fn set_up(paths: &Paths) -> bool {
    super::store::versions_dir(paths).exists() || sessions_dir(paths).exists()
}

// ---------------------------------------------------------------- doctor

/// How a line of `doctor` is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

/// One line of `doctor`'s Bedrock section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub level: Level,
    pub what: &'static str,
    pub detail: String,
}

fn line(level: Level, what: &'static str, detail: impl Into<String>) -> Line {
    Line { level, what, detail: detail.into() }
}

/// A path for the terminal: it comes from the file system.
fn shown(path: &Path) -> String {
    util::sanitize_line(path.as_os_str().as_bytes(), 200)
}

/// Most version ids listed by name; the rest are counted.
const LISTED_VERSIONS: usize = 8;

/// The ids of the imported versions (directories in the store whose name is a valid id), and
/// how many other entries the store holds; `None` when nothing was ever imported. A
/// placeholder until the store package's own listing replaces it: that one checks each
/// version against its manifest, this one only looks at names.
fn imported_versions(paths: &Paths) -> std::result::Result<Option<(Vec<String>, usize)>, String> {
    let dir = super::store::versions_dir(paths);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", shown(&dir))),
    };
    let (mut ids, mut others) = (Vec::new(), 0);
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", shown(&dir)))?;
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        match entry.file_name().to_str() {
            Some(name) if is_dir && validate_id(name).is_ok() => ids.push(name.to_string()),
            _ => others += 1,
        }
    }
    ids.sort();
    Ok(Some((ids, others)))
}

/// The line about the installed client.
fn client_line(cfg: &BedrockConfig) -> Line {
    match super::runtime::discover(cfg) {
        Ok(client) => {
            let version = client.version.as_deref().map(|v| util::sanitize_line(v.as_bytes(), 40));
            line(
                Level::Ok,
                "mcpelauncher-client",
                format!(
                    "{} ({}): root-owned, no writable path component; {} director(ies) of its own libraries; {} \
                     place(s) where it could find a sign-in window (mcpelauncher-webview), each covered by \
                     bastion-mc's stub in the sandbox",
                    shown(&client.program),
                    version.as_deref().unwrap_or("version not stated by the package"),
                    client.lib_dirs.len(),
                    client.webview_paths.len()
                ),
            )
        }
        Err(e) if cfg.accept_licence_substitution => {
            line(Level::Fail, "mcpelauncher-client", format!("{e} (a Bedrock launch is refused)"))
        }
        Err(e) => line(Level::Info, "mcpelauncher-client", e.to_string()),
    }
}

/// `doctor`'s Bedrock section. With the licence substitution not accepted, Bedrock cannot run:
/// then only the gate, the client, the imported versions and leftovers of earlier sessions
/// are reported (a crashed session's sign-in matters whatever the configuration says now).
pub fn doctor(paths: &Paths, cfg: &Config) -> Vec<Line> {
    let bedrock = &cfg.bedrock;
    let on = bedrock.accept_licence_substitution;
    let mut lines = Vec::new();

    lines.push(if on {
        line(
            Level::Info,
            "licence substitution",
            "accepted ([bedrock] accept_licence_substitution = true): mcpelauncher-client answers the game's Google \
             Play licence query by itself; bastion-mc decrypts, patches and answers nothing, and cannot check that \
             an imported version was bought",
        )
    } else {
        line(
            Level::Info,
            "licence substitution",
            "not accepted ([bedrock] accept_licence_substitution = false): every Bedrock launch is refused; \
             docs/BEDROCK.md says what accepting means",
        )
    });
    lines.push(client_line(bedrock));

    let none_yet = "none (`bastion-mc bedrock import <dir>` imports a version that mcpelauncher-ui downloaded)";
    lines.push(match imported_versions(paths) {
        Ok(Some((ids, others))) if !ids.is_empty() => {
            let mut listed = ids.iter().take(LISTED_VERSIONS).cloned().collect::<Vec<_>>().join(", ");
            if ids.len() > LISTED_VERSIONS {
                listed.push_str(&format!(" and {} more", ids.len() - LISTED_VERSIONS));
            }
            if others > 0 {
                listed.push_str(&format!(" ({others} other entr(ies) in the store, not versions)"));
            }
            line(Level::Ok, "imported versions", listed)
        }
        Ok(_) if on => line(Level::Warn, "imported versions", format!("{none_yet}; a Bedrock launch needs one")),
        Ok(_) => line(Level::Info, "imported versions", none_yet),
        Err(e) => line(Level::Warn, "imported versions", format!("cannot be listed: {e}")),
    });

    let policy = "the game's Xbox sign-in lives one session: the data directory the game keeps it in is per-session \
                  scratch, deleted when the session ends (and, after a crash, at the next start or by `bastion-mc \
                  scram`); sign-in pages open outside the game; no Java credential exists in a Bedrock session";
    lines.push(match look_at_sessions(paths) {
        Ok((_, left)) if left > 0 => line(
            Level::Warn,
            "the game's sign-in",
            format!(
                "{left} director(ies) of Bedrock sessions that did not end properly are still in {}: they can hold \
                 the game's Xbox sign-in. The next Bedrock start deletes them, and so does `bastion-mc scram`",
                shown(&sessions_dir(paths))
            ),
        ),
        Ok((running, _)) if running > 0 => {
            line(Level::Info, "the game's sign-in", format!("{policy}. {running} Bedrock session(s) running now"))
        }
        Ok(_) if on => line(Level::Ok, "the game's sign-in", policy),
        Ok(_) => line(Level::Info, "the game's sign-in", "no leftover of an earlier Bedrock session"),
        Err(e) => {
            line(Level::Warn, "the game's sign-in", format!("leftovers of earlier sessions cannot be looked for: {e}"))
        }
    });

    if !on {
        return lines;
    }

    lines.push(match bedrock.seccomp {
        super::SeccompMode::Enforce => {
            line(Level::Ok, "system-call filter", "enforced for mcpelauncher-client ([bedrock] seccomp = \"enforce\")")
        }
        super::SeccompMode::Log => line(
            Level::Warn,
            "system-call filter",
            "[bedrock] seccomp = \"log\": calls outside the client's allowlist are only reported, not refused. Meant \
             for profiling the real client once; set it back to \"enforce\"",
        ),
    });
    lines.push(if bedrock.telemetry {
        line(
            Level::Info,
            "Microsoft telemetry",
            "reachable during Bedrock sessions ([bedrock] telemetry = true). With false the game cannot reach it, \
             but whether a challenge cape is then still granted is not known",
        )
    } else {
        line(
            Level::Info,
            "Microsoft telemetry",
            "not reachable ([bedrock] telemetry = false): a challenge cape may then not be granted (not known)",
        )
    });
    lines.push(if bedrock.persist_worlds {
        line(
            Level::Info,
            "worlds",
            "kept between sessions, with the game's own settings ([bedrock] persist_worlds = true)",
        )
    } else {
        line(Level::Info, "worlds", "deleted with each session ([bedrock] persist_worlds = false)")
    });
    lines.push(match cfg.realms.approve {
        crate::realms::Approve::Off => {
            line(Level::Info, "Realms and event servers", "joins are refused ([realms] approve = \"off\")")
        }
        mode => line(
            Level::Info,
            "Realms and event servers",
            format!("each join waits for your approval outside the game ([realms] approve = \"{}\")", mode.as_str()),
        ),
    });
    lines.push(line(
        Level::Info,
        "Bedrock servers",
        format!(
            "{} in [bedrock] servers (an instance can add its own); LAN games and friends' worlds (NetherNet) are \
             refused in this version",
            bedrock.servers.len()
        ),
    ));
    lines
}

// ---------------------------------------------------------------- after a session

/// What to tell the user when a Bedrock session ends, about capes and the profile canary.
/// `signed_in`: whether the game was signed in during the session (the launch passes `true`
/// when it cannot tell). Without a sign-in no cape can have been earned, and nothing is said.
///
/// **Why a new cape needs nothing from the user.** The capes of Bedrock events, challenges and
/// sign-in promotions belong to the Microsoft account and also show on its Java profile, later
/// (Mojang has announced anything from "as soon as the sign-in completes" to "within 30 days").
/// The profile canary does not treat a newly owned cape as a change (`auth::canary`: nobody
/// breaks into an account to give it a cape), and the next Java sign-in pins it without asking.
/// So no `canary accept` is needed for it; one is needed only for a change that the user made
/// and the canary reports, and a change the user did not make is the alarm the canary exists
/// for. A Bedrock session is a reason to look: the game held the account's Xbox sign-in, which
/// it may be able to turn into a Java token (not verified).
pub fn after_session(signed_in: bool) -> Vec<String> {
    if !signed_in {
        return Vec::new();
    }
    vec![
        "Capes: a cape earned in this session (an event server, a challenge, or the Bedrock sign-in itself) belongs to \
         the Microsoft account you signed in with, and shows on that account's Java profile later; Mojang has \
         announced anything from at once to within 30 days."
            .into(),
        "  - `bastion-mc skin show` lists the capes your Java profile owns; `bastion-mc cape set \"<name>\"` wears one."
            .into(),
        "  - A newly owned cape needs nothing from you: the profile canary pins it at the next Java sign-in without a \
         question."
            .into(),
        "  - If that sign-in reports any other change (player name, skin, the cape you wear): you made it yourself → \
         `bastion-mc canary accept`; you did not → `bastion-mc wiki scram`. The Bedrock game held your Xbox sign-in."
            .into(),
    ]
}

/// [`after_session`], told through the launch's reporter (a terminal, or a front end).
pub fn report_after_session(reporter: &Reporter, signed_in: bool) {
    for text in after_session(signed_in) {
        reporter.status(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(tag: &str) -> (PathBuf, Paths) {
        let root = std::env::temp_dir()
            .join(format!("bastion-bpol-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        };
        paths.ensure().unwrap();
        (root, paths)
    }

    #[test]
    fn the_gate_names_what_is_accepted_and_what_bastion_does_not_do() {
        let text = gate(&BedrockConfig::default()).unwrap_err().to_string();
        for needle in [
            "accept_licence_substitution = true",
            "[bedrock]",
            "mcpelauncher-client",
            "mcpelauncher-ui",
            "Google Play",
            "decrypts nothing, patches nothing and answers no licence query",
            "cannot check that a version you import was bought",
            "docs/BEDROCK.md",
        ] {
            assert!(text.contains(needle), "{needle:?} missing from: {text}");
        }
        assert!(gate(&BedrockConfig { accept_licence_substitution: true, ..BedrockConfig::default() }).is_ok());
    }

    #[test]
    fn a_session_directory_is_new_private_locked_and_removed_with_what_the_game_wrote() {
        use std::os::unix::fs::PermissionsExt;
        let (root, paths) = paths("dir");
        let mut session = SessionDir::create(&paths, "bedrock-main").unwrap();
        let dir = session.path().to_path_buf();
        assert_eq!(dir.parent().unwrap(), sessions_dir(&paths));
        let name = dir.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("bedrock-main-") && name.len() == "bedrock-main-".len() + 16, "{name}");
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(running::in_use(&dir), "locked while the session lives");
        let other = SessionDir::create(&paths, "bedrock-main").unwrap();
        assert_ne!(other.path(), dir, "never reused");
        drop(other);
        assert!(SessionDir::create(&paths, "../escape").is_err());

        // What a game leaves: a token file, a directory it took every permission from.
        std::fs::write(dir.join("xal-tokens.json"), b"{\"refresh\":\"secret\"}").unwrap();
        std::fs::create_dir_all(dir.join("games/com.mojang/locked")).unwrap();
        std::fs::set_permissions(dir.join("games/com.mojang"), std::fs::Permissions::from_mode(0o000)).unwrap();
        // A sweep leaves the directory of a running session alone…
        let swept = remove_leftovers(&paths, None).unwrap();
        assert_eq!(swept.in_use.len(), 1, "{swept:?}");
        assert!(dir.join("xal-tokens.json").exists());
        // …and the session removes it at its end.
        session.remove().unwrap();
        assert!(!dir.exists());
        drop(session);
        assert_eq!(std::fs::read_dir(sessions_dir(&paths)).unwrap().count(), 0, "nothing else was left behind");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_session_directory_is_removed_on_drop_when_the_launch_ends_some_other_way() {
        let (root, paths) = paths("drop");
        let dir = {
            let session = SessionDir::create(&paths, "b").unwrap();
            std::fs::write(session.path().join("token"), b"secret").unwrap();
            session.path().to_path_buf()
        };
        assert!(!dir.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The child half of the test below: this test binary again, holding a lock on a directory
    /// as a running session does.
    #[test]
    fn locking_child() {
        if let Some(dir) = std::env::var_os("BASTION_BPOL_LOCK") {
            let _lock = running::lock_dir(Path::new(&dir)).unwrap();
            println!("ready");
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn leftovers_of_killed_sessions_are_removed_and_running_ones_are_not() {
        use std::io::BufRead;
        let (root, paths) = paths("left");
        let base = sessions_dir(&paths);
        std::fs::create_dir_all(base.join("b-00112233445566778899/sub")).unwrap();
        std::fs::write(base.join("b-00112233445566778899/sub/token"), b"secret").unwrap();
        std::fs::create_dir_all(base.join("b-aabbccddeeff00112233")).unwrap();
        std::fs::write(base.join("stray\u{1b}[2Jfile"), b"x").unwrap();
        std::os::unix::fs::symlink(&root, base.join("link")).unwrap();
        let running_dir = base.join("b-ffffffffffffffff");
        std::fs::create_dir_all(&running_dir).unwrap();
        std::fs::write(running_dir.join("token"), b"in use").unwrap();

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "bedrock::policy::tests::locking_child", "--test-threads=1", "--nocapture"])
            .env("BASTION_BPOL_LOCK", &running_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("ready") {
            line.clear();
            assert!(out.read_line(&mut line).unwrap() > 0, "the child ended before it was ready");
        }
        assert_eq!(look_at_sessions(&paths).unwrap(), (1, 4));

        let mut done = remove_leftovers(&paths, None).unwrap();
        done.removed.sort();
        assert_eq!(done.in_use, ["b-ffffffffffffffff"]);
        assert_eq!(done.removed, ["b-00112233445566778899", "b-aabbccddeeff00112233", "link", "stray\u{FFFD}[2Jfile"]);
        assert!(done.failed.is_empty(), "{done:?}");
        assert!(root.exists(), "a link is removed as a link");
        assert!(running_dir.join("token").exists(), "a running session's directory is not touched");

        child.kill().unwrap();
        let _ = child.wait();
        // The lock ends with its process, however that ends.
        assert_eq!(remove_leftovers(&paths, None).unwrap().removed, ["b-ffffffffffffffff"]);
        assert_eq!(look_at_sessions(&paths).unwrap(), (0, 0));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn what_cannot_be_removed_in_time_or_at_all_is_reported_not_counted() {
        use std::os::unix::fs::PermissionsExt;
        let (root, paths) = paths("stuck");
        let base = sessions_dir(&paths);
        std::fs::create_dir_all(base.join("b-0123456789abcdef/a")).unwrap();
        let over = Instant::now() - Duration::from_secs(1);
        let done = remove_leftovers(&paths, Some(over)).unwrap();
        assert!(done.removed.is_empty());
        assert!(done.failed.len() == 1 && done.failed[0].starts_with("b-0123456789abcdef: "), "{done:?}");
        // A directory of sessions that does not allow removals.
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o500)).unwrap();
        let done = remove_leftovers(&paths, None).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(done.removed.is_empty() && done.failed.len() == 1, "{done:?}");
        // One that cannot be read at all is not "nothing there".
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = remove_leftovers(&paths, None);
        let looked = look_at_sessions(&paths);
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(unreadable.is_err() && looked.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The directory of sessions is the user's private directory or nothing is deleted: a link
    /// in its place would otherwise make the removal delete whatever the link points to.
    #[test]
    fn a_directory_of_sessions_that_is_a_link_or_open_to_others_is_refused_and_nothing_is_deleted() {
        use std::os::unix::fs::PermissionsExt;
        let (root, paths) = paths("link");
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("precious")).unwrap();
        std::fs::write(elsewhere.join("precious/file"), b"keep me").unwrap();
        std::os::unix::fs::symlink(&elsewhere, sessions_dir(&paths)).unwrap();
        let refused = remove_leftovers(&paths, None).unwrap_err();
        assert!(refused.contains("symlink"), "{refused}");
        assert!(look_at_sessions(&paths).is_err());
        assert_eq!(std::fs::read(elsewhere.join("precious/file")).unwrap(), b"keep me");

        std::fs::remove_file(sessions_dir(&paths)).unwrap();
        std::fs::create_dir_all(sessions_dir(&paths).join("b-0011223344556677")).unwrap();
        std::fs::set_permissions(sessions_dir(&paths), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(remove_leftovers(&paths, None).unwrap_err().contains("writable by group or others"));
        assert!(sessions_dir(&paths).join("b-0011223344556677").exists());
        std::fs::set_permissions(sessions_dir(&paths), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    fn doctor_text(paths: &Paths, cfg: &Config) -> Vec<(Level, String)> {
        doctor(paths, cfg).into_iter().map(|l| (l.level, format!("{}: {}", l.what, l.detail))).collect()
    }

    #[test]
    fn doctor_says_little_while_bedrock_is_off_and_everything_once_it_is_on() {
        let (root, paths) = paths("doc");
        let mut cfg = Config::default();
        let off = doctor_text(&paths, &cfg);
        assert_eq!(off.len(), 4, "{off:?}");
        assert!(off.iter().all(|(level, _)| *level == Level::Info), "Bedrock off is nobody's problem: {off:?}");
        assert!(off[0].1.contains("not accepted") && off[0].1.contains("refused"));
        assert!(off[2].1.contains("none (`bastion-mc bedrock import <dir>`"), "{off:?}");

        cfg.bedrock.accept_licence_substitution = true;
        cfg.bedrock.seccomp = crate::bedrock::SeccompMode::Log;
        cfg.bedrock.telemetry = false;
        let on = doctor_text(&paths, &cfg);
        assert_eq!(on.len(), 9, "{on:?}");
        let find =
            |what: &str| on.iter().find(|(_, t)| t.starts_with(what)).unwrap_or_else(|| panic!("{what}: {on:?}"));
        assert!(find("licence substitution").1.contains("accepted ("));
        // (Until the store package's discovery is merged, the client is never found.)
        assert_eq!(find("mcpelauncher-client").0, Level::Fail, "{on:?}");
        assert_eq!(find("imported versions").0, Level::Warn);
        assert_eq!(find("the game's sign-in").0, Level::Ok);
        assert!(find("the game's sign-in").1.contains("no Java credential exists in a Bedrock session"));
        assert_eq!(find("system-call filter").0, Level::Warn);
        assert!(find("Microsoft telemetry").1.contains("not known"));
        assert!(find("Realms and event servers").1.contains("refused"));
        assert!(find("Bedrock servers").1.contains("NetherNet"));

        cfg.realms.approve = crate::realms::Approve::Code;
        let on = doctor_text(&paths, &cfg);
        assert!(
            on.iter().any(|(_, t)| t.contains("approval outside the game ([realms] approve = \"code\")")),
            "{on:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn doctor_lists_imported_versions_and_warns_about_leftover_sign_ins() {
        let (root, paths) = paths("doc2");
        let versions = crate::bedrock::store::versions_dir(&paths);
        for id in ["1.26.62.1", "1.26.70.24"] {
            std::fs::create_dir_all(versions.join(id)).unwrap();
        }
        std::fs::create_dir_all(versions.join("Not An Id")).unwrap();
        std::fs::write(versions.join("index.json"), b"{}").unwrap();
        std::fs::create_dir_all(sessions_dir(&paths).join("b-0011223344556677")).unwrap();
        let cfg = Config::default();
        let lines = doctor_text(&paths, &cfg);
        let versions_line = lines.iter().find(|(_, t)| t.starts_with("imported versions")).unwrap();
        assert_eq!(versions_line.0, Level::Ok);
        assert!(versions_line.1.contains("1.26.62.1, 1.26.70.24 (2 other entr(ies)"), "{lines:?}");
        let sign_in = lines.iter().find(|(_, t)| t.starts_with("the game's sign-in")).unwrap();
        assert_eq!(sign_in.0, Level::Warn, "a crashed session's sign-in is reported even with Bedrock off");
        assert!(sign_in.1.contains("1 director(ies)") && sign_in.1.contains("bastion-mc scram"), "{sign_in:?}");
        assert!(sessions_dir(&paths).join("b-0011223344556677").exists(), "doctor reports, it does not delete");
        for many in 0..10 {
            std::fs::create_dir_all(versions.join(format!("1.0.{many}"))).unwrap();
        }
        let lines = doctor_text(&paths, &cfg);
        assert!(lines.iter().any(|(_, t)| t.contains(" and 4 more")), "{lines:?}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn after_a_signed_in_session_the_user_learns_where_the_cape_shows_and_what_the_canary_does() {
        assert!(after_session(false).is_empty(), "no sign-in, no cape, nothing to say");
        let text = after_session(true).join("\n");
        for needle in [
            "bastion-mc skin show",
            "bastion-mc cape set \"<name>\"",
            "needs nothing from you",
            "pins it at the next Java sign-in",
            "bastion-mc canary accept",
            "bastion-mc wiki scram",
            "within 30 days",
        ] {
            assert!(text.contains(needle), "{needle:?} missing from:\n{text}");
        }
        let reporter = Reporter::capture();
        report_after_session(&reporter, true);
        assert_eq!(reporter.captured().len(), after_session(true).len());
        let silent = Reporter::capture();
        report_after_session(&silent, false);
        assert!(silent.captured().is_empty());
    }
}
