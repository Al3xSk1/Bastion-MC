//! `bastion-mc export-proofs`: what this launcher knows that helps prove an account is yours.
//!
//! Mojang's support decides ownership disputes from things the real owner knows and a
//! thief does not: the names the account has had, its capes, when it was used. The launcher
//! has recorded exactly that (the profile canary's history and the activity log), and this
//! writes it out as plain text for the user to read, complete and send.
//!
//! It contains no token, no key and nothing from the network at export time. It also
//! contains nothing the launcher does not have: no IP addresses, no hardware details —
//! keeping those would make the launcher's state directory worth stealing.

use std::fmt::Write as _;

use crate::{
    auth::{activity, canary},
    error::Result,
    paths::Paths,
};

/// Civil date and time (UTC) of a Unix time stamp.
pub fn timestamp(unix: u64) -> String {
    // Days to civil date: Howard Hinnant's algorithm.
    let z = (unix / 86_400) as i64 + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let secs = unix % 86_400;
    format!("{:04}-{m:02}-{d:02} {:02}:{:02} UTC", yoe + era * 400 + i64::from(m <= 2), secs / 3600, secs % 3600 / 60)
}

fn snapshot_line(out: &mut String, s: &canary::Snapshot) {
    let skin = match &s.skin {
        Some((texture, model)) => format!("{model} model, texture {texture}"),
        None => "default skin".to_string(),
    };
    let cape = s.cape.as_ref().map(|c| c.1.as_str()).unwrap_or("none");
    let owned = if s.capes_owned.is_empty() { "none".to_string() } else { s.capes_owned.join(", ") };
    let _ = writeln!(out, "  {}  name: {}", timestamp(s.recorded), s.name);
    let _ = writeln!(out, "      skin: {skin}");
    let _ = writeln!(out, "      cape worn: {cape}; capes owned: {owned}");
}

/// The bundle for `account`, as text. Fails only if the canary record is damaged.
pub fn bundle(paths: &Paths, account: &str, now: u64) -> Result<String> {
    let mut out = String::new();
    let _ = writeln!(out, "Minecraft account ownership notes");
    let _ = writeln!(out, "written by bastion-mc {} on {}", env!("CARGO_PKG_VERSION"), timestamp(now));
    let _ = writeln!(out, "local account label: {account}");
    let _ = writeln!(out);
    match canary::load(paths, account)? {
        None => {
            let _ = writeln!(out, "No profile was ever recorded for this account on this machine.");
        }
        Some(record) => {
            let _ = writeln!(out, "Player id (UUID): {}", record.current.uuid);
            let mut names: Vec<&str> =
                record.history.iter().chain(std::iter::once(&record.current)).map(|s| s.name.as_str()).collect();
            names.dedup();
            let _ = writeln!(out, "Player names seen by this launcher, oldest first: {}", names.join(", "));
            let _ = writeln!(out);
            let _ = writeln!(out, "Profile states this launcher recorded (each when it was first seen or accepted):");
            for s in record.history.iter().chain(std::iter::once(&record.current)) {
                snapshot_line(&mut out, s);
            }
        }
    }
    let _ = writeln!(out);
    let entries = activity::read(paths, account);
    if entries.is_empty() {
        let _ = writeln!(out, "No sign-ins or launches are recorded for this account.");
    } else {
        let count = |what: &str| entries.iter().filter(|e| e.what == what).count();
        let _ = writeln!(
            out,
            "Use of the account from this machine: {} sign-in(s), {} launch(es), first {} — last {}",
            count("sign-in"),
            count("launch"),
            timestamp(entries[0].time),
            timestamp(entries[entries.len() - 1].time)
        );
        // The most recent ones in full; support asks about recent use.
        for e in entries.iter().rev().take(40).rev() {
            let _ = writeln!(
                out,
                "  {}  {}{}",
                timestamp(e.time),
                e.what,
                if e.detail.is_empty() { String::new() } else { format!(" (instance {})", e.detail) }
            );
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "To add by hand before sending (the launcher does not have these):");
    let _ = writeln!(out, "  - transaction ID from the purchase e-mail, or the gift code and where it was bought:");
    let _ = writeln!(out, "  - e-mail address used for the purchase, and the approximate date:");
    let _ = writeln!(out, "  - e-mail address of the Microsoft account before it was changed:");
    let _ = writeln!(out);
    let _ = writeln!(out, "This file contains no password, token or key. It does contain your player name and");
    let _ = writeln!(out, "when you played; send it only to Minecraft Support, through the form linked in");
    let _ = writeln!(out, "`bastion-mc wiki scram`.");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth::minecraft::{Cape, Profile, Skin, Variant},
        util,
    };

    #[test]
    fn timestamps_are_utc_civil_time() {
        assert_eq!(timestamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(timestamp(1_794_528_000 + 15 * 3600 + 26 * 60 + 35), "2026-11-13 15:26 UTC");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn the_bundle_lists_names_capes_and_use_and_never_a_credential() {
        let root =
            std::env::temp_dir().join(format!("bastion-prf-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        let paths = Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            runtime: None,
        };
        paths.ensure().unwrap();
        let empty = bundle(&paths, "default", 1_794_528_000).unwrap();
        assert!(empty.contains("No profile was ever recorded") && empty.contains("No sign-ins or launches"));

        let mut profile = Profile {
            id: "069a79f444e94726a5befca90e38aaf5".into(),
            name: "OldName".into(),
            skins: vec![Skin {
                id: "6a6e65e576dd4c3ca625162924514568".into(),
                active: true,
                variant: Variant::Slim,
                texture: "1a4af718455d4aab".into(),
            }],
            capes: vec![Cape {
                id: "5af2037279e04e1f80f86bd8e3135995".into(),
                active: true,
                alias: "Migrator".into(),
                texture: "2340c0e03dd24a11".into(),
            }],
        };
        canary::accept(&paths, "default", &profile).unwrap();
        profile.name = "NewName".into();
        canary::accept(&paths, "default", &profile).unwrap();
        activity::record(&paths, "default", "sign-in", "").unwrap();
        activity::record(&paths, "default", "launch", "vanilla").unwrap();

        // Things that must never be exported, lying around in the same directories.
        std::fs::create_dir_all(paths.vault()).unwrap();
        std::fs::write(paths.vault().join("default.age"), "VAULT-CIPHERTEXT-MARKER").unwrap();
        std::fs::write(paths.state.join("activity/default.log.extra"), "eyJhbGciOiJSECRET").unwrap();

        let text = bundle(&paths, "default", 1_794_528_000).unwrap();
        assert!(text.contains("Player id (UUID): 069a79f444e94726a5befca90e38aaf5"));
        assert!(text.contains("oldest first: OldName, NewName"));
        assert!(text.contains("slim model, texture 1a4af718455d4aab"));
        assert!(text.contains("cape worn: Migrator; capes owned: Migrator"));
        assert!(text.contains("1 sign-in(s), 1 launch(es)"));
        assert!(text.contains("launch (instance vanilla)"));
        assert!(text.contains("transaction ID"));
        for forbidden in ["VAULT", "eyJ", "refresh", "access_token", "Bearer", "M.C5"] {
            assert!(!text.contains(forbidden), "{forbidden} in:\n{text}");
        }
        assert!(text.is_ascii() || text.chars().all(|c| !c.is_control() || c == '\n'), "plain text only");
        std::fs::remove_dir_all(root).unwrap();
    }
}
