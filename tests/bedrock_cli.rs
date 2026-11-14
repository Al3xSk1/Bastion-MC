//! The command line around a Bedrock instance: `instance new --bedrock`, how the instance is
//! listed, and every Java-only command refusing it with a sentence that says what to do. The
//! real binary, with its four XDG directories under the test's own directory (nothing here
//! reaches the network, a display, a session bus or the kernel keyring).

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

const BIN: &str = env!("CARGO_BIN_EXE_bastion-mc");

struct Home {
    root: PathBuf,
}

impl Home {
    fn new(tag: &str) -> Home {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "bedrock-cli-{tag}-{}",
            bastion_mc::util::hex_encode(&bastion_mc::util::random_bytes::<6>().unwrap())
        ));
        std::fs::create_dir_all(&root).unwrap();
        Home { root }
    }

    fn run(&self, args: &[&str]) -> Output {
        let xdg = |kind: &str| self.root.join(kind);
        Command::new(BIN)
            .args(args)
            .env_clear()
            .env("HOME", &self.root)
            .env("XDG_CONFIG_HOME", xdg("config"))
            .env("XDG_DATA_HOME", xdg("data"))
            .env("XDG_CACHE_HOME", xdg("cache"))
            .env("XDG_STATE_HOME", xdg("state"))
            .env("PATH", "/usr/bin")
            .output()
            .expect("running bastion-mc")
    }

    fn instance_dir(&self, id: &str) -> PathBuf {
        self.root.join("data/bastion-mc/instances").join(id)
    }

    fn config(&self, text: &str) {
        let dir = self.root.join("config/bastion-mc");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        std::fs::write(&file, text).unwrap();
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn a_bedrock_instance_is_created_listed_and_kept_apart_from_its_data() {
    let home = Home::new("new");
    let out = home.run(&["instance", "new", "phone", "--bedrock", "1.26.62.1"]);
    assert!(out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(said.contains("created Minecraft Bedrock instance phone for version 1.26.62.1"), "{said}");
    assert!(said.contains("not imported yet"), "{said}");
    let dir = home.instance_dir("phone");
    let toml = std::fs::read_to_string(dir.join("instance.toml")).unwrap();
    assert!(toml.contains("kind = \"bedrock\"") && toml.contains("version = \"1.26.62.1\""), "{toml}");
    assert!(dir.join("bedrock-data").is_dir(), "where the worlds will be kept");
    assert!(!dir.join("saves").exists() && !dir.join("mods").exists(), "none of the Java directories");

    let list = home.run(&["instance", "list"]);
    assert!(list.status.success(), "{}", text(&list));
    let listed = text(&list);
    assert!(listed.contains("phone") && listed.contains("bedrock"), "{listed}");
    assert!(!listed.contains("NOT LAUNCHABLE"), "{listed}");

    // In event mode the front end learns the kind.
    let events = home.run(&["--events", "instance", "list"]);
    let json = String::from_utf8_lossy(&events.stdout);
    assert!(json.contains("\"kind\":\"bedrock\""), "{json}");

    // The same name twice, a version id that is a path, both flags at once: refused.
    assert!(!home.run(&["instance", "new", "phone", "--bedrock", "1.26.62.1"]).status.success());
    let bad = home.run(&["instance", "new", "other", "--bedrock", "../escape"]);
    assert!(!bad.status.success() && text(&bad).contains("not a Bedrock version id"), "{}", text(&bad));
    let both = home.run(&["instance", "new", "other", "--bedrock", "1.26.62.1", "--minecraft", "1.21.4"]);
    assert!(!both.status.success() && text(&both).contains("exclude each other"), "{}", text(&both));
    assert!(!home.instance_dir("other").exists());
}

#[test]
fn every_java_only_command_refuses_a_bedrock_instance_with_a_sentence() {
    let home = Home::new("refuse");
    assert!(home.run(&["instance", "new", "phone", "--bedrock", "1.26.62.1"]).status.success());
    for args in [
        &["install", "phone"][..],
        &["profile", "verify", "phone"],
        &["profile", "lock", "phone"],
        &["profile", "audit", "phone"],
        &["profile", "check", "phone"],
        &["pack", "list", "phone"],
        &["pack", "adopt", "phone"],
    ] {
        let out = home.run(args);
        let said = text(&out);
        assert!(!out.status.success(), "{args:?} succeeded: {said}");
        assert!(
            said.contains("phone is a Bedrock instance") && said.contains("is for Java instances"),
            "{args:?}: {said}"
        );
        assert!(said.contains("bastion-mc launch phone"), "{args:?} says what to do instead: {said}");
    }
    // Nothing was created in the instance by those commands.
    let entries: Vec<String> = std::fs::read_dir(home.instance_dir("phone"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let mut entries = entries;
    entries.sort();
    assert_eq!(entries, ["bedrock-data", "instance.toml"]);
}

#[test]
fn a_bedrock_launch_is_gated_and_refuses_the_java_launch_options() {
    let home = Home::new("launch");
    assert!(home.run(&["instance", "new", "phone", "--bedrock", "1.26.62.1"]).status.success());
    // Without the user's acceptance of the licence substitution: refused, and the reason named.
    let gated = home.run(&["launch", "phone"]);
    assert!(!gated.status.success());
    assert!(text(&gated).contains("accept_licence_substitution"), "{}", text(&gated));
    // The Java launch options, each with what to do instead.
    for (flag, says) in [("--burner", "mode = \"none\""), ("--repair", "bedrock import")] {
        let out = home.run(&["launch", "phone", flag]);
        assert!(!out.status.success() && text(&out).contains(says), "{flag}: {}", text(&out));
    }
    let join = home.run(&["launch", "phone", "--join", "play.example.org"]);
    assert!(!join.status.success() && text(&join).contains("[bedrock] servers"), "{}", text(&join));
    // `--demo` asks for nothing a Bedrock launch does not do anyway: it gets as far as the gate.
    let demo = home.run(&["launch", "phone", "--demo"]);
    assert!(!demo.status.success() && text(&demo).contains("accept_licence_substitution"), "{}", text(&demo));
    // Accepted, the launch goes on to find the installed client (another package's part).
    home.config("[bedrock]\naccept_licence_substitution = true\n");
    let accepted = home.run(&["launch", "phone"]);
    assert!(!text(&accepted).contains("accept_licence_substitution = true` in config.toml"), "{}", text(&accepted));
}

#[test]
fn a_java_key_in_a_bedrock_instance_makes_it_not_launchable_with_the_reason() {
    let home = Home::new("keys");
    assert!(home.run(&["instance", "new", "phone", "--bedrock", "1.26.62.1"]).status.success());
    let toml = home.instance_dir("phone").join("instance.toml");
    let original = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, original.replace("kind = \"bedrock\"\n", "kind = \"bedrock\"\nmemory_mb = 4096\n")).unwrap();
    let list = home.run(&["instance", "list"]);
    let said = text(&list);
    assert!(said.contains("NOT LAUNCHABLE") && said.contains("Java heap"), "{said}");
    assert!(Path::new(&toml).exists());
}
