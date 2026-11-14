//! The JVM is always one of Arch's packaged OpenJDK builds under `/usr/lib/jvm` (the tree
//! `archlinux-java` manages). The launcher never downloads a runtime and never runs one from
//! a user-writable location: the binary and its home must be root-owned.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    config::JavaMatch,
    error::{Context, Error, Result},
    util,
};

pub const JVM_ROOT: &str = "/usr/lib/jvm";

#[derive(Debug, Clone)]
pub struct Jvm {
    /// Canonical JAVA_HOME (for Java 8 runtimes this is the `jre` sub-directory).
    pub home: PathBuf,
    pub java: PathBuf,
    /// Feature release: 8, 17, 21, 25, …
    pub major: u32,
    pub version: String,
    /// Root-owned directories outside `/usr` that the runtime reaches through symlinks
    /// (Arch keeps `conf` in `/etc/java-openjdk` and `cacerts` behind `/etc/ssl`).
    pub external_dirs: Vec<PathBuf>,
}

/// `JAVA_VERSION="21.0.5"` → 21, `"1.8.0_432"` → 8.
fn parse_major(version: &str) -> Option<u32> {
    let mut parts = version.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 { parts.next()?.parse().ok() } else { Some(first) }
}

fn read_release(home: &Path) -> Option<String> {
    for dir in [Some(home), home.parent()].into_iter().flatten() {
        if let Ok(text) = fs::read_to_string(dir.join("release")) {
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("JAVA_VERSION=") {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

/// Follow every symlink inside the JVM home hop by hop and collect the top-level `/etc`
/// directories those hops pass through. Anything pointing elsewhere outside `/usr` is refused.
fn external_dirs(home: &Path) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, depth: usize, links: &mut Vec<PathBuf>) {
        if depth > 6 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_symlink() => links.push(path),
                Ok(t) if t.is_dir() => walk(&path, depth + 1, links),
                _ => {}
            }
        }
    }
    let mut links = Vec::new();
    walk(home, 0, &mut links);

    let mut dirs = BTreeSet::new();
    for link in links {
        let mut current = link.clone();
        for _ in 0..16 {
            let Ok(target) = fs::read_link(&current) else { break };
            let next =
                if target.is_absolute() { target } else { current.parent().unwrap_or(Path::new("/")).join(target) };
            // Normalise without touching the filesystem so each hop is inspected.
            let mut norm = PathBuf::new();
            for c in next.components() {
                match c {
                    std::path::Component::ParentDir => {
                        norm.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => norm.push(other),
                }
            }
            if !norm.starts_with("/usr") {
                let mut comps = norm.components();
                let top: PathBuf = [comps.next(), comps.next(), comps.next()].into_iter().flatten().collect();
                if !top.starts_with("/etc") || top == Path::new("/etc") {
                    return Err(Error::Policy(format!(
                        "JVM file {} links to {}, outside /usr and /etc; refusing this runtime",
                        link.display(),
                        norm.display()
                    )));
                }
                dirs.insert(top);
            }
            current = norm;
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        if dir.exists() {
            out.push(util::require_root_owned(&dir)?);
        }
    }
    Ok(out)
}

/// Every usable runtime under `/usr/lib/jvm`, lowest feature release first.
pub fn discover() -> Result<Vec<Jvm>> {
    discover_in(Path::new(JVM_ROOT))
}

fn discover_in(root: &Path) -> Result<Vec<Jvm>> {
    let entries = fs::read_dir(root).ctx(|| format!("reading {} (is a JRE installed?)", root.display()))?;
    let mut found: Vec<Jvm> = Vec::new();
    for entry in entries.flatten() {
        // `default` and `default-runtime` are symlinks maintained by archlinux-java.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        for home in [entry.path(), entry.path().join("jre")] {
            let java = home.join("bin/java");
            if !java.is_file() {
                continue;
            }
            let Some(version) = read_release(&home) else { continue };
            let Some(major) = parse_major(&version) else { continue };
            // Ownership failures are not skipped silently: an attacker-writable JVM under
            // /usr/lib/jvm is a finding the user must see.
            let java = util::require_root_owned(&java)?;
            let home = util::require_root_owned(&home)?;
            if found.iter().any(|j| j.java == java) {
                continue;
            }
            let external_dirs = external_dirs(&home)?;
            found.push(Jvm { home, java, major, version, external_dirs });
        }
    }
    found.sort_by(|a, b| (a.major, &a.home).cmp(&(b.major, &b.home)));
    Ok(found)
}

/// Pick the runtime for a game version that declares `required` as its Java release.
pub fn select(installed: &[Jvm], required: u32, policy: JavaMatch, override_home: Option<&Path>) -> Result<Jvm> {
    let min_bootstrap = crate::BOOTSTRAP_MIN_JAVA;
    let chosen = if let Some(home) = override_home {
        let real = fs::canonicalize(home).ctx(|| format!("resolving java.home {}", home.display()))?;
        installed.iter().find(|j| j.home == real || j.home.parent() == Some(real.as_path())).ok_or_else(|| {
            Error::Config(format!(
                "java.home {} is not a root-owned runtime under {JVM_ROOT}; the launcher only runs system JVMs",
                home.display()
            ))
        })?
    } else {
        let pick = match policy {
            JavaMatch::Exact => installed.iter().find(|j| j.major == required),
            JavaMatch::Newer => installed.iter().find(|j| j.major >= required),
        };
        pick.ok_or_else(|| {
            let have: Vec<String> = installed.iter().map(|j| j.major.to_string()).collect();
            Error::Config(format!(
                "this game version needs Java {required}{}; installed under {JVM_ROOT}: [{}]. \
                 Install it with pacman ({}){}. The launcher finds a runtime by the `release` file in its directory, \
                 which Arch's jre packages of Java 17 and newer do not ship. The launcher never downloads a JVM.",
                if policy == JavaMatch::Newer { " or newer" } else { "" },
                have.join(", "),
                if required <= 8 {
                    "jre8-openjdk-headless or jdk8-openjdk".to_string()
                } else {
                    format!("jdk{required}-openjdk, or jdk-openjdk if {required} is the current release")
                },
                if policy == JavaMatch::Exact {
                    ", or set java.match = \"newer\" to accept a newer runtime"
                } else {
                    ""
                },
            ))
        })?
    };
    if chosen.major < min_bootstrap {
        return Err(Error::Config(format!(
            "Java {} cannot load the launch bootstrap (built for Java {min_bootstrap}+); rebuild bastion-mc with a JDK that supports `javac --release 8`",
            chosen.major
        )));
    }
    Ok(chosen.clone())
}

/// JVM options that the launcher always appends *after* everything a version file or profile
/// contributes, so they win.
///
/// Note for readers of the original requirements: `-XX:+DisableAttachAgent` does not exist —
/// HotSpot refuses to start with it. The real switch is `-XX:+DisableAttachMechanism`.
pub fn hardening_flags(major: u32, allow_serialization: bool) -> Vec<String> {
    let mut f: Vec<String> = vec![
        // No dynamic attach: no jcmd/jstack/agent injection into the running game.
        "-XX:+DisableAttachMechanism".into(),
        // No /tmp/hsperfdata_<user>/<pid> file advertising the JVM's command line.
        "-XX:-UsePerfData".into(),
        // Log4Shell class: lookups off (effective for Log4j 2.10–2.14; older versions are
        // covered by Mojang's patched logging configuration, newer ones are fixed).
        "-Dlog4j2.formatMsgNoLookups=true".into(),
        // JNDI must never load remote code.
        "-Dcom.sun.jndi.ldap.object.trustURLCodebase=false".into(),
        "-Dcom.sun.jndi.rmi.object.trustURLCodebase=false".into(),
        "-Dcom.sun.jndi.cosnaming.object.trustURLCodebase=false".into(),
        "-Djava.rmi.server.useCodebaseOnly=true".into(),
        "-Djava.io.tmpdir=/tmp".into(),
        "-Duser.home=/game".into(),
        // The trust store the launcher generated for this launch (see `sandbox::trust`),
        // named explicitly rather than left to where the JDK's own symlink happens to point.
        format!("-Djavax.net.ssl.trustStore={}", crate::sandbox::trust::JAVA_STORE),
        "-Djavax.net.ssl.trustStoreType=jks".into(),
    ];
    if !allow_serialization {
        // Reject every java.io.ObjectInputStream deserialisation JVM-wide. Vanilla does not
        // use Java serialisation; several historic mod RCEs (e.g. "BleedingPipe") did.
        f.push("-Djdk.serialFilter=!*".into());
    }
    if major >= 9 {
        // On a fatal error, exit instead of abort(): no core image is ever produced.
        f.push("-XX:-CreateCoredumpOnCrash".into());
    }
    if major >= 21 {
        f.push("-XX:-EnableDynamicAgentLoading".into());
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(parse_major("1.8.0_432"), Some(8));
        assert_eq!(parse_major("17.0.13"), Some(17));
        assert_eq!(parse_major("21"), Some(21));
        assert_eq!(parse_major("27.0.1.0.3"), Some(27));
        assert_eq!(parse_major("25-ea+12"), Some(25));
        assert_eq!(parse_major("garbage"), None);
    }

    fn jvm(major: u32) -> Jvm {
        Jvm {
            home: PathBuf::from(format!("/usr/lib/jvm/java-{major}-openjdk")),
            java: PathBuf::from(format!("/usr/lib/jvm/java-{major}-openjdk/bin/java")),
            major,
            version: major.to_string(),
            external_dirs: vec![],
        }
    }

    #[test]
    fn selection_policy() {
        let installed = vec![jvm(8), jvm(17), jvm(25)];
        assert_eq!(select(&installed, 17, JavaMatch::Exact, None).unwrap().major, 17);
        assert!(select(&installed, 21, JavaMatch::Exact, None).is_err(), "exact must not silently substitute");
        assert_eq!(select(&installed, 21, JavaMatch::Newer, None).unwrap().major, 25);
        assert_eq!(select(&installed, 8, JavaMatch::Newer, None).unwrap().major, 8, "lowest sufficient runtime");
        assert!(select(&installed, 26, JavaMatch::Newer, None).is_err());
        // An override outside /usr/lib/jvm is refused.
        assert!(select(&installed, 17, JavaMatch::Exact, Some(Path::new("/tmp"))).is_err());
    }

    #[test]
    fn flags_are_version_gated_and_use_the_real_attach_switch() {
        let f8 = hardening_flags(8, false);
        assert!(f8.contains(&"-XX:+DisableAttachMechanism".to_string()));
        assert!(!f8.iter().any(|a| a.contains("DisableAttachAgent")), "that flag does not exist in HotSpot");
        assert!(!f8.iter().any(|a| a.contains("CreateCoredumpOnCrash") || a.contains("EnableDynamicAgentLoading")));
        let f21 = hardening_flags(21, false);
        assert!(f21.contains(&"-XX:-EnableDynamicAgentLoading".to_string()));
        assert!(f21.contains(&"-Djdk.serialFilter=!*".to_string()));
        assert!(!hardening_flags(21, true).iter().any(|a| a.starts_with("-Djdk.serialFilter")));
    }

    #[test]
    fn system_runtimes_are_discovered_and_root_owned() {
        let found = discover().expect("discover system JVMs");
        assert!(!found.is_empty(), "this test machine has a JDK under /usr/lib/jvm");
        for j in &found {
            assert!(j.java.starts_with(JVM_ROOT));
            assert!(j.external_dirs.iter().all(|d| d.starts_with("/etc")));
        }
    }
}
