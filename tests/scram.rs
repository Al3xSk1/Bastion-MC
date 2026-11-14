//! `bastion-mc scram` end to end, against an isolated set of directories.
//!
//! The kernel keyring is not isolated by directories, so everything here that removes
//! keyring entries goes through `scram::run_for` with one uniquely named test account. The
//! command itself is only run in the form that changes nothing (confirmation refused).

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
};

use bastion_mc::{
    auth::{activity, canary, identity, minecraft::Profile, registry, store::TokenStore},
    paths::Paths,
    scram,
    secret::Secret,
};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

fn paths(tag: &str) -> (PathBuf, Paths) {
    let root = std::env::temp_dir().join(format!("bastion-scram-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = Paths {
        config: root.join("config/bastion-mc"),
        data: root.join("data/bastion-mc"),
        cache: root.join("cache/bastion-mc"),
        state: root.join("state/bastion-mc"),
        runtime: None,
    };
    for dir in [&paths.config, &paths.data, &paths.cache, &paths.state] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    (root, paths)
}

fn profile() -> Profile {
    Profile { id: "069a79f444e94726a5befca90e38aaf5".into(), name: "Alex".into(), skins: vec![], capes: vec![] }
}

#[test]
fn scram_removes_every_credential_and_keeps_the_evidence() {
    let (root, paths) = paths("run");
    let account = format!("scram-{}", std::process::id());
    let keyring_ok = TokenStore::Keyutils.load(&account).is_ok();
    if !keyring_ok && std::env::var_os("BASTION_ALLOW_NO_KEYRING").is_none() {
        panic!("kernel keyring unavailable");
    }

    // A signed-in account in the keyring, another one in an age vault, a default choice.
    if keyring_ok {
        TokenStore::Keyutils.save(&account, &Secret::from_slice(b"REFRESH-TOKEN-TO-REMOVE").unwrap()).unwrap();
    }
    let id = identity::Identity { player: "Alex".into(), uuid: "069a79f444e94726a5befca90e38aaf5".into() };
    identity::remember(&paths, &account, &id).unwrap();
    identity::remember(&paths, "vaulted", &id).unwrap();
    std::fs::create_dir_all(paths.vault()).unwrap();
    std::fs::write(paths.vault().join("vaulted.age"), b"age-encryption.org/v1 ...").unwrap();
    std::fs::write(paths.vault().join("left over.tmp"), b"partial").unwrap();
    registry::set_default(&paths, &account).unwrap();
    canary::accept(&paths, &account, &profile()).unwrap();
    activity::record(&paths, &account, "sign-in", "").unwrap();

    let report = scram::run_for(&paths, [account.clone()].into()).unwrap();
    let text = report.lines.join("\n");
    assert!(!report.incomplete, "{text}");

    if keyring_ok {
        assert!(text.contains(&format!("removed the refresh token of account {account} from the kernel keyring")));
        let mut gone = false;
        for _ in 0..50 {
            if TokenStore::Keyutils.load(&account).unwrap().is_none() {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone, "the token is still in the keyring");
    }
    assert!(std::fs::read_dir(paths.vault()).unwrap().next().is_none(), "the vault directory is empty: {text}");
    assert!(identity::recall(&paths, &account).is_none() && identity::recall(&paths, "vaulted").is_none());
    assert_eq!(registry::default_account(&paths), "default");
    assert_eq!(registry::list(&paths), ["default"], "no account is left");
    assert!(text.contains("no launch was in progress"));

    // Said plainly, every time.
    assert!(text.contains("revoked NOTHING at Microsoft"));
    assert!(text.contains("not securely wiped"));
    assert!(text.contains("bastion-mc wiki scram"));

    // Evidence is not a credential, and stays.
    assert!(canary::load(&paths, &account).unwrap().is_some());
    assert_eq!(activity::read(&paths, &account).len(), 1);

    // Running it again finds nothing and fails at nothing.
    let again = scram::run_for(&paths, [account].into()).unwrap();
    assert!(!again.incomplete);
    assert!(!again.lines.iter().any(|l| l.starts_with("removed") || l.starts_with("deleted")), "{:?}", again.lines);
    std::fs::remove_dir_all(root).unwrap();
}

fn launcher(root: &std::path::Path) -> Command {
    let mut command = Command::new(LAUNCHER);
    command
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_STATE_HOME", root.join("state"));
    command
}

#[test]
fn the_command_asks_first_and_a_wrong_answer_changes_nothing() {
    let (root, paths) = paths("ask");
    std::fs::create_dir_all(paths.vault()).unwrap();
    std::fs::write(paths.vault().join("default.age"), b"ciphertext").unwrap();

    for answer in ["", "yes\n", "scram\n", "SCRAM please\n"] {
        let mut child = launcher(&root)
            .arg("scram")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(answer.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{answer:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("cancelled; nothing was changed"), "{answer:?}");
        assert!(paths.vault().join("default.age").exists(), "{answer:?}: the vault was touched");
    }
    // A front end must have asked the user itself.
    let output = launcher(&root).args(["--events", "scram"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(paths.vault().join("default.age").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_runbook_and_the_proofs_are_available_without_a_network() {
    let (root, paths) = paths("docs");
    let out = launcher(&root).args(["wiki", "scram"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("# Emergency runbook") && text.contains("bastion-mc scram"));

    let out = launcher(&root).args(["wiki", "scram", "--links"]).output().unwrap();
    let links = String::from_utf8(out.stdout).unwrap();
    assert!(links.contains("https://account.live.com/proofs/manage/additional"));
    assert_eq!(links.lines().filter(|l| l.starts_with("https://")).count(), bastion_mc_proto::runbook::LINKS.len());

    canary::accept(&paths, "default", &profile()).unwrap();
    let file = root.join("proofs.txt");
    let out = launcher(&root).arg("export-proofs").arg(&file).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("Player id (UUID): 069a79f444e94726a5befca90e38aaf5"));
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    // Never over an existing file.
    let out = launcher(&root).arg("export-proofs").arg(&file).output().unwrap();
    assert!(!out.status.success());
    std::fs::remove_dir_all(root).unwrap();
}
