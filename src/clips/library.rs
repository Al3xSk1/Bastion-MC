//! The clips folder: the saved clips, their thumbnails, and the index that says which files
//! this feature made.
//!
//! **Only clips this feature made are ever removed.** The index (`.bastion-clips.json` in the
//! folder, written by the launcher) records each clip it saved: its id, its files, its size.
//! A clip is removed — by `delete`, or to stay within the caps — only through its index
//! entry, and its file only if it is still a regular file of the recorded size under the
//! recorded name. Anything else in the folder is never looked at. An index that does not
//! parse is treated as empty (nothing is removed on its account) and replaced at the next save.
//!
//! **Names are cleaned.** What the phone sends for `rename` becomes part of a file name only
//! as letters, digits, `_` and `-` (spaces become `_`), at most [`MAX_NAME`] characters,
//! followed by the clip's id: it cannot name another directory, a hidden file or another
//! clip's file.

use std::{
    fs::File,
    os::fd::AsFd,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::util;

/// The index's name in the clips folder.
pub const INDEX: &str = ".bastion-clips.json";
const MAX_INDEX_BYTES: u64 = 1 << 20;
/// Clips remembered in the index (the caps keep far fewer).
pub const MAX_ENTRIES: usize = 1000;
/// A clip's name, in characters.
pub const MAX_NAME: usize = 48;
pub const MAX_THUMB_BYTES: u64 = 2 << 20;

/// One clip this feature saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    /// Twelve hexadecimal digits.
    pub id: String,
    /// What the phone shows (cleaned).
    pub name: String,
    /// `<stem>.ts` in the folder.
    pub file: String,
    /// `<stem>.png`, if a thumbnail was saved.
    pub thumb: Option<String>,
    /// Unix seconds.
    pub created: u64,
    pub seconds: f64,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexJson {
    version: u32,
    clips: Vec<Clip>,
}

pub fn is_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A name for a clip from what the phone sent: letters, digits, spaces, `_` and `-`; runs of
/// spaces become one; at most [`MAX_NAME`] characters. `None` if nothing is left.
pub fn clean_name(text: &str) -> Option<String> {
    let mut out = String::new();
    for c in text.chars() {
        if out.chars().count() == MAX_NAME {
            break;
        }
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else if c.is_whitespace() && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
    }
    let out = out.trim().to_string();
    out.bytes().any(|b| b.is_ascii_alphanumeric()).then_some(out)
}

/// The file stem for a clip: the cleaned name with `_` for spaces, then `-<id>`.
pub fn stem(name: &str, id: &str) -> String {
    format!("{}-{id}", name.replace(' ', "_"))
}

fn valid(clip: &Clip) -> bool {
    let Some(name) = clean_name(&clip.name) else { return false };
    let stem = stem(&name, &clip.id);
    is_id(&clip.id)
        && name == clip.name
        && clip.file == format!("{stem}.ts")
        && clip.thumb.as_ref().is_none_or(|t| *t == format!("{stem}.png"))
        && clip.seconds.is_finite()
        && (0.0..=3600.0).contains(&clip.seconds)
        && clip.bytes <= super::buffer::MAX_CLIP_BYTES
}

/// The folder and what the index says is in it (oldest first).
pub struct Library {
    pub dir: PathBuf,
    pub clips: Vec<Clip>,
    /// Why the index was not used, if it was not.
    pub note: Option<String>,
}

impl Library {
    /// Read the index of `dir` (which must exist). An unusable index is empty, with a note.
    pub fn open(dir: &Path) -> Library {
        let mut library = Library { dir: dir.to_path_buf(), clips: Vec::new(), note: None };
        let path = dir.join(INDEX);
        match util::read_small_file(&path, MAX_INDEX_BYTES) {
            Err(_) if !path.exists() => {}
            Err(e) => library.note = Some(format!("the clips index could not be read ({e}); starting a new one")),
            Ok(raw) => match parse_index(&raw) {
                Ok(clips) => library.clips = clips,
                Err(why) => library.note = Some(format!("the clips index is not usable ({why}); starting a new one")),
            },
        }
        library
    }

    pub fn save(&self) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(&IndexJson { version: 1, clips: self.clips.clone() })
            .map_err(|e| e.to_string())?;
        util::write_atomic(&self.dir.join(INDEX), &json, 0o600).map_err(|e| e.to_string())
    }

    pub fn total_bytes(&self) -> u64 {
        self.clips.iter().map(|c| c.bytes).sum()
    }

    pub fn find(&self, id: &str) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    /// The newest `n`, newest first.
    pub fn newest(&self, n: usize) -> Vec<&Clip> {
        self.clips.iter().rev().take(n).collect()
    }

    fn dir_fd(&self) -> Result<File, String> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&self.dir)
            .map_err(|e| format!("the clips folder cannot be opened ({e})"))
    }

    /// Remove a file of a clip if it is still what was saved: a regular file (not a link),
    /// and for the clip itself, of the recorded size. Gone already: fine.
    fn remove_file(dir: &File, name: &str, size: Option<u64>) -> Result<(), String> {
        match util::openat_regular_nofollow(dir.as_fd(), std::ffi::OsStr::new(name)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(format!("{name} is no longer a file this feature saved; it was left alone")),
            Ok(file) => {
                if size.is_some_and(|s| file.metadata().map(|m| m.len()).ok() != Some(s)) {
                    return Err(format!("{name} has changed since it was saved; it was left alone"));
                }
            }
        }
        nix::unistd::unlinkat(dir, name, nix::unistd::UnlinkatFlags::NoRemoveDir)
            .map_err(|e| format!("removing {name}: {e}"))
    }

    /// Delete a clip (its files, then its entry). A file that changed is left, and the entry
    /// forgotten.
    pub fn delete(&mut self, id: &str) -> Result<Clip, String> {
        let at = self.clips.iter().position(|c| c.id == id).ok_or("no such clip")?;
        let clip = self.clips.remove(at);
        let dir = self.dir_fd()?;
        let outcome = Self::remove_file(&dir, &clip.file, Some(clip.bytes))
            .and(clip.thumb.as_deref().map_or(Ok(()), |t| Self::remove_file(&dir, t, None)));
        self.save()?;
        outcome.map(|()| clip)
    }

    /// Give a clip a new name (its files are renamed; neither may replace another file).
    pub fn rename(&mut self, id: &str, text: &str) -> Result<Clip, String> {
        let name = clean_name(text).ok_or("a clip's name needs letters or digits")?;
        let at = self.clips.iter().position(|c| c.id == id).ok_or("no such clip")?;
        let dir = self.dir_fd()?;
        let old = self.clips[at].clone();
        let new_stem = stem(&name, id);
        let (file, thumb) = (format!("{new_stem}.ts"), old.thumb.as_ref().map(|_| format!("{new_stem}.png")));
        let move_file = |from: &str, to: &str| {
            if from == to {
                return Ok(());
            }
            nix::fcntl::renameat2(&dir, from, &dir, to, nix::fcntl::RenameFlags::RENAME_NOREPLACE)
                .map_err(|e| format!("renaming the clip: {e}"))
        };
        move_file(&old.file, &file)?;
        if let (Some(from), Some(to)) = (&old.thumb, &thumb)
            && move_file(from, to).is_err()
        {
            // The clip is renamed; a thumbnail that could not follow is dropped from the entry.
            let _ = Self::remove_file(&dir, from, None);
            self.clips[at].thumb = None;
        } else {
            self.clips[at].thumb = thumb;
        }
        self.clips[at].name = name;
        self.clips[at].file = file;
        self.save()?;
        Ok(self.clips[at].clone())
    }

    /// Remove the oldest clips until there are at most `max_clips` and `max_bytes` in all.
    /// Returns what was removed (and what could not be, as errors in the same list).
    pub fn enforce(&mut self, max_clips: usize, max_bytes: u64) -> Vec<Result<String, String>> {
        let mut done = Vec::new();
        while !self.clips.is_empty() && (self.clips.len() > max_clips || self.total_bytes() > max_bytes) {
            let id = self.clips[0].id.clone();
            done.push(self.delete(&id).map(|c| c.name));
        }
        done
    }
}

/// The index's contents, strictly: every entry valid, ids unique, at most [`MAX_ENTRIES`].
pub fn parse_index(raw: &[u8]) -> Result<Vec<Clip>, String> {
    let index: IndexJson = serde_json::from_slice(raw).map_err(|_| "not an index of this version".to_string())?;
    if index.version != 1 || index.clips.len() > MAX_ENTRIES {
        return Err("not an index of this version".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for clip in &index.clips {
        if !valid(clip) || !seen.insert(clip.id.clone()) {
            return Err("an entry is malformed".into());
        }
    }
    Ok(index.clips)
}

/// The index parser and the name cleaner on arbitrary bytes.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    if let Ok(clips) = parse_index(data) {
        for clip in clips {
            assert!(is_id(&clip.id) && !clip.file.contains('/') && !clip.file.starts_with('.'));
        }
    }
    if let Ok(text) = std::str::from_utf8(data)
        && let Some(name) = clean_name(text)
    {
        assert!(name.chars().count() <= MAX_NAME);
        assert_eq!(clean_name(&name).as_deref(), Some(name.as_str()), "cleaning is idempotent");
        let stem = stem(&name, "0123456789ab");
        assert!(!stem.contains('/') && !stem.starts_with('.') && !stem.contains(".."), "{stem}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bmc-clips-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
        util::create_private_dir_all(&dir).unwrap();
        dir
    }

    fn add(library: &mut Library, name: &str, bytes: usize) -> Clip {
        let id = util::hex_encode(&util::random_bytes::<6>().unwrap());
        let stem = stem(name, &id);
        std::fs::write(library.dir.join(format!("{stem}.ts")), vec![0x47; bytes]).unwrap();
        std::fs::write(library.dir.join(format!("{stem}.png")), b"png").unwrap();
        let clip = Clip {
            id,
            name: name.into(),
            file: format!("{stem}.ts"),
            thumb: Some(format!("{stem}.png")),
            created: 1_763_047_800,
            seconds: 30.0,
            bytes: bytes as u64,
        };
        library.clips.push(clip.clone());
        library.save().unwrap();
        clip
    }

    #[test]
    fn names_are_cleaned_into_safe_file_names() {
        assert_eq!(clean_name("  Epic   jump! ").as_deref(), Some("Epic jump"));
        assert_eq!(clean_name("../../etc/passwd").as_deref(), Some("etcpasswd"));
        assert_eq!(clean_name(".hidden").as_deref(), Some("hidden"));
        assert_eq!(clean_name("Ümlaut ☕ ok").as_deref(), Some("mlaut ok"));
        assert_eq!(clean_name("!!! ..."), None);
        assert_eq!(clean_name(&"x".repeat(100)).unwrap().len(), MAX_NAME);
        assert_eq!(stem("Epic jump", "0123456789ab"), "Epic_jump-0123456789ab");
    }

    #[test]
    fn only_clips_this_feature_made_are_removed() {
        let dir = scratch();
        let mut library = Library::open(&dir);
        let a = add(&mut library, "first", 1000);
        let b = add(&mut library, "second", 2000);
        let c = add(&mut library, "third", 3000);
        std::fs::write(dir.join("my-own-video.ts"), b"the user's").unwrap();
        // Reopened from the index: the same clips.
        assert_eq!(Library::open(&dir).clips, [a.clone(), b.clone(), c.clone()]);
        // Caps: two clips at most → the oldest goes, files and entry.
        let removed = library.enforce(2, u64::MAX);
        assert_eq!(removed, [Ok("first".to_string())]);
        assert!(!dir.join(&a.file).exists() && !dir.join(a.thumb.as_ref().unwrap()).exists());
        // A clip whose file was replaced by something else: the entry goes, the file stays.
        std::fs::write(dir.join(&b.file), b"replaced").unwrap();
        assert!(library.delete(&b.id).unwrap_err().contains("changed"));
        assert!(dir.join(&b.file).exists());
        assert!(library.find(&b.id).is_none());
        // A link in place of a clip is not followed, not removed.
        std::fs::remove_file(dir.join(&c.file)).unwrap();
        std::os::unix::fs::symlink(dir.join("my-own-video.ts"), dir.join(&c.file)).unwrap();
        assert!(library.delete(&c.id).is_err());
        assert!(dir.join("my-own-video.ts").exists() && dir.join(&c.file).is_symlink());
        assert_eq!(std::fs::read(dir.join("my-own-video.ts")).unwrap(), b"the user's");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn renaming_moves_both_files_and_never_replaces_one() {
        let dir = scratch();
        let mut library = Library::open(&dir);
        let a = add(&mut library, "first", 10);
        let renamed = library.rename(&a.id, "Best moment!").unwrap();
        assert_eq!(renamed.name, "Best moment");
        assert_eq!(renamed.file, format!("Best_moment-{}.ts", a.id));
        assert!(dir.join(&renamed.file).exists() && dir.join(renamed.thumb.unwrap()).exists());
        assert!(!dir.join(&a.file).exists());
        assert!(library.rename(&a.id, "///").is_err());
        assert!(library.rename("000000000000", "x").is_err());
        // An occupied target name is not overwritten.
        std::fs::write(dir.join(format!("taken-{}.ts", a.id)), b"x").unwrap();
        assert!(library.rename(&a.id, "taken").is_err());
        assert_eq!(std::fs::read(dir.join(format!("taken-{}.ts", a.id))).unwrap(), b"x");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_index_is_set_aside_not_trusted() {
        let dir = scratch();
        for (raw, why) in [
            (&br#"{"version":1,"clips":[{"id":"x","name":"a","file":"../x.ts","thumb":null,"created":1,"seconds":1.0,"bytes":1}]}"#[..], "malformed"),
            (br#"{"version":2,"clips":[]}"#, "version"),
            (b"garbage", "version"),
        ] {
            std::fs::write(dir.join(INDEX), raw).unwrap();
            let library = Library::open(&dir);
            assert!(library.clips.is_empty());
            assert!(library.note.as_deref().is_some_and(|n| n.contains(why)), "{:?}", library.note);
        }
        let ok = br#"{"version":1,"clips":[{"id":"0123456789ab","name":"a b","file":"a_b-0123456789ab.ts","thumb":"a_b-0123456789ab.png","created":1,"seconds":1.5,"bytes":188}]}"#;
        assert_eq!(parse_index(ok).unwrap().len(), 1);
        fuzz(ok);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
