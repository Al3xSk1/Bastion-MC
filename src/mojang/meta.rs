//! Data model for `version_manifest_v2.json`, version JSON files and asset indexes, plus the
//! rule evaluation that decides what applies to Linux/x86_64.

use std::{collections::BTreeMap, path::Path};

use serde::Deserialize;

use crate::{
    config::validate_version_id,
    error::{Error, Result},
    paths::validate_relative,
    util::Expected,
};

pub const VERSION_MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const ASSET_OBJECT_BASE: &str = "https://resources.download.minecraft.net";

// ---------------------------------------------------------------- manifest

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub latest: Latest,
    pub versions: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub sha1: String,
    #[serde(rename = "releaseTime", default)]
    pub release_time: String,
}

impl VersionManifest {
    pub fn find(&self, id: &str) -> Result<&ManifestEntry> {
        self.versions
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| Error::Usage(format!("Minecraft version {id:?} is not in Mojang's version manifest")))
    }
}

// ---------------------------------------------------------------- version json

#[derive(Debug, Clone, Deserialize)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    #[serde(rename = "minecraftArguments")]
    pub legacy_arguments: Option<String>,
    pub arguments: Option<Arguments>,
    #[serde(rename = "assetIndex")]
    pub asset_index: AssetIndexRef,
    pub downloads: Downloads,
    pub libraries: Vec<Library>,
    pub logging: Option<Logging>,
    #[serde(rename = "javaVersion")]
    pub java_version: Option<JavaVersion>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional { rules: Vec<Rule>, value: OneOrMany },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: Action,
    pub os: Option<OsRule>,
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Downloads {
    pub client: Download,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Download {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    pub natives: Option<BTreeMap<String, String>>,
    pub extract: Option<Extract>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryDownloads {
    pub artifact: Option<LibraryArtifact>,
    pub classifiers: Option<BTreeMap<String, LibraryArtifact>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryArtifact {
    pub path: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Logging {
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingClient {
    pub argument: String,
    pub file: LoggingFile,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaVersion {
    #[serde(rename = "majorVersion")]
    pub major_version: u32,
}

// ---------------------------------------------------------------- rules

/// Launcher "features" referenced by argument rules. Anything not set is `false`.
#[derive(Debug, Clone, Default)]
pub struct Features(pub BTreeMap<String, bool>);

impl Features {
    pub fn set(&mut self, name: &str) {
        self.0.insert(name.to_string(), true);
    }
    fn get(&self, name: &str) -> bool {
        self.0.get(name).copied().unwrap_or(false)
    }
}

impl Rule {
    fn matches(&self, features: &Features) -> bool {
        if let Some(os) = &self.os {
            if os.name.as_deref().is_some_and(|n| n != "linux") {
                return false;
            }
            if os.arch.as_deref().is_some_and(|a| !matches!(a, "x86_64" | "x64" | "amd64")) {
                return false;
            }
            // `version` is a regular expression over the OS version and has only ever been
            // used for Windows and macOS. No regex engine is linked for this; a rule that
            // constrains the Linux kernel version is treated as not matching.
            if os.version.is_some() {
                return false;
            }
        }
        if let Some(wanted) = &self.features
            && wanted.iter().any(|(name, value)| features.get(name) != *value)
        {
            return false;
        }
        true
    }
}

/// Mojang semantics: no rules → allowed; otherwise the last matching rule decides and the
/// default is "disallow".
pub fn rules_allow(rules: &[Rule], features: &Features) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        if rule.matches(features) {
            allowed = rule.action == Action::Allow;
        }
    }
    allowed
}

// ---------------------------------------------------------------- resolved artifacts

/// One file with everything needed to fetch and verify it.
#[derive(Debug, Clone)]
pub struct Artifact {
    /// Human-readable identity for messages.
    pub name: String,
    /// Path relative to its store root (library store, version store, …).
    pub rel_path: String,
    pub url: String,
    pub expect: Expected,
    pub size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct NativeArtifact {
    pub artifact: Artifact,
    /// Entry-name prefixes that must not be extracted (`META-INF/`).
    pub exclude: Vec<String>,
}

fn library_artifact(name: &str, a: &LibraryArtifact) -> Result<Artifact> {
    validate_relative(Path::new(&a.path))?;
    Ok(Artifact {
        name: name.to_string(),
        rel_path: a.path.clone(),
        url: a.url.clone(),
        expect: Expected::sha1(&a.sha1)?,
        size: Some(a.size),
    })
}

impl VersionJson {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let v: VersionJson = serde_json::from_slice(bytes).map_err(|e| Error::Format(format!("version JSON: {e}")))?;
        validate_version_id(&v.id)?;
        validate_class_name(&v.main_class)?;
        Ok(v)
    }

    /// Versions older than the `javaVersion` field ran on Java 8.
    pub fn java_major(&self) -> u32 {
        self.java_version.as_ref().map(|j| j.major_version).unwrap_or(8)
    }

    /// Class-path libraries for Linux/x86_64, in declaration order, de-duplicated by path.
    pub fn classpath_libraries(&self) -> Result<Vec<Artifact>> {
        let features = Features::default();
        let mut out: Vec<Artifact> = Vec::new();
        for lib in &self.libraries {
            if !rules_allow(&lib.rules, &features) {
                continue;
            }
            let Some(downloads) = &lib.downloads else {
                // Mojang always publishes a digest. A library without one cannot be verified.
                return Err(Error::Integrity(format!("library {} has no published digest", lib.name)));
            };
            if let Some(a) = &downloads.artifact {
                let art = library_artifact(&lib.name, a)?;
                if !out.iter().any(|o| o.rel_path == art.rel_path) {
                    out.push(art);
                }
            }
        }
        Ok(out)
    }

    /// Pre-1.19 style natives: classifier jars that must be unpacked before launch.
    pub fn native_archives(&self) -> Result<Vec<NativeArtifact>> {
        let features = Features::default();
        let mut out: Vec<NativeArtifact> = Vec::new();
        for lib in &self.libraries {
            if !rules_allow(&lib.rules, &features) {
                continue;
            }
            let Some(classifier) = lib.natives.as_ref().and_then(|n| n.get("linux")) else { continue };
            let classifier = classifier.replace("${arch}", "64");
            let artifact = lib
                .downloads
                .as_ref()
                .and_then(|d| d.classifiers.as_ref())
                .and_then(|c| c.get(&classifier))
                .ok_or_else(|| {
                    Error::Integrity(format!("library {} declares Linux natives but publishes no digest", lib.name))
                })?;
            let art = library_artifact(&format!("{}:{classifier}", lib.name), artifact)?;
            if !out.iter().any(|o| o.artifact.rel_path == art.rel_path) {
                out.push(NativeArtifact { artifact: art, exclude: lib.extract.clone().unwrap_or_default().exclude });
            }
        }
        Ok(out)
    }

    pub fn client_artifact(&self) -> Result<Artifact> {
        Ok(Artifact {
            name: format!("minecraft client {}", self.id),
            rel_path: format!("{0}/{0}.jar", self.id),
            url: self.downloads.client.url.clone(),
            expect: Expected::sha1(&self.downloads.client.sha1)?,
            size: Some(self.downloads.client.size),
        })
    }

    pub fn asset_index_artifact(&self) -> Result<Artifact> {
        validate_version_id(&self.asset_index.id)?;
        Ok(Artifact {
            name: format!("asset index {}", self.asset_index.id),
            rel_path: format!("indexes/{}.json", self.asset_index.id),
            url: self.asset_index.url.clone(),
            expect: Expected::sha1(&self.asset_index.sha1)?,
            size: Some(self.asset_index.size),
        })
    }

    /// The Log4j configuration Mojang ships for this version. For 1.7–1.18 this file is the
    /// actual Log4Shell fix (`%msg{nolookups}`), so it is mandatory when declared.
    pub fn logging_artifact(&self) -> Result<Option<(String, Artifact)>> {
        let Some(client) = self.logging.as_ref().and_then(|l| l.client.as_ref()) else { return Ok(None) };
        crate::paths::validate_file_name(&client.file.id)?;
        Ok(Some((
            client.argument.clone(),
            Artifact {
                name: format!("logging configuration {}", client.file.id),
                rel_path: format!("log_configs/{}", client.file.id),
                url: client.file.url.clone(),
                expect: Expected::sha1(&client.file.sha1)?,
                size: Some(client.file.size),
            },
        )))
    }

    /// Unexpanded JVM argument templates that apply here. Legacy versions predate the field;
    /// they get what the official launcher used for them.
    pub fn jvm_arg_templates(&self, features: &Features) -> Vec<String> {
        match &self.arguments {
            Some(a) => flatten(&a.jvm, features),
            None => vec![
                "-Djava.library.path=${natives_directory}".into(),
                "-Dminecraft.launcher.brand=${launcher_name}".into(),
                "-Dminecraft.launcher.version=${launcher_version}".into(),
                "-cp".into(),
                "${classpath}".into(),
            ],
        }
    }

    pub fn game_arg_templates(&self, features: &Features) -> Vec<String> {
        match (&self.arguments, &self.legacy_arguments) {
            (Some(a), _) => flatten(&a.game, features),
            (None, Some(legacy)) => legacy.split_ascii_whitespace().map(str::to_string).collect(),
            (None, None) => Vec::new(),
        }
    }

    /// Does this version understand `--quickPlayMultiplayer`?
    pub fn has_feature_rule(&self, feature: &str) -> bool {
        self.arguments.as_ref().is_some_and(|a| {
            a.game.iter().any(|arg| match arg {
                Argument::Conditional { rules, .. } => {
                    rules.iter().any(|r| r.features.as_ref().is_some_and(|f| f.contains_key(feature)))
                }
                Argument::Plain(_) => false,
            })
        })
    }
}

fn flatten(args: &[Argument], features: &Features) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        match arg {
            Argument::Plain(s) => out.push(s.clone()),
            Argument::Conditional { rules, value } => {
                if rules_allow(rules, features) {
                    match value {
                        OneOrMany::One(s) => out.push(s.clone()),
                        OneOrMany::Many(v) => out.extend(v.iter().cloned()),
                    }
                }
            }
        }
    }
    out
}

/// A fully-qualified Java class name and nothing else: this string ends up as the entry point.
pub fn validate_class_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 255
        && name.split('.').all(|seg| {
            let mut chars = seg.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        });
    if ok { Ok(()) } else { Err(Error::Format(format!("invalid Java class name {name:?}"))) }
}

// ---------------------------------------------------------------- asset index

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    pub objects: BTreeMap<String, AssetObject>,
    /// 1.6–1.7.2 era: assets must also exist under their logical names.
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
    /// Pre-1.6: assets must appear as `<gameDir>/resources/<name>`.
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

impl AssetIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let idx: AssetIndex = serde_json::from_slice(bytes).map_err(|e| Error::Format(format!("asset index: {e}")))?;
        for (name, obj) in &idx.objects {
            validate_relative(Path::new(name))?;
            Expected::sha1(&obj.hash)?;
        }
        Ok(idx)
    }

    pub fn needs_logical_tree(&self) -> bool {
        self.is_virtual || self.map_to_resources
    }

    /// Unique objects (several logical names can share one blob).
    pub fn unique_objects(&self) -> Result<Vec<Artifact>> {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for obj in self.objects.values() {
            let expect = Expected::sha1(&obj.hash)?;
            let hash = expect.hex().to_string();
            if seen.insert(hash.clone()) {
                let rel = format!("objects/{}/{}", &hash[..2], hash);
                out.push(Artifact {
                    name: format!("asset {hash}"),
                    url: format!("{ASSET_OBJECT_BASE}/{}/{}", &hash[..2], hash),
                    rel_path: rel,
                    expect,
                    size: Some(obj.size),
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(json: &str) -> Vec<Rule> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn rule_semantics() {
        let f = Features::default();
        assert!(rules_allow(&[], &f));
        assert!(rules_allow(&rule(r#"[{"action":"allow","os":{"name":"linux"}}]"#), &f));
        assert!(!rules_allow(&rule(r#"[{"action":"allow","os":{"name":"osx"}}]"#), &f));
        assert!(!rules_allow(&rule(r#"[{"action":"allow","os":{"name":"windows"}}]"#), &f));
        // LWJGL 2 pattern: allow everywhere except osx
        assert!(rules_allow(&rule(r#"[{"action":"allow"},{"action":"disallow","os":{"name":"osx"}}]"#), &f));
        // 32-bit only flag must not apply
        assert!(!rules_allow(&rule(r#"[{"action":"allow","os":{"arch":"x86"}}]"#), &f));
        // OS-version regex rules never match here
        assert!(!rules_allow(&rule(r#"[{"action":"allow","os":{"name":"linux","version":"^5\\."}}]"#), &f));
        assert!(rules_allow(
            &rule(r#"[{"action":"allow"},{"action":"disallow","os":{"name":"osx","version":"^10\\.5\\.\\d$"}}]"#),
            &f
        ));
    }

    #[test]
    fn feature_rules() {
        let demo = rule(r#"[{"action":"allow","features":{"is_demo_user":true}}]"#);
        let mut f = Features::default();
        assert!(!rules_allow(&demo, &f));
        f.set("is_demo_user");
        assert!(rules_allow(&demo, &f));
    }

    /// Trimmed from the real 1.12.2 version file: LWJGL 2 with per-OS variants, classifier
    /// natives with an extraction exclude list, and a single-string argument line.
    const LEGACY: &str = r#"{
      "id": "1.12.2", "type": "release", "mainClass": "net.minecraft.client.main.Main",
      "minecraftArguments": "--username ${auth_player_name} --version ${version_name} --gameDir ${game_directory} --assetsDir ${assets_root} --assetIndex ${assets_index_name} --uuid ${auth_uuid} --accessToken ${auth_access_token} --userType ${user_type} --versionType ${version_type}",
      "assetIndex": {"id": "1.12", "sha1": "a21e1ded1a24ea1548dd8db0cf30b6acb02655a9", "size": 143136, "totalSize": 1, "url": "https://piston-meta.mojang.com/v1/packages/a21e1ded1a24ea1548dd8db0cf30b6acb02655a9/1.12.json"},
      "assets": "1.12",
      "downloads": {"client": {"sha1": "0f275bc1547d01fa5f56ba34bdc87d981ee12daf", "size": 10180113, "url": "https://piston-data.mojang.com/v1/objects/0f275bc1547d01fa5f56ba34bdc87d981ee12daf/client.jar"}},
      "logging": {"client": {"argument": "-Dlog4j.configurationFile=${path}", "file": {"id": "client-1.12.xml", "sha1": "bd65e7d2e3c237be76cfbef4c2405033d7f91521", "size": 888, "url": "https://piston-data.mojang.com/v1/objects/bd65e7d2e3c237be76cfbef4c2405033d7f91521/client-1.12.xml"}, "type": "log4j2-xml"}},
      "javaVersion": {"component": "jre-legacy", "majorVersion": 8},
      "libraries": [
        {"name": "com.mojang:patchy:1.3.9", "downloads": {"artifact": {"path": "com/mojang/patchy/1.3.9/patchy-1.3.9.jar", "sha1": "eb8bb7b66fa0e2152b1b40b3856e82f7619439ee", "size": 23581, "url": "https://libraries.minecraft.net/com/mojang/patchy/1.3.9/patchy-1.3.9.jar"}}},
        {"name": "org.lwjgl.lwjgl:lwjgl:2.9.4-nightly-20150209", "rules": [{"action": "allow"}, {"action": "disallow", "os": {"name": "osx"}}],
         "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/lwjgl/2.9.4-nightly-20150209/lwjgl-2.9.4-nightly-20150209.jar", "sha1": "697517568c68e78ae0b4544145af031c81082dfe", "size": 1047168, "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/lwjgl/2.9.4-nightly-20150209/lwjgl-2.9.4-nightly-20150209.jar"}}},
        {"name": "org.lwjgl.lwjgl:lwjgl:2.9.2-nightly-20140822", "rules": [{"action": "allow", "os": {"name": "osx"}}],
         "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/lwjgl/2.9.2-nightly-20140822/lwjgl-2.9.2-nightly-20140822.jar", "sha1": "7707204c9ffa5d91662de95f0a224e2f721b22af", "size": 1045632, "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/lwjgl/2.9.2-nightly-20140822/lwjgl-2.9.2-nightly-20140822.jar"}}},
        {"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209", "extract": {"exclude": ["META-INF/"]},
         "natives": {"linux": "natives-linux", "osx": "natives-osx", "windows": "natives-windows"},
         "rules": [{"action": "allow"}, {"action": "disallow", "os": {"name": "osx"}}],
         "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209.jar", "sha1": "b04f3ee8f5e43fa3b162981b50bb72fe1acabb33", "size": 22, "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209.jar"},
                       "classifiers": {"natives-linux": {"path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-linux.jar", "sha1": "931074f46c795d2f7b30ed6395df5715cfd7675b", "size": 578680, "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-linux.jar"},
                                       "natives-windows": {"path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-windows.jar", "sha1": "b84d5102b9dbfabfeb5e43c7e2828d98a7fc80e0", "size": 613748, "url": "https://libraries.minecraft.net/x.jar"}}}},
        {"name": "net.java.jinput:jinput-platform:2.0.5", "extract": {"exclude": ["META-INF/"]},
         "natives": {"linux": "natives-linux", "osx": "natives-osx", "windows": "natives-windows"},
         "downloads": {"classifiers": {"natives-linux": {"path": "net/java/jinput/jinput-platform/2.0.5/jinput-platform-2.0.5-natives-linux.jar", "sha1": "7ff832a6eb9ab6a767f1ade2b548092d0fa64795", "size": 10362, "url": "https://libraries.minecraft.net/net/java/jinput/jinput-platform/2.0.5/jinput-platform-2.0.5-natives-linux.jar"}}}},
        {"name": "ca.weblite:java-objc-bridge:1.0.0", "extract": {"exclude": ["META-INF/"]}, "natives": {"osx": "natives-osx"}, "rules": [{"action": "allow", "os": {"name": "osx"}}],
         "downloads": {"artifact": {"path": "ca/weblite/java-objc-bridge/1.0.0/java-objc-bridge-1.0.0.jar", "sha1": "6ef160c3133a78de015830860197602ca1c855d3", "size": 40502, "url": "https://libraries.minecraft.net/a.jar"}}}
      ]
    }"#;

    #[test]
    fn legacy_version_resolution_for_linux() {
        let v = VersionJson::parse(LEGACY.as_bytes()).unwrap();
        assert_eq!(v.java_major(), 8);
        let cp: Vec<String> = v.classpath_libraries().unwrap().into_iter().map(|a| a.name).collect();
        // The Linux LWJGL build is chosen, the macOS-only one and the macOS bridge are not.
        assert_eq!(
            cp,
            vec![
                "com.mojang:patchy:1.3.9",
                "org.lwjgl.lwjgl:lwjgl:2.9.4-nightly-20150209",
                "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209",
            ]
        );
        let natives = v.native_archives().unwrap();
        let names: Vec<&str> = natives.iter().map(|n| n.artifact.rel_path.rsplit('/').next().unwrap()).collect();
        assert_eq!(
            names,
            vec!["lwjgl-platform-2.9.4-nightly-20150209-natives-linux.jar", "jinput-platform-2.0.5-natives-linux.jar"]
        );
        assert!(natives.iter().all(|n| n.exclude == vec!["META-INF/".to_string()]));
        // Legacy argument line is split; the token placeholder is a separate argument.
        let game = v.game_arg_templates(&Features::default());
        let at = game.iter().position(|a| a == "--accessToken").unwrap();
        assert_eq!(game[at + 1], "${auth_access_token}");
        // No JVM section in old files: the launcher supplies the standard one.
        let jvm = v.jvm_arg_templates(&Features::default());
        assert!(
            jvm.contains(&"-Djava.library.path=${natives_directory}".to_string())
                && jvm.ends_with(&["-cp".to_string(), "${classpath}".to_string()])
        );
        // Mojang's patched Log4j configuration (the Log4Shell fix for this era) is picked up.
        let (arg, art) = v.logging_artifact().unwrap().unwrap();
        assert_eq!(arg, "-Dlog4j.configurationFile=${path}");
        assert_eq!(art.rel_path, "log_configs/client-1.12.xml");
        assert!(!v.has_feature_rule("is_quick_play_multiplayer"));
    }

    #[test]
    fn modern_arguments_follow_feature_rules() {
        let v = VersionJson::parse(
            br#"{"id":"26.3","type":"release","mainClass":"net.minecraft.client.main.Main",
              "arguments":{"game":["--username","${auth_player_name}",
                 {"rules":[{"action":"allow","features":{"is_demo_user":true}}],"value":"--demo"},
                 {"rules":[{"action":"allow","features":{"has_custom_resolution":true}}],"value":["--width","${resolution_width}","--height","${resolution_height}"]},
                 {"rules":[{"action":"allow","features":{"is_quick_play_multiplayer":true}}],"value":["--quickPlayMultiplayer","${quickPlayMultiplayer}"]}],
               "jvm":[{"rules":[{"action":"allow","os":{"name":"osx"}}],"value":["-XstartOnFirstThread"]},
                      {"rules":[{"action":"allow","os":{"arch":"x86"}}],"value":"-Xss1M"},"-cp","${classpath}"]},
              "assetIndex":{"id":"34","sha1":"e99f9973047785b5a9d52eaf72020b97545053c9","size":1,"url":"https://piston-meta.mojang.com/x.json"},
              "downloads":{"client":{"sha1":"e877b6a07acd633fb3bb475002175cec036e7b87","size":1,"url":"https://piston-data.mojang.com/c.jar"}},
              "libraries":[],"javaVersion":{"component":"java-runtime-epsilon","majorVersion":25}}"#,
        )
        .unwrap();
        assert_eq!(v.java_major(), 25);
        assert_eq!(
            v.jvm_arg_templates(&Features::default()),
            vec!["-cp", "${classpath}"],
            "macOS and 32-bit flags must not apply"
        );
        assert_eq!(v.game_arg_templates(&Features::default()), vec!["--username", "${auth_player_name}"]);
        let mut f = Features::default();
        f.set("is_demo_user");
        f.set("has_custom_resolution");
        assert_eq!(
            v.game_arg_templates(&f),
            vec![
                "--username",
                "${auth_player_name}",
                "--demo",
                "--width",
                "${resolution_width}",
                "--height",
                "${resolution_height}"
            ]
        );
        assert!(v.has_feature_rule("is_quick_play_multiplayer"));
    }

    #[test]
    fn version_files_with_unverifiable_or_hostile_content_are_rejected() {
        let base = |lib: &str, main: &str, id: &str| {
            format!(
                r#"{{"id":"{id}","type":"release","mainClass":"{main}",
                "assetIndex":{{"id":"1","sha1":"e99f9973047785b5a9d52eaf72020b97545053c9","size":1,"url":"https://piston-meta.mojang.com/x.json"}},
                "downloads":{{"client":{{"sha1":"e877b6a07acd633fb3bb475002175cec036e7b87","size":1,"url":"https://piston-data.mojang.com/c.jar"}}}},
                "libraries":[{lib}]}}"#
            )
        };
        let ok = base("", "a.B", "1.0");
        assert!(VersionJson::parse(ok.as_bytes()).is_ok());
        // a library with no digest cannot be verified
        let v = VersionJson::parse(
            base(r#"{"name":"a:b:1","url":"https://libraries.minecraft.net/"}"#, "a.B", "1.0").as_bytes(),
        )
        .unwrap();
        assert!(matches!(v.classpath_libraries(), Err(Error::Integrity(_))));
        // a path that climbs out of the library store
        let v = VersionJson::parse(
            base(r#"{"name":"a:b:1","downloads":{"artifact":{"path":"../../../.bashrc","sha1":"e877b6a07acd633fb3bb475002175cec036e7b87","size":1,"url":"https://libraries.minecraft.net/x"}}}"#, "a.B", "1.0").as_bytes(),
        )
        .unwrap();
        assert!(v.classpath_libraries().is_err());
        // a malformed digest
        let v = VersionJson::parse(
            base(r#"{"name":"a:b:1","downloads":{"artifact":{"path":"a/b.jar","sha1":"zz","size":1,"url":"https://libraries.minecraft.net/x"}}}"#, "a.B", "1.0").as_bytes(),
        )
        .unwrap();
        assert!(v.classpath_libraries().is_err());
        // ids and main classes are validated before they become paths or entry points
        assert!(VersionJson::parse(base("", "a.B", "../../x").as_bytes()).is_err());
        assert!(VersionJson::parse(base("", "-jar /tmp/x.jar", "1.0").as_bytes()).is_err());
    }

    #[test]
    fn class_names() {
        for good in ["net.minecraft.client.main.Main", "net.fabricmc.loader.impl.launch.knot.KnotClient", "A$B", "_x.y"]
        {
            assert!(validate_class_name(good).is_ok(), "{good}");
        }
        for bad in ["", "a..b", ".a", "a.", "a b", "a/b", "1a.b", "a;rm -rf", "-cp"] {
            assert!(validate_class_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn asset_index_rejects_traversal_and_bad_hashes() {
        assert!(
            AssetIndex::parse(
                br#"{"objects":{"../../etc/passwd":{"hash":"da39a3ee5e6b4b0d3255bfef95601890afd80709","size":0}}}"#
            )
            .is_err()
        );
        assert!(AssetIndex::parse(br#"{"objects":{"a/b.png":{"hash":"nothex","size":0}}}"#).is_err());
        let idx = AssetIndex::parse(
            br#"{"objects":{"a.png":{"hash":"da39a3ee5e6b4b0d3255bfef95601890afd80709","size":0},"b.png":{"hash":"DA39A3EE5E6B4B0D3255BFEF95601890AFD80709","size":0}}}"#,
        )
        .unwrap();
        let objs = idx.unique_objects().unwrap();
        assert_eq!(objs.len(), 1, "identical blobs are fetched once");
        assert_eq!(objs[0].rel_path, "objects/da/da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert!(!idx.needs_logical_tree());
    }
}
