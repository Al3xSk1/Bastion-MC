//! The Bedrock service broker: TLS termination of the two services a Bedrock join comes from —
//! the Realms API and the gatherings service (event servers, "experiences") — to read each join
//! answer, hold it until the user approves it **outside the game**, and give the game the join
//! pass (`bedrock::PASS`) in place of the server's address.
//!
//! **Why termination.** A Realm's server and an event server are named by an IP literal and a
//! port inside an HTTPS reply. The game's network namespace has no route to that address, and a
//! relay that decides by name cannot learn it. So, when joins are brokered, the launcher
//! terminates the HTTPS of exactly four hosts (`bedrock::net::REALMS_HOSTS` and
//! `GATHERINGS_HOSTS`) with a certificate authority made for this launch — in the sandbox's
//! trust store and nowhere else, its name constraints limited to those four names — forwards
//! each request to the real service over the launcher's own TLS 1.3 client, and reads the
//! answers:
//!
//! | request | what happens |
//! |---|---|
//! | a join: Realms `GET /worlds/<id>/join`; gatherings `POST /api/v2.0/join/experience` or `…/join/world/client` (letters in any case) | asked of the service once; its answer is read strictly and held (`realms::approval`): the game is told to wait (503, `Retry-After`), the user is shown the server outside the game, and after the approval the next request for the same join is answered with the service's JSON, the address replaced by the pass |
//! | a join answer that names NetherNet (WebRTC) or a protocol this module does not know, a host name instead of an address, or a non-public address | refused, with the reason, to the game and in the log |
//! | the answer to any other request, if it carries a server address the way a join answer does | withheld (502): it came from an endpoint this module does not know as a join |
//! | everything else | forwarded as the game sent it (less hop-by-hop headers), answered as it came (headers from a fixed list) |
//!
//! **What it is not.** It holds no credential: the game's own token — an Xbox token for the
//! relying party `https://pocket.realms.minecraft.net/` at Realms, a Minecraft services token at
//! the gatherings service — passes through as the game sent it. Nor is it a policy point for
//! what the game does with Realms: the game holds the account's Xbox identity (research §5.2),
//! and other names under the allowed suffixes reach the same services unterminated. It reads
//! replies only to find joins; where a join may then go is decided by the datagram relay, which
//! sends to an approved server only through the pass.
//!
//! **Never logged:** header values, bodies, query strings. A line names the host, the method,
//! the path (sanitised), the status and what was done.
//!
//! **Bounds:** hyper's header limits (a 64 KiB buffer), request bodies of at most 64 KiB, replies
//! of at most 8 MiB, every body held out of one budget (as the credential broker's), the
//! credential broker's timeouts; join answers of at most 64 KiB and 32 levels of nesting.

use std::{
    collections::HashMap,
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::{
    HeaderMap, Method, Request, Response,
    body::{Body, Incoming},
    header::{self, HeaderName},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use rustls::{
    ServerConfig,
    server::{Acceptor, NoServerSessionStorage},
};
use serde_json::{Map, Value};
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::LazyConfigAcceptor;

use super::{
    Logger, Stats,
    bedrock::PASS,
    broker::{self, Authority, Budget, ReplyBody},
    udp::Refusal,
};
use crate::{
    bedrock::net::{GATHERINGS_HOSTS, REALMS_HOSTS},
    error::{Error, Result},
    net::{
        http::{Connection, Headers, HttpClient},
        tls,
    },
    realms::{
        approval::{Approvals, JoinStep},
        table::Address,
    },
    util,
};

/// The largest request body taken (joins and the other requests of both services are small).
const MAX_REQUEST_BODY: usize = 64 * 1024;
/// The largest join answer read, and its deepest nesting.
const MAX_JOIN_ANSWER: usize = 64 * 1024;
const MAX_DEPTH: usize = 32;
/// The largest gatherings join request read for its experience.
const MAX_JOIN_REQUEST: usize = 4096;
/// After the first flow through the pass, the same approval opens at most this many flows in all,
/// to the same server, for this long: a client may ping from one socket and connect from another.
const PASS_FLOWS: u32 = 4;
const PASS_GRACE: Duration = Duration::from_secs(30);
/// Names of Realms and event servers remembered for the log.
const MAX_NAMES: usize = 64;
/// Realm ids are below this; gatherings keys have the top bit set.
const EVENT_BIT: u64 = 1 << 63;

const WAIT_TEXT: &str = "this join waits for your approval outside the game: answer the prompt in the launcher's \
                         window or terminal, then join again if the game has stopped waiting";
const NO_ROOM: &str = "the launcher is already holding as much of this game's traffic as it will";

/// Request headers that belong to one connection, not to the request (RFC 9110 §7.6.1), and those
/// the launcher's client writes itself.
const NOT_FORWARDED: [&str; 13] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
    "accept-encoding",
    "expect",
];

// ---------------------------------------------------------------- what a request is

/// What a request to a terminated host is, for this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `GET /worlds/<id>/join` on a Realms host.
    RealmsJoin(u64),
    /// A join of the gatherings service; the experience is named in the body.
    GatheringsJoin,
    Other,
}

/// Classify a request by host, method and (canonical) path. The letters of a join path may be in
/// any case: a service that folds case must not reach a join this module does not see.
pub fn classify(host: &str, method: &Method, path: &str) -> Kind {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let is = |segment: &&str, word: &str| segment.eq_ignore_ascii_case(word);
    if REALMS_HOSTS.contains(&host)
        && *method == Method::GET
        && let [worlds, id, join] = segments.as_slice()
        && is(worlds, "worlds")
        && is(join, "join")
        && let Some(world) = realm_id(id)
    {
        return Kind::RealmsJoin(world);
    }
    if GATHERINGS_HOSTS.contains(&host) && *method == Method::POST {
        let joins = match segments.as_slice() {
            [api, version, join, experience] => {
                is(api, "api") && is(version, "v2.0") && is(join, "join") && is(experience, "experience")
            }
            [api, version, join, world, client] => {
                is(api, "api") && is(version, "v2.0") && is(join, "join") && is(world, "world") && is(client, "client")
            }
            _ => false,
        };
        if joins {
            return Kind::GatheringsJoin;
        }
    }
    Kind::Other
}

/// A Realm's id: decimal, below [`EVENT_BIT`].
fn realm_id(segment: &str) -> Option<u64> {
    (!segment.is_empty() && segment.len() <= 19 && segment.bytes().all(|b| b.is_ascii_digit()))
        .then(|| segment.parse::<u64>().ok())
        .flatten()
        .filter(|id| *id < EVENT_BIT)
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// No deeper than [`MAX_DEPTH`] levels of arrays and objects (counted outside strings).
fn depth_ok(body: &[u8]) -> bool {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for &b in body {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return false;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    true
}

/// A bounded JSON object, or why not.
fn object(body: &[u8], max: usize) -> std::result::Result<Map<String, Value>, &'static str> {
    if body.len() > max {
        return Err("too large");
    }
    if !depth_ok(body) {
        return Err("nested too deeply");
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err("not a JSON object"),
    }
}

/// The event server a gatherings join asks for, as the key its approval goes by: the experience
/// (and the world, when the request names one), hashed, with the top bit set — so that it can
/// never be a Realm's id. Returns the key and the experience id.
pub fn experience_key(body: &[u8]) -> std::result::Result<(u64, String), &'static str> {
    let request =
        object(body, MAX_JOIN_REQUEST).map_err(|_| "the join request is not a JSON object the launcher reads")?;
    let experience = request
        .get("experienceId")
        .and_then(Value::as_str)
        .filter(|id| is_uuid(id))
        .ok_or("the join request names no experience")?
        .to_ascii_lowercase();
    let world = match request.get("worldId") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(id)) if is_uuid(id) => id.to_ascii_lowercase(),
        Some(_) => return Err("the join request's world is not an id"),
    };
    // FNV-1a: a stable key, not a secret.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in experience.bytes().chain(std::iter::once(b':')).chain(world.bytes()) {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok((hash | EVENT_BIT, experience))
}

// ---------------------------------------------------------------- join answers

/// A join answer, read strictly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    /// Where the server is.
    pub server: SocketAddr,
    /// What to call it in the prompt (the gatherings service names the experience).
    pub name: Option<String>,
    /// The answer as a whole, for the rewrite.
    object: Map<String, Value>,
}

/// Why a join answer is not passed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    Malformed(&'static str),
    /// The server is reached over NetherNet (WebRTC); the protocol as given.
    NetherNet(String),
    /// A protocol this module does not know.
    UnknownProtocol(String),
    /// A host name where an address belongs.
    HostName,
}

impl JoinError {
    pub fn reason(&self) -> String {
        match self {
            JoinError::Malformed(why) => format!("the join answer is malformed ({why})"),
            JoinError::NetherNet(protocol) => format!(
                "this server is reached over NetherNet ({protocol}), WebRTC, which bastion-mc does not relay in this \
                 version"
            ),
            JoinError::UnknownProtocol(protocol) => {
                format!("the join answer names a network protocol bastion-mc does not know ({protocol})")
            }
            JoinError::HostName => "the join answer names a host, not an address; bastion-mc does not resolve a \
                                    join's server in this version"
                .into(),
        }
    }
}

/// The protocol of a join answer: RakNet (`DEFAULT`/`Default`, or no field at all — the answers
/// of older services), NetherNet, or something else. Compared without regard to case and
/// surrounding space, as gophertunnel's `ParseNetworkProtocol` does.
fn protocol(value: Option<&Value>) -> std::result::Result<(), JoinError> {
    let text = match value {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::String(text)) => text,
        Some(_) => return Err(JoinError::Malformed("the network protocol is not text")),
    };
    let shown = util::sanitize_line(text.as_bytes(), 40);
    match text.trim().to_ascii_uppercase().as_str() {
        "DEFAULT" => Ok(()),
        "NETHERNET" | "NETHERNET_JSONRPC" => Err(JoinError::NetherNet(shown)),
        _ => Err(JoinError::UnknownProtocol(shown)),
    }
}

/// `a.b.c.d:port` or `[v6]:port`, nothing else.
fn ip_port(text: &str) -> std::result::Result<SocketAddr, JoinError> {
    if text.len() > 64 {
        return Err(JoinError::Malformed("the address is too long"));
    }
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return if addr.port() == 0 { Err(JoinError::Malformed("port 0")) } else { Ok(addr) };
    }
    match text.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() && crate::config::normalize_host(host).is_ok() => {
            Err(JoinError::HostName)
        }
        _ => Err(JoinError::Malformed("the address is not an IP address and a port")),
    }
}

/// A Realms join answer: `{"address": "IP:port", "networkProtocol": "DEFAULT", …}`.
pub fn parse_realms_join(body: &[u8]) -> std::result::Result<Join, JoinError> {
    let object = object(body, MAX_JOIN_ANSWER).map_err(JoinError::Malformed)?;
    protocol(object.get("networkProtocol"))?;
    let address = object.get("address").and_then(Value::as_str).ok_or(JoinError::Malformed("no server address"))?;
    let server = ip_port(address)?;
    Ok(Join { server, name: None, object })
}

/// A gatherings join answer: `{"result": {"networkProtocol": "Default", "ipV4Address": "…",
/// "port": n, "destinationInfo": {"experienceName": …, …}}}`.
pub fn parse_gatherings_join(body: &[u8]) -> std::result::Result<Join, JoinError> {
    let object = object(body, MAX_JOIN_ANSWER).map_err(JoinError::Malformed)?;
    let result = object.get("result").and_then(Value::as_object).ok_or(JoinError::Malformed("no result"))?;
    protocol(result.get("networkProtocol"))?;
    let ip = result.get("ipV4Address").and_then(Value::as_str).ok_or(JoinError::Malformed("no server address"))?;
    let ip: Ipv4Addr = match ip.parse() {
        Ok(ip) => ip,
        Err(_) if crate::config::normalize_host(ip).is_ok() => return Err(JoinError::HostName),
        Err(_) => return Err(JoinError::Malformed("the address is not an IPv4 address")),
    };
    let port = result
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .ok_or(JoinError::Malformed("no usable port"))?;
    let info = result.get("destinationInfo").and_then(Value::as_object);
    let name = ["experienceName", "worldName"]
        .iter()
        .find_map(|key| info.and_then(|i| i.get(*key)).and_then(Value::as_str))
        .filter(|n| !n.trim().is_empty())
        .map(|n| util::sanitize_line(n.as_bytes(), 64));
    Ok(Join { server: SocketAddr::new(ip.into(), port), name, object })
}

/// The Realms answer the game is given: the same object, the address replaced by the pass.
pub fn rewrite_realms_join(join: &Join) -> Vec<u8> {
    let mut object = join.object.clone();
    object.insert("address".into(), Value::String(PASS.to_string()));
    Value::Object(object).to_string().into_bytes()
}

/// The gatherings answer the game is given: the same object, address and port those of the pass.
pub fn rewrite_gatherings_join(join: &Join) -> Vec<u8> {
    let mut object = join.object.clone();
    if let Some(Value::Object(result)) = object.get_mut("result") {
        result.insert("ipV4Address".into(), Value::String(PASS.ip().to_string()));
        result.insert("port".into(), Value::from(PASS.port()));
    }
    Value::Object(object).to_string().into_bytes()
}

/// Does an answer from `host` carry a server address the way a join answer does?
pub fn looks_like_join(host: &str, body: &[u8]) -> bool {
    let Ok(object) = object(body, MAX_JOIN_ANSWER) else { return false };
    if REALMS_HOSTS.contains(&host) {
        object.get("address").is_some_and(Value::is_string)
    } else {
        object.get("result").and_then(Value::as_object).is_some_and(|r| r.contains_key("ipV4Address"))
    }
}

// ---------------------------------------------------------------- the broker

/// The flows an approval may still open after its first.
struct Grant {
    target: SocketAddr,
    label: String,
    until: Instant,
    left: u32,
}

pub struct Services {
    authority: Authority,
    tls: Mutex<HashMap<String, Arc<ServerConfig>>>,
    http: HttpClient,
    approvals: Arc<Approvals>,
    buffers: Budget,
    /// Names of Realms and event servers seen in this session, for the log.
    names: Mutex<HashMap<u64, String>>,
    grant: Mutex<Option<Grant>>,
    loopback_for_tests: bool,
}

impl Services {
    /// `authority` must be one made for [`Services::domains`]; `http` is the launcher's own
    /// client (system roots, built-in pins, TLS 1.3), narrowed here to the four hosts.
    pub fn new(authority: Authority, http: &HttpClient, approvals: Arc<Approvals>) -> Result<Self> {
        if approvals.pass_address() != PASS {
            return Err(Error::Sandbox("the approvals of a Bedrock session must hand out the Bedrock pass".into()));
        }
        Ok(Services {
            authority,
            tls: Mutex::new(HashMap::new()),
            http: http.clone().only(&Self::domains()),
            approvals,
            buffers: Budget::standard(),
            names: Mutex::new(HashMap::new()),
            grant: Mutex::new(None),
            loopback_for_tests: false,
        })
    }

    /// The names whose HTTPS this broker terminates: what its authority's name constraints
    /// permit, and nothing else.
    pub fn domains() -> Vec<&'static str> {
        REALMS_HOSTS.iter().chain(GATHERINGS_HOSTS).copied().collect()
    }

    pub fn terminates(name: &str) -> bool {
        REALMS_HOSTS.contains(&name) || GATHERINGS_HOSTS.contains(&name)
    }

    /// Test support, never set by a launch: accept a server on the loopback address (the tests'
    /// stand-in) where a real one must be public.
    #[doc(hidden)]
    pub fn allowing_loopback_for_tests(mut self) -> Self {
        self.loopback_for_tests = true;
        self
    }

    pub fn approvals(&self) -> &Arc<Approvals> {
        &self.approvals
    }

    /// May a join lead to `ip`? Public addresses only (`egress::is_public`).
    pub fn permits(&self, ip: IpAddr) -> bool {
        super::is_public(ip) || (self.loopback_for_tests && ip.is_loopback())
    }

    fn remember_name(&self, key: u64, name: &str) {
        let mut names = self.names.lock().expect("services names");
        if names.len() < MAX_NAMES || names.contains_key(&key) {
            names.insert(key, util::sanitize_line(name.as_bytes(), 64));
        }
    }

    fn name_of(&self, key: u64) -> String {
        match self.names.lock().expect("services names").get(&key) {
            Some(name) if key & EVENT_BIT != 0 => format!("the event server {name:?}"),
            Some(name) => format!("the Realm {name:?}"),
            None if key & EVENT_BIT != 0 => "an event server".into(),
            None => format!("Realm {key}"),
        }
    }

    /// The datagram relay asks where a new flow to the pass goes: a fresh approval is spent
    /// (`Approvals::take_pass`), or the approval spent last opens one of its few further flows
    /// to the same server within [`PASS_GRACE`] — never anywhere else.
    pub(super) fn take_pass(&self) -> std::result::Result<(SocketAddr, String), Refusal> {
        let now = Instant::now();
        match self.approvals.take_pass() {
            Ok((address, key)) => {
                let ip: IpAddr =
                    address.host.parse().map_err(|_| Refusal::Pass("the approved server has no address"))?;
                if !self.permits(ip) {
                    return Err(Refusal::Pass("the approved server is not at a public address"));
                }
                let target = SocketAddr::new(ip, address.port);
                let label = format!("the approved join to {}", self.name_of(key));
                *self.grant.lock().expect("services grant") =
                    Some(Grant { target, label: label.clone(), until: now + PASS_GRACE, left: PASS_FLOWS - 1 });
                Ok((target, label))
            }
            Err(why) => match self.grant.lock().expect("services grant").as_mut() {
                Some(grant) if grant.left > 0 && now < grant.until => {
                    grant.left -= 1;
                    Ok((grant.target, format!("{} (another flow of the same join)", grant.label)))
                }
                _ => Err(Refusal::Pass(why)),
            },
        }
    }

    fn tls_config(&self, name: &str) -> std::result::Result<Arc<ServerConfig>, String> {
        if !Self::terminates(name) {
            return Err("not a host this broker terminates".into());
        }
        let mut cache = self.tls.lock().expect("services tls lock");
        if let Some(config) = cache.get(name) {
            return Ok(config.clone());
        }
        let (certificate, key) = self.authority.issue(name).map_err(|e| e.to_string())?;
        let mut config = ServerConfig::builder_with_provider(tls::provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .map_err(|e| e.to_string())?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        // No resumption state: every connection proves itself afresh, and nothing persists.
        config.session_storage = Arc::new(NoServerSessionStorage {});
        config.send_tls13_tickets = 0;
        let config = Arc::new(config);
        cache.insert(name.to_string(), config.clone());
        Ok(config)
    }

    /// Serve one connection from the game that was addressed to `name`: a TLS handshake with a
    /// certificate for exactly that name, then HTTP/1.1 requests until either side is done.
    /// Returns the number of requests handled.
    pub async fn serve(
        &self,
        log: &Logger,
        stats: &Stats,
        inbound: TcpStream,
        name: &str,
    ) -> std::result::Result<u64, String> {
        let start = timeout(broker::HANDSHAKE_TIMEOUT, LazyConfigAcceptor::new(Acceptor::default(), inbound))
            .await
            .map_err(|_| "no TLS ClientHello received".to_string())?
            .map_err(|e| format!("not a TLS ClientHello ({e})"))?;
        match start.client_hello().server_name() {
            Some(sni) if sni.eq_ignore_ascii_case(name) => {}
            Some(other) => {
                return Err(format!(
                    "TLS SNI {:?} does not match the resolved name",
                    util::sanitize_line(other.as_bytes(), 120)
                ));
            }
            None => return Err("TLS ClientHello without SNI".into()),
        }
        let config = self.tls_config(name)?;
        let stream = timeout(broker::HANDSHAKE_TIMEOUT, start.into_stream(config))
            .await
            .map_err(|_| "TLS handshake with the game timed out".to_string())?
            .map_err(|e| format!("TLS handshake with the game failed ({e})"))?;
        let upstream = tokio::sync::Mutex::new(None);
        let requests = std::sync::atomic::AtomicU64::new(0);
        let service = service_fn(|request| {
            requests.fetch_add(1, Ordering::Relaxed);
            let upstream = &upstream;
            async move { Ok::<_, Infallible>(self.handle(log, stats, upstream, name, request).await) }
        });
        let connection = http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(broker::REQUEST_IDLE)
            .max_buf_size(64 * 1024)
            .keep_alive(true)
            .serve_connection(TokioIo::new(stream), service);
        // Errors here are the game hanging up mid-request or sending garbage; both end it.
        let _ = timeout(broker::CONNECTION_LIFETIME, connection).await;
        Ok(requests.load(Ordering::Relaxed))
    }

    async fn handle(
        &self,
        log: &Logger,
        stats: &Stats,
        upstream: &tokio::sync::Mutex<Option<Connection>>,
        host: &str,
        request: Request<Incoming>,
    ) -> Response<ReplyBody> {
        let (parts, body) = request.into_parts();
        let method = parts.method.clone();
        let path = parts.uri.path().to_string();
        let shown = util::sanitize_line(path.as_bytes(), 160);
        let say = |outcome: &str| log(format!("broker: {host} {method} {shown} → {outcome}"));
        let deny = |status: u16, reason: &str, security: bool| {
            stats.broker_refused.fetch_add(1, Ordering::Relaxed);
            say(&format!("{status} here{}: {reason}", if security { " [SECURITY]" } else { "" }));
            error_reply(status, reason, None)
        };

        // Only origin-form targets, and only for the host this connection was opened to.
        if parts.uri.scheme().is_some() || parts.uri.authority().is_some() {
            return deny(400, "absolute request targets are not relayed", false);
        }
        if let Some(named) = parts.headers.get(header::HOST) {
            let named = named.to_str().unwrap_or_default().to_ascii_lowercase();
            if named != host && named != format!("{host}:443") {
                return deny(421, "the Host header names a different server than the connection", true);
            }
        }
        if ![Method::GET, Method::POST, Method::PUT, Method::DELETE].contains(&method) {
            return deny(405, &format!("method {method} is not relayed"), false);
        }
        if !broker::path_is_canonical(&path) {
            return deny(400, "the request path is not in canonical form", false);
        }
        let query = parts.uri.query();
        if query.is_some_and(|q| !broker::query_is_plain(q)) {
            return deny(400, "the query string contains characters that are not relayed", false);
        }

        // The body, out of the budget like everything held here.
        let declared = body.size_hint().upper().unwrap_or(MAX_REQUEST_BODY as u64);
        if declared > MAX_REQUEST_BODY as u64 {
            return deny(413, "the request body is too large", false);
        }
        let request_room = match declared {
            0 => None,
            bytes => match self.buffers.reserve(bytes as usize).await {
                Some(room) => Some(room),
                None => return deny(503, NO_ROOM, false),
            },
        };
        let body = match Limited::new(body, MAX_REQUEST_BODY).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return deny(413, "the request body is too large or was cut short", false),
        };

        // A join is answered from the approvals when they have something to say.
        let kind = classify(host, &method, &path);
        let key = match kind {
            Kind::RealmsJoin(world) => Some(world),
            Kind::GatheringsJoin => match experience_key(&body) {
                Ok((key, _)) => Some(key),
                Err(why) => return deny(400, why, false),
            },
            Kind::Other => None,
        };
        if let Some(key) = key {
            match self.approvals.join_requested(key) {
                JoinStep::Forward => {}
                JoinStep::Wait(seconds) => {
                    say("503 here: waiting for your approval outside the game");
                    return error_reply(503, WAIT_TEXT, Some(seconds));
                }
                JoinStep::Deliver(answer) => {
                    say("200 here: approved — the game is given the join pass");
                    return json_reply(answer);
                }
                JoinStep::Refuse(reason) => return deny(403, &reason, false),
            }
        }

        let headers = forwardable(&parts.headers);
        let target = match query {
            Some(q) => format!("{path}?{q}"),
            None => path.clone(),
        };
        let body = (!body.is_empty() || method != Method::GET).then_some(body);
        let Some(mut reply_room) = self.buffers.reserve(broker::MAX_RESPONSE_BODY as usize).await else {
            return deny(503, NO_ROOM, false);
        };
        let (head, mut payload) = match self.forward(upstream, host, method.clone(), &target, &headers, body).await {
            Ok(reply) => reply,
            Err(e) => {
                stats.broker_refused.fetch_add(1, Ordering::Relaxed);
                // The client names the request target in its errors; the query is not logged.
                let mut why = e.to_string();
                if let Some(q) = query {
                    why = why.replace(&format!("?{q}"), "?…");
                }
                say(&format!("502 (upstream: {})", util::sanitize_line(why.as_bytes(), 200)));
                return error_reply(502, "the request could not be relayed to the service", None);
            }
        };
        drop(request_room);
        payload.shrink_to_fit();
        reply_room.shrink_to(payload.len());
        stats.brokered.fetch_add(1, Ordering::Relaxed);

        // Any success answers a join (Realms says 200; nothing else is let through unread). A join
        // that did not succeed (503 while a Realm starts, 403, …) is passed on: the game asks again.
        let success = (200..300).contains(&head.status);
        if let (Some(key), true) = (key, success) {
            return self.hold(key, kind, &head, &payload, &say, &deny);
        }
        // Whatever else came back, an address in the shape of a join answer does not reach the
        // game unless it came through a join (an encoded reply is passed on unread: what it could
        // name has no route from the sandbox anyway).
        if looks_like_join(host, &payload) {
            return deny(
                502,
                "an answer naming a server address came from a request the launcher does not know as a join; it was \
                 withheld",
                true,
            );
        }
        if head.status == 200 && method == Method::GET && REALMS_HOSTS.contains(&host) && path == "/worlds" {
            self.approvals.note_names(&payload);
            self.note_realm_names(&payload);
        }
        say(&head.status.to_string());
        let mut response = Response::builder().status(head.status);
        for name in &broker::RESPONSE_HEADERS {
            if let Some(value) = head.headers.get(name) {
                response = response.header(name, value);
            }
        }
        response
            .body(ReplyBody::held(Bytes::from(payload), reply_room))
            .unwrap_or_else(|_| error_reply(502, "malformed upstream reply", None))
    }

    /// The service answered a join: read it, check it, and hold it for the approval.
    fn hold(
        &self,
        key: u64,
        kind: Kind,
        head: &crate::net::http::Head,
        payload: &[u8],
        say: &dyn Fn(&str),
        deny: &dyn Fn(u16, &str, bool) -> Response<ReplyBody>,
    ) -> Response<ReplyBody> {
        let encoding = head.headers.get(header::CONTENT_ENCODING).map(|v| v.as_bytes().trim_ascii());
        if encoding.is_some_and(|e| !e.is_empty() && !e.eq_ignore_ascii_case(b"identity")) {
            return deny(502, "the join answer came back encoded; it cannot be read and was withheld", false);
        }
        let parsed = match kind {
            Kind::RealmsJoin(_) => parse_realms_join(payload),
            _ => parse_gatherings_join(payload),
        };
        let join = match parsed {
            Ok(join) => join,
            Err(why @ JoinError::NetherNet(_)) => return deny(403, &why.reason(), false),
            Err(why) => return deny(502, &why.reason(), false),
        };
        if !self.permits(join.server.ip()) {
            return deny(502, &format!("the service named a server at a non-public address ({})", join.server), true);
        }
        if let Some(name) = &join.name {
            self.approvals.note_name(key, name);
            self.remember_name(key, name);
        }
        say(&format!("200 from the service; server {} held back until you approve outside the game", join.server));
        let answer = match kind {
            Kind::RealmsJoin(_) => rewrite_realms_join(&join),
            _ => rewrite_gatherings_join(&join),
        };
        let address = Address { host: join.server.ip().to_string(), port: join.server.port() };
        match self.approvals.prompt(key, address, answer) {
            JoinStep::Wait(seconds) => error_reply(503, WAIT_TEXT, Some(seconds)),
            JoinStep::Refuse(reason) => deny(403, &reason, false),
            _ => deny(500, "internal error", false),
        }
    }

    /// Realm names from a Realms list, for the log (the approvals keep their own for the prompt).
    fn note_realm_names(&self, list: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(list) else { return };
        let Some(servers) = value.get("servers").and_then(Value::as_array) else { return };
        for server in servers.iter().take(MAX_NAMES) {
            if let (Some(id), Some(name)) = (
                server.get("id").and_then(Value::as_u64).filter(|id| *id < EVENT_BIT),
                server.get("name").and_then(Value::as_str),
            ) {
                self.remember_name(id, name);
            }
        }
    }

    /// One request to the real service, on a connection kept open between requests of the same
    /// inbound connection.
    async fn forward(
        &self,
        upstream: &tokio::sync::Mutex<Option<Connection>>,
        host: &str,
        method: Method,
        target: &str,
        headers: &Headers,
        body: Option<Bytes>,
    ) -> Result<(crate::net::http::Head, Vec<u8>)> {
        let mut slot = upstream.lock().await;
        if slot.as_ref().is_some_and(Connection::is_closed) {
            *slot = None;
        }
        let connection = match slot.as_mut() {
            Some(connection) => connection,
            None => slot.insert(self.http.connect(host).await?),
        };
        let mut payload = Vec::new();
        match connection.call(method, target, headers, body, broker::MAX_RESPONSE_BODY, &mut payload).await {
            Ok(head) => Ok((head, payload)),
            Err(e) => {
                *slot = None;
                Err(e)
            }
        }
    }
}

/// The game's request headers, less those of the connection (and those named by its
/// `Connection` header) and those the launcher's client writes itself. Credentials are marked
/// sensitive so that nothing prints them.
fn forwardable(headers: &HeaderMap) -> Headers {
    let named_by_connection: Vec<String> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();
    headers
        .iter()
        .filter(|(name, _)| {
            !NOT_FORWARDED.contains(&name.as_str()) && !named_by_connection.iter().any(|n| n == name.as_str())
        })
        .map(|(name, value)| {
            let mut value = value.clone();
            if *name == header::AUTHORIZATION || *name == header::COOKIE {
                value.set_sensitive(true);
            }
            (HeaderName::clone(name), value)
        })
        .collect()
}

/// An answer of this module's own, in the shapes both services' clients read (`errorMsg`,
/// `message`).
fn error_reply(status: u16, message: &str, retry_after: Option<u64>) -> Response<ReplyBody> {
    let text = format!("bastion-mc: {message}");
    let body = serde_json::json!({
        "errorCode": status,
        "errorMsg": text,
        "code": "bastion-mc",
        "message": text,
        "error": "bastion-mc",
        "errorMessage": text,
    })
    .to_string();
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(seconds) = retry_after {
        builder = builder.header(header::RETRY_AFTER, seconds.to_string());
    }
    builder.body(ReplyBody::local(Bytes::from(body))).expect("static response")
}

fn json_reply(answer: Vec<u8>) -> Response<ReplyBody> {
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(ReplyBody::local(Bytes::from(answer)))
        .unwrap_or_else(|_| error_reply(500, "internal error", None))
}

// ---------------------------------------------------------------- generated-input harnesses

/// For `fuzzing::bedrock_realms`: Realms join answers and the request classifier on arbitrary
/// bytes. Whatever they are, an answer that is accepted names an IP address and a port and a
/// RakNet server, and the answer handed to the game names the pass and nothing else of a
/// server; a join is recognised only on a Realms host, as a GET of three segments.
#[doc(hidden)]
pub fn fuzz_realms(data: &[u8]) {
    if let Ok(join) = parse_realms_join(data) {
        assert_ne!(join.server.port(), 0);
        let answer = rewrite_realms_join(&join);
        let again = parse_realms_join(&answer).expect("the rewritten answer is a join answer");
        assert_eq!(again.server, SocketAddr::V4(PASS), "the game is given the pass and nothing else");
        assert!(looks_like_join(REALMS_HOSTS[0], &answer));
    }
    let text = String::from_utf8_lossy(data);
    for host in REALMS_HOSTS.iter().chain(GATHERINGS_HOSTS).chain(["api.minecraftservices.com"].iter()) {
        for method in [Method::GET, Method::POST, Method::PUT] {
            match classify(host, &method, &text) {
                Kind::RealmsJoin(world) => {
                    assert!(REALMS_HOSTS.contains(host) && method == Method::GET && world < EVENT_BIT);
                    assert_eq!(text.trim_start_matches('/').split('/').count(), 3);
                }
                Kind::GatheringsJoin => assert!(GATHERINGS_HOSTS.contains(host) && method == Method::POST),
                Kind::Other => {}
            }
        }
    }
    let _ = looks_like_join(REALMS_HOSTS[1], data);
}

/// For `fuzzing::bedrock_gatherings`: gatherings join answers and join requests on arbitrary
/// bytes. An accepted answer names an IPv4 address and a port, its rewrite names the pass, a
/// name shown in a prompt is bounded and printable, and an accepted request is keyed outside the
/// range of Realm ids by a well-formed experience id.
#[doc(hidden)]
pub fn fuzz_gatherings(data: &[u8]) {
    if let Ok(join) = parse_gatherings_join(data) {
        assert!(join.server.is_ipv4() && join.server.port() != 0);
        if let Some(name) = &join.name {
            assert!(name.chars().count() <= 64 + 16 && !name.chars().any(char::is_control), "{name:?}");
        }
        let answer = rewrite_gatherings_join(&join);
        let again = parse_gatherings_join(&answer).expect("the rewritten answer is a join answer");
        assert_eq!(again.server, SocketAddr::V4(PASS));
        assert!(looks_like_join(GATHERINGS_HOSTS[0], &answer));
    }
    if let Ok((key, experience)) = experience_key(data) {
        assert!(key & EVENT_BIT != 0 && is_uuid(&experience));
        assert_eq!(experience, experience.to_ascii_lowercase());
    }
    let _ = looks_like_join(GATHERINGS_HOSTS[1], data);
    let _ = depth_ok(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    const REALMS: &str = "bedrock.frontendlegacy.realms.minecraft-services.net";
    const GATHERINGS: &str = "gatherings-secondary.franchise.minecraft-services.net";

    #[test]
    fn joins_are_recognised_on_their_hosts_in_any_spelling_of_the_letters_and_nowhere_else() {
        assert_eq!(classify(REALMS, &Method::GET, "/worlds/12345/join"), Kind::RealmsJoin(12345));
        assert_eq!(classify("pocket.realms.minecraft.net", &Method::GET, "/Worlds/7/JOIN"), Kind::RealmsJoin(7));
        for (host, method, path) in [
            (REALMS, Method::POST, "/worlds/1/join"),   // not how a join is asked
            (REALMS, Method::GET, "/worlds/1/join/pc"), // the Java path
            (REALMS, Method::GET, "/worlds/x/join"),    // not an id
            (REALMS, Method::GET, "/worlds/9223372036854775808/join"), // 2^63: an event key's range
            (REALMS, Method::GET, "/worlds"),
            (GATHERINGS, Method::GET, "/worlds/1/join"), // another service
            ("api.minecraftservices.com", Method::GET, "/worlds/1/join"),
        ] {
            assert_eq!(classify(host, &method, path), Kind::Other, "{host} {method} {path}");
        }
        for path in ["/api/v2.0/join/experience", "/API/V2.0/Join/Experience", "/api/v2.0/join/world/client"] {
            assert_eq!(classify(GATHERINGS, &Method::POST, path), Kind::GatheringsJoin, "{path}");
            assert_eq!(
                classify("gatherings.franchise.minecraft-services.net", &Method::POST, path),
                Kind::GatheringsJoin
            );
            assert_eq!(classify(GATHERINGS, &Method::GET, path), Kind::Other);
        }
        assert_eq!(classify(GATHERINGS, &Method::POST, "/api/v2.0/discovery/blob/client"), Kind::Other);
    }

    #[test]
    fn a_realms_join_answer_is_read_strictly_and_rewritten_to_the_pass() {
        let body = br#"{"address":"203.0.113.9:19132","networkProtocol":"DEFAULT","pendingUpdate":false,"sessionRegionData":{"regionName":"EastUS","serviceQuality":1}}"#;
        let join = parse_realms_join(body).unwrap();
        assert_eq!(join.server, "203.0.113.9:19132".parse().unwrap());
        let answer: Value = serde_json::from_slice(&rewrite_realms_join(&join)).unwrap();
        assert_eq!(answer["address"], "127.127.255.253:19132");
        assert_eq!(answer["sessionRegionData"]["regionName"], "EastUS", "everything else stays");
        assert_eq!(answer["networkProtocol"], "DEFAULT");
        // No protocol field (older answers) is RakNet; IPv6 literals are read too.
        assert!(parse_realms_join(br#"{"address":"203.0.113.9:30000"}"#).is_ok());
        assert_eq!(parse_realms_join(br#"{"address":"[2001:db8::9]:19133"}"#).unwrap().server.port(), 19133);
        // NetherNet, in any case, is refused with its name; unknown protocols too.
        for protocol in ["NETHERNET", "NETHERNET_JSONRPC", "NetherNet", " nethernet "] {
            let body = format!(r#"{{"address":"1234567890","networkProtocol":"{protocol}"}}"#);
            assert!(matches!(parse_realms_join(body.as_bytes()), Err(JoinError::NetherNet(_))), "{protocol}");
        }
        assert!(matches!(
            parse_realms_join(br#"{"address":"203.0.113.9:1","networkProtocol":"QUIC"}"#),
            Err(JoinError::UnknownProtocol(p)) if p == "QUIC"
        ));
        assert_eq!(parse_realms_join(br#"{"address":"realm.example.org:19132"}"#), Err(JoinError::HostName));
        for bad in [
            &br#"{"address":"203.0.113.9"}"#[..], // no port
            br#"{"address":"203.0.113.9:0"}"#,
            br#"{"address":"203.0.113.9:70000"}"#,
            br#"{"address":19132}"#,
            br#"{"networkProtocol":"DEFAULT"}"#,
            br#"{"address":"203.0.113.9:1","networkProtocol":7}"#,
            br#"["203.0.113.9:1"]"#,
            b"",
            b"{",
        ] {
            assert!(matches!(parse_realms_join(bad), Err(JoinError::Malformed(_))), "{}", String::from_utf8_lossy(bad));
        }
        // Bounded in size and depth.
        let deep = format!(r#"{{"address":"203.0.113.9:1","x":{}{}}}"#, "[".repeat(40), "]".repeat(40));
        assert!(matches!(parse_realms_join(deep.as_bytes()), Err(JoinError::Malformed("nested too deeply"))));
        let big = format!(r#"{{"address":"203.0.113.9:1","x":"{}"}}"#, "a".repeat(MAX_JOIN_ANSWER));
        assert!(matches!(parse_realms_join(big.as_bytes()), Err(JoinError::Malformed("too large"))));
        // Brackets inside strings do not count as nesting.
        let quoted = format!(r#"{{"address":"203.0.113.9:1","x":"{}"}}"#, "[".repeat(100));
        assert!(parse_realms_join(quoted.as_bytes()).is_ok());
    }

    #[test]
    fn a_gatherings_join_answer_is_read_strictly_and_rewritten_to_the_pass() {
        let body = br#"{"result":{"networkProtocol":"Default","ipV4Address":"198.51.100.20","port":19132,"destinationInfo":{"creatorId":"1","experienceId":"6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44","experienceName":"Minecraft Live","worldName":"Lobby"}}}"#;
        let join = parse_gatherings_join(body).unwrap();
        assert_eq!(join.server, "198.51.100.20:19132".parse().unwrap());
        assert_eq!(join.name.as_deref(), Some("Minecraft Live"));
        let answer: Value = serde_json::from_slice(&rewrite_gatherings_join(&join)).unwrap();
        assert_eq!(answer["result"]["ipV4Address"], "127.127.255.253");
        assert_eq!(answer["result"]["port"], 19132);
        assert_eq!(answer["result"]["destinationInfo"]["worldName"], "Lobby");
        for protocol in ["NetherNet", "NetherNet_JsonRpc"] {
            let body = format!(r#"{{"result":{{"networkProtocol":"{protocol}","ipV4Address":"","port":0}}}}"#);
            assert!(matches!(parse_gatherings_join(body.as_bytes()), Err(JoinError::NetherNet(_))), "{protocol}");
        }
        assert_eq!(
            parse_gatherings_join(br#"{"result":{"ipV4Address":"event.example.org","port":19132}}"#),
            Err(JoinError::HostName)
        );
        for bad in [
            &br#"{"result":{"ipV4Address":"198.51.100.20"}}"#[..],
            br#"{"result":{"ipV4Address":"198.51.100.20","port":65536}}"#,
            br#"{"result":{"ipV4Address":"198.51.100.20","port":"19132"}}"#,
            br#"{"result":{"ipV4Address":"2001:db8::1","port":19132}}"#,
            br#"{"result":[]}"#,
            br#"{"data":{"ipV4Address":"198.51.100.20","port":19132}}"#,
        ] {
            assert!(
                matches!(parse_gatherings_join(bad), Err(JoinError::Malformed(_))),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        // A name with control characters is shown sanitised.
        let odd = br#"{"result":{"ipV4Address":"198.51.100.20","port":1,"destinationInfo":{"experienceName":"Evil\u001b[2Jname"}}}"#;
        assert!(!parse_gatherings_join(odd).unwrap().name.unwrap().contains('\u{1b}'));
    }

    #[test]
    fn an_experience_is_keyed_outside_the_range_of_realm_ids() {
        let a = br#"{"experienceId":"6B3D2A5E-0C1F-4A7E-9A52-2F3C9D0E1B44"}"#;
        let (key, experience) = experience_key(a).unwrap();
        assert!(key & EVENT_BIT != 0);
        assert_eq!(experience, "6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44");
        assert_eq!(experience_key(br#"{"experienceId":"6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44"}"#).unwrap().0, key);
        let with_world =
            br#"{"experienceId":"6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44","worldId":"00000000-0000-0000-0000-000000000001"}"#;
        assert_ne!(experience_key(with_world).unwrap().0, key, "another world is another join");
        for bad in [
            &br#"{}"#[..],
            br#"{"experienceId":"not-a-uuid"}"#,
            br#"{"experienceId":42}"#,
            br#"{"experienceId":"6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44","worldId":7}"#,
            b"[]",
        ] {
            assert!(experience_key(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn answers_that_look_like_joins_are_recognised_by_their_shape() {
        assert!(looks_like_join(REALMS, br#"{"address":"203.0.113.9:1"}"#));
        assert!(!looks_like_join(REALMS, br#"{"servers":[{"id":1,"name":"x"}]}"#));
        assert!(looks_like_join(GATHERINGS, br#"{"result":{"ipV4Address":"203.0.113.9","port":1}}"#));
        assert!(!looks_like_join(GATHERINGS, br#"{"result":{"items":[]}}"#));
        assert!(!looks_like_join(REALMS, b"not json"));
    }

    #[test]
    fn only_end_to_end_headers_are_forwarded_and_credentials_are_marked_sensitive() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("authorization", "XBL3.0 x=1;token"),
            ("client-version", "1.26.62"),
            ("user-agent", "MCPE/Android"),
            ("connection", "keep-alive, x-private"),
            ("x-private", "1"),
            ("host", "elsewhere"),
            ("accept-encoding", "gzip"),
            ("transfer-encoding", "chunked"),
            ("content-length", "5"),
        ] {
            headers.append(HeaderName::from_static(name), value.parse().unwrap());
        }
        let out = forwardable(&headers);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["authorization", "client-version", "user-agent"]);
        assert!(out[0].1.is_sensitive());
    }

    /// A broker whose prompts' codes land in the returned slot.
    fn services() -> (Services, Arc<Mutex<Option<String>>>) {
        let http = HttpClient::new(
            tls::client_config(tls::load_system_roots().unwrap(), tls::PinSet::default()).unwrap(),
            &[],
        );
        let code: Arc<Mutex<Option<String>>> = Arc::default();
        let sink = code.clone();
        let announce = Arc::new(move |a: &crate::realms::approval::Announcement| {
            if a.code.is_some() {
                *sink.lock().unwrap() = a.code.clone();
            }
        });
        let approvals = Approvals::new(crate::realms::Approve::Code, None, PASS, announce);
        let authority = Authority::generate_constrained("Bedrock service broker", &Services::domains()).unwrap();
        (Services::new(authority, &http, approvals).unwrap(), code)
    }

    #[test]
    fn the_authority_signs_for_the_four_hosts_and_its_constraints_void_anything_else() {
        use rustls::{
            client::danger::ServerCertVerifier,
            pki_types::{CertificateDer, ServerName, UnixTime},
        };
        let authority = Authority::generate_constrained("Bedrock service broker", &Services::domains()).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(authority.certificate_der().to_vec())).unwrap();
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), tls::provider())
            .build()
            .unwrap();
        let check = |cert: &CertificateDer<'_>, name: &str| {
            let name = ServerName::try_from(name.to_string()).unwrap();
            verifier.verify_server_cert(cert, &[], &name, &[], UnixTime::now())
        };
        for host in Services::domains() {
            let (leaf, _) = authority.issue(host).unwrap();
            assert!(check(&leaf, host).is_ok(), "{host}");
        }
        // A leaf the authority signed for any other name is void (its name constraints), even
        // a parent of a permitted host or a sibling under the same suffix.
        for other in [
            "login.live.com",
            "api.minecraftservices.com",
            "client.discovery.minecraft-services.net",
            "realms.minecraft-services.net",
            "java.frontendlegacy.realms.minecraft-services.net",
        ] {
            let (stray, _) = authority.issue(other).unwrap();
            assert!(check(&stray, other).is_err(), "{other}");
        }
        // The PEM is the same certificate.
        let pem = authority.certificate_pem();
        let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        assert_eq!(util::b64_decode(&body).unwrap(), authority.certificate_der());
    }

    #[test]
    fn an_approval_opens_a_few_flows_to_its_server_and_nothing_after() {
        let (services, code) = services();
        assert_eq!(services.take_pass().err(), Some(Refusal::Pass("no Realms join was approved")));
        let approvals = services.approvals().clone();
        assert_eq!(approvals.join_requested(7), JoinStep::Forward);
        approvals.prompt(7, Address { host: "203.0.113.9".into(), port: 19132 }, b"{}".to_vec());
        assert_eq!(services.take_pass().err(), Some(Refusal::Pass("no Realms join was approved")), "not before");
        let code = code.lock().unwrap().clone().expect("a code was shown");
        assert_eq!(approvals.answer(&code), crate::realms::approval::Answer::Approved);
        assert!(matches!(approvals.join_requested(7), JoinStep::Deliver(_)), "the game is given the pass");
        let target: SocketAddr = "203.0.113.9:19132".parse().unwrap();
        for flow in 0..PASS_FLOWS {
            let (to, label) = services.take_pass().unwrap_or_else(|e| panic!("flow {flow}: {e:?}"));
            assert_eq!(to, target);
            assert_eq!(label.contains("another flow"), flow > 0, "{label}");
        }
        assert!(matches!(services.take_pass(), Err(Refusal::Pass(_))), "the approval's flows are used up");
        // A loopback server is permitted only to the tests' stand-ins.
        assert!(!services.permits("127.0.0.1".parse().unwrap()));
        assert!(services.allowing_loopback_for_tests().permits("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn the_broker_hands_out_only_the_bedrock_pass() {
        let http = HttpClient::new(
            tls::client_config(tls::load_system_roots().unwrap(), tls::PinSet::default()).unwrap(),
            &[],
        );
        let java = Approvals::new(crate::realms::Approve::Code, None, crate::realms::PASS_ADDRESS, Arc::new(|_| {}));
        let authority = Authority::generate_constrained("x", &Services::domains()).unwrap();
        assert!(Services::new(authority, &http, java).is_err());
    }

    #[test]
    fn the_harnesses_hold_on_their_starting_points() {
        for seed in [
            &br#"{"address":"203.0.113.9:19132","networkProtocol":"DEFAULT"}"#[..],
            br#"{"address":"1","networkProtocol":"NETHERNET"}"#,
            b"/worlds/7/join",
            b"/api/v2.0/join/experience",
        ] {
            for cut in 0..=seed.len() {
                fuzz_realms(&seed[..cut]);
            }
        }
        for seed in [
            &br#"{"result":{"networkProtocol":"Default","ipV4Address":"198.51.100.20","port":19132}}"#[..],
            br#"{"experienceId":"6b3d2a5e-0c1f-4a7e-9a52-2f3c9d0e1b44"}"#,
        ] {
            for cut in 0..=seed.len() {
                fuzz_gatherings(&seed[..cut]);
            }
        }
    }
}
