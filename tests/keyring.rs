//! The refresh token across login sessions.
//!
//! `auth::store` says about the default store: RAM only, and still there when you log in
//! again (until the kernel's persistent-keyring expiry). The second half depends on the key
//! being linked into the *persistent* keyring of the user, not only into the session
//! keyring of the process that stored it. This test stores a token in one session keyring
//! and looks for it from another, the way a logout and a new login would.
//!
//! Each "login session" is this test binary run again as a child that first joins a new,
//! empty session keyring (`keyctl(KEYCTL_JOIN_SESSION_KEYRING, NULL)`).

use std::process::Command;

use bastion_mc::{auth::store::TokenStore, secret::Secret};

const ROLE: &str = "BASTION_KEYRING_ROLE";
const ACCOUNT: &str = "BASTION_KEYRING_ACCOUNT";
const TOKEN: &[u8] = b"M.C5_BL2.persistent-keyring-test-token";

fn join_new_session_keyring() {
    const KEYCTL_JOIN_SESSION_KEYRING: libc::c_long = 1;
    // SAFETY: plain syscall; a null name asks for a new anonymous session keyring.
    let id = unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_JOIN_SESSION_KEYRING, std::ptr::null::<libc::c_char>()) };
    assert!(id > 0, "joining a new session keyring: {}", std::io::Error::last_os_error());
}

/// Not a test of its own: the body of the child processes.
#[test]
fn child() {
    let Ok(role) = std::env::var(ROLE) else { return };
    let account = std::env::var(ACCOUNT).expect("account");
    join_new_session_keyring();
    let store = TokenStore::Keyutils;
    match role.as_str() {
        "save" => store.save(&account, &Secret::from_slice(TOKEN).unwrap()).expect("save"),
        "load" => {
            let found = store.load(&account).expect("load");
            println!("FOUND={}", found.is_some_and(|t| t.expose() == TOKEN));
        }
        "delete" => store.delete(&account).expect("delete"),
        other => panic!("unknown role {other}"),
    }
}

fn session(role: &str, account: &str) -> String {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(ACCOUNT, account)
        .output()
        .expect("running the child");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{role}: {text}\n{}", String::from_utf8_lossy(&output.stderr));
    text
}

#[test]
fn a_stored_token_is_found_from_a_later_login_session() {
    if TokenStore::Keyutils.load("probe").is_err() {
        if std::env::var_os("BASTION_ALLOW_NO_KEYRING").is_some() {
            eprintln!("skipped: kernel keyring unavailable in this environment");
            return;
        }
        panic!("kernel keyring unavailable");
    }
    if !std::path::Path::new("/proc/sys/kernel/keys/persistent_keyring_expiry").exists() {
        eprintln!("skipped: this kernel has no persistent keyrings (CONFIG_PERSISTENT_KEYRINGS)");
        return;
    }
    let account = format!("persist-{}", std::process::id());
    assert!(session("load", &account).contains("FOUND=false"), "nothing stored yet");
    session("save", &account);
    // A different session keyring: the token is reachable only through the persistent one.
    assert!(
        session("load", &account).contains("FOUND=true"),
        "a token stored in one login session must be found in the next (persistent keyring)"
    );
    session("delete", &account);
    // Invalidation is asynchronous in the kernel.
    let mut gone = false;
    for _ in 0..50 {
        if session("load", &account).contains("FOUND=false") {
            gone = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    assert!(gone, "signing out in one session removes the token for every later one");
}
