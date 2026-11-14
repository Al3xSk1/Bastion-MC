//! `bastion-mc bedrock client|import|versions|remove` end to end: the launcher binary, run with
//! its four XDG directories and its home under this build's scratch directory, on a stand-in
//! version (`store::write_stand_in_version`: the layout mcpelauncher-ui writes, with ELF
//! headers for libraries). There is no mcpelauncher-client, game download or Google Play
//! purchase on the machine this was written on; `/usr/bin/true` stands in for a root-owned
//! client where one is needed.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

struct Env {
    root: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Env {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bedrock-store-{tag}-{}", std::process::id()));
        if root.exists() {
            bastion_mc::tree::remove(&root, None).unwrap();
        }
        for dir in ["config", "data", "cache", "state", "home"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        Env { root }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(LAUNCHER)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin")
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .output()
            .unwrap()
    }

    /// Where mcpelauncher-ui (the native package) keeps versions with this XDG_DATA_HOME.
    fn ui_versions(&self) -> PathBuf {
        self.root.join("data/mcpelauncher/versions")
    }

    fn store(&self) -> PathBuf {
        self.root.join("data/bastion-mc/bedrock/versions")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = bastion_mc::tree::remove(&self.root, None);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_version_is_imported_listed_verified_and_removed_with_the_cli() {
    let env = Env::new("cycle");
    let out = env.run(&["bedrock", "versions"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "no Bedrock versions imported yet\n");

    // mcpelauncher-ui has a version: offered for import, with its path.
    let ui = env.ui_versions().join("1.26.62.1");
    bastion_mc::bedrock::store::write_stand_in_version(&ui).unwrap();
    let out = env.run(&["bedrock", "versions"]);
    let listing = text(&out.stdout);
    assert!(listing.contains("mcpelauncher-ui has these versions, not imported"), "{listing}");
    assert!(listing.contains(&format!("{}\tMinecraft 1.26.62.1 (version code 982606201)", ui.display())), "{listing}");

    let out = env.run(&["bedrock", "import", ui.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "1.26.62.1\n");
    assert!(
        text(&out.stderr).contains("imported Minecraft 1.26.62.1 (version code 982606201) as 1.26.62.1"),
        "{}",
        text(&out.stderr)
    );
    let out = env.run(&["bedrock", "versions", "--verify"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let listing = text(&out.stdout);
    assert!(listing.starts_with("1.26.62.1\tMinecraft 1.26.62.1 (version code 982606201)\t6 files"), "{listing}");
    assert!(listing.contains("1.26.62.1: every file matches its SHA-256 (6 files)"), "{listing}");
    assert!(!listing.contains("not imported"), "an imported version is not offered again: {listing}");

    // Importing it again under the same id is refused; under another, fine.
    let out = env.run(&["bedrock", "import", ui.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(5), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("already imported"));
    let out = env.run(&["bedrock", "import", ui.to_str().unwrap(), "--id", "copy-2"]);
    assert!(out.status.success(), "{}", text(&out.stderr));

    // A file changed behind bastion's back: the check says which, with the integrity exit code.
    let font = env.store().join("copy-2/game/assets/fonts/Mojangles.ttf");
    std::fs::set_permissions(font.parent().unwrap(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&font, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&font, b"changed").unwrap();
    let out = env.run(&["bedrock", "versions", "--verify"]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    let listing = text(&out.stdout);
    assert!(
        listing.contains("copy-2: INTEGRITY VIOLATION: Bedrock version copy-2: assets/fonts/Mojangles.ttf is 7 bytes"),
        "{listing}"
    );
    assert!(listing.contains("1.26.62.1: every file matches"), "{listing}");

    for id in ["copy-2", "1.26.62.1"] {
        let out = env.run(&["bedrock", "remove", id]);
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
    let out = env.run(&["bedrock", "versions"]);
    assert!(text(&out.stdout).starts_with("no Bedrock versions imported yet\n"));
    let out = env.run(&["bedrock", "remove", "1.26.62.1"]);
    assert_eq!(out.status.code(), Some(2), "a version that is not there is a usage error");
}

#[test]
fn refusals_reach_the_terminal_with_their_reasons_and_exit_codes() {
    let env = Env::new("refusals");
    let apk = env.root.join("home/minecraft.apk");
    std::fs::write(&apk, b"PK\x03\x04").unwrap();
    let out = env.run(&["bedrock", "import", apk.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(5));
    assert!(text(&out.stderr).contains("never an APK"), "{}", text(&out.stderr));

    let arm = env.ui_versions().join("arm");
    bastion_mc::bedrock::store::write_stand_in_version(&arm).unwrap();
    std::fs::rename(arm.join("lib/x86_64"), arm.join("lib/arm64-v8a")).unwrap();
    let out = env.run(&["bedrock", "import", arm.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(5));
    assert!(text(&out.stderr).contains("bastion runs only x86_64 builds"), "{}", text(&out.stderr));
    let out = env.run(&["bedrock", "versions"]);
    assert!(text(&out.stdout).contains("no x86_64 libraries: cannot be imported"), "{}", text(&out.stdout));

    let out = env.run(&["bedrock", "import", arm.to_str().unwrap(), "--id", "../escape"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    let out = env.run(&["--events", "bedrock", "versions"]);
    assert_eq!(out.status.code(), Some(2), "not offered with --events");
    let out = env.run(&["bedrock", "frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn bedrock_client_reports_the_client_or_where_it_looked() {
    let env = Env::new("client");
    let out = env.run(&["bedrock", "client"]);
    if !Path::new("/usr/bin/mcpelauncher-client").exists() && !Path::new("/usr/local/bin/mcpelauncher-client").exists()
    {
        assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "looked for: /usr/bin/mcpelauncher-client, /usr/local/bin/mcpelauncher-client\n");
        assert!(text(&out.stderr).contains("AUR package mcpelauncher-linux"), "{}", text(&out.stderr));
    }
    // A root-owned program from a package, configured as the client.
    let config = env.root.join("config/bastion-mc/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[bedrock]\nclient = \"/usr/bin/true\"\n").unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let out = env.run(&["bedrock", "client"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report = text(&out.stdout);
    assert!(report.starts_with("client: /usr/bin/true\nversion: "), "{report}");
    assert!(report.contains("(coreutils)"), "{report}");
    // A program of the user's: refused, with the reason and the policy exit code.
    let own = env.root.join("home/mcpelauncher-client");
    std::fs::copy("/usr/bin/true", &own).unwrap();
    std::fs::write(&config, format!("[bedrock]\nclient = \"{}\"\n", own.display())).unwrap();
    let out = env.run(&["bedrock", "client"]);
    assert_eq!(out.status.code(), Some(5), "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with(&format!("looked for: {}", own.display())));
}
