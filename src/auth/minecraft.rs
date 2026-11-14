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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub skins: Vec<Skin>,
    pub capes: Vec<Cape>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Classic,
    Slim,
}

impl Variant {
    pub fn name(self) -> &'static str {
        match self {
            Variant::Classic => "classic",
            Variant::Slim => "slim",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skin {
    /// Mojang's id for this skin entry (UUID form).
    pub id: String,
    pub active: bool,
    pub variant: Variant,
    /// The texture's content hash: the last path segment of its `textures.minecraft.net`
    /// URL. This, not the URL, is what is kept and compared.
    pub texture: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cape {
    pub id: String,
    pub active: bool,
    /// Mojang's short name for the cape ("Migrator", "Vanilla", …).
    pub alias: String,
    pub texture: String,
}

impl Profile {
    pub fn active_skin(&self) -> Option<&Skin> {
        self.skins.iter().find(|s| s.active)
    }
    pub fn active_cape(&self) -> Option<&Cape> {
        self.capes.iter().find(|c| c.active)
    }
}

pub const TEXTURE_HOST: &str = "textures.minecraft.net";

/// Where a texture is fetched from, rebuilt from its hash (never a URL as received).
pub fn texture_url(texture: &str) -> String {
    format!("https://{TEXTURE_HOST}/texture/{texture}")
}

/// `http(s)://textures.minecraft.net/texture/<hex>` → `<hex>`. Anything else is refused:
/// the URL is server-provided text, and only its hash is ever used.
fn texture_hash(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let hash = rest.strip_prefix(TEXTURE_HOST)?.strip_prefix("/texture/")?;
    ((8..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| hash.to_string())
}

fn entry_id(id: &str) -> Option<String> {
    let plain: String = id.chars().filter(|c| *c != '-').collect();
    (plain.len() == 32 && plain.bytes().all(|b| b.is_ascii_hexdigit()) && id.len() <= 36)
        .then(|| plain.to_ascii_lowercase())
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
    #[serde(default)]
    skins: Vec<SkinReply>,
    #[serde(default)]
    capes: Vec<CapeReply>,
}

#[derive(Deserialize)]
struct SkinReply {
    id: String,
    state: String,
    url: String,
    #[serde(default)]
    variant: String,
}

#[derive(Deserialize)]
struct CapeReply {
    id: String,
    state: String,
    url: String,
    #[serde(default)]
    alias: String,
}

/// Most skin or cape entries a profile may list (real profiles have one skin and a handful
/// of capes).
const MAX_ENTRIES: usize = 256;

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
    parse_profile(reply.body.expose())
}

pub(crate) fn parse_profile(body: &[u8]) -> Result<Profile> {
    let malformed = |what: &str| Error::Auth(format!("profile lookup returned a malformed {what}"));
    let parsed: ProfileReply =
        serde_json::from_slice(body).map_err(|_| Error::Auth("profile lookup: unexpected response format".into()))?;
    // Id and name become program arguments; accept only their documented shapes.
    let id_ok = parsed.id.len() == 32 && parsed.id.bytes().all(|b| b.is_ascii_hexdigit());
    let name_ok =
        (1..=25).contains(&parsed.name.len()) && parsed.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !id_ok || !name_ok {
        return Err(malformed("player id or name"));
    }
    if parsed.skins.len() > MAX_ENTRIES || parsed.capes.len() > MAX_ENTRIES {
        return Err(malformed("skin or cape list"));
    }
    let active = |state: &str| match state {
        "ACTIVE" => Some(true),
        "INACTIVE" => Some(false),
        _ => None,
    };
    let mut skins = Vec::with_capacity(parsed.skins.len());
    for skin in &parsed.skins {
        let variant = match skin.variant.as_str() {
            "SLIM" => Variant::Slim,
            "CLASSIC" | "" => Variant::Classic,
            _ => return Err(malformed("skin entry")),
        };
        match (entry_id(&skin.id), active(&skin.state), texture_hash(&skin.url)) {
            (Some(id), Some(active), Some(texture)) => skins.push(Skin { id, active, variant, texture }),
            _ => return Err(malformed("skin entry")),
        }
    }
    let mut capes = Vec::with_capacity(parsed.capes.len());
    for cape in &parsed.capes {
        let alias_ok = cape.alias.len() <= 48
            && cape.alias.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'-' | b'_' | b'\'' | b'.'));
        match (entry_id(&cape.id), active(&cape.state), texture_hash(&cape.url)) {
            (Some(id), Some(active), Some(texture)) if alias_ok => {
                capes.push(Cape { id, active, alias: cape.alias.clone(), texture })
            }
            _ => return Err(malformed("cape entry")),
        }
    }
    if skins.iter().filter(|s| s.active).count() > 1 || capes.iter().filter(|c| c.active).count() > 1 {
        return Err(malformed("skin or cape list (more than one active entry)"));
    }
    Ok(Profile { id: parsed.id.to_ascii_lowercase(), name: parsed.name, skins, capes })
}

// ---------------------------------------------------------------- changing the profile

const ACTIVE_CAPE_URL: &str = "https://api.minecraftservices.com/minecraft/profile/capes/active";
const SKINS_URL: &str = "https://api.minecraftservices.com/minecraft/profile/skins";
const ACTIVE_SKIN_URL: &str = "https://api.minecraftservices.com/minecraft/profile/skins/active";

fn changed(what: &str, status: u16) -> Result<()> {
    match status {
        200 | 204 => Ok(()),
        401 | 403 => Err(Error::Auth(format!("{what}: Minecraft services refused the request (HTTP {status})"))),
        429 => Err(Error::Auth(format!("{what}: too many requests; try again in a minute (HTTP 429)"))),
        other => Err(Error::Auth(format!("{what} failed: HTTP {other}"))),
    }
}

/// Wear `cape_id` (an id from the profile's own cape list), or no cape at all.
pub async fn set_active_cape(api: &impl Api, token: &Secret, cape_id: Option<&str>) -> Result<()> {
    let mut headers = bearer(token)?;
    let (method, body) = match cape_id {
        Some(id) => {
            let id = entry_id(id).ok_or_else(|| Error::Usage("not a cape id".into()))?;
            // The API wants the dashed form.
            let dashed = format!("{}-{}-{}-{}-{}", &id[..8], &id[8..12], &id[12..16], &id[16..20], &id[20..]);
            headers.push(header("content-type", "application/json")?);
            (Method::PUT, Some(Bytes::from(format!(r#"{{"capeId":"{dashed}"}}"#))))
        }
        None => (Method::DELETE, None),
    };
    let reply = api.call(ApiRequest { method, url: ACTIVE_CAPE_URL, headers, body }).await?;
    changed("changing the cape", reply.status)
}

/// Back to the default skin.
pub async fn reset_skin(api: &impl Api, token: &Secret) -> Result<()> {
    let reply = api
        .call(ApiRequest { method: Method::DELETE, url: ACTIVE_SKIN_URL, headers: bearer(token)?, body: None })
        .await?;
    changed("resetting the skin", reply.status)
}

/// Upload a skin. `png` must have been through [`sanitize_skin_png`].
pub async fn upload_skin(api: &impl Api, token: &Secret, png: &[u8], variant: Variant) -> Result<()> {
    let boundary = format!("bastion-{}", crate::util::hex_encode(&crate::util::random_bytes::<16>()?));
    let mut body = Vec::with_capacity(png.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"variant\"\r\n\r\n{}\r\n--{boundary}\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"skin.png\"\r\nContent-Type: image/png\r\n\r\n",
            variant.name()
        )
        .as_bytes(),
    );
    body.extend_from_slice(png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut headers = bearer(token)?;
    headers.push(header("content-type", &format!("multipart/form-data; boundary={boundary}"))?);
    let reply =
        api.call(ApiRequest { method: Method::POST, url: SKINS_URL, headers, body: Some(Bytes::from(body)) }).await?;
    changed("uploading the skin", reply.status)
}

// ---------------------------------------------------------------- skin files

/// Mojang's limit for an uploaded skin.
pub const MAX_SKIN_BYTES: usize = 24 * 1024;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in parts.iter().flat_map(|p| p.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// Check that `raw` is a Minecraft skin — a 64×64 or 64×32 PNG, 8 bits per channel, not
/// interlaced, every chunk checksum correct — and rebuild it from only the chunks an image
/// needs (`IHDR`, `PLTE`, `tRNS`, `IDAT`, `IEND`).
///
/// What is uploaded is the rebuilt file, so nothing else that was in the original leaves
/// the machine: no text chunks (editors record software names, comments, sometimes file
/// paths), no timestamps, no EXIF, no colour profiles, no private chunks, no bytes after
/// the end marker. The pixel data itself is passed through as it is, not decoded.
pub fn sanitize_skin_png(raw: &[u8]) -> Result<Vec<u8>> {
    let bad = |why: &str| Error::Format(format!("not a usable skin file: {why}"));
    if raw.len() > 4 * MAX_SKIN_BYTES {
        return Err(bad("far too large for a 64×64 image"));
    }
    let mut rest = raw.strip_prefix(&PNG_SIGNATURE).ok_or_else(|| bad("not a PNG file"))?;
    let mut out = PNG_SIGNATURE.to_vec();
    let (mut seen_header, mut seen_data, mut seen_end, mut palette) = (false, false, false, false);
    let mut indexed = false;
    while !rest.is_empty() {
        if seen_end {
            return Err(bad("data after the end of the image"));
        }
        if rest.len() < 12 {
            return Err(bad("truncated chunk"));
        }
        let length = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        if length > rest.len() - 12 {
            return Err(bad("truncated chunk"));
        }
        let (kind, data) = (&rest[4..8], &rest[8..8 + length]);
        let crc = u32::from_be_bytes(rest[8 + length..12 + length].try_into().expect("four bytes"));
        if crc != crc32(&[kind, data]) {
            return Err(bad("chunk checksum mismatch (damaged file)"));
        }
        let chunk = &rest[..12 + length];
        rest = &rest[12 + length..];
        if !seen_header && kind != b"IHDR" {
            return Err(bad("does not start with an image header"));
        }
        match kind {
            b"IHDR" => {
                if seen_header || length != 13 {
                    return Err(bad("malformed image header"));
                }
                seen_header = true;
                let width = u32::from_be_bytes(data[0..4].try_into().expect("four bytes"));
                let height = u32::from_be_bytes(data[4..8].try_into().expect("four bytes"));
                let (depth, colour, compression, filter, interlace) = (data[8], data[9], data[10], data[11], data[12]);
                if width != 64 || !(height == 64 || height == 32) {
                    return Err(bad(&format!("skins are 64×64 (or 64×32) pixels, this is {width}×{height}")));
                }
                if depth != 8 || !matches!(colour, 2 | 3 | 6) || compression != 0 || filter != 0 || interlace != 0 {
                    return Err(bad("must be 8-bit RGB, RGBA or indexed colour, not interlaced"));
                }
                indexed = colour == 3;
                out.extend_from_slice(chunk);
            }
            b"PLTE" => {
                if palette || seen_data || length == 0 || !length.is_multiple_of(3) || length > 768 {
                    return Err(bad("malformed palette"));
                }
                palette = true;
                out.extend_from_slice(chunk);
            }
            b"tRNS" => {
                if seen_data || length > 256 {
                    return Err(bad("malformed transparency chunk"));
                }
                out.extend_from_slice(chunk);
            }
            b"IDAT" => {
                if indexed && !palette {
                    return Err(bad("indexed image without a palette"));
                }
                seen_data = true;
                out.extend_from_slice(chunk);
            }
            b"IEND" => {
                if length != 0 || !seen_data {
                    return Err(bad("malformed end of image"));
                }
                seen_end = true;
                out.extend_from_slice(chunk);
            }
            // Everything else is optional by definition (or the image is not a plain PNG).
            other if other[0] & 0x20 != 0 => {}
            _ => return Err(bad("contains a critical chunk this launcher does not know")),
        }
    }
    if !seen_end {
        return Err(bad("the image is incomplete"));
    }
    if out.len() > MAX_SKIN_BYTES {
        return Err(bad("larger than the 24 KiB Mojang accepts"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch",
      "skins":[{"id":"6a6e65e5-76dd-4c3c-a625-162924514568","state":"ACTIVE",
                "url":"http://textures.minecraft.net/texture/1a4af718455d4aab528e7a61f86fa25e6a369d1768dcb13f7df319a713eb810b",
                "variant":"SLIM","textureKey":"1a4af7"}],
      "capes":[{"id":"5af20372-79e0-4e1f-80f8-6bd8e3135995","state":"ACTIVE",
                "url":"http://textures.minecraft.net/texture/2340c0e03dd24a11b15a8b33c2a7e9e32abb2051b2481d0ba7defd635ca7a933","alias":"Migrator"},
               {"id":"2dfb7ba3-2bd3-4dbc-9e9e-6b1a0f0e7a37","state":"INACTIVE",
                "url":"https://textures.minecraft.net/texture/f9a76537647989f9a0b6d001e320dac591c359e9e61a31f4ce11c88f207f0ad4","alias":"Vanilla"}],
      "profileActions":{}}"#;

    #[test]
    fn the_profile_is_reduced_to_ids_states_and_texture_hashes() {
        let p = parse_profile(FULL.as_bytes()).unwrap();
        assert_eq!(p.name, "Notch");
        let skin = p.active_skin().unwrap();
        assert_eq!(skin.variant, Variant::Slim);
        assert_eq!(skin.id, "6a6e65e576dd4c3ca625162924514568");
        assert_eq!(skin.texture, "1a4af718455d4aab528e7a61f86fa25e6a369d1768dcb13f7df319a713eb810b");
        assert_eq!(p.active_cape().unwrap().alias, "Migrator");
        assert_eq!(p.capes.len(), 2);
        assert_eq!(
            texture_url(&p.capes[1].texture),
            "https://textures.minecraft.net/texture/f9a76537647989f9a0b6d001e320dac591c359e9e61a31f4ce11c88f207f0ad4"
        );
        // A profile without the lists (older replies) is still a profile.
        let bare = parse_profile(br#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch"}"#).unwrap();
        assert!(bare.skins.is_empty() && bare.active_cape().is_none());
    }

    #[test]
    fn hostile_profile_entries_are_refused() {
        let base = |skin: &str, cape: &str| {
            format!(r#"{{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch","skins":[{skin}],"capes":[{cape}]}}"#)
        };
        let good_skin = r#"{"id":"6a6e65e5-76dd-4c3c-a625-162924514568","state":"ACTIVE","url":"http://textures.minecraft.net/texture/1a4af718","variant":"CLASSIC"}"#;
        let good_cape = r#"{"id":"5af20372-79e0-4e1f-80f8-6bd8e3135995","state":"ACTIVE","url":"http://textures.minecraft.net/texture/2340c0e0","alias":"Migrator"}"#;
        assert!(parse_profile(base(good_skin, good_cape).as_bytes()).is_ok());
        for bad_skin in [
            good_skin.replace("textures.minecraft.net", "textures.minecraft.net.evil.example"),
            good_skin.replace("http://textures.minecraft.net/texture/", "https://evil.example/texture/"),
            good_skin.replace("/texture/1a4af718", "/texture/../../x"),
            good_skin.replace("/texture/1a4af718", "/texture/1a4af718?redirect=1"),
            good_skin.replace("ACTIVE", "MAYBE"),
            good_skin.replace("CLASSIC", "WIDE"),
            good_skin.replace("6a6e65e5-76dd-4c3c-a625-162924514568", "not-an-id"),
        ] {
            assert!(parse_profile(base(&bad_skin, good_cape).as_bytes()).is_err(), "{bad_skin}");
        }
        for bad_cape in [
            good_cape.replace("Migrator", "\\u001b[2J"),
            good_cape.replace("Migrator", &"x".repeat(60)),
            good_cape.replace("2340c0e0", "2340C0E0zz"),
        ] {
            assert!(parse_profile(base(good_skin, &bad_cape).as_bytes()).is_err(), "{bad_cape}");
        }
        // Two active entries cannot be told apart from a confused or hostile reply.
        let two = base(&format!("{good_skin},{good_skin}"), good_cape);
        assert!(parse_profile(two.as_bytes()).is_err());
    }

    pub(crate) fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
        c
    }

    fn header(width: u32, height: u32, colour: u8) -> Vec<u8> {
        let mut d = width.to_be_bytes().to_vec();
        d.extend_from_slice(&height.to_be_bytes());
        d.extend_from_slice(&[8, colour, 0, 0, 0]);
        chunk(b"IHDR", &d)
    }

    fn png(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut f = PNG_SIGNATURE.to_vec();
        for c in chunks {
            f.extend_from_slice(c);
        }
        f
    }

    #[test]
    fn crc32_matches_the_png_specification() {
        assert_eq!(crc32(&[b"IEND"]), 0xae42_6082);
        assert_eq!(crc32(&[b"123456789"]), 0xcbf4_3926);
    }

    #[test]
    fn a_skin_is_rebuilt_without_its_metadata() {
        let text = chunk(b"tEXt", b"Software\0/home/alex/secret-project/editor");
        let time = chunk(b"tIME", &[7, 234, 11, 13, 12, 0, 0]);
        let private = chunk(b"prVt", b"anything");
        let data = chunk(b"IDAT", b"compressed-pixels");
        let original = png(&[header(64, 64, 6), text, time, data.clone(), private, data.clone(), chunk(b"IEND", b"")]);
        let clean = sanitize_skin_png(&original).unwrap();
        assert_eq!(clean, png(&[header(64, 64, 6), data.clone(), data, chunk(b"IEND", b"")]));
        assert!(!clean.windows(4).any(|w| w == b"alex"), "the text chunk is gone");
        assert_eq!(sanitize_skin_png(&clean).unwrap(), clean, "idempotent");
        // The legacy 64×32 layout and indexed colour are accepted.
        let pal = chunk(b"PLTE", &[0; 6]);
        let legacy =
            png(&[header(64, 32, 3), pal, chunk(b"tRNS", &[0, 255]), chunk(b"IDAT", b"x"), chunk(b"IEND", b"")]);
        assert_eq!(sanitize_skin_png(&legacy).unwrap(), legacy);
    }

    #[test]
    fn files_that_are_not_skins_are_refused() {
        let data = chunk(b"IDAT", b"x");
        let end = chunk(b"IEND", b"");
        let good = png(&[header(64, 64, 6), data.clone(), end.clone()]);
        assert!(sanitize_skin_png(&good).is_ok());
        let mut flipped = good.clone();
        flipped[20] ^= 1; // inside IHDR: the checksum no longer matches
        let mut trailing = good.clone();
        trailing.extend_from_slice(b"PK\x03\x04 a zip appended to the image");
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("not a PNG", b"GIF89a".to_vec()),
            ("empty", Vec::new()),
            ("wrong size", png(&[header(128, 128, 6), data.clone(), end.clone()])),
            ("16-bit", {
                let mut d = 64u32.to_be_bytes().to_vec();
                d.extend_from_slice(&64u32.to_be_bytes());
                d.extend_from_slice(&[16, 6, 0, 0, 0]);
                png(&[chunk(b"IHDR", &d), data.clone(), end.clone()])
            }),
            ("greyscale", png(&[header(64, 64, 0), data.clone(), end.clone()])),
            ("no header first", png(&[data.clone(), header(64, 64, 6), end.clone()])),
            ("two headers", png(&[header(64, 64, 6), header(64, 64, 6), data.clone(), end.clone()])),
            ("no pixel data", png(&[header(64, 64, 6), end.clone()])),
            ("no end", png(&[header(64, 64, 6), data.clone()])),
            ("unknown critical chunk", png(&[header(64, 64, 6), chunk(b"CgBI", b""), data.clone(), end.clone()])),
            ("indexed without palette", png(&[header(64, 64, 3), data.clone(), end.clone()])),
            ("bad checksum", flipped),
            ("trailing data", trailing),
            ("too large", png(&[header(64, 64, 6), chunk(b"IDAT", &vec![0; MAX_SKIN_BYTES]), end.clone()])),
            ("truncated", good[..good.len() - 5].to_vec()),
            ("length beyond the file", {
                let mut f = png(&[header(64, 64, 6)]);
                f.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, b'I', b'D', b'A', b'T', 0, 0, 0, 0]);
                f
            }),
        ];
        for (what, file) in cases {
            assert!(sanitize_skin_png(&file).is_err(), "{what}");
        }
        for cut in 0..good.len() {
            assert!(sanitize_skin_png(&good[..cut]).is_err(), "prefix of {cut} bytes");
        }
    }

    #[tokio::test]
    async fn profile_changes_send_exactly_the_documented_requests() {
        use crate::auth::testing::Scripted;
        let api = Scripted::new(vec![
            ("/capes/active", 200, "{}"),
            ("/skins/active", 204, ""),
            ("/profile/skins", 200, "{}"),
        ]);
        let token = Secret::from_slice(b"MC-TOKEN").unwrap();
        set_active_cape(&api, &token, Some("5af2037279e04e1f80f86bd8e3135995")).await.unwrap();
        set_active_cape(&api, &token, None).await.unwrap();
        reset_skin(&api, &token).await.unwrap();
        let png = png(&[header(64, 64, 6), chunk(b"IDAT", b"x"), chunk(b"IEND", b"")]);
        upload_skin(&api, &token, &png, Variant::Slim).await.unwrap();
        let reqs = api.requests();
        assert_eq!(reqs[0].0, ACTIVE_CAPE_URL);
        assert_eq!(reqs[0].2, r#"{"capeId":"5af20372-79e0-4e1f-80f8-6bd8e3135995"}"#);
        assert_eq!((reqs[1].0.as_str(), reqs[1].2.as_str()), (ACTIVE_CAPE_URL, ""));
        assert_eq!(reqs[2].0, ACTIVE_SKIN_URL);
        assert_eq!(reqs[3].0, SKINS_URL);
        assert!(reqs[3].2.contains("name=\"variant\"\r\n\r\nslim\r\n"));
        assert!(reqs[3].2.contains("Content-Type: image/png\r\n\r\n\u{fffd}PNG"));
        assert!(reqs.iter().all(|r| r.1 == "Bearer MC-TOKEN"));
        // An id that is not an id never reaches the wire.
        assert!(set_active_cape(&api, &token, Some("\"}],\"x\":\"")).await.is_err());
        assert_eq!(api.requests().len(), 4);
        // Refusals are errors.
        let denied = Scripted::new(vec![("/capes/active", 403, "{}")]);
        assert!(set_active_cape(&denied, &token, None).await.is_err());
    }
}
