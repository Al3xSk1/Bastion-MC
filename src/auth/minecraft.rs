//! Minecraft services: access token, entitlement check, profile.

use bytes::Bytes;
use hyper::Method;
use serde::Deserialize;

use super::{Api, ApiRequest, clean, require_json_safe, xbox::XboxToken};
use crate::{
    error::{Error, Result},
    net::http::{Headers, header, sensitive_header},
    secret::Secret,
};

const LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const ENTITLEMENTS_URL: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum License {
    /// The store lists a Minecraft: Java Edition entitlement.
    Owned,
    /// No store entitlement, but a game profile exists (Xbox Game Pass). Only accepted when
    /// `auth.allow_game_pass = true`.
    Unlisted,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct LoginReply {
    access_token: Secret,
}

#[derive(Deserialize)]
struct ServiceError {
    #[serde(alias = "errorMessage")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct Entitlements {
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    name: String,
}

#[derive(Deserialize)]
struct ProfileReply {
    id: String,
    name: String,
}

fn bearer(token: &Secret) -> Result<Headers> {
    let mut value = Vec::with_capacity(token.len() + 7);
    value.extend_from_slice(b"Bearer ");
    value.extend_from_slice(token.expose());
    Ok(vec![sensitive_header("authorization", &value)?, header("accept", "application/json")?])
}

pub async fn login_with_xbox(api: &impl Api, xsts: &XboxToken) -> Result<Secret> {
    let mut body = Vec::with_capacity(xsts.token.len() + 96);
    body.extend_from_slice(br#"{"identityToken":"XBL3.0 x="#);
    body.extend_from_slice(xsts.user_hash.as_bytes());
    body.push(b';');
    body.extend_from_slice(xsts.token.expose());
    body.extend_from_slice(br#""}"#);
    let headers = vec![header("content-type", "application/json")?, header("accept", "application/json")?];
    let reply =
        api.call(ApiRequest { method: Method::POST, url: LOGIN_URL, headers, body: Some(Bytes::from(body)) }).await?;
    if reply.status != 200 {
        let detail =
            serde_json::from_slice::<ServiceError>(reply.body.expose()).ok().and_then(|e| e.error).unwrap_or_default();
        if detail.contains("Invalid app registration") {
            // The single most common failure with a self-provisioned client ID.
            return Err(Error::Auth(
                "Minecraft services refused your Azure application (\"Invalid app registration\"). New client IDs \
                 must be approved by Mojang before they may call the Minecraft API: see https://aka.ms/AppRegInfo \
                 and submit the review form linked there. Nothing in this launcher can bypass that."
                    .into(),
            ));
        }
        return Err(Error::Auth(format!("Minecraft login failed: HTTP {} {}", reply.status, clean(&detail))));
    }
    let parsed: LoginReply = serde_json::from_slice(reply.body.expose())
        .map_err(|_| Error::Auth("Minecraft login: unexpected response format".into()))?;
    require_json_safe(&parsed.access_token, "Minecraft access token")?;
    Ok(parsed.access_token)
}

pub async fn check_entitlement(api: &impl Api, token: &Secret, allow_game_pass: bool) -> Result<License> {
    let reply = api
        .call(ApiRequest { method: Method::GET, url: ENTITLEMENTS_URL, headers: bearer(token)?, body: None })
        .await?;
    if reply.status != 200 {
        return Err(Error::Auth(format!("entitlement check failed: HTTP {}", reply.status)));
    }
    let parsed: Entitlements = serde_json::from_slice(reply.body.expose())
        .map_err(|_| Error::Auth("entitlement check: unexpected response format".into()))?;
    if parsed.items.iter().any(|i| i.name == "game_minecraft" || i.name == "product_minecraft") {
        return Ok(License::Owned);
    }
    if allow_game_pass {
        // A Game Pass licence is not listed by this endpoint; the profile lookup that
        // follows still has to succeed.
        return Ok(License::Unlisted);
    }
    Err(Error::Auth(
        "this account does not own Minecraft: Java Edition (no store entitlement). If you play through Xbox Game \
         Pass, set `allow_game_pass = true` under [auth]."
            .into(),
    ))
}

pub async fn profile(api: &impl Api, token: &Secret) -> Result<Profile> {
    let reply =
        api.call(ApiRequest { method: Method::GET, url: PROFILE_URL, headers: bearer(token)?, body: None }).await?;
    match reply.status {
        200 => {}
        404 => {
            return Err(Error::Auth(
                "this account has no Minecraft profile (the game is not owned, or no player name was chosen yet)"
                    .into(),
            ));
        }
        other => return Err(Error::Auth(format!("profile lookup failed: HTTP {other}"))),
    }
    let parsed: ProfileReply = serde_json::from_slice(reply.body.expose())
        .map_err(|_| Error::Auth("profile lookup: unexpected response format".into()))?;
    // Both values become program arguments; accept only their documented shapes.
    let id_ok = parsed.id.len() == 32 && parsed.id.bytes().all(|b| b.is_ascii_hexdigit());
    let name_ok =
        (1..=25).contains(&parsed.name.len()) && parsed.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !id_ok || !name_ok {
        return Err(Error::Auth("profile lookup returned a malformed player id or name".into()));
    }
    Ok(Profile { id: parsed.id.to_ascii_lowercase(), name: parsed.name })
}
