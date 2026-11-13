//! Xbox Live user token and XSTS token.

use bytes::Bytes;
use hyper::Method;
use serde::Deserialize;

use super::{Api, ApiRequest, clean, require_json_safe};
use crate::{
    error::{Error, Result},
    net::http::header,
    secret::Secret,
};

const USER_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MINECRAFT_RELYING_PARTY: &str = "rp://api.minecraftservices.com/";

pub struct XboxToken {
    pub token: Secret,
    /// "User hash" claim, needed to build the Minecraft identity token. Not a secret.
    pub user_hash: String,
}

#[derive(Deserialize)]
struct TokenReply {
    #[serde(rename = "Token")]
    token: Secret,
    #[serde(rename = "DisplayClaims")]
    claims: Claims,
}

#[derive(Deserialize)]
struct Claims {
    xui: Vec<Xui>,
}

#[derive(Deserialize)]
struct Xui {
    uhs: String,
}

#[derive(Deserialize)]
struct XstsError {
    #[serde(rename = "XErr")]
    xerr: Option<u64>,
    #[serde(rename = "Message")]
    message: Option<String>,
}

fn json_headers() -> Result<crate::net::http::Headers> {
    Ok(vec![header("content-type", "application/json")?, header("accept", "application/json")?])
}

fn parse(reply: &Secret, what: &str) -> Result<XboxToken> {
    let parsed: TokenReply = serde_json::from_slice(reply.expose())
        .map_err(|_| Error::Auth(format!("{what}: unexpected response format")))?;
    let user_hash = parsed.claims.xui.into_iter().next().map(|x| x.uhs).unwrap_or_default();
    if user_hash.is_empty() || user_hash.len() > 64 || !user_hash.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(Error::Auth(format!("{what}: malformed user hash")));
    }
    require_json_safe(&parsed.token, what)?;
    Ok(XboxToken { token: parsed.token, user_hash })
}

/// Exchange the Microsoft access token for an Xbox Live user token.
pub async fn user_token(api: &impl Api, msa_access: &Secret) -> Result<XboxToken> {
    require_json_safe(msa_access, "Microsoft access token")?;
    let mut body = Vec::with_capacity(msa_access.len() + 200);
    body.extend_from_slice(br#"{"Properties":{"AuthMethod":"RPS","SiteName":"user.auth.xboxlive.com","RpsTicket":"d="#);
    body.extend_from_slice(msa_access.expose());
    body.extend_from_slice(br#""},"RelyingParty":"http://auth.xboxlive.com","TokenType":"JWT"}"#);
    let reply = api
        .call(ApiRequest {
            method: Method::POST,
            url: USER_AUTH_URL,
            headers: json_headers()?,
            body: Some(Bytes::from(body)),
        })
        .await?;
    if reply.status != 200 {
        return Err(Error::Auth(format!(
            "Xbox Live rejected the Microsoft token (HTTP {}). Check that the Azure app requests the `XboxLive.signin` scope.",
            reply.status
        )));
    }
    parse(&reply.body, "Xbox Live user token")
}

/// Exchange the user token for an XSTS token scoped to Minecraft services.
pub async fn xsts_token(api: &impl Api, user: &XboxToken) -> Result<XboxToken> {
    let mut body = Vec::with_capacity(user.token.len() + 200);
    body.extend_from_slice(br#"{"Properties":{"SandboxId":"RETAIL","UserTokens":[""#);
    body.extend_from_slice(user.token.expose());
    body.extend_from_slice(br#""]},"RelyingParty":""#);
    body.extend_from_slice(MINECRAFT_RELYING_PARTY.as_bytes());
    body.extend_from_slice(br#"","TokenType":"JWT"}"#);
    let reply = api
        .call(ApiRequest {
            method: Method::POST,
            url: XSTS_URL,
            headers: json_headers()?,
            body: Some(Bytes::from(body)),
        })
        .await?;
    if reply.status == 200 {
        return parse(&reply.body, "XSTS token");
    }
    let detail: Option<XstsError> = serde_json::from_slice(reply.body.expose()).ok();
    let reason = match detail.as_ref().and_then(|d| d.xerr) {
        Some(2148916227) => "this account is banned from Xbox Live".to_string(),
        Some(2148916233) => {
            "this Microsoft account has no Xbox profile yet; create one at https://www.xbox.com/live".to_string()
        }
        Some(2148916235) => "Xbox Live is not available in this account's country/region".to_string(),
        Some(2148916236 | 2148916237) => "this account needs adult verification on the Xbox website".to_string(),
        Some(2148916238) => "this is a child account; an adult must add it to a Microsoft Family first".to_string(),
        Some(code) => {
            format!("XSTS error {code} {}", clean(detail.as_ref().and_then(|d| d.message.as_deref()).unwrap_or("")))
        }
        None => format!("HTTP {}", reply.status),
    };
    Err(Error::Auth(format!("Xbox authorization failed: {reason}")))
}
