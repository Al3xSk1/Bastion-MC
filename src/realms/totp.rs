//! Time-based one-time passwords (RFC 6238 over RFC 4226): HMAC-SHA-1, 30-second steps, six
//! digits — what every authenticator app produces by default.
//!
//! The secret is created by `bastion-mc realms totp-setup`, kept in the token store (the
//! kernel keyring by default, never in a file of the launcher's own) and shown exactly once, on
//! the terminal: as an `otpauth://` URI and as base32. The launch that needs it reads it before
//! the game starts (the confined launcher can no longer reach the keyring) and keeps it in
//! locked memory; the game never sees it.
//!
//! A code is accepted for the current step and one step either side (clock drift), and only
//! once: a step that was used is never accepted again — in this session, nor in the next one
//! (the last used step is kept in the launcher's cache). Wrong guesses are bounded per prompt
//! and per session by the caller.

use sha1::{Digest, Sha1};

use crate::{
    error::{Error, Result},
    secret::Secret,
    util,
};

/// Seconds per step.
pub const STEP: u64 = 30;
/// Digits of a code.
pub const DIGITS: u32 = 6;
/// Bytes of a new secret (160 bits, as RFC 4226 recommends).
pub const SECRET_BYTES: usize = 20;

const BLOCK: usize = 64;

/// HMAC-SHA-1 (RFC 2104).
fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha1::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha1::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner);
    block.fill(0);
    outer.finalize().into()
}

/// The HOTP value of `counter` (RFC 4226 section 5.3), with `digits` digits.
pub fn hotp(key: &[u8], counter: u64, digits: u32) -> u32 {
    let mac = hmac_sha1(key, &counter.to_be_bytes());
    let offset = usize::from(mac[19] & 0x0f);
    let code = u32::from_be_bytes([mac[offset] & 0x7f, mac[offset + 1], mac[offset + 2], mac[offset + 3]]);
    code % 10u32.pow(digits)
}

/// The TOTP code at `unix_seconds`, as the digits an app shows.
pub fn code_at(key: &[u8], unix_seconds: u64, digits: u32) -> String {
    format!("{:0width$}", hotp(key, unix_seconds / STEP, digits), width = digits as usize)
}

/// Check `code` against the steps around `unix_seconds` (one either side). Returns the step it
/// matched, if it is newer than `last_used`; a step at or before `last_used` is never accepted
/// again. The comparison of the digits does not depend on where they differ.
pub fn verify(key: &[u8], code: &str, unix_seconds: u64, last_used: Option<u64>) -> Option<u64> {
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let now = unix_seconds / STEP;
    let mut matched = None;
    for step in [now.saturating_sub(1), now, now + 1] {
        let expected = format!("{:06}", hotp(key, step, DIGITS));
        if util::ct_eq(expected.as_bytes(), code.as_bytes()) && last_used.is_none_or(|used| step > used) {
            matched = Some(step);
        }
    }
    matched
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, without padding (what authenticator apps expect).
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    for chunk in bytes.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);
        let bits = u64::from_be_bytes([0, 0, 0, buf[0], buf[1], buf[2], buf[3], buf[4]]);
        let chars = (chunk.len() * 8).div_ceil(5);
        for i in 0..chars {
            out.push(BASE32[((bits >> (35 - 5 * i)) & 31) as usize] as char);
        }
    }
    out
}

/// Base32 as people type it: upper or lower case, with or without `=` padding and spaces.
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| !matches!(c, b' ' | b'=')) {
        let value = BASE32.iter().position(|b| *b == c.to_ascii_uppercase())? as u32;
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// A new secret from the system's random source.
pub fn new_secret() -> Result<Secret> {
    let bytes = util::random_bytes::<SECRET_BYTES>()?;
    Secret::from_slice(&bytes)
}

/// The `otpauth://` URI an authenticator app reads (as a QR code or pasted).
pub fn otpauth_uri(secret: &[u8], account: &str) -> String {
    let label: String =
        account.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(64).collect();
    format!(
        "otpauth://totp/bastion-mc:{label}?secret={}&issuer=bastion-mc&algorithm=SHA1&digits={DIGITS}&period={STEP}",
        base32_encode(secret)
    )
}

/// The last step that was used for an approval, kept between launches (`None`: none or
/// unreadable — then only this session's replays are refused, and that is said in the notes).
pub fn load_last_used(path: &std::path::Path) -> Option<u64> {
    let raw = util::read_small_file(path, 64).ok()?;
    std::str::from_utf8(&raw).ok()?.trim().parse().ok()
}

pub fn save_last_used(path: &std::path::Path, step: u64) -> Result<()> {
    util::write_atomic(path, format!("{step}\n").as_bytes(), 0o600)
}

/// Check a secret read back from the token store.
pub fn check_secret(secret: &Secret) -> Result<()> {
    if secret.len() < 10 || secret.len() > 64 {
        return Err(Error::Auth(
            "the stored Realms TOTP secret has an unexpected length; run `bastion-mc realms totp-setup --replace`"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238, appendix B (SHA-1 column): the 20-byte ASCII key "12345678901234567890",
    /// eight digits.
    #[test]
    fn rfc_6238_test_vectors() {
        let key = b"12345678901234567890";
        for (time, expected) in [
            (59u64, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ] {
            assert_eq!(code_at(key, time, 8), expected, "T = {time}");
        }
    }

    /// RFC 4226, appendix D: HOTP values for counters 0 to 9 with the same key, six digits.
    #[test]
    fn rfc_4226_test_vectors() {
        let key = b"12345678901234567890";
        let expected = [755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489];
        for (counter, value) in expected.iter().enumerate() {
            assert_eq!(hotp(key, counter as u64, 6), *value, "counter {counter}");
        }
        // HMAC-SHA-1 with a key longer than a block (RFC 2202, test case 6).
        let mac = hmac_sha1(&[0xaa; 80], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(util::hex_encode(&mac), "aa4ae5e15272d00e95705637ce8a3b55ed402112");
    }

    #[test]
    fn one_step_of_drift_and_never_the_same_step_twice() {
        let key = b"12345678901234567890";
        let t = 1_111_111_111;
        let now = code_at(key, t, 6);
        let before = code_at(key, t - STEP, 6);
        let after = code_at(key, t + STEP, 6);
        let far = code_at(key, t - 2 * STEP, 6);
        assert_eq!(verify(key, &now, t, None), Some(t / STEP));
        assert_eq!(verify(key, &before, t, None), Some(t / STEP - 1));
        assert_eq!(verify(key, &after, t, None), Some(t / STEP + 1));
        assert_eq!(verify(key, &far, t, None), None, "two steps away");
        // Replay: the step that was used, or an older one, is refused.
        assert_eq!(verify(key, &now, t, Some(t / STEP)), None);
        assert_eq!(verify(key, &before, t, Some(t / STEP)), None);
        assert_eq!(verify(key, &after, t, Some(t / STEP)), Some(t / STEP + 1), "a newer step is fine");
        for bad in ["", "12345", "1234567", "12345a", " 12345", "１２３４５６"] {
            assert_eq!(verify(key, bad, t, None), None, "{bad:?}");
        }
    }

    #[test]
    fn base32_and_the_otpauth_uri() {
        // RFC 4648 section 10.
        for (plain, encoded) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32_encode(plain.as_bytes()), encoded);
            assert_eq!(base32_decode(encoded).unwrap(), plain.as_bytes());
        }
        assert_eq!(base32_decode("mzxw 6ytb oi======").unwrap(), b"foobar");
        assert!(base32_decode("MZXW1").is_none(), "1 is not base32");
        let uri = otpauth_uri(b"12345678901234567890", "default");
        assert_eq!(
            uri,
            "otpauth://totp/bastion-mc:default?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=bastion-mc&algorithm=SHA1&digits=6&period=30"
        );
        let secret = new_secret().unwrap();
        assert_eq!(secret.len(), SECRET_BYTES);
        assert_ne!(secret.expose(), new_secret().unwrap().expose());
    }
}
