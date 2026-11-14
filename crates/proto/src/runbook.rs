//! The emergency runbook and the only links that may be opened from it.
//!
//! Both the launcher (`bastion-mc wiki scram`) and a front end show the same text, and a
//! front end offers to open its links. Which links is not left to whoever renders the
//! text: [`LINKS`] is the complete list, compiled in, compared **exactly** — no prefix or
//! domain matching, so nothing that merely looks like a Microsoft address can pass. A test
//! keeps the list and the text identical.

/// `docs/SCRAM_RUNBOOK.md`, as shipped.
pub const SCRAM: &str = include_str!("../../../docs/SCRAM_RUNBOOK.md");

/// Every address the runbook refers to: (address, what it is).
pub const LINKS: &[(&str, &str)] = &[
    ("https://go.microsoft.com/fwlink/p/?linkid=2071806", "Microsoft account: change the password"),
    ("https://go.microsoft.com/fwlink/p/?LinkID=238656", "Microsoft account: reset a password you no longer know"),
    (
        "https://account.live.com/proofs/manage/additional",
        "Microsoft account: manage how I sign in (sign out everywhere, recovery code, sign-in methods)",
    ),
    ("https://www.microsoft.com/consent", "Microsoft account: apps and services you have given access to"),
    ("https://account.live.com/Activity", "Microsoft account: recent activity"),
    ("https://account.microsoft.com/security", "Microsoft account: security overview"),
    (
        "https://support.microsoft.com/en-us/accounts-billing/manage/how-to-recover-a-hacked-or-compromised-microsoft-account",
        "Microsoft Support: how to recover a hacked or compromised account",
    ),
    (
        "https://support.microsoft.com/en-us/accounts-billing/manage/how-to-sign-out-of-your-microsoft-account-everywhere",
        "Microsoft Support: how to sign out everywhere",
    ),
    (
        "https://support.microsoft.com/en-us/accounts-billing/manage/how-to-get-a-microsoft-account-recovery-code",
        "Microsoft Support: recovery codes",
    ),
    ("https://go.microsoft.com/fwlink/?linkid=2288285", "Microsoft account: the account recovery form"),
    (
        "https://support.microsoft.com/en-us/accounts-billing/manage/help-with-the-microsoft-account-recovery-form",
        "Microsoft Support: help with the recovery form",
    ),
    ("https://help.minecraft.net/hc/en-us/requests/new", "Minecraft Support: submit a request"),
];

/// May this address be handed to the desktop's URL opener from the runbook?
pub fn is_runbook_link(url: &str) -> bool {
    LINKS.iter().any(|(link, _)| *link == url)
}

/// The addresses that appear in `text`, in order (anything from `https://` up to white space).
pub fn links_in(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("https://") {
        let tail = &rest[at..];
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '"' | '`')).unwrap_or(tail.len());
        found.push(&tail[..end]);
        rest = &tail[end..];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runbook_and_the_allowlist_name_exactly_the_same_links() {
        let mut in_text: Vec<&str> = links_in(SCRAM);
        in_text.sort_unstable();
        in_text.dedup();
        let mut listed: Vec<&str> = LINKS.iter().map(|(link, _)| *link).collect();
        listed.sort_unstable();
        assert_eq!(in_text, listed, "docs/SCRAM_RUNBOOK.md and runbook::LINKS must list the same addresses");
        for link in &in_text {
            assert!(is_runbook_link(link));
            assert!(link.is_ascii() && !link.ends_with('.') && !link.ends_with(','), "{link}");
        }
    }

    #[test]
    fn nothing_else_passes_however_similar() {
        for no in [
            "https://account.live.com/proofs/manage/additional/",
            "https://account.live.com/proofs/manage/additional?x=1",
            "https://account.live.com/proofs/Manage/additional",
            "https://account.live.com/",
            "https://account.live.com.evil.example/proofs/manage/additional",
            "http://account.live.com/proofs/manage/additional",
            "https://go.microsoft.com/fwlink/?linkid=1",
            "https://help.minecraft.net/hc/en-us/requests/new#x",
            "https://minecraft.net",
            " https://account.live.com/Activity",
            "",
        ] {
            assert!(!is_runbook_link(no), "{no:?}");
        }
    }

    #[test]
    fn the_runbook_puts_the_device_warning_before_any_step() {
        let warning = SCRAM.find("from a different device").expect("the clean-device warning");
        let first_step = SCRAM.find("## Phase 1").expect("phase 1");
        let first_link = SCRAM.find("https://").expect("a link");
        assert!(warning < first_step && warning < first_link);
        // Honest about what the local command cannot do.
        assert!(SCRAM.contains("revokes nothing at Microsoft"));
        assert!(SCRAM.contains("does not \"securely wipe\""));
    }
}
