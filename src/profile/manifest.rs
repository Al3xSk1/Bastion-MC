//! The signed profile manifest (`instances/<id>/profile.json`).
//!
//! The signature covers the exact bytes of this file. It is parsed only *after* the signature
//! verified, and never re-serialised for verification, so there is no canonicalisation step
//! for an attacker to play with.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    config::validate_version_id,
    error::{Error, Result},
    mojang::{maven::Coordinate, meta::validate_class_name},
    paths::{validate_file_name, validate_id},
    util::Expected,
};

pub const FORMAT: &str = "bastion-mc/profile/v1";
pub const MAX_MANIFEST_BYTES: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileManifest {
    pub format: String,
    /// Must equal the instance directory name: a manifest signed for one instance cannot be
    /// dropped into another.
    pub id: String,
    /// Monotonic. The launcher remembers the highest serial it accepted per instance and
    /// refuses anything lower, so an old (e.g. vulnerable) signed manifest cannot be replayed.
    pub serial: u64,
    pub minecraft: MinecraftPin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader: Option<Loader>,
    #[serde(default)]
    pub mods: Vec<ModEntry>,
    #[serde(default)]
    pub options: ProfileOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinecraftPin {
    pub version: String,
    /// Optional: pin the exact Mojang version file, extending the signature over Mojang's
    /// whole hash chain for this version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_json_sha1: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Fabric,
    Quilt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loader {
    pub kind: LoaderKind,
    pub version: String,
    pub main_class: String,
    /// Only `-Dname=value` system properties are accepted.
    #[serde(default)]
    pub jvm_args: Vec<String>,
    #[serde(default)]
    pub game_args: Vec<String>,
    pub libraries: Vec<LoaderLibrary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderLibrary {
    /// Maven coordinate.
    pub name: String,
    /// Repository base URL (HTTPS, launcher-allowlisted host).
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModEntry {
    pub file: String,
    pub sha256: String,
    pub size: u64,
    /// Informational: where the jar came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileOptions {
    /// Lift the JVM-wide deserialisation filter for this profile.
    #[serde(default)]
    pub allow_java_serialization: bool,
}

impl ProfileManifest {
    /// Parse bytes whose signature has already been verified.
    pub fn parse_verified(bytes: &[u8], instance_id: &str) -> Result<Self> {
        let m: ProfileManifest =
            serde_json::from_slice(bytes).map_err(|e| Error::Format(format!("profile manifest: {e}")))?;
        m.validate(instance_id)?;
        Ok(m)
    }

    pub fn validate(&self, instance_id: &str) -> Result<()> {
        let bad = |m: String| Err(Error::Policy(format!("profile manifest: {m}")));
        if self.format != FORMAT {
            return bad(format!("unsupported format {:?} (expected {FORMAT:?})", self.format));
        }
        validate_id(&self.id)?;
        if self.id != instance_id {
            return bad(format!("is for instance {:?}, not {instance_id:?}", self.id));
        }
        if self.serial == 0 {
            return bad("serial must be at least 1".into());
        }
        validate_version_id(&self.minecraft.version)?;
        if let Some(h) = &self.minecraft.version_json_sha1 {
            Expected::sha1(h)?;
        }
        if let Some(loader) = &self.loader {
            validate_class_name(&loader.main_class)?;
            validate_version_id(&loader.version)?;
            for arg in &loader.jvm_args {
                // The loader only ever needs system properties. Anything else (-javaagent,
                // -agentpath, -Xbootclasspath, -XX flags…) is refused even when signed.
                let is_property = arg.strip_prefix("-D").is_some_and(|rest| {
                    rest.split_once('=').is_some_and(|(k, _)| {
                        !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                    })
                });
                if !is_property || arg.contains('\0') {
                    return bad(format!("loader JVM argument {arg:?} is not a -Dname=value system property"));
                }
            }
            if loader.game_args.iter().any(|a| a.contains('\0')) {
                return bad("loader game argument contains NUL".into());
            }
            if loader.libraries.is_empty() {
                return bad("loader declares no libraries".into());
            }
            let mut seen = BTreeSet::new();
            for lib in &loader.libraries {
                let coord = Coordinate::parse(&lib.name)?;
                Expected::sha256(&lib.sha256)?;
                if lib.size == 0 {
                    return bad(format!("library {} has size 0", lib.name));
                }
                if !lib.url.starts_with("https://") {
                    return bad(format!("library {} repository is not HTTPS", lib.name));
                }
                if !seen.insert(coord.path()) {
                    return bad(format!("library {} listed twice", lib.name));
                }
            }
        } else if !self.mods.is_empty() {
            return bad("lists mods but no mod loader".into());
        }
        let mut names = BTreeSet::new();
        for m in &self.mods {
            validate_file_name(&m.file)?;
            if !m.file.ends_with(".jar") {
                return bad(format!("mod {:?} is not a .jar", m.file));
            }
            Expected::sha256(&m.sha256)?;
            if m.size == 0 {
                return bad(format!("mod {:?} has size 0", m.file));
            }
            // Case-insensitive uniqueness: "A.jar" and "a.jar" side by side is a trap.
            if !names.insert(m.file.to_ascii_lowercase()) {
                return bad(format!("mod {:?} listed twice", m.file));
            }
            if let Some(src) = &m.source
                && !src.starts_with("https://")
            {
                return bad(format!("mod {:?} source is not HTTPS", m.file));
            }
        }
        Ok(())
    }

    /// The exact bytes that get written to disk and signed.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out =
            serde_json::to_vec_pretty(self).map_err(|e| Error::Other(format!("serialising manifest: {e}")))?;
        out.push(b'\n');
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample(id: &str) -> ProfileManifest {
        ProfileManifest {
            format: FORMAT.into(),
            id: id.into(),
            serial: 1,
            minecraft: MinecraftPin { version: "1.21.4".into(), version_json_sha1: None },
            loader: Some(Loader {
                kind: LoaderKind::Fabric,
                version: "0.16.10".into(),
                main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".into(),
                jvm_args: vec!["-DFabricMcEmu= net.minecraft.client.main.Main ".into()],
                game_args: vec![],
                libraries: vec![LoaderLibrary {
                    name: "net.fabricmc:fabric-loader:0.16.10".into(),
                    url: "https://maven.fabricmc.net/".into(),
                    sha256: "a".repeat(64),
                    size: 10,
                }],
            }),
            mods: vec![ModEntry {
                file: "sodium-fabric-0.6.5+mc1.21.4.jar".into(),
                sha256: "b".repeat(64),
                size: 5,
                source: None,
            }],
            options: ProfileOptions::default(),
        }
    }

    #[test]
    fn roundtrip_and_binding_to_instance() {
        let m = sample("perf");
        let bytes = m.to_bytes().unwrap();
        assert_eq!(ProfileManifest::parse_verified(&bytes, "perf").unwrap(), m);
        assert!(
            ProfileManifest::parse_verified(&bytes, "other").is_err(),
            "manifest must not be accepted for another instance"
        );
    }

    #[test]
    fn rejects_dangerous_or_malformed_content() {
        let check = |f: &dyn Fn(&mut ProfileManifest)| {
            let mut m = sample("perf");
            f(&mut m);
            m.validate("perf")
        };
        assert!(check(&|_| {}).is_ok());
        assert!(check(&|m| m.serial = 0).is_err());
        assert!(check(&|m| m.format = "v0".into()).is_err());
        assert!(check(&|m| m.loader.as_mut().unwrap().jvm_args.push("-javaagent:/game/mods/x.jar".into())).is_err());
        assert!(check(&|m| m.loader.as_mut().unwrap().jvm_args.push("-XX:-DisableAttachMechanism".into())).is_err());
        assert!(check(&|m| m.loader.as_mut().unwrap().jvm_args.push("-Dno_equals".into())).is_err());
        assert!(check(&|m| m.loader.as_mut().unwrap().main_class = "x; rm -rf ~".into()).is_err());
        assert!(check(&|m| m.loader.as_mut().unwrap().libraries[0].url = "http://maven.fabricmc.net/".into()).is_err());
        assert!(check(&|m| m.loader.as_mut().unwrap().libraries[0].name = "../../x:y:1".into()).is_err());
        assert!(check(&|m| m.loader.as_mut().unwrap().libraries[0].sha256 = "short".into()).is_err());
        assert!(check(&|m| m.mods[0].file = "../evil.jar".into()).is_err());
        assert!(check(&|m| m.mods[0].file = "notajar.zip".into()).is_err());
        assert!(
            check(&|m| m.mods.push(ModEntry {
                file: "SODIUM-fabric-0.6.5+mc1.21.4.jar".into(),
                sha256: "c".repeat(64),
                size: 1,
                source: None
            }))
            .is_err()
        );
        assert!(check(&|m| m.loader = None).is_err(), "mods without a loader make no sense");
        assert!(check(&|m| m.minecraft.version = "../1.21".into()).is_err());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut v: serde_json::Value = serde_json::from_slice(&sample("perf").to_bytes().unwrap()).unwrap();
        v["extra_jvm_args"] = serde_json::json!(["-javaagent:x"]);
        assert!(ProfileManifest::parse_verified(&serde_json::to_vec(&v).unwrap(), "perf").is_err());
    }
}
