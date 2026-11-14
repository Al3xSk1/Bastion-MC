//! Profile canary: notice when the account changed behind your back.
//!
//! Someone who gets into a Minecraft account usually changes something visible before
//! anything else: the player name (to sell or to squat it), the skin, the cape. The
//! launcher sees the profile at every sign-in anyway, so it pins what it saw — player id,
//! name, active skin, active cape, owned capes — and compares the next time. A difference
//! that was not made through this launcher stops the launch (by default) until the user
//! says "yes, that was me" (`bastion-mc canary accept`).
//!
//! What this is and is not:
//! * It is a tripwire for *visible* tampering. A thief who only plays on the account, or
//!   reads chat, trips nothing.
//! * It is read-only towards Mojang. A mismatch revokes nothing; `docs/SCRAM_RUNBOOK.md`
//!   says what to do next.
//! * The pinned state is public information (anyone can look up a player's skin). It is
//!   kept in the state directory, private to the user, and is not a credential. A program
//!   already running as the user can rewrite it — the canary is about the *account* being
//!   changed elsewhere, not about this machine.
//!
//! Accepted states are kept as a short history: it is the evidence Mojang support asks for
//! in an ownership dispute (names and capes the account had, with dates).
//!
//! **What moves the pin**, and nothing else does: a first sight; `canary accept` (the whole
//! profile, because the user looked at the differences and said yes); and a change made
//! through this launcher — which pins exactly the part that command changed
//! ([`accept_change`]), not whatever else the profile has become in the meantime. With
//! `auth.canary = "warn"` a difference is reported and the pin stays where it was.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::minecraft::Profile;
use crate::{
    error::{Error, Result},
    paths::{Paths, validate_id},
    util,
};

/// Earlier states kept per account.
const MAX_HISTORY: usize = 64;
const MAX_FILE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub uuid: String,
    pub name: String,
    /// Texture hash and model of the active skin; `None` is the default skin.
    pub skin: Option<(String, String)>,
    /// Id and alias of the active cape.
    pub cape: Option<(String, String)>,
    /// Aliases of every cape the account owns, sorted.
    pub capes_owned: Vec<String>,
    /// Unix time at which this state was accepted.
    pub recorded: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub current: Snapshot,
    /// Earlier accepted states, oldest first.
    #[serde(default)]
    pub history: Vec<Snapshot>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    /// Nothing pinned yet for this account.
    First,
    Unchanged,
    /// Human-readable differences, pinned → now.
    Changed(Vec<String>),
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Snapshot {
    pub fn of(profile: &Profile, recorded: u64) -> Self {
        let mut capes_owned: Vec<String> = profile.capes.iter().map(|c| c.alias.clone()).collect();
        capes_owned.sort();
        capes_owned.dedup();
        Snapshot {
            uuid: profile.id.clone(),
            name: profile.name.clone(),
            skin: profile.active_skin().map(|s| (s.texture.clone(), s.variant.name().to_string())),
            cape: profile.active_cape().map(|c| (c.id.clone(), c.alias.clone())),
            capes_owned,
            recorded,
        }
    }

    /// Only what [`Snapshot::of`] can produce is accepted back from disk: the values are
    /// shown on a terminal.
    fn is_well_formed(&self) -> bool {
        let hex = |s: &str, max: usize| (1..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit());
        let alias = |s: &str| {
            s.len() <= 48
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'-' | b'_' | b'\'' | b'.'))
        };
        hex(&self.uuid, 32)
            && self.uuid.len() == 32
            && (1..=25).contains(&self.name.len())
            && self.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && self.skin.as_ref().is_none_or(|(t, v)| hex(t, 64) && matches!(v.as_str(), "classic" | "slim"))
            && self.cape.as_ref().is_none_or(|(id, a)| hex(id, 32) && alias(a))
            && self.capes_owned.len() <= 256
            && self.capes_owned.iter().all(|a| alias(a))
    }

    /// What differs between this (pinned) state and `now`. Gaining a cape is not a
    /// difference: capes are handed out by Mojang, and nobody breaks in to add one.
    pub fn differences(&self, now: &Snapshot) -> Vec<String> {
        let mut out = Vec::new();
        let short = |t: &str| t.chars().take(12).collect::<String>();
        if self.uuid != now.uuid {
            out.push(format!(
                "this is a DIFFERENT Minecraft account: player id {} was pinned, the token now belongs to {}",
                self.uuid, now.uuid
            ));
        }
        if self.name != now.name {
            out.push(format!("player name: {} → {}", self.name, now.name));
        }
        if self.skin != now.skin {
            let show = |s: &Option<(String, String)>| match s {
                Some((texture, variant)) => format!("{}… ({variant})", short(texture)),
                None => "default skin".to_string(),
            };
            out.push(format!("skin: {} → {}", show(&self.skin), show(&now.skin)));
        }
        if self.cape.as_ref().map(|c| &c.0) != now.cape.as_ref().map(|c| &c.0) {
            let show = |c: &Option<(String, String)>| match c {
                Some((_, alias)) => alias.clone(),
                None => "no cape".to_string(),
            };
            out.push(format!("cape: {} → {}", show(&self.cape), show(&now.cape)));
        }
        let lost: Vec<&str> =
            self.capes_owned.iter().filter(|c| !now.capes_owned.contains(c)).map(String::as_str).collect();
        if !lost.is_empty() {
            out.push(format!("capes no longer owned: {}", lost.join(", ")));
        }
        out
    }
}

fn file(paths: &Paths, account: &str) -> Result<PathBuf> {
    validate_id(account)?;
    Ok(paths.canaries().join(format!("{account}.json")))
}

/// The pinned record, or `None` when there is none. A file that is not exactly what
/// [`accept`] writes is an error, not "nothing pinned": otherwise damaging the file would
/// be a way to get a changed profile accepted as a first sight.
pub fn load(paths: &Paths, account: &str) -> Result<Option<Record>> {
    let path = file(paths, account)?;
    if !path.exists() {
        return Ok(None);
    }
    let damaged = || {
        Error::Policy(format!(
            "the profile canary for account {account:?} ({}) is damaged. If you did not touch it, treat this as \
             a warning sign. To pin the account's current state anew: `bastion-mc canary accept --account {account}`.",
            path.display()
        ))
    };
    util::require_private(&path)?;
    let bytes = util::read_small_file(&path, MAX_FILE_BYTES).map_err(|_| damaged())?;
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| damaged())?;
    if !record.current.is_well_formed()
        || record.history.len() > MAX_HISTORY
        || !record.history.iter().all(Snapshot::is_well_formed)
    {
        return Err(damaged());
    }
    Ok(Some(record))
}

pub fn check(paths: &Paths, account: &str, profile: &Profile) -> Result<Check> {
    Ok(match load(paths, account)? {
        None => Check::First,
        Some(record) => {
            let differences = record.current.differences(&Snapshot::of(profile, 0));
            if differences.is_empty() { Check::Unchanged } else { Check::Changed(differences) }
        }
    })
}

impl Record {
    /// Make `snapshot` the current state; the previous one, if it differs, moves to the
    /// history.
    fn advance(&mut self, snapshot: Snapshot) {
        let same = Snapshot { recorded: self.current.recorded, ..snapshot.clone() } == self.current;
        if !same {
            let previous = std::mem::replace(&mut self.current, snapshot);
            self.history.push(previous);
            let excess = self.history.len().saturating_sub(MAX_HISTORY);
            self.history.drain(..excess);
        }
    }
}

fn store(paths: &Paths, account: &str, record: &Record) -> Result<()> {
    let path = file(paths, account)?;
    util::create_private_dir_all(&paths.canaries())?;
    let json = serde_json::to_vec_pretty(record).map_err(|e| Error::Other(format!("encoding the canary: {e}")))?;
    util::write_atomic(&path, &json, 0o600)
}

/// Pin `profile` — all of it — as the account's state. The previous state, if it differs,
/// moves to the history. A damaged record is replaced.
///
/// For a first sight and for `canary accept`. A command that changed one thing uses
/// [`accept_change`]: pinning the whole profile there would also pin whatever somebody else
/// changed.
pub fn accept(paths: &Paths, account: &str, profile: &Profile) -> Result<()> {
    let snapshot = Snapshot::of(profile, now());
    let record = match load(paths, account) {
        Ok(Some(mut record)) => {
            record.advance(snapshot);
            record
        }
        Ok(None) | Err(_) => Record { current: snapshot, history: Vec::new() },
    };
    store(paths, account, &record)
}

/// The part of a profile that a command of this launcher changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aspect {
    Skin,
    Cape,
}

/// Record a change that was made through this launcher. `after` is the profile as Mojang
/// reports it once the change is made; of it, exactly the changed `aspect` is pinned, on top
/// of what was pinned before. Everything else stays as it was pinned: if the name, or the
/// other aspect, differs as well — because somebody changed it between this command's two
/// looks at the profile, or earlier while the canary only warns — that is still a
/// difference at the next check, and is returned here for the caller to report.
///
/// With nothing pinned yet, `after` is pinned as a first sight. A damaged record is an
/// error and is left as it is: only `canary accept` replaces one.
pub fn accept_change(paths: &Paths, account: &str, after: &Profile, aspect: Aspect) -> Result<Vec<String>> {
    let seen = Snapshot::of(after, now());
    let Some(mut record) = load(paths, account)? else {
        store(paths, account, &Record { current: seen, history: Vec::new() })?;
        return Ok(Vec::new());
    };
    let mut pinned = Snapshot { recorded: seen.recorded, ..record.current.clone() };
    match aspect {
        Aspect::Skin => pinned.skin = seen.skin.clone(),
        Aspect::Cape => pinned.cape = seen.cape.clone(),
    }
    let unpinned = pinned.differences(&seen);
    record.advance(pinned);
    store(paths, account, &record)?;
    Ok(unpinned)
}

pub fn forget(paths: &Paths, account: &str) -> Result<()> {
    match std::fs::remove_file(file(paths, account)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io("removing the profile canary", e)),
    }
}

/// The message for a launch or command stopped by the canary.
pub fn refusal(account: &str, differences: &[String]) -> Error {
    Error::Policy(format!(
        "PROFILE CANARY: account {account:?} changed since it was last pinned, and not through this launcher:\n  - {}\n\
         If you made this change yourself (on minecraft.net or in another launcher), accept it:\n    \
         bastion-mc canary accept --account {account}\n\
         If you did not, someone else has access to the account. Do not sign in anywhere from this machine; \
         run `bastion-mc wiki scram` for the recovery steps.",
        differences.join("\n  - ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::minecraft::{Cape, Skin, Variant};

    fn paths() -> (PathBuf, Paths) {
        let root =
            std::env::temp_dir().join(format!("bastion-can-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
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

    fn profile() -> Profile {
        Profile {
            id: "069a79f444e94726a5befca90e38aaf5".into(),
            name: "Notch".into(),
            skins: vec![Skin {
                id: "6a6e65e576dd4c3ca625162924514568".into(),
                active: true,
                variant: Variant::Classic,
                texture: "1a4af718455d4aab528e7a61f86fa25e".into(),
            }],
            capes: vec![
                Cape {
                    id: "5af2037279e04e1f80f86bd8e3135995".into(),
                    active: true,
                    alias: "Migrator".into(),
                    texture: "2340c0e03dd24a11".into(),
                },
                Cape {
                    id: "2dfb7ba32bd34dbc9e9e6b1a0f0e7a37".into(),
                    active: false,
                    alias: "Vanilla".into(),
                    texture: "f9a76537647989f9".into(),
                },
            ],
        }
    }

    #[test]
    fn first_sight_then_unchanged_then_each_kind_of_change() {
        let (root, paths) = paths();
        let p = profile();
        assert_eq!(check(&paths, "default", &p).unwrap(), Check::First);
        accept(&paths, "default", &p).unwrap();
        assert_eq!(check(&paths, "default", &p).unwrap(), Check::Unchanged);
        assert_eq!(check(&paths, "other", &p).unwrap(), Check::First, "accounts are pinned separately");

        let changed = |edit: &dyn Fn(&mut Profile)| {
            let mut q = profile();
            edit(&mut q);
            match check(&paths, "default", &q).unwrap() {
                Check::Changed(d) => d.join("; "),
                other => panic!("expected a change, got {other:?}"),
            }
        };
        assert!(changed(&|q| q.name = "Sold4Cheap".into()).contains("player name: Notch → Sold4Cheap"));
        assert!(changed(&|q| q.skins[0].texture = "ffffffffffff0000".into()).contains("skin: 1a4af718455d…"));
        assert!(changed(&|q| q.skins[0].variant = Variant::Slim).contains("(slim)"));
        assert!(changed(&|q| q.skins.clear()).contains("→ default skin"));
        assert!(changed(&|q| q.capes[0].active = false).contains("cape: Migrator → no cape"));
        assert!(
            changed(&|q| {
                q.capes[0].active = false;
                q.capes[1].active = true;
            })
            .contains("cape: Migrator → Vanilla")
        );
        assert!(changed(&|q| drop(q.capes.remove(1))).contains("capes no longer owned: Vanilla"));
        assert!(changed(&|q| q.id = "11111111111111111111111111111111".into()).contains("DIFFERENT Minecraft account"));

        // A new cape (an event reward) is not an alarm.
        let mut gained = profile();
        gained.capes.push(Cape {
            id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            active: false,
            alias: "15th Anniversary".into(),
            texture: "abcdef0123456789".into(),
        });
        assert_eq!(check(&paths, "default", &gained).unwrap(), Check::Unchanged);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accepting_keeps_the_previous_state_as_history() {
        let (root, paths) = paths();
        accept(&paths, "default", &profile()).unwrap();
        accept(&paths, "default", &profile()).unwrap();
        assert!(load(&paths, "default").unwrap().unwrap().history.is_empty(), "nothing changed, nothing added");
        let mut renamed = profile();
        renamed.name = "Notch2".into();
        accept(&paths, "default", &renamed).unwrap();
        let record = load(&paths, "default").unwrap().unwrap();
        assert_eq!(record.current.name, "Notch2");
        assert_eq!(record.history.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Notch"]);
        assert_eq!(check(&paths, "default", &renamed).unwrap(), Check::Unchanged);
        // Bounded.
        for i in 0..(MAX_HISTORY + 10) {
            let mut p = profile();
            p.name = format!("N{i}");
            accept(&paths, "default", &p).unwrap();
        }
        assert_eq!(load(&paths, "default").unwrap().unwrap().history.len(), MAX_HISTORY);
        forget(&paths, "default").unwrap();
        forget(&paths, "default").unwrap();
        assert_eq!(check(&paths, "default", &profile()).unwrap(), Check::First);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A change made through the launcher pins what that command changed and nothing else:
    /// a rename by somebody else — in the seconds between the command's two looks at the
    /// profile, or earlier under `warn` — does not become part of the pin.
    #[test]
    fn an_own_change_pins_its_own_aspect_and_leaves_foreign_changes_unpinned() {
        let (root, paths) = paths();
        accept(&paths, "default", &profile()).unwrap();

        // The user sets a skin; when the profile is fetched again, the name has changed too.
        let mut after = profile();
        after.skins[0].texture = "00000000000000000000000000000abc".into();
        after.name = "Sold4Cheap".into();
        let unpinned = accept_change(&paths, "default", &after, Aspect::Skin).unwrap();
        assert_eq!(unpinned, ["player name: Notch → Sold4Cheap"], "reported, and not pinned");
        let record = load(&paths, "default").unwrap().unwrap();
        assert_eq!(record.current.name, "Notch", "the foreign rename is not the pin");
        assert_eq!(record.current.skin.as_ref().unwrap().0, "00000000000000000000000000000abc", "the own change is");
        assert_eq!(record.history.len(), 1);
        // So the next check still raises the alarm, about exactly the foreign change.
        assert_eq!(
            check(&paths, "default", &after).unwrap(),
            Check::Changed(vec!["player name: Notch → Sold4Cheap".into()])
        );

        // A cape command pins the cape and not a skin somebody else set.
        let mut later = profile();
        later.capes[0].active = false;
        later.capes[1].active = true;
        later.skins[0].texture = "ffffffffffffffffffffffffffffffff".into();
        let unpinned = accept_change(&paths, "default", &later, Aspect::Cape).unwrap();
        assert!(unpinned.len() == 1 && unpinned[0].starts_with("skin: "), "{unpinned:?}");
        let record = load(&paths, "default").unwrap().unwrap();
        assert_eq!(record.current.cape.as_ref().unwrap().1, "Vanilla");
        assert_eq!(record.current.skin.as_ref().unwrap().0, "00000000000000000000000000000abc");

        // Nothing else changed: nothing is left unpinned, and the profile then checks clean.
        accept(&paths, "default", &profile()).unwrap();
        let mut own = profile();
        own.skins.clear();
        assert!(accept_change(&paths, "default", &own, Aspect::Skin).unwrap().is_empty());
        assert_eq!(check(&paths, "default", &own).unwrap(), Check::Unchanged);

        // A damaged record is not replaced by the way: that is `canary accept`'s decision.
        let path = paths.canaries().join("default.json");
        std::fs::write(&path, "damaged").unwrap();
        assert!(accept_change(&paths, "default", &own, Aspect::Skin).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"damaged");
        // Nothing pinned at all: a first sight.
        assert!(accept_change(&paths, "fresh", &own, Aspect::Skin).unwrap().is_empty());
        assert_eq!(check(&paths, "fresh", &own).unwrap(), Check::Unchanged);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_damaged_canary_is_an_alarm_not_a_first_sight() {
        let (root, paths) = paths();
        accept(&paths, "default", &profile()).unwrap();
        let path = paths.canaries().join("default.json");
        let good = std::fs::read_to_string(&path).unwrap();
        for bad in [
            String::new(),
            "not json".into(),
            good.replace("Notch", "\\u001b[2J"),
            good.replace("069a79f444e94726a5befca90e38aaf5", "zz"),
            good.replace("\"classic\"", "\"wide\""),
            good.replace("\"current\"", "\"extra\": 1, \"current\""),
        ] {
            std::fs::write(&path, &bad).unwrap();
            let err = check(&paths, "default", &profile()).unwrap_err();
            assert!(matches!(err, Error::Policy(_)), "{bad:?}: {err}");
        }
        // A file others can write to is not trusted either.
        std::fs::write(&path, &good).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o666)).unwrap();
        assert!(check(&paths, "default", &profile()).is_err());
        // `accept` is the way out.
        std::fs::write(&path, "damaged").unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        accept(&paths, "default", &profile()).unwrap();
        assert_eq!(check(&paths, "default", &profile()).unwrap(), Check::Unchanged);
        std::fs::remove_dir_all(root).unwrap();
    }
}
