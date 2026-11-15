//! Bring-your-own-OAuth authentication.
//!
//! There is no client ID in this program. The user registers their own Azure application and
//! puts its ID in `config.toml`; every request below goes directly from this machine to
//! Microsoft, Xbox Live and Mojang. No intermediary, no telemetry.
//!
//! Chain: Microsoft OAuth 2.0 (Authorization Code + PKCE on a loopback port, or Device Code)
//! → Xbox Live user token → XSTS token → Minecraft access token → entitlement check → profile.
//!
//! Token lifetime in this process:
//!  * every token is deserialised straight into a locked [`Secret`] (`memfd_secret` pages
//!    where the kernel has them);
//!  * each intermediate token is dropped (wiped) as soon as the next step has used it;
//!  * the refresh token is written back to the store and wiped before the game starts;
//!  * the Minecraft access token goes one of two ways. With the credential broker (the
//!    default in filtered network mode) it stays here, in secret memory, for as long as the
//!    game runs — the game is handed a placeholder and the launcher authenticates the few
//!    requests that need it (`egress::broker`). Without the broker it is written into the
//!    launch channel and wiped at that moment.
//!
//! So while the game runs the launcher holds either exactly one credential, the 24-hour
//! access token that would otherwise be inside the game, or none. It never holds the
//! refresh token then.
//!
//! Around the chain: [`registry`] (which local accounts exist), [`canary`] (did the
//! profile change behind our back), [`activity`] (when each account was used), and the
//! profile commands in [`minecraft`] (skins and capes).

pub mod activity;
pub mod canary;
pub mod client;
pub mod identity;
pub mod minecraft;
pub mod msa;
pub mod registry;
pub mod store;
pub mod xbox;

use std::future::Future;

use bytes::Bytes;
use hyper::Method;

use crate::{
    error::{Error, Result},
    net::http::{Headers, HttpClient, SecretReply},
    secret::Secret,
    util,
};

/// Largest token-endpoint response we accept (they are a few kilobytes).
const REPLY_LIMIT: usize = 64 * 1024;

pub struct ApiRequest<'a> {
    pub method: Method,
    pub url: &'a str,
    pub headers: Headers,
    pub body: Option<Bytes>,
}

/// The one seam between protocol logic and the network, so the chain can be exercised
/// against scripted replies.
pub trait Api: Sync {
    fn call(&self, request: ApiRequest<'_>) -> impl Future<Output = Result<SecretReply>> + Send;
}

impl Api for HttpClient {
    async fn call(&self, request: ApiRequest<'_>) -> Result<SecretReply> {
        self.request_secret(request.method, request.url, &request.headers, request.body, REPLY_LIMIT).await
    }
}

/// A ready-to-play session.
#[derive(Debug)]
pub struct Session {
    pub access_token: Secret,
    /// Undashed UUID.
    pub profile_id: String,
    pub profile_name: String,
    pub license: minecraft::License,
    /// The whole profile as Mojang returned it at sign-in: skins and capes included.
    pub profile: minecraft::Profile,
}

/// Tokens are embedded in hand-built JSON (so they never pass through a `String`); that is
/// only sound if they need no escaping.
pub(crate) fn require_json_safe(token: &Secret, what: &str) -> Result<()> {
    let ok = !token.is_empty() && token.expose().iter().all(|b| (0x20..0x7f).contains(b) && *b != b'"' && *b != b'\\');
    if ok { Ok(()) } else { Err(Error::Auth(format!("{what} contains unexpected characters"))) }
}

/// Server-provided text that ends up in an error message: bounded and stripped of controls.
pub(crate) fn clean(text: &str) -> String {
    util::sanitize_line(text.as_bytes(), 400)
}

/// Refresh-token → playable session. Returns the session and the refresh token to store
/// (Microsoft rotates it on use).
pub async fn authenticate(
    api: &impl Api,
    client: client::Client<'_>,
    refresh_token: &Secret,
    allow_game_pass: bool,
) -> Result<(Session, Secret)> {
    let msa = msa::refresh(api, client.id, refresh_token).await?;
    let next_refresh = match msa.refresh_token {
        Some(t) => t,
        None => refresh_token.try_clone()?,
    };
    let session = session_from_msa(api, msa.access_token, allow_game_pass, client.kind).await?;
    Ok((session, next_refresh))
}

/// Microsoft access token → Xbox Live → XSTS → Minecraft → entitlement → profile. `kind` is
/// whose application the token was issued to, for the message when Mojang refuses it.
pub async fn session_from_msa(
    api: &impl Api,
    msa_access: Secret,
    allow_game_pass: bool,
    kind: client::ClientKind,
) -> Result<Session> {
    let xbl = xbox::user_token(api, &msa_access).await?;
    drop(msa_access); // wiped
    let xsts = xbox::xsts_token(api, &xbl).await?;
    drop(xbl);
    let access_token = minecraft::login_with_xbox(api, &xsts, kind).await?;
    drop(xsts);
    let license = minecraft::check_entitlement(api, &access_token, allow_game_pass).await?;
    let profile = minecraft::profile(api, &access_token).await?;
    Ok(Session { access_token, profile_id: profile.id.clone(), profile_name: profile.name.clone(), license, profile })
}

#[cfg(test)]
pub(crate) mod testing {
    //! A scripted [`Api`]: replies are matched by URL suffix, requests are recorded.
    use super::*;
    use std::sync::Mutex;

    pub struct Scripted {
        pub replies: Vec<(&'static str, u16, String)>,
        pub seen: Mutex<Vec<(String, String, String)>>, // (url, authorization header, body)
    }

    impl Scripted {
        pub fn new(replies: Vec<(&'static str, u16, &str)>) -> Self {
            Scripted {
                replies: replies.into_iter().map(|(u, s, b)| (u, s, b.to_string())).collect(),
                seen: Mutex::new(Vec::new()),
            }
        }
        pub fn requests(&self) -> Vec<(String, String, String)> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Api for Scripted {
        async fn call(&self, request: ApiRequest<'_>) -> Result<SecretReply> {
            let auth = request
                .headers
                .iter()
                .find(|(k, _)| k.as_str() == "authorization")
                .map(|(_, v)| String::from_utf8_lossy(v.as_bytes()).into_owned())
                .unwrap_or_default();
            let body = request.body.as_ref().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            let mut seen = self.seen.lock().unwrap();
            let nth = seen.iter().filter(|(u, _, _)| u == request.url).count();
            seen.push((request.url.to_string(), auth, body));
            // The n-th call to a URL gets the n-th scripted reply for it (last one repeats).
            let matching: Vec<_> = self.replies.iter().filter(|(u, _, _)| request.url.ends_with(u)).collect();
            let (_, status, text) = matching
                .get(nth)
                .or(matching.last())
                .unwrap_or_else(|| panic!("no scripted reply for {}", request.url));
            Ok(SecretReply { status: *status, body: Secret::from_slice(text.as_bytes())? })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::client::{Client, ClientKind};

    /// The client of most tests: an own application.
    const OWN: Client<'static> = Client { id: "01234567-89ab-cdef-0123-456789abcdef", kind: ClientKind::Own };

    use super::{testing::Scripted, *};

    const MSA_OK: &str = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"MSA-ACCESS","refresh_token":"MSA-REFRESH-ROTATED"}"#;
    const XBL_OK: &str =
        r#"{"IssueInstant":"x","NotAfter":"y","Token":"XBL-TOKEN","DisplayClaims":{"xui":[{"uhs":"1234567890"}]}}"#;
    const XSTS_OK: &str = r#"{"Token":"XSTS-TOKEN","DisplayClaims":{"xui":[{"uhs":"1234567890"}]}}"#;
    const MC_OK: &str =
        r#"{"username":"u","roles":[],"access_token":"MC-ACCESS-TOKEN","token_type":"Bearer","expires_in":86400}"#;
    const ENT_OK: &str = r#"{"items":[{"name":"product_minecraft","signature":"s"},{"name":"game_minecraft","signature":"s"}],"signature":"s","keyId":"1"}"#;
    const PROFILE_OK: &str = r#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch","skins":[],"capes":[]}"#;

    fn happy() -> Scripted {
        Scripted::new(vec![
            ("/oauth2/v2.0/token", 200, MSA_OK),
            ("/user/authenticate", 200, XBL_OK),
            ("/xsts/authorize", 200, XSTS_OK),
            ("/authentication/login_with_xbox", 200, MC_OK),
            ("/entitlements/mcstore", 200, ENT_OK),
            ("/minecraft/profile", 200, PROFILE_OK),
        ])
    }

    #[tokio::test]
    async fn full_chain_passes_each_token_to_exactly_the_next_step() {
        let api = happy();
        let refresh = Secret::from_slice(b"OLD-REFRESH").unwrap();
        let (session, next) = authenticate(&api, OWN, &refresh, false).await.unwrap();
        assert_eq!(session.access_token.expose(), b"MC-ACCESS-TOKEN");
        assert_eq!(session.profile_name, "Notch");
        assert_eq!(session.profile_id, "069a79f444e94726a5befca90e38aaf5");
        assert_eq!(session.license, minecraft::License::Owned);
        assert_eq!(next.expose(), b"MSA-REFRESH-ROTATED", "the rotated refresh token is the one to store");

        let reqs = api.requests();
        let urls: Vec<&str> = reqs.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://login.microsoftonline.com/consumers/oauth2/v2.0/token",
                "https://user.auth.xboxlive.com/user/authenticate",
                "https://xsts.auth.xboxlive.com/xsts/authorize",
                "https://api.minecraftservices.com/authentication/login_with_xbox",
                "https://api.minecraftservices.com/entitlements/mcstore",
                "https://api.minecraftservices.com/minecraft/profile",
            ]
        );
        assert!(reqs[0].2.contains("grant_type=refresh_token") && reqs[0].2.contains("refresh_token=OLD-REFRESH"));
        assert!(reqs[0].2.contains("client_id=01234567-89ab-cdef-0123-456789abcdef"));
        assert!(!reqs[0].2.contains("client_secret"), "public client: no secret is ever sent");
        assert!(reqs[1].2.contains(r#""RpsTicket":"d=MSA-ACCESS""#));
        assert!(
            reqs[2].2.contains(r#""UserTokens":["XBL-TOKEN"]"#)
                && reqs[2].2.contains("rp://api.minecraftservices.com/")
        );
        assert!(reqs[3].2.contains(r#""identityToken":"XBL3.0 x=1234567890;XSTS-TOKEN""#));
        assert_eq!(reqs[4].1, "Bearer MC-ACCESS-TOKEN");
        assert_eq!(reqs[5].1, "Bearer MC-ACCESS-TOKEN");
        // Tokens only travel forward: the Microsoft token never reaches Mojang, etc.
        assert!(!reqs[3].2.contains("MSA-ACCESS") && !reqs[2].2.contains("MSA-ACCESS"));
    }

    #[tokio::test]
    async fn refresh_token_is_kept_when_not_rotated() {
        let mut api = happy();
        api.replies[0].2 = r#"{"access_token":"MSA-ACCESS","expires_in":3600}"#.into();
        let refresh = Secret::from_slice(b"OLD-REFRESH").unwrap();
        let (_, next) = authenticate(&api, OWN, &refresh, false).await.unwrap();
        assert_eq!(next.expose(), b"OLD-REFRESH");
    }

    #[tokio::test]
    async fn each_failure_stops_the_chain_with_a_specific_message() {
        let cases: Vec<(usize, u16, &str, &str)> = vec![
            (0, 400, r#"{"error":"invalid_grant","error_description":"AADSTS70000: expired"}"#, "bastion-mc login"),
            (1, 401, "", "Xbox Live"),
            (
                2,
                401,
                r#"{"Identity":"0","XErr":2148916233,"Message":"","Redirect":"https://start.ui.xboxlive.com/CreateAccount"}"#,
                "no Xbox profile",
            ),
            (2, 401, r#"{"Identity":"0","XErr":2148916238,"Message":""}"#, "Family"),
            (
                3,
                403,
                r#"{"path":"/authentication/login_with_xbox","errorMessage":"Invalid app registration, see https://aka.ms/AppRegInfo for more information"}"#,
                "AppRegInfo",
            ),
            (4, 200, r#"{"items":[],"signature":"s","keyId":"1"}"#, "does not own Minecraft"),
            (5, 404, r#"{"path":"/minecraft/profile","error":"NOT_FOUND"}"#, "no Minecraft profile"),
        ];
        for (step, status, body, expect) in cases {
            let mut api = happy();
            api.replies[step].1 = status;
            api.replies[step].2 = body.to_string();
            let refresh = Secret::from_slice(b"R").unwrap();
            let err = authenticate(&api, OWN, &refresh, false).await.unwrap_err();
            assert!(matches!(err, Error::Auth(_)), "step {step}: {err}");
            assert!(err.to_string().contains(expect), "step {step}: {err}");
            assert_eq!(api.requests().len(), step + 1, "step {step}: the chain must stop at the failing call");
        }
    }

    #[tokio::test]
    async fn an_unapproved_public_app_is_named_as_bastion_mcs_not_yours() {
        let mut api = happy();
        api.replies[3].1 = 403;
        api.replies[3].2 = r#"{"path":"/authentication/login_with_xbox","errorMessage":"Invalid app registration, see https://aka.ms/AppRegInfo for more information"}"#.to_string();
        let refresh = Secret::from_slice(b"R").unwrap();
        let public = Client { id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", kind: ClientKind::Public };
        let err = authenticate(&api, public, &refresh, false).await.unwrap_err().to_string();
        assert!(err.contains("bastion-mc's application"), "{err}");
        assert!(!err.contains("your Azure application"), "{err}");
    }

    #[tokio::test]
    async fn game_pass_is_opt_in() {
        let mut api = happy();
        api.replies[4].2 = r#"{"items":[],"signature":"s","keyId":"1"}"#.into();
        let refresh = Secret::from_slice(b"R").unwrap();
        assert!(authenticate(&api, OWN, &refresh, false).await.is_err());
        let (s, _) = authenticate(&api, OWN, &refresh, true).await.unwrap();
        assert_eq!(s.license, minecraft::License::Unlisted);
    }

    #[tokio::test]
    async fn hostile_token_and_profile_values_are_rejected() {
        // A token that would need JSON escaping could smuggle fields into the next request.
        let mut api = happy();
        api.replies[1].2 = r#"{"Token":"XBL\",\"injected\":\"x","DisplayClaims":{"xui":[{"uhs":"1"}]}}"#.into();
        let refresh = Secret::from_slice(b"R").unwrap();
        assert!(authenticate(&api, OWN, &refresh, false).await.is_err());
        // A profile name is later a program argument and a log line.
        for bad in [
            r#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"--demo"}"#,
            r#"{"id":"not-a-uuid","name":"Notch"}"#,
            r#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"a b"}"#,
        ] {
            let mut api = happy();
            api.replies[5].2 = bad.to_string();
            assert!(authenticate(&api, OWN, &refresh, false).await.is_err(), "{bad}");
        }
    }
}
