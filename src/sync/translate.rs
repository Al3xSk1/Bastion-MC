//! Settings whose name or format changed between the families: translated where Mojang's own
//! data fixers say exactly what the new form of an old value is, and skipped otherwise.
//!
//! Between 1.20.1 (DataVersion 3465) and 26.3 (5023) three synced settings changed. Each rule
//! below is the one the game itself applies when it loads an older `options.txt` (the fixers
//! in `net.minecraft.util.datafix.fixes` of 26.3, at the DataVersion given); the way back is
//! the exact inverse where there is one, and nothing where there is not:
//!
//! | setting | until | from | old → new (Mojang's fixer) | new → old |
//! |---|---|---|---|---|
//! | `menuBackgroundBlurriness` | 3944 | 3945 | number 0–1 → `round(x·10)` | `n/10`, for n in 0…10 |
//! | `graphicsMode` 0/1/2 | 4650 | 4651 | → `cutoutLeaves` false/true/true, `weatherRadius` 5/10/10, `vignette` false/true/true, `improvedTransparency` false/false/true, and `graphicsPreset` `"custom"` (4656) | only when the four match one of the three rows exactly |
//! | `showNowPlayingToast` | 4762 | 4763 | false → `musicToast` `"never"`, true → `"pause_and_toast"` | `"never"` → false, `"pause_and_toast"` → true, other values skipped |
//!
//! Key bindings keep their format (`key.keyboard.*`, `key.mouse.*`) across the whole range —
//! checked against real files of 1.20.1, 1.21.4 and 26.3. A binding for an action that a
//! family does not have is not added to that family's file.

use std::collections::BTreeMap;

/// The oldest DataVersion settings are synced from or to: Minecraft 1.20.1. Older files have
/// formats this module does not translate (`ao` was a number before 3319, for one).
pub const OLDEST: u32 = 3465;
/// The newest DataVersion whose formats were checked: Minecraft 26.3. Newer ones are synced
/// as if nothing had changed since; a format change this module does not know would make the
/// game fall back to its default for that one setting.
pub const NEWEST_KNOWN: u32 = 5023;

const BLUR_INT: u32 = 3945;
const GRAPHICS_SPLIT: u32 = 4651;
const GRAPHICS_PRESET: u32 = 4656;
const MUSIC_TOAST: u32 = 4763;

/// The split of `graphicsMode` (fast, fancy, fabulous): Mojang's `OptionsGraphicsModeSplitFix`.
const SPLIT: [(&str, [&str; 3]); 4] = [
    ("cutoutLeaves", ["false", "true", "true"]),
    ("weatherRadius", ["5", "10", "10"]),
    ("vignette", ["false", "true", "true"]),
    ("improvedTransparency", ["false", "false", "true"]),
];

/// One synced value: what it is, which DataVersion's format it is in, and when it was set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Synced {
    pub value: String,
    pub data_version: u32,
    pub changed: u64,
}

/// What a game of DataVersion `target` should be given: name → (value in its format, when
/// that value was set). Several stored settings may produce the same name; the most recently
/// set one wins.
pub fn view_for(stored: &BTreeMap<String, Synced>, target: u32) -> BTreeMap<String, (String, u64)> {
    let mut view: BTreeMap<String, (String, u64)> = BTreeMap::new();
    let mut offer = |name: &str, value: String, changed: u64| {
        let newer = view.get(name).is_none_or(|(_, have)| changed > *have);
        if newer {
            view.insert(name.to_string(), (value, changed));
        }
    };
    if target < OLDEST {
        return view;
    }
    for (name, entry) in stored {
        if entry.data_version < OLDEST {
            continue;
        }
        for (name, value) in translate(name, &entry.value, entry.data_version, target) {
            offer(&name, value, entry.changed);
        }
    }
    // New → old for the graphics mode needs four settings at once.
    if target < GRAPHICS_SPLIT {
        let parts: Option<Vec<&Synced>> =
            SPLIT.iter().map(|(name, _)| stored.get(*name).filter(|s| s.data_version >= GRAPHICS_SPLIT)).collect();
        if let Some(parts) = parts {
            let mode = (0..3).find(|mode| SPLIT.iter().zip(&parts).all(|((_, row), part)| part.value == row[*mode]));
            if let Some(mode) = mode {
                let changed = parts.iter().map(|p| p.changed).max().unwrap_or(0);
                offer("graphicsMode", mode.to_string(), changed);
            }
        }
    }
    view
}

/// The form of one stored setting for a game of DataVersion `target`: none, one or several
/// name–value pairs.
fn translate(name: &str, value: &str, from: u32, target: u32) -> Vec<(String, String)> {
    let one = |name: &str, value: &str| vec![(name.to_string(), value.to_string())];
    match name {
        "menuBackgroundBlurriness" => match (from >= BLUR_INT, target >= BLUR_INT) {
            (false, true) => match value.parse::<f32>() {
                // Java's Math.round(float): floor(x + 0.5).
                Ok(x) if x.is_finite() && (0.0..=1.0).contains(&x) => {
                    one(name, &((x * 10.0 + 0.5).floor() as i32).to_string())
                }
                _ => Vec::new(),
            },
            (true, false) => match value.parse::<u32>() {
                Ok(10) => one(name, "1.0"),
                Ok(n) if n < 10 => one(name, &format!("0.{n}")),
                _ => Vec::new(),
            },
            _ => one(name, value),
        },
        "graphicsMode" => {
            if from >= GRAPHICS_SPLIT {
                return Vec::new(); // not a setting of that family
            }
            if target < GRAPHICS_SPLIT {
                return one(name, value);
            }
            let Some(mode) = value.parse::<usize>().ok().filter(|m| *m < 3) else { return Vec::new() };
            let mut out: Vec<(String, String)> =
                SPLIT.iter().map(|(name, row)| (name.to_string(), row[mode].to_string())).collect();
            if target >= GRAPHICS_PRESET {
                out.push(("graphicsPreset".into(), "\"custom\"".into()));
            }
            out
        }
        // The four parts and the preset exist from the split on; back to an older family
        // they only travel together, as `graphicsMode` (see `view_for`).
        "cutoutLeaves" | "weatherRadius" | "vignette" | "improvedTransparency" => {
            if target >= GRAPHICS_SPLIT {
                one(name, value)
            } else {
                Vec::new()
            }
        }
        "graphicsPreset" => {
            if target >= GRAPHICS_PRESET {
                one(name, value)
            } else {
                Vec::new()
            }
        }
        "showNowPlayingToast" => {
            if from >= MUSIC_TOAST {
                return Vec::new();
            }
            if target < MUSIC_TOAST {
                return one(name, value);
            }
            match value {
                "false" => one("musicToast", "\"never\""),
                "true" => one("musicToast", "\"pause_and_toast\""),
                _ => Vec::new(),
            }
        }
        "musicToast" => {
            if target >= MUSIC_TOAST {
                return one(name, value);
            }
            match value {
                "\"never\"" => one("showNowPlayingToast", "false"),
                "\"pause_and_toast\"" => one("showNowPlayingToast", "true"),
                _ => Vec::new(),
            }
        }
        _ => one(name, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(entries: &[(&str, &str, u32, u64)]) -> BTreeMap<String, Synced> {
        entries
            .iter()
            .map(|(n, v, dv, t)| (n.to_string(), Synced { value: v.to_string(), data_version: *dv, changed: *t }))
            .collect()
    }

    fn get<'a>(view: &'a BTreeMap<String, (String, u64)>, name: &str) -> Option<&'a str> {
        view.get(name).map(|(v, _)| v.as_str())
    }

    #[test]
    fn settings_whose_format_did_not_change_pass_unchanged_in_both_directions() {
        let s =
            store(&[("fov", "0.25", 3465, 1), ("key_key.jump", "key.keyboard.j", 5023, 2), ("lang", "de_de", 4189, 3)]);
        for target in [3465, 4189, 4671, 5023, 6000] {
            let v = view_for(&s, target);
            assert_eq!(get(&v, "fov"), Some("0.25"));
            assert_eq!(get(&v, "key_key.jump"), Some("key.keyboard.j"));
            assert_eq!(get(&v, "lang"), Some("de_de"));
        }
        // Older than 1.20.1, either side: nothing.
        assert!(view_for(&s, 3337).is_empty());
        assert!(view_for(&store(&[("fov", "0.25", 3337, 1)]), 5023).is_empty());
    }

    #[test]
    fn the_graphics_mode_is_split_as_mojang_splits_it_and_joined_only_when_exact() {
        for (mode, leaves, radius, vignette, transparency) in [
            ("0", "false", "5", "false", "false"),
            ("1", "true", "10", "true", "false"),
            ("2", "true", "10", "true", "true"),
        ] {
            let v = view_for(&store(&[("graphicsMode", mode, 4189, 7)]), 5023);
            assert_eq!(get(&v, "cutoutLeaves"), Some(leaves));
            assert_eq!(get(&v, "weatherRadius"), Some(radius));
            assert_eq!(get(&v, "vignette"), Some(vignette));
            assert_eq!(get(&v, "improvedTransparency"), Some(transparency));
            assert_eq!(get(&v, "graphicsPreset"), Some("\"custom\""));
            assert_eq!(get(&v, "graphicsMode"), None, "26.3 has no graphicsMode");
            // …and back.
            let new = store(&[
                ("cutoutLeaves", leaves, 5023, 1),
                ("weatherRadius", radius, 5023, 2),
                ("vignette", vignette, 5023, 3),
                ("improvedTransparency", transparency, 5023, 4),
            ]);
            let old = view_for(&new, 3465);
            assert_eq!(get(&old, "graphicsMode"), Some(mode));
            assert_eq!(old["graphicsMode"].1, 4, "as recent as the most recent part");
            assert_eq!(get(&old, "cutoutLeaves"), None, "1.20.1 has none of the parts");
        }
        // A combination no mode produces, or a missing part: no graphicsMode at all.
        let odd = store(&[
            ("cutoutLeaves", "false", 5023, 1),
            ("weatherRadius", "10", 5023, 1),
            ("vignette", "true", 5023, 1),
            ("improvedTransparency", "false", 5023, 1),
        ]);
        assert_eq!(get(&view_for(&odd, 4189), "graphicsMode"), None);
        let partial = store(&[("cutoutLeaves", "true", 5023, 1), ("weatherRadius", "10", 5023, 1)]);
        assert_eq!(get(&view_for(&partial, 4189), "graphicsMode"), None);
        // A preset of 26.x says nothing certain about an old mode.
        assert!(view_for(&store(&[("graphicsPreset", "\"fancy\"", 5023, 1)]), 4189).is_empty());
        // Nonsense in the old setting: nothing.
        assert!(view_for(&store(&[("graphicsMode", "3", 4189, 1)]), 5023).is_empty());
    }

    #[test]
    fn the_newest_setting_wins_when_two_produce_the_same_name() {
        // Fancy chosen in 1.21.4 at t=9, after leaves were switched off in 26.3 at t=5.
        let s = store(&[("graphicsMode", "1", 4189, 9), ("cutoutLeaves", "false", 5023, 5)]);
        assert_eq!(get(&view_for(&s, 5023), "cutoutLeaves"), Some("true"));
        // The other way round.
        let s = store(&[("graphicsMode", "1", 4189, 5), ("cutoutLeaves", "false", 5023, 9)]);
        assert_eq!(get(&view_for(&s, 5023), "cutoutLeaves"), Some("false"));
        assert_eq!(get(&view_for(&s, 5023), "weatherRadius"), Some("10"));
    }

    #[test]
    fn blurriness_and_the_music_toast_follow_mojangs_fixers() {
        for (old, new) in [("0.5", "5"), ("0.0", "0"), ("1.0", "10"), ("0.25", "3"), ("0.24", "2")] {
            assert_eq!(
                get(&view_for(&store(&[("menuBackgroundBlurriness", old, 3839, 1)]), 3955), "menuBackgroundBlurriness"),
                Some(new),
                "{old}"
            );
        }
        for (new, old) in [("5", "0.5"), ("0", "0.0"), ("10", "1.0")] {
            assert_eq!(
                get(&view_for(&store(&[("menuBackgroundBlurriness", new, 5023, 1)]), 3839), "menuBackgroundBlurriness"),
                Some(old),
                "{new}"
            );
        }
        assert!(view_for(&store(&[("menuBackgroundBlurriness", "11", 5023, 1)]), 3839).is_empty());
        assert!(view_for(&store(&[("menuBackgroundBlurriness", "1.5", 3839, 1)]), 5023).is_empty());

        assert_eq!(
            get(&view_for(&store(&[("showNowPlayingToast", "true", 4671, 1)]), 5023), "musicToast"),
            Some("\"pause_and_toast\"")
        );
        assert_eq!(
            get(&view_for(&store(&[("showNowPlayingToast", "false", 4671, 1)]), 5023), "musicToast"),
            Some("\"never\"")
        );
        assert_eq!(
            get(&view_for(&store(&[("musicToast", "\"never\"", 5023, 1)]), 4671), "showNowPlayingToast"),
            Some("false")
        );
        assert!(view_for(&store(&[("musicToast", "\"toast\"", 5023, 1)]), 4671).is_empty(), "no certain old form");
        assert_eq!(
            get(&view_for(&store(&[("musicToast", "\"toast\"", 5023, 1)]), 5023), "musicToast"),
            Some("\"toast\"")
        );
    }
}
