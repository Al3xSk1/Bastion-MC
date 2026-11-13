//! Maven coordinates → repository-relative paths.

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coordinate {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub classifier: Option<String>,
    pub extension: String,
}

fn part_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('.')
        && !s.contains("..")
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
}

impl Coordinate {
    /// `group:artifact:version[:classifier][@extension]`
    pub fn parse(spec: &str) -> Result<Self> {
        let bad = || Error::Format(format!("invalid maven coordinate {spec:?}"));
        let (main, extension) = match spec.split_once('@') {
            Some((m, e)) => (m, e.to_string()),
            None => (spec, "jar".to_string()),
        };
        let parts: Vec<&str> = main.split(':').collect();
        if !(3..=4).contains(&parts.len()) || !parts.iter().all(|p| part_ok(p)) || !part_ok(&extension) {
            return Err(bad());
        }
        Ok(Coordinate {
            group: parts[0].to_string(),
            artifact: parts[1].to_string(),
            version: parts[2].to_string(),
            classifier: parts.get(3).map(|s| s.to_string()),
            extension,
        })
    }

    /// `org.ow2.asm:asm:9.7` → `org/ow2/asm/asm/9.7/asm-9.7.jar`
    pub fn path(&self) -> String {
        let classifier = self.classifier.as_ref().map(|c| format!("-{c}")).unwrap_or_default();
        format!(
            "{}/{}/{}/{}-{}{}.{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            self.artifact,
            self.version,
            classifier,
            self.extension
        )
    }

    /// Identity used when a profile library overrides a vanilla one (version is ignored).
    pub fn key(&self) -> String {
        match &self.classifier {
            Some(c) => format!("{}:{}:{c}", self.group, self.artifact),
            None => format!("{}:{}", self.group, self.artifact),
        }
    }

    pub fn file_name(&self) -> String {
        self.path().rsplit('/').next().unwrap_or_default().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(Coordinate::parse("org.ow2.asm:asm:9.7").unwrap().path(), "org/ow2/asm/asm/9.7/asm-9.7.jar");
        assert_eq!(
            Coordinate::parse("org.lwjgl:lwjgl:3.3.3:natives-linux").unwrap().path(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar"
        );
        assert_eq!(Coordinate::parse("a.b:c:1.0@zip").unwrap().path(), "a/b/c/1.0/c-1.0.zip");
        assert_eq!(
            Coordinate::parse("net.fabricmc:fabric-loader:0.16.10").unwrap().key(),
            "net.fabricmc:fabric-loader"
        );
    }

    #[test]
    fn rejects_traversal() {
        for bad in ["a:b", "a:b:c:d:e", "../x:b:1", "a:b:../../etc", "a:b:1@../sh", "a/b:c:1", "a:b:", "a b:c:1"] {
            assert!(Coordinate::parse(bad).is_err(), "{bad:?}");
        }
    }
}
