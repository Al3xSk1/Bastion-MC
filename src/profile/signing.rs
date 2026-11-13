//! Ed25519 signatures over profile manifests, the trust store, and rollback protection.
//!
//! * Verification uses `verify_strict` (rejects non-canonical signatures and small-order
//!   keys); weak public keys are refused when the trust store is loaded.
//! * The signed message is `DOMAIN || manifest bytes`. The domain prefix means a signature
//!   made for any other purpose with the same key can never be replayed as a manifest
//!   signature, and vice versa.
//! * Trust anchors: if the root-owned directory `/etc/bastion-mc/trusted-keys.d` holds at
//!   least one key, *only* those keys count. Otherwise the per-user directory is used. Code
//!   that merely runs as the user therefore cannot add a signer on a machine whose admin
//!   installed system keys.

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use zeroize::Zeroize;

use crate::{
    error::{Context, Error, Result},
    paths::{Paths, SYSTEM_TRUSTED_KEYS, validate_id},
    secret::Secret,
    util::{self, HashAlg},
};

const DOMAIN: &[u8] = b"bastion-mc/profile-manifest/v1\0";
const SECRET_PREFIX: &str = "bastion-mc-secret-key:v1:";
const PUBLIC_PREFIX: &str = "ed25519:";
const SIG_HEADER: &str = "bastion-mc-signature:v1";

fn message(manifest: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(DOMAIN.len() + manifest.len());
    m.extend_from_slice(DOMAIN);
    m.extend_from_slice(manifest);
    m
}

// ---------------------------------------------------------------- keys

#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    /// `ed25519:<base64>`; anything after whitespace is a comment.
    pub fn parse(text: &str) -> Result<Self> {
        let token = text.split_whitespace().next().unwrap_or_default();
        let b64 = token
            .strip_prefix(PUBLIC_PREFIX)
            .ok_or_else(|| Error::Format(format!("public key must start with {PUBLIC_PREFIX:?}")))?;
        let bytes: [u8; 32] =
            util::b64_decode(b64)?.try_into().map_err(|_| Error::Format("public key is not 32 bytes".into()))?;
        let key =
            VerifyingKey::from_bytes(&bytes).map_err(|_| Error::Format("not a valid Ed25519 public key".into()))?;
        if key.is_weak() {
            return Err(Error::Policy("refusing a small-order (weak) Ed25519 public key".into()));
        }
        Ok(PublicKey(key))
    }

    pub fn encode(&self) -> String {
        format!("{PUBLIC_PREFIX}{}", util::b64_encode(self.0.as_bytes()))
    }

    /// Short fingerprint for messages: first 8 bytes of SHA-256(public key).
    pub fn id(&self) -> String {
        util::hex_encode(&util::hash_bytes(HashAlg::Sha256, self.0.as_bytes())[..8])
    }
}

impl std::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublicKey({})", self.id())
    }
}

/// A signing key. The seed is zeroized on drop by ed25519-dalek.
pub struct SecretKey(SigningKey);

impl SecretKey {
    pub fn generate() -> Result<Self> {
        let mut seed = util::random_bytes::<32>()?;
        let key = SigningKey::from_bytes(&seed);
        seed.zeroize();
        Ok(SecretKey(key))
    }

    /// Text form, held in locked memory until written out.
    pub fn encode(&self) -> Result<Secret> {
        let mut seed = self.0.to_bytes();
        let mut b64 = util::b64_encode(&seed);
        seed.zeroize();
        let mut out = Secret::with_capacity(SECRET_PREFIX.len() + b64.len() + 1)?;
        out.push(SECRET_PREFIX.as_bytes())?;
        out.push(b64.as_bytes())?;
        out.push(b"\n")?;
        b64.zeroize();
        Ok(out)
    }

    pub fn decode(text: &Secret) -> Result<Self> {
        let s = text.expose_str()?.trim();
        let b64 = s.strip_prefix(SECRET_PREFIX).ok_or_else(|| Error::Format("not a bastion-mc secret key".into()))?;
        let mut raw = util::b64_decode(b64)?;
        let result = <[u8; 32]>::try_from(raw.as_slice())
            .map(|mut seed| {
                let key = SigningKey::from_bytes(&seed);
                seed.zeroize();
                SecretKey(key)
            })
            .map_err(|_| Error::Format("secret key is not 32 bytes".into()));
        raw.zeroize();
        result
    }

    /// Load from a file that only its owner can read.
    pub fn load(path: &Path) -> Result<Self> {
        let meta = fs::metadata(path).ctx(|| format!("inspecting {}", path.display()))?;
        if meta.mode() & 0o077 != 0 {
            return Err(Error::Policy(format!(
                "{} is readable by group or others (mode {:o}); a signing key must be 0600",
                path.display(),
                meta.mode() & 0o7777
            )));
        }
        let mut raw = util::read_small_file(path, 4096)?;
        let secret = Secret::from_slice(&raw);
        raw.zeroize();
        Self::decode(&secret?)
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }

    pub fn sign(&self, manifest: &[u8]) -> SignatureFile {
        SignatureFile { key: self.public(), sig: self.0.sign(&message(manifest)).to_bytes() }
    }
}

// ---------------------------------------------------------------- detached signature

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureFile {
    /// Which key claims to have signed. Only a hint for lookup: it must be in the trust store.
    pub key: PublicKey,
    pub sig: [u8; 64],
}

impl SignatureFile {
    pub fn render(&self) -> String {
        format!("{SIG_HEADER}\nkey: {}\nsig: {}\n", self.key.encode(), util::b64_encode(&self.sig))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let bad = |m: &str| Error::Format(format!("signature file: {m}"));
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        if lines.next() != Some(SIG_HEADER) {
            return Err(bad("missing or unsupported header"));
        }
        let (mut key, mut sig) = (None, None);
        for line in lines {
            match line.split_once(':').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("key", v)) if key.is_none() => key = Some(PublicKey::parse(v)?),
                Some(("sig", v)) if sig.is_none() => {
                    sig =
                        Some(<[u8; 64]>::try_from(util::b64_decode(v)?).map_err(|_| bad("signature is not 64 bytes"))?)
                }
                _ => return Err(bad("unexpected line")),
            }
        }
        Ok(SignatureFile { key: key.ok_or_else(|| bad("no key line"))?, sig: sig.ok_or_else(|| bad("no sig line"))? })
    }
}

// ---------------------------------------------------------------- trust store

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustSource {
    System,
    User,
}

#[derive(Debug)]
pub struct TrustStore {
    pub source: TrustSource,
    pub dir: PathBuf,
    pub keys: Vec<(String, PublicKey)>,
}

impl TrustStore {
    pub fn load(paths: &Paths) -> Result<Self> {
        Self::load_from(Path::new(SYSTEM_TRUSTED_KEYS), &paths.user_trusted_keys())
    }

    pub fn load_from(system_dir: &Path, user_dir: &Path) -> Result<Self> {
        let system = read_key_dir(system_dir, TrustSource::System)?;
        if !system.is_empty() {
            return Ok(TrustStore { source: TrustSource::System, dir: system_dir.to_path_buf(), keys: system });
        }
        let user = read_key_dir(user_dir, TrustSource::User)?;
        Ok(TrustStore { source: TrustSource::User, dir: user_dir.to_path_buf(), keys: user })
    }

    /// Returns the name of the trusted key that produced a valid signature.
    pub fn verify(&self, manifest: &[u8], sig: &SignatureFile) -> Result<&str> {
        let (name, key) = self.keys.iter().find(|(_, k)| *k == sig.key).ok_or_else(|| {
            Error::Signature(format!(
                "signed by key {} which is not in the trust store {} ({} key(s) trusted)",
                sig.key.id(),
                self.dir.display(),
                self.keys.len()
            ))
        })?;
        key.0.verify_strict(&message(manifest), &Signature::from_bytes(&sig.sig)).map_err(|_| {
            Error::Signature(format!("signature by {name:?} ({}) does not match the manifest", key.id()))
        })?;
        Ok(name)
    }
}

fn read_key_dir(dir: &Path, source: TrustSource) -> Result<Vec<(String, PublicKey)>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(format!("reading trust store {}", dir.display()), e)),
    };
    // The directory itself must be tamper-proof against everyone but its owner class.
    match source {
        TrustSource::System => {
            util::require_root_owned(dir)?;
        }
        TrustSource::User => util::require_private(dir)?,
    }
    let mut keys = Vec::new();
    for entry in entries {
        let path = entry.ctx(|| format!("reading trust store {}", dir.display()))?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pub") {
            continue;
        }
        match source {
            TrustSource::System => {
                util::require_root_owned(&path)?;
            }
            TrustSource::User => util::require_private(&path)?,
        }
        let text = String::from_utf8(util::read_small_file(&path, 4096)?)
            .map_err(|_| Error::Format(format!("{} is not UTF-8", path.display())))?;
        let key = PublicKey::parse(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unnamed").to_string();
        keys.push((name, key));
    }
    keys.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(keys)
}

// ---------------------------------------------------------------- rollback protection

/// Refuse a manifest whose serial is lower than one already accepted for this instance, or
/// that reuses an accepted serial with different content. Records the accepted state.
pub fn check_and_record_serial(paths: &Paths, instance_id: &str, serial: u64, manifest: &[u8]) -> Result<()> {
    validate_id(instance_id)?;
    let dir = paths.serials();
    util::create_private_dir_all(&dir)?;
    let file = dir.join(instance_id);
    let digest = util::hex_encode(&util::hash_bytes(HashAlg::Sha256, manifest));
    if file.exists() {
        let text = String::from_utf8(util::read_small_file(&file, 512)?).unwrap_or_default();
        let mut parts = text.split_whitespace();
        let seen: u64 = parts.next().and_then(|s| s.parse().ok()).ok_or_else(|| {
            Error::Integrity(format!("{} is corrupt; refusing to guess the last accepted serial", file.display()))
        })?;
        let seen_digest = parts.next().unwrap_or_default();
        if serial < seen {
            return Err(Error::Policy(format!(
                "profile {instance_id:?}: manifest serial {serial} is older than the last accepted serial {seen} (rollback refused)"
            )));
        }
        if serial == seen && seen_digest != digest {
            return Err(Error::Policy(format!(
                "profile {instance_id:?}: manifest changed but serial is still {serial}; bump the serial and re-sign"
            )));
        }
        if serial == seen {
            return Ok(());
        }
    }
    util::write_atomic(&file, format!("{serial} {digest}\n").as_bytes(), 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join(format!("bastion-sig-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        fs::create_dir_all(&d).unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
        d
    }

    fn store_with(dir: &Path, name: &str, key: &PublicKey) {
        let f = dir.join(format!("{name}.pub"));
        fs::write(&f, format!("{} test key\n", key.encode())).unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn sign_verify_roundtrip_through_text_forms() {
        let sk = SecretKey::generate().unwrap();
        let sk2 = SecretKey::decode(&sk.encode().unwrap()).unwrap();
        assert_eq!(sk.public(), sk2.public());
        let manifest = br#"{"format":"bastion-mc/profile/v1"}"#;
        let sig = SignatureFile::parse(&sk.sign(manifest).render()).unwrap();

        let dir = tmpdir("rt");
        store_with(&dir, "me", &PublicKey::parse(&sk.public().encode()).unwrap());
        let store = TrustStore::load_from(Path::new("/nonexistent-system-dir"), &dir).unwrap();
        assert_eq!(store.source, TrustSource::User);
        assert_eq!(store.verify(manifest, &sig).unwrap(), "me");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn any_tampering_is_detected() {
        let sk = SecretKey::generate().unwrap();
        let dir = tmpdir("tamper");
        store_with(&dir, "me", &sk.public());
        let store = TrustStore::load_from(Path::new("/nonexistent-system-dir"), &dir).unwrap();
        let manifest = b"manifest-bytes".to_vec();
        let sig = sk.sign(&manifest);

        // one flipped bit in the manifest
        let mut m2 = manifest.clone();
        m2[3] ^= 1;
        assert!(matches!(store.verify(&m2, &sig), Err(Error::Signature(_))));
        // appended data
        let mut m3 = manifest.clone();
        m3.push(b'\n');
        assert!(store.verify(&m3, &sig).is_err());
        // one flipped bit in the signature
        let mut s2 = sig.clone();
        s2.sig[10] ^= 0x80;
        assert!(store.verify(&manifest, &s2).is_err());
        // signature without the domain prefix must not verify (domain separation)
        let raw = SignatureFile { key: sk.public(), sig: sk.0.sign(&manifest).to_bytes() };
        assert!(store.verify(&manifest, &raw).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn untrusted_signer_is_rejected_even_with_valid_signature() {
        let trusted = SecretKey::generate().unwrap();
        let attacker = SecretKey::generate().unwrap();
        let dir = tmpdir("untrusted");
        store_with(&dir, "me", &trusted.public());
        let store = TrustStore::load_from(Path::new("/nonexistent-system-dir"), &dir).unwrap();
        let manifest = b"m";
        let err = store.verify(manifest, &attacker.sign(manifest)).unwrap_err();
        assert!(matches!(err, Error::Signature(_)));
        // attacker claims to be the trusted key but signs with their own
        let forged = SignatureFile { key: trusted.public(), sig: attacker.sign(manifest).sig };
        assert!(store.verify(manifest, &forged).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn user_trust_dir_must_not_be_writable_by_others() {
        let sk = SecretKey::generate().unwrap();
        let dir = tmpdir("perms");
        store_with(&dir, "me", &sk.public());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(TrustStore::load_from(Path::new("/nonexistent-system-dir"), &dir).is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(dir.join("me.pub"), fs::Permissions::from_mode(0o666)).unwrap();
        assert!(TrustStore::load_from(Path::new("/nonexistent-system-dir"), &dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn weak_and_malformed_keys_are_refused() {
        // The identity point: a classic small-order key under which forged signatures verify.
        let mut identity = [0u8; 32];
        identity[0] = 1;
        assert!(PublicKey::parse(&format!("ed25519:{}", util::b64_encode(&identity))).is_err());
        assert!(PublicKey::parse("ed25519:AAAA").is_err());
        assert!(PublicKey::parse("rsa:AAAA").is_err());
        assert!(SignatureFile::parse("garbage").is_err());
        assert!(SignatureFile::parse("bastion-mc-signature:v1\nkey: ed25519:AAAA\n").is_err());
    }

    #[test]
    fn secret_key_file_must_be_private() {
        let dir = tmpdir("skperm");
        let sk = SecretKey::generate().unwrap();
        let f = dir.join("key");
        fs::write(&f, sk.encode().unwrap().expose()).unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(SecretKey::load(&f).is_err());
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(SecretKey::load(&f).unwrap().public(), sk.public());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rollback_and_serial_reuse_are_refused() {
        let root = tmpdir("serial");
        let paths = Paths {
            config: root.join("c"),
            data: root.join("d"),
            cache: root.join("k"),
            state: root.join("s"),
            runtime: None,
        };
        paths.ensure().unwrap();
        check_and_record_serial(&paths, "perf", 5, b"v5").unwrap();
        check_and_record_serial(&paths, "perf", 5, b"v5").unwrap(); // same again is fine
        assert!(check_and_record_serial(&paths, "perf", 4, b"v4").is_err(), "older serial = rollback");
        assert!(check_and_record_serial(&paths, "perf", 5, b"v5-changed").is_err(), "same serial, new content");
        check_and_record_serial(&paths, "perf", 6, b"v6").unwrap();
        assert!(check_and_record_serial(&paths, "perf", 5, b"v5").is_err());
        check_and_record_serial(&paths, "other", 1, b"x").unwrap(); // independent per instance
        fs::remove_dir_all(root).unwrap();
    }
}
