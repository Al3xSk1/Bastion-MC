//! `bastion-mc scram`: remove every credential this launcher holds on this machine, now.
//!
//! For the moment a user suspects that the account or the machine is compromised. It only
//! ever removes things, so it asks for a confirmation but for no password, and it never
//! needs the network. The order is the point:
//!
//! 1. **Everything that could write a credential is stopped**: every launch in progress
//!    (the session token only exists in that launcher's memory; the game and its display
//!    server die with their launcher) and every sign-in, skin, cape, canary or vault command
//!    that is still running — otherwise one of them would store a fresh token a moment after
//!    this command said that none was left. This takes a signal and a bounded wait.
//! 2. **Every stored refresh token is removed** — from the kernel keyring *and* from the age
//!    vault, whichever store is configured now, because the configuration may have changed
//!    since a token was stored. Then the remembered identities and the default-account
//!    choice.
//! 3. If a command appeared in the meantime, 1 and 2 are done once more.
//! 4. **Last**, and for a bounded time only: what killed or crashed launches left in the
//!    cache (game directories). A game can leave millions of files there; no credential may
//!    wait behind their deletion.
//!
//! **Nothing is reported as done that was not confirmed.** A launcher is "killed" when the
//! kernel reported its end; a token is "removed" when the kernel's own key list no longer
//! shows it; a vault is empty when the directory was read again. Everything that could not
//! be done or could not be checked is a line starting with `COULD NOT`, and makes the
//! result *incomplete* (exit status 1). That contract is what front ends build on.
//!
//! What it deliberately keeps: the profile canary and the activity log. They hold no
//! credential, and they are the evidence `export-proofs` hands to Mojang's support.
//!
//! What it cannot do is said in its own output (and in `docs/SCRAM_RUNBOOK.md`): nothing is
//! revoked at Microsoft, and deleting a file is not a secure wipe. And one thing it cannot
//! reach: a key that only another login session of the same user still holds (the kernel
//! lists it, this session may not touch it) — reported as such. And one thing it can see
//! but not act on: a launch whose marker is gone (deleted by another program, or by a
//! launcher that was compromised) still holds its game directory locked. That lock is
//! reported as a launch that could not be stopped; the process is not hunted down by name.

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use crate::{
    auth::{identity, registry, store},
    error::Result,
    paths::{Paths, validate_id},
    running::{self, Activity, Stopped},
    util,
};

/// What was done, as lines for the user.
#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<String>,
    /// Something could not be removed, stopped or checked; the lines starting with
    /// `COULD NOT` say what.
    pub incomplete: bool,
}

impl Report {
    fn say(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn could_not(&mut self, what: impl AsRef<str>) {
        self.incomplete = true;
        self.lines.push(format!("COULD NOT {}", what.as_ref()));
    }
}

const KEY_PREFIX: &str = "bastion-mc:msa-refresh:";
/// For all marked processes together: SIGKILL needs no cooperation, so this is ample.
const STOP_WAIT: Duration = Duration::from_secs(5);
/// Invalidated keys leave the kernel's list asynchronously.
const KEY_SETTLE: Duration = Duration::from_secs(2);
/// Deleting what games left behind may take this long and no longer.
const SWEEP_TIME: Duration = Duration::from_secs(15);

/// Account labels with a live refresh-token key in the kernel's own list (`/proc/keys`
/// text). A key that is revoked, dead or already invalidated (flags `R`, `D`, `i`) is on its
/// way out and does not count.
fn keyring_accounts(proc_keys: &str) -> BTreeSet<String> {
    proc_keys
        .lines()
        .filter_map(|line| {
            let flags = line.split_ascii_whitespace().nth(1)?;
            if flags.contains(['R', 'D', 'i']) {
                return None;
            }
            let rest = &line[line.find(KEY_PREFIX)? + KEY_PREFIX.len()..];
            let label: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != ':').collect();
            validate_id(&label).is_ok().then_some(label)
        })
        .collect()
}

/// Where the kernel's key list comes from.
pub type KeyList<'a> = &'a dyn Fn() -> std::io::Result<String>;

pub fn run(paths: &Paths) -> Result<Report> {
    run_with(paths, None, &|| std::fs::read_to_string("/proc/keys"))
}

/// [`run`] for the given keyring accounts only. The kernel keyring is shared by everything
/// running as the user, whatever directories a caller points the launcher at — so the tests
/// name the one account they created, and can never remove (or even look for) a real
/// sign-in: of the kernel's list only the lines of these accounts are used.
pub fn run_for(paths: &Paths, keyring: BTreeSet<String>) -> Result<Report> {
    let wanted: Vec<String> = keyring.iter().map(|account| format!("{KEY_PREFIX}{account}:")).collect();
    run_with(paths, Some(keyring), &|| {
        let all = std::fs::read_to_string("/proc/keys")?;
        Ok(all
            .lines()
            .filter(|line| wanted.iter().any(|w| line.contains(w.as_str())))
            .map(|l| format!("{l}\n"))
            .collect())
    })
}

/// The whole of scram. `only`: keyring accounts to act on (`None`: every one the kernel
/// lists). `key_list` yields the kernel's key list.
#[doc(hidden)]
pub fn run_with(paths: &Paths, only: Option<BTreeSet<String>>, key_list: KeyList<'_>) -> Result<Report> {
    let mut report = Report::default();

    // 1. Everything that could still write a credential.
    let first = stop_commands(paths, &mut report);
    match running::unreadable(paths) {
        // Then a command whose marker could not be read was not stopped, and "nothing was
        // running" is not known.
        Some(why) => report.could_not(format!(
            "find out which launcher commands are running ({why}): a launch or a sign-in whose marker could not \
             be read was not stopped. Look for one yourself (`pgrep -a bastion-mc`), end it (`kill -9 <pid>`), \
             and run `bastion-mc scram` again."
        )),
        None if first.is_empty() => report.say("no launch was in progress"),
        None => {}
    }
    // 2. The credentials.
    remove_credentials(paths, only.as_ref(), key_list, &mut report);
    // 3. Something started while that was going on (a sign-in the user had forgotten in
    // another window, a launch from a front end): once more, then say so if it still is.
    if !running::list(paths).is_empty() {
        let second = stop_commands(paths, &mut report);
        remove_credentials(paths, only.as_ref(), key_list, &mut report);
        // One that was listed before and could not be stopped has said so (in each pass);
        // what is listed now and was in neither pass started after the second.
        for late in running::list(paths).into_iter().filter(|late| !first.contains(late) && !second.contains(late)) {
            report.could_not(format!(
                "make sure that nothing can store a sign-in any more: {} (pid {}) started while this was running. \
                 Run `bastion-mc scram` again.",
                describe(&late.activity),
                late.pid
            ));
        }
    }

    // 4. Last: what the games left behind. Bounded, because its size is the game's choice.
    let sweep = running::sweep_leftovers(paths, Duration::ZERO, Some(Instant::now() + SWEEP_TIME));
    if sweep.removed > 0 {
        report.say(format!(
            "deleted {} leftover game director(ies) of launches that were killed or crashed",
            sweep.removed
        ));
    }
    for failure in &sweep.failed {
        report.could_not(format!(
            "delete a leftover game directory in {} ({failure}). It holds what the game wrote, not the launcher's \
             credentials; the next launch or `bastion-mc scram` tries again.",
            paths.scratch().display()
        ));
    }
    // Every marked launch was stopped above (or is reported as not stopped). A directory
    // that a process still holds locked now belongs to a launch without a marker — one that
    // this command therefore did not find, and did not stop.
    if running::list(paths).is_empty() {
        for name in &sweep.in_use {
            report.could_not(format!(
                "stop a launch that left no marker: the directory {name} in {} is held by a running launcher. Its \
                 game — and a session token, if it was signed in — are still there. Find that process (`pgrep -a \
                 bastion-mc`), end it (`kill -9 <pid>`), and run `bastion-mc scram` again.",
                paths.cache.display()
            ));
        }
    }

    // A second pass repeats what it still could not do; say each thing once.
    let mut seen = BTreeSet::new();
    report.lines.retain(|line| seen.insert(line.clone()));

    report.say(String::new());
    report.say("This removed the copies on this machine. It revoked NOTHING at Microsoft or Mojang:");
    report.say("  - a refresh token that was copied earlier works until you \"sign out everywhere\";");
    report.say("  - a Minecraft session token that was already issued is valid for up to about 24 hours.");
    report.say("Deleted files are not securely wiped (SSDs and copy-on-write file systems keep old blocks);");
    report.say("an age vault file was encrypted, and its token is worthless once you sign out everywhere.");
    report.say("Kept on purpose: the profile canary history and the activity log (no credentials; evidence).");
    report.say("Next: `bastion-mc wiki scram` — the recovery steps, with verified links. If this machine may be");
    report.say("compromised, do those steps from another device.");
    Ok(report)
}

fn describe(activity: &Activity) -> String {
    match activity {
        Activity::Launch(instance) => format!("the launcher of instance {instance}"),
        Activity::Command(name) => format!("`bastion-mc {name}`"),
    }
}

/// Stop every marked command. Returns every one that was listed, whatever became of it.
fn stop_commands(paths: &Paths, report: &mut Report) -> Vec<running::Running> {
    let stopped = running::stop_all(paths, STOP_WAIT);
    for (running, outcome) in &stopped {
        let (what, pid) = (describe(&running.activity), running.pid);
        match (outcome, &running.activity) {
            (Stopped::Killed, Activity::Launch(_)) => {
                report.say(format!("killed {what} (pid {pid}): its session token is gone with it"))
            }
            (Stopped::Killed, Activity::Command(_)) => {
                report.say(format!("killed {what} (pid {pid}) before it could store a sign-in"))
            }
            (Stopped::AlreadyGone, _) => report.say(format!("{what} (pid {pid}) ended by itself")),
            (Stopped::Failed(why), _) => report.could_not(format!(
                "stop {what} (pid {pid}): {why}. While it runs it holds, or can store, a credential; end it yourself \
                 (`kill -9 {pid}`) and run `bastion-mc scram` again."
            )),
        }
    }
    stopped.into_iter().map(|(running, _)| running).collect()
}

fn remove_credentials(paths: &Paths, only: Option<&BTreeSet<String>>, key_list: KeyList<'_>, report: &mut Report) {
    // Refresh tokens in the kernel keyring. The kernel's own list finds tokens of accounts
    // that nothing on disk remembers.
    let read_list = || key_list().map(|text| keyring_accounts(&text));
    let before = match read_list() {
        Ok(found) => Some(found),
        Err(e) => {
            report.could_not(format!(
                "read the kernel's list of keys (/proc/keys: {e}): a refresh token of an account this launcher has \
                 no record of cannot be found, and removals cannot be confirmed"
            ));
            None
        }
    };
    let mut accounts: BTreeSet<String> = registry::list(paths).into_iter().collect();
    match (only, &before) {
        (Some(only), _) => accounts.extend(only.iter().cloned()),
        (None, Some(found)) => accounts.extend(found.iter().cloned()),
        (None, None) => {}
    }
    let in_scope = |account: &String| only.is_none_or(|only| only.contains(account));
    let mut invalidated: BTreeSet<&String> = BTreeSet::new();
    for account in accounts.iter().filter(|a| in_scope(a)) {
        // Until this login session finds no key under that name any more (bounded: a new
        // key cannot appear, everything that could store one was stopped).
        for _ in 0..8 {
            match store::keyutils_remove(account) {
                Ok(true) => {
                    invalidated.insert(account);
                }
                Ok(false) => break,
                Err(e) => {
                    report.could_not(format!("remove the refresh token of account {account} from the keyring: {e}"));
                    break;
                }
            }
        }
    }
    // Confirmed against the kernel's list, not assumed: invalidation is asynchronous, and a
    // key that only another login session holds is listed but cannot be reached from here.
    let deadline = Instant::now() + KEY_SETTLE;
    let mut after = read_list().ok();
    while let Some(left) = &after
        && accounts.iter().any(|a| in_scope(a) && left.contains(a))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(40));
        after = read_list().ok();
    }
    if before.is_some() && after.is_none() {
        report.could_not("read the kernel's list of keys a second time: removals from the keyring cannot be confirmed");
    }
    for account in accounts.iter().filter(|a| in_scope(a)) {
        let was_there = invalidated.contains(account) || before.as_ref().is_some_and(|b| b.contains(account));
        match &after {
            Some(left) if left.contains(account) => report.could_not(format!(
                "remove the refresh token of account {account}: the kernel still lists a key for it that this \
                 login session cannot reach (it belongs to another login session of yours){}. Run `bastion-mc \
                 scram` in that session too, or log out of it; the key is gone at the latest after a reboot.",
                if invalidated.contains(account) { "; the key this session could reach was removed" } else { "" }
            )),
            Some(_) if was_there => {
                report.say(format!("removed the refresh token of account {account} from the kernel keyring"))
            }
            // The list could not be read (said above): what this session could reach was
            // invalidated, but that it is gone was not confirmed.
            None if invalidated.contains(account) => report.say(format!(
                "invalidated the refresh token of account {account} in this login session's keyring (not confirmed \
                 against the kernel's list)"
            )),
            _ => {}
        }
    }

    // Every entry in the vault directory, whatever it is called.
    let vault = paths.vault();
    // Entries already reported as left there (a directory, a file that could not be deleted).
    let mut reported: BTreeSet<std::ffi::OsString> = BTreeSet::new();
    match std::fs::read_dir(&vault) {
        Ok(entries) => {
            // (An entry that cannot be read ends the listing; the second look below finds
            // whatever it hid.)
            for entry in entries.map_while(std::result::Result::ok) {
                let path = entry.path();
                let shown = util::sanitize_line(entry.file_name().as_encoded_bytes(), 120);
                // A file or a link is removed (a link as a link). A directory is not something
                // this launcher creates there: it is not searched, and it is not ignored.
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    report.could_not(format!(
                        "clear the vault directory: {shown:?} in {} is a directory, which this launcher never \
                         creates there. It was left alone; look at it yourself.",
                        vault.display()
                    ));
                    reported.insert(entry.file_name());
                    continue;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        report.could_not(format!("delete the vault file {shown:?} in {}: {e}", vault.display()));
                        reported.insert(entry.file_name());
                    }
                }
                if std::fs::symlink_metadata(&path).is_err() {
                    report.say(format!("deleted the vault file {shown:?}"));
                }
            }
            // Confirmed, not assumed from the deletions: the directory is read again. What a
            // listing passed over, or what appeared meanwhile, is said.
            let unconfirmed = |e: std::io::Error| {
                format!(
                    "read the vault directory {} again ({e}): that no vault file is left is not confirmed",
                    vault.display()
                )
            };
            match std::fs::read_dir(&vault) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry) if reported.contains(&entry.file_name()) => {}
                            Ok(entry) => report.could_not(format!(
                                "clear the vault directory: {:?} in {} is still there",
                                util::sanitize_line(entry.file_name().as_encoded_bytes(), 120),
                                vault.display()
                            )),
                            Err(e) => {
                                report.could_not(unconfirmed(e));
                                break;
                            }
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => report.could_not(unconfirmed(e)),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => report.could_not(format!(
            "read the vault directory {} ({e}): age vault files in it, if any, were NOT deleted",
            vault.display()
        )),
    }

    // What the launcher remembered about who is signed in. (Every label, also those outside
    // `only`: these are files in the directories this run was pointed at.)
    for account in &accounts {
        let file = paths.identities().join(format!("{account}.json"));
        if std::fs::symlink_metadata(&file).is_err() {
            continue;
        }
        match identity::forget(paths, account) {
            Ok(()) if std::fs::symlink_metadata(&file).is_err() => {
                report.say(format!("forgot the remembered player of account {account}"))
            }
            Ok(()) => report.could_not(format!("remove the remembered player of account {account}: it is still there")),
            Err(e) => report.could_not(format!("remove the remembered player of account {account}: {e}")),
        }
    }
    match std::fs::remove_file(paths.default_account_file()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => report.could_not(format!("forget which account is the default: {e}")),
    }
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
6f708192 I--Q--i     1 perm 3f010000  1000  1000 user      bastion-mc:msa-refresh:invalidated: 10
708192a3 IR-Q---     1 perm 3f010000  1000  1000 user      bastion-mc:msa-refresh:revoked: 10
";
        let found: Vec<String> = keyring_accounts(keys).into_iter().collect();
        assert_eq!(found, ["default", "work-2"]);
        assert!(keyring_accounts("").is_empty());
    }

    fn paths(tag: &str) -> (std::path::PathBuf, Paths) {
        let root = std::env::temp_dir()
            .join(format!("bastion-scram-u-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        };
        paths.ensure().unwrap();
        (root, paths)
    }

    /// The kernel lists a key that this session cannot reach (here: a made-up account for
    /// which the keyring holds nothing): scram says so and is incomplete, instead of
    /// printing nothing and exiting 0.
    #[test]
    fn a_listed_key_that_cannot_be_reached_makes_the_result_incomplete() {
        let (root, paths) = paths("ghost");
        let ghost = format!("ghost-{}", std::process::id());
        let line = format!("0a1b2c3d I--Q---     1 perm 3f010000  1000  1000 user      {KEY_PREFIX}{ghost}: 1432\n");
        let report = run_with(&paths, Some([ghost.clone()].into()), &|| Ok(line.clone())).unwrap();
        let text = report.lines.join("\n");
        assert!(report.incomplete, "{text}");
        assert!(
            text.contains(&format!("COULD NOT remove the refresh token of account {ghost}: the kernel still lists")),
            "{text}"
        );
        assert!(!text.contains(&format!("removed the refresh token of account {ghost}")), "{text}");

        // An unreadable key list is not silence either.
        let report = run_with(&paths, Some([ghost].into()), &|| Err(std::io::Error::other("no /proc here"))).unwrap();
        let text = report.lines.join("\n");
        assert!(report.incomplete && text.contains("COULD NOT read the kernel's list of keys"), "{text}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_or_unexpected_vault_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let (root, paths) = paths("vault");
        let none: BTreeSet<String> = BTreeSet::new();
        std::fs::create_dir_all(paths.vault().join("backup")).unwrap();
        std::fs::write(paths.vault().join("backup/old.age"), b"ciphertext").unwrap();
        std::fs::write(paths.vault().join("evil\u{1b}[2Jname.age"), b"ciphertext").unwrap();
        std::os::unix::fs::symlink("/etc/hostname", paths.vault().join("link.age")).unwrap();
        let report = run_with(&paths, Some(none.clone()), &|| Ok(String::new())).unwrap();
        let text = report.lines.join("\n");
        assert!(report.incomplete && text.contains("COULD NOT clear the vault directory: \"backup\""), "{text}");
        assert!(!text.contains('\u{1b}'), "file names are shown sanitised: {text:?}");
        assert!(text.contains("deleted the vault file \"link.age\""), "a link is removed as a link: {text}");
        assert!(std::path::Path::new("/etc/hostname").exists());
        assert!(paths.vault().join("backup/old.age").exists(), "left alone, and said so");
        // The vault is read a second time, to confirm what is left: what was reported on the
        // first look is not reported again in other words.
        assert_eq!(text.matches("\"backup\"").count(), 1, "{text}");
        assert!(!text.contains("is still there"), "{text}");

        // A file that cannot be deleted (the directory does not allow it): said once.
        std::fs::remove_dir_all(paths.vault().join("backup")).unwrap();
        std::fs::write(paths.vault().join("default.age"), b"ciphertext").unwrap();
        std::fs::set_permissions(paths.vault(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let report = run_with(&paths, Some(none.clone()), &|| Ok(String::new())).unwrap();
        std::fs::set_permissions(paths.vault(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let text = report.lines.join("\n");
        assert!(report.incomplete && text.contains("COULD NOT delete the vault file \"default.age\""), "{text}");
        assert_eq!(text.matches("\"default.age\"").count(), 1, "{text}");

        std::fs::set_permissions(paths.vault(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let report = run_with(&paths, Some(none), &|| Ok(String::new())).unwrap();
        std::fs::set_permissions(paths.vault(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let text = report.lines.join("\n");
        assert!(report.incomplete && text.contains("COULD NOT read the vault directory"), "{text}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
