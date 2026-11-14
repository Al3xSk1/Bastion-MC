//! The browser's only way out: a DNS responder and an HTTPS relay in the launcher, on two
//! sockets created inside the browser's network namespace (`sandbox::netns`, as for the
//! game's filtered network).
//!
//! The namespace has a loopback interface and nothing else. What the browser can reach:
//!
//! | it asks for | condition | what happens |
//! |---|---|---|
//! | the address of a name | the name is one of the **open** site's hosts | a synthetic address from 127.64.0.0/10 (`egress::FakeIps`) |
//! | the address of any other name | — | NXDOMAIN, logged once per name; nothing is asked upstream |
//! | TCP to a synthetic address, port 443 | the name is still the open site's, the site's connection budget has a token, the TLS ClientHello names exactly that host (SNI) | relayed to the real host (public addresses only; through `network.launcher_proxy` if set), bytes counted against the site's byte budget |
//! | anything else | — | there is no route |
//!
//! TLS is end-to-end between the browser and the site: the relay sees which host a
//! connection is for, never what is said on it. When the phone closes a site or opens
//! another one, every connection of the previous one is cut.
//!
//! Budgets (per site, per minute, from `[browser] sites`): connections beyond
//! `connections_per_minute` are refused; bytes beyond `megabytes_per_minute` wait for the
//! bucket to refill, so a site that exceeds its rate is slowed down rather than broken.
//! Requests are counted where they are visible, in the browser's remote control (`driver`).

use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr},
    os::fd::OwnedFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Semaphore, watch},
    task::{JoinHandle, JoinSet},
    time::timeout,
};

use super::{budget::Bucket, config::Site};
use crate::{
    egress::{FakeIps, Logger, dns, is_public, sni},
    error::{Context, Result},
    util,
};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections open at once, all sites together.
const MAX_CONNECTIONS: usize = 192;
const CHUNK: usize = 16 * 1024;
/// Names whose refusal is logged (each once).
const MAX_LOGGED_NAMES: usize = 512;

/// Which site is open: `None`, or its index in the site list. The driver holds the sender.
pub type OpenSite = watch::Receiver<Option<usize>>;

/// Where relayed connections go.
#[derive(Debug, Clone)]
pub enum Upstream {
    /// The host itself, looked up here; only public addresses.
    Direct,
    /// `network.launcher_proxy`: the proxy is given the name.
    Socks5(SocketAddr),
    /// Tests: fixed addresses per name (private ones allowed). Not reachable from a
    /// configuration file.
    Fixed(Vec<(String, SocketAddr)>),
}

/// What one site has used, for the log and the tests.
#[derive(Debug, Default)]
pub struct SiteStats {
    pub connections: AtomicU64,
    pub refused: AtomicU64,
    /// Browser to site.
    pub sent: AtomicU64,
    /// Site to browser.
    pub received: AtomicU64,
}

/// A site as the relay keeps it: its hosts, its budgets, its counters.
pub struct SiteState {
    pub site: Site,
    connections: Bucket,
    bytes: Bucket,
    pub stats: SiteStats,
}

impl SiteState {
    pub fn new(site: Site) -> SiteState {
        SiteState {
            connections: Bucket::per_minute(u64::from(site.limits.connections_per_minute)),
            bytes: Bucket::per_minute(u64::from(site.limits.megabytes_per_minute) << 20),
            site,
            stats: SiteStats::default(),
        }
    }
}

struct Shared {
    sites: Vec<SiteState>,
    open: OpenSite,
    fake: Mutex<FakeIps>,
    refused_names: Mutex<HashSet<String>>,
    upstream: Upstream,
    log: Logger,
    slots: Arc<Semaphore>,
    names_refused: AtomicU64,
}

impl Shared {
    /// The open site, if `name` is one of its hosts.
    fn site_for(&self, name: &str) -> Option<usize> {
        let open = (*self.open.borrow())?;
        self.sites.get(open).filter(|s| s.site.allows(name)).map(|_| open)
    }
}

/// The running relay. Dropping it does not stop it; [`Relay::shutdown`] does.
pub struct Relay {
    tasks: Vec<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Relay {
    pub fn shutdown(self) {
        for task in self.tasks {
            task.abort();
        }
    }

    /// One line per site that was used: connections, refusals, bytes.
    pub fn summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for s in &self.shared.sites {
            let count = |n: &AtomicU64| n.load(Ordering::Relaxed);
            let (connections, refused) = (count(&s.stats.connections), count(&s.stats.refused));
            let (sent, received) = (count(&s.stats.sent), count(&s.stats.received));
            if connections + refused > 0 {
                lines.push(format!(
                    "browser relay, site {}: {connections} connection(s), {refused} refused, ↑{sent} ↓{received} bytes",
                    s.site.name
                ));
            }
        }
        let names = self.shared.names_refused.load(Ordering::Relaxed);
        if names > 0 {
            lines.push(format!("browser relay: {names} name lookup(s) refused (not a host of the open site)"));
        }
        lines
    }

    /// The counters of site `index`.
    pub fn stats(&self, index: usize) -> Option<&SiteStats> {
        self.shared.sites.get(index).map(|s| &s.stats)
    }
}

/// Serve the DNS socket (bound to 127.0.0.1:53 in the namespace) and the TCP listener
/// (0.0.0.0:443 there). Neither socket is read before this is called.
pub fn start(
    sites: Vec<SiteState>,
    open: OpenSite,
    dns_socket: OwnedFd,
    listener: OwnedFd,
    upstream: Upstream,
    log: Logger,
) -> Result<Relay> {
    let shared = Arc::new(Shared {
        sites,
        open,
        fake: Mutex::new(FakeIps::new()),
        refused_names: Mutex::new(HashSet::new()),
        upstream,
        log,
        slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        names_refused: AtomicU64::new(0),
    });
    let dns_socket = std::net::UdpSocket::from(dns_socket);
    dns_socket.set_nonblocking(true).ctx(|| "configuring the browser's DNS socket")?;
    let dns_socket = UdpSocket::from_std(dns_socket).ctx(|| "registering the browser's DNS socket")?;
    let listener = std::net::TcpListener::from(listener);
    listener.set_nonblocking(true).ctx(|| "configuring the browser's relay listener")?;
    let listener = TcpListener::from_std(listener).ctx(|| "registering the browser's relay listener")?;
    let tasks =
        vec![tokio::spawn(serve_dns(shared.clone(), dns_socket)), tokio::spawn(serve_tcp(shared.clone(), listener))];
    Ok(Relay { tasks, shared })
}

/// The answer to one DNS query from the browser.
fn answer(shared: &Shared, packet: &[u8]) -> Option<Vec<u8>> {
    match dns::parse_query(packet) {
        Ok(q) if shared.site_for(&q.name).is_some() => Some(match q.qtype {
            dns::TYPE_A => match shared.fake.lock().expect("fake ip lock").assign(&q.name) {
                Some(ip) => dns::answer_a(&q, ip),
                None => dns::answer_empty(&q, dns::Rcode::ServFail),
            },
            // AAAA, HTTPS (no ECH configuration: the ClientHello must name the host) and
            // everything else: the name exists, the record does not.
            _ => dns::answer_empty(&q, dns::Rcode::NoError),
        }),
        Ok(q) => {
            shared.names_refused.fetch_add(1, Ordering::Relaxed);
            let mut seen = shared.refused_names.lock().expect("refused lock");
            if seen.len() < MAX_LOGGED_NAMES && seen.insert(q.name.clone()) {
                (shared.log)(format!(
                    "browser relay: DENY  name lookup {:?} (not a host of the open site)",
                    util::sanitize_line(q.name.as_bytes(), 120)
                ));
            }
            Some(dns::answer_empty(&q, dns::Rcode::NxDomain))
        }
        Err((Some(id), rcode)) => Some(dns::answer_error(id, rcode)),
        Err((None, _)) => None,
    }
}

async fn serve_dns(shared: Arc<Shared>, socket: UdpSocket) {
    let mut buf = [0u8; 1024];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf).await else { continue };
        if let Some(reply) = answer(&shared, &buf[..n]) {
            let _ = socket.send_to(&reply, from).await;
        }
    }
}

async fn serve_tcp(shared: Arc<Shared>, listener: TcpListener) {
    let mut conns = JoinSet::new();
    loop {
        let Ok(permit) = shared.slots.clone().acquire_owned().await else { return };
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let shared = shared.clone();
                    conns.spawn(async move {
                        let _permit = permit;
                        handle(&shared, stream).await;
                    });
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
}

fn deny(shared: &Shared, site: Option<usize>, label: &str, why: &str) {
    if let Some(s) = site.and_then(|i| shared.sites.get(i)) {
        s.stats.refused.fetch_add(1, Ordering::Relaxed);
    }
    (shared.log)(format!("browser relay: DENY  {label} ({why})"));
}

async fn handle(shared: &Shared, mut inbound: TcpStream) {
    let Ok(SocketAddr::V4(local)) = inbound.local_addr() else { return };
    let ip = *local.ip();
    let name = if FakeIps::contains(ip) { shared.fake.lock().expect("fake ip lock").name_of(ip) } else { None };
    let Some(name) = name else {
        return deny(shared, None, &ip.to_string(), "address was never handed out by the resolver");
    };
    let Some(index) = shared.site_for(&name) else {
        return deny(shared, None, &name, "not a host of the open site");
    };
    let state = &shared.sites[index];
    if !state.connections.take(1) {
        return deny(shared, Some(index), &name, "the site's connection budget for this minute is used up");
    }
    match relay(shared, index, &mut inbound, &name).await {
        Ok(detail) => (shared.log)(format!("browser relay: done  {name} {detail}")),
        Err(why) => deny(shared, Some(index), &name, &why),
    }
}

/// Read the ClientHello and check that it names `name`. Returns the bytes read.
async fn client_hello(inbound: &mut TcpStream, name: &str) -> std::result::Result<Vec<u8>, String> {
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
        sni::Hello::Complete(Some(sni)) if sni == name => Ok(buf),
        sni::Hello::Complete(Some(other)) => {
            Err(format!("TLS SNI {:?} does not match the resolved name", util::sanitize_line(other.as_bytes(), 120)))
        }
        sni::Hello::Complete(None) => Err("TLS ClientHello without SNI".into()),
        _ => Err("not a TLS ClientHello".into()),
    }
}

async fn connect(upstream: &Upstream, name: &str) -> std::result::Result<(TcpStream, String), String> {
    match upstream {
        Upstream::Socks5(proxy) => {
            let stream = crate::net::http::socks5_dial(*proxy, name, 443).await?;
            Ok((stream, format!("the SOCKS5 proxy at {proxy}")))
        }
        Upstream::Fixed(table) => {
            let addr = table.iter().find(|(n, _)| n == name).map(|(_, a)| *a).ok_or("no test address for the name")?;
            let stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
                .await
                .map_err(|_| "connect timed out".to_string())?
                .map_err(|e| format!("cannot connect ({addr}: {e})"))?;
            Ok((stream, addr.to_string()))
        }
        Upstream::Direct => {
            let addrs: Vec<SocketAddr> = timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((name, 443)))
                .await
                .map_err(|_| "DNS timeout".to_string())?
                .map_err(|e| format!("DNS failure: {e}"))?
                .collect();
            let usable: Vec<SocketAddr> = addrs.iter().copied().filter(|a| is_public(a.ip())).collect();
            if usable.is_empty() {
                return Err(match addrs.first() {
                    Some(a) => format!("resolves only to non-public addresses ({})", a.ip()),
                    None => "no addresses".into(),
                });
            }
            let mut last = String::new();
            for addr in usable {
                match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
                    Ok(Ok(stream)) => return Ok((stream, addr.to_string())),
                    Ok(Err(e)) => last = format!("{addr}: {e}"),
                    Err(_) => last = format!("{addr}: connect timed out"),
                }
            }
            Err(format!("cannot connect ({last})"))
        }
    }
}

async fn relay(
    shared: &Shared,
    index: usize,
    inbound: &mut TcpStream,
    name: &str,
) -> std::result::Result<String, String> {
    let hello = client_hello(inbound, name).await?;
    let (mut outbound, via) = connect(&shared.upstream, name).await?;
    let _ = outbound.set_nodelay(true);
    let _ = inbound.set_nodelay(true);
    let state = &shared.sites[index];
    state.stats.connections.fetch_add(1, Ordering::Relaxed);
    (shared.log)(format!("browser relay: ALLOW {name} (site {}, SNI verified) → {via}", state.site.name));
    state.bytes.wait_for(hello.len() as u64).await;
    outbound.write_all(&hello).await.map_err(|e| format!("upstream write: {e}"))?;
    state.stats.sent.fetch_add(hello.len() as u64, Ordering::Relaxed);
    let mut open = shared.open.clone();
    let (mut from_browser, mut to_browser) = inbound.split();
    let (mut from_site, mut to_site) = outbound.split();
    let up = pump(&mut from_browser, &mut to_site, state, &state.stats.sent);
    let down = pump(&mut from_site, &mut to_browser, state, &state.stats.received);
    tokio::select! {
        _ = async { tokio::join!(up, down) } => Ok("closed".into()),
        _ = closed(&mut open, index) => Ok("cut: the site was closed on the phone".into()),
    }
}

/// Copy one direction until its reader ends, taking every chunk from the site's byte
/// budget before it is written.
async fn pump(
    from: &mut (impl AsyncRead + Unpin),
    to: &mut (impl AsyncWrite + Unpin),
    state: &SiteState,
    counter: &AtomicU64,
) {
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        state.bytes.wait_for(n as u64).await;
        if to.write_all(&buf[..n]).await.is_err() {
            break;
        }
        counter.fetch_add(n as u64, Ordering::Relaxed);
    }
    let _ = to.shutdown().await;
}

/// Resolves once site `index` is no longer the open one (or the relay is going away).
async fn closed(open: &mut OpenSite, index: usize) {
    loop {
        if *open.borrow_and_update() != Some(index) {
            return;
        }
        if open.changed().await.is_err() {
            return;
        }
    }
}

/// The synthetic address the relay has handed out for `name`, for tests.
#[doc(hidden)]
pub fn fake_address_of(relay: &Relay, name: &str) -> Option<Ipv4Addr> {
    relay.shared.fake.lock().expect("fake ip lock").assign(name)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;
    use crate::{
        browser::config::SiteEntry, egress::sni::tests::client_hello as hello_with, egress::sni::tests::sni_ext,
    };

    fn site(name: &str, hosts: &[&str], start: &str) -> Site {
        SiteEntry {
            name: Some(name.into()),
            start: Some(start.into()),
            hosts: Some(hosts.iter().map(|h| h.to_string()).collect()),
            connections_per_minute: Some(3),
            megabytes_per_minute: Some(1),
            ..Default::default()
        }
        .resolve()
        .unwrap()
    }

    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut v = vec![0xab, 0xcd, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            v.push(label.len() as u8);
            v.extend_from_slice(label.as_bytes());
        }
        v.extend_from_slice(&[0, (qtype >> 8) as u8, qtype as u8, 0, 1]);
        v
    }

    struct Fixture {
        relay: Relay,
        switch: watch::Sender<Option<usize>>,
        dns: std::net::UdpSocket,
        dns_addr: SocketAddr,
        port: u16,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Fixture {
        fn new(upstreams: Vec<(String, SocketAddr)>) -> Fixture {
            let sites = vec![
                SiteState::new(site("chat", &["chat.example", "*.cdn.example"], "https://chat.example/")),
                SiteState::new(site("wiki", &["wiki.example"], "https://wiki.example/")),
            ];
            let (switch, open) = watch::channel(None);
            let dns_server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            let dns_addr = dns_server.local_addr().unwrap();
            let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let log = Arc::new(Mutex::new(Vec::new()));
            let sink = log.clone();
            let relay = start(
                sites,
                open,
                dns_server.into(),
                listener.into(),
                Upstream::Fixed(upstreams),
                Arc::new(move |line| sink.lock().unwrap().push(line)),
            )
            .unwrap();
            let dns = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            dns.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            Fixture { relay, switch, dns, dns_addr, port, log }
        }

        /// The rcode and, for an A answer, the address.
        fn resolve(&self, name: &str, qtype: u16) -> (u8, Option<Ipv4Addr>) {
            self.dns.send_to(&query(name, qtype), self.dns_addr).unwrap();
            let mut buf = [0u8; 512];
            let n = self.dns.recv(&mut buf).unwrap();
            let rcode = buf[3] & 0x0f;
            let answers = u16::from_be_bytes([buf[6], buf[7]]);
            let ip = (answers == 1).then(|| Ipv4Addr::new(buf[n - 4], buf[n - 3], buf[n - 2], buf[n - 1]));
            (rcode, ip)
        }

        fn logged(&self, needle: &str) -> bool {
            self.log.lock().unwrap().iter().any(|l| l.contains(needle))
        }
    }

    /// A stand-in site: accepts one connection, reads what arrives until `want` bytes are
    /// there, answers `reply`, and reports what it read.
    fn stand_in(want: usize, reply: &'static [u8]) -> (SocketAddr, std::thread::JoinHandle<Vec<u8>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 8192];
            while got.len() < want {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            let _ = s.write_all(reply);
            let _ = s.shutdown(std::net::Shutdown::Write);
            let _ = s.read_to_end(&mut Vec::new());
            got
        });
        (addr, thread)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn names_resolve_only_for_the_open_site() {
        let f = Fixture::new(Vec::new());
        tokio::task::spawn_blocking(move || {
            assert_eq!(f.resolve("chat.example", dns::TYPE_A).0, 3, "nothing is open: NXDOMAIN");
            f.switch.send(Some(0)).unwrap();
            let (rcode, ip) = f.resolve("chat.example", dns::TYPE_A);
            assert_eq!(rcode, 0);
            assert!(FakeIps::contains(ip.unwrap()));
            assert_eq!(f.resolve("Media.CDN.example", dns::TYPE_A).0, 0, "a host below a wildcard");
            assert_eq!(f.resolve("cdn.example", dns::TYPE_A).0, 3, "the wildcard's own name is not listed");
            assert_eq!(f.resolve("chat.example", dns::TYPE_AAAA), (0, None), "no IPv6 records, but the name exists");
            assert_eq!(f.resolve("chat.example", 65), (0, None), "no HTTPS records: no ECH, the SNI stays visible");
            assert_eq!(f.resolve("wiki.example", dns::TYPE_A).0, 3, "another configured site that is not open");
            assert_eq!(f.resolve("tracker.example", dns::TYPE_A).0, 3);
            f.switch.send(Some(1)).unwrap();
            assert_eq!(f.resolve("wiki.example", dns::TYPE_A).0, 0);
            assert_eq!(f.resolve("chat.example", dns::TYPE_A).0, 3, "the previous site's hosts are gone");
            assert!(f.logged("DENY  name lookup \"tracker.example\""));
            assert_eq!(f.relay.shared.names_refused.load(Ordering::Relaxed), 5);
            f.relay.shutdown();
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_connection_is_relayed_only_with_the_matching_sni_and_within_the_budget() {
        let (addr, server) = stand_in(hello_with(&[sni_ext("chat.example")]).len(), b"hello from the site");
        let f = Fixture::new(vec![("chat.example".into(), addr)]);
        tokio::task::spawn_blocking(move || {
            f.switch.send(Some(0)).unwrap();
            let ip = f.resolve("chat.example", dns::TYPE_A).1.unwrap();
            // The right name: relayed, the hello forwarded first, the answer comes back.
            let mut hello = hello_with(&[sni_ext("chat.example")]);
            let mut c = std::net::TcpStream::connect((ip, f.port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            c.write_all(&hello).unwrap();
            let mut reply = Vec::new();
            c.read_to_end(&mut reply).unwrap();
            assert_eq!(reply, b"hello from the site");
            drop(c);
            let got = server.join().unwrap();
            assert_eq!(&got[..hello.len().min(got.len())], &hello[..hello.len().min(got.len())]);
            assert!(f.logged("ALLOW chat.example (site chat, SNI verified)"), "{:?}", f.log.lock().unwrap());

            // Another name in the hello (domain fronting): refused.
            let ip_cdn = f.resolve("img.cdn.example", dns::TYPE_A).1.unwrap();
            let mut c = std::net::TcpStream::connect((ip_cdn, f.port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            c.write_all(&hello).unwrap();
            assert_eq!(c.read(&mut [0u8; 16]).unwrap_or(0), 0, "closed");
            assert!(f.logged("does not match the resolved name"));

            // Not TLS at all.
            let mut c = std::net::TcpStream::connect((ip, f.port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            c.write_all(b"GET / HTTP/1.1\r\nHost: chat.example\r\n\r\n").unwrap();
            assert_eq!(c.read(&mut [0u8; 16]).unwrap_or(0), 0);
            assert!(f.logged("not a TLS ClientHello"));

            // Three connections a minute: the fourth is refused before anything is read.
            hello = hello_with(&[sni_ext("chat.example")]);
            let mut c = std::net::TcpStream::connect((ip, f.port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let _ = c.write_all(&hello);
            assert_eq!(c.read(&mut [0u8; 16]).unwrap_or(0), 0);
            assert!(f.logged("connection budget for this minute is used up"));

            // An address in the synthetic range that was never handed out.
            let mut c = std::net::TcpStream::connect((Ipv4Addr::new(127, 64, 200, 1), f.port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            assert_eq!(c.read(&mut [0u8; 16]).unwrap_or(0), 0);
            assert!(f.logged("never handed out"));
            let stats = f.relay.stats(0).unwrap();
            assert_eq!(stats.connections.load(Ordering::Relaxed), 1);
            assert_eq!(stats.refused.load(Ordering::Relaxed), 3);
            assert!(f.relay.summary().iter().any(|l| l.contains("site chat: 1 connection(s), 3 refused")));
            f.relay.shutdown();
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closing_the_site_cuts_its_connections_and_names() {
        // A site that never answers: the connection stays open until the phone closes it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let held = std::thread::spawn(move || listener.accept().map(|(s, _)| s));
        let f = Fixture::new(vec![("wiki.example".into(), addr)]);
        tokio::task::spawn_blocking(move || {
            f.switch.send(Some(1)).unwrap();
            let ip = f.resolve("wiki.example", dns::TYPE_A).1.unwrap();
            let mut c = std::net::TcpStream::connect((ip, f.port)).unwrap();
            c.write_all(&hello_with(&[sni_ext("wiki.example")])).unwrap();
            let _server_side = held.join().unwrap().unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            f.switch.send(None).unwrap();
            assert_eq!(c.read(&mut [0u8; 16]).unwrap_or(0), 0, "cut");
            for _ in 0..50 {
                if f.logged("cut: the site was closed on the phone") {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(f.logged("cut: the site was closed on the phone"), "{:?}", f.log.lock().unwrap());
            assert_eq!(f.resolve("wiki.example", dns::TYPE_A).0, 3);
            // A connection to the address it had, made after the close: refused.
            let mut c = std::net::TcpStream::connect((ip, f.port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            assert_eq!(c.read(&mut [0u8; 16]).unwrap_or(0), 0);
            assert!(f.logged("DENY  wiki.example (not a host of the open site)"));
            f.relay.shutdown();
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bytes_beyond_the_budget_wait_for_the_bucket() {
        // One megabyte a minute: 1 MiB passes at once, the next 64 KiB take about four seconds.
        let size = (1 << 20) + (64 << 10);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hello_len = hello_with(&[sni_ext("chat.example")]).len();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            // All of the hello first: a socket closed with unread data ends in a reset.
            let mut hello = vec![0u8; hello_len];
            s.read_exact(&mut hello).unwrap();
            let start = std::time::Instant::now();
            let _ = s.write_all(&vec![7u8; size]);
            let _ = s.shutdown(std::net::Shutdown::Write);
            let _ = s.read_to_end(&mut Vec::new());
            start
        });
        let f = Fixture::new(vec![("chat.example".into(), addr)]);
        tokio::task::spawn_blocking(move || {
            f.switch.send(Some(0)).unwrap();
            let ip = f.resolve("chat.example", dns::TYPE_A).1.unwrap();
            let mut c = std::net::TcpStream::connect((ip, f.port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            c.write_all(&hello_with(&[sni_ext("chat.example")])).unwrap();
            let mut got = Vec::new();
            c.read_to_end(&mut got).unwrap();
            let took_reading = std::time::Instant::now();
            drop(c);
            let started = server.join().unwrap();
            let took = took_reading.duration_since(started);
            assert!(got.len() == size, "{} bytes", got.len());
            assert!(took >= Duration::from_secs(2), "the budget was not applied: {took:?}");
            assert!(took < Duration::from_secs(20), "{took:?}");
            f.relay.shutdown();
        })
        .await
        .unwrap();
    }
}
