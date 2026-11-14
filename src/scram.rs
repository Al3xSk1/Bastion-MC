//! `bastion-mc scram`: remove every credential this launcher holds on this machine, now.
//!
//! For the moment a user suspects that the account or the machine is compromised. It only
//! ever removes things, so it asks for a confirmation but for no password, and it never
//! needs the network:
//!
//! 1. every launch in progress is killed (the session token only exists in that launcher's
//!    memory; the game and its display server die with their launcher);
//! 2. every stored refresh token is removed — from the kernel keyring *and* from the age
//!    vault, whichever store is configured now, because the configuration may have changed
//!    since a token was stored;
//! 3. the remembered identities and the default-account choice are removed.
//!
//! What it deliberately keeps: the profile canary and the activity log. They hold no
//! credential, and they are the evidence `export-proofs` hands to Mojang's support.
//!
//! What it cannot do is said in its own output (and in `docs/SCRAM_RUNBOOK.md`): nothing is
//! revoked at Microsoft, and deleting a file is not a secure wipe.

use std::collections::BTreeSet;

use crate::{
    auth::{identity, registry, store::TokenStore},
    error::Result,
    paths::{Paths, validate_id},
    running,
};

/// What was done, as lines for the user.
#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<String>,
    /// Something could not be removed; the lines say what.
    pub incomplete: bool,
}

const KEY_PREFIX: &str = "bastion-mc:msa-refresh:";

/// Account labels with a refresh token in a keyring this process can see, read from
/// `/proc/keys` (the kernel's own list). This finds tokens of accounts that nothing else on
/// disk remembers.
fn keyring_accounts(proc_keys: &str) -> BTreeSet<String> {
    proc_keys
        .lines()
        .filter_map(|line| {
            let rest = &line[line.find(KEY_PREFIX)? + KEY_PREFIX.len()..];
            let label: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != ':').collect();
            validate_id(&label).is_ok().then_some(label)
        })
        .collect()
}

pub fn run(paths: &Paths) -> Result<Report> {
    run_for(paths, keyring_accounts(&std::fs::read_to_string("/proc/keys").unwrap_or_default()))
}

/// [`run`] with the keyring accounts given instead of discovered. The kernel keyring is
/// shared by everything running as the user, whatever directories a caller points the
/// launcher at — so the tests name the one account they created, and can never remove a
/// real sign-in.
pub fn run_for(paths: &Paths, keyring: BTreeSet<String>) -> Result<Report> {
    let mut report = Report::default();
    let mut say = |line: String| report.lines.push(line);

    // 1. Running launches.
    let killed = running::kill_all(paths);
    if killed.is_empty() {
        say("no launch was in progress".into());
    }
    for launch in &killed {
        say(format!(
            "killed the running launcher of instance {} (pid {}): its session token is gone with it",
            launch.instance, launch.pid
        ));
    }

    if !killed.is_empty() {
        // The kernel delivers SIGKILL at once; give the process trees a moment to be gone.
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let swept = running::sweep_leftovers(paths, std::time::Duration::ZERO);
    if swept > 0 {
        say(format!("deleted {swept} leftover game director(ies) of launches that were killed or crashed"));
    }

    // 2. Refresh tokens, in both kinds of store.
    let mut accounts: BTreeSet<String> = registry::list(paths).into_iter().collect();
    accounts.extend(keyring);
    let mut incomplete = false;
    for account in &accounts {
        match TokenStore::Keyutils.has(account) {
            Ok(true) => match TokenStore::Keyutils.delete(account) {
                Ok(()) => say(format!("removed the refresh token of account {account} from the kernel keyring")),
                Err(e) => {
                    incomplete = true;
                    say(format!("COULD NOT remove the refresh token of account {account} from the keyring: {e}"));
                }
            },
            Ok(false) => {}
            Err(e) => {
                incomplete = true;
                say(format!("COULD NOT look for account {account} in the kernel keyring: {e}"));
            }
        }
    }
    // Every file in the vault directory, whatever it is called.
    if let Ok(entries) = std::fs::read_dir(paths.vault()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => say(format!("deleted the vault file {}", path.display())),
                Err(e) => {
                    incomplete = true;
                    say(format!("COULD NOT delete the vault file {}: {e}", path.display()));
                }
            }
        }
    }

    // 3. What the launcher remembered about who is signed in.
    for account in &accounts {
        if identity::recall(paths, account).is_some() || paths.identities().join(format!("{account}.json")).exists() {
            match identity::forget(paths, account) {
                Ok(()) => say(format!("forgot the remembered player of account {account}")),
                Err(e) => {
                    incomplete = true;
                    say(format!("COULD NOT remove the remembered player of account {account}: {e}"));
                }
            }
        }
    }
    let _ = std::fs::remove_file(paths.default_account_file());

    say(String::new());
    say("This removed the copies on this machine. It revoked NOTHING at Microsoft or Mojang:".into());
    say("  - a refresh token that was copied earlier works until you \"sign out everywhere\";".into());
    say("  - a Minecraft session token that was already issued is valid for up to about 24 hours.".into());
    say("Deleted files are not securely wiped (SSDs and copy-on-write file systems keep old blocks);".into());
    say("an age vault file was encrypted, and its token is worthless once you sign out everywhere.".into());
    say("Kept on purpose: the profile canary history and the activity log (no credentials; evidence).".into());
    say("Next: `bastion-mc wiki scram` — the recovery steps, with verified links. If this machine may be".into());
    say("compromised, do those steps from another device.".into());
    report.incomplete = incomplete;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_accounts_are_read_from_the_kernels_list() {
        let keys = "\
0a1b2c3d I--Q---     1 perm 3f010000  1000  1000 user      bastion-mc:msa-refresh:default: 1432
12345678 I--Q---     3 perm 3f030000  1000  1000 keyring   _ses: 2
2b3c4d5e I--Q---     1 perm 3f010000  1000  1000 user      bastion-mc:msa-refresh:work-2: 1401
3c4d5e6f I--Q---     1 perm 3f010000  1000  1000 user      bastion-mc:msa-refresh:../etc: 10
4d5e6f70 I--Q---     1 perm 3f010000  1000  1000 user      other-app:msa-refresh:nope: 10
5e6f7081 I--Q---     1 perm 3f010000  1000  1000 user      bastion-mc:msa-refresh:: 10
";
        let found: Vec<String> = keyring_accounts(keys).into_iter().collect();
        assert_eq!(found, ["default", "work-2"]);
        assert!(keyring_accounts("").is_empty());
    }
}
