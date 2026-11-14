//! `profile add` and `profile check` over real TLS, through the launcher's own HTTP client,
//! against a local stand-in for the registry's two hosts — and, with `BASTION_NET_TESTS=1`,
//! against Modrinth itself.
//!
//! The rules of resolution are tested against scripted replies in `profile::catalog`; what
//! is added here is the wire: TLS 1.3 with a throw-away certificate authority, HTTP/1.1 with
//! kept-alive connections, chunked answers, redirects, truncated and oversized bodies.

use std::{
    collections::BTreeMap,
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use bastion_mc::{
    Error, instance,
    net::{
        http::HttpClient,
        tls::{self, PinSet},
    },
    paths::Paths,
    profile::{
        catalog::{self, Options, Output},
        manifest::{FORMAT, Loader, LoaderKind, LoaderLibrary, MinecraftPin, ProfileManifest, ProfileOptions},
        modrinth::{self, Channel, Live, Registry},
    },
    util::{self, HashAlg},
};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha512};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;

/// 2026-11-14T00:00:00Z.
const NOW: u64 = 1_794_614_400;

// ------------------------------------------------------------------ a stand-in registry

#[derive(Clone, Copy, PartialEq, Eq)]
enum How {
    /// The whole body under a `Content-Length`.
    Whole,
    /// `Transfer-Encoding: chunked`, as the real API answers.
    Chunked,
    /// Announce the whole body, send half of it, hang up.
    Truncated,
    /// Take the request and never answer.
    Silent,
}

#[derive(Clone)]
struct Reply {
    status: u16,
    body: Vec<u8>,
    how: How,
    location: Option<String>,
    /// A `Last-Modified` header, as the real file server sends one.
    stored: Option<&'static str>,
}

#[derive(Clone, Debug)]
struct Request {
    method: String,
    path: String,
    host: String,
    agent: String,
    content_type: String,
    body: Vec<u8>,
}

struct StandIn {
    addr: SocketAddr,
    routes: Arc<Mutex<BTreeMap<(String, String), Reply>>>,
    seen: Arc<Mutex<Vec<Request>>>,
    connections: Arc<AtomicUsize>,
    authority: CertificateDer<'static>,
}

fn authority(name: &str) -> (CertificateParams, KeyPair, CertificateDer<'static>) {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, name);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap().der().clone();
    (params, key, cert)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

impl StandIn {
    /// A TLS 1.3 server whose certificate is valid for `names`. With `hang_up` it closes
    /// the connection after every answer without having announced it.
    async fn start(names: &[&str], hang_up: bool) -> StandIn {
        let (ca_params, ca_key, ca_cert) = authority("Stand-in Root");
        let mut params = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
        params.distinguished_name.push(DnType::CommonName, names[0]);
        let key = KeyPair::generate().unwrap();
        let leaf = params.signed_by(&key, &Issuer::from_params(&ca_params, &ca_key)).unwrap().der().clone();
        let config = ServerConfig::builder_with_provider(tls::provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![leaf], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())))
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stand_in = StandIn {
            addr: listener.local_addr().unwrap(),
            routes: Arc::default(),
            seen: Arc::default(),
            connections: Arc::default(),
            authority: ca_cert,
        };
        let (routes, seen, connections) =
            (stand_in.routes.clone(), stand_in.seen.clone(), stand_in.connections.clone());
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { break };
                let (acceptor, routes, seen, connections) =
                    (acceptor.clone(), routes.clone(), seen.clone(), connections.clone());
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(tcp).await else { return };
                    connections.fetch_add(1, Ordering::SeqCst);
                    let mut buffer = Vec::new();
                    loop {
                        // One request: the head, then as many bytes of body as it announces.
                        let head_end = loop {
                            if let Some(at) = find(&buffer, b"\r\n\r\n") {
                                break at + 4;
                            }
                            let mut chunk = [0u8; 4096];
                            match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
                        let mut lines = head.split("\r\n");
                        let request_line: Vec<&str> = lines.next().unwrap().split(' ').collect();
                        assert_eq!(request_line.len(), 3, "{head}");
                        assert_eq!(request_line[2], "HTTP/1.1");
                        let headers: BTreeMap<String, String> = lines
                            .filter_map(|line| line.split_once(':'))
                            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
                            .collect();
                        let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                        while buffer.len() < head_end + length {
                            let mut chunk = [0u8; 4096];
                            match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                            }
                        }
                        let header = |name: &str| headers.get(name).cloned().unwrap_or_default();
                        let request = Request {
                            method: request_line[0].to_string(),
                            path: request_line[1].split('?').next().unwrap().to_string(),
                            host: header("host"),
                            agent: header("user-agent"),
                            content_type: header("content-type"),
                            body: buffer[head_end..head_end + length].to_vec(),
                        };
                        buffer.drain(..head_end + length);
                        let key = (request.method.clone(), request.path.clone());
                        seen.lock().unwrap().push(request);
                        let reply = routes.lock().unwrap().get(&key).cloned().unwrap_or(Reply {
                            status: 404,
                            body: Vec::new(),
                            how: How::Whole,
                            location: None,
                            stored: None,
                        });
                        if reply.how == How::Silent {
                            std::future::pending::<()>().await;
                        }
                        // Every answer is dated 2026-11-13 23:00 UTC: an hour before the tests' NOW.
                        let mut out =
                            format!("HTTP/1.1 {} Stand-In\r\nDate: Fri, 13 Nov 2026 23:00:00 GMT\r\n", reply.status)
                                .into_bytes();
                        if let Some(location) = &reply.location {
                            out.extend_from_slice(format!("Location: {location}\r\n").as_bytes());
                        }
                        if let Some(stored) = reply.stored {
                            out.extend_from_slice(format!("Last-Modified: {stored}\r\n").as_bytes());
                        }
                        match reply.how {
                            How::Whole => {
                                out.extend_from_slice(
                                    format!("Content-Length: {}\r\n\r\n", reply.body.len()).as_bytes(),
                                );
                                out.extend_from_slice(&reply.body);
                            }
                            How::Chunked => {
                                out.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n");
                                for piece in reply.body.chunks(1000) {
                                    out.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
                                    out.extend_from_slice(piece);
                                    out.extend_from_slice(b"\r\n");
                                }
                                out.extend_from_slice(b"0\r\n\r\n");
                            }
                            How::Truncated => {
                                out.extend_from_slice(
                                    format!("Content-Length: {}\r\n\r\n", reply.body.len()).as_bytes(),
                                );
                                out.extend_from_slice(&reply.body[..reply.body.len() / 2]);
                            }
                            How::Silent => unreachable!("a silent route never answers"),
                        }
                        if stream.write_all(&out).await.is_err() || stream.flush().await.is_err() {
                            return;
                        }
                        if hang_up || reply.how == How::Truncated {
                            let _ = stream.shutdown().await;
                            return;
                        }
                    }
                });
            }
        });
        stand_in
    }

    fn on(&self, method: &str, path: &str, status: u16, body: impl Into<Vec<u8>>, how: How) -> &Self {
        let reply = Reply { status, body: body.into(), how, location: None, stored: None };
        self.routes.lock().unwrap().insert((method.to_string(), path.to_string()), reply);
        self
    }

    /// From now on the answer to `GET path` says when the server stored what it sends.
    fn stored(&self, path: &str, when: &'static str) -> &Self {
        let mut routes = self.routes.lock().unwrap();
        routes.get_mut(&("GET".to_string(), path.to_string())).expect("a route for that path").stored = Some(when);
        self
    }

    fn json(&self, method: &str, path: &str, value: &Value) -> &Self {
        self.on(method, path, 200, serde_json::to_vec(value).unwrap(), How::Chunked)
    }

    fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    fn asked(&self, path: &str) -> usize {
        self.requests().iter().filter(|request| request.path == path).count()
    }

    /// The registry client of the launcher, trusting `trusted` only and dialling this
    /// server whatever the host name: certificate validation against the name is untouched.
    fn client(&self, trusted: &CertificateDer<'static>) -> Live {
        let mut roots = RootCertStore::empty();
        roots.add(trusted.clone()).unwrap();
        let config = tls::client_config(roots, PinSet::default()).unwrap();
        let hosts: Vec<String> = modrinth::HOSTS.iter().map(|host| host.to_string()).collect();
        Live::with_client(HttpClient::new(config, &hosts).with_dial_override(self.addr))
    }
}

// ------------------------------------------------------------------ a profile to add to

struct Profile {
    root: PathBuf,
    paths: Paths,
    dir: PathBuf,
}

impl Drop for Profile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A draft Fabric profile `perf` for `minecraft`, with an empty `mods/`.
fn profile(tag: &str, minecraft: &str) -> Profile {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("catalog-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
    let paths = Paths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        state: root.join("state"),
        runtime: None,
    };
    paths.ensure().unwrap();
    let dir = instance::create(&paths, "perf", None).unwrap();
    std::fs::create_dir(dir.join("mods")).unwrap();
    let manifest = ProfileManifest {
        format: FORMAT.into(),
        id: "perf".into(),
        serial: 1,
        minecraft: MinecraftPin { version: minecraft.into(), version_json_sha1: None },
        loader: Some(Loader {
            kind: LoaderKind::Fabric,
            version: "0.20.1".into(),
            main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".into(),
            jvm_args: Vec::new(),
            game_args: Vec::new(),
            libraries: vec![LoaderLibrary {
                name: "net.fabricmc:fabric-loader:0.20.1".into(),
                url: "https://maven.fabricmc.net/".into(),
                sha256: "a".repeat(64),
                size: 10,
            }],
        }),
        mods: Vec::new(),
        options: ProfileOptions::default(),
    };
    std::fs::write(dir.join("profile.json"), manifest.to_bytes().unwrap()).unwrap();
    std::fs::write(dir.join("profile.json.sig"), "a signature\n").unwrap();
    Profile { root, paths, dir }
}

impl Profile {
    fn manifest(&self) -> ProfileManifest {
        serde_json::from_slice(&std::fs::read(self.dir.join("profile.json")).unwrap()).unwrap()
    }

    fn mods(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.join("mods"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// Nothing of an attempt is left: `mods/` empty, the manifest and signature as made,
    /// no staging directory.
    fn untouched(&self) -> bool {
        let entries: Vec<String> =
            std::fs::read_dir(&self.dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        self.mods().is_empty()
            && self.manifest().serial == 1
            && std::fs::read(self.dir.join("profile.json.sig")).unwrap() == b"a signature\n"
            && !entries.iter().any(|name| name.starts_with(".catalog-"))
    }
}

fn mod_jar(mod_id: &str, salt: &str) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("fabric.mod.json", options).unwrap();
    writer.write_all(format!(r#"{{"schemaVersion":1,"id":"{mod_id}","version":"{salt}"}}"#).as_bytes()).unwrap();
    writer.finish().unwrap().into_inner()
}

fn sha512_hex(content: &[u8]) -> String {
    util::hex_encode(&Sha512::digest(content))
}

const PROJECT: &str = "AAAAAAAA";
const VERSION: &str = "A0000001";
const FILE: &str = "alpha-1.0+mc.jar";
const FILE_PATH: &str = "/data/AAAAAAAA/versions/A0000001/alpha-1.0%2Bmc.jar";

fn project_json() -> Value {
    json!({
        "id": PROJECT, "slug": "alpha", "title": "Alpha Mod", "project_type": "mod", "status": "approved",
        "license": { "id": "MIT", "name": "MIT License", "url": null }, "body": "text", "versions": [VERSION]
    })
}

fn version_json(content: &[u8]) -> Value {
    json!({
        "id": VERSION, "project_id": PROJECT, "author_id": "UUUUUUUU", "name": "1.0", "version_number": "1.0+mc",
        "version_type": "release", "status": "listed", "date_published": "2026-10-01T00:00:00.123456Z",
        "downloads": 3, "featured": false, "game_versions": ["1.21.4"], "loaders": ["fabric"],
        "files": [{
            "hashes": { "sha512": sha512_hex(content), "sha1": util::hex_encode(&Sha1::digest(content)) },
            "url": format!("https://cdn.modrinth.com{FILE_PATH}"), "filename": FILE, "primary": true,
            "size": content.len(), "file_type": null
        }],
        "dependencies": []
    })
}

/// A stand-in that offers one project with one build, published on 2026-10-01; its file
/// server dates the file two seconds after the build, as the real one does.
async fn offering(content: &[u8], hang_up: bool) -> StandIn {
    let stand_in = StandIn::start(&modrinth::HOSTS, hang_up).await;
    stand_in
        .json("GET", "/v2/project/alpha", &project_json())
        .json("GET", &format!("/v2/project/{PROJECT}/version"), &json!([version_json(content)]))
        .json("POST", "/v2/version_files", &json!({}))
        .on("GET", FILE_PATH, 200, content, How::Whole)
        .stored(FILE_PATH, "Thu, 01 Oct 2026 00:00:02 GMT");
    stand_in
}

async fn add(profile: &Profile, client: &Live) -> Result<bool, Error> {
    add_with(profile, client, Options::default()).await.0
}

/// The outcome and everything the command said.
async fn add_with(profile: &Profile, client: &Live, options: Options) -> (Result<bool, Error>, String) {
    let mut out = Output::silent();
    let outcome = catalog::add(&profile.paths, client, "perf", &["alpha".to_string()], options, NOW, &mut out).await;
    (outcome, out.lines.join("\n"))
}

// ------------------------------------------------------------------ over TLS, locally

#[tokio::test]
async fn a_mod_is_added_and_checked_over_tls_through_the_launchers_own_client() {
    let content = mod_jar("alpha", "1.0");
    let stand_in = offering(&content, false).await;
    let client = stand_in.client(&stand_in.authority);
    let profile = profile("happy", "1.21.4");

    let (outcome, said) = add_with(&profile, &client, Options::default()).await;
    assert!(outcome.unwrap(), "{said}");
    assert!(!said.contains("file server"), "a file as old as its build is not remarked upon:\n{said}");
    assert_eq!(profile.mods(), [FILE]);
    assert_eq!(std::fs::read(profile.dir.join("mods").join(FILE)).unwrap(), content);
    let manifest = profile.manifest();
    assert_eq!(manifest.serial, 2);
    assert_eq!(manifest.mods[0].sha256, util::hex_encode(&util::hash_bytes(HashAlg::Sha256, &content)));
    assert_eq!(manifest.mods[0].source.as_deref(), Some(format!("https://cdn.modrinth.com{FILE_PATH}").as_str()));
    assert!(!profile.dir.join("profile.json.sig").exists());

    // The registry now knows the file; the same client checks it.
    stand_in.json("POST", "/v2/version_files", &json!({ sha512_hex(&content): version_json(&content) })).json(
        "GET",
        "/v2/projects",
        &json!([project_json()]),
    );
    let mut out = Output::silent();
    let summary = catalog::check(&profile.paths, &client, "perf", NOW, &mut out).await.unwrap();
    assert_eq!((summary.jars, summary.known, summary.unknown, summary.fresh), (1, 1, 0, 0), "{:?}", out.lines);
    assert!(out.lines.iter().any(|line| line.contains("known: Alpha Mod (alpha · project AAAAAAAA · licence MIT)")));

    // What went over the wire: each request to the host it belongs to, as this program.
    let requests = stand_in.requests();
    let line = |r: &Request| format!("{} {}{}", r.method, r.host, r.path);
    assert_eq!(
        requests.iter().map(line).collect::<Vec<_>>(),
        [
            "GET api.modrinth.com/v2/project/alpha",
            "GET api.modrinth.com/v2/project/AAAAAAAA/version",
            "GET cdn.modrinth.com/data/AAAAAAAA/versions/A0000001/alpha-1.0%2Bmc.jar",
            "POST api.modrinth.com/v2/version_files",
            "GET api.modrinth.com/v2/projects",
        ]
    );
    assert!(requests.iter().all(|r| r.agent == modrinth::USER_AGENT), "{requests:?}");
    assert_eq!(requests[3].content_type, "application/json");
    let asked: Value = serde_json::from_slice(&requests[3].body).unwrap();
    assert_eq!(asked, json!({ "algorithm": "sha512", "hashes": [sha512_hex(&content)] }));
    assert!(requests[0].body.is_empty() && requests[2].body.is_empty());
    // One connection per host for the whole of both commands.
    assert_eq!(stand_in.connections.load(Ordering::SeqCst), 2);
    // The registry's clock is read off its answers (here: an hour behind this test's NOW).
    let registry = Registry::new(&client);
    assert_eq!(registry.server_time(), None);
    registry.project("alpha").await.unwrap().unwrap();
    assert_eq!(registry.server_time(), Some(NOW - 3600));
    assert_eq!(registry.now(NOW), NOW - 3600);
}

#[tokio::test]
async fn a_server_that_hangs_up_after_every_answer_is_connected_to_again() {
    let content = mod_jar("alpha", "1.0");
    let stand_in = offering(&content, true).await;
    let profile = profile("hangup", "1.21.4");
    assert!(add(&profile, &stand_in.client(&stand_in.authority)).await.unwrap());
    assert_eq!(profile.mods(), [FILE]);
    assert_eq!(stand_in.connections.load(Ordering::SeqCst), 3, "one connection per request");
}

#[tokio::test]
async fn a_file_that_does_not_match_never_reaches_mods_over_tls() {
    let content = mod_jar("alpha", "1.0");

    // Other bytes of the same length.
    let stand_in = offering(&content, false).await;
    let mut other = content.clone();
    other[10] ^= 0x01;
    stand_in.on("GET", FILE_PATH, 200, other, How::Whole);
    let profile = profile("mismatch", "1.21.4");
    let error = add(&profile, &stand_in.client(&stand_in.authority)).await.unwrap_err();
    assert!(matches!(&error, Error::Integrity(m) if m.contains("SHA-512")), "{error}");
    assert_eq!(error.exit_code(), 4);
    assert!(profile.untouched());
    assert_eq!(stand_in.asked(FILE_PATH), 1, "wrong bytes are not asked for again");

    // More bytes than listed: cut off, not read to the end.
    let stand_in = offering(&content, false).await;
    stand_in.on("GET", FILE_PATH, 200, [content.clone(), vec![0u8; 3 << 20]].concat(), How::Chunked);
    let error = add(&profile, &stand_in.client(&stand_in.authority)).await.unwrap_err();
    assert!(matches!(&error, Error::Integrity(m) if m.contains("more than")), "{error}");
    assert!(profile.untouched());

    // A transfer that breaks off half way: tried once more, then given up.
    let stand_in = offering(&content, false).await;
    stand_in.on("GET", FILE_PATH, 200, content.clone(), How::Truncated);
    let error = add(&profile, &stand_in.client(&stand_in.authority)).await.unwrap_err();
    assert!(matches!(error, Error::Net(_)), "{error}");
    assert_eq!(stand_in.asked(FILE_PATH), 2);
    assert!(profile.untouched());

    // A redirect is an answer that is refused; where it points is never asked.
    let stand_in = offering(&content, false).await;
    stand_in.on("GET", "/elsewhere.jar", 200, content.clone(), How::Whole);
    stand_in.routes.lock().unwrap().insert(
        ("GET".into(), FILE_PATH.into()),
        Reply {
            status: 302,
            body: Vec::new(),
            how: How::Whole,
            location: Some("https://cdn.modrinth.com/elsewhere.jar".into()),
            stored: None,
        },
    );
    let error = add(&profile, &stand_in.client(&stand_in.authority)).await.unwrap_err();
    assert!(matches!(&error, Error::Net(m) if m.contains("HTTP 302")), "{error}");
    assert_eq!(stand_in.asked("/elsewhere.jar"), 0);
    assert!(profile.untouched());

    // An answer of the API beyond what a project description may weigh.
    let stand_in = offering(&content, false).await;
    let mut bloated = project_json();
    bloated["body"] = json!("x".repeat(5 << 20));
    stand_in.json("GET", "/v2/project/alpha", &bloated);
    let error = add(&profile, &stand_in.client(&stand_in.authority)).await.unwrap_err();
    assert!(matches!(&error, Error::Net(m) if m.contains("limit")), "{error}");
    assert!(profile.untouched());
}

#[tokio::test]
async fn the_registry_must_present_a_certificate_for_its_name_from_a_trusted_authority() {
    let content = mod_jar("alpha", "1.0");
    let profile = profile("tls", "1.21.4");

    // An authority this client does not trust.
    let stand_in = offering(&content, false).await;
    let (_, _, stranger) = authority("Some Other Root");
    let error = add(&profile, &stand_in.client(&stranger)).await.unwrap_err();
    assert!(matches!(error, Error::Tls(_)), "{error}");
    assert!(stand_in.requests().is_empty(), "no request is sent to a server that failed the handshake");
    assert!(profile.untouched());

    // A certificate for the API's name only: the file server is somebody else.
    let stand_in = StandIn::start(&["api.modrinth.com"], false).await;
    stand_in
        .json("GET", "/v2/project/alpha", &project_json())
        .json("GET", &format!("/v2/project/{PROJECT}/version"), &json!([version_json(&content)]))
        .on("GET", FILE_PATH, 200, content.clone(), How::Whole);
    let error = add(&profile, &stand_in.client(&stand_in.authority)).await.unwrap_err();
    assert!(matches!(error, Error::Tls(_)), "{error}");
    assert_eq!(stand_in.asked(FILE_PATH), 0);
    assert!(profile.untouched());
}

/// The file server's own date for a file (`Last-Modified`) is read off the answer that
/// delivers it. The build here is six weeks old; bytes the file server has held for three
/// hours cannot be shown to have been there for 48, whatever the build's age.
#[tokio::test]
async fn a_file_the_file_server_has_held_for_less_than_48_hours_is_refused_over_tls() {
    let content = mod_jar("alpha", "1.0");
    let profile = profile("stored", "1.21.4");
    // The stand-in dates its answers 2026-11-13 23:00 UTC; that is "now" (the earlier clock).
    let stand_in = offering(&content, false).await;
    stand_in.stored(FILE_PATH, "Fri, 13 Nov 2026 20:00:00 GMT");
    let client = stand_in.client(&stand_in.authority);
    let (outcome, said) = add_with(&profile, &client, Options::default()).await;
    let error = outcome.unwrap_err();
    assert!(
        matches!(&error, Error::Policy(m) if m.contains("only since 2026-11-13 20:00 UTC, 3 hours ago, although the build is dated 2026-10-01 00:00 UTC")),
        "{error}\n{said}"
    );
    assert_eq!(error.exit_code(), 5);
    assert!(profile.untouched());
    assert_eq!(stand_in.asked(FILE_PATH), 1, "it was fetched once: its date comes with it");

    // --allow-fresh takes it, and says what it took.
    let (outcome, said) = add_with(&profile, &client, Options { allow_fresh: true, ..Default::default() }).await;
    assert!(outcome.unwrap(), "{said}");
    assert!(
        said.contains("note: FRESH FILE: the file server has held its copy of alpha-1.0+mc.jar only since"),
        "{said}"
    );
    assert_eq!(profile.mods(), [FILE]);

    // A date that cannot be read is no date: the build's date decides, and that is said.
    for header in ["yesterday", "Fri, 13 Nov 2026 20:00:00 +0000", "1794600000"] {
        let stand_in = offering(&content, false).await;
        stand_in.stored(FILE_PATH, header);
        let fresh_profile = self::profile("stored-unreadable", "1.21.4");
        let (outcome, said) = add_with(&fresh_profile, &stand_in.client(&stand_in.authority), Options::default()).await;
        assert!(outcome.unwrap(), "{header}: {said}");
        assert!(said.contains("the file server stated no date for alpha-1.0+mc.jar"), "{header}: {said}");
    }
}

/// Every exchange has a deadline of its own, on top of the HTTP client's idle timeouts: a
/// server that takes the request and then says nothing does not hold a command for ever.
#[tokio::test]
async fn an_exchange_that_gets_no_answer_ends_at_its_deadline() {
    use bastion_mc::net::http::Sink;
    use modrinth::{Call, Transport};
    let stand_in = StandIn::start(&modrinth::HOSTS, false).await;
    stand_in.on("GET", "/v2/project/quiet", 200, "", How::Silent);
    let client = stand_in.client(&stand_in.authority);
    let mut answer: Vec<u8> = Vec::new();
    let sink: &mut (dyn Sink + Send) = &mut answer;
    let started = std::time::Instant::now();
    let call = Call {
        method: hyper::Method::GET,
        url: "https://api.modrinth.com/v2/project/quiet",
        body: None,
        limit: 1024,
        deadline: std::time::Duration::from_millis(400),
    };
    let error = client.call(call, sink).await.unwrap_err();
    assert!(matches!(&error, Error::Net(m) if m.contains("no complete answer within")), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(stand_in.asked("/v2/project/quiet"), 1, "the request did arrive; it is the answer that never came");
}

/// A SOCKS5 proxy that notes what it is asked to connect to and then connects to `upstream`.
async fn socks5_proxy(upstream: SocketAddr) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = asked.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else { break };
            let log = log.clone();
            tokio::spawn(async move {
                let mut greeting = [0u8; 3];
                client.read_exact(&mut greeting).await.unwrap();
                client.write_all(&[5, 0]).await.unwrap();
                let mut head = [0u8; 5];
                client.read_exact(&mut head).await.unwrap();
                assert_eq!(&head[..4], &[5, 1, 0, 3], "CONNECT to a name: the proxy does the resolving");
                let mut name = vec![0u8; head[4] as usize + 2];
                client.read_exact(&mut name).await.unwrap();
                let port = u16::from_be_bytes([name[name.len() - 2], name[name.len() - 1]]);
                name.truncate(name.len() - 2);
                log.lock().unwrap().push(format!("{}:{port}", String::from_utf8(name).unwrap()));
                let mut server = tokio::net::TcpStream::connect(upstream).await.unwrap();
                client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
            });
        }
    });
    (addr, asked)
}

/// With `network.launcher_proxy` set, the registry and its file server are reached through
/// the proxy and by name: this process neither resolves nor dials them.
#[tokio::test]
async fn the_registry_is_reached_through_the_configured_socks5_proxy() {
    let content = mod_jar("alpha", "1.0");
    let stand_in = offering(&content, false).await;
    let (proxy, asked) = socks5_proxy(stand_in.addr).await;
    let mut roots = RootCertStore::empty();
    roots.add(stand_in.authority.clone()).unwrap();
    let hosts: Vec<String> = modrinth::HOSTS.iter().map(|host| host.to_string()).collect();
    // The dial address leads nowhere: with a proxy it must not be used.
    let http = HttpClient::new(tls::client_config(roots, PinSet::default()).unwrap(), &hosts)
        .with_dial_override("127.0.0.1:1".parse().unwrap())
        .with_socks5(Some(proxy));
    let profile = profile("socks", "1.21.4");
    assert!(add(&profile, &Live::with_client(http)).await.unwrap());
    assert_eq!(profile.mods(), [FILE]);
    assert_eq!(asked.lock().unwrap().as_slice(), ["api.modrinth.com:443", "cdn.modrinth.com:443"]);

    // And the client the command builds takes the proxy from the configuration.
    let cfg: bastion_mc::config::Config =
        toml::from_str("[network]\nlauncher_proxy = \"socks5h://127.0.0.1:9050\"\n").unwrap();
    let live = Live::new(&cfg).unwrap();
    assert_eq!(live.proxy(), Some("127.0.0.1:9050".parse().unwrap()));
    assert_eq!(Live::new(&bastion_mc::config::Config::default()).unwrap().proxy(), None);
}

// ------------------------------------------------------------------ against Modrinth itself

fn live() -> Option<Live> {
    if std::env::var_os("BASTION_NET_TESTS").is_none() {
        eprintln!("skipped: set BASTION_NET_TESTS=1 to run tests that contact Modrinth");
        return None;
    }
    Some(Live::new(&bastion_mc::config::Config::default()).expect("a client for the registry"))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// Every built-in alias still names the project it was recorded for. Prints, for the
/// notes, what each has for the newest Minecraft version the test data uses.
#[tokio::test]
async fn live_every_alias_names_the_project_it_was_recorded_for() {
    let Some(client) = live() else { return };
    let registry = Registry::new(&client);
    let now = unix_now();
    registry.project("fabric-api").await.unwrap().expect("Fabric API is on Modrinth");
    let server = registry.server_time().expect("the registry dates its answers");
    assert!(server.abs_diff(now) < 600, "the registry's clock ({server}) and this machine's ({now}) are minutes apart");
    for alias in catalog::ALIASES {
        let project = registry.project(alias.id).await.unwrap().unwrap_or_else(|| panic!("{} is gone", alias.name));
        assert_eq!(project.id, alias.id);
        assert_eq!(project.slug, alias.slug, "{}: the project was renamed; update the alias table", alias.name);
        assert_eq!(project.kind, "mod");
        let mut versions = registry.versions(&project.id, "26.3", &["fabric"]).await.unwrap();
        versions.retain(|v| v.is_listed() && v.supports("26.3", &["fabric"]));
        versions.sort_by_key(|v| std::cmp::Reverse(v.published));
        let newest = |channel: Channel| {
            versions
                .iter()
                .find(|v| v.channel == channel)
                .map(|v| format!("{} ({}, {})", v.number, modrinth::utc(v.published), modrinth::age(now, v.published)))
        };
        eprintln!(
            "{:<18} {} {:<18} {:<20} licence {:<34} 26.3/fabric: release {} | beta {} | alpha {}",
            alias.name,
            project.id,
            project.slug,
            project.title,
            project.license,
            newest(Channel::Release).unwrap_or_else(|| "none".into()),
            newest(Channel::Beta).unwrap_or_else(|| "none".into()),
            newest(Channel::Alpha).unwrap_or_else(|| "none".into()),
        );
        // Whatever is listed, its file must pass this program's checks.
        for version in versions.iter().take(3) {
            version.primary_jar().unwrap_or_else(|e| panic!("{} {}: {e}", alias.name, version.number));
        }
    }
}

/// The whole of `profile add` and `profile check` against the real registry: Lithium
/// (LGPL-3.0, under 1 MiB) for Minecraft 26.3 into a throw-away profile.
#[tokio::test]
async fn live_a_mod_is_added_and_then_known() {
    let Some(client) = live() else { return };
    let profile = profile("live", "26.3");
    let mut out = Output::silent();
    let wanted = ["lithium".to_string()];
    // --allow-fresh: the test must not depend on when Lithium last published.
    let options = Options { allow_fresh: true, ..Default::default() };
    let changed = catalog::add(&profile.paths, &client, "perf", &wanted, options, unix_now(), &mut out).await;
    assert!(changed.unwrap_or_else(|e| panic!("{e}\n{}", out.lines.join("\n"))));
    eprintln!("{}", out.lines.join("\n"));
    // Modrinth's file server dates what it sends (`Last-Modified`); the file-date rule rests on that.
    assert!(!out.lines.iter().any(|line| line.contains("stated no date")), "{:?}", out.lines);
    let manifest = profile.manifest();
    assert_eq!((manifest.serial, manifest.mods.len()), (2, 1));
    let entry = &manifest.mods[0];
    assert!(entry.file.starts_with("lithium-") && entry.file.ends_with(".jar"), "{}", entry.file);
    assert!(entry.source.as_deref().unwrap().starts_with("https://cdn.modrinth.com/data/gvQqBUqZ/versions/"));
    let on_disk = std::fs::read(profile.dir.join("mods").join(&entry.file)).unwrap();
    assert_eq!(util::hex_encode(&util::hash_bytes(HashAlg::Sha256, &on_disk)), entry.sha256);
    // The plan printed the registry's SHA-512; what is on disk has it.
    assert!(out.lines.iter().any(|line| line.contains(&sha512_hex(&on_disk))), "{:?}", out.lines);
    assert!(out.lines.iter().any(|line| line.starts_with("  lithium  (")), "the scan of the new jar is shown");

    let mut out = Output::silent();
    let summary = catalog::check(&profile.paths, &client, "perf", unix_now(), &mut out).await.unwrap();
    eprintln!("{}", out.lines.join("\n"));
    assert_eq!((summary.jars, summary.known, summary.unknown), (1, 1, 0));
    assert!(out.lines.iter().any(|line| line.contains("known: Lithium (lithium · project gvQqBUqZ")));

    // Asking again finds it present and changes nothing.
    let mut out = Output::silent();
    assert!(!catalog::add(&profile.paths, &client, "perf", &wanted, options, unix_now(), &mut out).await.unwrap());
    assert_eq!(profile.manifest().serial, 2);
    // A project that does not exist is a usage error, not a crash.
    let nothing = ["this-slug-should-not-exist-zzz9".to_string()];
    let mut out = Output::silent();
    let error =
        catalog::add(&profile.paths, &client, "perf", &nothing, options, unix_now(), &mut out).await.unwrap_err();
    assert!(matches!(error, Error::Usage(_)), "{error}");
}
