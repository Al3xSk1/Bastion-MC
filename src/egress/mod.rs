//! Egress control for the sandboxed game ("filtered" network mode).
//!
//! The requirements suggested a SOCKS/HTTP proxy. That cannot work for Minecraft: the game
//! connection is made by Netty, which ignores Java's proxy settings, so a proxy-aware design
//! would either break multiplayer or leave the real exfiltration path open. Instead the game
//! runs in a network namespace with **no route at all** (see `sandbox::netns`) and everything
//! it can reach is a socket this module holds:
//!
//! * a DNS responder on 127.0.0.1:53 that answers *only* allowlisted names, with synthetic
//!   addresses from 127.64.0.0/10 ("fake IPs"), and never forwards a query upstream;
//! * wildcard TCP listeners on the allowlisted ports. A connection to a fake IP arrives here;
//!   the original destination (`getsockname`) says which name was meant.
//!
//! For each connection the relay then decides:
//!
//! | destination | condition | action |
//! |---|---|---|
//! | Mojang service domain, port 443 | ClientHello SNI equals the resolved name | splice to the real host (TLS stays end-to-end) |
//! | `textures.minecraft.net`, port 80 | `GET /texture/<hex>` only | fetched by the launcher over TLS 1.3 and replayed |
//! | allowlisted game server | name/IP and port match a rule | splice (SRV indirection resolved here) |
//! | anything else | — | closed, logged |
//!
//! Outbound connections refuse private, loopback and link-local addresses unless the rule
//! says `allow_private`, so a poisoned DNS answer cannot turn the relay against the LAN.
//! UDP other than DNS, ICMP and IPv6 do not exist inside the sandbox.

pub mod broker;
pub mod dns;
pub mod sni;

use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    os::fd::OwnedFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::timeout,
};

use crate::{
    config::{DEFAULT_GAME_PORT, HostSpec, InstanceNetwork, NetworkConfig, ServerRule, normalize_host},
    error::{Context, Error, Result},
    net::http::HttpClient,
    sandbox::netns::NetnsSpec,
    util,
};

const TEXTURE_HOST: &str = "textures.minecraft.net";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 256;
const MAX_TEXTURE_BYTES: u64 = 4 << 20;

// ---------------------------------------------------------------- fake addresses

/// Name ↔ synthetic loopback address. 127.64.0.0/10 is far from anything a local service
/// uses and is, like all of 127/8, local to the sandbox's loopback interface.
pub struct FakeIps {
    by_name: HashMap<String, Ipv4Addr>,
    by_ip: HashMap<Ipv4Addr, String>,
    next: u32,
}

impl FakeIps {
    const BASE: u32 = u32::from_be_bytes([127, 64, 0, 0]);
    const MASK: u32 = 0xFFC0_0000;
    /// Bounded so that resolving endless sub-domains of an allowed zone cannot exhaust memory.
    const CAPACITY: usize = 4096;

    pub fn new() -> Self {
        FakeIps { by_name: HashMap::new(), by_ip: HashMap::new(), next: 1 }
    }

    pub fn contains(ip: Ipv4Addr) -> bool {
        u32::from(ip) & Self::MASK == Self::BASE
    }

    pub fn assign(&mut self, name: &str) -> Option<Ipv4Addr> {
        if let Some(ip) = self.by_name.get(name) {
            return Some(*ip);
        }
        if self.by_name.len() >= Self::CAPACITY {
            return None;
        }
        let ip = Ipv4Addr::from(Self::BASE + self.next);
        self.next += 1;
        self.by_name.insert(name.to_string(), ip);
        self.by_ip.insert(ip, name.to_string());
        Some(ip)
    }

    pub fn name_of(&self, ip: Ipv4Addr) -> Option<String> {
        self.by_ip.get(&ip).cloned()
    }
}

impl Default for FakeIps {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------- policy

#[derive(Debug, Clone)]
pub struct Policy {
    service_domains: Vec<String>,
    servers: Vec<ServerRule>,
    texture_gateway: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Dest {
    Name(String),
    Ip(Ipv4Addr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Route {
    Service(String),
    Texture,
    Game(ServerRule),
    Deny(&'static str),
}

impl Policy {
    pub fn new(global: &NetworkConfig, instance: &InstanceNetwork, extra: Option<ServerRule>) -> Result<Self> {
        let service_domains = global.service_domains.iter().map(|d| normalize_host(d)).collect::<Result<Vec<_>>>()?;
        let mut servers: Vec<ServerRule> = Vec::new();
        for entry in global.servers.iter().chain(&instance.servers) {
            servers.push(entry.to_rule()?);
        }
        servers.extend(extra);
        for rule in &servers {
            if let HostSpec::Ip(ip) = &rule.host
                && FakeIps::contains(*ip)
            {
                return Err(Error::Config(format!("server address {ip} lies in the range reserved for name mapping")));
            }
        }
        Ok(Policy { service_domains, servers, texture_gateway: global.texture_gateway })
    }

    fn is_service(&self, name: &str) -> bool {
        self.service_domains
            .iter()
            .any(|d| name == d || name.strip_suffix(d.as_str()).is_some_and(|p| p.ends_with('.')))
    }

    /// May the sandbox learn an address for this name at all?
    pub fn resolvable(&self, name: &str) -> bool {
        self.is_service(name) || self.servers.iter().any(|r| matches!(&r.host, HostSpec::Name(n) if n == name))
    }

    fn route(&self, dest: &Dest, port: u16) -> Route {
        match dest {
            Dest::Name(name) => {
                if let Some(rule) =
                    self.servers.iter().find(|r| r.port == port && matches!(&r.host, HostSpec::Name(n) if n == name))
                {
                    return Route::Game(rule.clone());
                }
                if port == 443 && self.is_service(name) {
                    return Route::Service(name.clone());
                }
                if port == 80 && self.texture_gateway && name == TEXTURE_HOST && self.is_service(name) {
                    return Route::Texture;
                }
                Route::Deny("name is allowlisted but not on this port")
            }
            Dest::Ip(ip) => self
                .servers
                .iter()
                .find(|r| r.port == port && matches!(&r.host, HostSpec::Ip(i) if i == ip))
                .map(|r| Route::Game(r.clone()))
                .unwrap_or(Route::Deny("address is not an allowlisted server")),
        }
    }

    /// TCP ports the sandbox may connect to (Landlock) and that need a listener.
    pub fn tcp_ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = self.servers.iter().map(|r| r.port).collect();
        if !self.service_domains.is_empty() {
            ports.push(443);
            if self.texture_gateway && self.is_service(TEXTURE_HOST) {
                ports.push(80);
            }
        }
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    pub fn netns_spec(&self) -> NetnsSpec {
        let mut local_addrs: Vec<Ipv4Addr> = self
            .servers
            .iter()
            .filter_map(|r| match &r.host {
                // Loopback addresses are already local inside the namespace.
                HostSpec::Ip(ip) if !ip.is_loopback() => Some(*ip),
                _ => None,
            })
            .collect();
        local_addrs.sort_unstable();
        local_addrs.dedup();
        NetnsSpec { tcp_ports: self.tcp_ports(), dns: true, local_addrs }
    }

    /// Names whose `_minecraft._tcp` SRV record decides where the relay really connects.
    fn srv_names(&self) -> Vec<&str> {
        self.servers
            .iter()
            .filter(|r| !r.explicit_port && r.port == DEFAULT_GAME_PORT)
            .filter_map(|r| if let HostSpec::Name(name) = &r.host { Some(name.as_str()) } else { None })
            .collect()
    }

    /// Look the SRV records up now. The answers are handed to [`start`], so the relay never
    /// has to ask again — which lets the launcher fix, before it confines itself, the set of
    /// ports it will ever connect to.
    pub async fn resolve_srv(&self) -> SrvTargets {
        let mut targets = SrvTargets::new();
        for name in self.srv_names() {
            if !targets.contains_key(name) {
                targets.insert(name.to_string(), lookup_srv_target(name).await);
            }
        }
        targets
    }

    /// Every TCP port the relay itself may connect *out* to: HTTPS, the allowlisted servers'
    /// ports and their SRV targets' ports.
    pub fn outbound_ports(&self, srv: &SrvTargets) -> Vec<u16> {
        let mut ports: Vec<u16> = self.servers.iter().map(|r| r.port).collect();
        ports.extend(srv.values().flatten().map(|(_, port)| *port));
        if !self.service_domains.is_empty() {
            ports.push(443);
        }
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    pub fn describe(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "network: filtered — HTTPS to *.{{{}}} only{}",
            self.service_domains.join(", "),
            if self.texture_gateway { " (+ skin textures via HTTPS gateway)" } else { "" }
        )];
        if self.servers.is_empty() {
            lines.push("network: no game servers allowlisted (multiplayer will not connect)".into());
        }
        for r in &self.servers {
            lines.push(format!(
                "network: game server {}:{}{}{}",
                r.host,
                r.port,
                if r.explicit_port { "" } else { " (or its SRV target)" },
                if r.allow_private { " [private addresses allowed]" } else { "" }
            ));
        }
        lines
    }
}

/// Is this an address on the public internet? Used for every outbound relay connection.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64/10 carrier-grade NAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0/24 protocol assignments
                || (o[0] == 198 && (o[1] & 0xFE) == 18) // 198.18/15 benchmarking
                || o[0] >= 240) // reserved
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xFE00) == 0xFC00 // unique local
                || (s[0] & 0xFFC0) == 0xFE80 // link local
                || (s[0] & 0xFFC0) == 0xFEC0 // deprecated site local
                || v6 == Ipv6Addr::from([0u16; 8]))
        }
    }
}

// ---------------------------------------------------------------- runtime

pub type Logger = Arc<dyn Fn(String) + Send + Sync>;

/// Server name → SRV target (`None`: there is no record).
pub type SrvTargets = HashMap<String, Option<(String, u16)>>;

async fn lookup_srv_target(name: &str) -> Option<(String, u16)> {
    match dns::lookup_srv(&format!("_minecraft._tcp.{name}")).await {
        Ok(Some(srv)) => normalize_host(&srv.target).ok().map(|target| (target, srv.port)),
        _ => None,
    }
}

#[derive(Default)]
pub struct Stats {
    pub relayed: AtomicU64,
    pub denied: AtomicU64,
    pub names_refused: AtomicU64,
    /// Requests the credential broker forwarded to Mojang…
    pub brokered: AtomicU64,
    /// …of which it authenticated this many with the real token.
    pub credentialed: AtomicU64,
    /// Requests the broker answered itself (refused, or upstream failed).
    pub broker_refused: AtomicU64,
}

struct Shared {
    policy: Policy,
    fake: Mutex<FakeIps>,
    srv_cache: Mutex<HashMap<String, Option<(String, u16)>>>,
    refused_seen: Mutex<HashSet<String>>,
    http: HttpClient,
    log: Logger,
    stats: Arc<Stats>,
    slots: Arc<Semaphore>,
    broker: Option<broker::Broker>,
    /// Open game-server connections; what a server join is tied to.
    gate: broker::JoinGate,
}

pub struct Egress {
    tasks: Vec<JoinHandle<()>>,
    pub stats: Arc<Stats>,
}

impl Egress {
    pub fn shutdown(self) {
        for t in self.tasks {
            t.abort();
        }
    }
}

/// Start serving the sockets handed over by the namespace helper (`spec` gives their order).
/// `srv` are SRV answers obtained beforehand ([`Policy::resolve_srv`]); names missing from
/// it are looked up on first use.
pub fn start(
    policy: Policy,
    spec: &NetnsSpec,
    sockets: Vec<OwnedFd>,
    http: HttpClient,
    log: Logger,
    srv: SrvTargets,
    broker: Option<broker::Broker>,
) -> Result<Egress> {
    if sockets.len() != spec.socket_count() {
        return Err(Error::Sandbox("socket count does not match the namespace specification".into()));
    }
    let stats = Arc::new(Stats::default());
    let shared = Arc::new(Shared {
        policy,
        fake: Mutex::new(FakeIps::new()),
        srv_cache: Mutex::new(srv),
        refused_seen: Mutex::new(HashSet::new()),
        http,
        log,
        stats: stats.clone(),
        slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        broker,
        gate: broker::JoinGate::default(),
    });
    let mut sockets = sockets.into_iter();
    let mut tasks = Vec::new();
    if spec.dns {
        let std_sock = std::net::UdpSocket::from(sockets.next().expect("counted"));
        std_sock.set_nonblocking(true).ctx(|| "configuring DNS socket")?;
        let sock = UdpSocket::from_std(std_sock).ctx(|| "registering DNS socket")?;
        tasks.push(tokio::spawn(serve_dns(shared.clone(), sock)));
    }
    for fd in sockets {
        let std_listener = std::net::TcpListener::from(fd);
        std_listener.set_nonblocking(true).ctx(|| "configuring relay listener")?;
        let listener = TcpListener::from_std(std_listener).ctx(|| "registering relay listener")?;
        tasks.push(tokio::spawn(serve_tcp(shared.clone(), listener)));
    }
    Ok(Egress { tasks, stats })
}

async fn serve_dns(shared: Arc<Shared>, sock: UdpSocket) {
    let mut buf = [0u8; 1024];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
        let reply = match dns::parse_query(&buf[..n]) {
            Ok(q) if shared.policy.resolvable(&q.name) => match q.qtype {
                dns::TYPE_A => match shared.fake.lock().expect("fake ip lock").assign(&q.name) {
                    Some(ip) => dns::answer_a(&q, ip),
                    None => dns::answer_empty(&q, dns::Rcode::ServFail),
                },
                // AAAA, SRV and everything else: the name exists, there is no such record.
                // (SRV indirection for game servers is performed by the relay itself.)
                _ => dns::answer_empty(&q, dns::Rcode::NoError),
            },
            Ok(q) => {
                shared.stats.names_refused.fetch_add(1, Ordering::Relaxed);
                let mut seen = shared.refused_seen.lock().expect("refused lock");
                if seen.len() < 512 && seen.insert(q.name.clone()) {
                    (shared.log)(format!(
                        "egress: DENY  name lookup {:?} (not allowlisted)",
                        util::sanitize_line(q.name.as_bytes(), 120)
                    ));
                }
                dns::answer_empty(&q, dns::Rcode::NxDomain)
            }
            Err((Some(id), rcode)) => dns::answer_error(id, rcode),
            Err((None, _)) => continue,
        };
        let _ = sock.send_to(&reply, from).await;
    }
}

async fn serve_tcp(shared: Arc<Shared>, listener: TcpListener) {
    // Dropping the JoinSet (when this task is aborted) aborts every connection it spawned.
    let mut conns = JoinSet::new();
    loop {
        // A slot is taken *before* accepting: when all are busy, further connections wait in
        // the kernel's listen backlog instead of piling up as tasks and descriptors here.
        let Ok(permit) = shared.slots.clone().acquire_owned().await else { return };
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _peer)) => {
                        let shared = shared.clone();
                        conns.spawn(async move {
                            let _permit = permit;
                            handle(&shared, stream).await;
                        });
                    }
                    // e.g. EMFILE: do not spin.
                    Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
                }
            }
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
}

async fn handle(shared: &Shared, mut inbound: TcpStream) {
    let Ok(SocketAddr::V4(local)) = inbound.local_addr() else { return };
    let (ip, port) = (*local.ip(), local.port());
    let dest = if FakeIps::contains(ip) {
        match shared.fake.lock().expect("fake ip lock").name_of(ip) {
            Some(name) => Dest::Name(name),
            None => return deny(shared, format!("{ip}:{port}"), "address was never handed out by the resolver"),
        }
    } else {
        Dest::Ip(ip)
    };
    let label = match &dest {
        Dest::Name(n) => format!("{n}:{port}"),
        Dest::Ip(i) => format!("{i}:{port}"),
    };
    let result = match shared.policy.route(&dest, port) {
        Route::Deny(why) => return deny(shared, label, why),
        Route::Service(name) => match shared.broker.as_ref().filter(|_| broker::Broker::intercepts(&name)) {
            // Mojang's API domains: terminated here, request by request (see `broker`).
            Some(broker) => {
                allow(shared, &label, "tls terminated by the credential broker");
                let ctx = broker::Context { gate: &shared.gate, log: &shared.log, stats: &shared.stats };
                broker.serve(&ctx, inbound, &name).await.map(|requests| format!("{requests} request(s)"))
            }
            None => relay_service(shared, &mut inbound, &name, &label).await,
        },
        Route::Texture => serve_texture(shared, &mut inbound, &label).await,
        Route::Game(rule) => relay_game(shared, &mut inbound, &rule, &label).await,
    };
    match result {
        Ok(detail) => (shared.log)(format!("egress: done  {label} {detail}")),
        Err(why) => deny(shared, label, &why),
    }
}

/// Record a connection the policy let through, at the moment it is established (a keep-alive
/// connection may outlive the game, so waiting for it to close would under-report).
fn allow(shared: &Shared, label: &str, detail: &str) {
    shared.stats.relayed.fetch_add(1, Ordering::Relaxed);
    (shared.log)(format!("egress: ALLOW {label} {detail}"));
}

fn deny(shared: &Shared, label: String, why: &str) {
    shared.stats.denied.fetch_add(1, Ordering::Relaxed);
    (shared.log)(format!("egress: DENY  {label} ({why})"));
}

/// Resolve on the host and connect, refusing non-public addresses unless permitted.
async fn connect_out(host: &str, port: u16, allow_private: bool) -> std::result::Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| "DNS timeout".to_string())?
        .map_err(|e| format!("DNS failure: {e}"))?
        .collect();
    connect_addrs(addrs, allow_private).await
}

async fn connect_addrs(addrs: Vec<SocketAddr>, allow_private: bool) -> std::result::Result<TcpStream, String> {
    if addrs.is_empty() {
        return Err("no addresses".into());
    }
    let usable: Vec<SocketAddr> = addrs.iter().copied().filter(|a| allow_private || is_public(a.ip())).collect();
    if usable.is_empty() {
        return Err(format!(
            "resolves only to non-public addresses ({}); set allow_private for a LAN server",
            addrs[0].ip()
        ));
    }
    let mut last = String::new();
    for addr in usable {
        match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => last = format!("{addr}: {e}"),
            Err(_) => last = format!("{addr}: connect timed out"),
        }
    }
    Err(format!("cannot connect ({last})"))
}

async fn splice(inbound: &mut TcpStream, outbound: &mut TcpStream) -> (u64, u64) {
    tokio::io::copy_bidirectional(inbound, outbound).await.unwrap_or((0, 0))
}

/// HTTPS to a Mojang service: forwarded only if the ClientHello asks for exactly that host.
async fn relay_service(
    shared: &Shared,
    inbound: &mut TcpStream,
    name: &str,
    label: &str,
) -> std::result::Result<String, String> {
    let mut buf = Vec::with_capacity(2048);
    let hello = timeout(HELLO_TIMEOUT, async {
        loop {
            match sni::parse_client_hello(&buf) {
                sni::Hello::Incomplete if buf.len() < sni::MAX_HELLO => {}
                other => return other,
            }
            let mut chunk = [0u8; 4096];
            match inbound.read(&mut chunk).await {
                Ok(0) | Err(_) => return sni::Hello::Invalid,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    })
    .await
    .map_err(|_| "no TLS ClientHello received".to_string())?;
    match hello {
        sni::Hello::Complete(Some(sni)) if sni == name => {}
        sni::Hello::Complete(Some(other)) => return Err(format!("TLS SNI {other:?} does not match the resolved name")),
        sni::Hello::Complete(None) => return Err("TLS ClientHello without SNI".into()),
        _ => return Err("not a TLS ClientHello".into()),
    }
    let mut outbound = connect_out(name, 443, false).await?;
    allow(
        shared,
        label,
        &format!("tls, SNI verified → {}", outbound.peer_addr().map(|a| a.to_string()).unwrap_or_default()),
    );
    outbound.write_all(&buf).await.map_err(|e| format!("upstream write: {e}"))?;
    let (up, down) = splice(inbound, &mut outbound).await;
    Ok(format!("↑{} ↓{} bytes", up + buf.len() as u64, down))
}

async fn srv_target(shared: &Shared, name: &str) -> Option<(String, u16)> {
    if let Some(cached) = shared.srv_cache.lock().expect("srv lock").get(name) {
        return cached.clone();
    }
    let found = lookup_srv_target(name).await;
    shared.srv_cache.lock().expect("srv lock").insert(name.to_string(), found.clone());
    found
}

async fn relay_game(
    shared: &Shared,
    inbound: &mut TcpStream,
    rule: &ServerRule,
    label: &str,
) -> std::result::Result<String, String> {
    let (mut outbound, via) = match &rule.host {
        HostSpec::Ip(ip) => {
            (connect_addrs(vec![SocketAddr::new((*ip).into(), rule.port)], rule.allow_private).await?, String::new())
        }
        HostSpec::Name(name) => {
            // The client inside cannot see SRV records, so it dials the default port; the
            // indirection Minecraft would have followed is applied here instead.
            let srv = if rule.explicit_port || rule.port != DEFAULT_GAME_PORT {
                None
            } else {
                srv_target(shared, name).await
            };
            match srv {
                Some((target, port)) => {
                    (connect_out(&target, port, rule.allow_private).await?, format!(" via SRV {target}:{port}"))
                }
                None => (connect_out(name, rule.port, rule.allow_private).await?, String::new()),
            }
        }
    };
    let peer = outbound.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    allow(shared, label, &format!("game server → {peer}{via}"));
    // While this connection is open (and young), the game may ask for one server join.
    let _joinable = shared.gate.opened(label);
    let (up, down) = splice(inbound, &mut outbound).await;
    Ok(format!("↑{up} ↓{down} bytes"))
}

/// Skin textures are published as `http://` URLs. Rather than letting plaintext HTTP out,
/// accept exactly one shape of request and fetch it ourselves over TLS.
async fn serve_texture(shared: &Shared, inbound: &mut TcpStream, label: &str) -> std::result::Result<String, String> {
    let mut buf = Vec::with_capacity(1024);
    let head_end = timeout(HELLO_TIMEOUT, async {
        loop {
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                return Some(pos);
            }
            if buf.len() > 8192 {
                return None;
            }
            let mut chunk = [0u8; 2048];
            match inbound.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| "no complete HTTP request".to_string())?;

    let path = match parse_texture_request(&buf[..head_end]) {
        Ok(p) => p,
        Err(why) => {
            let _ =
                inbound.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            return Err(why.to_string());
        }
    };
    match shared.http.get(&format!("https://{TEXTURE_HOST}{path}"), MAX_TEXTURE_BYTES).await {
        Ok(body) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = inbound.write_all(head.as_bytes()).await;
            let _ = inbound.write_all(&body).await;
            let _ = inbound.shutdown().await;
            allow(shared, label, "skin texture, fetched by the launcher over TLS");
            Ok(format!("{} bytes", body.len()))
        }
        Err(e) => {
            let _ =
                inbound.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            Err(format!("texture fetch failed: {e}"))
        }
    }
}

/// Accept only `GET /texture/<hex> HTTP/1.x` with `Host: textures.minecraft.net`.
fn parse_texture_request(head: &[u8]) -> std::result::Result<String, &'static str> {
    let text = std::str::from_utf8(head).map_err(|_| "request is not ASCII")?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (method, path, version) = (first.next(), first.next(), first.next());
    if method != Some("GET") || first.next().is_some() || !matches!(version, Some("HTTP/1.1" | "HTTP/1.0")) {
        return Err("only GET is served");
    }
    let path = path.unwrap_or_default();
    let hash = path.strip_prefix("/texture/").ok_or("only /texture/<hash> is served")?;
    if hash.is_empty() || hash.len() > 128 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("texture id is not a hash");
    }
    let host_ok = lines.any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.eq_ignore_ascii_case("host")
                && matches!(v.trim().to_ascii_lowercase().as_str(), TEXTURE_HOST | "textures.minecraft.net:80")
        })
    });
    if !host_ok {
        return Err("Host header is not the texture server");
    }
    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerEntry, parse_server};

    fn policy(servers: &[&str]) -> Policy {
        let net = NetworkConfig {
            servers: servers.iter().map(|s| ServerEntry::Short(s.to_string())).collect(),
            ..Default::default()
        };
        Policy::new(&net, &InstanceNetwork::default(), None).unwrap()
    }

    #[test]
    fn fake_addresses_are_stable_unique_and_bounded() {
        let mut f = FakeIps::new();
        let a = f.assign("a.example").unwrap();
        let b = f.assign("b.example").unwrap();
        assert_ne!(a, b);
        assert_eq!(f.assign("a.example"), Some(a));
        assert_eq!(f.name_of(b).as_deref(), Some("b.example"));
        assert!(FakeIps::contains(a) && a.is_loopback());
        assert!(!FakeIps::contains(Ipv4Addr::LOCALHOST));
        assert!(!FakeIps::contains(Ipv4Addr::new(127, 128, 0, 1)));
        assert!(f.name_of(Ipv4Addr::new(127, 64, 9, 9)).is_none());
        for i in 0..FakeIps::CAPACITY {
            f.assign(&format!("h{i}.example"));
        }
        assert!(f.assign("one-too-many.example").is_none());
    }

    #[test]
    fn service_domain_matching_is_label_exact() {
        let p = policy(&[]);
        for ok in ["minecraft.net", "sessionserver.mojang.com", "api.minecraftservices.com", "a.b.minecraft.net"] {
            assert!(p.resolvable(ok), "{ok}");
        }
        for bad in [
            "evilminecraft.net",
            "minecraft.net.evil.example",
            "mojang.com.evil.example",
            "example.com",
            "notmojang.com",
            "",
        ] {
            assert!(!p.resolvable(bad), "{bad}");
        }
    }

    #[test]
    fn routing_table() {
        let p = policy(&["play.example.org", "lan.example.org:25570", "203.0.113.7:25565"]);
        let name = |n: &str| Dest::Name(n.to_string());
        assert_eq!(p.route(&name("sessionserver.mojang.com"), 443), Route::Service("sessionserver.mojang.com".into()));
        assert_eq!(p.route(&name("textures.minecraft.net"), 80), Route::Texture);
        assert!(
            matches!(p.route(&name("sessionserver.mojang.com"), 80), Route::Deny(_)),
            "plain HTTP only for textures"
        );
        assert!(matches!(p.route(&name("sessionserver.mojang.com"), 25565), Route::Deny(_)));
        assert!(matches!(p.route(&name("play.example.org"), 25565), Route::Game(_)));
        assert!(
            matches!(p.route(&name("play.example.org"), 443), Route::Deny(_)),
            "a game server is not an HTTPS allowance"
        );
        assert!(matches!(p.route(&name("lan.example.org"), 25570), Route::Game(_)));
        assert!(matches!(p.route(&name("lan.example.org"), 25565), Route::Deny(_)));
        assert!(matches!(p.route(&Dest::Ip(Ipv4Addr::new(203, 0, 113, 7)), 25565), Route::Game(_)));
        assert!(matches!(p.route(&Dest::Ip(Ipv4Addr::new(203, 0, 113, 8)), 25565), Route::Deny(_)));
        assert!(matches!(p.route(&Dest::Ip(Ipv4Addr::LOCALHOST), 443), Route::Deny(_)));
        assert_eq!(p.tcp_ports(), vec![80, 443, 25565, 25570]);
        // Outbound: no plaintext port (textures are fetched over HTTPS), plus SRV targets.
        assert_eq!(p.srv_names(), vec!["play.example.org"], "only names with an implicit port follow SRV");
        let srv = SrvTargets::from([
            ("play.example.org".to_string(), Some(("node7.example.net".to_string(), 30123))),
            ("unused.example.org".to_string(), None),
        ]);
        assert_eq!(p.outbound_ports(&srv), vec![443, 25565, 25570, 30123]);
        assert_eq!(p.outbound_ports(&SrvTargets::new()), vec![443, 25565, 25570]);
        let spec = p.netns_spec();
        assert_eq!(spec.local_addrs, vec![Ipv4Addr::new(203, 0, 113, 7)]);
        assert!(spec.dns);
    }

    #[test]
    fn reserved_range_cannot_be_allowlisted() {
        let net = NetworkConfig { servers: vec![ServerEntry::Short("127.64.0.5:25565".into())], ..Default::default() };
        assert!(Policy::new(&net, &InstanceNetwork::default(), None).is_err());
        let extra = parse_server("mc.example.net", false).unwrap();
        let p = Policy::new(&NetworkConfig::default(), &InstanceNetwork::default(), Some(extra)).unwrap();
        assert!(p.resolvable("mc.example.net"));
    }

    #[test]
    fn public_address_classification() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "198.18.0.1",
            "240.0.0.1",
            "192.0.0.8",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
            "ff02::1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
            "fec0::1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private} must not count as public");
        }
        for public in
            ["1.1.1.1", "8.8.8.8", "172.32.0.1", "100.128.0.1", "198.20.0.1", "2606:4700::1111", "::ffff:8.8.8.8"]
        {
            assert!(is_public(public.parse().unwrap()), "{public} is public");
        }
    }

    #[test]
    fn texture_gateway_accepts_one_request_shape() {
        let ok = b"GET /texture/1a4af718455d4aab528e7a61f86fa25e HTTP/1.1\r\nUser-Agent: Java\r\nHost: textures.minecraft.net\r\nAccept: */*";
        assert_eq!(parse_texture_request(ok).unwrap(), "/texture/1a4af718455d4aab528e7a61f86fa25e");
        for bad in [
            &b"POST /texture/abcd HTTP/1.1\r\nHost: textures.minecraft.net"[..],
            b"GET /texture/../../etc/passwd HTTP/1.1\r\nHost: textures.minecraft.net",
            b"GET /texture/abcd?exfil=secret HTTP/1.1\r\nHost: textures.minecraft.net",
            b"GET /other/abcd HTTP/1.1\r\nHost: textures.minecraft.net",
            b"GET /texture/abcd HTTP/1.1\r\nHost: attacker.example",
            b"GET /texture/abcd HTTP/1.1",
            b"GET http://attacker.example/texture/abcd HTTP/1.1\r\nHost: textures.minecraft.net",
            b"GET /texture/abcd HTTP/2\r\nHost: textures.minecraft.net",
            b"GET /texture/ HTTP/1.1\r\nHost: textures.minecraft.net",
            b"GET  /texture/abcd HTTP/1.1\r\nHost: textures.minecraft.net",
            b"\xff\xfe",
        ] {
            assert!(parse_texture_request(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }
}
