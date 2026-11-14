//! A Bedrock instance on disk: what `instance.toml` says, and how a Java-only command
//! refuses one.
//!
//! ```toml
//! kind = "bedrock"
//! [bedrock]
//! version = "1.26.62.1"          # an id in the store (`bastion-mc bedrock versions`)
//! servers = [{ host = "play.example.org", port = 19132 }]   # added to [bedrock] servers
//! ```
//!
//! The Java keys that mean nothing here are refused with the reason, not ignored: a user who
//! writes `memory_mb` or `[network] servers` into a Bedrock instance expects an effect, and
//! silently getting none would be a surprise in a file that decides what the game may reach.

use std::path::Path;

use serde::Deserialize;

use crate::{
    bedrock::BedrockServer,
    config::InstanceConfig,
    error::{Error, Result},
    paths::{Paths, validate_id},
};

/// The instance's persistent Bedrock data (worlds and the game's settings; see
/// `bedrock::launch::profile`), inside the instance directory.
pub const DATA_DIR: &str = "bedrock-data";

/// Most servers one instance may list (the network package validates each).
pub const MAX_SERVERS: usize = 64;

/// `kind` in `instance.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceKind {
    /// The default when `kind` is absent: Minecraft: Java Edition (vanilla or curated).
    Java,
    /// Minecraft: Bedrock Edition, the Android build (see `bedrock`).
    Bedrock,
}

/// `[bedrock]` in a Bedrock instance's `instance.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceSettings {
    /// The imported version this instance runs: an id in the store, validated like an
    /// instance id (it names a directory).
    pub version: String,
    /// Servers this instance may join, after `[bedrock] servers` of config.toml.
    #[serde(default)]
    pub servers: Vec<BedrockServer>,
}

/// Is this configuration a Bedrock instance's?
pub fn is_bedrock(cfg: &InstanceConfig) -> bool {
    cfg.kind == Some(InstanceKind::Bedrock)
}

/// The checks `InstanceConfig::load` runs for the `kind` and `[bedrock]` keys. `file` is
/// only used in messages.
pub fn validate(cfg: &InstanceConfig, file: &Path) -> Result<()> {
    let bad = |what: String| Err(Error::Config(format!("{}: {what}", file.display())));
    if !is_bedrock(cfg) {
        if cfg.bedrock.is_some() {
            return bad("a [bedrock] section belongs to a Bedrock instance; add `kind = \"bedrock\"` at the top".into());
        }
        return Ok(());
    }
    let Some(settings) = &cfg.bedrock else {
        return bad("`kind = \"bedrock\"` needs a [bedrock] section with `version = \"<id>\"` (an imported version: \
                    `bastion-mc bedrock versions`)"
            .into());
    };
    if validate_id(&settings.version).is_err() {
        return bad(format!(
            "[bedrock] version {:?} is not a version id (lower-case letters, digits, '.', '_' and '-')",
            crate::util::sanitize_line(settings.version.as_bytes(), 80)
        ));
    }
    if settings.servers.len() > MAX_SERVERS {
        return bad(format!("[bedrock] servers lists more than {MAX_SERVERS} servers"));
    }
    if cfg.minecraft.is_some() {
        return bad(
            "`minecraft = …` is a Java setting; a Bedrock instance names its version as [bedrock] version".into()
        );
    }
    if cfg.memory_mb.is_some() {
        return bad("`memory_mb` is the Java heap; a Bedrock session's memory ceiling is [sandbox] memory_max_mb in \
                    config.toml"
            .into());
    }
    if !cfg.network.servers.is_empty() {
        return bad("[network] servers are Java servers; a Bedrock instance lists its servers as [bedrock] servers \
                    (RakNet over UDP, default port 19132)"
            .into());
    }
    if cfg.companion != crate::companion::CompanionOverride::default() {
        return bad("[companion] is the Java companion mod; it does not exist for Bedrock".into());
    }
    Ok(())
}

/// The `instance.toml` that `bastion-mc instance new <id> --bedrock <version>` writes.
pub fn template(version: &str) -> String {
    format!(
        "# bastion-mc instance settings. Unknown keys are errors.\n\
         kind = \"bedrock\"\n\
         # width = 1280\n\
         # height = 720\n\
         \n\
         [bedrock]\n\
         version = \"{version}\"\n\
         # servers = [{{ host = \"play.example.org\", port = 19132 }}]\n\
         \n\
         [network]\n\
         # mode = \"filtered\"   # or \"none\": offline single-player, nothing reachable (\"open\" is refused)\n"
    )
}

/// Does the instance `id` exist and say `kind = "bedrock"`? A configuration that cannot be
/// read is not a Bedrock instance here: the command that reads it next reports the problem.
pub fn is_bedrock_instance(paths: &Paths, id: &str) -> bool {
    paths.instance(id).ok().and_then(|dir| InstanceConfig::load(&dir).ok()).is_some_and(|cfg| is_bedrock(&cfg))
}

/// The refusal of a Java-only command (`install`, `profile …`, `pack …`) for a Bedrock
/// instance: a sentence that says what the instance is and what to do instead.
pub fn java_only(command: &str, id: &str) -> Error {
    Error::Usage(format!(
        "{id} is a Bedrock instance: `{command}` is for Java instances. A Bedrock instance runs a version imported \
         with `bastion-mc bedrock import`, and `bastion-mc launch {id}` starts it."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<InstanceConfig> {
        let cfg: InstanceConfig = toml::from_str(text).map_err(|e| Error::Config(e.to_string()))?;
        validate(&cfg, Path::new("instance.toml"))?;
        Ok(cfg)
    }

    #[test]
    fn the_template_is_a_valid_bedrock_instance() {
        let cfg = parse(&template("1.26.62.1")).unwrap();
        assert!(is_bedrock(&cfg));
        let settings = cfg.bedrock.unwrap();
        assert_eq!(settings.version, "1.26.62.1");
        assert!(settings.servers.is_empty());
    }

    #[test]
    fn a_bedrock_instance_takes_its_servers_with_bedrocks_default_port() {
        let cfg = parse(
            "kind = \"bedrock\"\n[bedrock]\nversion = \"1.26.62.1\"\nservers = [{ host = \"play.example.org\" }, \
             { host = \"203.0.113.7\", port = 19133 }]\n",
        )
        .unwrap();
        let servers = cfg.bedrock.unwrap().servers;
        assert_eq!(servers[0], BedrockServer { host: "play.example.org".into(), port: crate::bedrock::DEFAULT_PORT });
        assert_eq!(servers[1].port, 19133);
    }

    /// A Java key in a Bedrock instance is an error with the reason, never silently ignored.
    #[test]
    fn java_keys_in_a_bedrock_instance_are_refused_with_the_reason() {
        let head = "kind = \"bedrock\"\n";
        let body = "[bedrock]\nversion = \"1.26.62.1\"\n";
        for (extra, says) in [("minecraft = \"1.21.4\"\n", "Java setting"), ("memory_mb = 4096\n", "Java heap")] {
            let why = parse(&format!("{head}{extra}{body}")).unwrap_err().to_string();
            assert!(why.contains(says), "{extra}: {why}");
        }
        let why = parse(&format!("{head}{body}[network]\nservers = [\"play.example.org\"]\n")).unwrap_err();
        assert!(why.to_string().contains("[bedrock] servers"), "{why}");
        let why = parse(&format!("{head}{body}[companion]\nzoom = true\n")).unwrap_err();
        assert!(why.to_string().contains("companion"), "{why}");
        // The network mode is the one [network] key that means something here.
        assert!(parse(&format!("{head}{body}[network]\nmode = \"none\"\n")).is_ok());
    }

    #[test]
    fn kind_and_section_must_come_together_and_the_version_must_be_an_id() {
        assert!(parse("kind = \"bedrock\"\n").unwrap_err().to_string().contains("needs a [bedrock] section"));
        assert!(parse("[bedrock]\nversion = \"1.26.62.1\"\n").unwrap_err().to_string().contains("add `kind"));
        for bad in ["../escape", "", "1.26/62", "UPPER", ".hidden"] {
            let text = format!("kind = \"bedrock\"\n[bedrock]\nversion = {bad:?}\n");
            assert!(parse(&text).is_err(), "{bad:?}");
        }
        assert!(parse("kind = \"bedrock\"\n[bedrock]\nversion = \"1.26.62.1\"\nlan = true\n").is_err(), "unknown key");
        assert!(parse("kind = \"windows\"\n").is_err(), "unknown kind");
        // An explicit Java kind is the default spelled out.
        let java = parse("kind = \"java\"\nminecraft = \"1.21.4\"\n").unwrap();
        assert!(!is_bedrock(&java));
        let many: String = (0..=MAX_SERVERS).map(|i| format!("{{ host = \"s{i}.example.org\" }},")).collect();
        let text = format!("kind = \"bedrock\"\n[bedrock]\nversion = \"1.26.62.1\"\nservers = [{many}]\n");
        assert!(parse(&text).unwrap_err().to_string().contains("more than"));
    }
}
