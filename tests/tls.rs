//! TLS policy tests against a local TLS server with a throw-away certificate hierarchy.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use bastion_mc::{
    Error,
    net::{
        http::HttpClient,
        tls::{self, PinSet},
    },
    util::{self, HashAlg},
};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose, PublicKeyData};
use rustls::{
    RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;

struct Authority {
    params: CertificateParams,
    key: KeyPair,
    cert: CertificateDer<'static>,
}

impl Authority {
    fn pin(&self) -> String {
        pin_of(&self.key)
    }
}

fn pin_of(key: &KeyPair) -> String {
    format!("sha256/{}", util::b64_encode(&util::hash_bytes(HashAlg::Sha256, &key.subject_public_key_info())))
}

fn ca_params(name: &str) -> CertificateParams {
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.distinguished_name.push(DnType::CommonName, name);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p
}

fn root(name: &str) -> Authority {
    let params = ca_params(name);
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap().der().clone();
    Authority { params, key, cert }
}

fn intermediate(name: &str, parent: &Authority) -> Authority {
    let params = ca_params(name);
    let key = KeyPair::generate().unwrap();
    let issuer = Issuer::from_params(&parent.params, &parent.key);
    let cert = params.signed_by(&key, &issuer).unwrap().der().clone();
    Authority { params, key, cert }
}

/// A server certificate for `localhost`, with its key.
fn leaf(parent: &Authority) -> (CertificateDer<'static>, KeyPair) {
    let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, "localhost");
    let key = KeyPair::generate().unwrap();
    let issuer = Issuer::from_params(&parent.params, &parent.key);
    (params.signed_by(&key, &issuer).unwrap().der().clone(), key)
}

/// Serve `response` to every TLS connection; returns the address and a connection counter.
async fn serve(
    chain: Vec<CertificateDer<'static>>,
    key: &KeyPair,
    response: Vec<u8>,
) -> (SocketAddr, Arc<AtomicUsize>) {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let config = ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { break };
            let (acceptor, response, counter) = (acceptor.clone(), response.clone(), counter.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else { return };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 2048];
                let _ = tls.read(&mut buf).await;
                let _ = tls.write_all(&response).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, hits)
}

fn client(trusted: &[&Authority], pins: &[(&str, Vec<String>)], addr: SocketAddr) -> HttpClient {
    let mut roots = RootCertStore::empty();
    for a in trusted {
        roots.add(a.cert.clone()).unwrap();
    }
    let map: BTreeMap<String, Vec<String>> = pins.iter().map(|(h, p)| (h.to_string(), p.clone())).collect();
    let config = tls::client_config(roots, PinSet::from_config(&map, false).unwrap()).unwrap();
    HttpClient::new(config, &["localhost".to_string()]).with_dial_override(addr)
}

const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

#[test]
fn tls12_is_not_compiled_in() {
    // rustls is built without its `tls12` feature: there is exactly one protocol version.
    assert_eq!(rustls::ALL_VERSIONS.len(), 1);
    assert_eq!(rustls::ALL_VERSIONS[0].version, rustls::ProtocolVersion::TLSv1_3);
}

#[tokio::test]
async fn chain_validation_without_pins() {
    let ca = root("Trusted Root");
    let (cert, key) = leaf(&ca);
    let (addr, _) = serve(vec![cert], &key, OK.to_vec()).await;
    assert_eq!(client(&[&ca], &[], addr).get("https://localhost/", 1024).await.unwrap(), b"ok");

    // Same server, but the client does not trust that CA.
    let stranger = root("Some Other Root");
    let err = client(&[&stranger], &[], addr).get("https://localhost/", 1024).await.unwrap_err();
    assert!(matches!(err, Error::Tls(_)), "{err}");

    // Right CA, wrong name: the certificate is for `localhost` only.
    let other = HttpClient::new(
        tls::client_config(
            {
                let mut r = RootCertStore::empty();
                r.add(ca.cert.clone()).unwrap();
                r
            },
            PinSet::default(),
        )
        .unwrap(),
        &["example.test".to_string()],
    )
    .with_dial_override(addr);
    assert!(matches!(other.get("https://example.test/", 1024).await, Err(Error::Tls(_))));
}

#[tokio::test]
async fn pins_match_leaf_intermediate_or_root_of_the_validated_chain() {
    let ca = root("Root A");
    let mid = intermediate("Intermediate A", &ca);
    let (cert, key) = leaf(&mid);
    let (addr, _) = serve(vec![cert, mid.cert.clone()], &key, OK.to_vec()).await;

    for (what, pin) in [("leaf", pin_of(&key)), ("intermediate", mid.pin()), ("root", ca.pin())] {
        let c = client(&[&ca], &[("localhost", vec![pin])], addr);
        assert_eq!(c.get("https://localhost/", 1024).await.unwrap(), b"ok", "pin on the {what} key");
    }
    // A suffix pattern and a list with one matching entry both work.
    let unrelated = pin_of(&KeyPair::generate().unwrap());
    let c = client(
        &[&ca],
        &[(".localhost", vec![unrelated.clone()]), ("localhost", vec![unrelated.clone(), ca.pin()])],
        addr,
    );
    assert!(c.get("https://localhost/", 1024).await.is_ok());

    // A pin that matches nothing in the chain: refused although the chain itself is valid.
    let c = client(&[&ca], &[("localhost", vec![unrelated])], addr);
    let err = c.get("https://localhost/", 1024).await.unwrap_err();
    assert!(matches!(err, Error::Tls(_)), "{err}");
}

/// The attack pinning exists for: a *different* trusted CA (compromised, coerced or simply
/// mis-issuing) certifies the attacker's key for our host. To get past a naive "is the pinned
/// certificate among those presented?" check, the attacker also sends the genuine pinned
/// certificate as an unused extra. The pin must be evaluated on the path that validated.
#[tokio::test]
async fn presenting_the_pinned_certificate_as_an_unused_extra_does_not_satisfy_the_pin() {
    let genuine_root = root("Genuine Root");
    let genuine_mid = intermediate("Genuine Intermediate", &genuine_root);
    let (genuine_leaf, _genuine_key) = leaf(&genuine_mid);

    let rogue_root = root("Other Trusted Root");
    let (rogue_leaf, rogue_key) = leaf(&rogue_root);

    // The MITM presents its own leaf, then the genuine leaf and intermediate as decoys.
    let (addr, hits) = serve(vec![rogue_leaf, genuine_leaf, genuine_mid.cert.clone()], &rogue_key, OK.to_vec()).await;

    // Both roots are trusted by the client, as in a real trust store.
    for pin in [genuine_mid.pin(), genuine_root.pin()] {
        let c = client(&[&genuine_root, &rogue_root], &[("localhost", vec![pin])], addr);
        let err = c.get("https://localhost/", 1024).await.unwrap_err();
        assert!(matches!(err, Error::Tls(_)), "MITM chain was accepted: {err}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no request may reach a server that failed the pin");

    // Control: without a pin the rogue chain *is* valid — which is exactly the risk.
    let c = client(&[&genuine_root, &rogue_root], &[], addr);
    assert!(c.get("https://localhost/", 1024).await.is_ok());
}

#[tokio::test]
async fn redirects_are_not_followed_and_bodies_are_bounded() {
    let ca = root("Root");
    let (cert, key) = leaf(&ca);

    // A redirect is an error, and the target is never contacted.
    let redirect = b"HTTP/1.1 302 Found\r\nLocation: https://localhost/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let (addr, hits) = serve(vec![cert.clone()], &key, redirect.to_vec()).await;
    let err = client(&[&ca], &[], addr).get("https://localhost/start", 1024).await.unwrap_err();
    assert!(err.to_string().contains("HTTP 302"), "{err}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "exactly one connection: the redirect was not followed");

    // Declared length over the limit: refused before reading the body.
    let mut big = b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\nConnection: close\r\n\r\n".to_vec();
    big.extend(std::iter::repeat_n(b'x', 5000));
    let (addr, _) = serve(vec![cert.clone()], &key, big).await;
    let err = client(&[&ca], &[], addr).get("https://localhost/", 1000).await.unwrap_err();
    assert!(err.to_string().contains("limit"), "{err}");

    // No declared length (body until close): cut off when the limit is crossed.
    let mut stream = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    stream.extend(std::iter::repeat_n(b'y', 5000));
    let (addr, _) = serve(vec![cert.clone()], &key, stream).await;
    let err = client(&[&ca], &[], addr).get("https://localhost/", 1000).await.unwrap_err();
    assert!(err.to_string().contains("limit"), "{err}");

    // A compressed answer is not decoded: the caller gets the raw bytes (and a hash check
    // would fail), never a decompression bomb.
    let gz =
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\nConnection: close\r\n\r\n\x1f\x8b\x08\x00";
    let (addr, _) = serve(vec![cert], &key, gz.to_vec()).await;
    assert_eq!(client(&[&ca], &[], addr).get("https://localhost/", 1000).await.unwrap(), b"\x1f\x8b\x08\x00");
}
