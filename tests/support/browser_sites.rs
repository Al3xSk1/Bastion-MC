//! Stand-in HTTPS sites for the browser's tests (`tests/browser.rs`, `tests/browser_game.rs`):
//! a test certificate authority, put into the browser's profile with `certutil` (NSS's own
//! tool), and a small HTTP/1.1 server over TLS 1.3 with a page for each behaviour the tests
//! look at. Included with `#[path]`, not a test target of its own.
#![allow(dead_code)]

use std::{
    net::SocketAddr,
    path::Path,
    process::Command as Process,
    sync::{Arc, Mutex},
};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// The names the stand-in answers for. Only the first two belong to the test sites.
pub const NAMES: &[&str] = &["chat.example", "cdn.chat.example", "evil.example", "tracker.example"];

pub fn pem(der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
}

pub struct TestCa {
    pub pem: String,
    params: CertificateParams,
    key: KeyPair,
}

impl TestCa {
    pub fn new() -> TestCa {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        params.distinguished_name.push(DnType::CommonName, "bastion-mc browser test authority");
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::hours(1);
        params.not_after = now + time::Duration::days(2);
        let cert = params.self_signed(&key).unwrap();
        TestCa { pem: pem(cert.der()), params, key }
    }

    pub fn issue(&self, names: &[&str]) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
        params.distinguished_name.push(DnType::CommonName, names[0]);
        params.not_before = self.params.not_before;
        params.not_after = self.params.not_after;
        params.use_authority_key_identifier_extension = true;
        let issuer = Issuer::from_params(&self.params, &self.key);
        let cert = params.signed_by(&key, &issuer).unwrap();
        (vec![cert.der().clone()], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())))
    }

    /// Into the browser's profile, as NSS keeps it, before Firefox first starts.
    pub fn install(&self, profile: &Path, scratch: &Path) {
        std::fs::create_dir_all(profile).unwrap();
        let pem = scratch.join("test-ca.pem");
        std::fs::write(&pem, &self.pem).unwrap();
        let db = format!("sql:{}", profile.display());
        let status = Process::new("certutil").args(["-N", "-d", &db, "--empty-password"]).status().expect("certutil");
        assert!(status.success(), "certutil -N");
        let status = Process::new("certutil")
            .args(["-A", "-n", "bastion-mc browser test", "-t", "C,,", "-i"])
            .arg(&pem)
            .args(["-d", &db])
            .status()
            .unwrap();
        assert!(status.success(), "certutil -A");
    }
}

// ------------------------------------------------------------------ stand-in sites

/// What the stand-in saw: one entry per request, `host path cookie`.
#[derive(Clone, Default)]
pub struct Seen(pub Arc<Mutex<Vec<(String, String, String)>>>);

impl Seen {
    pub fn hosts(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|(h, _, _)| h.clone()).collect()
    }
    pub fn paths_of(&self, host: &str) -> Vec<String> {
        self.0.lock().unwrap().iter().filter(|(h, _, _)| h == host).map(|(_, p, _)| p.clone()).collect()
    }
    pub fn cookies(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|(_, _, c)| c.clone()).filter(|c| !c.is_empty()).collect()
    }
}

const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D,
    0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=utf-8><title>{title}</title><style>html,body{{margin:0;height:100%;\
         background:#203040;color:#fff;font:24px sans-serif}}</style></head><body>{body}</body></html>"
    )
}

/// The stand-in's answer to one request.
pub fn respond(host: &str, path: &str, cookie: &str) -> (String, String, Vec<u8>) {
    let html = |s: String| ("200 OK".to_string(), "text/html; charset=utf-8".to_string(), s.into_bytes());
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    match (host, route) {
        ("chat.example", "/") => {
            let who = if cookie.contains("session=tester") { "signed in as tester" } else { "Chat stand-in" };
            html(page(
                who,
                "<form action=/sent><input id=t name=msg autofocus style='font-size:32px;width:90%' \
                 oninput=\"document.title='typed:'+this.value\"></form>\
                 <button id=b style='position:absolute;left:300px;top:300px;width:200px;height:100px' \
                 onclick=\"document.title='clicked'\">press</button>",
            ))
        }
        ("chat.example", "/login") => (
            "200 OK".into(),
            "text/html; charset=utf-8\r\nSet-Cookie: session=tester; Secure; Path=/; Max-Age=86400".into(),
            page("logged in", "<p>The stand-in set a session cookie.</p>").into_bytes(),
        ),
        ("chat.example", "/sent") => html(page(&format!("sent:{query}"), "<p>sent</p>")),
        ("chat.example", "/link") => html(page(
            "Link stand-in",
            "<a href='https://evil.example/' style='display:block;width:100%;height:100%'>elsewhere</a>",
        )),
        ("chat.example", "/tracker") => html(page(
            "Tracker stand-in",
            "<img src='https://tracker.example/pixel.png'><img src='https://cdn.chat.example/ok.png'>\
             <img src='http://chat.example/plain.png'>",
        )),
        ("chat.example", "/many") => {
            let images: String = (0..12).map(|i| format!("<img src='/img/{i}.png'>")).collect();
            html(page("Many stand-in", &images))
        }
        ("chat.example", "/calls") => html(page(
            "Calls stand-in",
            "<script>for(let i=0;i<12;i++){fetch('/api/'+i).then(r=>r.text()).catch(()=>{})}</script>",
        )),
        ("chat.example", r) if r.starts_with("/api/") => ("200 OK".into(), "application/json".into(), b"{}".to_vec()),
        ("chat.example", "/anim") => html(page(
            "Anim stand-in",
            "<div id=d style='width:100%;height:100%'></div><script>let n=0;setInterval(()=>{n++;\
             document.getElementById('d').style.background='hsl('+(n*37%360)+',70%,50%)'},20)</script>",
        )),
        ("chat.example", "/webdriver") => html(page(
            "WebDriver stand-in",
            "<script>document.title='webdriver:'+navigator.webdriver+' dark:'+matchMedia('(prefers-color-scheme: dark)').matches</script>",
        )),
        ("chat.example", "/static") => html(page("Static stand-in", "<p>nothing moves here</p>")),
        (_, r) if r.ends_with(".png") => ("200 OK".into(), "image/png".into(), PNG.to_vec()),
        _ => html(page("elsewhere", "<p>a page the browser must never reach</p>")),
    }
}

async fn serve_one(stream: tokio::net::TcpStream, acceptor: tokio_rustls::TlsAcceptor, seen: Seen) {
    let Ok(mut tls) = acceptor.accept(stream).await else { return };
    let host = tls.get_ref().1.server_name().unwrap_or_default().to_string();
    let mut buf = Vec::new();
    loop {
        let end = loop {
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break p + 4;
            }
            let mut chunk = [0u8; 4096];
            match tls.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..end]).to_string();
        buf.drain(..end);
        let path = head.split(' ').nth(1).unwrap_or("/").to_string();
        let cookie = head
            .lines()
            .find_map(|l| {
                l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("cookie")).map(|(_, v)| v.trim().to_string())
            })
            .unwrap_or_default();
        seen.0.lock().unwrap().push((host.clone(), path.clone(), cookie.clone()));
        let (status, kind, body) = respond(&host, &path, &cookie);
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n",
            body.len()
        );
        if tls.write_all(reply.as_bytes()).await.is_err() || tls.write_all(&body).await.is_err() {
            return;
        }
        let _ = tls.flush().await;
    }
}

pub async fn stand_in(ca: &TestCa) -> (SocketAddr, Seen) {
    let (chain, key) = ca.issue(NAMES);
    let mut config =
        rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Seen::default();
    let sink = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            tokio::spawn(serve_one(stream, acceptor.clone(), sink.clone()));
        }
    });
    (addr, seen)
}
