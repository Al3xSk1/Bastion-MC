//! The sandbox's TLS trust store: generated, never bound from the host.
//!
//! The game used to see the host's `/etc/ssl` and `/etc/ca-certificates`. It now gets a
//! trust store the launcher writes for each launch, held in sealed memory and mounted
//! read-only:
//!
//! * the distribution's root certificates (so TLS that merely passes through the relay
//!   validates exactly as before), and
//! * when the credential broker is on, its per-launch certificate authority — the only way
//!   that certificate ever reaches the game, and it never touches the disk.
//!
//! Two encodings of the same list are provided, at the paths software looks for them: a PEM
//! bundle (OpenSSL, GnuTLS, curl) and a Java keystore (`cacerts`), which is what the JVM
//! reads. The keystore is the old "JKS" format because it is trivial to write correctly: a
//! flat list of DER certificates and a keyed SHA-1 over the file.

use std::time::{SystemTime, UNIX_EPOCH};

use sha1::{Digest, Sha1};

use super::Mount;
use crate::{error::Result, net::tls, util};

/// Where the JVM is told to look (`-Djavax.net.ssl.trustStore`). Arch's JDK packages also
/// point their own `lib/security/cacerts` here.
pub const JAVA_STORE: &str = "/etc/ssl/certs/java/cacerts";
/// The conventional keystore password. It protects nothing: the store holds public
/// certificates and is integrity-protected by being a read-only mount.
pub const JAVA_STORE_PASSWORD: &str = "changeit";
/// Where a PEM bundle is expected by OpenSSL (`cert.pem`), by Arch's `ca-certificates`
/// layout, and by software that hard-codes the Debian or Fedora path.
const PEM_PATHS: [&str; 4] = [
    "/etc/ssl/cert.pem",
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/ssl/certs/ca-bundle.crt",
    "/etc/ca-certificates/extracted/tls-ca-bundle.pem",
];

pub struct TrustStore {
    pem: Vec<u8>,
    jks: Vec<u8>,
    /// Number of certificates in the store.
    pub anchors: usize,
}

impl TrustStore {
    /// The host's root certificates plus `extra` (alias, DER certificate).
    pub fn build(extra: Option<(&str, &[u8])>) -> Result<Self> {
        let mut certificates: Vec<(String, Vec<u8>)> = Vec::new();
        if let Some((alias, der)) = extra {
            certificates.push((alias.to_string(), der.to_vec()));
        }
        for (index, der) in tls::load_system_certificates()?.into_iter().enumerate() {
            certificates.push((format!("system-{index:04}"), der.as_ref().to_vec()));
        }
        Ok(Self::from_certificates(&certificates))
    }

    /// A store holding exactly these certificates (alias, DER). Aliases must be distinct,
    /// lower-case ASCII.
    pub fn from_certificates(certificates: &[(String, Vec<u8>)]) -> Self {
        let created_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        TrustStore {
            pem: pem_bundle(certificates),
            jks: java_keystore(certificates, created_ms),
            anchors: certificates.len(),
        }
    }

    pub fn pem(&self) -> &[u8] {
        &self.pem
    }

    pub fn java_keystore(&self) -> &[u8] {
        &self.jks
    }

    /// The mounts that make this store the sandbox's `/etc/ssl`.
    pub fn mounts(&self) -> Vec<Mount> {
        let mut mounts: Vec<Mount> = PEM_PATHS
            .iter()
            .map(|path| Mount::RoData { data: self.pem.clone(), dst: (*path).into(), mode: 0o444 })
            .collect();
        mounts.push(Mount::RoData { data: self.jks.clone(), dst: JAVA_STORE.into(), mode: 0o444 });
        mounts
    }
}

fn pem_bundle(certificates: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (alias, der) in certificates {
        out.extend_from_slice(format!("# {alias}\n-----BEGIN CERTIFICATE-----\n").as_bytes());
        for line in util::b64_encode(der).as_bytes().chunks(64) {
            out.extend_from_slice(line);
            out.push(b'\n');
        }
        out.extend_from_slice(b"-----END CERTIFICATE-----\n");
    }
    out
}

/// `sun.security.provider.JavaKeyStore`, version 2, trusted-certificate entries only.
fn java_keystore(certificates: &[(String, Vec<u8>)], created_ms: u64) -> Vec<u8> {
    // DataOutput.writeUTF: a 16-bit length and (for the ASCII used here) the bytes.
    fn utf(out: &mut Vec<u8>, text: &str) {
        debug_assert!(text.is_ascii() && text.len() <= u16::MAX as usize);
        out.extend_from_slice(&(text.len() as u16).to_be_bytes());
        out.extend_from_slice(text.as_bytes());
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0xFEED_FEEDu32.to_be_bytes());
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(certificates.len() as u32).to_be_bytes());
    for (alias, der) in certificates {
        out.extend_from_slice(&2u32.to_be_bytes()); // entry type: trusted certificate
        utf(&mut out, alias);
        out.extend_from_slice(&created_ms.to_be_bytes());
        utf(&mut out, "X.509");
        out.extend_from_slice(&(der.len() as u32).to_be_bytes());
        out.extend_from_slice(der);
    }
    // Integrity value: SHA-1 over the password as UTF-16BE, a fixed phrase, and the store.
    let mut digest = Sha1::new();
    for unit in JAVA_STORE_PASSWORD.encode_utf16() {
        digest.update(unit.to_be_bytes());
    }
    digest.update(b"Mighty Aphrodite");
    digest.update(&out);
    out.extend_from_slice(&digest.finalize());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(n: u8) -> (String, Vec<u8>) {
        (format!("test-{n}"), vec![0x30, 0x03, 0x02, 0x01, n])
    }

    #[test]
    fn keystore_layout_is_the_documented_one() {
        let jks = java_keystore(&[fake(1), fake(2)], 0x0102_0304_0506_0708);
        let mut expected = vec![0xFE, 0xED, 0xFE, 0xED, 0, 0, 0, 2, 0, 0, 0, 2];
        for n in [1u8, 2] {
            expected.extend_from_slice(&[0, 0, 0, 2, 0, 6]);
            expected.extend_from_slice(format!("test-{n}").as_bytes());
            expected.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 0, 5]);
            expected.extend_from_slice(b"X.509");
            expected.extend_from_slice(&[0, 0, 0, 5, 0x30, 0x03, 0x02, 0x01, n]);
        }
        assert_eq!(&jks[..expected.len()], &expected[..]);
        assert_eq!(jks.len(), expected.len() + 20, "followed by a SHA-1");

        // The integrity value depends on every byte before it.
        let other = java_keystore(&[fake(1), fake(3)], 0x0102_0304_0506_0708);
        assert_ne!(jks[jks.len() - 20..], other[other.len() - 20..]);
        let empty = java_keystore(&[], 0);
        assert_eq!(empty.len(), 12 + 20);
    }

    #[test]
    fn pem_bundle_wraps_at_64_columns_and_round_trips() {
        let der: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let pem = pem_bundle(&[("big".into(), der.clone())]);
        let text = String::from_utf8(pem).unwrap();
        assert!(
            text.starts_with("# big\n-----BEGIN CERTIFICATE-----\n") && text.ends_with("-----END CERTIFICATE-----\n")
        );
        let body: String = text.lines().filter(|l| !l.starts_with(['#', '-'])).collect();
        assert!(text.lines().filter(|l| !l.starts_with(['#', '-'])).all(|l| l.len() <= 64));
        assert_eq!(util::b64_decode(&body).unwrap(), der);
    }

    #[test]
    fn the_store_is_mounted_where_tls_stacks_look() {
        let store = TrustStore::from_certificates(&[fake(1)]);
        assert_eq!(store.anchors, 1);
        let paths: Vec<String> = store
            .mounts()
            .iter()
            .map(|m| match m {
                Mount::RoData { dst, mode, .. } => {
                    assert_eq!(*mode, 0o444);
                    dst.display().to_string()
                }
                _ => panic!("only generated files"),
            })
            .collect();
        assert!(paths.contains(&JAVA_STORE.to_string()));
        assert!(paths.contains(&"/etc/ssl/cert.pem".to_string()));
        assert!(paths.contains(&"/etc/ssl/certs/ca-certificates.crt".to_string()));
    }

    #[test]
    fn the_system_bundle_is_included_after_the_extra_certificate() {
        let (alias, der) = fake(9);
        let store = TrustStore::build(Some((&alias, &der))).unwrap();
        assert!(store.anchors > 50, "the distribution ships well over fifty roots, got {}", store.anchors);
        let text = String::from_utf8_lossy(store.pem());
        assert!(text.starts_with("# test-9\n"), "the extra certificate comes first");
        assert_eq!(text.matches("-----BEGIN CERTIFICATE-----").count(), store.anchors);
        // Entry count in the keystore header.
        assert_eq!(u32::from_be_bytes(store.java_keystore()[8..12].try_into().unwrap()) as usize, store.anchors);
    }
}
