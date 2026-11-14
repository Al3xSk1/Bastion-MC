//! The datagram relay (`egress::udp`), end to end: a sandboxed client in a private network
//! namespace (Python standing in for a voice-chat mod, and once a real JVM), the real
//! namespace helper, DNS responder and relays on the host side, and stand-ins for a game
//! server (TCP) and its voice server (UDP echo) on the host's loopback — reached through
//! rules with `allow_private`, as a LAN server would be.
//!
//! What cannot be tested here is said in docs/notes/voice-chat.md: a real voice session
//! needs a server and a signed-in account.

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, ToSocketAddrs, UdpSocket},
    os::fd::AsFd,
    path::Path,
    process::Command,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

use bastion_mc::{
    BOOTSTRAP_CLASS, BOOTSTRAP_CLASS_NAME,
    config::{InstanceNetwork, JavaMatch, NetworkConfig, ServerEntry},
    confine::{self, Confinement, Demand, Support},
    egress::{
        self,
        udp::{Limits, Rate},
    },
    java,
    mojang::args::{Vars, build_frame},
    net::{http::HttpClient, tls},
    sandbox::{
        self, BaseOptions, Mount, Network, Resolv, SandboxSpec, landlock,
        netns::{self, NetnsSpec, SocketRole},
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
};
use tokio::io::AsyncReadExt;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

const PRELUDE: &str = r#"
import errno, os, socket, sys, time
def attempt(f):
    try:
        f(); return "ok"
    except OSError as e:
        return errno.errorcode.get(e.errno, type(e).__name__ + ":" + str(e.errno))
def show(k, v): print(f"{k}={v}", flush=True)
def fake_ip(name):
    # Ask the launcher's responder itself: libc would answer "localhost" from /etc/hosts.
    q = b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
    q += b"".join(bytes([len(l)]) + l.encode() for l in name.split(".")) + b"\x00\x00\x01\x00\x01"
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(5)
    s.sendto(q, ("127.0.0.1", 53)); r = s.recv(512)
    return socket.inet_ntoa(r[-4:])
def udp(): return socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
def ask(sock, dest, data, wait):
    # One datagram out; the answer and where it came from, or that there was none.
    sock.settimeout(wait)
    try:
        sock.sendto(data, dest)
        reply, src = sock.recvfrom(4096)
        return f"{len(reply)}:{reply[:20].decode(errors='replace')}@{src[0]}:{src[1]}"
    except socket.timeout:
        return "no-answer"
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
def count_answers(sock, wait):
    sock.settimeout(wait); n = 0
    try:
        while True:
            sock.recv(4096); n += 1
    except socket.timeout:
        return n
def game(ip, port):
    # A game connection through the relay: open, and proven to reach the server.
    c = socket.create_connection((ip, port), timeout=5); c.sendall(b"hello")
    assert c.recv(16) == b"hello"
    return c
"#;

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    log: Vec<String>,
    /// Flows opened, datagrams relayed, datagrams refused.
    stats: (u64, u64, u64),
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
    fn count(&self, needle: &str) -> usize {
        self.log.iter().filter(|l| l.contains(needle)).count()
    }
    fn dump(&self) -> String {
        format!(
            "--- stdout ---\n{}\n--- stderr ---\n{}\n--- egress log ---\n{}",
            self.stdout,
            self.stderr,
            self.log.join("\n")
        )
    }
}

fn http() -> HttpClient {
    HttpClient::new(tls::client_config(tls::load_system_roots().unwrap(), tls::PinSet::default()).unwrap(), &[])
}

/// A rule for a stand-in server on the loopback (hence `allow_private`).
fn rule(host: &str, game_port: u16, udp: &[u16]) -> ServerEntry {
    ServerEntry::Full { host: host.into(), port: Some(game_port), allow_private: true, voice: None, udp: udp.to_vec() }
}

/// Run `body` in the sandbox the way a launch runs the game in filtered mode: the namespace
/// with the sockets the policy asks for, Landlock with exactly the policy's TCP and UDP ports.
async fn run(servers: Vec<ServerEntry>, limits: Option<Limits>, body: &str) -> Run {
    let net = NetworkConfig { servers, ..Default::default() };
    let mut policy = egress::Policy::new(&net, &InstanceNetwork::default(), None).unwrap();
    if let Some(limits) = limits {
        policy = policy.with_udp_limits(limits);
    }
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
    base.landlock.udp_send = Some(policy.sandbox_udp_ports());
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
    tokio::time::sleep(Duration::from_millis(300)).await; // let the relay log what ended
    let stats = &relay.stats;
    let stats = (
        stats.udp_flows.load(Ordering::Relaxed),
        stats.udp_relayed.load(Ordering::Relaxed),
        stats.udp_refused.load(Ordering::Relaxed),
    );
    relay.shutdown();
    let log = log.lock().unwrap().clone();
    Run { code: status.code(), stdout: so, stderr: se, log, stats }
}

/// Is `peer` this machine talking to a stand-in server bound to `bound`? The stand-ins
/// answer nobody else: one test binds them to this machine's address on the local network
/// (there is no other way to have a server at a literal address that is no loopback
/// address), and an echo service, however short-lived, is nothing to offer to that network.
/// A connection from this machine to one of its own addresses comes from that address, or
/// from a loopback address.
fn from_this_machine(peer: SocketAddr, bound: std::net::IpAddr) -> bool {
    peer.ip().is_loopback() || peer.ip() == bound
}

/// A "game server" on the host's loopback: echoes, and keeps a connection for as long as
/// the client does.
fn game_server(bind: &str) -> u16 {
    let listener = TcpListener::bind((bind, 0)).unwrap();
    let local = listener.local_addr().unwrap();
    let port = local.port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { break };
            if !c.peer_addr().is_ok_and(|peer| from_this_machine(peer, local.ip())) {
                continue;
            }
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

/// A "voice server" on the host's loopback: answers every datagram with the same bytes,
/// and remembers what it was sent and by whom.
struct Voice {
    port: u16,
    seen: Seen,
}

/// What a voice server was sent, and by whom.
type Seen = Arc<Mutex<Vec<(SocketAddr, Vec<u8>)>>>;

impl Voice {
    fn payloads(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|(_, data)| String::from_utf8_lossy(data).into_owned()).collect()
    }
    fn saw(&self, payload: &str) -> bool {
        self.payloads().iter().any(|p| p == payload)
    }
    fn senders(&self) -> Vec<SocketAddr> {
        self.seen.lock().unwrap().iter().map(|(from, _)| *from).collect()
    }
}

fn voice_server(bind: &str) -> Voice {
    let socket = UdpSocket::bind((bind, 0)).unwrap();
    let local = socket.local_addr().unwrap();
    let port = local.port();
    let seen: Seen = Arc::default();
    let record = seen.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok((len, from)) = socket.recv_from(&mut buf) {
            if !from_this_machine(from, local.ip()) {
                continue;
            }
            let data = &buf[..len];
            record.lock().unwrap().push((from, data.to_vec()));
            match data {
                // More than the relay carries back.
                b"big" => drop(socket.send_to(&[b'B'; 1401], from)),
                // A hundred answers to one question.
                b"many" => (0..100).for_each(|i| drop(socket.send_to(format!("answer-{i}").as_bytes(), from))),
                _ => drop(socket.send_to(data, from)),
            }
        }
    });
    Voice { port, seen }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_datagram_reaches_the_voice_server_and_the_answer_comes_from_where_it_was_sent() {
    let (game, voice) = (game_server("127.0.0.1"), voice_server("127.0.0.1"));
    let (literal_game, literal_voice) = (game_server("127.0.0.1"), voice_server("127.0.0.1"));
    let servers = vec![
        // By name: the sandbox gets a synthetic address for it (`voice = N` is `udp = [N]`).
        ServerEntry::Full {
            host: "localhost".into(),
            port: Some(game),
            allow_private: true,
            voice: Some(voice.port),
            udp: vec![],
        },
        // By literal address.
        rule("127.0.0.1", literal_game, &[literal_voice.port]),
    ];
    let body = format!(
        r#"
ip = fake_ip("localhost")
show("synthetic", ip)
g = game(ip, {game})
u = udp()
show("first", ask(u, (ip, {vp}), b"voice-1", 5))
show("second", ask(u, (ip, {vp}), b"voice-2", 5))
# A connected socket: the kernel itself discards an answer from any other address or port.
c = udp(); c.settimeout(5); c.connect((ip, {vp})); c.send(b"voice-3")
show("connected", c.recv(64).decode())
g2 = game("127.0.0.1", {literal_game})
show("literal", ask(udp(), ("127.0.0.1", {lvp}), b"voice-4", 5))
"#,
        vp = voice.port,
        lvp = literal_voice.port,
    );
    let r = run(servers, None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    let synthetic = r.line("synthetic");
    assert!(synthetic.starts_with("127.") && synthetic != "127.0.0.1", "a synthetic address, got {synthetic}");
    // The game sees its answers come from the address and port it sent to — not from
    // 127.0.0.1, which is where a wildcard socket would answer from by itself.
    assert_eq!(r.line("first"), format!("7:voice-1@{synthetic}:{}", voice.port), "{}", r.dump());
    assert_eq!(r.line("second"), format!("7:voice-2@{synthetic}:{}", voice.port));
    assert_eq!(r.line("connected"), "voice-3", "{}", r.dump());
    assert_eq!(r.line("literal"), format!("7:voice-4@127.0.0.1:{}", literal_voice.port), "{}", r.dump());

    // The voice server got each datagram once, from the launcher: one socket per flow.
    assert_eq!(voice.payloads(), ["voice-1", "voice-2", "voice-3"]);
    let senders = voice.senders();
    assert!(senders.iter().all(|s| s.ip().is_loopback()));
    assert_eq!(senders[0], senders[1], "one flow, one socket");
    assert_ne!(senders[0], senders[2], "another socket of the game is another flow");
    assert_eq!(literal_voice.payloads(), ["voice-4"]);

    // Logged per flow, not per datagram: opened, and closed with its counts.
    let opened = format!("ALLOW udp localhost:{vp} → 127.0.0.1:{vp} (flow", vp = voice.port);
    assert_eq!(r.count(&opened), 2, "{}", r.dump());
    assert!(r.logged(&format!("follows the game connection localhost:{game}")), "{}", r.dump());
    assert!(r.logged(&format!("ALLOW udp 127.0.0.1:{lvp} → 127.0.0.1:{lvp}", lvp = literal_voice.port)));
    let closed = format!("done  udp localhost:{} ↑2 ↓2 datagrams, 0 refused (its game connection closed)", voice.port);
    assert!(r.logged(&closed), "{}", r.dump());
    assert_eq!(r.count("udp"), 6, "three flows, two lines each, and nothing per datagram:\n{}", r.dump());
    assert_eq!(r.stats, (3, 8, 0), "flows, datagrams relayed (four each way), refused");
}

/// An IPv4 address of this machine that is not a loopback address (the one it would use to
/// reach the internet), to stand in for a server on the LAN. Nothing is sent to find it:
/// connecting a datagram socket only asks the routing table.
fn own_address() -> Option<Ipv4Addr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    probe.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match probe.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// A LAN server is allowlisted by its literal address, and that is not a loopback address.
/// Such an address takes a path of its own: the helper makes it local inside the namespace
/// (an alias on `lo`), so that a datagram to it arrives on the wildcard socket — and the
/// answer has to leave from that address, whichever local address the game sent from.
///
/// The stand-in servers are bound to this machine's own address on the local network for
/// the length of this test; they answer only this machine (`from_this_machine`), and nothing
/// is sent to the network: a machine reaches its own addresses over its loopback interface.
#[tokio::test(flavor = "multi_thread")]
async fn a_literal_address_that_is_no_loopback_address_is_answered_from_that_address() {
    let Some(own) = own_address() else {
        eprintln!("skipped: this machine has no IPv4 address other than its loopback addresses");
        return;
    };
    let (game, voice) = (game_server(&own.to_string()), voice_server(&own.to_string()));
    let body = format!(
        r#"
g = game("{own}", {game})
show("answer", ask(udp(), ("{own}", {vp}), b"voice-lan", 5))
c = udp(); c.settimeout(5); c.connect(("{own}", {vp})); c.send(b"voice-lan-2")
show("connected", c.recv(64).decode())
# From a socket bound to another local address. (The two sockets above were given the
# server's own address as their source by the kernel, and an answer to that address leaves
# from it whatever the relay does: only this one shows that the relay names the source.)
b = udp(); b.bind(("127.0.0.1", 0))
show("from_loopback", ask(b, ("{own}", {vp}), b"voice-lan-3", 5))
show("neighbour", ask(udp(), ("203.0.113.9", {vp}), b"t-not-local", 0.4))
"#,
        vp = voice.port,
    );
    let r = run(vec![rule(&own.to_string(), game, &[voice.port])], None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    assert_eq!(r.line("answer"), format!("9:voice-lan@{own}:{}", voice.port), "{}", r.dump());
    assert_eq!(r.line("connected"), "voice-lan-2", "{}", r.dump());
    assert_eq!(r.line("from_loopback"), format!("11:voice-lan-3@{own}:{}", voice.port), "{}", r.dump());
    // Only the allowlisted address was made local: any other one has no route at all.
    assert_eq!(r.line("neighbour"), "ENETUNREACH", "{}", r.dump());
    assert_eq!(voice.payloads(), ["voice-lan", "voice-lan-2", "voice-lan-3"], "{}", r.dump());
    // On the host side the relay reached the server at that address, from this machine.
    assert!(voice.senders().iter().all(|s| s.ip() == std::net::IpAddr::V4(own)), "{:?}", voice.senders());
    assert!(r.logged(&format!("ALLOW udp {own}:{vp} → {own}:{vp}", vp = voice.port)), "{}", r.dump());
    assert_eq!(r.stats, (3, 6, 0), "{}", r.dump());
}

/// The address the game connection uses may be an IPv6 one (names resolve to both on the
/// host side); the sandbox still only ever sees its synthetic IPv4 address.
#[tokio::test(flavor = "multi_thread")]
async fn datagrams_follow_a_game_connection_that_uses_ipv6() {
    if TcpListener::bind("[::1]:0").is_err() {
        eprintln!("skipped: no IPv6 loopback on this machine");
        return;
    }
    if !"localhost:0".to_socket_addrs().is_ok_and(|mut all| all.any(|a| a.is_ipv6())) {
        eprintln!("skipped: `localhost` has no IPv6 address on this machine");
        return;
    }
    let (game, voice) = (game_server("::1"), voice_server("::1"));
    let body = format!(
        r#"
ip = fake_ip("localhost")
g = game(ip, {game})
show("answer", ask(udp(), (ip, {vp}), b"voice-6", 5))
show("sent_to", f"{{ip}}:{vp}")
"#,
        vp = voice.port,
    );
    let r = run(vec![rule("localhost", game, &[voice.port])], None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    assert_eq!(r.line("answer"), format!("7:voice-6@{}", r.line("sent_to")), "{}", r.dump());
    assert!(voice.senders()[0].is_ipv6());
    assert!(r.logged(&format!("ALLOW udp localhost:{vp} → [::1]:{vp}", vp = voice.port)), "{}", r.dump());
}

/// A server that takes game connections over both address families and voice over IPv4
/// only. The resolver on a dual-stack machine puts IPv6 first; the game connection of a
/// rule with UDP ports goes over IPv4 all the same, as the Java client's own would, and
/// voice with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_with_udp_ports_is_reached_over_ipv4_when_it_has_both() {
    let resolved: Vec<SocketAddr> = "localhost:0".to_socket_addrs().map(Iterator::collect).unwrap_or_default();
    if !(resolved.iter().any(SocketAddr::is_ipv6) && resolved.iter().any(SocketAddr::is_ipv4)) {
        eprintln!("skipped: `localhost` does not have both an IPv4 and an IPv6 address on this machine");
        return;
    }
    // One port number on both loopbacks.
    let mut both = None;
    for _ in 0..20 {
        let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = v4.local_addr().unwrap().port();
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            both = Some((port, v4, v6));
            break;
        }
    }
    let Some((game, v4, v6)) = both else {
        eprintln!("skipped: could not get one port on both loopback addresses");
        return;
    };
    let families = Arc::new(Mutex::new(Vec::new()));
    for listener in [v4, v6] {
        let families = families.clone();
        std::thread::spawn(move || {
            for mut conn in listener.incoming().flatten() {
                families.lock().unwrap().push(if conn.local_addr().unwrap().is_ipv4() { "IPv4" } else { "IPv6" });
                std::thread::spawn(move || {
                    let mut buf = [0u8; 64];
                    while let Ok(n) = conn.read(&mut buf) {
                        if n == 0 || conn.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                });
            }
        });
    }
    let voice = voice_server("127.0.0.1");
    let body = format!(
        r#"
ip = fake_ip("localhost")
g = game(ip, {game})
show("answer", ask(udp(), (ip, {vp}), b"voice-7", 5))
"#,
        vp = voice.port,
    );
    let with_voice = run(vec![rule("localhost", game, &[voice.port])], None, &body).await;
    assert_eq!(with_voice.code, Some(0), "{}", with_voice.dump());
    assert!(with_voice.line("answer").starts_with("7:voice-7@127."), "{}", with_voice.dump());
    assert_eq!(*families.lock().unwrap(), ["IPv4"], "{}", with_voice.dump());

    // Without UDP ports in the rule nothing about the game connection changes: it goes
    // where the resolver said first.
    let first = if resolved[0].is_ipv4() { "IPv4" } else { "IPv6" };
    let plain =
        run(vec![rule("localhost", game, &[])], None, &format!("ip = fake_ip('localhost')\ng = game(ip, {game})\n"))
            .await;
    assert_eq!(plain.code, Some(0), "{}", plain.dump());
    assert_eq!(*families.lock().unwrap(), ["IPv4", first], "{}", plain.dump());
}

#[tokio::test(flavor = "multi_thread")]
async fn datagrams_without_a_rule_a_port_or_a_game_connection_are_refused() {
    // A: by name, with a voice port. B: literal, no UDP rule. C: literal, a UDP port of its own.
    let (game_a, voice_a) = (game_server("127.0.0.1"), voice_server("127.0.0.1"));
    let game_b = game_server("127.0.0.1");
    let (game_c, voice_c) = (game_server("127.0.0.2"), voice_server("127.0.0.2"));
    let servers = vec![
        rule("localhost", game_a, &[voice_a.port]),
        rule("127.0.0.1", game_b, &[]),
        rule("127.0.0.2", game_c, &[voice_c.port]),
    ];
    let body = format!(
        r#"
ip = fake_ip("localhost")
u = udp()
show("before_game", ask(u, (ip, {va}), b"t-before-game", 0.4))
ga = game(ip, {game_a})
show("with_game", ask(u, (ip, {va}), b"t-with-game", 5))
show("other_servers_port", ask(udp(), (ip, {vc}), b"t-other-port", 0.4))
gb = game("127.0.0.1", {game_b})
show("no_udp_rule", ask(udp(), ("127.0.0.1", {va}), b"t-no-udp-rule", 0.4))
show("no_connection", ask(udp(), ("127.0.0.2", {vc}), b"t-no-connection", 0.4))
show("neighbour", ask(udp(), ("127.0.0.3", {va}), b"t-neighbour", 0.4))
show("unassigned", ask(udp(), ("127.99.0.1", {va}), b"t-unassigned", 0.4))
show("service_name", ask(udp(), (fake_ip("sessionserver.mojang.com"), {va}), b"t-service", 0.4))
show("off_machine", ask(udp(), ("192.0.2.1", {va}), b"t-off-machine", 0.4))
gc = game("127.0.0.2", {game_c})
show("with_connection", ask(udp(), ("127.0.0.2", {vc}), b"t-with-connection", 5))
# Noise: forty addresses that are no server. Then a refusal of a kind not seen so far.
n = udp()
for i in range(40): n.sendto(b"t-noise", ("127.1.0.%d" % i, {va}))
show("after_noise", ask(udp(), ("127.0.0.2", {vc}), b"X" * 1401, 0.4))
# The game connection closes: nothing follows it any more, on the old socket or a new one.
ga.close(); time.sleep(0.5)
show("after_game", ask(u, (ip, {va}), b"t-after-game", 0.4))
"#,
        va = voice_a.port,
        vc = voice_c.port,
    );
    let r = run(servers, None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    assert!(r.logged("egress: ALLOW localhost:"), "the game connection by name was relayed:\n{}", r.dump());
    for refused in [
        "before_game",
        "other_servers_port",
        "no_udp_rule",
        "no_connection",
        "neighbour",
        "unassigned",
        "service_name",
        "after_noise",
        "after_game",
    ] {
        assert_eq!(r.line(refused), "no-answer", "{refused}\n{}", r.dump());
    }
    // An address that is no allowlisted server and not on the loopback has no route at all.
    assert_eq!(r.line("off_machine"), "ENETUNREACH");
    assert!(r.line("with_game").starts_with("11:t-with-game@127."), "{}", r.dump());
    assert_eq!(r.line("with_connection"), format!("17:t-with-connection@127.0.0.2:{}", voice_c.port), "{}", r.dump());

    // Nothing that was refused reached either voice server.
    assert_eq!(voice_a.payloads(), ["t-with-game"], "{}", r.dump());
    assert_eq!(voice_c.payloads(), ["t-with-connection"], "{}", r.dump());

    // Every kind of refusal is in the log, once, with its reason.
    let (va, vc) = (voice_a.port, voice_c.port);
    for (label, why) in [
        (format!("localhost:{va}"), "no game connection to this server is open"),
        (format!("localhost:{vc}"), "this UDP port is not named by a rule for this server"),
        (format!("127.0.0.1:{va}"), "the rule for this server names no UDP port"),
        (format!("127.0.0.2:{vc}"), "no game connection to this server is open"),
        (format!("127.0.0.3:{va}"), "not an allowlisted game server"),
        (format!("127.99.0.1:{va}"), "address was never handed out by the resolver"),
        (format!("sessionserver.mojang.com:{va}"), "not an allowlisted game server"),
    ] {
        let line = format!("egress: DENY  udp {label} ({why}");
        assert_eq!(r.count(&line), 1, "{line}\n{}", r.dump());
    }
    // The log cannot be flooded, nor one kind of line crowded out by another: of the forty
    // other addresses that were no server, fourteen more are named (sixteen per kind), the
    // last one saying that the rest is only counted — and the oversize datagram that came
    // after all of them still got its line.
    assert_eq!(r.count("(not an allowlisted game server"), 16, "{}", r.dump());
    assert_eq!(r.count("further destinations refused for this reason are counted, not logged"), 1, "{}", r.dump());
    assert!(r.logged(&format!("DENY  udp 127.0.0.2:{vc} (datagram larger than 1400 bytes)")), "{}", r.dump());
    // (Before and after the game connection: the same kind for the same destination, one line.)
    assert_eq!(r.count("DENY  udp"), 7 + 14 + 1, "{}", r.dump());
    assert!(r.logged(&format!("done  udp localhost:{va} ↑1 ↓1 datagrams, 0 refused (its game connection closed)")));
    assert_eq!(r.stats, (2, 4, 8 + 40 + 1), "flows, relayed, refused\n{}", r.dump());
}

#[tokio::test(flavor = "multi_thread")]
async fn size_rate_and_flow_limits_hold_on_real_sockets() {
    let (game, voice) = (game_server("127.0.0.1"), voice_server("127.0.0.1"));
    // The size limit is the one every launch has; rates and the number of flows are small
    // here so that reaching them takes a handful of datagrams.
    let limits = Limits {
        max_flows: 3,
        to_server: Rate { per_second: 5, burst: 20 },
        to_game: Rate { per_second: 5, burst: 10 },
        ..Limits::default()
    };
    let largest = limits.max_datagram;
    let body = format!(
        r#"
ip = fake_ip("localhost")
g = game(ip, {game})
voice = (ip, {vp})
u = udp()
show("largest", ask(u, voice, b"L" * {largest}, 5))
show("too_large", ask(u, voice, b"X" * ({largest} + 1), 0.4))
show("huge", ask(u, voice, b"H" * 60000, 0.4))
show("large_answer", ask(u, voice, b"big", 0.4))
show("still_works", ask(u, voice, b"t-still", 5))
show("empty", ask(u, voice, b"", 5))
# Towards the server: 150 datagrams at once on a new flow.
r = udp()
for i in range(150): r.sendto(b"r%03d" % i, voice)
show("answers_to_burst", count_answers(r, 0.5))
# Towards the game: one datagram, to which the server sends a hundred answers.
d = udp(); d.sendto(b"many", voice)
show("answers_to_one", count_answers(d, 0.5))
# Three flows exist now; there is no fourth.
show("fourth_flow", ask(udp(), voice, b"t-fourth", 0.4))
"#,
        vp = voice.port,
    );
    let r = run(vec![rule("localhost", game, &[voice.port])], Some(limits), &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    assert!(r.line("largest").starts_with(&format!("{largest}:LLLL")), "{}", r.dump());
    for refused in ["too_large", "huge", "large_answer", "fourth_flow"] {
        assert_eq!(r.line(refused), "no-answer", "{refused}\n{}", r.dump());
    }
    assert!(r.line("still_works").starts_with("7:t-still@"), "a refused datagram does not end its flow\n{}", r.dump());
    // The smallest datagram there is — no payload at all — is a datagram like any other.
    assert!(r.line("empty").starts_with("0:@127."), "{}", r.dump());

    let seen = voice.payloads();
    assert!(!seen.iter().any(|p| p.len() > largest), "an oversize datagram reached the server");
    assert!(seen.iter().any(|p| p == "big"), "the question was small; its answer was not");
    // The burst towards the server: the bucket's 20, plus what it earned while they arrived.
    let burst = seen.iter().filter(|p| p.starts_with('r')).count();
    assert!((20..=23).contains(&burst), "{burst} of 150 datagrams reached the server\n{}", r.dump());
    // Of the server's answers to them, and of its hundred answers to one datagram, what the
    // bucket towards the game lets through: 10, plus what it earned meanwhile.
    for key in ["answers_to_burst", "answers_to_one"] {
        let answers: usize = r.line(key).parse().unwrap();
        assert!((10..=13).contains(&answers), "{key}: {answers}\n{}", r.dump());
    }
    assert!(!voice.saw("t-fourth"));

    let label = format!("localhost:{}", voice.port);
    for why in [
        format!("datagram larger than {largest} bytes"),
        "rate limit: more than 5 datagrams a second from the game on one flow".to_string(),
        "rate limit: more than 5 datagrams a second from the server on one flow".to_string(),
        "3 flows are open already".to_string(),
    ] {
        assert_eq!(r.count(&format!("egress: DENY  udp {label} ({why})")), 1, "{why}\n{}", r.dump());
    }
    assert_eq!(r.count("DENY  udp"), 4, "one line per kind, however many datagrams:\n{}", r.dump());
    let (flows, relayed, refused) = r.stats;
    assert_eq!(flows, 3);
    // Every datagram either way is in exactly one of the two counts: six from the game and
    // four from the server on the first flow, 150 on the second and the server's answers to
    // those that got through, 1 and the 100 answers on the third, and the one that found
    // no flow.
    let total = (6 + 4) + (150 + burst as u64) + (1 + 100) + 1;
    assert_eq!(relayed + refused, total, "{}", r.dump());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flow_ends_when_the_game_falls_silent_and_when_its_game_connection_closes() {
    let (game, voice) = (game_server("127.0.0.1"), voice_server("127.0.0.1"));
    // One flow at a time, so that a flow ending is visible as room for another.
    let limits = Limits { max_flows: 1, idle: Duration::from_secs(1), ..Limits::default() };
    let body = format!(
        r#"
ip = fake_ip("localhost")
g = game(ip, {game})
voice = (ip, {vp})
a = udp(); b = udp()
show("first", ask(a, voice, b"t-first", 5))
show("second_while_first_lives", ask(b, voice, b"t-early", 0.3))
time.sleep(1.4)
show("second_after_first_expired", ask(b, voice, b"t-late", 5))
g.close(); time.sleep(0.5)
show("after_the_game_connection", ask(b, voice, b"t-after", 0.3))
"#,
        vp = voice.port,
    );
    let r = run(vec![rule("localhost", game, &[voice.port])], Some(limits), &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    assert!(r.line("first").starts_with("7:t-first@"), "{}", r.dump());
    assert_eq!(r.line("second_while_first_lives"), "no-answer", "{}", r.dump());
    assert!(r.line("second_after_first_expired").starts_with("6:t-late@"), "{}", r.dump());
    assert_eq!(r.line("after_the_game_connection"), "no-answer", "{}", r.dump());
    assert_eq!(voice.payloads(), ["t-first", "t-late"]);

    let label = format!("localhost:{}", voice.port);
    assert!(
        r.logged(&format!("done  udp {label} ↑1 ↓1 datagrams, 0 refused (the game sent nothing for 1 s)")),
        "{}",
        r.dump()
    );
    assert!(
        r.logged(&format!("done  udp {label} ↑1 ↓1 datagrams, 0 refused (its game connection closed)")),
        "{}",
        r.dump()
    );
    assert!(r.logged(&format!("DENY  udp {label} (1 flows are open already)")));
    assert!(r.logged(&format!("DENY  udp {label} (no game connection to this server is open")));
    assert_eq!(r.stats, (2, 4, 2), "{}", r.dump());
}

/// A "game server" that does not hang up when the client does (a frozen server, or a close
/// that is swallowed on the way): it keeps its end of the connection. Told `bye`, it hangs
/// up first instead.
fn stubborn_game_server() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 64];
                loop {
                    match conn.read(&mut buf) {
                        // The client is gone: keep the connection for longer than the test runs.
                        Ok(0) | Err(_) => return std::thread::sleep(Duration::from_secs(120)),
                        Ok(n) if &buf[..n] == b"bye" => return, // dropped: this end closes
                        Ok(n) => conn.write_all(&buf[..n]).unwrap(),
                    }
                }
            });
        }
    });
    port
}

/// "Open" means open in both directions. As soon as either side has hung up, datagrams no
/// longer follow that game connection — however long the other side keeps its end.
/// Otherwise a server that never closes would keep the datagram path open after the player
/// has left, and code in the game could keep it open after the server has hung up, by not
/// closing a socket.
#[tokio::test(flavor = "multi_thread")]
async fn datagrams_stop_as_soon_as_either_side_hangs_up_the_game_connection() {
    let (game, voice) = (stubborn_game_server(), voice_server("127.0.0.1"));
    let body = format!(
        r#"
ip = fake_ip("localhost")
voice = (ip, {vp})
u = udp()
# The game hangs up; the server keeps its end.
g = game(ip, {game})
show("while_open", ask(u, voice, b"t-open", 5))
g.close(); time.sleep(0.5)
show("game_hung_up", ask(u, voice, b"t-game-hung-up", 0.4))
# The server hangs up; the game keeps its end and says nothing.
g = game(ip, {game})
show("open_again", ask(u, voice, b"t-open-again", 5))
g.sendall(b"bye"); time.sleep(0.5)
show("server_hung_up", ask(u, voice, b"t-server-hung-up", 0.4))
show("game_still_holds_its_end", g.fileno() >= 0)
"#,
        vp = voice.port,
    );
    let r = run(vec![rule("localhost", game, &[voice.port])], None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    assert!(r.line("while_open").starts_with("6:t-open@"), "{}", r.dump());
    assert_eq!(r.line("game_hung_up"), "no-answer", "{}", r.dump());
    assert!(r.line("open_again").starts_with("12:t-open-again@"), "{}", r.dump());
    assert_eq!(r.line("server_hung_up"), "no-answer", "{}", r.dump());
    assert_eq!(r.line("game_still_holds_its_end"), "True");
    assert_eq!(voice.payloads(), ["t-open", "t-open-again"], "{}", r.dump());

    let label = format!("localhost:{}", voice.port);
    let closed = format!("done  udp {label} ↑1 ↓1 datagrams, 0 refused (its game connection closed)");
    assert_eq!(r.count(&closed), 2, "{}", r.dump());
    assert_eq!(r.count(&format!("DENY  udp {label} (no game connection to this server is open")), 1, "{}", r.dump());
    assert_eq!(r.stats, (2, 4, 2), "{}", r.dump());
    // The game connection itself is carried as before, to its end: the second one is done
    // when the game lets go of it too (`hello` and `bye` up, the echo of `hello` down).
    assert!(r.logged(&format!("egress: done  localhost:{game} ↑8 ↓5 bytes")), "{}", r.dump());
}

/// The first hang-up ends the datagrams, not the game connection: what the other side
/// still has to say is delivered. Half a megabyte each way through a connection that
/// datagrams follow, the game closing its direction first, as a check that such a
/// connection carries exactly the bytes any other does.
#[tokio::test(flavor = "multi_thread")]
async fn a_game_connection_that_datagrams_follow_carries_its_bytes_to_the_end() {
    let (game, voice) = (game_server("127.0.0.1"), voice_server("127.0.0.1"));
    let body = format!(
        r#"
import threading
ip = fake_ip("localhost")
voice = (ip, {vp})
# (`game` waits for an echo: only then has the relay connected and can datagrams follow.)
g = game(ip, {game}); g.settimeout(30)
got = bytearray()
def reader():
    while True:
        d = g.recv(65536)
        if not d: break
        got.extend(d)
t = threading.Thread(target=reader); t.start()
u = udp()
show("while_open", ask(u, voice, b"t-open", 5))
payload = bytes(range(256)) * 2048
g.sendall(payload); g.shutdown(socket.SHUT_WR)
t.join(30)
show("echoed", f"{{len(got)}}:{{bytes(got) == payload}}")
show("after_the_game_closed_its_direction", ask(u, voice, b"t-half-closed", 0.4))
"#,
        vp = voice.port,
    );
    let r = run(vec![rule("localhost", game, &[voice.port])], None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    assert!(r.line("while_open").starts_with("6:t-open@"), "{}", r.dump());
    assert_eq!(r.line("echoed"), "524288:True", "{}", r.dump());
    assert_eq!(r.line("after_the_game_closed_its_direction"), "no-answer", "{}", r.dump());
    // (Five bytes more each way: the `hello` that opened the connection, and its echo.)
    assert!(r.logged(&format!("egress: done  localhost:{game} ↑524293 ↓524293 bytes")), "{}", r.dump());
    assert_eq!(voice.payloads(), ["t-open"], "{}", r.dump());
}

/// Behind the namespace, Landlock (ABI 10): the sandbox can send datagrams to the DNS port
/// and to the ports that rules name; with no such rule, to the DNS port alone, as before.
#[tokio::test(flavor = "multi_thread")]
async fn landlock_lets_the_sandbox_send_to_named_udp_ports_only() {
    if landlock::kernel_abi() < landlock::UDP_ABI {
        eprintln!("skipped: Landlock ABI {} has no UDP rules (need {})", landlock::kernel_abi(), landlock::UDP_ABI);
        return;
    }
    let (game, voice) = (game_server("127.0.0.1"), voice_server("127.0.0.1"));
    let body = format!(
        r#"
ip = fake_ip("localhost")
v = udp()
show("named_port", attempt(lambda: v.sendto(b"t-landlock", (ip, {vp}))))
show("connect_named", attempt(lambda: udp().connect((ip, {vp}))))
show("other_port", attempt(lambda: v.sendto(b"x", (ip, {vp} + 1))))
show("connect_other", attempt(lambda: udp().connect((ip, {vp} + 1))))
show("game_port_is_tcp_only", attempt(lambda: v.sendto(b"x", (ip, {game}))))
show("v6_other", attempt(lambda: socket.socket(socket.AF_INET6, socket.SOCK_DGRAM).sendto(b"x", ("::1", {vp} + 1))))
show("bind_named", attempt(lambda: udp().bind(("0.0.0.0", {vp}))))
show("bind_chosen", attempt(lambda: udp().bind(("127.0.0.1", 4445))))
show("bind_ephemeral", attempt(lambda: udp().bind(("127.0.0.1", 0))))
"#,
        vp = voice.port,
    );
    let r = run(vec![rule("localhost", game, &[voice.port])], None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    for allowed in ["named_port", "connect_named", "bind_ephemeral"] {
        assert_eq!(r.line(allowed), "ok", "{allowed}\n{}", r.dump());
    }
    for denied in ["other_port", "connect_other", "game_port_is_tcp_only", "v6_other", "bind_named", "bind_chosen"] {
        assert_eq!(r.line(denied), "EACCES", "{denied}\n{}", r.dump());
    }
    // Landlock let the one datagram out of the sandbox; the relay still had no reason to carry it.
    assert!(!voice.saw("t-landlock"));
    assert!(r.logged("no game connection to this server is open"), "{}", r.dump());

    // The same server without `voice`/`udp` in its rule: no socket in the namespace, and
    // the port is as closed as every other.
    let body = format!(
        r#"
ip = fake_ip("localhost")
g = game(ip, {game})
show("without_a_rule", attempt(lambda: udp().sendto(b"t-no-rule", (ip, {vp}))))
show("dns_still_works", ip)
"#,
        vp = voice.port,
    );
    let r = run(vec![rule("localhost", game, &[])], None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    assert_eq!(r.line("without_a_rule"), "EACCES", "{}", r.dump());
    assert!(r.line("dns_still_works").starts_with("127."));
    assert!(!voice.saw("t-no-rule"));
    assert_eq!(r.stats, (0, 0, 0));
    assert!(!r.logged("udp"), "{}", r.dump());
}

/// The helper creates the sockets inside the namespace and passes them out as bare
/// descriptors: position is all that says which is which. `NetnsSpec::sockets` is that
/// order; here it is checked against what the kernel says about each descriptor and
/// against where the sandbox's traffic actually arrives.
#[tokio::test(flavor = "multi_thread")]
async fn the_helper_hands_over_its_sockets_in_the_order_the_specification_gives() {
    let ns = NetnsSpec {
        tcp_ports: vec![443, 25565],
        dns: true,
        local_addrs: vec![Ipv4Addr::new(203, 0, 113, 7)],
        udp_ports: vec![24454, 24455],
    };
    let roles =
        [SocketRole::Dns, SocketRole::Tcp(443), SocketRole::Tcp(25565), SocketRole::Udp(24454), SocketRole::Udp(24455)];
    assert_eq!(ns.sockets(), roles);
    assert_eq!(ns.socket_count(), 5);

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
    base.landlock.tcp = TcpPolicy::Ports(vec![443, 25565]);
    base.landlock.udp_send = Some(vec![53, 24454, 24455]);
    let body = r#"
import socket
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
u.sendto(b"for-dns", ("127.0.0.1", 53))
u.sendto(b"for-udp-24454", ("127.64.0.9", 24454))
u.sendto(b"for-udp-24455", ("203.0.113.7", 24455))
for port in (443, 25565):
    c = socket.create_connection(("127.64.0.9", port), timeout=5); c.sendall(b"for-tcp-%d" % port); c.close()
print("sent", flush=True)
"#;
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::Filtered(ns.clone()),
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: python,
            argv: vec!["python3".into(), "-I".into(), "-c".into(), body.into()],
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    running.channel.shutdown(Shutdown::Write).unwrap();
    let sockets = std::mem::take(&mut running.egress);
    assert_eq!(sockets.len(), 5);

    // What the kernel says about each descriptor (`sandbox::spawn` has checked this too):
    for (socket, role) in sockets.iter().zip(roles) {
        netns::check_socket(socket.as_fd(), role).unwrap_or_else(|e| panic!("{role:?}: {e}"));
    }
    // …and every other reading of the order is refused.
    for (index, wrong) in [
        (0, SocketRole::Udp(24454)),
        (0, SocketRole::Tcp(53)),
        (1, SocketRole::Tcp(25565)),
        (1, SocketRole::Udp(443)),
        (3, SocketRole::Udp(24455)),
        (3, SocketRole::Tcp(24454)),
        (3, SocketRole::Dns),
        (4, SocketRole::Udp(24454)),
    ] {
        assert!(netns::check_socket(sockets[index].as_fd(), wrong).is_err(), "socket {index} passed as {wrong:?}");
    }

    let mut out = String::new();
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        running.child.stdout.take().unwrap().read_to_string(&mut out).await.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("payload timed out");
    assert_eq!((status.code(), out.trim()), (Some(0), "sent"));

    // Where the traffic arrived: each message on the socket that position says is its own.
    let mut sockets = sockets.into_iter();
    let datagram = |socket: std::os::fd::OwnedFd| {
        let socket = UdpSocket::from(socket);
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = [0u8; 64];
        let len = socket.recv(&mut buf).expect("a datagram");
        String::from_utf8_lossy(&buf[..len]).into_owned()
    };
    let stream = |socket: std::os::fd::OwnedFd| {
        let (mut conn, _) = TcpListener::from(socket).accept().expect("a connection");
        let mut text = String::new();
        conn.read_to_string(&mut text).unwrap();
        text
    };
    assert_eq!(datagram(sockets.next().unwrap()), "for-dns");
    assert_eq!(stream(sockets.next().unwrap()), "for-tcp-443");
    assert_eq!(stream(sockets.next().unwrap()), "for-tcp-25565");
    assert_eq!(datagram(sockets.next().unwrap()), "for-udp-24454");
    assert_eq!(datagram(sockets.next().unwrap()), "for-udp-24455");
}

/// The rule is right, the game connection is open, and nothing listens on the voice port
/// (the server runs no voice chat, or the rule names the wrong port). The flow exists, the
/// datagrams leave, the server's address answers with ICMP errors: that is said once.
#[tokio::test(flavor = "multi_thread")]
async fn a_voice_port_on_which_nothing_listens_is_reported_once() {
    let game = game_server("127.0.0.1");
    let closed = {
        let unused = UdpSocket::bind("127.0.0.1:0").unwrap();
        unused.local_addr().unwrap().port()
    };
    let body = format!(
        r#"
ip = fake_ip("localhost")
g = game(ip, {game})
u = udp()
for i in range(6):
    show(f"try{{i}}", ask(u, (ip, {closed}), b"anyone-%d" % i, 0.15))
# The same without pauses, on another socket: the error for one datagram is then still
# pending when the next is to be sent, and the kernel reports it there instead.
b = udp()
for i in range(20): b.sendto(b"burst-%d" % i, (ip, {closed}))
time.sleep(0.3)
"#
    );
    let r = run(vec![rule("localhost", game, &[closed])], None, &body).await;
    assert_eq!(r.code, Some(0), "{}", r.dump());
    for i in 0..6 {
        assert_eq!(r.line(&format!("try{i}")), "no-answer", "{}", r.dump());
    }
    let label = format!("localhost:{closed}");
    assert_eq!(r.count(&format!("ALLOW udp {label}")), 2, "{}", r.dump());
    assert_eq!(
        r.count(&format!("DENY  udp {label} (the server's address reports that datagrams to this port do not arrive")),
        1,
        "{}",
        r.dump()
    );
    // Said once, and as what it is, on whichever call the kernel reported it: not as a
    // fault of this machine ("could not send it on").
    assert_eq!(r.count("DENY  udp"), 1, "{}", r.dump());
    let (flows, relayed, refused) = r.stats;
    // A datagram either left (and was then answered by an ICMP error) or was refused by the
    // kernel on the way out because of the error before it: twenty-six in all.
    assert_eq!((flows, relayed + refused), (2, 6 + 20), "{}", r.dump());
    assert!(relayed >= 2, "{}", r.dump());
}

const CONFINED_CHILD: &str = "BASTION_VOICE_CONFINED_CHILD";

/// The re-executed half of the next test: confines itself the way the launcher does once
/// the game runs, then does what the relay does for a new flow. Nothing in a normal run.
#[test]
fn confined_child() {
    let Ok(servers) = std::env::var(CONFINED_CHILD) else { return };
    // The whole confinement, as `launcher.confine = "require"` applies it: Landlock and the
    // system-call filter on every thread.
    let applied = confine::apply(&Confinement::supervisor(), Demand::Whole).expect("confining the child");
    assert!(applied.is_whole(), "the whole confinement is in force");
    for server in servers.split(',') {
        let server: SocketAddr = server.parse().unwrap();
        let exchange = || -> std::io::Result<String> {
            let any: SocketAddr =
                if server.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
            let socket = UdpSocket::bind(any)?;
            socket.connect(server)?;
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            socket.send(b"after-confinement")?;
            let mut buf = [0u8; 64];
            let len = socket.recv(&mut buf)?;
            Ok(String::from_utf8_lossy(&buf[..len]).into_owned())
        };
        // (libtest writes "test confined_child ... " without a newline first.)
        println!("\nflow {server}={:?}", exchange());
    }
    // Control: the confinement is in force — a TCP connection to a port it does not list.
    let tcp = server_port_is_refused(servers.split(',').next().unwrap().parse().unwrap());
    println!("tcp_elsewhere={tcp}");
}

fn server_port_is_refused(server: SocketAddr) -> String {
    match std::net::TcpStream::connect(server) {
        Ok(_) => "connected".into(),
        Err(e) if e.raw_os_error() == Some(libc::EACCES) => "EACCES".into(),
        Err(e) => format!("{e}"),
    }
}

/// The relay opens its sockets towards the server when a flow begins — long after the
/// launcher has confined itself. That confinement (Landlock and a system-call filter, every
/// thread) governs files, program execution, TCP and the kernel keyring; it has no rule for
/// UDP (docs/THREAT_MODEL.md, section 6). So the relay keeps working after it — and what
/// bounds where datagrams go is the relay's own decision table, not the kernel. If the
/// launcher's confinement ever learns about UDP, this test is where it shows.
#[test]
fn the_relay_can_still_open_flows_after_the_launcher_has_confined_itself() {
    if confine::support() != Support::Whole {
        eprintln!("skipped: {:?}", confine::support());
        return;
    }
    let mut servers = vec![format!("127.0.0.1:{}", voice_server("127.0.0.1").port)];
    if UdpSocket::bind("[::1]:0").is_ok() {
        servers.push(format!("[::1]:{}", voice_server("::1").port));
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["confined_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CONFINED_CHILD, servers.join(","))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    for server in &servers {
        assert!(stdout.contains(&format!("flow {server}=Ok(\"after-confinement\")")), "{server}:\n{stdout}");
    }
    assert!(stdout.contains("tcp_elsewhere=EACCES"), "the child was confined:\n{stdout}");
}

fn compile(source: &str) -> std::path::PathBuf {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("voice-classes-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/java").join(source);
    let status =
        Command::new("javac").args(["--release", "8", "-Xlint:-options", "-d"]).arg(&out).arg(src).status().unwrap();
    assert!(status.success(), "javac failed");
    out
}

/// What a voice-chat mod does, done by a real JVM under the game's own seccomp allowlist and
/// Landlock rules: `java.net.DatagramSocket` (what Simple Voice Chat uses) and a connected
/// NIO `DatagramChannel`. Nothing in the sandbox configuration is loosened for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_jvm_in_the_sandbox_talks_to_the_voice_server_with_plain_java_sockets() {
    let classes = compile("VoiceProbe.java");
    let jvms = java::discover().expect("system JVMs");
    // The runtime a current game version would get (Minecraft 26.x declares Java 25), where
    // there is one; else whatever is installed.
    let jvm = java::select(&jvms, 25, JavaMatch::Newer, None)
        .or_else(|_| java::select(&jvms, 8, JavaMatch::Newer, None))
        .expect("a JVM");
    eprintln!("JVM under test: Java {}", jvm.version);
    let (game, voice) = (game_server("127.0.0.1"), voice_server("127.0.0.1"));

    // A name that the sandbox must ask the launcher's resolver about (so that the JVM gets a
    // synthetic address, as for any real server) and that this host resolves to its own
    // loopback without the network. Where the host's resolver has no `*.localhost`, the
    // literal address has to do; the JVM then talks to 127.0.0.1 directly.
    let name = if "voice.localhost:0".to_socket_addrs().is_ok_and(|mut all| all.any(|a| a.ip().is_loopback())) {
        "voice.localhost"
    } else {
        eprintln!("note: this host does not resolve *.localhost; testing with the literal address instead of a name");
        "127.0.0.1"
    };
    let net = NetworkConfig { servers: vec![rule(name, game, &[voice.port])], ..Default::default() };
    let policy = egress::Policy::new(&net, &InstanceNetwork::default(), None).unwrap();
    let ns = policy.netns_spec();

    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 256,
        jvm: Some(&jvm),
        exec_allow: &[],
        resolv: Resolv::Loopback,
        expose_timezone: false,
        trust: None,
    })
    .unwrap();
    base.mounts.push(Mount::RoData {
        data: BOOTSTRAP_CLASS.to_vec(),
        dst: format!("/bastion/boot/{BOOTSTRAP_CLASS_NAME}.class").into(),
        mode: 0o444,
    });
    base.mounts.push(Mount::RoBind { src: classes.clone(), dst: "/bastion/test".into() });
    base.mounts.push(Mount::Dir { dst: "/game".into(), mode: 0o700 });
    base.landlock.write.push("/game".into());
    base.landlock.tcp = TcpPolicy::Ports(policy.tcp_ports());
    base.landlock.udp_send = Some(policy.sandbox_udp_ports());
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
    vars.set("host", name).set("game", game.to_string()).set("voice", voice.port.to_string());
    let templates = ["${host}".to_string(), "${game}".to_string(), "${voice}".to_string()];
    let frame = build_frame("VoiceProbe", &templates, &vars).unwrap();
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    running.channel.write_all(frame.expose()).unwrap();
    running.channel.shutdown(Shutdown::Write).unwrap();

    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
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
    let status = tokio::time::timeout(Duration::from_secs(120), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut so), err.read_to_string(&mut se));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("JVM timed out");
    tokio::time::sleep(Duration::from_millis(300)).await;
    relay.shutdown();
    let log = lines.lock().unwrap().join("\n");
    let context = format!("--- stdout ---\n{so}\n--- stderr ---\n{se}\n--- relay ---\n{log}");
    let get = |key: &str| {
        so.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .unwrap_or_else(|| panic!("missing {key}\n{context}"))
            .to_string()
    };
    assert_eq!(status.code(), Some(0), "JVM failed\n{context}");
    assert_eq!(get("done"), "ok");
    let address = get("address");
    if name != "127.0.0.1" {
        assert!(address.starts_with("127.") && address != "127.0.0.1", "a synthetic address, got {address}");
    }
    assert_eq!(get("before_game"), "no-answer", "{context}");
    assert_eq!(get("game"), "hello");
    assert_eq!(get("socket_reply"), "voice-from-java", "{context}");
    assert_eq!(get("socket_reply_from"), format!("{address}:{}", voice.port), "{context}");
    assert_eq!(get("channel_reply"), "voice-from-nio", "{context}");
    if landlock::kernel_abi() >= landlock::UDP_ABI {
        assert!(get("other_port").contains("Permission denied"), "{}", get("other_port"));
    }
    assert_eq!(voice.payloads(), ["voice-from-java", "voice-from-nio"], "{context}");
    assert!(log.contains(&format!("ALLOW udp {name}:{}", voice.port)), "{context}");
    let _ = std::fs::remove_dir_all(classes);
}
