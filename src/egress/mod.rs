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
//! ICMP and IPv6 do not exist inside the sandbox, and neither does UDP — other than DNS to
//! the responder above and, only where a server rule names UDP ports, datagrams to those
//! ports, which the launcher relays to the address of the open game connection to that
//! server and nowhere else (voice chat; see [`udp`]).
//!
//! With `network.launcher_proxy` set, everything in this table that goes to a Mojang service
//! leaves through that SOCKS5 proxy, which also resolves the name: the pass-through HTTPS
//! connections of the first row, like the texture gateway's and the credential broker's own
//! requests. Game servers are dialled directly, and their names are resolved here.

pub mod broker;
pub mod dns;
pub mod sni;
pub mod udp;

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
    sandbox::netns::{NetnsSpec, SocketRole},
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
    /// `network.launcher_proxy` is set: Mojang's services are reached through it.
    proxied: bool,
    udp_limits: udp::Limits,
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
        let policy = Policy {
            service_domains,
            servers,
            texture_gateway: global.texture_gateway,
            proxied: global.launcher_proxy.is_some(),
            udp_limits: udp::Limits::default(),
        };
        // Each port is a socket in the sandbox's namespace and a task here.
        if policy.udp_ports().len() > udp::MAX_PORTS {
            return Err(Error::Config(format!(
                "the server rules name {} different UDP ports; at most {} in total",
                policy.udp_ports().len(),
                udp::MAX_PORTS
            )));
        }
        Ok(policy)
    }

    /// Other limits for the datagram relay than the built-in ones. Not reachable from a
    /// configuration file; the tests use it to see a flow expire without waiting half a minute.
    #[doc(hidden)]
    pub fn with_udp_limits(mut self, limits: udp::Limits) -> Self {
        self.udp_limits = limits;
        self
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

    /// UDP ports that server rules name: each needs a socket in the sandbox's namespace.
    pub fn udp_ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = self.servers.iter().flat_map(|r| r.udp.iter().copied()).collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    /// Every UDP port the sandbox may send to (Landlock): the DNS responder's, and the ports
    /// that server rules name. Without such a rule that is port 53 alone, as it always was.
    pub fn sandbox_udp_ports(&self) -> Vec<u16> {
        let mut ports = vec![53];
        ports.extend(self.udp_ports());
        ports
    }

    /// The rules under which a datagram to `dest`, UDP port `port`, may be relayed at all
    /// (whether it is, depends on an open game connection: see [`udp`]) — or why none does.
    fn udp_rules(&self, dest: &Dest, port: u16) -> std::result::Result<Vec<&ServerRule>, udp::Refusal> {
        let of_host: Vec<&ServerRule> = self
            .servers
            .iter()
            .filter(|r| match (dest, &r.host) {
                (Dest::Name(name), HostSpec::Name(host)) => name == host,
                (Dest::Ip(ip), HostSpec::Ip(host)) => ip == host,
                _ => false,
            })
            .collect();
        if of_host.is_empty() {
            return Err(udp::Refusal::NotAServer);
        }
        if of_host.iter().all(|r| r.udp.is_empty()) {
            return Err(udp::Refusal::NoUdpRule);
        }
        let naming: Vec<&ServerRule> = of_host.into_iter().filter(|r| r.udp.contains(&port)).collect();
        if naming.is_empty() { Err(udp::Refusal::PortNotListed) } else { Ok(naming) }
    }

    /// May datagrams follow a game connection made under `rule` (as picked for a TCP
    /// connection)? They may if this rule, or another one for the same host and port, names
    /// UDP ports. For every other game connection nothing about the relay differs from a
    /// launch without any such rule.
    fn followed_by_datagrams(&self, rule: &ServerRule) -> bool {
        self.servers.iter().any(|r| r.host == rule.host && r.port == rule.port && !r.udp.is_empty())
    }

    /// How to order the addresses of the server that `rule` (as picked for a TCP connection)
    /// stands for: IPv4 first if datagrams may follow that connection.
    fn address_order(&self, rule: &ServerRule) -> AddressOrder {
        if self.followed_by_datagrams(rule) { AddressOrder::V4First } else { AddressOrder::Resolver }
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
        NetnsSpec { tcp_ports: self.tcp_ports(), dns: true, local_addrs, udp_ports: self.udp_ports() }
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
    ///
    /// With `network.launcher_proxy` set, HTTPS is not among them: Mojang's services are
    /// then reached through the proxy (whose port the launcher adds separately), so the
    /// launcher, once confined to this list, cannot open a direct connection to port 443
    /// even by mistake — unless a game server of yours listens there.
    pub fn outbound_ports(&self, srv: &SrvTargets) -> Vec<u16> {
        let mut ports: Vec<u16> = self.servers.iter().map(|r| r.port).collect();
        ports.extend(srv.values().flatten().map(|(_, port)| *port));
        if !self.service_domains.is_empty() && !self.proxied {
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
                "network: game server {}:{}{}{}{}",
                r.host,
                r.port,
                if r.explicit_port { "" } else { " (or its SRV target)" },
                if r.allow_private { " [private addresses allowed]" } else { "" },
                if r.udp.is_empty() {
                    String::new()
                } else {
                    format!(
                        " + UDP port(s) {} of the same address, while a game connection to it is open",
                        r.udp.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
                    )
                }
            ));
        }
        lines
    }
}

/// What `network.launcher_proxy` carries and what it does not, for the launch log. Exact,
/// because "my traffic goes through Tor" is a claim people rely on: the launcher's own
/// requests always do; what the game sends does only where the launcher stands in between
/// (filtered mode), and then only towards Mojang's services — never towards game servers.
pub fn describe_proxy(proxy: SocketAddr, mode: crate::config::NetworkMode) -> String {
    use crate::config::NetworkMode;
    let own = format!(
        "network: through the SOCKS5 proxy at {proxy}, which also resolves the names: the launcher's sign-in and \
         downloads"
    );
    match mode {
        NetworkMode::Filtered => format!(
            "{own}, and everything that goes to Mojang's services while the game runs (what the credential broker \
             forwards, skin textures, and the game's own HTTPS connections that the relay passes through). NOT \
             through the proxy: connections to multiplayer servers, the lookup of their names and SRV records (your \
             resolver sees those), and UDP voice — all of these come from your address"
        ),
        NetworkMode::Open => format!(
            "{own}. NOT through the proxy: anything the game itself sends — with the open network it connects to \
             Mojang's services and to servers directly, from your address, and resolves names with your resolver"
        ),
        NetworkMode::None => format!("{own}. The game itself has no network in this session"),
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
    /// Datagram flows the UDP relay opened…
    pub udp_flows: AtomicU64,
    /// …datagrams it relayed, both directions together…
    pub udp_relayed: AtomicU64,
    /// …and datagrams it refused or could not deliver.
    pub udp_refused: AtomicU64,
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
    /// The datagram relay. `None` unless a server rule names a UDP port.
    udp: Option<udp::Relay>,
}

impl Shared {
    /// What a destination address inside the sandbox stands for: the name a synthetic
    /// address was handed out for, or the literal address itself. `None`: a synthetic
    /// address the resolver never handed out.
    fn dest_of(&self, ip: Ipv4Addr) -> Option<Dest> {
        if FakeIps::contains(ip) {
            self.fake.lock().expect("fake ip lock").name_of(ip).map(Dest::Name)
        } else {
            Some(Dest::Ip(ip))
        }
    }
}

pub struct Egress {
    tasks: Vec<JoinHandle<()>>,
    shared: Arc<Shared>,
    pub stats: Arc<Stats>,
}

impl Egress {
    pub fn shutdown(self) {
        for t in self.tasks {
            t.abort();
        }
        // Flows that were still open say what they carried.
        udp::stop(&self.shared);
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
    let udp = (!spec.udp_ports.is_empty()).then(|| udp::Relay::new(policy.udp_limits.clone()));
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
        udp,
    });
    let mut tasks = Vec::new();
    // Which socket is which is a matter of position: `NetnsSpec::sockets` is the order the
    // helper created them in (and `sandbox::spawn` checked each against it).
    for (role, fd) in spec.sockets().into_iter().zip(sockets) {
        match role {
            SocketRole::Dns => {
                let std_sock = std::net::UdpSocket::from(fd);
                std_sock.set_nonblocking(true).ctx(|| "configuring DNS socket")?;
                let sock = UdpSocket::from_std(std_sock).ctx(|| "registering DNS socket")?;
                tasks.push(tokio::spawn(serve_dns(shared.clone(), sock)));
            }
            SocketRole::Tcp(_) => {
                let std_listener = std::net::TcpListener::from(fd);
                std_listener.set_nonblocking(true).ctx(|| "configuring relay listener")?;
                let listener = TcpListener::from_std(std_listener).ctx(|| "registering relay listener")?;
                tasks.push(tokio::spawn(serve_tcp(shared.clone(), listener)));
            }
            SocketRole::Udp(port) => {
                let socket = udp::GameSocket::new(fd, port)?;
                tasks.push(tokio::spawn(udp::serve(shared.clone(), socket)));
            }
        }
    }
    Ok(Egress { tasks, shared, stats })
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
    let Some(dest) = shared.dest_of(ip) else {
        return deny(shared, format!("{ip}:{port}"), "address was never handed out by the resolver");
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
async fn connect_out(
    host: &str,
    port: u16,
    allow_private: bool,
    order: AddressOrder,
) -> std::result::Result<TcpStream, String> {
    let mut addrs: Vec<SocketAddr> = timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| "DNS timeout".to_string())?
        .map_err(|e| format!("DNS failure: {e}"))?
        .collect();
    order.apply(&mut addrs);
    connect_addrs(addrs, allow_private).await
}

/// In which order the addresses of a name are tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddressOrder {
    /// As the system resolver returned them.
    Resolver,
    /// IPv4 addresses first, otherwise as returned. For game servers whose rule names UDP
    /// ports: datagrams go to the address the game connection uses, so the choice made
    /// here is the choice for voice as well. The Java client, left to itself, prefers IPv4
    /// for both, and a voice port is more often reachable over IPv4 only than over IPv6
    /// only; the resolver's order (IPv6 first on most dual-stack networks) would send voice
    /// where the unsandboxed game would not have sent it. The set of addresses is the same.
    V4First,
}

impl AddressOrder {
    fn apply(self, addrs: &mut [SocketAddr]) {
        if self == AddressOrder::V4First {
            addrs.sort_by_key(SocketAddr::is_ipv6); // stable: the resolver's order within each family
        }
    }
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

/// [`splice`] for a game connection that datagrams follow. The bytes are copied in the same
/// way (each direction until its reader ends, then the writer is shut down; the first error
/// ends both). In addition `anchor` is dropped — which ends the datagram flows that follow
/// this connection — as soon as **either** side has closed its direction or failed, and not
/// only when both have.
///
/// A connection that one side has hung up on is not an open game connection, however long
/// the other side keeps its end. `splice` alone would say otherwise: it returns when both
/// directions are done, so a server that never closes (or whose close is lost on the way)
/// would keep the datagram path open after the player has left, and code in the game could
/// keep it open after the server has hung up, simply by not closing its socket.
async fn splice_anchored(inbound: &mut TcpStream, outbound: &mut TcpStream, anchor: udp::Anchor<'_>) -> (u64, u64) {
    let anchor = Mutex::new(Some(anchor));
    let release = || drop(anchor.lock().expect("anchor lock").take());
    let (mut from_game, mut to_game) = inbound.split();
    let (mut from_server, mut to_server) = outbound.split();
    let up = async {
        let copied = tokio::io::copy(&mut from_game, &mut to_server).await;
        release();
        let copied = copied?;
        to_server.shutdown().await?;
        Ok::<u64, std::io::Error>(copied)
    };
    let down = async {
        let copied = tokio::io::copy(&mut from_server, &mut to_game).await;
        release();
        let copied = copied?;
        to_game.shutdown().await?;
        Ok::<u64, std::io::Error>(copied)
    };
    tokio::try_join!(up, down).unwrap_or((0, 0))
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
    let (mut outbound, via) = match shared.http.socks5() {
        // `network.launcher_proxy`: the launcher's traffic to Mojang goes through the proxy,
        // and this connection is such traffic even though the bytes are the game's. The
        // proxy is given the name; it is not looked up here, so neither the local resolver
        // nor the local network learns it, and Mojang sees the proxy's address. (What the
        // name resolves to is then the proxy's business: the refusal of private addresses
        // that `connect_out` applies cannot be applied to an address this process never sees.)
        Some(proxy) => (
            crate::net::http::socks5_dial(proxy, name, 443).await?,
            format!("the SOCKS5 proxy at {proxy}, which resolved the name"),
        ),
        None => {
            let outbound = connect_out(name, 443, false, AddressOrder::Resolver).await?;
            let peer = outbound.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            (outbound, peer)
        }
    };
    allow(shared, label, &format!("tls, SNI verified → {via}"));
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
            let order = shared.policy.address_order(rule);
            match srv {
                Some((target, port)) => {
                    (connect_out(&target, port, rule.allow_private, order).await?, format!(" via SRV {target}:{port}"))
                }
                None => (connect_out(name, rule.port, rule.allow_private, order).await?, String::new()),
            }
        }
    };
    let peer = outbound.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    allow(shared, label, &format!("game server → {peer}{via}"));
    // While this connection is open (and young), the game may ask for one server join.
    let _joinable = shared.gate.opened(label);
    // While it is open — in both directions — datagrams to the UDP ports named for this
    // server go to the address it uses. A connection that no datagram can follow is spliced
    // exactly as it always was.
    let anchor = if shared.policy.followed_by_datagrams(rule) {
        udp::anchor(shared, rule, label, outbound.peer_addr().ok())
    } else {
        None
    };
    let (up, down) = match anchor {
        Some(anchor) => splice_anchored(inbound, &mut outbound, anchor).await,
        None => splice(inbound, &mut outbound).await,
    };
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
    fn udp_ports_come_from_server_rules_and_from_nowhere_else() {
        // No rule names a UDP port: nothing about the sandbox changes.
        let none = policy(&["play.example.org", "203.0.113.7:25570"]);
        assert!(none.udp_ports().is_empty());
        assert_eq!(none.sandbox_udp_ports(), [53], "the DNS port alone, as before");
        assert!(none.netns_spec().udp_ports.is_empty());
        assert_eq!(none.netns_spec().socket_count(), 1 + none.tcp_ports().len());

        let net: NetworkConfig = toml::from_str(
            r#"servers = [
                "plain.example.org",
                { host = "play.example.org", voice = 24454 },
                { host = "203.0.113.7", port = 25570, udp = [24455, 24454] },
            ]"#,
        )
        .unwrap();
        let instance: InstanceNetwork =
            toml::from_str(r#"servers = [{ host = "lan.box", allow_private = true, voice = 30000 }]"#).unwrap();
        let join = parse_server("join.example.org", false).unwrap();
        let p = Policy::new(&net, &instance, Some(join)).unwrap();
        assert_eq!(p.udp_ports(), [24454, 24455, 30000], "global and instance rules, each port once");
        assert_eq!(p.sandbox_udp_ports(), [53, 24454, 24455, 30000], "exactly those, and DNS");
        let spec = p.netns_spec();
        assert_eq!(spec.udp_ports, [24454, 24455, 30000]);
        assert_eq!(spec.socket_count(), 1 + p.tcp_ports().len() + 3);
        assert_eq!(p.tcp_ports(), [80, 443, 25565, 25570], "a UDP port is not a TCP allowance");
        assert_eq!(p.outbound_ports(&SrvTargets::new()), [443, 25565, 25570], "nor one for the launcher");

        // Which rules let a datagram through, by destination and port.
        let name = |n: &str| Dest::Name(n.to_string());
        let literal = |a, b, c, d| Dest::Ip(Ipv4Addr::new(a, b, c, d));
        let allowed = |dest: &Dest, port| p.udp_rules(dest, port).map(|rules| rules.len());
        assert_eq!(allowed(&name("play.example.org"), 24454), Ok(1));
        assert_eq!(allowed(&name("lan.box"), 30000), Ok(1));
        assert_eq!(allowed(&literal(203, 0, 113, 7), 24455), Ok(1));
        assert_eq!(
            allowed(&name("play.example.org"), 24455),
            Err(udp::Refusal::PortNotListed),
            "another server's port"
        );
        assert_eq!(allowed(&name("play.example.org"), 25565), Err(udp::Refusal::PortNotListed), "its own TCP port");
        assert_eq!(allowed(&name("plain.example.org"), 24454), Err(udp::Refusal::NoUdpRule));
        assert_eq!(allowed(&name("join.example.org"), 24454), Err(udp::Refusal::NoUdpRule), "--join allows TCP only");
        assert_eq!(allowed(&name("sessionserver.mojang.com"), 24454), Err(udp::Refusal::NotAServer));
        assert_eq!(allowed(&name("example.com"), 24454), Err(udp::Refusal::NotAServer));
        assert_eq!(allowed(&literal(203, 0, 113, 8), 24455), Err(udp::Refusal::NotAServer), "the address next door");
        assert_eq!(allowed(&literal(127, 0, 0, 1), 24454), Err(udp::Refusal::NotAServer));

        // The launch says what is open.
        let lines = p.describe();
        let line = |host: &str| lines.iter().find(|l| l.contains(host)).unwrap_or_else(|| panic!("{host}: {lines:?}"));
        assert!(line("play.example.org").contains("UDP port(s) 24454"), "{lines:?}");
        assert!(line("203.0.113.7").contains("UDP port(s) 24454, 24455"), "{lines:?}");
        assert!(!line("plain.example.org").contains("UDP") && !line("join.example.org").contains("UDP"), "{lines:?}");
    }

    #[test]
    fn a_server_with_udp_ports_is_dialled_over_ipv4_first() {
        let p = Policy::new(
            &toml::from_str(
                r#"servers = ["plain.example.org", "twice.example.org", { host = "voice.example.org", voice = 24454 }]"#,
            )
            .unwrap(),
            // The same server again, in the instance's settings, now with a voice port.
            &toml::from_str(r#"servers = [{ host = "twice.example.org", voice = 24454 }]"#).unwrap(),
            None,
        )
        .unwrap();
        let picked = |host: &str| match p.route(&Dest::Name(host.to_string()), 25565) {
            Route::Game(rule) => p.address_order(&rule),
            other => panic!("{host}: {other:?}"),
        };
        assert_eq!(picked("plain.example.org"), AddressOrder::Resolver, "nothing changes without UDP ports");
        assert_eq!(picked("voice.example.org"), AddressOrder::V4First);
        assert_eq!(picked("twice.example.org"), AddressOrder::V4First, "whichever of the two rules routing picked");

        let addr = |text: &str| text.parse::<SocketAddr>().unwrap();
        let resolved = [
            addr("[2001:db8::1]:25565"),
            addr("192.0.2.1:25565"),
            addr("[2001:db8::2]:25565"),
            addr("192.0.2.2:25565"),
        ];
        let mut addrs = resolved;
        AddressOrder::Resolver.apply(&mut addrs);
        assert_eq!(addrs, resolved);
        AddressOrder::V4First.apply(&mut addrs);
        assert_eq!(addrs, [resolved[1], resolved[3], resolved[0], resolved[2]], "the same addresses, IPv4 first");
    }

    #[test]
    fn the_number_of_udp_ports_is_bounded() {
        let servers = |count: usize| -> Vec<ServerEntry> {
            (0..count)
                .map(|i| ServerEntry::Full {
                    host: format!("s{i}.example.org"),
                    port: None,
                    allow_private: false,
                    voice: None,
                    udp: (0..4).map(|j| 20000 + (i * 4 + j) as u16).collect(),
                })
                .collect()
        };
        let build = |count| {
            Policy::new(
                &NetworkConfig { servers: servers(count), ..Default::default() },
                &InstanceNetwork::default(),
                None,
            )
        };
        assert_eq!(build(4).unwrap().udp_ports().len(), udp::MAX_PORTS);
        assert!(matches!(build(5), Err(Error::Config(m)) if m.contains("at most 16")));
        // Many servers that share one voice port are one port.
        let shared: Vec<ServerEntry> = (0..40)
            .map(|i| ServerEntry::Full {
                host: format!("s{i}.example.org"),
                port: None,
                allow_private: false,
                voice: Some(24454),
                udp: vec![],
            })
            .collect();
        let p =
            Policy::new(&NetworkConfig { servers: shared, ..Default::default() }, &InstanceNetwork::default(), None);
        assert_eq!(p.unwrap().udp_ports(), [24454]);
    }

    #[test]
    fn the_proxy_note_says_what_is_and_what_is_not_proxied_in_each_network_mode() {
        use crate::config::NetworkMode;
        let proxy: SocketAddr = "127.0.0.1:9050".parse().unwrap();
        let filtered = describe_proxy(proxy, NetworkMode::Filtered);
        for said in ["127.0.0.1:9050", "sign-in", "downloads", "credential broker", "skin textures", "passes through"] {
            assert!(filtered.contains(said), "{said}: {filtered}");
        }
        let (proxied, direct) = filtered.split_once("NOT through the proxy").expect("both halves");
        assert!(!proxied.contains("multiplayer") && !proxied.contains("UDP"), "{proxied}");
        for said in ["multiplayer servers", "lookup of their names", "UDP voice", "your address"] {
            assert!(direct.contains(said), "{said}: {direct}");
        }
        // With the open network the game's traffic is not the launcher's to route.
        let open = describe_proxy(proxy, NetworkMode::Open);
        let (proxied, direct) = open.split_once("NOT through the proxy").expect("both halves");
        assert!(proxied.contains("sign-in") && !proxied.contains("broker") && !proxied.contains("game"), "{proxied}");
        assert!(direct.contains("Mojang's services") && direct.contains("directly"), "{direct}");
        let offline = describe_proxy(proxy, NetworkMode::None);
        assert!(offline.contains("sign-in") && offline.contains("no network"), "{offline}");
    }

    #[test]
    fn with_a_proxy_the_launcher_itself_needs_no_direct_https() {
        let servers =
            vec![ServerEntry::Short("play.example.org".into()), ServerEntry::Short("lan.example.org:25570".into())];
        let proxied = NetworkConfig {
            launcher_proxy: Some("socks5h://127.0.0.1:9050".into()),
            servers: servers.clone(),
            ..Default::default()
        };
        let p = Policy::new(&proxied, &InstanceNetwork::default(), None).unwrap();
        assert_eq!(p.outbound_ports(&SrvTargets::new()), [25565, 25570], "Mojang's services are behind the proxy");
        // Inside the sandbox nothing changes: the game still dials port 443, and reaches the relay.
        assert_eq!(p.tcp_ports(), [80, 443, 25565, 25570]);
        // A game server that listens on 443 is dialled directly, proxy or not.
        let odd = NetworkConfig { servers: vec![ServerEntry::Short("odd.example.org:443".into())], ..proxied };
        let p = Policy::new(&odd, &InstanceNetwork::default(), None).unwrap();
        assert_eq!(p.outbound_ports(&SrvTargets::new()), [443]);
        // Without a proxy: as before.
        let direct = NetworkConfig { servers, ..Default::default() };
        let p = Policy::new(&direct, &InstanceNetwork::default(), None).unwrap();
        assert_eq!(p.outbound_ports(&SrvTargets::new()), [443, 25565, 25570]);
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
