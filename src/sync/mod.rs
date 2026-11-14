//! Settings sync: the game's settings follow you from one instance to the next.
//!
//! `[sync] options = true` (the default). The launcher keeps a synced copy of the settings
//! that mean the same in every Minecraft family ([`keys`]) under its data directory
//! ([`store`]). Each launch:
//!
//! 1. **before the game starts**, the synced values are merged into the instance's
//!    `persist/options.txt` — translated to that instance's Minecraft version where the format
//!    changed and the meaning is certain ([`translate`]), skipped where it is not, and only for
//!    settings that instance's game has (its own `options.txt` lists them). An instance without
//!    an `options.txt` yet is given one with the synced settings and its game's DataVersion
//!    (read from its verified client jar);
//! 2. **at exit**, after the launcher has carried the game's `options.txt` back (parsed and
//!    re-encoded, `persist`), every synced setting whose value differs from what the game was
//!    given at the start is written to the synced copy — per setting, last writer wins. The
//!    phone's `settings` app can do the same during the session (`sync-now`).
//!
//! A burner session takes no part: nothing it changes may outlive it, so it neither changes the
//! synced copy nor has its copy of the instance's settings changed.
//!
//! The companion mod's own settings (zoom key, HUD, …) are not in `options.txt`: they come from
//! `config.toml`, globally, with per-instance overrides in `instance.toml` (`[companion]`), and
//! so are the same in every instance already.
//!
//! What the game can do through this: put values into the synced copy — but only booleans,
//! numbers, short identifiers and key names, for settings on the list; every other line of its
//! `options.txt` stays in its own instance. A hostile game in one instance can therefore change
//! the listed settings of your other instances (bind a key elsewhere, turn the volume down) —
//! nothing beyond that list, and nothing that is not one of those shapes.

pub mod keys;
pub mod store;
pub mod translate;

use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::Deserialize;

use crate::{
    error::{Error, Result},
    paths::Paths,
    persist::{self, Kind},
    util,
};

/// The file this module syncs.
pub const OPTIONS: &str = "options.txt";

/// `[sync]` in config.toml.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncConfig {
    /// Sync the game's settings (video, sound volumes, mouse, key bindings, language, chat,
    /// accessibility) across instances.
    pub options: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig { options: true }
    }
}

/// Where the synced copy lives.
pub fn dir(paths: &Paths) -> PathBuf {
    paths.data.join("sync")
}

/// The DataVersion of a Minecraft client jar (`world_version` in its `version.json`).
pub fn data_version_of_jar(jar: &File) -> Option<u32> {
    let mut reader = jar.try_clone().ok()?;
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut archive = zip::ZipArchive::new(reader).ok()?;
    let entry = archive.by_name("version.json").ok()?;
    let mut text = Vec::new();
    entry.take(64 * 1024).read_to_end(&mut text).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&text).ok()?;
    value.get("world_version")?.as_u64().and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0)
}

/// An `options.txt` as the launcher's sanitiser reads it: its lines, in order.
fn read_options(path: &Path) -> Result<Option<Vec<(String, String)>>> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(format!("inspecting {}", path.display()), e)),
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(Error::Policy(format!("{} is not a regular file", path.display())));
        }
        Ok(_) => {}
    }
    let raw = util::read_small_file(path, persist::MAX_INPUT_BYTES)?;
    Ok(Some(parse_options(&raw)))
}

/// The lines of an `options.txt`, after the same pass the launcher carries it through.
pub fn parse_options(raw: &[u8]) -> Vec<(String, String)> {
    let Ok((clean, _)) = persist::sanitize(Kind::Options, raw) else { return Vec::new() };
    String::from_utf8_lossy(&clean)
        .lines()
        .filter_map(|line| line.split_once(':').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

fn data_version_in(lines: &[(String, String)]) -> Option<u32> {
    lines.iter().find(|(k, _)| k == "version").and_then(|(_, v)| v.parse().ok()).filter(|v| *v > 0)
}

fn encode_options(lines: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in lines {
        out.extend_from_slice(k.as_bytes());
        out.push(b':');
        out.extend_from_slice(v.as_bytes());
        out.push(b'\n');
    }
    out
}

/// The synced settings of a file: name → value.
fn syncable_of(lines: &[(String, String)]) -> BTreeMap<String, String> {
    lines.iter().filter(|(k, v)| keys::syncable(k, v)).map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// What merging the synced copy into one instance's settings did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    /// The file the game will be given.
    pub lines: Vec<(String, String)>,
    /// Settings whose value was taken from the synced copy.
    pub taken: Vec<String>,
    /// There was no `options.txt`: this is a new one.
    pub created: bool,
}

/// Merge the synced copy into an instance's `options.txt` (`existing`, `None` if it has none
/// yet) for a game of DataVersion `jar_version`. Pure: no file is read or written. `None`
/// means nothing can be merged (no DataVersion known, or one older than 1.20.1).
pub fn merge(
    store: &store::Store,
    existing: Option<Vec<(String, String)>>,
    jar_version: Option<u32>,
) -> Option<Merged> {
    let target = existing.as_deref().and_then(data_version_in).or(jar_version)?;
    if target < translate::OLDEST {
        return None;
    }
    let view = translate::view_for(&store.synced(), target);
    let mut taken = Vec::new();
    let (lines, created) = match existing {
        Some(mut lines) => {
            for (name, value) in lines.iter_mut() {
                if let Some((synced, _)) = view.get(name.as_str())
                    && synced != value
                    && keys::syncable(name, synced)
                {
                    *value = synced.clone();
                    taken.push(name.clone());
                }
            }
            (lines, false)
        }
        None => {
            let mut lines = vec![("version".to_string(), target.to_string())];
            for (name, (value, _)) in &view {
                if keys::syncable(name, value) {
                    lines.push((name.clone(), value.clone()));
                    taken.push(name.clone());
                }
            }
            (lines, true)
        }
    };
    Some(Merged { lines, taken, created })
}

/// Settings of `now` that differ from `baseline`: the ones changed during the session.
pub fn changed_since(baseline: &BTreeMap<String, String>, now: &[(String, String)]) -> Vec<(String, String)> {
    syncable_of(now).into_iter().filter(|(k, v)| baseline.get(k) != Some(v)).collect()
}

struct State {
    enabled: bool,
    /// Why sync is off for this session, if it is.
    off_because: Option<String>,
    dir: PathBuf,
    instance: String,
    data_version: Option<u32>,
    /// The synced settings as the game was given them (and as synced back since).
    baseline: BTreeMap<String, String>,
    changed_this_session: usize,
    last_error: Option<String>,
}

/// One launch's part in settings sync: what the game was given, so that what it changed can be
/// told apart at the end. Cheap to clone (the phone's `settings` app holds one).
#[derive(Clone)]
pub struct Session {
    state: Arc<Mutex<State>>,
}

impl Session {
    fn off(dir: PathBuf, instance: &str, why: impl Into<String>) -> Session {
        Session {
            state: Arc::new(Mutex::new(State {
                enabled: false,
                off_because: Some(why.into()),
                dir,
                instance: instance.to_string(),
                data_version: None,
                baseline: BTreeMap::new(),
                changed_this_session: 0,
                last_error: None,
            })),
        }
    }

    /// Before the game starts: merge the synced copy into `persist_dir/options.txt`. Returns
    /// the session and the lines for the launch log. Never fails the launch: a problem with
    /// sync is reported and the game starts with the instance's own settings.
    pub fn prepare(
        cfg: &SyncConfig,
        dir: PathBuf,
        instance: &str,
        persist_dir: &Path,
        jar_version: Option<u32>,
        burner: bool,
        carried: bool,
    ) -> (Session, Vec<String>) {
        if burner {
            return (
                Session::off(dir, instance, "a burner session takes no part in settings sync"),
                vec!["settings sync: not in a burner session (nothing it changes may outlive it)".into()],
            );
        }
        if !cfg.options {
            return (Session::off(dir, instance, "[sync] options = false"), vec![]);
        }
        if !carried {
            return (
                Session::off(dir, instance, "options.txt is not in sandbox.persist_files"),
                vec![
                    "settings sync: off — options.txt is not among sandbox.persist_files, so the launcher does not \
                      carry it"
                        .into(),
                ],
            );
        }
        let file = persist_dir.join(OPTIONS);
        type Prepared = (Option<Merged>, store::Store, Option<Vec<(String, String)>>);
        let prepared = (|| -> Result<Prepared> {
            util::create_private_dir_all(&dir)?;
            let store = store::Store::load(&dir)?;
            let existing = read_options(&file)?;
            Ok((merge(&store, existing.clone(), jar_version), store, existing))
        })();
        let (merged, store, existing) = match prepared {
            Ok(done) => done,
            Err(e) => {
                let why = format!("the synced copy or the instance's options.txt could not be read ({e})");
                return (
                    Session::off(dir, instance, why.clone()),
                    vec![format!("settings sync: off for this launch — {why}")],
                );
            }
        };
        let mut notes = Vec::new();
        if store.dropped > 0 {
            notes.push(format!(
                "settings sync: {} entr(y/ies) of the synced copy did not fit the expected form and were ignored",
                store.dropped
            ));
        }
        let Some(merged) = merged else {
            let why = "the Minecraft version of this instance is not known, or older than 1.20.1";
            notes.push(format!("settings sync: off for this instance — {why}"));
            return (Session::off(dir, instance, why), notes);
        };
        if (!merged.taken.is_empty() || merged.created)
            && let Err(e) = util::write_atomic(&file, &encode_options(&merged.lines), 0o600)
        {
            let why = format!("{} could not be written ({e})", file.display());
            notes.push(format!("settings sync: off for this launch — {why}"));
            return (Session::off(dir, instance, why), notes);
        }
        let data_version = data_version_in(&merged.lines);
        let from = store.last_change().map(|(_, id)| id.to_string());
        notes.push(match (merged.taken.len(), merged.created, existing.is_some()) {
            (0, _, _) if store.is_empty() => "settings sync: on — nothing synced yet; what you change in this \
                                              session will apply to your other instances"
                .to_string(),
            (0, _, _) => format!(
                "settings sync: on — this instance already has the synced settings ({} in the synced copy)",
                store.len()
            ),
            (n, true, _) => format!(
                "settings sync: this instance had no options.txt; it starts with {n} synced setting(s){}",
                from.map(|id| format!(" (last set in instance {id:?})")).unwrap_or_default()
            ),
            (n, false, _) => format!(
                "settings sync: {n} setting(s) taken from the synced copy{}: {}",
                from.map(|id| format!(" (last set in instance {id:?})")).unwrap_or_default(),
                summarize(&merged.taken)
            ),
        });
        let session = Session {
            state: Arc::new(Mutex::new(State {
                enabled: true,
                off_because: None,
                dir,
                instance: instance.to_string(),
                data_version,
                baseline: syncable_of(&merged.lines),
                changed_this_session: 0,
                last_error: None,
            })),
        };
        (session, notes)
    }

    /// The directory the synced copy is written to, while sync is on: the confined launcher
    /// must keep write access to it.
    pub fn writes(&self) -> Option<PathBuf> {
        let state = self.state.lock().expect("sync state");
        state.enabled.then(|| state.dir.clone())
    }

    pub fn enabled(&self) -> bool {
        self.state.lock().expect("sync state").enabled
    }

    /// Compare `file` (the instance's carried-back `options.txt` at exit) with what the game
    /// was given, and write what changed to the synced copy. Returns the names of the
    /// settings that were synced.
    pub fn sync_back(&self, file: &Path) -> Result<Vec<String>> {
        if !self.enabled() {
            return Ok(Vec::new());
        }
        match read_options(file) {
            Ok(Some(lines)) => self.sync_lines(&lines),
            Ok(None) => Ok(Vec::new()),
            Err(e) => {
                self.state.lock().expect("sync state").last_error =
                    Some(util::sanitize_line(e.to_string().as_bytes(), 200));
                Err(e)
            }
        }
    }

    /// [`Session::sync_back`] for the lines of an `options.txt` already read.
    pub fn sync_lines(&self, lines: &[(String, String)]) -> Result<Vec<String>> {
        let (dir, instance, baseline, fallback_version) = {
            let state = self.state.lock().expect("sync state");
            if !state.enabled {
                return Ok(Vec::new());
            }
            (state.dir.clone(), state.instance.clone(), state.baseline.clone(), state.data_version)
        };
        let result = (|| -> Result<Vec<(String, String)>> {
            let Some(version) = data_version_in(lines).or(fallback_version) else { return Ok(Vec::new()) };
            if version < translate::OLDEST {
                return Ok(Vec::new());
            }
            let changed = changed_since(&baseline, lines);
            if changed.is_empty() {
                return Ok(changed);
            }
            let when = store::now();
            store::update(&dir, |store| {
                for (name, value) in &changed {
                    store.set(name, value, version, when, &instance);
                }
            })?;
            Ok(changed)
        })();
        let mut state = self.state.lock().expect("sync state");
        match result {
            Ok(changed) => {
                state.changed_this_session += changed.len();
                state.last_error = None;
                let names = changed.iter().map(|(k, _)| k.clone()).collect();
                state.baseline.extend(changed);
                Ok(names)
            }
            Err(e) => {
                state.last_error = Some(util::sanitize_line(e.to_string().as_bytes(), 200));
                Err(e)
            }
        }
    }

    /// The phone's view: `{"sync", "last_sync", "keys", "instances", "error"}` (and a little more).
    pub fn status(&self) -> serde_json::Value {
        let (enabled, off_because, dir, changed, error) = {
            let state = self.state.lock().expect("sync state");
            (
                state.enabled,
                state.off_because.clone(),
                state.dir.clone(),
                state.changed_this_session,
                state.last_error.clone(),
            )
        };
        let store = store::Store::load(&dir).unwrap_or_default();
        let last = store.last_change();
        serde_json::json!({
            "sync": enabled,
            // Seconds since the epoch.
            "last_sync": last.map(|(when, _)| when / 1000),
            "last_instance": last.map(|(_, id)| id.to_string()),
            "keys": store.len(),
            "instances": store.instances(),
            "changed_this_session": changed,
            "error": error.or(off_because),
        })
    }
}

/// A short list of setting names for a log line.
pub fn summarize(names: &[String]) -> String {
    const SHOWN: usize = 8;
    let mut text = names.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > SHOWN {
        text.push_str(&format!(" and {} more", names.len() - SHOWN));
    }
    text
}

// ---------------------------------------------------------------- the phone's settings app

/// The phone's `settings` app (contract: verbs `status`, `sync-now`).
pub struct SettingsService {
    session: Session,
    /// The game's own `options.txt` (in the scratch game root) for `sync-now`.
    live: PathBuf,
}

impl SettingsService {
    pub fn new(session: Session, live: PathBuf) -> Self {
        SettingsService { session, live }
    }
}

const SETTINGS_LIMITS: &[crate::phone::VerbLimit] = &[
    crate::phone::VerbLimit { verb: "status", per_minute: 30, burst: 5 },
    crate::phone::VerbLimit { verb: "sync-now", per_minute: 6, burst: 2 },
];

impl crate::phone::Service for SettingsService {
    fn name(&self) -> &'static str {
        "settings"
    }

    fn describe(&self) -> String {
        match self.session.writes() {
            Some(dir) => format!("phone: settings app — sync on, synced copy in {}", dir.display()),
            None => "phone: settings app — sync off for this launch (status only)".into(),
        }
    }

    fn confinement_needs(&self, keep: &mut crate::confine::Confinement) {
        if let Some(dir) = self.session.writes() {
            keep.write.push(dir);
        }
    }

    fn into_app(self: Box<Self>) -> Box<dyn crate::phone::PhoneApp> {
        Box::new(SettingsApp { session: self.session, live: self.live, busy: Arc::default() })
    }
}

pub struct SettingsApp {
    session: Session,
    live: PathBuf,
    busy: Arc<std::sync::atomic::AtomicBool>,
}

impl crate::phone::PhoneApp for SettingsApp {
    fn name(&self) -> &'static str {
        "settings"
    }

    fn limits(&self) -> &'static [crate::phone::VerbLimit] {
        SETTINGS_LIMITS
    }

    fn hello(&mut self, out: &crate::phone::Outbox) {
        out.state("settings", &self.session.status());
    }

    fn command(&mut self, command: crate::phone::Command, out: &crate::phone::Outbox) {
        use std::sync::atomic::Ordering;
        if !command.args.is_empty() || command.text.is_some() {
            out.notice("settings", "settings: this command takes no arguments");
            return;
        }
        match command.verb.as_str() {
            "status" => {
                out.state("settings", &self.session.status());
            }
            "sync-now" => {
                if !self.session.enabled() {
                    out.notice("settings", "settings sync is off for this launch");
                    out.state("settings", &self.session.status());
                    return;
                }
                if self.busy.swap(true, Ordering::AcqRel) {
                    return;
                }
                let (session, live, out, busy) =
                    (self.session.clone(), self.live.clone(), out.clone(), self.busy.clone());
                // File work off the async runtime; the game may be writing the file right now,
                // so it is read until two reads agree.
                tokio::task::spawn_blocking(move || {
                    let result = settled(&live).and_then(|raw| session.sync_lines(&parse_options(&raw)));
                    match result {
                        Ok(names) if names.is_empty() => {
                            out.notice("settings", "settings: nothing changed since the last sync");
                        }
                        Ok(names) => {
                            out.notice(
                                "settings",
                                &format!("settings: {} setting(s) now apply to your other instances", names.len()),
                            );
                        }
                        Err(_) => {
                            out.notice("settings", "settings: the sync failed (see the launch log)");
                        }
                    }
                    out.state("settings", &session.status());
                    busy.store(false, Ordering::Release);
                });
            }
            _ => {
                out.notice("settings", "settings: unknown command");
            }
        }
    }
}

/// The content of a file the game may be writing at this moment: read until two reads, a
/// moment apart, agree (at most a few tries). Nothing is written next to it.
fn settled(file: &Path) -> Result<Vec<u8>> {
    let read = || util::read_small_file(file, persist::MAX_INPUT_BYTES);
    let mut last = read()?;
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let again = read()?;
        if again == last {
            return Ok(again);
        }
        last = again;
    }
    Err(Error::Policy("the game kept rewriting options.txt; try again".into()))
}

/// For the robustness tests and fuzz targets: an instance's `options.txt` (before a NUL byte)
/// and a synced copy (after it), both arbitrary. Whatever they are, the merge writes only
/// settings on the list in the shape of their kind, changes nothing else of the file, is its
/// own fixed point, and what is synced back is only what may be synced.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    let (options, copy) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &b""[..]),
    };
    let store = store::Store::parse(copy);
    let reread = store::Store::parse(&store.encode());
    assert_eq!((reread.synced(), reread.dropped), (store.synced(), 0), "the copy re-reads as it was written");
    let existing = parse_options(options);
    for version in [None, Some(translate::OLDEST), Some(4189), Some(translate::NEWEST_KNOWN)] {
        let file = (!existing.is_empty()).then(|| existing.clone());
        let Some(merged) = merge(&store, file.clone(), version) else { continue };
        for (name, value) in &merged.lines {
            if merged.taken.contains(name) {
                assert!(keys::syncable(name, value), "{name}:{value} was merged");
            }
        }
        if let Some(original) = &file {
            assert_eq!(merged.lines.len(), original.len(), "a merge into a file adds no lines");
            for ((k, v), (k0, v0)) in merged.lines.iter().zip(original) {
                assert_eq!(k, k0);
                assert!(v == v0 || merged.taken.contains(k), "{k} changed without being taken");
            }
        }
        // The merge's own output: nothing more to take, and it re-reads as it was written.
        let again = merge(&store, Some(merged.lines.clone()), version).expect("it merged once");
        assert!(again.taken.is_empty(), "not a fixed point: {:?}", again.taken);
        assert_eq!(parse_options(&encode_options(&merged.lines)), merged.lines);
        let baseline = syncable_of(&merged.lines);
        assert!(changed_since(&baseline, &merged.lines).is_empty());
        for (name, value) in changed_since(&BTreeMap::new(), &existing) {
            assert!(keys::syncable(&name, &value));
        }
    }
}

#[cfg(test)]
mod tests;
