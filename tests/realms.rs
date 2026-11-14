//! Realms through the credential broker, end to end: a stand-in for the Realms service (real
//! TLS, a throw-away certificate hierarchy, scripted answers), the real broker in the middle,
//! a client that — like the game — trusts only the broker's per-launch authority and holds
//! only the placeholder; the approval as the phone gives it; the real dialer in its sandbox;
//! and, in the last test, the real relay with a sandboxed client in a private network
//! namespace standing in for the game.
//!
//! The live Realms service is not used: it needs a signed-in account with a Realm.

use std::{
    convert::Infallible,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener as StdListener},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use bastion_mc::{
    config::{BrokerConfig, InstanceNetwork, NetworkConfig},
    egress::{
        self, Logger, Stats,
        broker::{Authority, Broker, Context, Credential, JoinGate, RealmsBroker},
    },
    net::{
        http::{HttpClient, header, sensitive_header},
        tls::{self, PinSet},
    },
    realms::{
        Approve, PASS_ADDRESS,
        approval::{Announcement, Approvals, Outcome},
        dial::Dialer,
    },
    sandbox::{
        self, BaseOptions, Network, Resolv, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
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
use tokio::{io::AsyncReadExt, net::TcpListener};
use tokio_rustls::TlsAcceptor;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");
const PC: &str = "pc.realms.minecraft.net";
const FRONTEND: &str = "java.frontend.realms.minecraft-services.net";
const REAL_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.REAL-MINECRAFT-ACCESS-TOKEN.c2lnbmF0dXJl";
const PLACEHOLDER: &str = "bastion-placeholder-0123456789abcdef0123456789abcdef";
const PROFILE: &str = "069a79f444e94726a5befca90e38aaf5";

// ---------------------------------------------------------------- the stand-in Realms service

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
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
}

/// Realms as the stand-in plays it. `server` is where it says Realm 7's server is.
async fn upstream(server: String) -> Upstream {
    let mut root_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.distinguished_name.push(DnType::CommonName, "Stand-in Realms Root");
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
    let root_key = KeyPair::generate().unwrap();
    let root = root_params.self_signed(&root_key).unwrap().der().clone();
    let leaf_params = CertificateParams::new(vec![
        PC.to_string(),
        "java.frontendlegacy.realms.minecraft-services.net".to_string(),
        FRONTEND.to_string(),
    ])
    .unwrap();
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
            let (acceptor, record, server) = (acceptor.clone(), record.clone(), server.clone());
            tokio::spawn(async move {
                let Ok(stream) = acceptor.accept(tcp).await else { return };
                let service = service_fn(move |request: Request<Incoming>| {
                    let (record, server) = (record.clone(), server.clone());
                    async move {
                        let (parts, body) = request.into_parts();
                        let _ = body.collect().await;
                        let headers: Vec<(String, String)> = parts
                            .headers
                            .iter()
                            .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
                            .collect();
                        let authorized = headers.iter().any(|(k, v)| {
                            (k == "cookie" && v.starts_with(&format!("sid=token:{REAL_TOKEN}:{PROFILE}")))
                                || (k == "x-mojangauth" && v == REAL_TOKEN)
                        });
                        record.lock().unwrap().push(Seen {
                            method: parts.method.to_string(),
                            target: parts.uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default(),
                            headers,
                        });
                        let reply = |status: u16, text: String| {
                            Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .header("set-cookie", "realms-internal=1")
                                .body(Full::new(Bytes::from(text)))
                                .unwrap()
                        };
                        if !authorized {
                            return Ok::<_, Infallible>(reply(
                                401,
                                r#"{"errorCode":401,"errorMsg":"Unauthorized"}"#.into(),
                            ));
                        }
                        Ok::<_, Infallible>(match parts.uri.path() {
                            "/worlds" => reply(
                                200,
                                r#"{"servers":[{"id":7,"name":"Our Realm","state":"OPEN"},{"id":8,"name":"Other"}]}"#
                                    .into(),
                            ),
                            "/mco/available" => reply(200, "true".into()),
                            "/worlds/v1/7/join/pc" => reply(
                                200,
                                format!(
                                    r#"{{"address":"{server}","resourcePackUrl":null,"resourcePackHash":null,"sessionRegionData":{{"regionName":"EU"}}}}"#
                                ),
                            ),
                            "/worlds/v1/8/join/pc" => reply(503, r#"{"errorCode":503,"errorMsg":"starting"}"#.into()),
                            path if path.starts_with("/api/v1.0/invite/codes/use/") => reply(200, "{}".into()),
                            _ => reply(404, r#"{"errorCode":404,"errorMsg":"Not Found"}"#.into()),
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

/// A TCP echo server on the host's loopback, standing in for a Realm's game server.
fn echo_server() -> SocketAddr {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { break };
            std::thread::spawn(move || {
                let mut buf = [0u8; 256];
                while let Ok(n) = c.read(&mut buf) {
                    if n == 0 || c.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

// ---------------------------------------------------------------- the broker under test

type Heard = Arc<Mutex<Vec<Announcement>>>;

struct Harness {
    upstream: Upstream,
    broker: Arc<Broker>,
    approvals: Arc<Approvals>,
    heard: Heard,
    gate: Arc<JoinGate>,
    stats: Arc<Stats>,
    log: Logger,
    lines: Arc<Mutex<Vec<String>>>,
    authority_der: Vec<u8>,
}

fn quiet_log() -> Arc<dyn Fn(String) + Send + Sync> {
    Arc::new(|_| {})
}

impl Harness {
    async fn new(server: String, signed_in: bool, dialer: Option<Arc<Dialer>>) -> Self {
        let upstream = upstream(server).await;
        let mut roots = RootCertStore::empty();
        roots.add(upstream.root.clone()).unwrap();
        let to_upstream = HttpClient::new(tls::client_config(roots, PinSet::default()).unwrap(), &[])
            .with_dial_override(upstream.addr);
        let authority = Authority::generate_for(true).unwrap();
        let authority_der = authority.certificate_der().to_vec();
        let credential = signed_in.then(|| Credential {
            token: Secret::from_slice(REAL_TOKEN.as_bytes()).unwrap(),
            profile_id: PROFILE.into(),
        });
        let heard: Heard = Arc::default();
        let sink = heard.clone();
        let approvals = Approvals::new(
            Approve::Code,
            None,
            PASS_ADDRESS,
            Arc::new(move |a: &Announcement| sink.lock().unwrap().push(a.clone())),
        );
        let broker = Broker::new(authority, &BrokerConfig::default(), &to_upstream, PLACEHOLDER.into(), credential)
            .unwrap()
            .with_realms(RealmsBroker { approvals: approvals.clone(), dialer, loopback_for_tests: true })
            .unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        let log: Logger = Arc::new(move |line: String| sink.lock().unwrap().push(line));
        Harness {
            upstream,
            broker: Arc::new(broker),
            approvals,
            heard,
            gate: Arc::default(),
            stats: Arc::default(),
            log,
            lines,
            authority_der,
        }
    }

    /// Where the game dials to reach the broker as `name`.
    async fn serve_as(&self, name: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (broker, gate, stats, log) = (self.broker.clone(), self.gate.clone(), self.stats.clone(), self.log.clone());
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { break };
                let (broker, gate, stats, log) = (broker.clone(), gate.clone(), stats.clone(), log.clone());
                tokio::spawn(async move {
                    let ctx = Context { gate: &gate, log: &log, stats: &stats };
                    let _ = broker.serve(&ctx, tcp, name).await;
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
            .with_domains(&["realms.minecraft.net", "realms.minecraft-services.net"])
            .with_dial_override(self.serve_as(name).await)
    }

    fn log(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    fn last_heard(&self) -> Announcement {
        self.heard.lock().unwrap().last().cloned().expect("an announcement")
    }
}

/// The headers the Realms client sends (26.3), with the placeholder in its session cookie, and
/// some that must not reach Realms.
fn realms_headers(cookie: &str) -> Vec<(hyper::header::HeaderName, hyper::header::HeaderValue)> {
    vec![
        sensitive_header("cookie", cookie.as_bytes()).unwrap(),
        header("client-ref", "26.3").unwrap(),
        header("x-networkprotocolversion", "777").unwrap(),
        header("is-prerelease", "false").unwrap(),
        header("x-smuggled", "header").unwrap(),
        header("authorization", "Bearer something-else").unwrap(),
        header("user-agent", "Java/25").unwrap(),
    ]
}

fn game_cookie() -> String {
    format!("sid=token:{PLACEHOLDER}:{PROFILE};user=Steve;version=26.3;tracking=1")
}

/// One request on its own connection; status, Retry-After and body.
async fn call(
    client: &HttpClient,
    host: &str,
    method: Method,
    path: &str,
    cookie: &str,
) -> (u16, Option<String>, String) {
    let mut connection = client.connect(host).await.unwrap();
    let mut body = Vec::new();
    let payload = (method != Method::GET).then(|| Bytes::from_static(b"[]"));
    let head = connection.call(method, path, &realms_headers(cookie), payload, 1 << 20, &mut body).await.unwrap();
    let retry = head.headers.get("retry-after").map(|v| v.to_str().unwrap().to_string());
    (head.status, retry, String::from_utf8_lossy(&body).into_owned())
}

// ---------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread")]
async fn realms_requests_carry_the_real_token_in_a_rebuilt_cookie_and_nothing_else() {
    let h = Harness::new("203.0.113.9:31234".into(), true, None).await;
    let game = h.game(PC).await;
    let (status, _, body) = call(&game, PC, Method::GET, "/worlds", &game_cookie()).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Our Realm"));
    let seen = h.upstream.requests().pop().unwrap();
    assert_eq!((seen.method.as_str(), seen.target.as_str()), ("GET", "/worlds"));
    assert_eq!(
        seen.header("cookie"),
        Some(format!("sid=token:{REAL_TOKEN}:{PROFILE};user=Steve;version=26.3").as_str()),
        "the cookie is rebuilt around the real token; the stray cookie is gone"
    );
    assert_eq!(seen.header("client-ref"), Some("26.3"));
    assert_eq!(seen.header("x-networkprotocolversion"), Some("777"));
    assert_eq!(seen.header("is-prerelease"), Some("false"));
    assert_eq!(seen.header("x-smuggled"), None, "unknown headers are not relayed");
    assert_eq!(seen.header("authorization"), None, "nor the game's authorization");
    assert!(!format!("{seen:?}").contains(PLACEHOLDER), "the placeholder never leaves the machine");
    assert!(
        h.log().iter().any(|l| l.contains("GET /worlds → 200 [credential attached: read the Realms list]")),
        "{:?}",
        h.log()
    );
    assert!(!h.log().iter().any(|l| l.contains(REAL_TOKEN)), "the token is never logged");

    // Somebody else's token, or another profile, gets nothing.
    let (status, _, _) = call(&game, PC, Method::GET, "/worlds", &format!("sid=token:another:{PROFILE}")).await;
    assert_eq!(status, 401);
    let (status, _, body) =
        call(&game, PC, Method::GET, "/worlds", &format!("sid=token:{PLACEHOLDER}:00000000000000000000000000000000"))
            .await;
    assert_eq!(status, 403, "{body}");
    assert!(h.log().iter().any(|l| l.contains("[SECURITY]") && l.contains("different profile")));
    assert_eq!(h.upstream.requests().len(), 1, "neither left the machine");

    // The 26.3 frontend: x-mojangauth gets the real token too, for the one request on its table.
    let frontend = h.game(FRONTEND).await;
    let mut connection = frontend.connect(FRONTEND).await.unwrap();
    let mut headers = realms_headers(&game_cookie());
    headers.push(sensitive_header("x-mojangauth", PLACEHOLDER.as_bytes()).unwrap());
    let mut body = Vec::new();
    let head = connection
        .call(Method::POST, "/api/v1.0/invite/codes/use/AbC-123", &headers, Some(Bytes::new()), 1 << 20, &mut body)
        .await
        .unwrap();
    assert_eq!(head.status, 200);
    let seen = h.upstream.requests().pop().unwrap();
    assert_eq!(seen.header("x-mojangauth"), Some(REAL_TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn what_an_owner_does_to_a_realm_never_leaves_the_machine() {
    let h = Harness::new("203.0.113.9:31234".into(), true, None).await;
    let game = h.game(PC).await;
    for (method, path) in [
        (Method::DELETE, "/worlds/7"),
        (Method::POST, "/worlds/7/reset"),
        (Method::PUT, "/worlds/7/close"),
        (Method::POST, "/invites/7"),
        (Method::POST, "/ops/7/069a79f444e94726a5befca90e38aaf5"),
        (Method::PUT, "/worlds/7/backups/upload"),
    ] {
        let (status, _, body) = call(&game, PC, method.clone(), path, &game_cookie()).await;
        assert_eq!(status, 403, "{method} {path}: {body}");
        assert!(body.contains("\"errorMsg\":\"bastion-mc: "), "the game shows why: {body}");
    }
    assert!(h.upstream.requests().is_empty(), "nothing reached Realms");
    // Without an account in the session there is nothing to attach.
    let demo = Harness::new("203.0.113.9:31234".into(), false, None).await;
    let game = demo.game(PC).await;
    let (status, _, _) = call(&game, PC, Method::GET, "/worlds", &game_cookie()).await;
    assert_eq!(status, 401);
    assert!(demo.upstream.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_waits_for_the_code_and_then_the_dialer_reaches_exactly_the_approved_server_once() {
    let server = echo_server();
    let dialer =
        Arc::new(Dialer::start_allowing_loopback(Path::new(LAUNCHER), quiet_log()).expect("the dialer starts"));
    let h = Harness::new(server.to_string(), true, Some(dialer.clone())).await;
    let game = h.game(PC).await;
    // The list names the Realm; the prompt will show its name.
    assert_eq!(call(&game, PC, Method::GET, "/worlds", &game_cookie()).await.0, 200);

    // The join: Realms is asked once; the game is told to wait; the prompt is shown with a code.
    let (status, retry, body) = call(&game, PC, Method::GET, "/worlds/v1/7/join/pc", &game_cookie()).await;
    assert_eq!((status, retry.as_deref()), (503, Some("3")), "{body}");
    assert!(!body.contains(&server.to_string()), "the server's address is not given to the game");
    let shown = h.last_heard();
    assert_eq!((shown.world.as_str(), shown.address.clone()), ("Our Realm", server.to_string()));
    let code = shown.code.clone().expect("a code is shown outside the game");
    let joins = || h.upstream.requests().iter().filter(|s| s.target == "/worlds/v1/7/join/pc").count();
    assert_eq!(joins(), 1);
    let (status, retry, _) = call(&game, PC, Method::GET, "/worlds/v1/7/join/pc", &game_cookie()).await;
    assert_eq!((status, retry.as_deref()), (503, Some("3")), "still waiting");
    assert_eq!(joins(), 1, "the retries are answered here");
    // Another Realm meanwhile: refused, one prompt at a time.
    let (status, _, body) = call(&game, PC, Method::GET, "/worlds/v1/8/join/pc", &game_cookie()).await;
    assert_eq!(status, 403, "{body}");
    // No connection before the approval.
    assert!(h.approvals.take_pass().is_err());

    // The phone sends the code.
    assert_eq!(h.approvals.answer(&code), bastion_mc::realms::approval::Answer::Approved);
    assert_eq!(h.last_heard().outcome, Some(Outcome::Approved));
    let (status, _, body) = call(&game, PC, Method::GET, "/worlds/v1/7/join/pc", &game_cookie()).await;
    assert_eq!(status, 200, "{body}");
    let answer: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(answer["address"], PASS_ADDRESS.to_string(), "the game is given the pass, not the server");
    assert_eq!(answer["sessionRegionData"]["regionName"], "EU", "the rest of the answer is Realms'");
    assert_eq!(joins(), 1);

    // The pass: one connection, made by the dialer, to exactly the approved server.
    let (target, world) = h.approvals.take_pass().expect("armed");
    assert_eq!((target.to_string(), world), (server.to_string(), 7));
    let mut stream = dialer.dial(server).await.expect("the dialer connects");
    use tokio::io::AsyncWriteExt;
    stream.write_all(b"hello realm").await.unwrap();
    let mut echoed = [0u8; 11];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"hello realm");
    assert!(h.approvals.take_pass().is_err(), "one connection per approval");
    // The dialer itself refuses what is not public (this one allows loopback for the stand-in).
    let refused = dialer.dial("10.1.2.3:25565".parse().unwrap()).await.unwrap_err();
    assert!(refused.contains("not a public address"), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_codes_and_time_running_out_refuse_the_join() {
    let h = Harness::new("203.0.113.9:31234".into(), true, None).await;
    let game = h.game(PC).await;
    assert_eq!(call(&game, PC, Method::GET, "/worlds/v1/7/join/pc", &game_cookie()).await.0, 503);
    let code = h.last_heard().code.unwrap();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..3 {
        h.approvals.answer(wrong);
    }
    assert_eq!(h.last_heard().outcome, Some(Outcome::Refused));
    let (status, _, body) = call(&game, PC, Method::GET, "/worlds/v1/7/join/pc", &game_cookie()).await;
    assert_eq!(status, 403, "{body}");
    assert!(h.approvals.take_pass().is_err());
    // After a moment the game may ask again: a new prompt with a new code, which runs out.
    h.approvals.advance_clock(Duration::from_secs(10));
    assert_eq!(call(&game, PC, Method::GET, "/worlds/v1/7/join/pc", &game_cookie()).await.0, 503);
    let second = h.last_heard().code.unwrap();
    h.approvals.advance_clock(Duration::from_secs(121));
    let (status, _, _) = call(&game, PC, Method::GET, "/worlds/v1/7/join/pc", &game_cookie()).await;
    assert_eq!(status, 403);
    assert_eq!(h.heard.lock().unwrap().iter().filter(|a| a.outcome == Some(Outcome::Expired)).count(), 1);
    assert_eq!(h.approvals.answer(&second), bastion_mc::realms::approval::Answer::NoPrompt, "too late");
    // Realms' own "starting, retry" passes through to the game unchanged (no prompt yet).
    h.approvals.advance_clock(Duration::from_secs(10));
    let (status, _, body) = call(&game, PC, Method::GET, "/worlds/v1/8/join/pc", &game_cookie()).await;
    assert_eq!((status, body.contains("starting")), (503, true));
}

// ---------------------------------------------------------------- the pass in the real relay

const PRELUDE: &str = r#"
import errno, socket, sys
def show(k, v): print(f"{k}={v}", flush=True)
def talk(addr, port):
    try:
        s = socket.create_connection((addr, port), timeout=5)
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e))
    try:
        s.sendall(b"ping")
        data = s.recv(16)
        return data.decode() if data else "closed"
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e))
    finally:
        s.close()
"#;

/// The game's side in its private network namespace, through the real relay: before the
/// approval the pass leads nowhere; after it, exactly one connection reaches the approved
/// server; a second one does not.
#[tokio::test(flavor = "multi_thread")]
async fn the_relay_lets_exactly_one_connection_through_the_pass_to_the_approved_server() {
    let server = echo_server();
    let dialer =
        Arc::new(Dialer::start_allowing_loopback(Path::new(LAUNCHER), quiet_log()).expect("the dialer starts"));
    let h = Harness::new(server.to_string(), true, Some(dialer)).await;
    // The approval, as the broker and the phone would give it.
    assert_eq!(h.approvals.join_requested(7), bastion_mc::realms::approval::JoinStep::Forward);
    let target = bastion_mc::realms::table::Address { host: "127.0.0.1".into(), port: server.port() };
    h.approvals.prompt(7, target, b"{}".to_vec());
    assert_eq!(h.approvals.answer(&h.last_heard().code.unwrap()), bastion_mc::realms::approval::Answer::Approved);

    let policy =
        egress::Policy::new(&NetworkConfig::default(), &InstanceNetwork::default(), None).unwrap().with_realms();
    assert!(policy.tcp_ports().contains(&PASS_ADDRESS.port()), "the pass has a listener");
    let ns = policy.netns_spec();
    let python = std::fs::canonicalize("/usr/bin/python3").unwrap();
    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: std::slice::from_ref(&python),
        resolv: Resolv::Loopback,
        expose_timezone: false,
        trust: None,
    })
    .unwrap();
    base.landlock.tcp = TcpPolicy::Ports(policy.tcp_ports());
    let (ip, port) = (PASS_ADDRESS.ip().to_string(), PASS_ADDRESS.port());
    let body = format!(
        r#"
show("before", talk("{ip}", {port}))
import time; time.sleep(0.5)
show("first", talk("{ip}", {port}))
time.sleep(0.5)
show("second", talk("{ip}", {port}))
show("server_direct", talk("127.0.0.1", {server_port}))
"#,
        server_port = server.port()
    );
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::Filtered(ns.clone()),
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: python,
            argv: vec!["python3".into(), "-I".into(), "-c".into(), format!("{PRELUDE}\n{body}")],
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    running.channel.shutdown(Shutdown::Write).unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let broker = Arc::into_inner(h.broker).expect("the only owner");
    // Before the pass is armed the first connection is refused: arm it only once the client
    // has tried. (The approval above armed nothing yet: the pass is armed when the game is
    // given the answer.)
    let approvals = h.approvals.clone();
    let relay = egress::start(
        policy,
        &ns,
        std::mem::take(&mut running.egress),
        HttpClient::new(tls::client_config(tls::load_system_roots().unwrap(), PinSet::default()).unwrap(), &[]),
        Arc::new(move |l| sink.lock().unwrap().push(l)),
        egress::SrvTargets::new(),
        Some(broker),
    )
    .unwrap();
    let mut out = running.child.stdout.take().unwrap();
    let mut first_line = Vec::new();
    // Wait for the client's first attempt, then deliver the answer (which arms the pass).
    let mut byte = [0u8; 1];
    while !first_line.ends_with(b"\n") {
        if out.read(&mut byte).await.unwrap() == 0 {
            break;
        }
        first_line.push(byte[0]);
    }
    assert!(matches!(approvals.join_requested(7), bastion_mc::realms::approval::JoinStep::Deliver(_)));
    let mut rest = String::new();
    let mut err = String::new();
    let mut stderr = running.child.stderr.take().unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut rest), stderr.read_to_string(&mut err));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("the client finished");
    tokio::time::sleep(Duration::from_millis(200)).await;
    relay.shutdown();
    let stdout = format!("{}{rest}", String::from_utf8_lossy(&first_line));
    let log = lines.lock().unwrap().join("\n");
    let get = |key: &str| {
        stdout
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("no {key}=\n{stdout}\n--- stderr ---\n{err}\n--- log ---\n{log}"))
            .to_string()
    };
    assert_ne!(get("before"), "ping", "before the answer, the pass leads nowhere");
    assert_eq!(get("first"), "ping", "the one approved connection reaches the server\n{log}");
    assert_ne!(get("second"), "ping", "a second connection does not");
    assert_ne!(get("server_direct"), "ping", "the server itself is not reachable from inside");
    assert!(log.contains("approved Realms join (Realm 7)"), "{log}");
    assert!(log.contains("no Realms join was approved"), "{log}");
    assert!(log.contains("already used"), "{log}");
}
