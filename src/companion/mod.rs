//! The companion mod: zoom and other conveniences for Fabric instances, built from source
//! with the launcher (`java/companion`, `jar`) and added to a launch by the launcher itself.
//!
//! **It enforces nothing.** The mod runs in the game's JVM next to every other mod, any of
//! which can change it, imitate it or switch it off. Nothing the launcher promises depends
//! on it, and nothing it says is believed (`channel`). It is a convenience, and later the
//! place where an in-game view of launcher-side decisions can live.
//!
//! How a launch gets it, when [`decide`] says so:
//!
//! * the jar ([`JAR`], embedded in this binary) and a settings file are materialised
//!   read-only inside the sandbox under [`SANDBOX_DIR`] — outside the game directory and
//!   outside `mods/`, which stays exactly the set of jars the profile's signature covers;
//! * Fabric Loader is pointed at the jar with its `fabric.addMods` system property;
//! * one UNIX socket is bound next to them, the launcher listening on the other side.
//!
//! When: Fabric profiles only (a vanilla instance cannot load a mod; Quilt has a different
//! switch), and only for Minecraft versions the hooks were verified against
//! ([`TESTED_MINECRAFT`]) unless `companion.force` is set. The mod finds the game by name;
//! on a version where a name is missing, that feature switches itself off and says so.
//!
//! Settings come from `[companion]` in config.toml, overridden per instance in
//! instance.toml. They reach the game as a read-only file, so nothing in the game can
//! change them for the next session — and where that file exists the mod reads no other
//! (`Config.java`): a JVM option of a signed profile cannot point it at a file in the game
//! directory. What the sandbox additionally grants for all this is read access to two files
//! and connecting to one socket: no writable path, no network destination, nothing
//! executable.

pub mod channel;
#[cfg(test)]
mod jar;

use std::path::PathBuf;

use serde::Deserialize;

use crate::{
    error::{Error, Result},
    instance::{Instance, Kind},
    paths::Paths,
    profile::manifest::LoaderKind,
    sandbox::{Mount, SUPPORT_DIR},
    util,
};

/// The mod's jar, built by `build.rs` from `java/companion` (see `jar`).
pub const JAR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bastion-companion.jar"));
/// The mod's version: the launcher's own, they are built together.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Its class files need at least this Java release (`jar::CLASS_RELEASE`; a test compares).
pub const MIN_JAVA: u32 = 17;
/// Minecraft versions against whose classes every hook and lookup was checked, in a real
/// launch (`tests/companion_game.rs`). To add one: run that test against it, then list it
/// here.
pub const TESTED_MINECRAFT: &[&str] = &["26.3"];

/// Where the launcher puts the mod's files inside the sandbox.
pub const SANDBOX_DIR: &str = "/bastion/companion";
const JAR_NAME: &str = "bastion-companion.jar";
const SETTINGS_NAME: &str = "config.properties";
const SOCKET_NAME: &str = "channel";
/// The property Fabric Loader reads extra mod paths from (`ArgumentModCandidateFinder`).
const ADD_MODS: &str = "-Dfabric.addMods=";

// ---------------------------------------------------------------- configuration

/// `[companion]` in config.toml.
///
/// To add a feature: its switch (and settings) here and in [`CompanionOverride`], a line in
/// [`CompanionConfig::merged`], [`CompanionConfig::features`] and [`CompanionConfig::settings`]
/// — and the feature itself in `java/companion` (see `Feature.java`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompanionConfig {
    /// Add the companion mod to Fabric instances.
    pub enabled: bool,
    /// Also for Minecraft versions it was not tested with.
    pub force: bool,
    /// Hold a key to zoom.
    pub zoom: bool,
    /// The key, by Minecraft's name for it (as in options.txt): `key.keyboard.c`, or just `c`.
    pub zoom_key: String,
    /// How much the key zooms: the field of view is divided by this.
    pub zoom_factor: f64,
    /// The mouse wheel changes the factor while the key is held.
    pub zoom_wheel: bool,
    /// Frames per second, position and facing in a corner of the screen.
    pub hud: bool,
    /// Brighten dark places. Some servers do not allow this.
    pub fullbright: bool,
    /// What the game's brightness setting (0 to 1) is raised to.
    pub fullbright_gamma: f64,
    /// Test support, deliberately not settable from a configuration file: make the mod run
    /// its scripted self-test once the game has loaded (`tests/companion_game.rs`).
    #[serde(skip)]
    pub self_test: Option<SelfTest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfTest {
    /// Exercise every feature at the title screen.
    Title,
    /// Then enter the demo world and measure there.
    World,
}

impl Default for CompanionConfig {
    fn default() -> Self {
        CompanionConfig {
            enabled: true,
            force: false,
            zoom: true,
            zoom_key: "key.keyboard.c".into(),
            zoom_factor: 4.0,
            zoom_wheel: true,
            hud: false,
            fullbright: false,
            fullbright_gamma: 15.0,
            self_test: None,
        }
    }
}

/// `[companion]` in an instance's instance.toml: any of the keys, for that instance only.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompanionOverride {
    pub enabled: Option<bool>,
    pub force: Option<bool>,
    pub zoom: Option<bool>,
    pub zoom_key: Option<String>,
    pub zoom_factor: Option<f64>,
    pub zoom_wheel: Option<bool>,
    pub hud: Option<bool>,
    pub fullbright: Option<bool>,
    pub fullbright_gamma: Option<f64>,
}

impl CompanionOverride {
    /// The keys set here must be valid whatever the global settings are.
    pub fn validate(&self) -> Result<()> {
        CompanionConfig::default().merged(self).validate()
    }
}

const KEY_PREFIX: &str = "key.keyboard.";

/// Minecraft's name for a keyboard key (`key.keyboard.left.alt`), from that or from its
/// last part alone (`left.alt`, `c`). Only the shape is checked here; whether the game
/// knows a key of that name is for the game to say (the mod reports it).
pub fn key_name(text: &str) -> Result<String> {
    let bad = |why: &str| {
        Error::Config(format!(
            "companion zoom_key {text:?}: {why}. Use Minecraft's name for a keyboard key as it appears in \
             options.txt, with or without the leading \"key.keyboard.\": \"c\", \"left.alt\", \"key.keyboard.f6\""
        ))
    };
    if text.starts_with("key.mouse") {
        return Err(bad("mouse buttons are not supported"));
    }
    let name = text.strip_prefix(KEY_PREFIX).unwrap_or(text);
    let shaped = (1..=40).contains(&name.len())
        && name
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
    if !shaped || name == "unknown" {
        return Err(bad("not the name of a keyboard key"));
    }
    Ok(format!("{KEY_PREFIX}{name}"))
}

impl CompanionConfig {
    pub fn validate(&self) -> Result<()> {
        key_name(&self.zoom_key)?;
        if !(1.1..=50.0).contains(&self.zoom_factor) {
            return Err(Error::Config("companion zoom_factor must be between 1.1 and 50".into()));
        }
        if !(1.0..=100.0).contains(&self.fullbright_gamma) {
            return Err(Error::Config("companion fullbright_gamma must be between 1 and 100".into()));
        }
        Ok(())
    }

    /// These settings with an instance's own on top.
    pub fn merged(&self, instance: &CompanionOverride) -> CompanionConfig {
        let o = instance.clone();
        CompanionConfig {
            enabled: o.enabled.unwrap_or(self.enabled),
            force: o.force.unwrap_or(self.force),
            zoom: o.zoom.unwrap_or(self.zoom),
            zoom_key: o.zoom_key.unwrap_or_else(|| self.zoom_key.clone()),
            zoom_factor: o.zoom_factor.unwrap_or(self.zoom_factor),
            zoom_wheel: o.zoom_wheel.unwrap_or(self.zoom_wheel),
            hud: o.hud.unwrap_or(self.hud),
            fullbright: o.fullbright.unwrap_or(self.fullbright),
            fullbright_gamma: o.fullbright_gamma.unwrap_or(self.fullbright_gamma),
            self_test: self.self_test,
        }
    }

    /// Every feature and whether it is switched on. The names are the mod's (`Feature.id()`).
    pub fn features(&self) -> [(&'static str, bool); 3] {
        [("zoom", self.zoom), ("hud", self.hud), ("fullbright", self.fullbright)]
    }

    fn active(&self) -> Vec<&'static str> {
        self.features().into_iter().filter_map(|(name, on)| on.then_some(name)).collect()
    }

    /// The file the mod reads its settings from: `key=value` lines. `channel` is the
    /// socket's path inside the sandbox, if there is one.
    pub fn settings(&self, channel: Option<&str>) -> Result<String> {
        self.validate()?;
        let mut text = String::from(
            "# Written by bastion-mc for this launch, from [companion] in its configuration.\n\
             # Read-only: nothing in the game can change it.\n",
        );
        let mut set = |key: &str, value: String| text.push_str(&format!("{key}={value}\n"));
        set("zoom", self.zoom.to_string());
        set("zoom.key", key_name(&self.zoom_key)?);
        set("zoom.factor", self.zoom_factor.to_string());
        set("zoom.wheel", self.zoom_wheel.to_string());
        set("hud", self.hud.to_string());
        set("fullbright", self.fullbright.to_string());
        set("fullbright.gamma", self.fullbright_gamma.to_string());
        if let Some(path) = channel {
            set("channel", path.to_string());
        }
        match self.self_test {
            Some(SelfTest::Title) => set("selftest", "title".into()),
            Some(SelfTest::World) => set("selftest", "world".into()),
            None => {}
        }
        Ok(text)
    }
}

// ---------------------------------------------------------------- whether to inject

/// Why a launch goes without the companion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// `companion.enabled = false`.
    Disabled,
    /// Enabled, but every feature is off: the mod would have nothing to do.
    NothingOn,
    /// A vanilla instance, or a profile without a mod loader.
    NoLoader,
    /// A loader this launcher does not know how to hand an extra mod to.
    OtherLoader(String),
    /// A Minecraft version outside [`TESTED_MINECRAFT`], and `force` is off.
    Untested(String),
    /// The signed profile sets `fabric.addMods` itself; the launcher does not change what a
    /// signature covers.
    ProfileAddsMods,
    /// The JVM cannot load the mod's classes.
    Java(u32),
}

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Skip::Disabled => write!(f, "switched off (companion.enabled = false)"),
            Skip::NothingOn => write!(f, "every feature is switched off"),
            Skip::NoLoader => write!(
                f,
                "this instance has no mod loader (zoom and the other conveniences need a Fabric profile: \
                 `bastion-mc profile new <id> --minecraft <version> --fabric`)"
            ),
            Skip::OtherLoader(kind) => write!(f, "the profile's loader is {kind}; only Fabric is supported"),
            Skip::Untested(version) => write!(
                f,
                "Minecraft {version} is not a version it was tested with ({}); `force = true` under [companion] \
                 adds it anyway, and features whose hooks do not fit switch themselves off",
                TESTED_MINECRAFT.join(", ")
            ),
            Skip::ProfileAddsMods => {
                write!(f, "the signed profile sets fabric.addMods itself, and the launcher does not alter it")
            }
            Skip::Java(major) => write!(f, "it needs Java {MIN_JAVA} or newer; this launch uses Java {major}"),
        }
    }
}

/// Whether this launch gets the companion. `cfg` is the merged configuration.
pub fn decide(cfg: &CompanionConfig, kind: &Kind, minecraft: &str, java_major: u32) -> std::result::Result<(), Skip> {
    if !cfg.enabled {
        return Err(Skip::Disabled);
    }
    let loader = match kind {
        Kind::Vanilla => None,
        Kind::Curated { manifest, .. } => manifest.loader.as_ref(),
    };
    let Some(loader) = loader else { return Err(Skip::NoLoader) };
    if loader.kind != LoaderKind::Fabric {
        return Err(Skip::OtherLoader(format!("{:?}", loader.kind)));
    }
    if cfg.active().is_empty() {
        return Err(Skip::NothingOn);
    }
    if !cfg.force && !TESTED_MINECRAFT.contains(&minecraft) {
        return Err(Skip::Untested(minecraft.to_string()));
    }
    if loader.jvm_args.iter().any(|arg| arg.starts_with(ADD_MODS)) {
        return Err(Skip::ProfileAddsMods);
    }
    if java_major < MIN_JAVA {
        return Err(Skip::Java(java_major));
    }
    Ok(())
}

// ---------------------------------------------------------------- one launch

/// What a launch does about the companion.
pub struct Plan {
    outcome: std::result::Result<Injection, Skip>,
}

struct Injection {
    features: Vec<&'static str>,
    untested: Option<String>,
    settings: String,
    /// The channel's socket until it is being served; why there is none, otherwise.
    endpoint: std::result::Result<channel::Endpoint, String>,
}

fn sandbox_path(name: &str) -> String {
    format!("{SANDBOX_DIR}/{name}")
}

/// Decide, and prepare what an injection needs. Creating the channel's socket is the only
/// side effect, and failing to create it only costs the channel.
pub fn plan(global: &CompanionConfig, instance: &Instance, java_major: u32, paths: &Paths) -> Result<Plan> {
    debug_assert!(SANDBOX_DIR.starts_with(SUPPORT_DIR));
    let cfg = global.merged(&instance.config.companion);
    cfg.validate()?;
    if let Err(skip) = decide(&cfg, &instance.kind, &instance.minecraft, java_major) {
        return Ok(Plan { outcome: Err(skip) });
    }
    let endpoint = channel::Endpoint::create(paths.runtime.as_deref(), &paths.cache).map_err(|e| e.to_string());
    let settings = cfg.settings(endpoint.is_ok().then(|| sandbox_path(SOCKET_NAME)).as_deref())?;
    let untested = (!TESTED_MINECRAFT.contains(&instance.minecraft.as_str())).then(|| instance.minecraft.clone());
    Ok(Plan { outcome: Ok(Injection { features: cfg.active(), untested, settings, endpoint }) })
}

impl Plan {
    pub fn injected(&self) -> bool {
        self.outcome.is_ok()
    }

    /// Add the mod to the sandbox being assembled: two read-only files, one JVM option, and
    /// (with a channel) one socket the game may connect to.
    pub fn attach(&self, mounts: &mut Vec<Mount>, jvm_args: &mut Vec<String>, unix_sockets: &mut Vec<PathBuf>) {
        let Ok(injection) = &self.outcome else { return };
        mounts.push(Mount::RoData { data: JAR.to_vec(), dst: sandbox_path(JAR_NAME).into(), mode: 0o444 });
        mounts.push(Mount::RoData {
            data: injection.settings.clone().into_bytes(),
            dst: sandbox_path(SETTINGS_NAME).into(),
            mode: 0o444,
        });
        jvm_args.push(format!("{ADD_MODS}{}", sandbox_path(JAR_NAME)));
        if let Ok(endpoint) = &injection.endpoint {
            let dst = PathBuf::from(sandbox_path(SOCKET_NAME));
            mounts.push(Mount::RoBind { src: endpoint.path().to_path_buf(), dst: dst.clone() });
            unix_sockets.push(dst);
        }
    }

    /// One line for the launch log: what was injected (its SHA-256, the features that are
    /// on), or why nothing was.
    pub fn describe(&self) -> String {
        match &self.outcome {
            Err(skip) => format!("companion: not added — {skip}"),
            Ok(injection) => format!(
                "companion: bastion-companion {VERSION} added for this session (sha256 {}, {} bytes; read-only, \
                 outside the signed mods directory); features: {}; {}{}. It is a convenience and enforces nothing.",
                util::hex_encode(&util::hash_bytes(util::HashAlg::Sha256, JAR)),
                JAR.len(),
                injection.features.join(", "),
                match &injection.endpoint {
                    Ok(_) => "channel to the launcher: on (what arrives is logged, never acted on)".to_string(),
                    Err(why) => format!("no channel to the launcher ({why})"),
                },
                match &injection.untested {
                    Some(version) => format!("; Minecraft {version} is untested (companion.force)"),
                    None => String::new(),
                }
            ),
        }
    }

    /// Whether this launch serves a channel (the mod is added and its socket exists).
    pub fn has_channel(&self) -> bool {
        matches!(&self.outcome, Ok(Injection { endpoint: Ok(_), .. }))
    }

    /// Remove the channel socket's name on the host. Call once bubblewrap has bound the
    /// socket (the program inside the sandbox exists) and BEFORE the launcher confines
    /// itself: a confined launcher can remove nothing in the runtime directory, and a name
    /// left there lets any process of the user write to the channel. A name that could not
    /// be removed is swept by the next launch.
    pub fn release_name(&self) {
        if let Ok(Injection { endpoint: Ok(endpoint), .. }) = &self.outcome {
            endpoint.unlink();
        }
    }

    /// Start reading the channel. Needs nothing but the socket that already exists, so it
    /// works in a launcher that has confined itself — the launch calls it only then, because
    /// nothing the game writes is read before. Remove the name first ([`Plan::release_name`]).
    pub fn serve(self, logger: channel::Logger) -> Option<channel::Serving> {
        let endpoint = self.outcome.ok()?.endpoint.ok()?;
        match endpoint.serve(logger.clone()) {
            Ok(serving) => Some(serving),
            Err(e) => {
                logger(channel::Voice::Launcher, format!("companion channel: not served ({e})"));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::profile::manifest::tests::sample;

    fn entries() -> Vec<(String, Vec<u8>)> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(JAR)).expect("the embedded jar is a zip archive");
        (0..archive.len())
            .map(|index| {
                let mut entry = archive.by_index(index).unwrap();
                let mut data = Vec::new();
                entry.read_to_end(&mut data).unwrap();
                (entry.name().expect("an entry name").into_owned(), data)
            })
            .collect()
    }

    #[test]
    fn the_embedded_jar_holds_the_mod_and_nothing_else() {
        let entries = entries();
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names[0], "META-INF/MANIFEST.MF", "the manifest comes first");
        let mut sorted = names[1..].to_vec();
        sorted.sort_unstable();
        assert_eq!(names[1..], sorted, "the rest is in name order");
        for wanted in [
            "META-INF/LICENSE",
            "fabric.mod.json",
            "bastion-companion.mixins.json",
            "bastion/companion/CompanionMod.class",
            "bastion/companion/Hooks.class",
            "bastion/companion/feature/Zoom.class",
            "bastion/companion/feature/HudReadout.class",
            "bastion/companion/feature/Fullbright.class",
            "bastion/companion/mixin/CameraMixin.class",
        ] {
            assert!(names.contains(&wanted), "{wanted} is missing from {names:?}");
        }
        // The compile-only stand-ins and anything of the game must not be shipped.
        for name in &names {
            assert!(
                name.starts_with("bastion/companion/") || name.starts_with("META-INF/") || !name.contains('/'),
                "{name} does not belong in the mod"
            );
        }
        for (name, data) in &entries {
            if name.ends_with(".class") {
                assert_eq!(data[..4], [0xCA, 0xFE, 0xBA, 0xBE], "{name}");
                let major = u32::from(u16::from_be_bytes([data[6], data[7]]));
                assert_eq!(major, 44 + MIN_JAVA, "{name} has class-file version {major}");
                let text = String::from_utf8_lossy(data);
                for debug in ["SourceFile", "LineNumberTable", "LocalVariableTable"] {
                    assert!(!text.contains(debug), "{name} carries debug information ({debug})");
                }
            }
        }
        assert_eq!(MIN_JAVA, jar::CLASS_RELEASE);
    }

    /// What a class file refers to, from its constant pool: every text entry, and every
    /// member reference as (owning class, member name).
    fn references(class: &[u8]) -> (Vec<String>, Vec<(String, String)>) {
        let u16_at = |at: usize| usize::from(u16::from_be_bytes([class[at], class[at + 1]]));
        let (count, mut at, mut index) = (u16_at(8), 10, 1);
        let mut text = vec![None; count];
        let mut pairs = vec![None; count];
        let mut members = Vec::new();
        while index < count {
            let (size, slots) = match class[at] {
                1 => {
                    let length = u16_at(at + 1);
                    text[index] = Some(String::from_utf8_lossy(&class[at + 3..at + 3 + length]).into_owned());
                    (3 + length, 1)
                }
                // Class: the index of its name.
                7 => {
                    pairs[index] = Some((u16_at(at + 1), 0));
                    (3, 1)
                }
                8 | 16 | 19 | 20 => (3, 1),
                15 => (4, 1),
                // Field, method, interface method: class, then name-and-type.
                9..=11 => {
                    members.push((u16_at(at + 1), u16_at(at + 3)));
                    (5, 1)
                }
                // Name and type.
                12 => {
                    pairs[index] = Some((u16_at(at + 1), u16_at(at + 3)));
                    (5, 1)
                }
                3 | 4 | 17 | 18 => (5, 1),
                5 | 6 => (9, 2),
                tag => panic!("constant pool tag {tag}"),
            };
            at += size;
            index += slots;
        }
        let name = |index: usize| text[pairs[index].expect("a class or name-and-type").0].clone().expect("a name");
        let members = members.into_iter().map(|(class, member)| (name(class), name(member))).collect();
        (text.into_iter().flatten().collect(), members)
    }

    /// A tripwire for whoever extends the mod: what its code names is what it can do
    /// without going out of its way. It names no network API beyond the one UNIX-domain
    /// socket to the launcher, starts no process, loads no native code, defines no classes,
    /// changes no system property, and has no way of writing a file. (Reading: its settings
    /// file, with two calls.)
    #[test]
    fn the_mod_names_no_api_for_networking_processes_native_code_or_writing_files() {
        let allowed_net = [
            "java/net/StandardProtocolFamily",
            "java/net/UnixDomainSocketAddress",
            "java/net/ProtocolFamily",
            "java/net/SocketAddress",
        ];
        let forbidden_types = [
            "java/lang/Runtime",
            "java/lang/ProcessBuilder",
            "java/lang/Process",
            "java/lang/foreign/",
            "java/lang/instrument/",
            "java/io/File",
            "java/io/FileOutputStream",
            "java/io/FileWriter",
            "java/io/RandomAccessFile",
            "java/nio/channels/FileChannel",
            "java/nio/file/StandardOpenOption",
            "java/util/prefs/",
            "sun/misc/Unsafe",
            "jdk/internal/",
        ];
        // (owner, member): `*` for any owner.
        let forbidden_members = [
            ("java/lang/System", "load"),
            ("java/lang/System", "loadLibrary"),
            ("java/lang/System", "setProperty"),
            ("java/lang/System", "setProperties"),
            ("java/lang/System", "exit"),
            ("*", "defineClass"),
            ("*", "defineHiddenClass"),
            ("*", "openConnection"),
        ];
        let mut reads_files = Vec::new();
        let mut seen_net = std::collections::BTreeSet::new();
        let entries = entries();
        for (class, data) in entries.iter().filter(|(name, _)| name.ends_with(".class")) {
            let (text, members) = references(data);
            for constant in &text {
                // Types appear bare ("java/net/X") and inside descriptors ("(Ljava/net/X;)V").
                for part in constant.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '$' | '_'))) {
                    let part = part.strip_prefix('L').filter(|rest| rest.contains('/')).unwrap_or(part);
                    if part.starts_with("java/net/") {
                        assert!(allowed_net.contains(&part), "{class} refers to {part}");
                        seen_net.insert(part.to_string());
                    }
                    for forbidden in forbidden_types {
                        let hit =
                            if forbidden.ends_with('/') { part.starts_with(forbidden) } else { part == forbidden };
                        assert!(!hit, "{class} refers to {part}");
                    }
                }
            }
            for (owner, member) in &members {
                for (forbidden_owner, forbidden) in forbidden_members {
                    assert!(
                        !(member == forbidden && (forbidden_owner == "*" || owner == forbidden_owner)),
                        "{class} calls {owner}.{member}"
                    );
                }
                if owner == "java/nio/file/Files" {
                    // The only two things done with a file: its size, and reading it whole.
                    assert!(matches!(member.as_str(), "size" | "readAllBytes"), "{class} calls Files.{member}");
                    reads_files.push(class.as_str());
                }
            }
        }
        reads_files.dedup();
        assert_eq!(reads_files, ["bastion/companion/Config.class"]);
        // The scan sees what is there: the socket to the launcher is in it.
        assert!(seen_net.contains("java/net/UnixDomainSocketAddress"), "{seen_net:?}");
    }

    #[test]
    fn the_mod_describes_itself_to_fabric_loader_with_the_launchers_version() {
        let entries = entries();
        let text = |name: &str| {
            String::from_utf8(entries.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name}")).1.clone())
                .unwrap()
        };
        let about: serde_json::Value = serde_json::from_str(&text("fabric.mod.json")).unwrap();
        assert_eq!(about["id"], "bastion-companion");
        assert_eq!(about["version"], VERSION);
        assert_eq!(about["environment"], "client");
        assert_eq!(about["entrypoints"]["client"][0], "bastion.companion.CompanionMod");
        assert!(about.get("depends").is_none(), "a version requirement here would stop the game where `force` is set");
        let mixins: serde_json::Value = serde_json::from_str(&text(about["mixins"][0].as_str().unwrap())).unwrap();
        // Not required: a mixin that does not fit is skipped by Mixin instead of ending the game.
        assert_eq!(mixins["required"], false);
        assert_eq!(mixins["injectors"]["defaultRequire"], 0);
        assert_eq!(mixins["compatibilityLevel"], format!("JAVA_{MIN_JAVA}"));
        let package = mixins["package"].as_str().unwrap().replace('.', "/");
        for class in mixins["client"].as_array().unwrap() {
            let name = format!("{package}/{}.class", class.as_str().unwrap());
            assert!(entries.iter().any(|(n, _)| *n == name), "{name} is listed but not in the jar");
        }
        let listed = mixins["client"].as_array().unwrap().len();
        let present = entries.iter().filter(|(n, _)| n.starts_with(&format!("{package}/"))).count();
        assert_eq!(listed, present, "every mixin class in the jar is listed");
    }

    #[test]
    fn building_the_jar_again_gives_the_same_bytes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let javac = std::env::var_os("JAVAC").unwrap_or_else(|| "javac".into());
        let scratch = std::env::temp_dir()
            .join(format!("bastion-companion-jar-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        // Two builds in two different directories, neither of them where build.rs worked.
        let first = jar::build(root, &javac, VERSION, &scratch.join("one")).expect("first build");
        let second = jar::build(root, &javac, VERSION, &scratch.join("a/different/place")).expect("second build");
        if !first.warnings.trim().is_empty() {
            eprintln!("javac -Xlint:all on the companion's sources:\n{}", first.warnings);
        }
        assert!(first.jar == second.jar, "two builds from the same sources differ");
        assert!(first.jar == JAR, "the embedded jar is not what the sources build to");
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn the_archive_writer_is_a_function_of_its_entries() {
        let entries = vec![
            ("b/Two.class".to_string(), vec![1u8, 2, 3]),
            ("META-INF/MANIFEST.MF".to_string(), b"Manifest-Version: 1.0\r\n\r\n".to_vec()),
            ("a.json".to_string(), b"{}".to_vec()),
            ("b/One$Inner.class".to_string(), Vec::new()),
        ];
        let mut reversed = entries.clone();
        reversed.reverse();
        let bytes = jar::archive(entries).unwrap();
        assert_eq!(bytes, jar::archive(reversed).unwrap(), "the order the entries were given in does not matter");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let names: Vec<String> =
            (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().unwrap().into_owned()).collect();
        assert_eq!(names, ["META-INF/MANIFEST.MF", "a.json", "b/One$Inner.class", "b/Two.class"]);
        let mut two = Vec::new();
        archive.by_name("b/Two.class").unwrap().read_to_end(&mut two).unwrap();
        assert_eq!(two, [1, 2, 3], "contents and checksums read back");
        for bad in ["", "/abs", "a/../b", "a//b", "sp ace", "dir/"] {
            assert!(jar::archive(vec![(bad.to_string(), Vec::new())]).is_err(), "{bad:?}");
        }
        assert!(jar::archive(vec![("x".into(), Vec::new()), ("x".into(), Vec::new())]).is_err(), "a name twice");
    }

    #[test]
    fn the_settings_file_says_what_the_configuration_says() {
        let cfg = CompanionConfig::default();
        assert_eq!(
            cfg.settings(Some("/bastion/companion/channel")).unwrap(),
            "# Written by bastion-mc for this launch, from [companion] in its configuration.\n\
             # Read-only: nothing in the game can change it.\n\
             zoom=true\nzoom.key=key.keyboard.c\nzoom.factor=4\nzoom.wheel=true\nhud=false\nfullbright=false\n\
             fullbright.gamma=15\nchannel=/bastion/companion/channel\n"
        );
        let custom = CompanionConfig {
            zoom_key: "left.alt".into(),
            zoom_factor: 2.5,
            zoom_wheel: false,
            hud: true,
            fullbright: true,
            fullbright_gamma: 8.0,
            self_test: Some(SelfTest::World),
            ..CompanionConfig::default()
        };
        let text = custom.settings(None).unwrap();
        for line in [
            "zoom.key=key.keyboard.left.alt",
            "zoom.factor=2.5",
            "zoom.wheel=false",
            "hud=true",
            "fullbright=true",
            "fullbright.gamma=8",
            "selftest=world",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} is missing from:\n{text}");
        }
        assert!(!text.contains("channel="), "no channel, no path");
        assert!(!cfg.settings(None).unwrap().contains("selftest"), "the self-test is never on by configuration");
        // Every line is a comment or key=value with a value that cannot break out of its line.
        for line in text.lines() {
            assert!(line.starts_with('#') || line.split_once('=').is_some_and(|(k, v)| !k.is_empty() && !v.is_empty()));
            assert!(line.is_ascii() && !line.chars().any(char::is_control));
        }
    }

    #[test]
    fn settings_are_validated_and_keys_are_minecrafts_names() {
        assert_eq!(key_name("c").unwrap(), "key.keyboard.c");
        assert_eq!(key_name("key.keyboard.c").unwrap(), "key.keyboard.c");
        assert_eq!(key_name("left.alt").unwrap(), "key.keyboard.left.alt");
        assert_eq!(key_name("key.keyboard.f6").unwrap(), "key.keyboard.f6");
        for bad in [
            "",
            "C",
            "key.keyboard.",
            "key.keyboard..c",
            "c.",
            ".c",
            "left alt",
            "c\nselftest=world",
            "c=true",
            "key.mouse.4",
            "unknown",
            "key.keyboard.unknown",
            &"k".repeat(41),
            "ö",
        ] {
            assert!(matches!(key_name(bad), Err(Error::Config(_))), "{bad:?}");
        }
        let mut cfg = CompanionConfig::default();
        cfg.validate().unwrap();
        for factor in [1.0, 0.0, -4.0, 50.5, f64::NAN, f64::INFINITY] {
            cfg.zoom_factor = factor;
            assert!(cfg.validate().is_err(), "zoom_factor {factor}");
            assert!(cfg.settings(None).is_err());
        }
        cfg.zoom_factor = 4.0;
        for gamma in [0.5, 100.5, f64::NAN] {
            cfg.fullbright_gamma = gamma;
            assert!(cfg.validate().is_err(), "fullbright_gamma {gamma}");
        }
    }

    #[test]
    fn an_instance_overrides_single_keys() {
        let global = CompanionConfig { hud: true, ..CompanionConfig::default() };
        assert_eq!(global.merged(&CompanionOverride::default()), global);
        let instance: CompanionOverride =
            toml::from_str("zoom = false\nzoom_key = \"v\"\nfullbright = true\nfullbright_gamma = 3.0\n").unwrap();
        let merged = global.merged(&instance);
        assert_eq!(
            merged,
            CompanionConfig {
                zoom: false,
                zoom_key: "v".into(),
                hud: true,
                fullbright: true,
                fullbright_gamma: 3.0,
                ..CompanionConfig::default()
            }
        );
        assert!(toml::from_str::<CompanionOverride>("zom = true\n").is_err(), "unknown keys are errors");
        assert!(toml::from_str::<CompanionOverride>("self_test = \"world\"\n").is_err(), "not a configuration key");
        assert!(toml::from_str::<CompanionConfig>("self_test = \"world\"\n").is_err());
        let off: CompanionOverride = toml::from_str("enabled = false\n").unwrap();
        assert!(!global.merged(&off).enabled);
    }

    fn fabric(minecraft: &str) -> Kind {
        let mut manifest = sample("perf");
        manifest.minecraft.version = minecraft.into();
        Kind::Curated { manifest: Box::new(manifest), signed_by: "me".into() }
    }

    #[test]
    fn only_fabric_profiles_of_tested_versions_get_the_mod() {
        let cfg = CompanionConfig::default();
        assert_eq!(decide(&cfg, &fabric("26.3"), "26.3", 27), Ok(()));
        assert_eq!(decide(&cfg, &Kind::Vanilla, "26.3", 27), Err(Skip::NoLoader));

        // A signed profile without a loader cannot load a mod either.
        let Kind::Curated { manifest: mut bare, .. } = fabric("26.3") else { unreachable!() };
        bare.loader = None;
        bare.mods.clear();
        assert_eq!(
            decide(&cfg, &Kind::Curated { manifest: bare, signed_by: "me".into() }, "26.3", 27),
            Err(Skip::NoLoader)
        );

        let Kind::Curated { manifest: mut quilt, .. } = fabric("26.3") else { unreachable!() };
        quilt.loader.as_mut().unwrap().kind = LoaderKind::Quilt;
        assert_eq!(
            decide(&cfg, &Kind::Curated { manifest: quilt, signed_by: "me".into() }, "26.3", 27),
            Err(Skip::OtherLoader("Quilt".into()))
        );

        for untested in ["26.4", "26.3.1", "26.2", "1.21.4", "26.3 ", ""] {
            assert_eq!(
                decide(&cfg, &fabric(untested), untested, 27),
                Err(Skip::Untested(untested.into())),
                "{untested:?}"
            );
        }
        // The force switch lifts the version list and nothing else.
        let forced = CompanionConfig { force: true, ..cfg.clone() };
        assert_eq!(decide(&forced, &fabric("1.21.4"), "1.21.4", 21), Ok(()));
        assert_eq!(decide(&forced, &Kind::Vanilla, "1.21.4", 21), Err(Skip::NoLoader));
        assert_eq!(decide(&forced, &fabric("1.16.5"), "1.16.5", 8), Err(Skip::Java(8)));
        assert_eq!(decide(&forced, &fabric("1.20.1"), "1.20.1", 17), Ok(()));

        let off = CompanionConfig { enabled: false, force: true, ..cfg.clone() };
        assert_eq!(decide(&off, &fabric("26.3"), "26.3", 27), Err(Skip::Disabled));
        let idle = CompanionConfig { zoom: false, ..cfg.clone() };
        assert_eq!(decide(&idle, &fabric("26.3"), "26.3", 27), Err(Skip::NothingOn));
        let hud_only = CompanionConfig { zoom: false, hud: true, ..cfg.clone() };
        assert_eq!(decide(&hud_only, &fabric("26.3"), "26.3", 27), Ok(()));
    }

    #[test]
    fn a_profile_that_adds_mods_itself_is_left_exactly_as_signed() {
        let cfg = CompanionConfig::default();
        let Kind::Curated { manifest: mut own, .. } = fabric("26.3") else { unreachable!() };
        own.loader.as_mut().unwrap().jvm_args.push("-Dfabric.addMods=/game/config/extra.jar".into());
        assert_eq!(
            decide(&cfg, &Kind::Curated { manifest: own, signed_by: "me".into() }, "26.3", 27),
            Err(Skip::ProfileAddsMods)
        );
        // Another property that merely starts alike is not it.
        let Kind::Curated { manifest: mut other, .. } = fabric("26.3") else { unreachable!() };
        other.loader.as_mut().unwrap().jvm_args.push("-Dfabric.addModsExtra=1".into());
        assert_eq!(decide(&cfg, &Kind::Curated { manifest: other, signed_by: "me".into() }, "26.3", 27), Ok(()));
    }

    fn instance(kind: Kind, minecraft: &str, companion: CompanionOverride) -> Instance {
        let config = crate::config::InstanceConfig { companion, ..Default::default() };
        Instance { id: "perf".into(), dir: "/nonexistent".into(), config, minecraft: minecraft.into(), kind }
    }

    fn scratch_paths(tag: &str) -> (PathBuf, Paths) {
        let root = std::env::temp_dir()
            .join(format!("bmc-cmp-{tag}-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("c"),
            state: root.join("state"),
            runtime: Some(root.join("rt")),
        };
        util::create_private_dir_all(&root.join("rt")).unwrap();
        (root, paths)
    }

    #[test]
    fn an_injection_adds_two_read_only_files_one_option_and_one_socket() {
        let (root, paths) = scratch_paths("inject");
        let plan = plan(&CompanionConfig::default(), &instance(fabric("26.3"), "26.3", Default::default()), 27, &paths)
            .unwrap();
        assert!(plan.injected());
        let (mut mounts, mut jvm_args, mut sockets) = (Vec::new(), vec!["-Dlog4j=x".to_string()], Vec::new());
        plan.attach(&mut mounts, &mut jvm_args, &mut sockets);

        assert_eq!(jvm_args, ["-Dlog4j=x", "-Dfabric.addMods=/bastion/companion/bastion-companion.jar"]);
        assert_eq!(sockets, [PathBuf::from("/bastion/companion/channel")]);
        assert_eq!(mounts.len(), 3);
        let mut socket_on_host = None;
        for mount in &mounts {
            match mount {
                // Read-only data that exists only in the sandbox: nothing on the host is exposed.
                Mount::RoData { data, dst, mode } => {
                    assert_eq!(*mode, 0o444);
                    match dst.to_str().unwrap() {
                        "/bastion/companion/bastion-companion.jar" => assert!(data == JAR),
                        "/bastion/companion/config.properties" => {
                            let text = String::from_utf8(data.clone()).unwrap();
                            assert!(
                                text.contains("zoom=true\n") && text.contains("channel=/bastion/companion/channel\n")
                            );
                        }
                        other => panic!("unexpected file {other}"),
                    }
                }
                Mount::RoBind { src, dst } => {
                    assert_eq!(dst, &PathBuf::from("/bastion/companion/channel"));
                    assert!(std::fs::metadata(src).unwrap().file_type().is_socket_like(), "{}", src.display());
                    assert!(src.starts_with(root.join("rt").join("bastion-mc")), "{}", src.display());
                    socket_on_host = Some(src.clone());
                }
                _ => panic!("the companion asks for a kind of mount it must not need"),
            }
        }
        // Nothing of it lies in the game directory, least of all in mods/.
        for mount in &mounts {
            let (Mount::RoData { dst, .. } | Mount::RoBind { dst, .. }) = mount else { unreachable!() };
            assert!(dst.starts_with(SANDBOX_DIR) && !dst.starts_with(crate::sandbox::GAME_DIR));
        }
        let line = plan.describe();
        let digest = util::hex_encode(&util::hash_bytes(util::HashAlg::Sha256, JAR));
        assert!(line.contains(&format!("sha256 {digest}")), "{line}");
        assert!(line.contains("features: zoom;") && line.contains("enforces nothing"), "{line}");
        assert!(!line.contains("untested"), "{line}");

        // Dropping the plan (a launch that failed before the sandbox started) leaves no socket behind.
        let socket_on_host = socket_on_host.expect("a socket");
        drop(plan);
        assert!(!socket_on_host.exists() && !socket_on_host.parent().unwrap().exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    trait SocketLike {
        fn is_socket_like(&self) -> bool;
    }
    impl SocketLike for std::fs::FileType {
        fn is_socket_like(&self) -> bool {
            std::os::unix::fs::FileTypeExt::is_socket(self)
        }
    }

    #[test]
    fn a_launch_without_the_mod_changes_nothing_and_says_why() {
        let (root, paths) = scratch_paths("skip");
        let global = CompanionConfig::default();
        for (instance, expect) in [
            (instance(Kind::Vanilla, "26.3", Default::default()), "no mod loader"),
            (
                instance(fabric("26.4"), "26.4", Default::default()),
                "Minecraft 26.4 is not a version it was tested with (26.3)",
            ),
            (
                instance(fabric("26.3"), "26.3", CompanionOverride { enabled: Some(false), ..Default::default() }),
                "switched off",
            ),
        ] {
            let plan = plan(&global, &instance, 27, &paths).unwrap();
            assert!(!plan.injected());
            let (mut mounts, mut jvm_args, mut sockets) = (Vec::new(), Vec::new(), Vec::new());
            plan.attach(&mut mounts, &mut jvm_args, &mut sockets);
            assert!(mounts.is_empty() && jvm_args.is_empty() && sockets.is_empty());
            let line = plan.describe();
            assert!(line.starts_with("companion: not added — ") && line.contains(expect), "{line}");
        }
        assert!(!root.join("rt").join("bastion-mc").exists(), "no socket is created for a launch without the mod");

        // Forced onto an untested version: injected, and the log says that it is untested.
        let forced = CompanionOverride { force: Some(true), ..Default::default() };
        let plan = plan(&global, &instance(fabric("26.4"), "26.4", forced), 27, &paths).unwrap();
        assert!(plan.injected() && plan.describe().contains("Minecraft 26.4 is untested (companion.force)"));

        // A bad setting in the instance's file stops the launch instead of being ignored.
        let bad = CompanionOverride { zoom_key: Some("key.mouse.4".into()), ..Default::default() };
        assert!(matches!(plan_err(&global, &instance(fabric("26.3"), "26.3", bad), &paths), Error::Config(_)));
        drop(plan);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn plan_err(global: &CompanionConfig, instance: &Instance, paths: &Paths) -> Error {
        match plan(global, instance, 27, paths) {
            Err(e) => e,
            Ok(_) => panic!("expected a refusal"),
        }
    }

    #[test]
    fn without_a_place_for_the_socket_the_mod_is_still_added_just_without_a_channel() {
        let (root, mut paths) = scratch_paths("nochannel");
        paths.runtime = None;
        paths.cache = root.join("x".repeat(110));
        let plan = plan(&CompanionConfig::default(), &instance(fabric("26.3"), "26.3", Default::default()), 27, &paths)
            .unwrap();
        let (mut mounts, mut jvm_args, mut sockets) = (Vec::new(), Vec::new(), Vec::new());
        plan.attach(&mut mounts, &mut jvm_args, &mut sockets);
        assert_eq!((mounts.len(), jvm_args.len(), sockets.len()), (2, 1, 0));
        assert!(plan.describe().contains("no channel to the launcher ("), "{}", plan.describe());
        let Mount::RoData { data, .. } = &mounts[1] else { panic!("settings") };
        assert!(!String::from_utf8_lossy(data).contains("channel="));
        std::fs::remove_dir_all(root).unwrap();
    }
}
