//! When each local account signed in and played.
//!
//! One line per event in the state directory: Unix time, what happened, and for a launch
//! the instance. Nothing here is a credential and nothing here came from the network: no
//! addresses, no server names, no hardware details. It exists for one purpose — the dates
//! in `bastion-mc export-proofs`, which is what an account owner can show Mojang's support
//! that a thief cannot.

use std::path::PathBuf;

use crate::{
    error::Result,
    paths::{Paths, validate_id},
    util,
};

/// Lines kept per account (the oldest are dropped).
const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub time: u64,
    /// `sign-in` or `launch`.
    pub what: String,
    /// The instance, for a launch.
    pub detail: String,
}

fn file(paths: &Paths, account: &str) -> Result<PathBuf> {
    validate_id(account)?;
    Ok(paths.state.join("activity").join(format!("{account}.log")))
}

fn parse(line: &str) -> Option<Entry> {
    let mut parts = line.split(' ');
    let time = parts.next()?.parse().ok()?;
    let what = parts.next().filter(|w| matches!(*w, "sign-in" | "launch"))?.to_string();
    let detail = parts.next().unwrap_or("").to_string();
    (parts.next().is_none() && (detail.is_empty() || validate_id(&detail).is_ok())).then_some(Entry {
        time,
        what,
        detail,
    })
}

/// Everything recorded for `account`, oldest first. Lines that are not exactly what
/// [`record`] writes are skipped.
pub fn read(paths: &Paths, account: &str) -> Vec<Entry> {
    let Ok(path) = file(paths, account) else { return Vec::new() };
    let Ok(bytes) = util::read_small_file(&path, 256 * 1024) else { return Vec::new() };
    String::from_utf8_lossy(&bytes).lines().filter_map(parse).collect()
}

/// Note a sign-in (`detail` empty) or a launch (`detail` = instance). Best effort by
/// design: the caller ignores a failure, a missing line never blocks a launch.
pub fn record(paths: &Paths, account: &str, what: &str, detail: &str) -> Result<()> {
    let path = file(paths, account)?;
    if !detail.is_empty() {
        validate_id(detail)?;
    }
    let mut entries = read(paths, account);
    entries.push(Entry { time: super::canary::now(), what: what.to_string(), detail: detail.to_string() });
    let excess = entries.len().saturating_sub(MAX_ENTRIES);
    let text: String = entries[excess..]
        .iter()
        .map(|e| format!("{} {}{}{}\n", e.time, e.what, if e.detail.is_empty() { "" } else { " " }, e.detail))
        .collect();
    util::create_private_dir_all(path.parent().expect("a file in a directory"))?;
    util::write_atomic(&path, text.as_bytes(), 0o600)
}

pub fn forget(paths: &Paths, account: &str) {
    if let Ok(path) = file(paths, account) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_is_appended_bounded_and_strictly_parsed() {
        let root =
            std::env::temp_dir().join(format!("bastion-act-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        };
        paths.ensure().unwrap();
        assert!(read(&paths, "default").is_empty());
        record(&paths, "default", "sign-in", "").unwrap();
        record(&paths, "default", "launch", "vanilla-26.3").unwrap();
        let entries = read(&paths, "default");
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].what.as_str(), entries[0].detail.as_str()), ("sign-in", ""));
        assert_eq!((entries[1].what.as_str(), entries[1].detail.as_str()), ("launch", "vanilla-26.3"));
        assert!(entries[1].time >= entries[0].time && entries[0].time > 1_700_000_000);
        assert!(read(&paths, "other").is_empty());
        assert!(record(&paths, "default", "launch", "../x").is_err());
        assert!(record(&paths, "../x", "launch", "a").is_err());

        // Foreign lines are not shown (the output of export-proofs is sent to a third party).
        let path = paths.state.join("activity/default.log");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("1700000000 launch a b\n1700000000 token eyJhbGciOi\nnot a line\n1700000001 launch \u{1b}[2J\n");
        std::fs::write(&path, text).unwrap();
        assert_eq!(read(&paths, "default").len(), 2);

        let many: String = (0..MAX_ENTRIES + 50).map(|i| format!("{} launch a\n", 1_700_000_000 + i)).collect();
        std::fs::write(&path, many).unwrap();
        record(&paths, "default", "sign-in", "").unwrap();
        let entries = read(&paths, "default");
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries.last().unwrap().what, "sign-in");
        forget(&paths, "default");
        assert!(read(&paths, "default").is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
