//! Where the Microsoft refresh token rests between launches.
//!
//! * `keyutils` (default): the kernel keyring, via the `keyring-core` ecosystem. RAM only —
//!   never written to disk, gone at reboot (and after the persistent-keyring expiry, three
//!   days by default, if you stay logged out).
//! * `age`: a file encrypted by the external `age` tool. With an identity backed by a
//!   hardware token (`age-plugin-yubikey`, `age-plugin-tpm`) every decryption needs the
//!   device — the only option here that also resists other software running as you.
//! * `none`: nothing is stored; sign in on every launch.
//!
//! What the kernel keyring does and does not give you (see docs/THREAT_MODEL.md): other
//! *users* cannot read it, it never touches disk, and the sandboxed game cannot reach it
//! (fresh session keyring + seccomp). Other *processes running as you in the same login
//! session* can read it; no user-space launcher can change that.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use keyring_core::api::CredentialStoreApi;
use zeroize::Zeroize;

use crate::{
    config::{AgeConfig, AuthConfig, TokenStore as Kind},
    error::{Context, Error, Result},
    paths::{Paths, validate_id},
    secret::Secret,
    util,
};

const MAX_TOKEN_BYTES: usize = 16 * 1024;
/// A vault blob is a token plus age framing: a few kilobytes. The cap is kept below the
/// kernel pipe buffer (64 KiB) so that writing all of it to `age` before reading any output
/// can never block, however the file on disk was grown.
const MAX_VAULT_BYTES: u64 = 32 * 1024;

pub enum TokenStore {
    Keyutils,
    Age(AgeVault),
    None,
}

pub struct AgeVault {
    program: PathBuf,
    recipients_file: PathBuf,
    identity_file: PathBuf,
    /// Accounts with keys of their own: label → (recipients, identity).
    per_account: HashMap<String, (PathBuf, PathBuf)>,
    dir: PathBuf,
}

impl TokenStore {
    pub fn open(cfg: &AuthConfig, paths: &Paths) -> Result<Self> {
        Ok(match cfg.token_store {
            Kind::Keyutils => TokenStore::Keyutils,
            Kind::None => TokenStore::None,
            Kind::Age => TokenStore::Age(AgeVault::new(&cfg.age, paths.vault(), true)?),
        })
    }

    pub fn describe(&self) -> &'static str {
        match self {
            TokenStore::Keyutils => "kernel keyring (RAM only, cleared at reboot)",
            TokenStore::Age(_) => "age-encrypted vault file",
            TokenStore::None => "not stored (sign in on every launch)",
        }
    }

    /// Is a refresh token stored for `account`? For the age vault this only looks for the
    /// file: answering must never cost a decryption (and a hardware-key touch).
    pub fn has(&self, account: &str) -> Result<bool> {
        validate_id(account)?;
        match self {
            TokenStore::None => Ok(false),
            TokenStore::Keyutils => Ok(keyutils_load(account)?.is_some()),
            TokenStore::Age(v) => Ok(v.file(account).is_file()),
        }
    }

    pub fn load(&self, account: &str) -> Result<Option<Secret>> {
        validate_id(account)?;
        match self {
            TokenStore::None => Ok(None),
            TokenStore::Keyutils => keyutils_load(account),
            TokenStore::Age(v) => v.load(account),
        }
    }

    pub fn save(&self, account: &str, token: &Secret) -> Result<()> {
        validate_id(account)?;
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return Err(Error::Auth("refusing to store an empty or oversized token".into()));
        }
        match self {
            TokenStore::None => Ok(()),
            TokenStore::Keyutils => keyutils_entry(account)?.set_secret(token.expose()).map_err(keyring_err),
            TokenStore::Age(v) => v.save(account, token),
        }
    }

    pub fn delete(&self, account: &str) -> Result<()> {
        validate_id(account)?;
        match self {
            TokenStore::None => Ok(()),
            TokenStore::Keyutils => match keyutils_entry(account)?.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(e) => Err(keyring_err(e)),
            },
            TokenStore::Age(v) => v.delete(account),
        }
    }
}

// ---------------------------------------------------------------- kernel keyring

fn keyring_err(e: keyring_core::Error) -> Error {
    Error::Auth(format!("kernel keyring: {e}"))
}

fn keyutils_entry(account: &str) -> Result<keyring_core::Entry> {
    let store = linux_keyutils_keyring_store::Store::new().map_err(keyring_err)?;
    let description = format!("bastion-mc:msa-refresh:{account}");
    let modifiers = HashMap::from([("description", description.as_str())]);
    store.build("bastion-mc", account, Some(&modifiers)).map_err(keyring_err)
}

fn keyutils_load(account: &str) -> Result<Option<Secret>> {
    match keyutils_entry(account)?.get_secret() {
        Ok(mut bytes) => {
            let secret = Secret::from_slice(&bytes);
            bytes.zeroize();
            Ok(Some(secret?))
        }
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(e) => Err(keyring_err(e)),
    }
}

// ---------------------------------------------------------------- age vault

impl AgeVault {
    /// `strict` requires the `age` binary to be a root-owned system file (always the case
    /// outside tests).
    pub fn new(cfg: &AgeConfig, dir: PathBuf, strict: bool) -> Result<Self> {
        let missing = |what: &str| Error::Config(format!("auth.age.{what} is required for the age token store"));
        let program = if strict { util::require_root_owned(&cfg.program)? } else { cfg.program.clone() };
        Ok(AgeVault {
            program,
            recipients_file: cfg.recipients_file.clone().ok_or_else(|| missing("recipients_file"))?,
            identity_file: cfg.identity_file.clone().ok_or_else(|| missing("identity_file"))?,
            per_account: cfg
                .accounts
                .iter()
                .map(|(label, keys)| (label.clone(), (keys.recipients_file.clone(), keys.identity_file.clone())))
                .collect(),
            dir,
        })
    }

    /// The recipients and identity files for `account`: its own, or the common ones.
    fn keys(&self, account: &str) -> (&Path, &Path) {
        match self.per_account.get(account) {
            Some((recipients, identity)) => (recipients, identity),
            None => (&self.recipients_file, &self.identity_file),
        }
    }

    fn file(&self, account: &str) -> PathBuf {
        self.dir.join(format!("{account}.age"))
    }

    /// `age` (and its plugins) get a minimal environment; prompts go to the terminal.
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.env_clear().env("PATH", "/usr/bin:/usr/local/bin");
        for var in ["HOME", "TERM", "XDG_RUNTIME_DIR"] {
            if let Some(v) = std::env::var_os(var) {
                cmd.env(var, v);
            }
        }
        cmd
    }

    fn save(&self, account: &str, token: &Secret) -> Result<()> {
        util::create_private_dir_all(&self.dir)?;
        let mut child = self
            .command()
            .arg("--encrypt")
            .arg("--recipients-file")
            .arg(self.keys(account).0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .ctx(|| format!("running {}", self.program.display()))?;
        // The token is far smaller than a pipe buffer, so write-then-read cannot deadlock.
        child.stdin.take().expect("piped stdin").write_all(token.expose()).ctx(|| "sending token to age")?;
        let mut ciphertext = Vec::new();
        child.stdout.take().expect("piped stdout").read_to_end(&mut ciphertext).ctx(|| "reading age output")?;
        let status = child.wait().ctx(|| "waiting for age")?;
        if !status.success() || ciphertext.is_empty() {
            return Err(Error::Auth(format!("age encryption failed ({status})")));
        }
        util::write_atomic(&self.file(account), &ciphertext, 0o600)
    }

    fn load(&self, account: &str) -> Result<Option<Secret>> {
        let file = self.file(account);
        if !file.exists() {
            return Ok(None);
        }
        util::require_private(&file)?;
        let ciphertext = util::read_small_file(&file, MAX_VAULT_BYTES)?;
        let mut child = self
            .command()
            .arg("--decrypt")
            .arg("--identity")
            .arg(self.keys(account).1)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .ctx(|| format!("running {}", self.program.display()))?;
        child.stdin.take().expect("piped stdin").write_all(&ciphertext).ctx(|| "sending vault to age")?;
        // Plaintext goes from the pipe straight into locked memory.
        let mut secret = Secret::with_capacity(MAX_TOKEN_BYTES)?;
        let mut out = child.stdout.take().expect("piped stdout");
        let mut chunk = [0u8; 1024];
        let read = loop {
            match out.read(&mut chunk) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if secret.push(&chunk[..n]).is_err() {
                        break Err(Error::Auth("decrypted token is implausibly large".into()));
                    }
                }
                Err(e) => break Err(Error::io("reading age output", e)),
            }
        };
        chunk.zeroize();
        let status = child.wait().ctx(|| "waiting for age")?;
        read?;
        if !status.success() || secret.is_empty() {
            return Err(Error::Auth(format!(
                "age decryption failed ({status}); is the identity (hardware key) available?"
            )));
        }
        Ok(Some(secret))
    }

    fn delete(&self, account: &str) -> Result<()> {
        match std::fs::remove_file(self.file(account)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io("removing vault file", e)),
        }
    }
}

/// What kind of key an age identity file holds, judged by its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityKind {
    /// A plugin identity (`AGE-PLUGIN-<NAME>-1…`): the key lives in a device.
    Plugin(String),
    /// An ordinary age or SSH private key in the file itself.
    Software,
    /// Encrypted with a passphrase (`age -p`): a software key, but not usable as it lies.
    PassphraseProtected,
    Unreadable(String),
}

/// Look at an identity file without using it. Hardware-backed means: every decryption needs
/// the device (and usually a touch or PIN); a software key can be copied by anything that
/// can read the file.
pub fn identity_kind(path: &Path) -> IdentityKind {
    let bytes = match util::read_small_file(path, 64 * 1024) {
        Ok(bytes) => bytes,
        Err(e) => return IdentityKind::Unreadable(e.to_string()),
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut software = false;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some(rest) = line.strip_prefix("AGE-PLUGIN-") {
            // `AGE-PLUGIN-YUBIKEY-1Q…`, `AGE-PLUGIN-FIDO2-HMAC-1Q…`: the plugin's name ends at
            // the last `-1`, the separator of the Bech32 data that follows.
            let name = rest.rfind("-1").map(|at| &rest[..at]).unwrap_or("");
            let plain = !name.is_empty()
                && name.len() <= 32
                && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-');
            return IdentityKind::Plugin(if plain { name.to_ascii_lowercase() } else { "unknown".into() });
        }
        if line.starts_with("age-encryption.org/") || line.starts_with("-----BEGIN AGE ENCRYPTED FILE") {
            return IdentityKind::PassphraseProtected;
        }
        if line.starts_with("AGE-SECRET-KEY-") || line.starts_with("-----BEGIN OPENSSH PRIVATE KEY") {
            software = true;
        }
    }
    if software { IdentityKind::Software } else { IdentityKind::Unreadable("no age identity found in the file".into()) }
}

impl TokenStore {
    /// The identity file used for `account`, when the store is an age vault.
    pub fn age_identity(&self, account: &str) -> Option<&Path> {
        match self {
            TokenStore::Age(vault) => Some(vault.keys(account).1),
            _ => None,
        }
    }

    /// Decrypt `account`'s vault file and encrypt it again to the recipients configured
    /// now. This is how a vault follows a change of keys: add the new recipient to the
    /// recipients file, re-seal with the old identity still present, then retire the old
    /// one. Returns `false` when the account has no vault file (or the store is not age).
    pub fn reseal(&self, account: &str) -> Result<bool> {
        validate_id(account)?;
        let TokenStore::Age(vault) = self else { return Ok(false) };
        match vault.load(account)? {
            Some(token) => vault.save(account, &token).map(|()| true),
            None => Ok(false),
        }
    }
}

/// Used by `doctor`: is the configured `age` binary acceptable?
pub fn check_age_program(path: &Path) -> Result<PathBuf> {
    util::require_root_owned(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn keyring_roundtrip_and_delete() {
        let account = format!("test-{}", util::hex_encode(&util::random_bytes::<4>().unwrap()));
        let store = TokenStore::Keyutils;
        match store.load(&account) {
            Ok(found) => assert!(found.is_none()),
            // Clean build chroots (systemd-nspawn) filter the keyring syscalls. The package
            // build opts in to skipping; everywhere else an unusable keyring is a failure.
            Err(e) if std::env::var_os("BASTION_ALLOW_NO_KEYRING").is_some() => {
                eprintln!("skipped: kernel keyring unavailable in this environment ({e})");
                return;
            }
            Err(e) => panic!("kernel keyring unavailable: {e}"),
        }
        let token = Secret::from_slice(b"M.C5_BL2.refresh-token-value").unwrap();
        store.save(&account, &token).unwrap();
        assert_eq!(store.load(&account).unwrap().unwrap().expose(), token.expose());
        assert!(store.has(&account).unwrap());
        store.delete(&account).unwrap();
        store.delete(&account).unwrap(); // idempotent
        // Invalidation is asynchronous inside the kernel; the key must be gone shortly after.
        let mut gone = false;
        for _ in 0..50 {
            if store.load(&account).unwrap().is_none() {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone, "deleted key is still readable");
    }

    #[test]
    fn account_names_cannot_escape_their_namespace() {
        let store = TokenStore::Keyutils;
        let token = Secret::from_slice(b"x").unwrap();
        for bad in ["../other", "a/b", "", "A", "with space"] {
            assert!(store.save(bad, &token).is_err(), "{bad:?}");
            assert!(store.load(bad).is_err(), "{bad:?}");
        }
        assert!(store.save("ok", &Secret::with_capacity(1).unwrap()).is_err(), "empty token");
    }

    #[test]
    fn identity_files_are_classified_by_what_they_hold() {
        let dir =
            std::env::temp_dir().join(format!("bastion-idk-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        let kind = |text: &str| {
            let path = dir.join("identity");
            std::fs::write(&path, text).unwrap();
            identity_kind(&path)
        };
        assert_eq!(
            kind("#       Serial: 1234, Slot: 1\n#   PIN policy: once\nAGE-PLUGIN-YUBIKEY-1QQPQQQQQQQQQ\n"),
            IdentityKind::Plugin("yubikey".into())
        );
        assert_eq!(kind("AGE-PLUGIN-TPM-1QYQSZQGPQYQSZQGP\n"), IdentityKind::Plugin("tpm".into()));
        assert_eq!(kind("AGE-PLUGIN-FIDO2-HMAC-1QQPQZ\n"), IdentityKind::Plugin("fido2-hmac".into()));
        assert_eq!(kind("# created: 2026-01-01\nAGE-SECRET-KEY-1QQQQQQQQQQQQQQQQ\n"), IdentityKind::Software);
        assert_eq!(kind("-----BEGIN OPENSSH PRIVATE KEY-----\nb3Bl\n"), IdentityKind::Software);
        assert_eq!(kind("age-encryption.org/v1\n-> scrypt abc 18\n"), IdentityKind::PassphraseProtected);
        assert!(matches!(kind("# only a comment\n"), IdentityKind::Unreadable(_)));
        assert!(matches!(identity_kind(&dir.join("missing")), IdentityKind::Unreadable(_)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn none_store_never_persists() {
        let store = TokenStore::None;
        store.save("default", &Secret::from_slice(b"t").unwrap()).unwrap();
        assert!(store.load("default").unwrap().is_none());
    }

    /// The vault plumbing against a stand-in for `age` (XOR "encryption"). This checks the
    /// pipe handling, permissions and error paths — not age itself, which is not installed on
    /// the development machine.
    #[test]
    fn age_vault_plumbing_with_stub_program() {
        let dir =
            std::env::temp_dir().join(format!("bastion-age-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let stub = dir.join("fake-age");
        std::fs::write(
            &stub,
            "#!/usr/bin/python3 -I\nimport sys\nargs = sys.argv[1:]\nassert args[0] in ('--encrypt', '--decrypt'), args\n\
             assert args[1] in ('--recipients-file', '--identity') and len(args) == 3, args\n\
             key = open(args[2], 'rb').read(1)[0]\ndata = sys.stdin.buffer.read()\nsys.stdout.buffer.write(bytes(b ^ key for b in data))\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let keyfile = dir.join("key");
        std::fs::write(&keyfile, "k").unwrap();
        // The stand-in XORs with the first byte of the key file, so different keys give
        // different ciphertexts and a vault opens only with the key it was sealed with.
        let other_key = dir.join("other-key");
        std::fs::write(&other_key, "o").unwrap();
        let cfg = AgeConfig {
            program: stub,
            recipients_file: Some(keyfile.clone()),
            identity_file: Some(keyfile.clone()),
            accounts: [(
                "work".to_string(),
                crate::config::AgeAccountKeys { recipients_file: other_key.clone(), identity_file: other_key.clone() },
            )]
            .into(),
        };
        let vault = TokenStore::Age(AgeVault::new(&cfg, dir.join("vault"), false).unwrap());

        assert!(vault.load("default").unwrap().is_none());
        let token = Secret::from_slice(b"refresh-token-123").unwrap();
        vault.save("default", &token).unwrap();
        let on_disk = std::fs::read(dir.join("vault/default.age")).unwrap();
        assert!(!on_disk.windows(7).any(|w| w == b"refresh"), "plaintext must not be on disk");
        let mode = std::fs::metadata(dir.join("vault/default.age")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(vault.load("default").unwrap().unwrap().expose(), b"refresh-token-123");
        assert!(vault.has("default").unwrap() && !vault.has("other").unwrap());

        // An account with keys of its own is sealed with those, not with the common ones.
        vault.save("work", &token).unwrap();
        let common = std::fs::read(dir.join("vault/default.age")).unwrap();
        let own = std::fs::read(dir.join("vault/work.age")).unwrap();
        assert_ne!(common, own, "the same token under two different keys");
        assert_eq!(own[0], b'r' ^ b'o');
        assert_eq!(common[0], b'r' ^ b'k');
        assert_eq!(vault.load("work").unwrap().unwrap().expose(), b"refresh-token-123");
        vault.delete("work").unwrap();

        // Re-sealing: the same token, encrypted again to whatever the recipients are now.
        let sealed_before = std::fs::read(dir.join("vault/default.age")).unwrap();
        std::fs::write(&keyfile, "k").unwrap();
        assert!(vault.reseal("default").unwrap());
        assert_eq!(std::fs::read(dir.join("vault/default.age")).unwrap(), sealed_before, "same key, same result");
        assert!(!vault.reseal("nobody").unwrap(), "no vault file, nothing to do");
        assert!(!TokenStore::Keyutils.reseal("default").unwrap());

        // A vault file writable by others is refused rather than decrypted.
        std::fs::set_permissions(dir.join("vault/default.age"), std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(vault.load("default").is_err());
        std::fs::set_permissions(dir.join("vault/default.age"), std::fs::Permissions::from_mode(0o600)).unwrap();

        // Failure of the external program is an error, not an empty token.
        std::fs::remove_file(&keyfile).unwrap();
        assert!(vault.load("default").is_err());
        assert!(vault.save("default", &token).is_err());
        vault.delete("default").unwrap();
        vault.delete("default").unwrap();

        // The real constructor insists on a root-owned binary.
        assert!(AgeVault::new(&cfg, dir.join("vault"), true).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
