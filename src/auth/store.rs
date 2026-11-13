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
            dir,
        })
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
            .arg(&self.recipients_file)
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
            .arg(&self.identity_file)
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
             open(args[2]).close()\ndata = sys.stdin.buffer.read()\nsys.stdout.buffer.write(bytes(b ^ 0x5a for b in data))\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let keyfile = dir.join("key");
        std::fs::write(&keyfile, "k").unwrap();
        let cfg =
            AgeConfig { program: stub, recipients_file: Some(keyfile.clone()), identity_file: Some(keyfile.clone()) };
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
