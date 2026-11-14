//! The synced copy: `$XDG_DATA_HOME/bastion-mc/sync/options.json`.
//!
//! One entry per synced setting: its value, the DataVersion whose format the value is in,
//! when it was set (milliseconds since the epoch) and in which instance. It is the launcher's own
//! file, but every value in it came out of some game's `options.txt`, so reading it is held to
//! the same rules as reading what a game wrote: bounded in size and count, every entry checked
//! against the allowlist and its value against the shape for that setting
//! ([`super::keys`]), anything else dropped. A timestamp from the future (a clock that was
//! wrong once) is read as "now", so that it cannot win every later comparison.
//!
//! Two launches may end at the same moment. Each update holds an exclusive lock on
//! `options.lock` while it reads the file, merges its own changes and writes it again (written
//! to a temporary name and renamed, so a reader never sees half a file).

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{keys, translate::Synced};
use crate::{
    error::{Error, Result},
    util,
};

pub const FILE: &str = "options.json";
const LOCK: &str = "options.lock";
const FORMAT: &str = "bastion-mc/sync/v1";
/// Largest synced copy that is read.
pub const MAX_BYTES: u64 = 1 << 20;
/// Most settings in it (a modded instance has a few hundred key bindings at most).
pub const MAX_ENTRIES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    value: String,
    data_version: u32,
    changed: u64,
    instance: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct File {
    format: String,
    settings: BTreeMap<String, Entry>,
}

/// The synced copy as read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Store {
    entries: BTreeMap<String, (Synced, String)>,
    /// Entries of the file that were not taken (malformed, not on the list, too many).
    pub dropped: usize,
}

/// Milliseconds since the epoch: the clock "last writer wins" is decided by.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Store {
    /// Read the synced copy from `bytes` (the file's content).
    pub fn parse(bytes: &[u8]) -> Store {
        let mut store = Store::default();
        if bytes.len() as u64 > MAX_BYTES {
            store.dropped = 1;
            return store;
        }
        let Ok(file) = serde_json::from_slice::<File>(bytes) else {
            store.dropped = 1;
            return store;
        };
        if file.format != FORMAT {
            store.dropped = file.settings.len().max(1);
            return store;
        }
        let latest = now();
        for (name, entry) in file.settings {
            let fits = store.entries.len() < MAX_ENTRIES
                && keys::syncable(&name, &entry.value)
                && (1..=1_000_000).contains(&entry.data_version)
                && crate::paths::validate_id(&entry.instance).is_ok();
            if !fits {
                store.dropped += 1;
                continue;
            }
            let synced =
                Synced { value: entry.value, data_version: entry.data_version, changed: entry.changed.min(latest) };
            store.entries.insert(name, (synced, entry.instance));
        }
        store
    }

    /// The synced copy at `dir`, or an empty one if there is none yet.
    pub fn load(dir: &Path) -> Result<Store> {
        let path = dir.join(FILE);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(Error::io(format!("inspecting {}", path.display()), e)),
            Ok(meta) if !meta.file_type().is_file() => {
                Err(Error::Policy(format!("{} is not a regular file", path.display())))
            }
            Ok(_) => Ok(Store::parse(&util::read_small_file(&path, MAX_BYTES)?)),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Setting → value, format and time, as the translation needs them.
    pub fn synced(&self) -> BTreeMap<String, Synced> {
        self.entries.iter().map(|(name, (synced, _))| (name.clone(), synced.clone())).collect()
    }

    /// How many different instances set the values in it.
    pub fn instances(&self) -> usize {
        let mut ids: Vec<&str> = self.entries.values().map(|(_, id)| id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }

    /// When the most recent value was set (milliseconds since the epoch), and in which instance.
    pub fn last_change(&self) -> Option<(u64, &str)> {
        self.entries.values().max_by_key(|(s, _)| s.changed).map(|(s, id)| (s.changed, id.as_str()))
    }

    /// Set `name` (last writer wins). Values that may not be synced are ignored.
    pub fn set(&mut self, name: &str, value: &str, data_version: u32, changed: u64, instance: &str) -> bool {
        if !keys::syncable(name, value) || (self.entries.len() >= MAX_ENTRIES && !self.entries.contains_key(name)) {
            return false;
        }
        self.entries.insert(
            name.to_string(),
            (Synced { value: value.to_string(), data_version, changed }, instance.to_string()),
        );
        true
    }

    pub fn encode(&self) -> Vec<u8> {
        let settings = self
            .entries
            .iter()
            .map(|(name, (s, id))| {
                let entry = Entry {
                    value: s.value.clone(),
                    data_version: s.data_version,
                    changed: s.changed,
                    instance: id.clone(),
                };
                (name.clone(), entry)
            })
            .collect();
        let mut out = serde_json::to_vec_pretty(&File { format: FORMAT.into(), settings }).unwrap_or_default();
        out.push(b'\n');
        out
    }
}

/// Read the synced copy, let `change` modify it, write it back — under the lock, so that two
/// launches that end together do not lose each other's changes. Returns what `change`
/// returned.
pub fn update<T>(dir: &Path, change: impl FnOnce(&mut Store) -> T) -> Result<T> {
    util::create_private_dir_all(dir)?;
    let lock_path: PathBuf = dir.join(LOCK);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&lock_path)
        .map_err(|e| Error::io(format!("opening {}", lock_path.display()), e))?;
    let _held = nix::fcntl::Flock::lock(lock, nix::fcntl::FlockArg::LockExclusive)
        .map_err(|(_, e)| Error::io(format!("locking {}", lock_path.display()), std::io::Error::from(e)))?;
    let mut store = Store::load(dir)?;
    let result = change(&mut store);
    util::write_atomic(&dir.join(FILE), &store.encode(), 0o600)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_round_trips_and_keeps_only_what_may_be_synced() {
        let mut s = Store::default();
        assert!(s.set("fov", "0.25", 5023, 100, "fab"));
        assert!(s.set("key_key.jump", "key.keyboard.j", 3465, 50, "v1201"));
        assert!(!s.set("lastServer", "evil.example", 5023, 1, "fab"), "not on the list");
        assert!(!s.set("lang", "en_us\n", 5023, 1, "fab"), "not the shape of a language");
        let again = Store::parse(&s.encode());
        assert_eq!(again, s);
        assert_eq!((again.len(), again.instances(), again.last_change()), (2, 2, Some((100, "fab"))));
    }

    #[test]
    fn a_damaged_or_foreign_copy_is_read_as_far_as_it_is_sound() {
        let text = format!(
            r#"{{"format":"{FORMAT}","settings":{{
                "fov":{{"value":"0.5","data_version":5023,"changed":1,"instance":"a"}},
                "resourcePacks":{{"value":"[\"evil\"]","data_version":5023,"changed":1,"instance":"a"}},
                "gamma":{{"value":"0.5\u001b[2J","data_version":5023,"changed":1,"instance":"a"}},
                "guiScale":{{"value":"2","data_version":0,"changed":1,"instance":"a"}},
                "maxFps":{{"value":"60","data_version":5023,"changed":1,"instance":"../x"}},
                "renderDistance":{{"value":"8","data_version":5023,"changed":99999999999999,"instance":"a"}}
            }}}}"#
        );
        let s = Store::parse(text.as_bytes());
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(s.dropped, 4);
        assert!(s.synced()["renderDistance"].changed <= now(), "a future time is read as now");
        for bad in [&b""[..], b"[]", b"{\"format\":\"other\",\"settings\":{}}", b"\xff\xfe"] {
            let s = Store::parse(bad);
            assert!(s.is_empty() && s.dropped >= 1, "{:?}", String::from_utf8_lossy(bad));
        }
        assert!(Store::parse(&vec![b' '; MAX_BYTES as usize + 1]).is_empty());
    }

    #[test]
    fn updates_hold_the_lock_and_keep_each_others_changes() {
        let dir = std::env::temp_dir().join(format!("bmc-sync-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(Store::load(&dir).unwrap().is_empty(), "no copy yet: empty");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    update(&dir, |s| s.set(&format!("key_key.hotbar.{i}"), "key.keyboard.1", 5023, i, "fab")).unwrap()
                })
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap());
        }
        let s = Store::load(&dir).unwrap();
        assert_eq!(s.len(), 8, "no update was lost");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
