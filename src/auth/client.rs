//! Which Microsoft application a sign-in goes to.
//!
//! Two kinds: **your own app** — the Azure application whose client ID you put in
//! `[auth] client_id` — and **bastion-mc's app**, a public client registered for the project,
//! whose ID ships in [`PUBLIC_CLIENT_ID`] once Mojang has approved it. Your own wins whenever it
//! is set. Every place that signs in, refreshes, or reports which app it uses asks
//! [`effective`], so they cannot disagree.
//!
//! What the kind does not change: the requests of the token chain, where the tokens are kept,
//! the broker. A client ID is public data; neither app's owner ever receives anyone's tokens.

use std::path::Path;

use crate::config::AuthConfig;

/// bastion-mc's own Azure application (a public client). `None` until it is registered and
/// Mojang has approved it for the Minecraft API; then its client ID goes here.
pub const PUBLIC_CLIENT_ID: Option<&str> = None;

/// Whose application a sign-in goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    /// The application named by `[auth] client_id`.
    Own,
    /// bastion-mc's public application.
    Public,
}

/// The application a sign-in goes to: its client ID and whose it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Client<'a> {
    pub id: &'a str,
    pub kind: ClientKind,
}

/// The application this configuration signs in through: your own over bastion-mc's; `None`
/// when there is neither (only demo mode then).
pub fn effective(auth: &AuthConfig) -> Option<Client<'_>> {
    effective_with(auth, PUBLIC_CLIENT_ID)
}

/// [`effective`] with the public ID given, for tests.
pub fn effective_with<'a>(auth: &'a AuthConfig, public: Option<&'a str>) -> Option<Client<'a>> {
    match (auth.client_id.as_deref(), public) {
        (Some(id), _) => Some(Client { id, kind: ClientKind::Own }),
        (None, Some(id)) => Some(Client { id, kind: ClientKind::Public }),
        (None, None) => None,
    }
}

/// One phrase naming the application, for `status`, `doctor` and the sign-in line.
pub fn describe(client: Option<Client<'_>>) -> String {
    match client {
        Some(Client { id, kind: ClientKind::Public }) => format!("bastion-mc's app (client ID {id})"),
        Some(Client { id, kind: ClientKind::Own }) => format!("your own app (client ID {id}, from auth.client_id)"),
        None => "none (only demo mode)".to_string(),
    }
}

/// Why signing in is refused when there is no application at all.
pub fn none_message(config_file: &Path) -> String {
    format!(
        "No Microsoft client ID: this build has no public bastion-mc app yet and auth.client_id is not set in {}. \
         Register your own Azure application (README, \"Signing in\"); until then only `launch --demo` works.",
        config_file.display()
    )
}

/// What Minecraft's "Invalid app registration" means, depending on whose application it is.
pub fn not_approved_message(kind: ClientKind) -> String {
    match kind {
        ClientKind::Own => {
            "Minecraft services refused your Azure application (\"Invalid app registration\"). New client IDs \
             must be approved by Mojang before they may call the Minecraft API: see https://aka.ms/AppRegInfo \
             and submit the review form linked there. Nothing in this launcher can bypass that."
                .to_string()
        }
        ClientKind::Public => {
            "Minecraft services refused bastion-mc's application (\"Invalid app registration\"): Mojang \
             has not approved it yet, or has withdrawn the approval (https://aka.ms/AppRegInfo). Nothing on this \
             machine can change that; until it is approved, sign in through your own Azure application (README, \
             \"Signing in\") or play in demo mode."
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::AuthConfig;

    #[test]
    fn your_own_app_wins_over_the_public_one() {
        let own =
            AuthConfig { client_id: Some("11111111-2222-3333-4444-555555555555".into()), ..AuthConfig::default() };
        let c = effective_with(&own, Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")).unwrap();
        assert_eq!((c.id, c.kind), ("11111111-2222-3333-4444-555555555555", ClientKind::Own));
    }

    #[test]
    fn the_public_app_is_used_when_no_own_app_is_set() {
        let none = AuthConfig::default();
        let c = effective_with(&none, Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")).unwrap();
        assert_eq!(c.kind, ClientKind::Public);
    }

    #[test]
    fn without_either_there_is_no_client_and_the_message_names_both_ways() {
        assert!(effective_with(&AuthConfig::default(), None).is_none());
        let m = none_message(Path::new("/x/config.toml"));
        assert!(m.contains("no public bastion-mc app") && m.contains("auth.client_id") && m.contains("--demo"));
    }

    #[test]
    fn the_built_in_public_id_if_any_is_shaped_like_a_client_id() {
        if let Some(id) = PUBLIC_CLIENT_ID {
            crate::config::validate_client_id(id).unwrap();
        }
    }

    #[test]
    fn not_approved_speaks_of_whose_app_it_is() {
        assert!(not_approved_message(ClientKind::Own).contains("your Azure application"));
        let public = not_approved_message(ClientKind::Public);
        assert!(public.contains("bastion-mc's application") && !public.contains("your Azure application"));
        assert!(public.contains("AppRegInfo") || public.contains("Mojang"));
    }
}
