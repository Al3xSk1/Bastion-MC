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

pub async fn login_with_xbox(api: &impl Api, xsts: &XboxToken, kind: super::client::ClientKind) -> Result<Secret> {
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
            // The single most common failure with a newly registered client ID; whose it is
            // decides what the user can do about it.
            return Err(Error::Auth(super::client::not_approved_message(kind)));
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

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
    out
}

const SKIN_WIDTH: usize = 64;

/// A skin as an image: 64 pixels wide, 64 or 32 high, four bytes (R, G, B, A) per pixel.
struct SkinImage {
    height: usize,
    rgba: Vec<u8>,
}

fn not_a_skin(why: &str) -> Error {
    Error::Format(format!("not a usable skin file: {why}"))
}

/// Inflate a zlib stream that must produce exactly `expected` bytes and must end where
/// `data` ends. Never produces more than `expected + 1` bytes, whatever the stream claims.
fn inflate_exact(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    // One byte more than the image has room for: a stream that would go on is seen as such.
    let mut out = vec![0u8; expected + 1];
    let mut inflater = flate2::Decompress::new(true);
    let status = inflater
        .decompress(data, &mut out, flate2::FlushDecompress::Finish)
        .map_err(|_| not_a_skin("the pixel data is not a valid compressed stream"))?;
    let (read, written) = (inflater.total_in(), inflater.total_out());
    if status != flate2::Status::StreamEnd || written != expected as u64 {
        return Err(not_a_skin("the pixel data is not exactly the size of the image"));
    }
    if read != data.len() as u64 {
        return Err(not_a_skin("there is data behind the end of the pixel data"));
    }
    out.truncate(expected);
    Ok(out)
}

/// Undo PNG's per-row filters in place. `rows` holds, per row, one filter byte and then
/// `stride` bytes; `bpp` is the number of bytes per pixel.
fn unfilter(rows: &mut [u8], stride: usize, bpp: usize) -> Result<()> {
    let mut previous = vec![0u8; stride];
    for row in rows.chunks_exact_mut(stride + 1) {
        let (filter, line) = row.split_first_mut().expect("a row has a filter byte");
        for x in 0..stride {
            let left = if x >= bpp { line[x - bpp] } else { 0 };
            let (up, up_left) = (previous[x], if x >= bpp { previous[x - bpp] } else { 0 });
            let predicted = match *filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => {
                    // Paeth: whichever of the three neighbours is closest to left + up - up_left.
                    let (a, b, c) = (i16::from(left), i16::from(up), i16::from(up_left));
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        up_left
                    }
                }
                _ => return Err(not_a_skin("the pixel data uses a row filter that does not exist")),
            };
            line[x] = line[x].wrapping_add(predicted);
        }
        previous.copy_from_slice(line);
    }
    Ok(())
}

/// Read `raw` as a skin: a PNG of 64×64 or 64×32 pixels, 8 bits per channel, RGB, RGBA or
/// indexed colour, not interlaced, every chunk checksum correct, the chunks in the order
/// and number the format allows — and decode it to its pixels. Nothing of the file but the
/// pixels is in the result.
fn decode_skin_png(raw: &[u8]) -> Result<SkinImage> {
    let bad = not_a_skin;
    if raw.len() > 4 * MAX_SKIN_BYTES {
        return Err(bad("far too large for a 64×64 image"));
    }
    let mut rest = raw.strip_prefix(&PNG_SIGNATURE).ok_or_else(|| bad("not a PNG file"))?;
    // (height, colour type), the palette, the transparency chunk, the compressed pixels.
    let mut header: Option<(usize, u8)> = None;
    let mut palette: Option<&[u8]> = None;
    let mut transparency: Option<&[u8]> = None;
    let mut compressed: Vec<u8> = Vec::new();
    // Pixel data: not yet seen, being read (its chunks follow each other), or complete.
    let (mut data_started, mut data_closed, mut seen_end) = (false, false, false);
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
        rest = &rest[12 + length..];
        if header.is_none() && kind != b"IHDR" {
            return Err(bad("does not start with an image header"));
        }
        if data_started && kind != b"IDAT" {
            data_closed = true;
        }
        match kind {
            b"IHDR" => {
                if header.is_some() || length != 13 {
                    return Err(bad("malformed image header"));
                }
                let width = u32::from_be_bytes(data[0..4].try_into().expect("four bytes"));
                let height = u32::from_be_bytes(data[4..8].try_into().expect("four bytes"));
                let (depth, colour, compression, filter, interlace) = (data[8], data[9], data[10], data[11], data[12]);
                if width as usize != SKIN_WIDTH || !(height == 64 || height == 32) {
                    return Err(bad(&format!("skins are 64×64 (or 64×32) pixels, this is {width}×{height}")));
                }
                if depth != 8 || !matches!(colour, 2 | 3 | 6) || compression != 0 || filter != 0 || interlace != 0 {
                    return Err(bad("must be 8-bit RGB, RGBA or indexed colour, not interlaced"));
                }
                header = Some((height as usize, colour));
            }
            b"PLTE" => {
                if palette.is_some()
                    || transparency.is_some()
                    || data_started
                    || length == 0
                    || !length.is_multiple_of(3)
                    || length > 768
                {
                    return Err(bad("malformed palette"));
                }
                palette = Some(data);
            }
            b"tRNS" => {
                // Once, in front of the pixels, and only in the form the colour type defines:
                // one alpha value per palette entry, or one RGB colour (as three 16-bit
                // numbers) that stands for "transparent". RGBA has alpha of its own.
                let fits = match header.expect("the header comes first").1 {
                    3 => palette.is_some_and(|p| (1..=p.len() / 3).contains(&length)),
                    2 => length == 6,
                    _ => false,
                };
                if transparency.is_some() || data_started || !fits {
                    return Err(bad("malformed transparency chunk"));
                }
                transparency = Some(data);
            }
            b"IDAT" => {
                if data_closed {
                    return Err(bad("pixel data in separate places"));
                }
                data_started = true;
                compressed.extend_from_slice(data);
            }
            b"IEND" => {
                if length != 0 || !data_started {
                    return Err(bad("malformed end of image"));
                }
                seen_end = true;
            }
            // Everything else is optional by definition (or the image is not a plain PNG),
            // and none of it is kept.
            other if other[0] & 0x20 != 0 => {}
            _ => return Err(bad("contains a critical chunk this launcher does not know")),
        }
    }
    if !seen_end {
        return Err(bad("the image is incomplete"));
    }
    let (height, colour) = header.expect("an end without a header was refused");
    let bpp = match colour {
        2 => 3,
        3 => 1,
        _ => 4,
    };
    if colour == 3 && palette.is_none() {
        return Err(bad("indexed image without a palette"));
    }
    let stride = SKIN_WIDTH * bpp;
    let mut rows = inflate_exact(&compressed, height * (stride + 1))?;
    unfilter(&mut rows, stride, bpp)?;

    let mut rgba = Vec::with_capacity(height * SKIN_WIDTH * 4);
    for row in rows.chunks_exact(stride + 1) {
        for pixel in row[1..].chunks_exact(bpp) {
            match colour {
                6 => rgba.extend_from_slice(pixel),
                2 => {
                    // The one colour a transparency chunk names (its samples are 16 bits wide;
                    // for 8-bit images the high byte is zero).
                    let transparent = transparency
                        .is_some_and(|t| t[0] == 0 && t[2] == 0 && t[4] == 0 && [t[1], t[3], t[5]] == *pixel);
                    rgba.extend_from_slice(pixel);
                    rgba.push(if transparent { 0 } else { 255 });
                }
                _ => {
                    let (palette, index) = (palette.expect("checked above"), usize::from(pixel[0]));
                    let entry = palette
                        .get(index * 3..index * 3 + 3)
                        .ok_or_else(|| bad("a pixel refers to a colour that is not in the palette"))?;
                    rgba.extend_from_slice(entry);
                    rgba.push(transparency.and_then(|t| t.get(index).copied()).unwrap_or(255));
                }
            }
        }
    }
    Ok(SkinImage { height, rgba })
}

/// Write `image` as a PNG in one fixed form: 8-bit RGBA, not interlaced, no row filters,
/// one block of pixel data, no optional chunks. The same pixels always give the same file.
fn encode_skin_png(image: &SkinImage) -> Result<Vec<u8>> {
    use std::io::Write;
    let stride = SKIN_WIDTH * 4;
    let mut rows = Vec::with_capacity(image.height * (stride + 1));
    for row in image.rgba.chunks_exact(stride) {
        rows.push(0); // filter type "none"
        rows.extend_from_slice(row);
    }
    let mut deflater = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(9));
    let compressed = deflater
        .write_all(&rows)
        .and_then(|()| deflater.finish())
        .map_err(|e| Error::Other(format!("encoding the skin: {e}")))?;
    let mut header = (SKIN_WIDTH as u32).to_be_bytes().to_vec();
    header.extend_from_slice(&(image.height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = PNG_SIGNATURE.to_vec();
    out.extend(chunk(b"IHDR", &header));
    out.extend(chunk(b"IDAT", &compressed));
    out.extend(chunk(b"IEND", b""));
    Ok(out)
}

/// Check that `raw` is a Minecraft skin and return the same image as a new file.
///
/// The image is **decoded to its pixels and encoded again** ([`decode_skin_png`],
/// [`encode_skin_png`]). What is uploaded is that new file, so what leaves the machine is
/// the image and nothing else that was in the original: its height (64 or 32) and, for
/// every pixel, its four colour values. Not the text chunks (editors record software names,
/// comments, sometimes file paths), timestamps, EXIF or colour profiles; not private
/// chunks; not a palette, with whatever its unused entries held; not the way the original
/// was filtered and compressed; not bytes behind the pixel data or behind the end marker.
///
/// What does leave with the pixels, because it is part of them: the colour values of
/// pixels that are fully transparent. They are invisible in an image viewer, but the game
/// shows some of them (it ignores transparency on the skin's base layer), so changing them
/// would change the skin.
pub fn sanitize_skin_png(raw: &[u8]) -> Result<Vec<u8>> {
    let out = encode_skin_png(&decode_skin_png(raw)?)?;
    // (Cannot happen: 64×64 RGBA is 16 KiB before compression.)
    if out.len() > MAX_SKIN_BYTES {
        return Err(not_a_skin("larger than the 24 KiB Mojang accepts"));
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

    fn deflate(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    /// Filter `pixels` (rows of `stride` bytes) the way an encoder would, with the given
    /// filter type per row (cycled), and compress them: the content of an IDAT chunk.
    fn pixel_data(pixels: &[u8], stride: usize, bpp: usize, filters: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut previous = vec![0u8; stride];
        for (y, row) in pixels.chunks_exact(stride).enumerate() {
            let filter = filters[y % filters.len()];
            out.push(filter);
            for x in 0..stride {
                let left = if x >= bpp { row[x - bpp] } else { 0 };
                let (up, up_left) = (previous[x], if x >= bpp { previous[x - bpp] } else { 0 });
                let predicted = match filter {
                    0 => 0,
                    1 => left,
                    2 => up,
                    3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                    _ => {
                        let p = i16::from(left) + i16::from(up) - i16::from(up_left);
                        let d = |v: u8| (p - i16::from(v)).abs();
                        if d(left) <= d(up) && d(left) <= d(up_left) {
                            left
                        } else if d(up) <= d(up_left) {
                            up
                        } else {
                            up_left
                        }
                    }
                };
                out.push(row[x].wrapping_sub(predicted));
            }
            previous.copy_from_slice(row);
        }
        deflate(&out)
    }

    /// Pixels with some structure and some noise, `bpp` bytes each.
    fn pixels(height: usize, bpp: usize) -> Vec<u8> {
        (0..height * 64 * bpp).map(|i| ((i * 31 + i / 7) % 251) as u8).collect()
    }

    /// An ordinary RGBA skin as an editor writes it.
    fn rgba_skin(height: usize, extra: &[Vec<u8>]) -> (Vec<u8>, Vec<u8>) {
        let image = pixels(height, 4);
        let mut chunks = vec![header(64, height as u32, 6)];
        chunks.extend_from_slice(extra);
        chunks.push(chunk(b"IDAT", &pixel_data(&image, 256, 4, &[0, 1, 2, 3, 4])));
        chunks.push(chunk(b"IEND", b""));
        (png(&chunks), image)
    }

    #[test]
    fn crc32_matches_the_png_specification() {
        assert_eq!(crc32(&[b"IEND"]), 0xae42_6082);
        assert_eq!(crc32(&[b"123456789"]), 0xcbf4_3926);
    }

    #[test]
    fn a_skin_is_decoded_and_encoded_again_and_only_its_pixels_remain() {
        let text = chunk(b"tEXt", b"Software\0/home/alex/secret-project/editor");
        let time = chunk(b"tIME", &[7, 234, 11, 13, 12, 0, 0]);
        let private = chunk(b"prVt", b"anything at all");
        let (original, image) = rgba_skin(64, &[text, time, private]);
        let clean = sanitize_skin_png(&original).unwrap();
        // The same picture…
        let decoded = decode_skin_png(&clean).unwrap();
        assert_eq!((decoded.height, &decoded.rgba), (64, &image));
        // …in the launcher's one form: header, one block of pixel data, end. Nothing else.
        let mut kinds = Vec::new();
        let mut rest = &clean[8..];
        while !rest.is_empty() {
            let length = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            kinds.push(String::from_utf8_lossy(&rest[4..8]).into_owned());
            rest = &rest[12 + length..];
        }
        assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
        assert!(!clean.windows(4).any(|w| w == b"alex"), "the text chunk is gone");
        assert_eq!(sanitize_skin_png(&clean).unwrap(), clean, "its own output is a fixed point");
        // However the original was filtered, split or compressed: same pixels, same file.
        let split = pixel_data(&image, 256, 4, &[4, 4, 0]);
        let (a, b) = split.split_at(split.len() / 3);
        let other = png(&[header(64, 64, 6), chunk(b"IDAT", a), chunk(b"IDAT", b), chunk(b"IEND", b"")]);
        assert_eq!(sanitize_skin_png(&other).unwrap(), clean);

        // The legacy 64×32 layout.
        let (legacy, image) = rgba_skin(32, &[]);
        let decoded = decode_skin_png(&sanitize_skin_png(&legacy).unwrap()).unwrap();
        assert_eq!((decoded.height, decoded.rgba), (32, image));
    }

    #[test]
    fn rgb_and_indexed_skins_become_the_same_rgba_image() {
        // RGB, with one colour declared transparent.
        let rgb = pixels(64, 3);
        let key = [rgb[0], rgb[1], rgb[2]];
        let trns = chunk(b"tRNS", &[0, key[0], 0, key[1], 0, key[2]]);
        let file =
            png(&[header(64, 64, 2), trns, chunk(b"IDAT", &pixel_data(&rgb, 192, 3, &[1, 4])), chunk(b"IEND", b"")]);
        let decoded = decode_skin_png(&sanitize_skin_png(&file).unwrap()).unwrap();
        for (out, src) in decoded.rgba.as_chunks::<4>().0.iter().zip(rgb.as_chunks::<3>().0) {
            assert_eq!(&out[..3], src);
            assert_eq!(out[3], if *src == key { 0 } else { 255 });
        }
        assert_eq!(decoded.rgba[3], 0, "the first pixel has the transparent colour");

        // Indexed: four colours in use, the first one half transparent. The palette has room
        // for more entries than are used — and what those hold is not in the result.
        let mut palette = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
        palette.extend_from_slice(b"HIDDEN-IN-AN-UNUSED-PALETTE-ENTRY");
        let indices: Vec<u8> = (0..64 * 64).map(|i| (i % 4) as u8).collect();
        let file = png(&[
            header(64, 64, 3),
            chunk(b"PLTE", &palette),
            chunk(b"tRNS", &[128]),
            chunk(b"IDAT", &pixel_data(&indices, 64, 1, &[0, 2])),
            chunk(b"IEND", b""),
        ]);
        let clean = sanitize_skin_png(&file).unwrap();
        let decoded = decode_skin_png(&clean).unwrap();
        assert_eq!(&decoded.rgba[..16], [10, 20, 30, 128, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255]);
        let pixels_out = decode_skin_png(&clean).unwrap().rgba;
        assert!(!pixels_out.windows(6).any(|w| w == b"HIDDEN"), "an unused palette entry is not a pixel");
        assert!(!clean.windows(4).any(|w| w == b"PLTE" || w == b"tRNS"));
        // An index beyond the palette is not a colour.
        let mut beyond = indices.clone();
        beyond[100] = 200;
        let file = png(&[
            header(64, 64, 3),
            chunk(b"PLTE", &palette[..12]),
            chunk(b"IDAT", &pixel_data(&beyond, 64, 1, &[0])),
            chunk(b"IEND", b""),
        ]);
        assert!(sanitize_skin_png(&file).unwrap_err().to_string().contains("not in the palette"));
    }

    /// The ways a file that passes for an image could carry something else out. Each of
    /// these went through the earlier sanitiser unchanged (it kept `tRNS`, `PLTE` and `IDAT`
    /// chunks as they were).
    #[test]
    fn nothing_but_the_pixels_can_ride_along() {
        let (plain, image) = rgba_skin(64, &[]);
        let clean = sanitize_skin_png(&plain).unwrap();
        let secret = b"-----BEGIN OPENSSH PRIVATE KEY-----";
        let carries = |file: &[u8]| file.windows(secret.len()).any(|w| w == secret);
        let data = pixel_data(&image, 256, 4, &[0]);
        let end = chunk(b"IEND", b"");

        // Ninety "transparency" chunks of arbitrary bytes in an RGBA image.
        let mut chunks = vec![header(64, 64, 6)];
        for _ in 0..90 {
            let mut bytes = secret.to_vec();
            bytes.resize(256, 0x41);
            chunks.push(chunk(b"tRNS", &bytes));
        }
        chunks.extend([chunk(b"IDAT", &data), end.clone()]);
        assert!(sanitize_skin_png(&png(&chunks)).unwrap_err().to_string().contains("transparency"));
        // A "suggested palette" in an RGBA image: legal, and not a pixel. It is gone.
        let mut bytes = secret.to_vec();
        bytes.resize(36, 0);
        let file = png(&[header(64, 64, 6), chunk(b"PLTE", &bytes), chunk(b"IDAT", &data), end.clone()]);
        assert!(carries(&file));
        assert_eq!(sanitize_skin_png(&file).unwrap(), clean);
        // Bytes behind the compressed pixels, inside the pixel-data chunk…
        let file = png(&[header(64, 64, 6), chunk(b"IDAT", &[data.as_slice(), secret].concat()), end.clone()]);
        assert!(sanitize_skin_png(&file).unwrap_err().to_string().contains("behind the end of the pixel data"));
        // …or in a pixel-data chunk of their own.
        let file = png(&[header(64, 64, 6), chunk(b"IDAT", &data), chunk(b"IDAT", secret), end.clone()]);
        assert!(sanitize_skin_png(&file).is_err());
        // A zip archive (or anything else) where the pixels should be.
        let file = png(&[header(64, 64, 6), chunk(b"IDAT", b"PK\x03\x04 not an image at all"), end.clone()]);
        assert!(sanitize_skin_png(&file).unwrap_err().to_string().contains("not a valid compressed stream"));
        // More pixel rows than the image has, fewer, and a row filter that does not exist.
        let taller = pixel_data(&pixels(65, 4), 256, 4, &[0]);
        let shorter = pixel_data(&pixels(63, 4), 256, 4, &[0]);
        for wrong in [taller, shorter] {
            let file = png(&[header(64, 64, 6), chunk(b"IDAT", &wrong), end.clone()]);
            assert!(sanitize_skin_png(&file).unwrap_err().to_string().contains("exactly the size of the image"));
        }
        let mut rows = vec![0u8; 64 * 257];
        rows[257 * 3] = 5;
        let file = png(&[header(64, 64, 6), chunk(b"IDAT", &deflate(&rows)), end.clone()]);
        assert!(sanitize_skin_png(&file).unwrap_err().to_string().contains("row filter"));
        // A stream that claims to inflate to far more than an image: stopped at the image's size.
        let bomb = deflate(&vec![0u8; 8 << 20]);
        assert!(bomb.len() < 4 * MAX_SKIN_BYTES - 200, "small enough to be read at all: {}", bomb.len());
        let file = png(&[header(64, 64, 6), chunk(b"IDAT", &bomb), end.clone()]);
        assert!(sanitize_skin_png(&file).unwrap_err().to_string().contains("exactly the size of the image"));
        // Pixel data in two places with something in between.
        let (a, b) = data.split_at(data.len() / 2);
        let file =
            png(&[header(64, 64, 6), chunk(b"IDAT", a), chunk(b"tEXt", b"Comment\0x"), chunk(b"IDAT", b), end.clone()]);
        assert!(sanitize_skin_png(&file).unwrap_err().to_string().contains("separate places"));
        // Two palettes, a palette behind the transparency, transparency for the wrong type.
        let indices = vec![0u8; 64 * 64];
        let idat = chunk(b"IDAT", &pixel_data(&indices, 64, 1, &[0]));
        let (pal, trns) = (chunk(b"PLTE", &[1, 2, 3]), chunk(b"tRNS", &[9]));
        for chunks in [
            vec![header(64, 64, 3), pal.clone(), pal.clone(), idat.clone(), end.clone()],
            vec![header(64, 64, 3), trns.clone(), pal.clone(), idat.clone(), end.clone()],
            vec![header(64, 64, 3), pal.clone(), trns.clone(), trns.clone(), idat.clone(), end.clone()],
            vec![header(64, 64, 3), pal.clone(), chunk(b"tRNS", &[9, 9]), idat.clone(), end.clone()],
            vec![header(64, 64, 3), pal.clone(), idat.clone(), trns.clone(), end.clone()],
            vec![header(64, 64, 2), chunk(b"tRNS", &[0, 1, 0, 2]), idat.clone(), end.clone()],
        ] {
            assert!(sanitize_skin_png(&png(&chunks)).is_err());
        }
    }

    #[test]
    fn files_that_are_not_skins_are_refused() {
        let (good, image) = rgba_skin(64, &[]);
        let data = chunk(b"IDAT", &pixel_data(&image, 256, 4, &[0]));
        let end = chunk(b"IEND", b"");
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
            ("pixels of another colour type", png(&[header(64, 64, 2), data.clone(), end.clone()])),
            ("bad checksum", flipped),
            ("trailing data", trailing),
            ("too large", png(&[header(64, 64, 6), chunk(b"IDAT", &vec![0; 4 * MAX_SKIN_BYTES]), end.clone()])),
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
        let png = sanitize_skin_png(&rgba_skin(64, &[]).0).unwrap();
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
