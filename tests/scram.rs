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
    auth::{
        activity, canary, identity,
        minecraft::Profile,
        registry,
        store::{self, TokenStore},
    },
    paths::Paths,
    running, scram,
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
    // Bedrock was never set up here: nothing is said about it.
    assert!(!text.contains("Bedrock"), "{text}");

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

// ---------------------------------------------------------------- other processes

const ROLE: &str = "BASTION_SCRAM_ROLE";
const ROOT: &str = "BASTION_SCRAM_ROOT";
const ACCOUNT: &str = "BASTION_SCRAM_ACCOUNT";

fn paths_in(root: &std::path::Path) -> Paths {
    Paths {
        config: root.join("config/bastion-mc"),
        data: root.join("data/bastion-mc"),
        cache: root.join("cache/bastion-mc"),
        state: root.join("state/bastion-mc"),
        runtime: None,
    }
}

fn join_new_session_keyring() {
    const KEYCTL_JOIN_SESSION_KEYRING: libc::c_long = 1;
    // SAFETY: plain syscall; a null name asks for a new anonymous session keyring.
    let id = unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_JOIN_SESSION_KEYRING, std::ptr::null::<libc::c_char>()) };
    assert!(id > 0, "joining a new session keyring: {}", std::io::Error::last_os_error());
}

fn wait_for_a_line() {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// Not a test of its own: the body of the processes the tests below start. Each stands for
/// a launcher command — or a login session — other than the one that runs scram.
#[test]
fn other_process() {
    let Ok(role) = std::env::var(ROLE) else { return };
    let paths = paths_in(std::path::Path::new(&std::env::var_os(ROOT).unwrap()));
    let account = std::env::var(ACCOUNT).unwrap_or_default();
    match role.as_str() {
        // A launch in progress: marker, and a game root it holds, as `launch` does.
        "launch" => {
            let _marker = running::Marker::create(&paths, "vanilla").unwrap();
            let root = paths.scratch().join("vanilla-00112233aabbccdd");
            std::fs::create_dir_all(root.join("saves/world")).unwrap();
            std::fs::write(root.join("saves/world/level.dat"), b"world").unwrap();
            let _lock = running::lock_dir(&root).unwrap();
            println!("\nready");
            std::thread::sleep(std::time::Duration::from_secs(120));
        }
        // A sign-in waiting for the user: it would store a token when it completes.
        "login" => {
            let _marker = running::Marker::for_command(&paths, "login").unwrap();
            println!("\nready");
            wait_for_a_line();
            TokenStore::Keyutils.save(&account, &Secret::from_slice(b"STORED-AFTER-SCRAM").unwrap()).unwrap();
            println!("stored");
        }
        // An earlier login session that signed in and is still open.
        "session-a" => {
            join_new_session_keyring();
            TokenStore::Keyutils.save(&account, &Secret::from_slice(b"TOKEN-OF-SESSION-A").unwrap()).unwrap();
            println!("\nready");
            wait_for_a_line();
            let still =
                TokenStore::Keyutils.load(&account).unwrap().is_some_and(|t| t.expose() == b"TOKEN-OF-SESSION-A");
            println!("still_readable={still}");
            // Clean up what this test created.
            while store::keyutils_remove(&account).unwrap() {}
        }
        // A process with a second thread, whose id it leaves in a file for the test.
        "threaded" => {
            let (tx, rx) = std::sync::mpsc::channel();
            let _thread = std::thread::spawn(move || {
                // SAFETY: gettid takes no arguments and cannot fail.
                tx.send(unsafe { libc::syscall(libc::SYS_gettid) }).unwrap();
                std::thread::sleep(std::time::Duration::from_secs(120));
            });
            let tid = rx.recv().unwrap();
            let root = std::path::PathBuf::from(std::env::var_os(ROOT).unwrap());
            std::fs::write(root.join("thread-id"), tid.to_string()).unwrap();
            println!("\nready");
            wait_for_a_line();
        }
        // A Bedrock session's data directory, held by a process with a second thread whose id it
        // leaves for the test (which marks the thread: a marker that cannot be stopped).
        "bedrock-threaded" => {
            let _session = bastion_mc::bedrock::policy::SessionDir::create(&paths, "bedrock-main").unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let _thread = std::thread::spawn(move || {
                // SAFETY: gettid takes no arguments and cannot fail.
                tx.send(unsafe { libc::syscall(libc::SYS_gettid) }).unwrap();
                std::thread::sleep(std::time::Duration::from_secs(120));
            });
            let tid = rx.recv().unwrap();
            let root = std::path::PathBuf::from(std::env::var_os(ROOT).unwrap());
            std::fs::write(root.join("thread-id"), tid.to_string()).unwrap();
            println!("\nready");
            wait_for_a_line();
        }
        // A Bedrock session in progress: its marker, and its data directory (locked), into which
        // the game wrote its sign-in.
        "bedrock" | "bedrock-unmarked" => {
            let _marker = (role == "bedrock").then(|| running::Marker::bedrock(&paths, "bedrock-main").unwrap());
            let session = bastion_mc::bedrock::policy::SessionDir::create(&paths, "bedrock-main").unwrap();
            std::fs::write(session.path().join("xal_tokens.json"), b"{\"refresh_token\":\"M.C5_BAY.STAND-IN\"}")
                .unwrap();
            std::fs::create_dir_all(session.path().join("games/com.mojang")).unwrap();
            println!("\nready");
            std::thread::sleep(std::time::Duration::from_secs(120));
        }
        // A second login session: signs in again, then runs scram.
        "session-b" => {
            join_new_session_keyring();
            TokenStore::Keyutils.save(&account, &Secret::from_slice(b"TOKEN-OF-SESSION-B").unwrap()).unwrap();
            let report = scram::run_for(&paths, [account.clone()].into()).unwrap();
            println!("\nincomplete={}", report.incomplete);
            for line in report.lines {
                println!("line={line}");
            }
            let own = TokenStore::Keyutils.load(&account).unwrap().is_some();
            println!("own_token_left={own}");
        }
        other => panic!("unknown role {other}"),
    }
}

fn other(role: &str, root: &std::path::Path, account: &str) -> std::process::Child {
    use std::io::BufRead;
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "other_process", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(ROOT, root)
        .env(ACCOUNT, account)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if role != "session-b" {
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while line.trim() != "ready" {
            line.clear();
            assert!(out.read_line(&mut line).unwrap() > 0, "{role} ended before it was ready");
        }
        child.stdout = Some(out.into_inner());
    }
    child
}

fn signal_of(child: &mut std::process::Child) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(&child.wait().unwrap())
}

/// Scram stops what could store a credential first, removes the credentials next, and only
/// then deletes what the games left behind — and says each of these when it is confirmed.
#[test]
fn writers_are_stopped_first_credentials_go_next_and_game_directories_last() {
    let (root, paths) = paths("order");
    let account = format!("scram-order-{}", std::process::id());
    let mut launch = other("launch", &root, &account);
    let mut login = other("login", &root, &account);
    assert_eq!(running::list(&paths).len(), 2, "both commands are marked");
    std::fs::create_dir_all(paths.vault()).unwrap();
    std::fs::write(paths.vault().join("default.age"), b"age-encryption.org/v1 ...").unwrap();
    // What a crashed launch left behind earlier, including a directory without any permission bits.
    let stale = paths.scratch().join("vanilla-ffeeddccbbaa9988");
    std::fs::create_dir_all(stale.join("locked/inner")).unwrap();
    std::fs::write(stale.join("locked/inner/file"), b"x").unwrap();
    std::fs::set_permissions(stale.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();

    let report = scram::run_for(&paths, [account.clone()].into()).unwrap();
    let text = report.lines.join("\n");
    assert!(!report.incomplete, "{text}");
    assert_eq!(signal_of(&mut launch), Some(libc::SIGKILL), "the launch was killed");
    assert_eq!(signal_of(&mut login), Some(libc::SIGKILL), "the pending sign-in was killed");
    // The sign-in never got to store its token.
    assert!(TokenStore::Keyutils.load(&account).unwrap().is_none(), "a token was stored after scram");
    assert!(running::list(&paths).is_empty());

    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("no {needle:?} in:\n{text}"));
    let killed_launch = at("killed the launcher of instance vanilla");
    let killed_login = at("killed `bastion-mc login`");
    assert!(text.contains("before it could store a sign-in"));
    let vault = at("deleted the vault file \"default.age\"");
    let swept = at("deleted 2 leftover game director(ies)");
    assert!(killed_launch < vault && killed_login < vault, "writers are stopped before credentials are removed");
    assert!(vault < swept, "game directories are deleted last:\n{text}");
    assert!(!stale.exists() && !paths.scratch().join("vanilla-00112233aabbccdd").exists());
    std::fs::remove_dir_all(root).unwrap();
}

/// A launch whose marker is gone — deleted by another program, or by a launcher that was
/// compromised and can write the cache — is not found by its marker. It still holds its game
/// directory locked, and scram says that a launch is running that it did not stop, instead
/// of "no launch was in progress" and a clean exit.
#[test]
fn a_launch_without_a_marker_is_reported_by_the_directory_it_holds() {
    let (root, paths) = paths("unmarked");
    let account = format!("scram-unmarked-{}", std::process::id());
    let mut launch = other("launch", &root, &account);
    // The marker disappears; the process and its lock do not.
    let markers = paths.cache.join("running");
    for entry in std::fs::read_dir(&markers).unwrap().flatten() {
        std::fs::remove_file(entry.path()).unwrap();
    }
    assert!(running::list(&paths).is_empty());

    let report = scram::run_for(&paths, [account].into()).unwrap();
    let text = report.lines.join("\n");
    assert!(report.incomplete, "{text}");
    assert!(
        text.contains("COULD NOT stop a launch that left no marker: the directory vanilla-00112233aabbccdd"),
        "{text}"
    );
    assert!(!text.contains("no launch was in progress"), "not next to a launch that runs:\n{text}");
    assert!(
        paths.scratch().join("vanilla-00112233aabbccdd/saves/world/level.dat").exists(),
        "its files are not deleted"
    );
    launch.kill().unwrap();
    let _ = launch.wait();
    // Once that process is gone, a second scram is complete and removes what it left.
    let report = scram::run_for(&paths, [format!("scram-unmarked-{}", std::process::id())].into()).unwrap();
    assert!(!report.incomplete, "{}", report.lines.join("\n"));
    assert!(!paths.scratch().join("vanilla-00112233aabbccdd").exists());
    std::fs::remove_dir_all(root).unwrap();
}

/// A command that cannot be stopped is reported as that, once — and not, when scram looks a
/// second time, as one that "started while this was running": it was there all along.
#[test]
fn a_command_that_cannot_be_stopped_is_reported_as_that_and_as_nothing_else() {
    let (root, paths) = paths("unstoppable");
    let account = format!("scram-unstoppable-{}", std::process::id());
    let mut process = other("threaded", &root, &account);
    let tid: u32 = std::fs::read_to_string(root.join("thread-id")).unwrap().parse().unwrap();
    // A marker that names the thread, with the thread's own start time and name. It passes
    // as a live command, and stopping it fails before any signal is sent: a pidfd is only
    // had for a process, not for one of its threads. (How a real command fails to stop —
    // stuck in the kernel for longer than scram waits — cannot be arranged in a test.)
    let stat = std::fs::read_to_string(format!("/proc/{tid}/stat")).unwrap();
    let close = stat.rfind(')').unwrap();
    let comm = &stat[stat.find('(').unwrap() + 1..close];
    let start = stat[close + 1..].split_ascii_whitespace().nth(19).unwrap();
    let markers = paths.cache.join("running");
    std::fs::create_dir_all(&markers).unwrap();
    std::fs::write(markers.join(tid.to_string()), format!("{start}\ncommand:login\n{comm}\n")).unwrap();
    assert_eq!(running::list(&paths).len(), 1, "the marker passes as a live command");

    let report = scram::run_for(&paths, [account].into()).unwrap();
    let text = report.lines.join("\n");
    process.stdin.take().unwrap().write_all(b"go\n").unwrap();
    assert!(process.wait().unwrap().success(), "the process was never signalled");
    assert!(report.incomplete, "{text}");
    let about: Vec<&String> = report.lines.iter().filter(|l| l.contains(&format!("(pid {tid})"))).collect();
    assert_eq!(about.len(), 1, "one line about it:\n{text}");
    assert!(about[0].starts_with(&format!("COULD NOT stop `bastion-mc login` (pid {tid})")), "{text}");
    assert!(!text.contains("started while this was running"), "it did not:\n{text}");
    std::fs::remove_dir_all(root).unwrap();
}

/// When the markers of running commands cannot be read, scram does not know that nothing is
/// running — and says that, instead of "no launch was in progress".
#[test]
fn markers_that_cannot_be_read_are_not_taken_for_nothing_running() {
    let (root, paths) = paths("unlisted");
    let markers = paths.cache.join("running");
    std::fs::create_dir_all(&markers).unwrap();
    std::fs::set_permissions(&markers, std::fs::Permissions::from_mode(0o000)).unwrap();
    let report = scram::run_for(&paths, [format!("scram-unlisted-{}", std::process::id())].into()).unwrap();
    std::fs::set_permissions(&markers, std::fs::Permissions::from_mode(0o700)).unwrap();
    let text = report.lines.join("\n");
    assert!(report.incomplete, "{text}");
    assert!(text.contains("COULD NOT find out which launcher commands are running"), "{text}");
    assert!(!text.contains("no launch was in progress"), "that is not known:\n{text}");
    std::fs::remove_dir_all(root).unwrap();
}

/// A game directory that cannot be deleted does not stand between the user and the removal
/// of the credentials — and it is not passed over in silence either.
#[test]
fn what_cannot_be_deleted_is_reported_after_the_credentials_are_gone() {
    let (root, paths) = paths("stuck");
    std::fs::create_dir_all(paths.vault()).unwrap();
    std::fs::write(paths.vault().join("default.age"), b"age-encryption.org/v1 ...").unwrap();
    let stale = paths.scratch().join("vanilla-0f0f0f0f0f0f0f0f");
    std::fs::create_dir_all(stale.join("saves")).unwrap();
    // Not removable: the directory that holds it does not allow it.
    std::fs::set_permissions(paths.scratch(), std::fs::Permissions::from_mode(0o500)).unwrap();

    let report = scram::run_for(&paths, [format!("scram-stuck-{}", std::process::id())].into()).unwrap();
    std::fs::set_permissions(paths.scratch(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let text = report.lines.join("\n");
    assert!(report.incomplete, "{text}");
    assert!(!paths.vault().join("default.age").exists(), "the credential is gone regardless");
    let deleted = text.find("deleted the vault file").expect("vault line");
    let could_not = text.find("COULD NOT delete a leftover game directory").unwrap_or_else(|| panic!("{text}"));
    assert!(deleted < could_not);
    assert!(text.contains("vanilla-0f0f0f0f0f0f0f0f"), "{text}");
    assert!(!text.contains("deleted 1 leftover"), "nothing is reported as deleted that was not: {text}");
    // Every line about something left undone starts the same way: front ends rely on it.
    for line in report.lines.iter().filter(|l| l.contains("COULD NOT")) {
        assert!(line.starts_with("COULD NOT "), "{line}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

/// Two login sessions of the same user. The second signs in again (which displaces the
/// first session's key in the persistent keyring) and runs scram: the first session's key
/// is still there, still readable in that session — and scram, which cannot reach it, must
/// say so instead of reporting success.
#[test]
fn a_token_that_only_another_login_session_holds_is_reported_not_passed_over() {
    // (A name of this test's own: the keyring that is asked is the user's real one.)
    if TokenStore::Keyutils.load(&format!("scram-probe-{}", std::process::id())).is_err() {
        if std::env::var_os("BASTION_ALLOW_NO_KEYRING").is_some() {
            eprintln!("skipped: kernel keyring unavailable in this environment");
            return;
        }
        panic!("kernel keyring unavailable");
    }
    let (root, _paths) = paths("sessions");
    let account = format!("scram-sessions-{}", std::process::id());
    let mut first = other("session-a", &root, &account);
    let second = other("session-b", &root, &account).wait_with_output().unwrap();
    let second_out = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(second.status.success(), "{second_out}\n{}", String::from_utf8_lossy(&second.stderr));

    // Let the first session look, then clean up after itself.
    first.stdin.take().unwrap().write_all(b"go\n").unwrap();
    let first = first.wait_with_output().unwrap();
    let first_out = String::from_utf8_lossy(&first.stdout).into_owned();
    assert!(first.status.success(), "{first_out}\n{}", String::from_utf8_lossy(&first.stderr));

    assert!(
        second_out.contains("own_token_left=false"),
        "scram removed what its own session could reach:\n{second_out}"
    );
    if first_out.contains("still_readable=true") {
        // The situation the finding describes exists on this kernel: then it must be reported.
        assert!(second_out.contains("incomplete=true"), "{second_out}");
        assert!(
            second_out.contains(&format!(
                "line=COULD NOT remove the refresh token of account {account}: the kernel still lists a key"
            )),
            "{second_out}"
        );
    } else {
        // (A kernel where the second session's key replaced the first everywhere.)
        eprintln!("note: the first session's token did not survive; nothing to report:\n{first_out}");
        assert!(second_out.contains("incomplete=false"), "{second_out}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

// ---------------------------------------------------------------- Bedrock

/// A world of a Bedrock instance, where the launch keeps them (a stand-in path: the layout is
/// the launch package's). Scram must never touch it.
fn bedrock_world(paths: &Paths) -> PathBuf {
    let world = paths.instances().join("bedrock-main/data/games/com.mojang/minecraftWorlds/w1");
    std::fs::create_dir_all(&world).unwrap();
    std::fs::write(world.join("level.dat"), b"a world").unwrap();
    world.join("level.dat")
}

/// A running Bedrock session is stopped like a Java launch, and what its game kept of the
/// Xbox sign-in on disk goes with the credentials — after the launcher's own, before what Java
/// games left — while Bedrock worlds stay, and the output says both.
#[test]
fn a_bedrock_session_is_stopped_and_its_sign_in_on_disk_goes_with_the_credentials() {
    let (root, paths) = paths("bedrock");
    let account = format!("scram-bedrock-{}", std::process::id());
    let world = bedrock_world(&paths);
    std::fs::create_dir_all(paths.vault()).unwrap();
    std::fs::write(paths.vault().join("default.age"), b"age-encryption.org/v1 ...").unwrap();
    std::fs::create_dir_all(paths.scratch().join("vanilla-0011001100110011/saves")).unwrap();
    let mut session = other("bedrock", &root, &account);
    let sessions = bastion_mc::bedrock::policy::sessions_dir(&paths);
    assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 1);
    assert!(matches!(running::list(&paths)[0].activity, running::Activity::Bedrock(_)));

    let report = scram::run_for(&paths, [account].into()).unwrap();
    let text = report.lines.join("\n");
    assert!(!report.incomplete, "{text}");
    assert_eq!(signal_of(&mut session), Some(libc::SIGKILL), "the session was killed");
    assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 0, "its sign-in is still on disk:\n{text}");
    assert_eq!(std::fs::read(&world).unwrap(), b"a world", "a Bedrock world was touched");

    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("no {needle:?} in:\n{text}"));
    let killed = at("killed the Bedrock session of instance bedrock-main");
    let vault = at("deleted the vault file \"default.age\"");
    let bedrock = at("deleted what 1 Bedrock session(s) left on disk (bedrock-main-");
    let swept = at("deleted 1 leftover game director(ies)");
    assert!(killed < vault && vault < bedrock && bedrock < swept, "order:\n{text}");
    assert!(text.contains("a Bedrock game holds your Xbox sign-in itself while it runs"), "{text}");
    assert!(text.contains("Also kept: Bedrock worlds and the game's own settings"), "{text}");
    assert!(!text.contains("COULD NOT"), "{text}");
    std::fs::remove_dir_all(root).unwrap();
}

/// What a crashed session left is removed. A session that runs without a marker — it was
/// deleted, or the launcher was compromised — is not found by its marker; its directory is
/// still locked, so it is neither deleted under it nor passed over: the result is incomplete
/// and says to end it and to sign out everywhere. Once it is gone, scram completes.
#[test]
fn a_crashed_sessions_sign_in_is_removed_and_a_running_session_without_marker_is_reported() {
    let (root, paths) = paths("bedrock-left");
    let account = format!("scram-bedrock-left-{}", std::process::id());
    let sessions = bastion_mc::bedrock::policy::sessions_dir(&paths);
    let crashed = sessions.join("bedrock-main-0123456789abcdef");
    std::fs::create_dir_all(crashed.join("games")).unwrap();
    std::fs::write(crashed.join("xal_tokens.json"), b"{\"refresh_token\":\"left behind\"}").unwrap();
    let mut session = other("bedrock-unmarked", &root, &account);
    assert!(running::list(&paths).is_empty(), "no marker anywhere");

    let report = scram::run_for(&paths, [account.clone()].into()).unwrap();
    let text = report.lines.join("\n");
    assert!(report.incomplete, "{text}");
    assert!(!crashed.exists(), "the crashed session's sign-in is still there:\n{text}");
    assert!(text.contains("deleted what 1 Bedrock session(s) left on disk (bedrock-main-0123456789abcdef)"), "{text}");
    let about: Vec<&String> = report.lines.iter().filter(|l| l.contains("a running Bedrock session")).collect();
    assert_eq!(about.len(), 1, "one line about it:\n{text}");
    assert!(about[0].starts_with("COULD NOT delete the Bedrock session directory bedrock-main-"), "{text}");
    assert!(about[0].contains("one that left no marker") && about[0].contains("sign out everywhere"), "{text}");
    assert!(!text.contains("no launch was in progress"), "not next to a session that runs:\n{text}");
    assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 1, "nothing is deleted under a running session");

    session.kill().unwrap();
    let _ = session.wait();
    let report = scram::run_for(&paths, [account].into()).unwrap();
    assert!(!report.incomplete, "{}", report.lines.join("\n"));
    assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

/// A Bedrock session that cannot be stopped is reported with what makes it different from a
/// Java launch: its game holds the Xbox sign-in, so the line says to sign out everywhere; and
/// its directory is not deleted under it, said once, as belonging to that session.
#[test]
fn a_bedrock_session_that_cannot_be_stopped_says_to_sign_out_everywhere() {
    let (root, paths) = paths("bedrock-unstoppable");
    let account = format!("scram-bedrock-unstoppable-{}", std::process::id());
    let mut process = other("bedrock-threaded", &root, &account);
    let tid: u32 = std::fs::read_to_string(root.join("thread-id")).unwrap().parse().unwrap();
    // (See `a_command_that_cannot_be_stopped_is_reported_as_that_and_as_nothing_else`: a marker
    // naming a thread passes as live, and no pidfd can be had for it.)
    let stat = std::fs::read_to_string(format!("/proc/{tid}/stat")).unwrap();
    let close = stat.rfind(')').unwrap();
    let comm = &stat[stat.find('(').unwrap() + 1..close];
    let start = stat[close + 1..].split_ascii_whitespace().nth(19).unwrap();
    let markers = paths.cache.join("running");
    std::fs::create_dir_all(&markers).unwrap();
    std::fs::write(markers.join(tid.to_string()), format!("{start}\nbedrock:bedrock-main\n{comm}\n")).unwrap();

    let report = scram::run_for(&paths, [account].into()).unwrap();
    let text = report.lines.join("\n");
    process.stdin.take().unwrap().write_all(b"go\n").unwrap();
    assert!(process.wait().unwrap().success(), "the process was never signalled");
    assert!(report.incomplete, "{text}");
    let stop: Vec<&String> = report.lines.iter().filter(|l| l.contains(&format!("(pid {tid})"))).collect();
    assert_eq!(stop.len(), 1, "one line about stopping it:\n{text}");
    assert!(stop[0].starts_with(&format!("COULD NOT stop the Bedrock session of instance bedrock-main (pid {tid})")));
    assert!(stop[0].contains("sign out everywhere"), "{text}");
    let dir: Vec<&String> =
        report.lines.iter().filter(|l| l.contains("COULD NOT delete the Bedrock session")).collect();
    assert_eq!(dir.len(), 1, "one line about its directory:\n{text}");
    assert!(dir[0].contains("one this run could not stop"), "{text}");
    assert!(!text.contains("started while this was running"), "{text}");
    std::fs::remove_dir_all(root).unwrap();
}

/// A directory of Bedrock sessions that cannot be read is not taken for an empty one.
#[test]
fn an_unreadable_directory_of_bedrock_sessions_is_not_taken_for_an_empty_one() {
    let (root, paths) = paths("bedrock-unreadable");
    let sessions = bastion_mc::bedrock::policy::sessions_dir(&paths);
    std::fs::create_dir_all(sessions.join("bedrock-main-0011223344556677")).unwrap();
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o000)).unwrap();
    let report = scram::run_for(&paths, [format!("scram-bedrock-unreadable-{}", std::process::id())].into()).unwrap();
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
    let text = report.lines.join("\n");
    assert!(report.incomplete, "{text}");
    assert!(text.contains("COULD NOT read the directory of Bedrock sessions"), "{text}");
    assert!(text.contains("was NOT deleted"), "{text}");
    std::fs::remove_dir_all(root).unwrap();
}

/// The in-game browser's profile holds site sessions (a signed-in Discord): scram deletes it,
/// with the record of sign-ins — but not while a browser still uses it (a `browser login`
/// window holds the profile's lock): then the run is incomplete and says so.
#[test]
fn scram_deletes_the_browsers_profile_unless_a_browser_holds_it() {
    let (root, paths) = paths("browser");
    let account = format!("scram-browser-{}", std::process::id());
    let profile = bastion_mc::browser::sandbox::profile_dir(&paths);
    std::fs::create_dir_all(profile.join("storage")).unwrap();
    std::fs::write(profile.join("cookies.sqlite"), b"a session").unwrap();
    std::fs::write(profile.join("storage").join("ls.sqlite"), b"more").unwrap();
    std::fs::write(profile.with_file_name("logins.json"), b"{}").unwrap();

    // Held: kept, and said.
    let lock = bastion_mc::browser::sandbox::ProfileLock::take(&paths).unwrap();
    let report = scram::run_for(&paths, [account.clone()].into()).unwrap();
    let text = report.lines.join("\n");
    assert!(report.incomplete, "{text}");
    assert!(text.contains("COULD NOT delete the in-game browser's profile"), "{text}");
    assert!(profile.join("cookies.sqlite").exists(), "a profile in use is not deleted under the browser");
    drop(lock);

    // Free: deleted, with the record of sign-ins.
    let report = scram::run_for(&paths, [account].into()).unwrap();
    let text = report.lines.join("\n");
    assert!(text.contains("deleted the in-game browser's profile"), "{text}");
    assert!(!profile.exists() && !profile.with_file_name("logins.json").exists(), "{text}");
    let _ = std::fs::remove_dir_all(root);
}
