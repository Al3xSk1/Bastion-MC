//! The credential broker end to end: a stand-in for Mojang upstream (real TLS, a throw-away
//! certificate hierarchy), the real broker in the middle, and a client that — like the game —
//! trusts only the broker's per-launch authority and holds only the placeholder.
//!
//! The last test does it with the real JVM inside the real sandbox.

use std::{
    convert::Infallible,
    io::Write,
    net::{Shutdown, SocketAddr},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use bastion_mc::{
    BOOTSTRAP_CLASS, BOOTSTRAP_CLASS_NAME, BOOTSTRAP_CLASS_PATH,
    config::{BrokerConfig, InstanceNetwork, JavaMatch, NetworkConfig, ServerEntry},
    egress::{
        self, Logger, Stats,
        broker::{Authority, Broker, Context, Credential, JoinGate},
    },
    java,
    mojang::args::{Vars, build_frame},
    net::{
        http::{HttpClient, header, sensitive_header},
        tls::{self, PinSet},
    },
    sandbox::{
        self, BaseOptions, Mount, Network, Resolv, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
        trust::TrustStore,
    },
    secret::Secret,
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
};
use tokio_rustls::TlsAcceptor;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");
const API: &str = "api.minecraftservices.com";
const SESSION: &str = "sessionserver.mojang.com";
const REAL_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.REAL-MINECRAFT-ACCESS-TOKEN.c2lnbmF0dXJl";
const PLACEHOLDER: &str = "bastion-placeholder-0123456789abcdef0123456789abcdef";
const PROFILE: &str = "069a79f444e94726a5befca90e38aaf5";

// ---------------------------------------------------------------- stand-in for Mojang

#[derive(Debug, Clone)]
struct Seen {
    host: String,
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

struct Upstream {
    addr: SocketAddr,
    root: CertificateDer<'static>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Upstream {
    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
    fn last(&self) -> Seen {
        self.requests().last().cloned().expect("upstream saw a request")
    }
}

async fn upstream() -> Upstream {
    // root → leaf valid for both API hosts
    let mut root_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.distinguished_name.push(DnType::CommonName, "Stand-in Root");
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
    let root_key = KeyPair::generate().unwrap();
    let root = root_params.self_signed(&root_key).unwrap().der().clone();
    let leaf_params =
        CertificateParams::new(vec![API.to_string(), SESSION.to_string(), "api.mojang.com".to_string()]).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = leaf_params.signed_by(&leaf_key, &Issuer::from_params(&root_params, &root_key)).unwrap().der().clone();

    let config = ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())))
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { break };
            let (acceptor, record) = (acceptor.clone(), record.clone());
            tokio::spawn(async move {
                let Ok(stream) = acceptor.accept(tcp).await else { return };
                let service = service_fn(move |request: Request<Incoming>| {
                    let record = record.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        let body = body.collect().await.map(|b| b.to_bytes()).unwrap_or_default();
                        let headers: Vec<(String, String)> = parts
                            .headers
                            .iter()
                            .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
                            .collect();
                        let authorization = headers
                            .iter()
                            .find(|(k, _)| k == "authorization")
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default();
                        let target = parts.uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default();
                        record.lock().unwrap().push(Seen {
                            host: headers.iter().find(|(k, _)| k == "host").map(|(_, v)| v.clone()).unwrap_or_default(),
                            method: parts.method.to_string(),
                            target: target.clone(),
                            headers,
                            body: String::from_utf8_lossy(&body).into_owned(),
                        });
                        let reply = |status: u16, text: String| {
                            Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .header("etag", "\"v1\"")
                                .header("set-cookie", "upstream-session=1")
                                .header("x-internal", "not for the game")
                                .body(Full::new(Bytes::from(text)))
                                .unwrap()
                        };
                        Ok::<_, Infallible>(match parts.uri.path() {
                            "/player/attributes" => {
                                reply(200, r#"{"privileges":{"onlineChat":{"enabled":true}}}"#.into())
                            }
                            "/publickeys" => reply(200, r#"{"profilePropertyKeys":[]}"#.into()),
                            "/session/minecraft/join" => {
                                Response::builder().status(204).body(Full::new(Bytes::new())).unwrap()
                            }
                            // A misbehaving endpoint that echoes the credential it was sent.
                            "/player/certificates" => reply(200, format!(r#"{{"echo":"{authorization}"}}"#)),
                            // One that answers in an encoding nobody asked for (and could hide
                            // an echo of the credential in it).
                            "/encoded" => Response::builder()
                                .status(200)
                                .header("content-type", "application/json")
                                .header("content-encoding", "gzip")
                                .body(Full::new(Bytes::from_static(b"\x1f\x8b\x08 not really gzip")))
                                .unwrap(),
                            // Large objects: `/blob/<n>` is n MiB.
                            path if path.starts_with("/blob/") => {
                                let megabytes: usize = path["/blob/".len()..].parse().unwrap_or(1);
                                Response::builder()
                                    .status(200)
                                    .header("content-type", "application/octet-stream")
                                    .body(Full::new(Bytes::from(vec![b'x'; megabytes << 20])))
                                    .unwrap()
                            }
                            "/privacy/blocklist" => reply(429, r#"{"error":"TOO_MANY_REQUESTS"}"#.into()),
                            _ => reply(404, r#"{"error":"NOT_FOUND"}"#.into()),
                        })
                    }
                });
                let _ =
                    hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
            });
        }
    });
    Upstream { addr, root, seen }
}

// ---------------------------------------------------------------- the broker under test

struct Inner {
    broker: Broker,
    gate: JoinGate,
    stats: Stats,
    log: Logger,
    lines: Arc<Mutex<Vec<String>>>,
}

struct Harness {
    upstream: Upstream,
    inner: Arc<Inner>,
    authority_der: Vec<u8>,
}

impl Harness {
    async fn new(config: BrokerConfig, signed_in: bool) -> Self {
        Self::build(config, signed_in, None).await
    }

    /// `budget`: the broker's budget for buffered bodies and how long a request waits for
    /// room in it, where a test needs other values than the launcher's.
    async fn build(config: BrokerConfig, signed_in: bool, budget: Option<(usize, Duration)>) -> Self {
        let upstream = upstream().await;
        let mut roots = RootCertStore::empty();
        roots.add(upstream.root.clone()).unwrap();
        let to_upstream = HttpClient::new(tls::client_config(roots, PinSet::default()).unwrap(), &[])
            .with_dial_override(upstream.addr);
        let authority = Authority::generate().unwrap();
        let authority_der = authority.certificate_der().to_vec();
        let credential = signed_in.then(|| Credential {
            token: Secret::from_slice(REAL_TOKEN.as_bytes()).unwrap(),
            profile_id: PROFILE.into(),
        });
        let mut broker = Broker::new(authority, &config, &to_upstream, PLACEHOLDER.into(), credential).unwrap();
        if let Some((bytes, wait)) = budget {
            broker = broker.with_buffer_budget(bytes, wait);
        }
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        let log: Logger = Arc::new(move |line: String| sink.lock().unwrap().push(line));
        Harness {
            upstream,
            inner: Arc::new(Inner { broker, gate: JoinGate::default(), stats: Stats::default(), log, lines }),
            authority_der,
        }
    }

    /// A listener whose connections are served by the broker as `name`, the way the relay
    /// does it for a connection to that name's address. Returns where to dial.
    async fn serve_as(&self, name: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let inner = self.inner.clone();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { break };
                // A small, fixed send buffer: what a client does not read stays with the
                // broker, where the tests can see it, whatever this machine's TCP tuning is.
                let _ = nix::sys::socket::setsockopt(&tcp, nix::sys::socket::sockopt::SndBuf, &(16 * 1024));
                let inner = inner.clone();
                tokio::spawn(async move {
                    let ctx = Context { gate: &inner.gate, log: &inner.log, stats: &inner.stats };
                    if let Err(why) = inner.broker.serve(&ctx, tcp, name).await {
                        (inner.log)(format!("serve error: {why}"));
                    }
                });
            }
        });
        addr
    }

    /// The game's side: trusts the broker's authority and nothing else.
    async fn game(&self, name: &'static str) -> HttpClient {
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(self.authority_der.clone())).unwrap();
        HttpClient::new(tls::client_config(roots, PinSet::default()).unwrap(), &[])
            .with_domains(&["mojang.com", "minecraftservices.com"])
            .with_dial_override(self.serve_as(name).await)
    }

    fn log(&self) -> Vec<String> {
        self.inner.lines.lock().unwrap().clone()
    }

    /// A game connection that sends `request` and then reads nothing: TLS to the broker at
    /// `addr`, trusting its authority, with a receive buffer too small to take a reply off
    /// the broker's hands.
    async fn stalling_game(&self, addr: SocketAddr, request: &str) -> tokio_rustls::client::TlsStream<TcpStream> {
        use tokio::io::AsyncWriteExt;
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(self.authority_der.clone())).unwrap();
        let config = tls::client_config(roots, PinSet::default()).unwrap();
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(4096).unwrap();
        let tcp = socket.connect(addr).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from(API).unwrap();
        let mut stream = tokio_rustls::TlsConnector::from(config).connect(name, tcp).await.expect("TLS to the broker");
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        stream
    }
}

/// Poll `condition` (bounded) until it holds.
async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..400 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never happened: {what}");
}

fn bearer(token: &str) -> Vec<(hyper::header::HeaderName, hyper::header::HeaderValue)> {
    vec![
        sensitive_header("authorization", format!("Bearer {token}").as_bytes()).unwrap(),
        header("cookie", "tracking=1").unwrap(),
        header("x-smuggled", "header").unwrap(),
        header("user-agent", "Minecraft Java/26.3").unwrap(),
        header("accept", "application/json").unwrap(),
    ]
}

async fn send(
    client: &HttpClient,
    method: Method,
    url: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let mut headers = token.map(bearer).unwrap_or_default();
    if body.is_some() {
        headers.push(header("content-type", "application/json; charset=utf-8").unwrap());
    }
    let reply = client
        .request(method, url, &headers, body.map(|b| Bytes::from(b.to_string())), 1 << 20)
        .await
        .unwrap_or_else(|e| panic!("request to the broker failed: {e}"));
    (reply.status, String::from_utf8_lossy(&reply.body).into_owned())
}

fn join_request(server_id: &str) -> String {
    format!(r#"{{"accessToken":"{PLACEHOLDER}","selectedProfile":"{PROFILE}","serverId":"{server_id}"}}"#)
}

// ---------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread")]
async fn the_placeholder_becomes_the_real_token_only_on_the_way_out() {
    let h = Harness::new(BrokerConfig::default(), true).await;
    let game = h.game(API).await;

    let (status, body) =
        send(&game, Method::GET, "https://api.minecraftservices.com/player/attributes", Some(PLACEHOLDER), None).await;
    assert_eq!((status, body.as_str()), (200, r#"{"privileges":{"onlineChat":{"enabled":true}}}"#));
    let seen = h.upstream.last();
    assert_eq!((seen.method.as_str(), seen.target.as_str(), seen.host.as_str()), ("GET", "/player/attributes", API));
    assert_eq!(
        seen.header("authorization"),
        Some(format!("Bearer {REAL_TOKEN}").as_str()),
        "Mojang gets the real token"
    );
    assert_eq!(seen.header("user-agent"), Some("Minecraft Java/26.3"), "the game's identification is kept");
    assert_eq!(seen.header("accept"), Some("application/json"));
    assert_eq!(seen.header("cookie"), None, "cookies are not relayed");
    assert_eq!(seen.header("x-smuggled"), None, "unknown headers are not relayed");
    assert!(!format!("{seen:?}").contains(PLACEHOLDER), "the placeholder never leaves the machine");
    assert!(
        h.log().iter().any(|l| l.contains("GET /player/attributes → 200 [credential attached: account_read]")),
        "{:?}",
        h.log()
    );
    assert!(!h.log().iter().any(|l| l.contains(REAL_TOKEN)), "the token is never logged");
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_that_need_no_credential_get_none() {
    let h = Harness::new(BrokerConfig::default(), true).await;
    let game = h.game(API).await;
    // Presenting the placeholder to an endpoint that is not on the list does not unlock it.
    let (status, body) =
        send(&game, Method::GET, "https://api.minecraftservices.com/publickeys?x=1", Some(PLACEHOLDER), None).await;
    assert_eq!((status, body.as_str()), (200, r#"{"profilePropertyKeys":[]}"#));
    let seen = h.upstream.last();
    assert_eq!(seen.target, "/publickeys?x=1", "the query is passed on");
    assert_eq!(seen.header("authorization"), None);
    // Somebody else's token is not relayed either.
    let _ =
        send(&game, Method::GET, "https://api.minecraftservices.com/player/attributes", Some("another-token"), None)
            .await;
    assert_eq!(h.upstream.last().header("authorization"), None);
    // Upstream errors come back as they are.
    let (status, body) =
        send(&game, Method::GET, "https://api.minecraftservices.com/privacy/blocklist", Some(PLACEHOLDER), None).await;
    assert_eq!((status, body.as_str()), (429, r#"{"error":"TOO_MANY_REQUESTS"}"#));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_listed_response_headers_reach_the_game() {
    let h = Harness::new(BrokerConfig::default(), true).await;
    let game = h.game(API).await;
    let mut connection = game.connect(API).await.unwrap();
    let mut body = Vec::new();
    let head = connection
        .call(Method::GET, "/player/attributes", &bearer(PLACEHOLDER), None, 1 << 20, &mut body)
        .await
        .unwrap();
    assert_eq!(head.status, 200);
    assert_eq!(head.headers.get("etag").unwrap(), "\"v1\"");
    assert_eq!(head.headers.get("content-type").unwrap(), "application/json");
    assert!(head.headers.get("set-cookie").is_none() && head.headers.get("x-internal").is_none());
    // The same connection serves the next request (and upstream is reused too).
    let mut second = Vec::new();
    let head = connection.call(Method::GET, "/publickeys", &Vec::new(), None, 1 << 20, &mut second).await.unwrap();
    assert_eq!(head.status, 200);
    assert_eq!(h.upstream.requests().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_account_cannot_be_changed_and_nothing_of_the_attempt_leaves() {
    let h = Harness::new(BrokerConfig::default(), true).await;
    let game = h.game(API).await;
    for (method, path, body) in [
        (Method::PUT, "/minecraft/profile/capes/active", Some(r#"{"capeId":"x"}"#)),
        (Method::DELETE, "/minecraft/profile/capes/active", None),
        (Method::POST, "/minecraft/profile/skins", Some(r#"{"variant":"slim","url":"http://x"}"#)),
        (Method::DELETE, "/minecraft/profile/skins/active", None),
        (Method::PUT, "/minecraft/profile/name/Hijacked", None),
        (Method::POST, "/player/attributes", Some(r#"{"profanityFilterPreferences":{"profanityFilterOn":false}}"#)),
        (Method::PUT, "/friends", Some(r#"{"updateType":"ADD","profileId":"x"}"#)),
        (Method::POST, "/player/report", Some("{}")),
        (Method::POST, "/events", Some("{}")),
        (Method::POST, "/brand/new/endpoint", Some("{}")),
    ] {
        let url = format!("https://api.minecraftservices.com{path}");
        let (status, text) = send(&game, method.clone(), &url, Some(PLACEHOLDER), body).await;
        assert_eq!(status, 403, "{method} {path}: {text}");
        assert!(text.contains("bastion-mc"), "the refusal says who refused: {text}");
    }
    assert!(h.upstream.requests().is_empty(), "nothing was forwarded: {:?}", h.upstream.requests());
    let log = h.log();
    assert!(log.iter().any(|l| l.contains("PUT /minecraft/profile/capes/active → 403 here [SECURITY]")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("network.broker.friends = false")), "the log says how to allow what is off");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_needs_a_game_connection_and_is_rebuilt() {
    let h = Harness::new(BrokerConfig::default(), true).await;
    let game = h.game(SESSION).await;
    let url = "https://sessionserver.mojang.com/session/minecraft/join";

    let (status, text) = send(&game, Method::POST, url, None, Some(&join_request("-3f9a01bc"))).await;
    assert_eq!(status, 403, "{text}");
    assert!(h.upstream.requests().is_empty());

    let connection = h.inner.gate.opened("play.example.org:25565");
    let (status, text) = send(&game, Method::POST, url, None, Some(&join_request("-3f9a01bc"))).await;
    assert_eq!((status, text.as_str()), (204, ""));
    let seen = h.upstream.last();
    assert_eq!(seen.host, SESSION);
    assert_eq!(
        seen.body,
        format!(r#"{{"accessToken":"{REAL_TOKEN}","selectedProfile":"{PROFILE}","serverId":"-3f9a01bc"}}"#)
    );
    assert_eq!(seen.header("content-type"), Some("application/json; charset=utf-8"));
    assert!(h.log().iter().any(|l| l.contains("join authorised for game connection play.example.org:25565")));

    // The same connection does not buy a second join; a join for someone else's profile or
    // with extra members is refused whatever is open.
    let (status, _) = send(&game, Method::POST, url, None, Some(&join_request("1234abcd"))).await;
    assert_eq!(status, 403);
    drop(connection);
    let _fresh = h.inner.gate.opened("play.example.org:25565");
    let other = format!(
        r#"{{"accessToken":"{PLACEHOLDER}","selectedProfile":"ffffffffffffffffffffffffffffffff","serverId":"ab"}}"#
    );
    assert_eq!(send(&game, Method::POST, url, None, Some(&other)).await.0, 403);
    let extra = format!(r#"{{"accessToken":"{PLACEHOLDER}","selectedProfile":"{PROFILE}","serverId":"ab","x":"y"}}"#);
    assert_eq!(send(&game, Method::POST, url, None, Some(&extra)).await.0, 400);
    assert_eq!(h.upstream.requests().len(), 1, "only the one legitimate join went out");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_that_contains_the_token_is_withheld() {
    let h = Harness::new(BrokerConfig::default(), true).await;
    let game = h.game(API).await;
    let (status, text) = send(
        &game,
        Method::POST,
        "https://api.minecraftservices.com/player/certificates",
        Some(PLACEHOLDER),
        Some("{}"),
    )
    .await;
    assert_eq!(h.upstream.last().header("authorization"), Some(format!("Bearer {REAL_TOKEN}").as_str()));
    assert_eq!(status, 502, "{text}");
    assert!(!text.contains(REAL_TOKEN), "the token must not come back to the game: {text}");
    assert!(h.log().iter().any(|l| l.contains("[SECURITY]") && l.contains("withheld")), "{:?}", h.log());
}

/// A reply to a request that carried the token is checked for the token as it arrived. If
/// it arrives in an encoding (which the launcher did not ask for), that check would be
/// blind: such a reply is withheld. The same answer to a request that carried no credential
/// has nothing to leak and is passed on.
#[tokio::test(flavor = "multi_thread")]
async fn an_encoded_reply_to_a_credentialed_request_is_withheld() {
    let config = BrokerConfig {
        extra_token_endpoints: vec!["GET api.minecraftservices.com/encoded".into()],
        ..BrokerConfig::default()
    };
    let h = Harness::new(config, true).await;
    let game = h.game(API).await;
    let url = "https://api.minecraftservices.com/encoded";
    let (status, text) = send(&game, Method::GET, url, Some(PLACEHOLDER), None).await;
    assert_eq!(h.upstream.last().header("authorization"), Some(format!("Bearer {REAL_TOKEN}").as_str()));
    assert_eq!(h.upstream.last().header("accept-encoding"), Some("identity"), "the launcher asked for no encoding");
    assert_eq!(status, 502, "{text}");
    assert!(text.contains("came back encoded") && !text.contains("not really gzip"), "{text}");
    assert!(h.log().iter().any(|l| l.contains("[SECURITY]") && l.contains("encoded")), "{:?}", h.log());

    // Without the credential the token was never sent, so there is nothing to check for.
    let (status, text) = send(&game, Method::GET, url, None, None).await;
    assert_eq!(h.upstream.last().header("authorization"), None);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains("not really gzip"));
}

/// A game that asks for large objects on many connections and never reads the answers gets
/// the launcher to hold at most one budget's worth of them — not one reply per connection —
/// and is refused beyond that; room comes back when a reply is read or its connection ends.
#[tokio::test(flavor = "multi_thread")]
async fn replies_the_game_does_not_read_come_out_of_one_bounded_budget() {
    const MIB: usize = 1 << 20;
    // Room for two replies of the largest size (8 MiB is reserved before a reply's real size
    // is known), and a wait short enough for a test.
    let h = Harness::build(BrokerConfig::default(), true, Some((16 * MIB, Duration::from_millis(500)))).await;
    let broker = &h.inner.broker;
    let addr = h.serve_as(API).await;
    let blob = "GET /blob/7 HTTP/1.1\r\nHost: api.minecraftservices.com\r\n\r\n";
    let fetched = || h.log().iter().filter(|l| l.contains("GET /blob/7 → 200")).count();

    // Two connections each have the launcher fetch 7 MiB and then do not read it.
    let first = h.stalling_game(addr, blob).await;
    let second = h.stalling_game(addr, blob).await;
    eventually("both replies were fetched", || fetched() == 2).await;
    assert_eq!(broker.buffered_bytes(), 14 * MIB, "each unread reply occupies its size, no more");

    // A third finds no room for a reply (2 MiB are left, 8 would be reserved): it waits, is
    // refused, and Mojang is not even asked.
    let third = h.stalling_game(addr, blob).await;
    eventually("the third request was refused", || h.log().iter().any(|l| l.contains("GET /blob/7 → 503 here"))).await;
    assert_eq!(fetched(), 2);
    assert_eq!(h.upstream.requests().iter().filter(|s| s.target == "/blob/7").count(), 2);
    assert!(broker.buffered_bytes() <= 16 * MIB, "never more than the budget: {}", broker.buffered_bytes());
    let mut third = third;
    let mut refusal = vec![0u8; 4096];
    let n = third.read(&mut refusal).await.unwrap();
    let refusal = String::from_utf8_lossy(&refusal[..n]).into_owned();
    assert!(refusal.starts_with("HTTP/1.1 503") && refusal.contains("already holding"), "{refusal}");
    // An ordinary small request is refused for the same reason while the two are stalled.
    let game = h.game(API).await;
    let (status, _) = send(&game, Method::GET, "https://api.minecraftservices.com/publickeys", None, None).await;
    assert_eq!(status, 503);

    // The first connection reads its reply after all: its room comes back.
    let mut first = first;
    let mut got = 0usize;
    let mut chunk = vec![0u8; 64 * 1024];
    while got < 7 * MIB {
        let n = tokio::time::timeout(Duration::from_secs(20), first.read(&mut chunk)).await.expect("reply").unwrap();
        assert!(n > 0, "the reply ended after {got} bytes");
        got += n;
    }
    eventually("the first reply's room was given back", || broker.buffered_bytes() <= 7 * MIB).await;
    let (status, _) = send(&game, Method::GET, "https://api.minecraftservices.com/publickeys", None, None).await;
    assert_eq!(status, 200, "with room again, requests are served again");

    // The second hangs up without ever reading: that gives its room back as well.
    drop(second);
    drop(first);
    drop(third);
    eventually("everything was given back", || broker.buffered_bytes() == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_account_nothing_is_authenticated() {
    let h = Harness::new(BrokerConfig::default(), false).await;
    let game = h.game(API).await;
    let (status, _) =
        send(&game, Method::GET, "https://api.minecraftservices.com/player/attributes", Some(PLACEHOLDER), None).await;
    assert_eq!(status, 401);
    let (status, _) = send(&game, Method::GET, "https://api.minecraftservices.com/publickeys", None, None).await;
    assert_eq!(status, 200);
    assert_eq!(h.upstream.requests().len(), 1);
    assert_eq!(h.upstream.last().header("authorization"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_certificate_is_for_the_resolved_name_only() {
    let h = Harness::new(BrokerConfig::default(), true).await;
    // A connection that the relay attributes to the session server, but whose TLS hello asks
    // for the API host: refused before any certificate is produced.
    let addr = h.serve_as(SESSION).await;
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(h.authority_der.clone())).unwrap();
    let confused = HttpClient::new(tls::client_config(roots, PinSet::default()).unwrap(), &[]).with_dial_override(addr);
    assert!(confused.get("https://api.minecraftservices.com/publickeys", 1 << 16).await.is_err());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.log().iter().any(|l| l.contains("does not match the resolved name")), "{:?}", h.log());

    // A client that trusts the ordinary roots (not the per-launch authority) does not accept
    // the broker: its certificates mean nothing outside this sandbox.
    let addr = h.serve_as(API).await;
    let outsider =
        HttpClient::new(tls::client_config(tls::load_system_roots().unwrap(), PinSet::default()).unwrap(), &[])
            .with_dial_override(addr);
    assert!(outsider.get("https://api.minecraftservices.com/publickeys", 1 << 16).await.is_err());
    assert!(h.upstream.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_upstream_is_a_gateway_error_not_a_hang() {
    let h = Harness::new(BrokerConfig::default(), true).await;
    // api.mojang.com is in the stand-in's certificate; point the broker's client nowhere
    // by asking for a host the stand-in has no certificate for.
    let game = h.game("status.mojang.com").await;
    let (status, text) = send(&game, Method::GET, "https://status.mojang.com/check", None, None).await;
    assert_eq!(status, 502, "{text}");
    assert!(text.contains("could not be relayed"));
}

// ---------------------------------------------------------------- the real JVM, sandboxed

fn compile(source: &str) -> std::path::PathBuf {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("broker-classes-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/java").join(source);
    let status =
        Command::new("javac").args(["--release", "8", "-Xlint:-options", "-d"]).arg(&out).arg(src).status().unwrap();
    assert!(status.success(), "javac failed");
    out
}

/// The whole path, as in a launch: JVM in the sandbox with the generated trust store and the
/// placeholder; fake-IP DNS; relay; broker; a stand-in for Mojang.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_jvm_holding_only_the_placeholder() {
    let classes = compile("BrokerProbe.java");
    let jvms = java::discover().expect("system JVMs");
    let jvm = java::select(&jvms, 8, JavaMatch::Newer, None).expect("a JVM");

    // A "game server" on the host's loopback. It answers what it is sent, as a real one does
    // before a client can ask for a join: that answer is how the probe knows that its game
    // connection exists end to end (and is therefore known to the relay).
    let game_server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let game_port = game_server.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in game_server.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = [0u8; 64];
                while let Ok(n @ 1..) = std::io::Read::read(&mut stream, &mut buf) {
                    if std::io::Write::write_all(&mut stream, &buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });

    let up = upstream().await;
    let mut roots = RootCertStore::empty();
    roots.add(up.root.clone()).unwrap();
    let to_upstream =
        HttpClient::new(tls::client_config(roots, PinSet::default()).unwrap(), &[]).with_dial_override(up.addr);
    let authority = Authority::generate().unwrap();
    let trust = TrustStore::build(Some(("bastion-mc-credential-broker", authority.certificate_der()))).unwrap();
    let credential =
        Credential { token: Secret::from_slice(REAL_TOKEN.as_bytes()).unwrap(), profile_id: PROFILE.into() };
    let broker =
        Broker::new(authority, &BrokerConfig::default(), &to_upstream, PLACEHOLDER.into(), Some(credential)).unwrap();

    let net = NetworkConfig {
        servers: vec![ServerEntry::Full {
            host: "127.0.0.1".into(),
            port: Some(game_port),
            allow_private: true,
            voice: None,
            udp: vec![],
        }],
        ..Default::default()
    };
    let policy = egress::Policy::new(&net, &InstanceNetwork::default(), None).unwrap();
    let ns = policy.netns_spec();

    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 256,
        jvm: Some(&jvm),
        exec_allow: &[],
        resolv: Resolv::Loopback,
        expose_timezone: false,
        trust: Some(&trust),
    })
    .unwrap();
    base.mounts.push(Mount::RoData {
        data: BOOTSTRAP_CLASS.to_vec(),
        dst: format!("/bastion/boot/{BOOTSTRAP_CLASS_PATH}").into(),
        mode: 0o444,
    });
    base.mounts.push(Mount::RoBind { src: classes.clone(), dst: "/bastion/test".into() });
    base.mounts.push(Mount::Dir { dst: "/game".into(), mode: 0o700 });
    base.landlock.write.push("/game".into());
    base.landlock.tcp = TcpPolicy::Ports(policy.tcp_ports());
    let mut argv = vec!["java".to_string(), "-Xmx256m".into(), "-cp".into(), "/bastion/boot:/bastion/test".into()];
    argv.extend(java::hardening_flags(jvm.major, false));
    argv.push(BOOTSTRAP_CLASS_NAME.into());
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::Filtered(ns.clone()),
        cwd: "/game".into(),
        limits: None,
        policy: InitPolicy {
            exec: jvm.java.clone(),
            argv,
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };

    let mut vars = Vars::default();
    vars.set("placeholder", PLACEHOLDER).set("port", game_port.to_string());
    let frame = build_frame("BrokerProbe", &["${placeholder}".to_string(), "${port}".to_string()], &vars).unwrap();
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    running.channel.write_all(frame.expose()).unwrap();
    running.channel.shutdown(Shutdown::Write).unwrap();

    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let relay = egress::start(
        policy,
        &ns,
        std::mem::take(&mut running.egress),
        to_upstream.clone(),
        Arc::new(move |l| sink.lock().unwrap().push(l)),
        egress::SrvTargets::new(),
        Some(broker),
    )
    .unwrap();

    let mut out = running.child.stdout.take().unwrap();
    let mut err = running.child.stderr.take().unwrap();
    let (mut so, mut se) = (String::new(), String::new());
    let status = tokio::time::timeout(Duration::from_secs(120), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut so), err.read_to_string(&mut se));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("JVM timed out");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let brokered = relay.stats.brokered.load(std::sync::atomic::Ordering::Relaxed);
    let credentialed = relay.stats.credentialed.load(std::sync::atomic::Ordering::Relaxed);
    relay.shutdown();
    let log = lines.lock().unwrap().join("\n");
    let get = |key: &str| -> Vec<String> {
        so.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .unwrap_or_else(|| {
                panic!("missing {key}\n--- stdout ---\n{so}\n--- stderr ---\n{se}\n--- relay ---\n{log}")
            })
            .split('|')
            .map(str::to_string)
            .collect()
    };
    assert_eq!(status.code(), Some(0), "JVM failed\n--- stdout ---\n{so}\n--- stderr ---\n{se}\n--- relay ---\n{log}");
    assert_eq!(get("done"), ["ok"]);
    assert_eq!(get("truststore"), ["/etc/ssl/certs/java/cacerts"]);

    // The JVM completed TLS against the broker's per-launch authority…
    let attributes = get("attributes");
    assert_eq!(attributes[0], "200", "{attributes:?}\n{se}\n{log}");
    assert_eq!(attributes[1], r#"{"privileges":{"onlineChat":{"enabled":true}}}"#);
    assert!(attributes[2].contains("bastion-mc credential broker"), "issuer seen by the JVM: {}", attributes[2]);
    assert_eq!(attributes[3], "\"v1\"", "listed headers come through");
    assert_eq!(attributes[4], "null", "Set-Cookie does not");
    // …and Mojang got the real token for it, which the JVM never had.
    let seen = up.requests();
    let for_attributes = seen.iter().find(|s| s.target == "/player/attributes").expect("forwarded");
    assert_eq!(for_attributes.header("authorization"), Some(format!("Bearer {REAL_TOKEN}").as_str()));
    assert_eq!(for_attributes.header("cookie"), None);
    assert!(!so.contains(REAL_TOKEN) && !se.contains(REAL_TOKEN), "the real token is nowhere in the JVM's output");

    assert_eq!(get("publickeys")[0], "200");
    assert_eq!(seen.iter().find(|s| s.target == "/publickeys").unwrap().header("authorization"), None);
    for refused in ["cape", "skin", "friends"] {
        assert_eq!(get(refused)[0], "403", "{refused}: {:?}", get(refused));
    }
    assert!(!seen.iter().any(|s| s.target.contains("/minecraft/profile") || s.target == "/friends"));

    // Joining: refused cold, allowed once while a game connection is open.
    assert_eq!(get("join_cold")[0], "403");
    assert_eq!(get("join_warm")[0], "204", "{:?}\n{log}", get("join_warm"));
    assert_eq!(get("join_again")[0], "403");
    let joins: Vec<&Seen> = seen.iter().filter(|s| s.target == "/session/minecraft/join").collect();
    assert_eq!(joins.len(), 1);
    assert!(joins[0].body.contains(REAL_TOKEN) && !joins[0].body.contains(PLACEHOLDER));

    // A name outside the allowlist does not even resolve.
    assert!(get("other")[0] == "exception", "{:?}", get("other"));
    assert!(log.contains("SECURITY"), "{log}");
    assert!(!log.contains(REAL_TOKEN), "the relay's log never contains the token");
    assert_eq!(
        (brokered, credentialed),
        (3, 2),
        "attributes + publickeys + join forwarded; two with the credential\n{log}"
    );
    let _ = std::fs::remove_dir_all(classes);
}
