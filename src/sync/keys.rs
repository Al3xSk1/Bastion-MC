//! Which settings follow you from instance to instance, and what a value of each may look like.
//!
//! Only settings that mean the same thing in every Minecraft family from 1.20.1 to 26.3 are
//! on the list: video, sound volumes, mouse and controls, key bindings, language, chat and
//! accessibility. Never anything that belongs to one world, one server or one instance — the
//! last server, tutorial and first-run flags, the resource-pack lists (packs are verified per
//! instance) — and never the choices that are about privacy or about this machine's devices
//! (telemetry, server listing, presence, the sound device, the graphics backend).
//!
//! A value is only synced if it has the shape the game itself writes for that setting
//! ([`Kind`]); anything else stays in the instance it came from. That keeps one instance's
//! damaged or hostile `options.txt` from spreading to the others: what crosses is a boolean,
//! a number, a short identifier or a key name, never free text.

/// The shape of a value, as Minecraft writes it in `options.txt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `true` / `false`
    Bool,
    /// A decimal integer.
    Int,
    /// A decimal number as Java prints a float or double (`0.5`, `1.0`, `1.0E-4`).
    Float,
    /// A short identifier in double quotes (`"fancy"`, `"right"`, `"DEFAULT"`).
    Quoted,
    /// A language code (`en_us`, `enws`).
    Lang,
    /// A key name (`key.keyboard.left.shift`, `key.mouse.4`).
    Key,
}

/// The settings that are synced, with the shape of their values.
const LIST: &[(&str, Kind)] = &[
    // Video
    ("ao", Kind::Bool),
    ("attackIndicator", Kind::Int),
    ("biomeBlendRadius", Kind::Int),
    ("bobView", Kind::Bool),
    ("chunkSectionFadeInTime", Kind::Float),
    ("cloudRange", Kind::Int),
    ("cutoutLeaves", Kind::Bool),
    ("damageTiltStrength", Kind::Float),
    ("darknessEffectScale", Kind::Float),
    ("enableVsync", Kind::Bool),
    ("entityDistanceScaling", Kind::Float),
    ("entityShadows", Kind::Bool),
    ("exclusiveFullscreen", Kind::Bool),
    ("fov", Kind::Float),
    ("fovEffectScale", Kind::Float),
    ("fullscreen", Kind::Bool),
    ("gamma", Kind::Float),
    ("glintSpeed", Kind::Float),
    ("glintStrength", Kind::Float),
    ("graphicsMode", Kind::Int),
    ("graphicsPreset", Kind::Quoted),
    ("guiScale", Kind::Int),
    ("improvedTransparency", Kind::Bool),
    ("inactivityFpsLimit", Kind::Quoted),
    ("maxAnisotropyBit", Kind::Int),
    ("maxFps", Kind::Int),
    // An integer from 0 to 10 since 1.21 (DataVersion 3945), a number from 0 to 1 in
    // 1.20.5/1.20.6: see `translate`.
    ("menuBackgroundBlurriness", Kind::Float),
    ("mipmapLevels", Kind::Int),
    ("particles", Kind::Int),
    ("pauseOnLostFocus", Kind::Bool),
    ("prioritizeChunkUpdates", Kind::Int),
    ("renderClouds", Kind::Quoted),
    ("renderDistance", Kind::Int),
    ("screenEffectScale", Kind::Float),
    ("showAutosaveIndicator", Kind::Bool),
    ("simulationDistance", Kind::Int),
    ("textureFiltering", Kind::Int),
    ("vignette", Kind::Bool),
    ("weatherRadius", Kind::Int),
    // Sound (the volumes are the `soundCategory_` prefix below)
    ("directionalAudio", Kind::Bool),
    ("musicFrequency", Kind::Quoted),
    ("musicToast", Kind::Quoted),
    ("showNowPlayingToast", Kind::Bool),
    ("showSubtitles", Kind::Bool),
    // Mouse and controls (the bindings are the `key_` prefix below)
    ("allowCursorChanges", Kind::Bool),
    ("autoJump", Kind::Bool),
    ("ctrlClickEmulatesRightClick", Kind::Bool),
    ("discrete_mouse_scroll", Kind::Bool),
    ("invertXMouse", Kind::Bool),
    ("invertYMouse", Kind::Bool),
    ("mainHand", Kind::Quoted),
    ("mouseSensitivity", Kind::Float),
    ("mouseWheelSensitivity", Kind::Float),
    ("quitShortcuts", Kind::Bool),
    ("rawMouseInput", Kind::Bool),
    ("rotateWithMinecart", Kind::Bool),
    ("sprintWindow", Kind::Int),
    ("toggleAttack", Kind::Bool),
    ("toggleCrouch", Kind::Bool),
    ("toggleSprint", Kind::Bool),
    ("toggleUse", Kind::Bool),
    ("touchscreen", Kind::Bool),
    // Language
    ("forceUnicodeFont", Kind::Bool),
    ("japaneseGlyphVariants", Kind::Bool),
    ("lang", Kind::Lang),
    // Chat
    ("autoSuggestions", Kind::Bool),
    ("backgroundForChatOnly", Kind::Bool),
    ("chatColors", Kind::Bool),
    ("chatDelay", Kind::Float),
    ("chatHeightFocused", Kind::Float),
    ("chatHeightUnfocused", Kind::Float),
    ("chatLineSpacing", Kind::Float),
    ("chatLinks", Kind::Bool),
    ("chatLinksPrompt", Kind::Bool),
    ("chatOpacity", Kind::Float),
    ("chatScale", Kind::Float),
    ("chatVisibility", Kind::Int),
    ("chatWidth", Kind::Float),
    ("hideMatchedNames", Kind::Bool),
    ("onlyShowSecureChat", Kind::Bool),
    ("saveChatDrafts", Kind::Bool),
    ("textBackgroundOpacity", Kind::Float),
    // Accessibility
    ("darkMojangStudiosBackground", Kind::Bool),
    ("hideLightningFlashes", Kind::Bool),
    ("hideSplashTexts", Kind::Bool),
    ("highContrast", Kind::Bool),
    ("highContrastBlockOutline", Kind::Bool),
    ("narrator", Kind::Int),
    ("narratorHotkey", Kind::Bool),
    ("notificationDisplayTime", Kind::Float),
    ("panoramaScrollSpeed", Kind::Float),
];

/// Settings that are deliberately never synced, and why — for the documentation and for a
/// test that keeps the two lists apart.
pub const NEVER: &[(&str, &str)] = &[
    ("version", "the DataVersion of the game that wrote the file"),
    ("resourcePacks", "packs are verified per instance"),
    ("incompatibleResourcePacks", "packs are verified per instance"),
    ("lastServer", "one server"),
    ("joinedFirstServer", "a first-run flag"),
    ("skipMultiplayerWarning", "a first-run flag"),
    ("skipRealms32bitWarning", "a first-run flag"),
    ("tutorialStep", "one world's tutorial"),
    ("hideBundleTutorial", "a tutorial flag"),
    ("onboardAccessibility", "a first-run flag"),
    ("startedCleanly", "crash detection of this instance"),
    ("realmsNotifications", "a Realms prompt"),
    ("hideServerAddress", "server-related"),
    ("allowServerListing", "a privacy choice"),
    ("telemetryOptInExtra", "a privacy choice"),
    ("sharePresence", "a privacy choice"),
    ("inGameNotification", "a privacy choice"),
    ("advancedItemTooltips", "a debug view"),
    ("reducedDebugInfo", "a debug view"),
    ("debugGuiScale", "a debug view"),
    ("glDebugVerbosity", "a debug setting"),
    ("operatorItemsTab", "an operator setting"),
    ("overrideWidth", "this window"),
    ("overrideHeight", "this window"),
    ("fullscreenResolution", "this monitor"),
    ("soundDevice", "this machine's sound device"),
    ("preferredGraphicsBackend", "this machine's graphics driver"),
    ("useNativeTransport", "networking internals"),
    ("syncChunkWrites", "world storage internals"),
    ("macFullscreenMenuVisibility", "macOS only"),
];

/// Key bindings: `key_<action>` — vanilla actions are `key_key.<name>`, mods add their own.
const KEY_PREFIX: &str = "key_";
/// Volumes: `soundCategory_<category>`.
const VOLUME_PREFIX: &str = "soundCategory_";
/// The longest setting name that is synced.
const MAX_NAME: usize = 96;
/// The longest value that is synced.
pub const MAX_VALUE: usize = 64;

/// Is this setting synced, and what may its value look like?
pub fn kind_of(name: &str) -> Option<Kind> {
    if name.is_empty()
        || name.len() > MAX_NAME
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return None;
    }
    if let Some(action) = name.strip_prefix(KEY_PREFIX) {
        return (!action.is_empty()).then_some(Kind::Key);
    }
    if let Some(category) = name.strip_prefix(VOLUME_PREFIX) {
        return (!category.is_empty() && category.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
            .then_some(Kind::Float);
    }
    LIST.iter().find(|(n, _)| *n == name).map(|(_, kind)| *kind)
}

/// Does `value` have the shape the game writes for a setting of this kind?
pub fn value_fits(kind: Kind, value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_VALUE {
        return false;
    }
    match kind {
        Kind::Bool => value == "true" || value == "false",
        Kind::Int => {
            let digits = value.strip_prefix('-').unwrap_or(value);
            !digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|b| b.is_ascii_digit())
        }
        Kind::Float => {
            value.len() <= 24
                && value.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'E' | b'e'))
                && value.bytes().any(|b| b.is_ascii_digit())
                && value.parse::<f64>().is_ok_and(f64::is_finite)
        }
        Kind::Quoted => value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .is_some_and(|inner| inner.len() <= 32 && inner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')),
        Kind::Lang => {
            let (base, region) = value.split_once('_').unwrap_or((value, ""));
            (2..=4).contains(&base.len())
                && base.bytes().all(|b| b.is_ascii_lowercase())
                && (region.is_empty() && !value.contains('_')
                    || (2..=8).contains(&region.len())
                        && region.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
        }
        Kind::Key => ["key.keyboard.", "key.mouse."].iter().any(|prefix| {
            value.strip_prefix(prefix).is_some_and(|rest| {
                !rest.is_empty()
                    && rest.len() <= 40
                    && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_'))
                    && !rest.starts_with('.')
                    && !rest.ends_with('.')
                    && !rest.contains("..")
            })
        }),
    }
}

/// Is the pair `name:value` one that may be synced?
pub fn syncable(name: &str, value: &str) -> bool {
    kind_of(name).is_some_and(|kind| value_fits(kind, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_and_the_exclusions_never_overlap() {
        for (name, why) in NEVER {
            assert_eq!(kind_of(name), None, "{name} is excluded ({why}) and must not be synced");
        }
        let mut names: Vec<&str> = LIST.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "a setting is listed twice");
        // Settings for one world, one server or one instance never look like synced ones.
        for name in ["modelPart_cape", "version", "resourcePacks", "key", "soundCategory_", "lastServer"] {
            assert_eq!(kind_of(name), None, "{name}");
        }
    }

    #[test]
    fn every_setting_of_three_real_files_is_either_synced_or_deliberately_not() {
        // The real files of three families (tests/data/options): nothing in them is left
        // undecided — a setting a new version adds shows up here until it is classified.
        for file in ["1.20.1.txt", "1.21.4.txt", "26.3.txt"] {
            let text =
                std::fs::read_to_string(format!("{}/tests/data/options/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
            for line in text.lines() {
                let (name, value) = line.split_once(':').unwrap();
                if name.starts_with("modelPart_") {
                    continue; // skin customisation: not among the synced categories
                }
                match kind_of(name) {
                    Some(kind) => assert!(value_fits(kind, value), "{file}: {name}:{value} does not fit {kind:?}"),
                    None => {
                        assert!(NEVER.iter().any(|(n, _)| *n == name), "{file}: {name} is neither synced nor excluded")
                    }
                }
            }
        }
    }

    #[test]
    fn values_must_have_the_shape_the_game_writes() {
        for (kind, good, bad) in [
            (Kind::Bool, &["true", "false"][..], &["True", "1", "", "yes"][..]),
            (Kind::Int, &["0", "12", "-1", "120"], &["1.0", "", "-", "1234567890", "0x10", "+1"]),
            (Kind::Float, &["0.0", "0.4375", "1.0E-4", "-0.5", "1"], &["NaN", "inf", "1,0", "", ".", "1e999"]),
            (Kind::Quoted, &["\"fancy\"", "\"DEFAULT\"", "\"\""], &["fancy", "\"a b\"", "\"x\\\"\"", "\"é\""]),
            (Kind::Lang, &["en_us", "enws", "lol_us", "qya_aa", "zh_cn"], &["EN_US", "en-us", "e", "en_", "_us"]),
            (
                Kind::Key,
                &["key.keyboard.w", "key.mouse.left", "key.keyboard.left.shift", "key.mouse.4", "key.keyboard.unknown"],
                &["key.keyboard.", "key.joystick.a", "w", "key.keyboard.W", "key.keyboard..a", "key.mouse.left "],
            ),
        ] {
            for value in good {
                assert!(value_fits(kind, value), "{kind:?} {value:?}");
            }
            for value in bad {
                assert!(!value_fits(kind, value), "{kind:?} {value:?}");
            }
        }
        assert!(!value_fits(Kind::Key, &format!("key.keyboard.{}", "a".repeat(60))));
        assert!(syncable("key_key.jump", "key.keyboard.space"));
        assert!(syncable("key_key.voice_chat", "key.keyboard.v"), "a mod's binding is a binding");
        assert!(syncable("soundCategory_ui", "0.5"));
        assert!(!syncable("soundCategory_Master", "0.5"));
        assert!(!syncable("lang", "en_us\u{1b}[31m"));
    }
}
