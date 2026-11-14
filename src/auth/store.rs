//! Where the Microsoft refresh token rests between launches.
//!
//! * `keyutils` (default): the kernel keyring, via the `keyring-core` ecosystem. RAM only —
//!   never written to disk, gone at reboot (and after the persistent-keyring expiry, three
//!   days by default, if you stay logged out).
//! * `age`: a file encrypted by the external `age` tool, to the recipients in a recipients
//!   file, decrypted with an identity file. See below for what that is worth.
//! * `none`: nothing is stored; sign in on every launch.
//!
//! What the kernel keyring does and does not give you (see docs/THREAT_MODEL.md): other
//! *users* cannot read it, it never touches disk, and the sandboxed game cannot reach it
//! (fresh session keyring + seccomp). Other *processes running as you in the same login
//! session* can read it; no user-space launcher can change that.
//!
//! **What the age vault gives you, exactly.** Two different questions decide it, and they
//! have different answers:
//!
//! * *Who can decrypt a vault file that exists?* Whoever holds the secret half of one of
//!   the recipients it was encrypted to. With an identity kept in a device
//!   (`age-plugin-yubikey`, `age-plugin-tpm`, `age-plugin-fido2-hmac`) that takes the
//!   device: a copy of the file — a backup, a stolen disk, a file read by other software —
//!   is useless without it. It does **not** take more than the device: a program running as
//!   you can ask a connected device to decrypt, just as the launcher does. Whether that
//!   needs a touch or a PIN is the device's policy, not something this launcher sees or
//!   enforces, and a touch you give expecting the launcher can be spent on somebody else's
//!   request. A software identity (a key in a file) can simply be read.
//! * *To whom is the next vault file encrypted?* To whatever the recipients file says at
//!   that moment — and encrypting needs no device and no touch. The launcher re-encrypts the
//!   (rotated) refresh token at every launch. So if the recipients file is one of your own
//!   files, a program running as you can add a recipient of its own to it, and from the
//!   next launch on it reads your token offline, hardware key or not.
//!
//! The second point is closed only by taking the recipients out of the user's reach:
//! when the root-owned file [`SYSTEM_AGE_RECIPIENTS`] exists, the vault is encrypted to
//! exactly the recipients in it, `recipients_file` settings in the configuration are
//! ignored, only a program called `age` (or `rage`) is run, and the keyring is refused as a
//! store (otherwise the same program would simply switch the configuration to it). Without
//! that file the vault protects the token at rest and against other *users*; against other
//! software running as you it is no stronger than the files it is configured with.
//!
//! **What the root-owned file does not do.** It takes the *recipients* out of the reach of
//! software running as you — the quiet, lasting attack: one line added once, and every
//! token stored afterwards readable offline. It does not take the *launcher* out of that
//! reach. A program that can put itself between you and the launcher (another binary earlier
//! in `$PATH`, a wrapper, a library preloaded into it) sees the token in the moment you
//! launch, whatever the vault is encrypted to; and it can still ask a connected device to
//! decrypt the vault file that exists. Only a separate UNIX account for the launcher keeps
//! such a program away (docs/MC_VAULT_ACCOUNT.md).
//!
//! None of this was exercised with a real `age` binary or a real device on the development
//! machine: the plumbing is tested against a stand-in program.

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
    paths::{Paths, SYSTEM_AGE_RECIPIENTS, validate_id},
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
    /// The root-owned recipients file, when there is one: then every vault file is
    /// encrypted to it, and the recipients files above are not used.
    anchor: Option<PathBuf>,
}

/// The root-owned recipients file of this machine ([`SYSTEM_AGE_RECIPIENTS`]), if there is
/// one. Something at that path that is not a plain root-owned file is an error — it is not
/// passed over in favour of the user's own recipients.
pub fn system_recipients() -> Result<Option<PathBuf>> {
    anchor_at(Path::new(SYSTEM_AGE_RECIPIENTS), true)
}

/// [`system_recipients`] for a given path. `root_owned: false` (tests only) accepts a file
/// of the user's own in its place.
#[doc(hidden)]
pub fn anchor_at(path: &Path, root_owned: bool) -> Result<Option<PathBuf>> {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(format!("inspecting {}", path.display()), e)),
        Ok(meta) => meta,
    };
    let unusable = |why: &str| {
        Error::Policy(format!(
            "{} exists but {why}. It is the list of recipients the age vault must be encrypted to; the launcher does \
             not fall back to the configured recipients file while it is there. Fix it (owner root, mode 0644, a \
             plain file in a root-owned directory) or remove it.",
            path.display()
        ))
    };
    // A plain file: a link could lead to somewhere that is not root's to keep unchanged.
    if !meta.file_type().is_file() {
        return Err(unusable("is not a regular file"));
    }
    if root_owned {
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(unusable("is not owned by root, or is writable by others"));
        }
        // Whoever can write the directory can replace the file, or take it away.
        let dir = path.parent().ok_or_else(|| unusable("has no directory"))?;
        let dir_meta = std::fs::metadata(dir).ctx(|| format!("inspecting {}", dir.display()))?;
        if dir_meta.uid() != 0 || dir_meta.mode() & 0o022 != 0 {
            return Err(unusable("lies in a directory that is not root's alone to write"));
        }
    }
    Ok(Some(path.to_path_buf()))
}

impl TokenStore {
    pub fn open(cfg: &AuthConfig, paths: &Paths) -> Result<Self> {
        Self::open_anchored(cfg, paths, system_recipients()?, true)
    }

    /// [`TokenStore::open`] with the root-owned recipients file given (`None`: there is
    /// none). `strict: false` (tests only) accepts an `age` program that is not a system
    /// file.
    #[doc(hidden)]
    pub fn open_anchored(cfg: &AuthConfig, paths: &Paths, anchor: Option<PathBuf>, strict: bool) -> Result<Self> {
        Ok(match (cfg.token_store, anchor) {
            // With the recipients anchored by root, a configuration that stores the token
            // somewhere else — where no recipients list applies — is not followed: changing
            // one line of config.toml must not be a way around the anchor.
            (Kind::Keyutils, Some(anchor)) => {
                return Err(Error::Config(format!(
                    "auth.token_store = \"keyutils\", but {} exists: on this machine a refresh token is stored in \
                     the age vault, encrypted to the recipients in that file, or not at all. Set auth.token_store = \
                     \"age\" (or \"none\"). A token that is still in the kernel keyring is removed by `bastion-mc \
                     scram`.",
                    anchor.display()
                )));
            }
            (Kind::Keyutils, None) => TokenStore::Keyutils,
            (Kind::None, _) => TokenStore::None,
            (Kind::Age, anchor) => TokenStore::Age(AgeVault::anchored(&cfg.age, paths.vault(), strict, anchor)?),
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

/// A secret that is not an account's refresh token, kept in the same store — kernel keyring,
/// age vault or nowhere, as `auth.token_store` says — under a name of its own. No account
/// label can take that name: labels start with a letter or a digit, slot names with `_`, and
/// the keyring description has a prefix of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The secret of the TOTP approval of Realms joins (`realms::totp`).
    RealmsTotp,
}

impl Slot {
    fn name(self) -> &'static str {
        match self {
            Slot::RealmsTotp => "_realms-totp",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Slot::RealmsTotp => "bastion-mc:realms-totp",
        }
    }

    fn keyutils_entry(self) -> Result<keyring_core::Entry> {
        let store = linux_keyutils_keyring_store::Store::new().map_err(keyring_err)?;
        let modifiers = HashMap::from([("description", self.description())]);
        store.build("bastion-mc", self.name(), Some(&modifiers)).map_err(keyring_err)
    }
}

impl TokenStore {
    pub fn load_slot(&self, slot: Slot) -> Result<Option<Secret>> {
        match self {
            TokenStore::None => Ok(None),
            TokenStore::Keyutils => match slot.keyutils_entry()?.get_secret() {
                Ok(mut bytes) => {
                    let secret = Secret::from_slice(&bytes);
                    bytes.zeroize();
                    Ok(Some(secret?))
                }
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(e) => Err(keyring_err(e)),
            },
            TokenStore::Age(v) => v.load(slot.name()),
        }
    }

    /// Store `secret` in `slot`. With `token_store = "none"` nothing can be kept: an error, so
    /// that a setup command does not report success for a secret that is gone.
    pub fn save_slot(&self, slot: Slot, secret: &Secret) -> Result<()> {
        if secret.is_empty() || secret.len() > MAX_TOKEN_BYTES {
            return Err(Error::Auth("refusing to store an empty or oversized secret".into()));
        }
        match self {
            TokenStore::None => Err(Error::Config(
                "auth.token_store = \"none\" keeps no secrets; this needs the kernel keyring or the age vault".into(),
            )),
            TokenStore::Keyutils => slot.keyutils_entry()?.set_secret(secret.expose()).map_err(keyring_err),
            TokenStore::Age(v) => v.save(slot.name(), secret),
        }
    }

    pub fn delete_slot(&self, slot: Slot) -> Result<()> {
        match self {
            TokenStore::None => Ok(()),
            TokenStore::Keyutils => match slot.keyutils_entry()?.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(e) => Err(keyring_err(e)),
            },
            TokenStore::Age(v) => v.delete(slot.name()),
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

/// Invalidate the key of `account` that this login session's keyrings lead to. `Ok(false)`:
/// there is none (any more). For `scram`, which must know whether something was there
/// without reading it, and which repeats this until nothing is found.
pub fn keyutils_remove(account: &str) -> Result<bool> {
    validate_id(account)?;
    match keyutils_entry(account)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring_core::Error::NoEntry) => Ok(false),
        Err(e) => Err(keyring_err(e)),
    }
}

// ---------------------------------------------------------------- age vault

impl AgeVault {
    /// `strict` requires the `age` binary to be a root-owned system file (always the case
    /// outside tests).
    pub fn new(cfg: &AgeConfig, dir: PathBuf, strict: bool) -> Result<Self> {
        Self::anchored(cfg, dir, strict, None)
    }

    /// `anchor`: the root-owned recipients file, if this machine has one.
    pub fn anchored(cfg: &AgeConfig, dir: PathBuf, strict: bool, anchor: Option<PathBuf>) -> Result<Self> {
        let missing = |what: &str| Error::Config(format!("auth.age.{what} is required for the age token store"));
        let program = if strict { util::require_root_owned(&cfg.program)? } else { cfg.program.clone() };
        if let Some(anchor) = &anchor {
            // The program decides what "encrypt to these recipients" means. With the
            // recipients fixed by root, the configuration does not get to name some other
            // root-owned program in its place.
            let name = program.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if !matches!(name, "age" | "rage") {
                return Err(Error::Config(format!(
                    "auth.age.program = {:?}, but {} exists: with the recipients fixed by root, only a program \
                     called `age` (or `rage`) is run to encrypt to them",
                    cfg.program.display().to_string(),
                    anchor.display()
                )));
            }
        }
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
            anchor,
        })
    }

    /// The recipients and identity files for `account`: its own, or the common ones — and,
    /// for the recipients, the root-owned file instead of either when there is one.
    fn keys(&self, account: &str) -> (&Path, &Path) {
        let (recipients, identity) = match self.per_account.get(account) {
            Some((recipients, identity)) => (recipients.as_path(), identity.as_path()),
            None => (self.recipients_file.as_path(), self.identity_file.as_path()),
        };
        (self.anchor.as_deref().unwrap_or(recipients), identity)
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

/// Plugins that, by their own documentation, keep the secret key in a device and never hand
/// it out. A plugin identity of any other name may be exactly as much software as a key in a
/// file (there are plugins for password managers and cloud key services); the launcher does
/// not know, and does not say "hardware" for it. None of the three was available on the
/// development machine: this list is what their documentation says, not something tested.
pub const DEVICE_PLUGINS: [&str; 3] = ["yubikey", "tpm", "fido2-hmac"];

/// What an age identity file holds, judged by its content. Every identity in the file
/// counts: `age` tries all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityKind {
    /// Only plugin identities (`AGE-PLUGIN-<NAME>-1…`). The plugins' names, lower case,
    /// sorted, each once; a name that is not a plain plugin name is given as `unknown`.
    Plugins(Vec<String>),
    /// At least one ordinary age or SSH private key in the file itself — alone, or next to
    /// identities of the named plugins. Such a key can be copied by anything that can read
    /// the file; whether the vault can be opened with it depends on the recipients.
    Software {
        plugins: Vec<String>,
    },
    /// Encrypted with a passphrase (`age -p`): a software key, but not usable as it lies.
    PassphraseProtected,
    Unreadable(String),
}

/// Look at an identity file without using it.
pub fn identity_kind(path: &Path) -> IdentityKind {
    let bytes = match util::read_small_file(path, 64 * 1024) {
        Ok(bytes) => bytes,
        Err(e) => return IdentityKind::Unreadable(e.to_string()),
    };
    let text = String::from_utf8_lossy(&bytes);
    let (mut software, mut plugins) = (false, std::collections::BTreeSet::new());
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some(rest) = line.strip_prefix("AGE-PLUGIN-") {
            // `AGE-PLUGIN-YUBIKEY-1Q…`, `AGE-PLUGIN-FIDO2-HMAC-1Q…`: the plugin's name ends at
            // the last `-1`, the separator of the Bech32 data that follows.
            let name = rest.rfind("-1").map(|at| &rest[..at]).unwrap_or("");
            let plain = !name.is_empty()
                && name.len() <= 32
                && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-');
            plugins.insert(if plain { name.to_ascii_lowercase() } else { "unknown".into() });
        } else if line.starts_with("age-encryption.org/") || line.starts_with("-----BEGIN AGE ENCRYPTED FILE") {
            return IdentityKind::PassphraseProtected;
        } else if line.starts_with("AGE-SECRET-KEY-") || line.starts_with("-----BEGIN OPENSSH PRIVATE KEY") {
            software = true;
        }
    }
    let plugins: Vec<String> = plugins.into_iter().collect();
    match (software, plugins.is_empty()) {
        (true, _) => IdentityKind::Software { plugins },
        (false, false) => IdentityKind::Plugins(plugins),
        (false, true) => IdentityKind::Unreadable("no age identity found in the file".into()),
    }
}

/// Whom a recipients file names, by kind. Every one of them can decrypt what is encrypted
/// to the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recipients {
    /// Plugin recipients (`age1<name>1…`), by plugin name: lower case, sorted, each once.
    pub plugins: Vec<String>,
    /// Native age recipients (`age1…`): keys whose secret half is a string in a file,
    /// wherever that file is kept.
    pub native: usize,
    /// SSH public keys (`ssh-ed25519 …`, `ssh-rsa …`): likewise software keys.
    pub ssh: usize,
    /// Lines that are none of the above.
    pub other: usize,
}

impl Recipients {
    /// Recipients that are not known to be bound to a device.
    pub fn not_device_bound(&self) -> usize {
        self.native
            + self.ssh
            + self.other
            + self.plugins.iter().filter(|name| !DEVICE_PLUGINS.contains(&name.as_str())).count()
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.plugins.is_empty() {
            parts.push(format!("plugin recipient(s) for {}", self.plugins.join(", ")));
        }
        for (count, what) in
            [(self.native, "native age key(s)"), (self.ssh, "SSH key(s)"), (self.other, "line(s) of another kind")]
        {
            if count > 0 {
                parts.push(format!("{count} {what}"));
            }
        }
        if parts.is_empty() { "no recipients".into() } else { parts.join(", ") }
    }
}

/// Classify the lines of a recipients file. Nothing is checked beyond their shape: whether a
/// line is a *valid* recipient is for `age` to say.
pub fn recipient_kinds(path: &Path) -> Result<Recipients> {
    let bytes = util::read_small_file(path, 64 * 1024)?;
    let text = String::from_utf8_lossy(&bytes);
    let (mut found, mut plugins) = (Recipients::default(), std::collections::BTreeSet::new());
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        // Bech32: the data part never contains `1`, so the last `1` ends the prefix. A
        // native recipient's prefix is `age`; a plugin's is `age1<name>`.
        let prefix = line.rfind('1').map(|at| &line[..at]).unwrap_or("");
        let plugin = prefix.strip_prefix("age1").filter(|name| {
            (1..=32).contains(&name.len())
                && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
        if prefix == "age" {
            found.native += 1;
        } else if let Some(name) = plugin {
            plugins.insert(name.to_string());
        } else if line.starts_with("ssh-") || line.starts_with("ecdsa-") || line.starts_with("sk-") {
            found.ssh += 1;
        } else {
            found.other += 1;
        }
    }
    found.plugins = plugins.into_iter().collect();
    Ok(found)
}

impl TokenStore {
    /// The identity file used for `account`, when the store is an age vault.
    pub fn age_identity(&self, account: &str) -> Option<&Path> {
        match self {
            TokenStore::Age(vault) => Some(vault.keys(account).1),
            _ => None,
        }
    }

    /// The recipients file `account`'s vault is encrypted to, when the store is an age
    /// vault: the root-owned one if this machine has it, else the configured one.
    pub fn age_recipients(&self, account: &str) -> Option<&Path> {
        match self {
            TokenStore::Age(vault) => Some(vault.keys(account).0),
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
        let plugins = |names: &[&str]| IdentityKind::Plugins(names.iter().map(|n| n.to_string()).collect());
        let software =
            |names: &[&str]| IdentityKind::Software { plugins: names.iter().map(|n| n.to_string()).collect() };
        assert_eq!(
            kind("#       Serial: 1234, Slot: 1\n#   PIN policy: once\nAGE-PLUGIN-YUBIKEY-1QQPQQQQQQQQQ\n"),
            plugins(&["yubikey"])
        );
        assert_eq!(kind("AGE-PLUGIN-TPM-1QYQSZQGPQYQSZQGP\n"), plugins(&["tpm"]));
        assert_eq!(kind("AGE-PLUGIN-FIDO2-HMAC-1QQPQZ\n"), plugins(&["fido2-hmac"]));
        assert_eq!(kind("# created: 2026-01-01\nAGE-SECRET-KEY-1QQQQQQQQQQQQQQQQ\n"), software(&[]));
        assert_eq!(kind("-----BEGIN OPENSSH PRIVATE KEY-----\nb3Bl\n"), software(&[]));
        assert_eq!(kind("age-encryption.org/v1\n-> scrypt abc 18\n"), IdentityKind::PassphraseProtected);
        // Every identity in the file counts, in whatever order: a software key next to a
        // device identity is a software key that opens whatever is encrypted to it.
        assert_eq!(kind("AGE-PLUGIN-YUBIKEY-1QQPQ\nAGE-SECRET-KEY-1QQQQ\n"), software(&["yubikey"]));
        assert_eq!(kind("AGE-SECRET-KEY-1QQQQ\nAGE-PLUGIN-YUBIKEY-1QQPQ\n"), software(&["yubikey"]));
        assert_eq!(
            kind("AGE-PLUGIN-YUBIKEY-1QQPQ\nAGE-PLUGIN-TPM-1QYQS\nAGE-PLUGIN-YUBIKEY-1QQPZ\n"),
            plugins(&["tpm", "yubikey"])
        );
        // A plugin is not a device by being a plugin: only the names on the list are said to be.
        assert_eq!(kind("AGE-PLUGIN-SOMEVAULT-1QQPQ\n"), plugins(&["somevault"]));
        assert!(!DEVICE_PLUGINS.contains(&"somevault") && DEVICE_PLUGINS.contains(&"yubikey"));
        assert_eq!(kind("AGE-PLUGIN-\u{1b}[2J-1QQPQ\n"), plugins(&["unknown"]), "an odd name is not echoed");
        assert!(matches!(kind("# only a comment\n"), IdentityKind::Unreadable(_)));
        assert!(matches!(identity_kind(&dir.join("missing")), IdentityKind::Unreadable(_)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recipients_are_counted_by_kind() {
        let dir =
            std::env::temp_dir().join(format!("bastion-rcp-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("recipients");
        std::fs::write(
            &path,
            "# my keys\nage1yubikey1qwt50d05nh5vutpdzmlg5wn80xq5negm4uj9ghv0snvdd3yysf5yw3rhl3t\n\
             age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p\n\
             ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJ backup\n\
             age1tpm1qg86fn5esp30u9h6jy6zvu9gcsvnac09vn8jzjxt8s3qtlcv5h2x287wm36\n\
             age1somevault1qqqq\n\nnot a recipient\n",
        )
        .unwrap();
        let found = recipient_kinds(&path).unwrap();
        assert_eq!(found.plugins, ["somevault", "tpm", "yubikey"]);
        assert_eq!((found.native, found.ssh, found.other), (1, 1, 1));
        assert_eq!(found.not_device_bound(), 4, "the native key, the SSH key, the odd line, the unknown plugin");
        assert!(found.describe().contains("1 native age key(s)") && found.describe().contains("yubikey"));
        std::fs::write(&path, "age1yubikey1qwt50d05nh5\n").unwrap();
        assert_eq!(recipient_kinds(&path).unwrap().not_device_bound(), 0);
        assert!(recipient_kinds(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The root-owned recipients file: what stands at its path is used or is an error —
    /// never ignored — and with it in force the configuration cannot move the token to
    /// other recipients, to another program, or into the keyring.
    #[test]
    fn a_root_owned_recipients_file_overrides_the_configuration() {
        let dir =
            std::env::temp_dir().join(format!("bastion-anc-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        // The stand-in for age: prints which recipients file it was told to encrypt to.
        let age = dir.join("bin/age");
        std::fs::write(
            &age,
            "#!/usr/bin/python3 -I\nimport sys\nargs = sys.argv[1:]\nsys.stdin.buffer.read()\n\
             sys.stdout.write('sealed-to:' + args[2] if args[0] == '--encrypt' else 'opened-with:' + args[2])\n",
        )
        .unwrap();
        std::fs::set_permissions(&age, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (mine, anchor) = (dir.join("my-recipients"), dir.join("root-recipients"));
        std::fs::write(&mine, "age1attackeraddedthis\n").unwrap();
        std::fs::write(&anchor, "age1yubikey1qqqq\n").unwrap();
        let paths = Paths {
            config: dir.join("config"),
            data: dir.join("data"),
            cache: dir.join("cache"),
            state: dir.join("state"),
            runtime: None,
        };
        let mut cfg = AuthConfig {
            token_store: Kind::Age,
            age: AgeConfig {
                program: age.clone(),
                recipients_file: Some(mine.clone()),
                identity_file: Some(dir.join("identity")),
                accounts: [(
                    "work".to_string(),
                    crate::config::AgeAccountKeys { recipients_file: mine.clone(), identity_file: dir.join("id2") },
                )]
                .into(),
            },
            ..AuthConfig::default()
        };
        let token = Secret::from_slice(b"refresh").unwrap();
        let sealed_to = |store: &TokenStore, account: &str| {
            store.save(account, &token).unwrap();
            String::from_utf8(std::fs::read(paths.vault().join(format!("{account}.age"))).unwrap()).unwrap()
        };

        // No anchor: the configured files are what is used (and what a program running as
        // the user can change).
        let plain = TokenStore::open_anchored(&cfg, &paths, None, false).unwrap();
        assert_eq!(sealed_to(&plain, "default"), format!("sealed-to:{}", mine.display()));
        assert_eq!(plain.age_recipients("default"), Some(mine.as_path()));

        // With it: every account's vault is encrypted to the anchor, whatever is configured.
        let anchored = TokenStore::open_anchored(&cfg, &paths, Some(anchor.clone()), false).unwrap();
        for account in ["default", "work"] {
            assert_eq!(sealed_to(&anchored, account), format!("sealed-to:{}", anchor.display()), "{account}");
            assert_eq!(anchored.age_recipients(account), Some(anchor.as_path()));
        }
        // Decryption still uses the account's own identity.
        assert_eq!(anchored.age_identity("work"), Some(dir.join("id2").as_path()));
        // Re-sealing moves an existing vault file over to the anchor's recipients.
        sealed_to(&plain, "default");
        assert!(anchored.reseal("default").unwrap());
        assert_eq!(
            std::fs::read_to_string(paths.vault().join("default.age")).unwrap(),
            format!("sealed-to:{}", anchor.display())
        );

        // The configuration cannot leave the vault while the anchor exists…
        cfg.token_store = Kind::Keyutils;
        let refused = TokenStore::open_anchored(&cfg, &paths, Some(anchor.clone()), false).err().unwrap().to_string();
        assert!(refused.contains("keyutils") && refused.contains("or not at all"), "{refused}");
        assert!(matches!(TokenStore::open_anchored(&cfg, &paths, None, false).unwrap(), TokenStore::Keyutils));
        cfg.token_store = Kind::None;
        assert!(matches!(
            TokenStore::open_anchored(&cfg, &paths, Some(anchor.clone()), false).unwrap(),
            TokenStore::None
        ));
        // …and cannot name another program to do the "encrypting".
        cfg.token_store = Kind::Age;
        cfg.age.program = dir.join("bin/not-age");
        std::fs::copy(&age, &cfg.age.program).unwrap();
        let refused = TokenStore::open_anchored(&cfg, &paths, Some(anchor.clone()), false).err().unwrap().to_string();
        assert!(refused.contains("only a program called `age`"), "{refused}");
        assert!(
            TokenStore::open_anchored(&cfg, &paths, None, false).is_ok(),
            "without the anchor that is the user's choice"
        );

        // What may stand at the anchor's path. (A file of the user's own is not root's: the
        // real check refuses it instead of falling back to the configured recipients.)
        assert_eq!(anchor_at(&dir.join("absent"), true).unwrap(), None);
        assert_eq!(anchor_at(&anchor, false).unwrap(), Some(anchor.clone()));
        let not_roots = anchor_at(&anchor, true).err().unwrap().to_string();
        assert!(not_roots.contains("not owned by root") && not_roots.contains("does not fall back"), "{not_roots}");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&anchor, &link).unwrap();
        assert!(anchor_at(&link, false).is_err(), "a link is not a plain file");
        assert!(anchor_at(&dir.join("bin"), false).is_err(), "nor is a directory");
        // On a machine without the file (the development machine has none) there is no anchor.
        if !Path::new(SYSTEM_AGE_RECIPIENTS).exists() {
            assert_eq!(system_recipients().unwrap(), None);
        }
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

    /// A secret slot lives next to the accounts' refresh tokens but never collides with one:
    /// against the same stand-in for `age` as above.
    #[test]
    fn a_slot_is_kept_apart_from_every_account() {
        let dir = std::env::temp_dir()
            .join(format!("bastion-slot-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let stub = dir.join("fake-age");
        std::fs::write(
            &stub,
            "#!/usr/bin/python3 -I\nimport sys\nargs = sys.argv[1:]\nkey = open(args[2], 'rb').read(1)[0]\n\
             data = sys.stdin.buffer.read()\nsys.stdout.buffer.write(bytes(b ^ key for b in data))\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let keyfile = dir.join("key");
        std::fs::write(&keyfile, "k").unwrap();
        let cfg = AgeConfig {
            program: stub,
            recipients_file: Some(keyfile.clone()),
            identity_file: Some(keyfile),
            accounts: Default::default(),
        };
        let vault = TokenStore::Age(AgeVault::new(&cfg, dir.join("vault"), false).unwrap());
        assert!(vault.load_slot(Slot::RealmsTotp).unwrap().is_none());
        vault.save("realms-totp", &Secret::from_slice(b"an account's token").unwrap()).unwrap();
        vault.save_slot(Slot::RealmsTotp, &Secret::from_slice(b"totp secret").unwrap()).unwrap();
        assert!(dir.join("vault/_realms-totp.age").is_file(), "a name no account label can have");
        assert_eq!(vault.load_slot(Slot::RealmsTotp).unwrap().unwrap().expose(), b"totp secret");
        assert_eq!(vault.load("realms-totp").unwrap().unwrap().expose(), b"an account's token", "untouched");
        assert!(validate_id(Slot::RealmsTotp.name()).is_err(), "not a valid account label");
        vault.delete_slot(Slot::RealmsTotp).unwrap();
        assert!(vault.load_slot(Slot::RealmsTotp).unwrap().is_none());
        assert!(vault.load("realms-totp").unwrap().is_some());
        // Nowhere to keep it: an error, not a silent success.
        assert!(TokenStore::None.save_slot(Slot::RealmsTotp, &Secret::from_slice(b"x").unwrap()).is_err());
        assert!(TokenStore::None.load_slot(Slot::RealmsTotp).unwrap().is_none());
        assert_ne!(Slot::RealmsTotp.description(), "bastion-mc:msa-refresh:_realms-totp");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
