//! The Bedrock network (`bedrock::net`, the relay's Bedrock profile), end to end: a sandboxed
//! client in a private network namespace — Python for RakNet over UDP, and the system's `curl`
//! for HTTPS, which reads the sandbox's generated trust store exactly as the game's libcurl does
//! — the real namespace helper, resolver, relay and service broker on the host side, a stand-in
//! RakNet server (unconnected ping/pong, open connection request 1 and 2) and a stand-in for the
//! Realms API and the gatherings service (real TLS, a throw-away certificate hierarchy, scripted
//! answers), and the approval given through `bedrock::net::answer_join`, as a trusted input
//! would give it.
//!
//! Not used: the real game, a real Bedrock server, Realms or the gatherings service, and a
//! Microsoft account — none of them exists on this machine (docs/notes/bedrock-net.md).

use std::{
    convert::Infallible,
    net::{Ipv4Addr, Shutdown, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

use bastion_mc::{
    bedrock::{
        BedrockConfig, BedrockServer,
        net::{self as bnet, Options, Prepared},
    },
    confine::{self, Confinement, Demand, Support},
    egress::raknet::{self, Offline},
    net::{
        http::HttpClient,
        tls::{self, PinSet},
    },
    realms::{Approve, approval::Announcement},
    sandbox::{
        self, BaseOptions, Network, Resolv, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
        trust::TrustStore,
    },
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use tokio::{io::AsyncReadExt, net::TcpListener};
use tokio_rustls::TlsAcceptor;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");
const REALMS: &str = "bedrock.frontendlegacy.realms.minecraft-services.net";
const GATHERINGS: &str = "gatherings-secondary.franchise.minecraft-services.net";
const EXPERIENCE: &str = "6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44";
/// What the game would send as its own Realms token: it must reach the service unchanged and
/// never appear in the launcher's log.
const GAME_TOKEN: &str = "XBL3.0 x=4711;SECRET-GAME-TOKEN-eyJhbGciOi";
const CONFINED_CHILD: &str = "BASTION_TEST_BEDROCK_CONFINED_CHILD";
/// The stand-in server's description, as a Bedrock server's pong carries one.
const MOTD: &[u8] = b"MCPE;bastion stand-in;800;1.26.62;0;10;77;Stand-in;Survival;1;19132;19133;";

const PRELUDE: &str = r#"
import errno, json, os, socket, struct, subprocess, sys, time
MAGIC = bytes([0x00,0xff,0xff,0x00,0xfe,0xfe,0xfe,0xfe,0xfd,0xfd,0xfd,0xfd,0x12,0x34,0x56,0x78])
def show(k, v): print(f"{k}={v}", flush=True)
def fake_ip(name):
    # Ask the launcher's responder itself (libc would answer "localhost" from /etc/hosts).
    q = b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
    q += b"".join(bytes([len(l)]) + l.encode() for l in name.split(".")) + b"\x00\x00\x01\x00\x01"
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(5)
    s.sendto(q, ("127.0.0.1", 53)); r = s.recv(512)
    if r[3] & 0x0f == 3: return "NXDOMAIN"
    return socket.inet_ntoa(r[-4:])
def ping(): return b"\x01" + struct.pack(">q", 1234) + MAGIC + struct.pack(">q", 42)
def ocr1(mtu):
    b = b"\x05" + MAGIC + bytes([11])
    return b + bytes(mtu - 28 - len(b))
def ocr2(cookie, ip, port, mtu):
    addr = bytes([4]) + bytes(255 - int(o) for o in ip.split(".")) + struct.pack(">H", port)
    return b"\x07" + MAGIC + struct.pack(">I", cookie) + b"\x00" + addr + struct.pack(">H", mtu) + struct.pack(">q", -5)
def udp(): return socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
def exchange(sock, dest, data, wait=1.0, tries=1):
    # One datagram out (again, if nothing came back); the answer and where it came from.
    for _ in range(tries):
        sock.settimeout(wait)
        try:
            sock.sendto(data, dest)
            reply, src = sock.recvfrom(4096)
            return reply, f"{src[0]}:{src[1]}"
        except socket.timeout:
            continue
        except OSError as e:
            return errno.errorcode.get(e.errno, str(e.errno)), None
    return None, None
def kind(reply):
    if isinstance(reply, bytes): return f"{reply[0]:#04x}/{len(reply)}"
    return str(reply)
def curl(url, method="GET", data=None):
    args = ["/usr/bin/curl", "-sS", "--max-time", "15", "-o", "/tmp/body", "-w", "%{http_code}",
            "-H", "Authorization: " + TOKEN, "-H", "Client-Version: 1.26.62", "-X", method, url]
    if data is not None: args += ["-H", "Content-Type: application/json", "--data-binary", data]
    if os.path.exists("/tmp/body"): os.remove("/tmp/body")
    r = subprocess.run(args, capture_output=True, text=True)
    body = open("/tmp/body").read() if os.path.exists("/tmp/body") else ""
    if r.returncode != 0: show("curl_error", r.stderr.strip().replace("\n", " "))
    return r.stdout.strip(), body
def join_waiting(url, method="GET", data=None, limit=30):
    # Ask for a join until it is no longer waiting (503): as the game polls a starting Realm.
    waits, deadline = 0, time.time() + limit
    while True:
        status, body = curl(url, method, data)
        if status != "503" or time.time() > deadline: return status, body, waits
        waits += 1; time.sleep(0.5)
"#;

// ---------------------------------------------------------------- stand-ins

/// A RakNet server on this machine's loopback: answers pings and the open-connection
/// handshake as go-raknet's listener does, acknowledges anything else with its first bytes,
/// and remembers what came from where.
struct RakNetServer {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<(SocketAddr, usize, String)>>>,
}

impl RakNetServer {
    fn saw(&self, what: &str) -> Vec<(SocketAddr, usize)> {
        self.seen.lock().unwrap().iter().filter(|(_, _, k)| k.contains(what)).map(|(a, n, _)| (*a, *n)).collect()
    }
}

fn raknet_server() -> RakNetServer {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<(SocketAddr, usize, String)>>> = Arc::default();
    let record = seen.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        while let Ok((len, from)) = socket.recv_from(&mut buf) {
            // It answers this machine and nobody else.
            if !from.ip().is_loopback() {
                continue;
            }
            let data = &buf[..len];
            let parsed = raknet::parse(data);
            let what = match &parsed {
                Ok(message) => message.describe(),
                Err(_) => "connected datagram".to_string(),
            };
            record.lock().unwrap().push((from, len, what));
            let reply = match parsed {
                Ok(Offline::UnconnectedPing { time, .. }) => {
                    Offline::UnconnectedPong { time, server_guid: 77, data: MOTD.to_vec() }.encode()
                }
                Ok(Offline::OpenConnectionRequest1 { protocol, mtu }) if protocol == raknet::PROTOCOL => {
                    Offline::OpenConnectionReply1 { server_guid: 77, cookie: Some(0x1234_5678), mtu: mtu.min(1400) }
                        .encode()
                }
                Ok(Offline::OpenConnectionRequest1 { .. }) => {
                    Offline::IncompatibleProtocol { protocol: raknet::PROTOCOL, server_guid: 77 }.encode()
                }
                Ok(Offline::OpenConnectionRequest2 { mtu, cookie: Some(0x1234_5678), .. }) => {
                    Offline::OpenConnectionReply2 { server_guid: 77, client: from, mtu, security: false }.encode()
                }
                Ok(_) => continue,
                // Anything else is acknowledged with its first bytes (short, whatever it was).
                Err(_) => [b"echo:".as_slice(), &data[..data.len().min(32)]].concat(),
            };
            let _ = socket.send_to(&reply, from);
        }
    });
    RakNetServer { addr, seen }
}

/// What the stand-in services were asked.
#[derive(Debug, Clone)]
struct Seen {
    host: String,
    method: String,
    path: String,
    authorization: Option<String>,
    client_version: Option<String>,
}

/// The Realms API and the gatherings service as the stand-in plays them. `raknet` is where
/// Realm 7 and the event server are.
struct Upstream {
    addr: SocketAddr,
    root: CertificateDer<'static>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

async fn upstream(raknet: SocketAddr) -> Upstream {
    let mut root_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.distinguished_name.push(DnType::CommonName, "Stand-in Bedrock services root");
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
    let root_key = KeyPair::generate().unwrap();
    let root = root_params.self_signed(&root_key).unwrap().der().clone();
    let names: Vec<String> = bnet::REALMS_HOSTS.iter().chain(bnet::GATHERINGS_HOSTS).map(|h| h.to_string()).collect();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(names)
        .unwrap()
        .signed_by(&leaf_key, &Issuer::from_params(&root_params, &root_key))
        .unwrap()
        .der()
        .clone();
    let config = ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())))
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let record = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { break };
            let (acceptor, record) = (acceptor.clone(), record.clone());
            tokio::spawn(async move {
                let Ok(stream) = acceptor.accept(tcp).await else { return };
                let sni = stream.get_ref().1.server_name().unwrap_or_default().to_string();
                let service = service_fn(move |request: Request<Incoming>| {
                    let (record, sni) = (record.clone(), sni.clone());
                    async move {
                        let header =
                            |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
                        record.lock().unwrap().push(Seen {
                            host: sni.clone(),
                            method: request.method().to_string(),
                            path: request.uri().path().to_string(),
                            authorization: header("authorization"),
                            client_version: header("client-version"),
                        });
                        let path = request.uri().path().to_string();
                        let _ = request.into_body().collect().await;
                        let (status, body) = answer(&sni, &path, raknet);
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(body)))
                                .unwrap(),
                        )
                    }
                });
                let _ =
                    hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
            });
        }
    });
    Upstream { addr, root, seen }
}

/// The scripted answers.
fn answer(host: &str, path: &str, raknet: SocketAddr) -> (u16, String) {
    let join = |address: &str, protocol: &str| {
        format!(
            r#"{{"address":"{address}","networkProtocol":"{protocol}","pendingUpdate":false,"sessionRegionData":{{"regionName":"Stand-in","serviceQuality":1}}}}"#
        )
    };
    if host == GATHERINGS {
        return match path {
            "/api/v2.0/join/experience" => (
                200,
                format!(
                    r#"{{"result":{{"networkProtocol":"Default","ipV4Address":"{}","port":{},"destinationInfo":{{"experienceId":"{EXPERIENCE}","experienceName":"Stand-in Live Event","worldName":"Lobby"}}}}}}"#,
                    raknet.ip(),
                    raknet.port()
                ),
            ),
            _ => (404, r#"{"code":"NotFound","message":"stand-in"}"#.into()),
        };
    }
    // Like the services' own routing (ASP.NET), paths are matched without regard to case.
    match path.to_ascii_lowercase().as_str() {
        "/worlds" => (
            200,
            r#"{"servers":[{"id":7,"name":"Castle","state":"OPEN"},{"id":8,"name":"Web","state":"OPEN"}]}"#.into(),
        ),
        "/worlds/7/join" => (200, join(&raknet.to_string(), "DEFAULT")),
        "/worlds/8/join" => (200, join("4242424242", "NETHERNET")),
        "/worlds/9/join" => (200, join("10.0.0.5:19132", "DEFAULT")),
        "/worlds/10/join" => (200, join("realm.example.org:19132", "DEFAULT")),
        // Not a join, and shaped like one.
        "/worlds/11" => (200, join("203.0.113.11:19132", "DEFAULT")),
        "/mco/client/compatible" => (200, "COMPATIBLE".into()),
        _ => (404, r#"{"errorCode":404,"errorMsg":"stand-in"}"#.into()),
    }
}

// ---------------------------------------------------------------- the session

struct Session {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    log: Vec<String>,
    heard: Vec<Announcement>,
    answers: Vec<String>,
    /// Flows, datagrams relayed, datagrams refused.
    udp: (u64, u64, u64),
}

impl Session {
    fn get(&self, key: &str) -> &str {
        self.stdout.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('='))).unwrap_or_else(|| {
            panic!("no `{key}=`\n{}", self.dump());
        })
    }
    fn logged(&self, needle: &str) -> bool {
        self.log.iter().any(|l| l.contains(needle))
    }
    fn dump(&self) -> String {
        format!(
            "--- stdout ---\n{}\n--- stderr ---\n{}\n--- relay log ---\n{}\n--- heard ---\n{:?}\n--- answers ---\n{:?}",
            self.stdout,
            self.stderr,
            self.log.join("\n"),
            self.heard,
            self.answers
        )
    }
}

fn servers(list: &[(&str, u16)]) -> Vec<BedrockServer> {
    list.iter().map(|(host, port)| BedrockServer { host: host.to_string(), port: *port }).collect()
}

fn system_program(name: &str) -> PathBuf {
    std::fs::canonicalize(Path::new("/usr/bin").join(name)).unwrap_or_else(|e| panic!("/usr/bin/{name}: {e}"))
}

/// Prepare the network as a launch does, start a sandbox the way a Bedrock launch would (the
/// namespace and Landlock from [`Prepared`], the trust store with its authority), and return
/// the pieces for the relay.
fn sandboxed(prepared: &Prepared, body: &str) -> sandbox::Running {
    let (python, curl) = (system_program("python3"), system_program("curl"));
    let trust = TrustStore::build(prepared.trust_anchor()).unwrap();
    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: &[python.clone(), curl],
        resolv: Resolv::Loopback,
        expose_timezone: false,
        trust: Some(&trust),
    })
    .unwrap();
    base.landlock.tcp = TcpPolicy::Ports(prepared.sandbox_tcp_ports());
    base.landlock.udp_send = Some(prepared.sandbox_udp_ports());
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::Filtered(prepared.netns_spec()),
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: python,
            argv: vec![
                "python3".into(),
                "-I".into(),
                "-c".into(),
                format!("{PRELUDE}\nTOKEN = {GAME_TOKEN:?}\n{body}"),
            ],
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    running.channel.shutdown(Shutdown::Write).unwrap();
    running
}

/// The launcher's own HTTPS client, trusting the stand-in's root and dialling it for every
/// service name.
fn http_for(upstream: Option<&Upstream>) -> HttpClient {
    match upstream {
        Some(upstream) => {
            let mut roots = RootCertStore::empty();
            roots.add(upstream.root.clone()).unwrap();
            HttpClient::new(tls::client_config(roots, PinSet::default()).unwrap(), &[])
                .with_dial_override(upstream.addr)
        }
        None => HttpClient::new(tls::client_config(tls::load_system_roots().unwrap(), PinSet::default()).unwrap(), &[]),
    }
}

type Heard = Arc<Mutex<Vec<Announcement>>>;

fn options<'a>(cfg: &'a BedrockConfig, approve: Approve, heard: &Heard) -> Options<'a> {
    let heard = heard.clone();
    Options {
        cfg,
        instance_servers: &[],
        approve,
        proxied: false,
        announce: Arc::new(move |a: &Announcement| heard.lock().unwrap().push(a.clone())),
    }
}

/// Answer each prompt with its code, as a trusted input would (`answer realms <digits>`) — after
/// a second and a half, as a person reading the prompt would, so that the game asks again while
/// the join waits.
fn answerer(
    prepared: &Prepared,
    heard: &Heard,
    answers: &Arc<Mutex<Vec<String>>>,
) -> Option<tokio::task::JoinHandle<()>> {
    let approvals = prepared.approvals()?;
    let (heard, answers) = (heard.clone(), answers.clone());
    Some(tokio::spawn(async move {
        let mut answered = 0;
        // Bounded: 60 s of looking, twenty times a second.
        for _ in 0..1200 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let codes: Vec<String> =
                heard.lock().unwrap().iter().filter(|a| a.outcome.is_none()).filter_map(|a| a.code.clone()).collect();
            if codes.len() > answered {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                let said = bnet::answer_join(&approvals, &codes[answered]);
                answers.lock().unwrap().push(said);
                answered += 1;
            }
        }
    }))
}

/// One session: the client's script, start to end.
async fn session(cfg: BedrockConfig, approve: Approve, upstream: Option<&Upstream>, body: &str) -> Session {
    let heard: Heard = Arc::default();
    let answers: Arc<Mutex<Vec<String>>> = Arc::default();
    let prepared = bnet::prepare(options(&cfg, approve, &heard)).unwrap().allowing_loopback_for_tests();
    let mut running = sandboxed(&prepared, body);
    let answering = answerer(&prepared, &heard, &answers);
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    let spec = prepared.netns_spec();
    let relay = prepared
        .start(
            &spec,
            std::mem::take(&mut running.egress),
            http_for(upstream),
            Arc::new(move |l| sink.lock().unwrap().push(l)),
        )
        .unwrap();
    let mut out = running.child.stdout.take().unwrap();
    let mut err = running.child.stderr.take().unwrap();
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let status = tokio::time::timeout(Duration::from_secs(120), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut stdout), err.read_to_string(&mut stderr));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("the client finished");
    tokio::time::sleep(Duration::from_millis(300)).await; // let the relay log what ended
    if let Some(task) = answering {
        task.abort();
    }
    let stats = relay.stats();
    let udp = (
        stats.udp_flows.load(Ordering::Relaxed),
        stats.udp_relayed.load(Ordering::Relaxed),
        stats.udp_refused.load(Ordering::Relaxed),
    );
    relay.shutdown();
    let log = log.lock().unwrap().clone();
    let heard = heard.lock().unwrap().clone();
    let answers = answers.lock().unwrap().clone();
    Session { code: status.code(), stdout, stderr, log, heard, answers, udp }
}

// ---------------------------------------------------------------- RakNet

#[tokio::test(flavor = "multi_thread")]
async fn a_raknet_client_reaches_the_listed_servers_and_their_answers_come_from_where_it_sent() {
    let named = raknet_server();
    let literal = raknet_server();
    let cfg = BedrockConfig {
        servers: servers(&[("localhost", named.addr.port()), ("127.0.0.1", literal.addr.port())]),
        ..BedrockConfig::default()
    };
    let (p1, p2) = (named.addr.port(), literal.addr.port());
    let body = format!(
        r#"
ip = fake_ip("localhost"); show("fake", ip)
s = udp()
r, src = exchange(s, (ip, {p1}), ping(), wait=0.5, tries=10); show("pong", kind(r)); show("pong_from", src)
r, src = exchange(s, (ip, {p1}), ocr1(1492), tries=3); show("reply1", kind(r)); show("reply1_from", src)
cookie = struct.unpack(">I", r[26:30])[0] if isinstance(r, bytes) and len(r) >= 30 else 0
r, src = exchange(s, (ip, {p1}), ocr2(cookie, ip, {p1}, 1400), tries=3); show("reply2", kind(r))
r, src = exchange(s, (ip, {p1}), b"\x84\x00\x00\x00game bytes", tries=3); show("connected", r)
r, src = exchange(s, (ip, {p1}), b"\x84" + bytes(1471), tries=2); show("largest", kind(r))
r, src = exchange(s, (ip, {p1}), b"\x84" + bytes(1472), tries=1); show("oversize", kind(r))
# A new flow must start a RakNet exchange.
t = udp()
r, src = exchange(t, (ip, {p1}), b"\x84\x00\x00\x00no handshake", tries=2); show("no_handshake", kind(r))
# The literal address answers from itself.
u = udp()
r, src = exchange(u, ("127.0.0.1", {p2}), ping(), tries=5); show("literal", kind(r)); show("literal_from", src)
# Another port of the same server, a name that is no server, a name that never resolves.
v = udp()
r, src = exchange(v, (ip, 19134), ping()); show("other_port", kind(r))
web = fake_ip("login.live.com"); show("web", "fake" if web.startswith("127.") else web)
r, src = exchange(v, (web, {p1}), ping()); show("not_a_server", kind(r))
show("unlisted", fake_ip("unlisted.example.org"))
show("java", fake_ip("api.minecraftservices.com"))
show("nethernet", fake_ip("signal.franchise.minecraft-services.net"))
# No TCP to a RakNet server.
c = socket.socket(); c.settimeout(3)
try: c.connect((ip, {p1})); show("tcp", "connected")
except OSError as e: show("tcp", errno.errorcode.get(e.errno, str(e.errno)))
"#
    );
    let s = session(cfg, Approve::Off, None, &body).await;
    assert_eq!(s.code, Some(0), "{}", s.dump());
    let fake = s.get("fake");
    assert!(fake.starts_with("127.64."), "a synthetic address: {}", s.dump());
    assert_eq!(s.get("pong"), format!("0x1c/{}", 35 + MOTD.len()), "{}", s.dump());
    assert_eq!(s.get("pong_from"), format!("{fake}:{p1}"), "the answer comes from where the game sent");
    assert!(s.get("reply1").starts_with("0x06/"), "{}", s.dump());
    assert_eq!(s.get("reply1_from"), format!("{fake}:{p1}"));
    assert!(s.get("reply2").starts_with("0x08/"), "{}", s.dump());
    assert_eq!(
        s.get("connected"),
        "b'echo:\\x84\\x00\\x00\\x00game bytes'",
        "an open flow carries what the game sends"
    );
    assert!(s.get("largest").starts_with("0x65/"), "1472 bytes pass (echoed with a prefix): {}", s.dump());
    assert_eq!(s.get("oversize"), "None", "1473 bytes do not");
    assert_eq!(s.get("no_handshake"), "None", "{}", s.dump());
    assert!(s.get("literal").starts_with("0x1c/"), "{}", s.dump());
    assert_eq!(s.get("literal_from"), format!("127.0.0.1:{p2}"));
    assert_eq!(s.get("other_port"), "EACCES", "Landlock (ABI 10): no UDP to a port no rule names");
    assert_eq!(s.get("web"), "fake", "an HTTPS name resolves…");
    assert_eq!(s.get("not_a_server"), "None", "…but is no RakNet server");
    for key in ["unlisted", "java", "nethernet"] {
        assert_eq!(s.get(key), "NXDOMAIN", "{key}");
    }
    assert!(matches!(s.get("tcp"), "EACCES" | "ECONNREFUSED"), "{}", s.get("tcp"));

    // What the servers saw: the host side's datagrams, at their sizes.
    let ocr1 = named.saw("open connection request 1");
    assert!(!ocr1.is_empty() && ocr1.iter().all(|(from, len)| from.ip().is_loopback() && *len == 1464), "{ocr1:?}");
    assert_eq!(named.saw("connected datagram").iter().map(|(_, n)| *n).collect::<Vec<_>>(), [14, 1472]);
    assert!(!literal.saw("ping").is_empty());

    // What the log says, once each.
    for line in [
        &format!("Bedrock server localhost:{p1} resolved to 127.0.0.1:{p1}") as &str,
        &format!("ALLOW udp localhost:{p1} → 127.0.0.1:{p1}"),
        "RakNet ping (server list); Bedrock server localhost",
        "(the first datagram of a flow must be a RakNet ping or open connection request)",
        &format!("DENY  udp login.live.com:{p1} (not a Bedrock server you listed"),
        "DENY  name lookup \"unlisted.example.org\" (not allowlisted for Bedrock; if it is a server you want to join, add { host = \"unlisted.example.org\" }",
        "DENY  name lookup \"api.minecraftservices.com\" (a Java Edition service",
        "DENY  name lookup \"signal.franchise.minecraft-services.net\" (NetherNet signalling",
        "datagram larger than 1472 bytes",
    ] {
        assert!(s.logged(line), "missing {line:?}\n{}", s.dump());
    }
    let (flows, relayed, refused) = s.udp;
    assert!(flows >= 2 && relayed >= 10 && refused >= 3, "{:?}\n{}", s.udp, s.dump());
}

// ---------------------------------------------------------------- Realms and event servers

#[tokio::test(flavor = "multi_thread")]
async fn a_realms_join_waits_for_the_approval_and_then_the_pass_reaches_exactly_that_server() {
    let server = raknet_server();
    let up = upstream(server.addr).await;
    let body = format!(
        r#"
R = "https://{REALMS}"
status, body = curl(R + "/worlds"); show("list", status)
status, body, waits = join_waiting(R + "/worlds/7/join"); show("join", status); show("waits", waits)
j = json.loads(body); show("address", j["address"]); show("region", j["sessionRegionData"]["regionName"])
ip, port = j["address"].rsplit(":", 1); port = int(port)
s = udp()
r, src = exchange(s, (ip, port), ocr1(1492), tries=3); show("reply1", kind(r)); show("reply1_from", src)
cookie = struct.unpack(">I", r[26:30])[0] if isinstance(r, bytes) and len(r) >= 30 else 0
r, src = exchange(s, (ip, port), ocr2(cookie, ip, port, 1400), tries=3); show("reply2", kind(r))
r, src = exchange(s, (ip, port), ping(), tries=3); show("same_flow", kind(r))
for i in range(4):
    r, src = exchange(udp(), (ip, port), ping(), wait=1.0, tries=2); show(f"more_{{i}}", kind(r))
# Asking again starts a new approval; nothing is delivered without one.
status, body = curl(R + "/worlds/7/join"); show("again", status)
"#
    );
    let s = session(BedrockConfig::default(), Approve::Code, Some(&up), &body).await;
    assert_eq!(s.code, Some(0), "{}", s.dump());
    assert_eq!(s.get("list"), "200", "{}", s.dump());
    assert_eq!(s.get("join"), "200", "{}", s.dump());
    assert!(s.get("waits").parse::<u32>().unwrap() >= 2, "the game waited for the approval");
    assert_eq!(s.get("address"), "127.127.255.253:19132", "the game is given the pass, not the server");
    assert_eq!(s.get("region"), "Stand-in", "the rest of the answer is the service's");
    assert!(s.get("reply1").starts_with("0x06/"), "{}", s.dump());
    assert_eq!(s.get("reply1_from"), "127.127.255.253:19132", "answers come from the pass");
    assert!(s.get("reply2").starts_with("0x08/"));
    assert!(s.get("same_flow").starts_with("0x1c/"));
    // The first flow and three more of the same join; then no more.
    for i in 0..3 {
        assert!(s.get(&format!("more_{i}")).starts_with("0x1c/"), "flow {i}: {}", s.dump());
    }
    assert_eq!(s.get("more_3"), "None", "{}", s.dump());
    assert_eq!(s.get("again"), "503", "a new join waits for a new approval");

    // The prompt, outside the game: the Realm's name and its real address, with a code.
    let prompt = &s.heard[0];
    assert_eq!(prompt.world, "Castle", "{:?}", s.heard);
    assert_eq!(prompt.address, server.addr.to_string());
    assert!(prompt.code.is_some() && prompt.outcome.is_none());
    assert_eq!(s.answers[0], "approved: the game may now join that server, once");
    // What the server saw came from the host side; the handshake reached it intact.
    assert!(server.saw("open connection request 1").iter().all(|(from, len)| from.ip().is_loopback() && *len == 1464));
    assert!(!server.saw("open connection request 2").is_empty());
    // The service saw the game's own token, unchanged; the log never shows it.
    let seen = up.seen.lock().unwrap().clone();
    let join = seen.iter().find(|r| r.path == "/worlds/7/join").expect("the join was asked of Realms");
    assert_eq!(join.host, REALMS);
    assert_eq!(join.authorization.as_deref(), Some(GAME_TOKEN));
    assert_eq!(join.client_version.as_deref(), Some("1.26.62"));
    assert_eq!(seen.iter().filter(|r| r.path == "/worlds/7/join").count(), 2, "once per approval: {seen:?}");
    let log = s.log.join("\n");
    assert!(!log.contains("SECRET-GAME-TOKEN"), "a token in the log:\n{log}");
    for line in [
        "tls terminated to broker joins",
        "server 127.0.0.1:",
        "held back until you approve outside the game",
        "503 here: waiting for your approval outside the game",
        "200 here: approved — the game is given the join pass",
        "ALLOW udp 127.127.255.253:19132 (join pass) → 127.0.0.1:",
        "the approved join to the Realm \"Castle\"",
        "(another flow of the same join)",
        "join pass:",
    ] {
        assert!(log.contains(line), "missing {line:?}\n{}", s.dump());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn nethernet_private_addresses_host_names_and_unknown_join_answers_are_refused_with_the_reason() {
    let server = raknet_server();
    let up = upstream(server.addr).await;
    let body = format!(
        r#"
R = "https://{REALMS}"
for world in ["8", "9", "10"]:
    status, body = curl(R + "/worlds/" + world + "/join"); show("join_" + world, status)
    show("why_" + world, json.loads(body).get("errorMsg", "") if body.startswith("{{") else body)
status, body = curl(R + "/worlds/11"); show("lookalike", status)
status, body = curl(R + "/WORLDS/7/Join"); show("spelling", status)
"#
    );
    let s = session(BedrockConfig::default(), Approve::Code, Some(&up), &body).await;
    assert_eq!(s.code, Some(0), "{}", s.dump());
    assert_eq!(s.get("join_8"), "403", "{}", s.dump());
    assert!(s.get("why_8").contains("NetherNet (NETHERNET)") && s.get("why_8").contains("does not relay"));
    assert_eq!(s.get("join_9"), "502");
    assert!(s.get("why_9").contains("non-public address (10.0.0.5:19132)"));
    assert_eq!(s.get("join_10"), "502");
    assert!(s.get("why_10").contains("names a host, not an address"));
    assert_eq!(s.get("lookalike"), "502", "an address from an endpoint that is no join is withheld");
    // Another spelling of a join is still a join: it waits for an approval like any other.
    assert_eq!(s.get("spelling"), "503", "{}", s.dump());
    assert!(s.heard.iter().any(|a| a.address == server.addr.to_string()), "{:?}", s.heard);
    assert!(s.logged("[SECURITY]"), "{}", s.dump());
    assert!(!s.heard.iter().any(|a| a.address.contains("10.0.0.5") || a.address.contains("203.0.113.11")));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_event_server_join_from_the_gatherings_service_is_approved_the_same_way() {
    let server = raknet_server();
    let up = upstream(server.addr).await;
    let body = format!(
        r#"
G = "https://{GATHERINGS}"
status, body, waits = join_waiting(G + "/api/v2.0/join/experience", "POST", json.dumps({{"experienceId": "{EXPERIENCE}"}}))
show("join", status); show("waits", waits)
res = json.loads(body)["result"]; show("target", res["ipV4Address"] + ":" + str(res["port"])); show("world", res["destinationInfo"]["worldName"])
r, src = exchange(udp(), (res["ipV4Address"], res["port"]), ping(), tries=3); show("pong", kind(r)); show("pong_from", src)
status, body = curl(G + "/api/v2.0/join/experience", "POST", "not json"); show("garbage", status)
"#
    );
    let s = session(BedrockConfig::default(), Approve::Totp, Some(&up), &body).await;
    assert_eq!(s.code, Some(0), "{}", s.dump());
    assert_eq!(s.get("join"), "200", "{}", s.dump());
    assert!(s.get("waits").parse::<u32>().unwrap() >= 1);
    assert_eq!(s.get("target"), "127.127.255.253:19132");
    assert_eq!(s.get("world"), "Lobby");
    assert!(s.get("pong").starts_with("0x1c/"), "{}", s.dump());
    assert_eq!(s.get("pong_from"), "127.127.255.253:19132");
    assert_eq!(s.get("garbage"), "400", "a join request that names no experience is refused");
    assert_eq!(s.heard[0].world, "Stand-in Live Event", "{:?}", s.heard);
    assert_eq!(s.heard[0].address, server.addr.to_string());
    assert_eq!(s.answers[0], "approved: the game may now join that server, once", "totp acts as the code");
    assert!(s.logged("the approved join to the event server \"Stand-in Live Event\""), "{}", s.dump());
    let seen = up.seen.lock().unwrap().clone();
    assert!(
        seen.iter()
            .any(|r| r.host == GATHERINGS && r.method == "POST" && r.authorization.as_deref() == Some(GAME_TOKEN))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_approvals_nothing_is_terminated_and_no_pass_exists() {
    let body = r#"
show("pass", kind(exchange(udp(), ("127.127.255.253", 19132), ping())[0]))
show("realms", fake_ip("bedrock.frontendlegacy.realms.minecraft-services.net").startswith("127.64."))
"#;
    let s = session(BedrockConfig::default(), Approve::Off, None, body).await;
    assert_eq!(s.code, Some(0), "{}", s.dump());
    assert_eq!(s.get("pass"), "EACCES", "no rule names port 19132 without servers or joins: {}", s.dump());
    assert_eq!(s.get("realms"), "True", "the Realms host is still reachable by name (passed through, not terminated)");
    assert!(s.heard.is_empty());
}

// ---------------------------------------------------------------- after step 9

/// The order of a launch: the sandbox exists, the launcher confines itself (Landlock and the
/// system-call filter on every thread, `launcher.confine = "require"`), and only then does the
/// relay start reading what the game sends. Everything a Bedrock session needs still works:
/// the server rule's name is looked up, RakNet flows open, the broker reaches the service (the
/// stand-in's port stands for 443 in the confinement), and the approved join's flow opens.
#[tokio::test(flavor = "multi_thread")]
async fn confined_child() {
    let Ok(_) = std::env::var(CONFINED_CHILD) else { return };
    let server = raknet_server();
    let up = upstream(server.addr).await;
    let cfg = BedrockConfig { servers: servers(&[("localhost", server.addr.port())]), ..BedrockConfig::default() };
    let heard: Heard = Arc::default();
    let answers: Arc<Mutex<Vec<String>>> = Arc::default();
    let prepared = bnet::prepare(options(&cfg, Approve::Code, &heard)).unwrap().allowing_loopback_for_tests();
    let port = server.addr.port();
    let body = format!(
        r#"
time.sleep(1.0)  # the launcher confines itself meanwhile
ip = fake_ip("localhost")
r, src = exchange(udp(), (ip, {port}), ping(), wait=0.5, tries=10); show("server", kind(r))
status, body, waits = join_waiting("https://{REALMS}/worlds/7/join"); show("join", status)
a = json.loads(body)["address"].rsplit(":", 1)
r, src = exchange(udp(), (a[0], int(a[1])), ping(), tries=3); show("pass", kind(r))
"#
    );
    let mut running = sandboxed(&prepared, &body);
    let mut keep = Confinement::supervisor();
    prepared.confinement_needs(&mut keep);
    keep.tcp_connect.push(up.addr.port()); // the stand-in's port, in place of 443
    let applied = confine::apply(&keep, Demand::Whole).expect("confining the child");
    assert!(applied.is_whole());
    let answering = answerer(&prepared, &heard, &answers);
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    let spec = prepared.netns_spec();
    let relay = prepared
        .start(
            &spec,
            std::mem::take(&mut running.egress),
            http_for(Some(&up)),
            Arc::new(move |l| sink.lock().unwrap().push(l)),
        )
        .unwrap();
    let mut out = running.child.stdout.take().unwrap();
    let mut stdout = String::new();
    tokio::time::timeout(Duration::from_secs(90), out.read_to_string(&mut stdout)).await.unwrap().unwrap();
    let _ = running.child.wait().await;
    if let Some(task) = answering {
        task.abort();
    }
    relay.shutdown();
    // Control: the confinement is in force — a TCP connection to a port it does not list.
    let control = match std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, 9)) {
        Ok(_) => "connected".to_string(),
        Err(e) if e.raw_os_error() == Some(libc::EACCES) => "EACCES".into(),
        Err(e) => e.to_string(),
    };
    // (libtest writes "test confined_child ... " without a newline first.)
    println!("\n{stdout}tcp_elsewhere={control}\nlog={}", log.lock().unwrap().join(" | "));
}

#[test]
fn the_bedrock_relay_works_after_the_launcher_has_confined_itself() {
    if confine::support() != Support::Whole {
        eprintln!("skipped: {:?}", confine::support());
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["confined_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CONFINED_CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let all = format!("{stdout}\n--- stderr ---\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "{all}");
    assert!(stdout.contains("server=0x1c/"), "a listed server, resolved after confinement:\n{all}");
    assert!(stdout.contains("join=200"), "the broker reached the service after confinement:\n{all}");
    assert!(stdout.contains("pass=0x1c/"), "the approved join's flow opened after confinement:\n{all}");
    assert!(stdout.contains("tcp_elsewhere=EACCES"), "the child was confined:\n{all}");
}
