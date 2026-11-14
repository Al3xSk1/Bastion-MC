//! Credential broker: the game never holds the Minecraft access token.
//!
//! Before this existed the token travelled to the JVM (over a socket, never a command
//! line) and lived in its heap for the session. Anything running inside the JVM — a
//! malicious mod, or a server exploiting the client — could read it and use it for a day:
//! change the skin, the name, the friends list, or log in elsewhere as you.
//!
//! Now the JVM is given a **placeholder**. The real token stays in the launcher, in locked
//! memory. HTTPS from the game to Mojang's two API domains is not passed through the relay
//! but *terminated* in it, with a certificate authority created for this launch only and
//! known to nobody but this process and the sandbox's generated trust store. For each
//! request the broker decides, from a fixed table, what happens:
//!
//! | request | action |
//! |---|---|
//! | one of the endpoints the game needs a credential for, **and** that capability is enabled, **and** the request presents the placeholder | forwarded to Mojang over the launcher's own pinned TLS 1.3, with the real token in place of the placeholder |
//! | the server-join handshake | checked (your profile, a well-formed server hash, a game connection opened moments ago through the relay), rebuilt from scratch around the real token, forwarded |
//! | any other `GET` | forwarded with no credential at all |
//! | anything that would change something — profile, skin, cape, name, and every method/endpoint not in the table | answered `403` here; it never leaves the machine |
//!
//! Requests are rebuilt, not passed on: method, path and an allowlist of headers are copied,
//! everything else (cookies, the game's `Authorization`, unknown headers) is dropped.
//! Responses are checked for the real token before they go back; none of Mojang's endpoints
//! echo it, and if one ever did, the reply is withheld. The check reads the reply as it
//! arrived, so a reply to a request that carried the token must arrive unencoded (the
//! launcher asks for exactly that): one that comes back with a `Content-Encoding` cannot be
//! checked and is withheld too.
//!
//! **Memory.** The broker holds request bodies while it decides and whole replies while it
//! checks them, in the launcher's memory — which is not charged to the game's resource
//! scope. A game that opens many connections, asks for large objects and never reads the
//! answers must not be able to turn that into gigabytes: all bodies the broker holds, over
//! all connections, come out of one budget ([`BUFFER_BUDGET`]). A request that finds no room
//! waits briefly and is then answered `503`; a reply keeps its share until the game has
//! taken it or its connection has ended. The only one this can starve is the game itself.
//!
//! What this does **not** close: code inside the JVM can still ask for *a* server join while
//! a game connection is open (one per connection), and whoever holds that join can complete
//! one login on one server with it. Removing that would need a proxy that speaks the game
//! protocol itself. See docs/THREAT_MODEL.md, J13.

use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::{
    HeaderMap, Method, Request, Response,
    body::{Body, Frame, Incoming, SizeHint},
    header::{self, HeaderName},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, NameConstraints,
};
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    server::{Acceptor, NoServerSessionStorage},
};
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::LazyConfigAcceptor;

use super::{Logger, Stats};
use crate::{
    auth,
    config::BrokerConfig,
    error::{Error, Result},
    net::{
        http::{Connection, Headers, HttpClient, sensitive_header},
        tls,
    },
    secret::Secret,
    util,
};

/// Domains whose HTTPS is terminated here instead of passed through. Everything Mojang
/// authenticates with the access token lives under these two.
pub const DOMAINS: [&str; 2] = ["mojang.com", "minecraftservices.com"];

const API: &str = "api.minecraftservices.com";
const SESSION: &str = "sessionserver.mojang.com";
const JOIN_PATH: &str = "/session/minecraft/join";

/// A join is honoured only for a game connection opened this recently…
const JOIN_WINDOW: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_IDLE: Duration = Duration::from_secs(60);
const CONNECTION_LIFETIME: Duration = Duration::from_secs(30 * 60);
const MAX_REQUEST_BODY: usize = 2 << 20;
const MAX_RESPONSE_BODY: u64 = 8 << 20;
const MAX_NAMES: usize = 64;
/// Most bytes of request and reply bodies the broker holds at one time, over all of the
/// game's connections together. Room for a handful of the largest replies at once; what the
/// game really asks for (profiles, key lists, block lists) is a few kilobytes each.
///
/// What is counted is body bytes. On top of that come, per connection, hyper's own buffers
/// (bounded at 64 KiB each way) and whatever slack the allocator leaves in a buffer that
/// grew — so the launcher's real use for this is bounded by about twice this figure plus
/// that, not by the figure exactly.
pub const BUFFER_BUDGET: usize = 64 << 20;
/// How long a request waits for room in the budget before it is refused.
const BUDGET_WAIT: Duration = Duration::from_secs(10);
/// What one permit of the budget's semaphore stands for.
const BUDGET_UNIT: usize = 4096;
/// A reply is handed to the HTTP layer in pieces of this size, so that the part the game has
/// not taken yet stays in the body — where it is counted — and not in that layer's buffers.
const REPLY_FRAME: usize = 16 * 1024;

/// Copied from the game's request to the launcher's. Nothing else is.
const REQUEST_HEADERS: [HeaderName; 6] = [
    header::ACCEPT,
    header::ACCEPT_LANGUAGE,
    header::CONTENT_TYPE,
    header::USER_AGENT,
    header::IF_NONE_MATCH,
    header::IF_MODIFIED_SINCE,
];
/// Copied from Mojang's response to the game's.
const RESPONSE_HEADERS: [HeaderName; 7] = [
    header::CONTENT_TYPE,
    header::ETAG,
    header::RETRY_AFTER,
    header::CACHE_CONTROL,
    header::EXPIRES,
    header::LAST_MODIFIED,
    header::CONTENT_ENCODING,
];

// ---------------------------------------------------------------- certificate authority

/// The per-launch certificate authority. Created in memory, used for this launch, gone with
/// the process. Its certificate is constrained to [`DOMAINS`].
pub struct Authority {
    params: CertificateParams,
    key: KeyPair,
    certificate: CertificateDer<'static>,
    /// Its name constraints include the Realms domains (`realms::table::DOMAINS`).
    realms: bool,
}

fn pki(e: impl std::fmt::Display) -> Error {
    Error::Tls(format!("credential broker certificate: {e}"))
}

impl Authority {
    pub fn generate() -> Result<Self> {
        Self::generate_for(false)
    }

    /// With `realms`, the authority may also issue certificates for the Realms hosts
    /// (`[realms] approve` is on): its name constraints then also permit
    /// `realms.minecraft.net` and `realms.minecraft-services.net`, and nothing else is added.
    pub fn generate_for(realms: bool) -> Result<Self> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(pki)?;
        let mut params = CertificateParams::new(Vec::<String>::new()).map_err(pki)?;
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::hours(1);
        // Longer than any session; the access token itself is good for a day.
        params.not_after = now + time::Duration::days(7);
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let mut name = DistinguishedName::new();
        name.push(DnType::OrganizationName, "bastion-mc (this computer, this launch only)");
        name.push(
            DnType::CommonName,
            format!("bastion-mc credential broker {}", util::hex_encode(&util::random_bytes::<4>()?)),
        );
        params.distinguished_name = name;
        let realms_domains: &[&str] = if realms { &crate::realms::table::DOMAINS } else { &[] };
        params.name_constraints = Some(NameConstraints {
            permitted_subtrees: DOMAINS
                .iter()
                .chain(realms_domains)
                .map(|d| GeneralSubtree::DnsName((*d).to_string()))
                .collect(),
            excluded_subtrees: Vec::new(),
        });
        let certificate = params.self_signed(&key).map_err(pki)?.der().clone();
        Ok(Authority { params, key, certificate, realms })
    }

    /// The certificate to put into the sandbox's trust store.
    pub fn certificate_der(&self) -> &[u8] {
        self.certificate.as_ref()
    }

    fn issue(&self, name: &str) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(pki)?;
        let mut params = CertificateParams::new(vec![name.to_string()]).map_err(pki)?;
        params.not_before = self.params.not_before;
        params.not_after = self.params.not_after;
        let mut subject = DistinguishedName::new();
        subject.push(DnType::CommonName, name);
        params.distinguished_name = subject;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let issuer = Issuer::from_params(&self.params, &self.key);
        let certificate = params.signed_by(&key, &issuer).map_err(pki)?.der().clone();
        Ok((certificate, PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()))))
    }
}

// ---------------------------------------------------------------- join gate

/// Ties server joins to game connections. The relay registers every game-server connection
/// it opens; a join request is honoured only if such a connection was opened within
/// [`JOIN_WINDOW`] and has not been used for a join yet.
#[derive(Default)]
pub struct JoinGate {
    inner: Mutex<GateInner>,
}

#[derive(Default)]
struct GateInner {
    next: u64,
    open: HashMap<u64, Ticket>,
}

struct Ticket {
    opened: Instant,
    label: String,
    used: bool,
}

/// A relayed game connection, for as long as it is open.
pub struct GameConnection<'a> {
    gate: &'a JoinGate,
    id: u64,
}

impl Drop for GameConnection<'_> {
    fn drop(&mut self) {
        self.gate.inner.lock().expect("join gate lock").open.remove(&self.id);
    }
}

impl JoinGate {
    pub fn opened(&self, label: &str) -> GameConnection<'_> {
        let mut inner = self.inner.lock().expect("join gate lock");
        inner.next += 1;
        let id = inner.next;
        inner.open.insert(id, Ticket { opened: Instant::now(), label: label.to_string(), used: false });
        GameConnection { gate: self, id }
    }

    /// Spend the newest eligible game connection on a join. Returns its label.
    pub fn authorize(&self) -> std::result::Result<String, &'static str> {
        let mut inner = self.inner.lock().expect("join gate lock");
        if inner.open.is_empty() {
            return Err("no game-server connection is open");
        }
        let ticket = inner
            .open
            .values_mut()
            .filter(|t| !t.used && t.opened.elapsed() <= JOIN_WINDOW)
            .max_by_key(|t| t.opened)
            .ok_or("no game-server connection was opened in the last 30 seconds that has not already been joined")?;
        ticket.used = true;
        Ok(ticket.label.clone())
    }
}

// ---------------------------------------------------------------- policy

/// What a credential may be used for. Each maps to a key under `[network.broker]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Join,
    ChatSigning,
    AccountRead,
    Friends,
    Settings,
    Reports,
    Telemetry,
    /// An endpoint named in `extra_token_endpoints`.
    Extra,
}

impl Capability {
    pub fn key(self) -> &'static str {
        match self {
            Capability::Join => "join",
            Capability::ChatSigning => "chat_signing",
            Capability::AccountRead => "account_read",
            Capability::Friends => "friends",
            Capability::Settings => "settings",
            Capability::Reports => "reports",
            Capability::Telemetry => "telemetry",
            Capability::Extra => "extra_token_endpoints",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    host: String,
    method: Method,
    path: String,
    capability: Capability,
}

/// Every endpoint the game sends the access token to (Minecraft 1.16 – 26.x; the current
/// list is what `discovery.minecraftservices.com/minecraft/client` hands the game).
fn builtin_endpoints() -> Vec<Endpoint> {
    use Capability::*;
    [
        (SESSION, Method::POST, JOIN_PATH, Join),
        (API, Method::POST, "/player/certificates", ChatSigning),
        (API, Method::GET, "/player/attributes", AccountRead),
        (API, Method::GET, "/privacy/blocklist", AccountRead),
        (API, Method::GET, "/friends", Friends),
        (API, Method::PUT, "/friends", Friends),
        (API, Method::POST, "/presence", Friends),
        (API, Method::POST, "/player/attributes", Settings),
        (API, Method::POST, "/player/report", Reports),
        (API, Method::POST, "/events", Telemetry),
    ]
    .into_iter()
    .map(|(host, method, path, capability)| Endpoint { host: host.into(), method, path: path.into(), capability })
    .collect()
}

/// `POST` requests that carry no credential and change nothing (name lookups).
const ANONYMOUS_POSTS: [(&str, &str); 2] =
    [("api.mojang.com", "/profiles/minecraft"), (API, "/minecraft/profile/lookup/bulk/byname")];

/// Paths under which any change is an attack on the account itself.
const ACCOUNT_PATHS: [&str; 5] =
    ["/minecraft/profile", "/authentication", "/entitlements", "/productvoucher", "/purchase"];

/// A path the broker is willing to reason about: plain segments, nothing a server could
/// normalise into a different path than the one that was checked.
pub fn path_is_canonical(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 512
        && path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~'))
        && !path.contains("//")
        && path.split('/').all(|segment| segment != "." && segment != "..")
}

fn query_is_plain(query: &str) -> bool {
    query.len() <= 2048
        && query.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'-' | b'_'
                        | b'.'
                        | b'~'
                        | b'%'
                        | b'&'
                        | b'='
                        | b'+'
                        | b','
                        | b':'
                        | b';'
                        | b'@'
                        | b'!'
                        | b'$'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'/'
                        | b'?'
                )
        })
}

/// `"METHOD host/path"`, as written in `extra_token_endpoints`.
pub fn parse_endpoint(spec: &str) -> Result<(Method, String, String)> {
    let bad = |why: &str| Error::Config(format!("network.broker.extra_token_endpoints: {spec:?}: {why}"));
    let (method, target) = spec.trim().split_once(' ').ok_or_else(|| bad("expected \"METHOD host/path\""))?;
    let method = match method {
        "GET" => Method::GET,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "DELETE" => Method::DELETE,
        _ => return Err(bad("the method must be GET, POST, PUT or DELETE")),
    };
    let slash = target.find('/').ok_or_else(|| bad("no path"))?;
    let (host, path) = target.split_at(slash);
    let host = crate::config::normalize_host(host).map_err(|_| bad("invalid host name"))?;
    if !Broker::intercepts(&host) {
        return Err(bad("the broker only handles hosts under mojang.com and minecraftservices.com"));
    }
    if !path_is_canonical(path) {
        return Err(bad("the path must consist of plain segments, without a query"));
    }
    if path.to_ascii_lowercase().starts_with("/minecraft/profile") || path == JOIN_PATH {
        return Err(bad("profile-changing endpoints and the join handshake cannot be added here"));
    }
    Ok((method, host, path.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Forward without any credential.
    Anonymous,
    /// Forward with the real token attached.
    Credentialed(Capability),
    /// The join handshake: validated and rebuilt, see [`Broker::join_body`].
    Join,
    /// Answer here. `security` marks attempts on the account itself.
    Refuse { status: u16, reason: String, security: bool },
}

fn refuse(status: u16, reason: impl Into<String>) -> Verdict {
    Verdict::Refuse { status, reason: reason.into(), security: false }
}

// ---------------------------------------------------------------- the broker

/// The real credential, kept by the launcher for the length of the session.
pub struct Credential {
    pub token: Secret,
    /// Undashed lower-case profile UUID.
    pub profile_id: String,
}

pub struct Broker {
    authority: Authority,
    tls: Mutex<HashMap<String, Arc<ServerConfig>>>,
    placeholder: String,
    credential: Option<Credential>,
    enabled: Vec<Capability>,
    endpoints: Vec<Endpoint>,
    http: HttpClient,
    buffers: Budget,
    /// Realms, when `[realms] approve` is on: the approvals and the dialer.
    realms: Option<RealmsBroker>,
}

/// What the broker needs for Realms (see `realms`).
pub struct RealmsBroker {
    pub approvals: Arc<crate::realms::approval::Approvals>,
    /// Connects the one approved connection; `None` only in tests that stop before that.
    pub dialer: Option<Arc<crate::realms::dial::Dialer>>,
    /// Test support, never set by a launch: accept a Realm server on the loopback address
    /// (the tests' stand-in) where a real one must be public.
    #[doc(hidden)]
    pub loopback_for_tests: bool,
}

impl RealmsBroker {
    /// May the approved connection go to `ip`? Public addresses only (see `egress::is_public`).
    pub fn permits(&self, ip: std::net::IpAddr) -> bool {
        super::is_public(ip) || (self.loopback_for_tests && ip.is_loopback())
    }
}

/// The one budget all buffered bodies come out of (see the module documentation).
struct Budget {
    permits: Arc<tokio::sync::Semaphore>,
    bytes: usize,
    wait: Duration,
}

/// A share of the [`Budget`], returned to it when dropped.
struct Room(tokio::sync::OwnedSemaphorePermit);

fn budget_units(bytes: usize) -> usize {
    bytes.div_ceil(BUDGET_UNIT)
}

impl Budget {
    fn new(bytes: usize, wait: Duration) -> Self {
        Budget { permits: Arc::new(tokio::sync::Semaphore::new(budget_units(bytes))), bytes, wait }
    }

    /// Room for `bytes`, waiting for it if need be — but not for long. `None`: there is none.
    async fn reserve(&self, bytes: usize) -> Option<Room> {
        let units = u32::try_from(budget_units(bytes)).ok()?;
        match timeout(self.wait, self.permits.clone().acquire_many_owned(units)).await {
            Ok(Ok(permit)) => Some(Room(permit)),
            _ => None,
        }
    }

    fn in_use(&self) -> usize {
        (budget_units(self.bytes) - self.permits.available_permits()) * BUDGET_UNIT
    }
}

impl Room {
    /// Give back what turned out not to be needed.
    fn shrink_to(&mut self, bytes: usize) {
        let excess = self.0.num_permits().saturating_sub(budget_units(bytes));
        drop(self.0.split(excess));
    }
}

/// The body of a reply to the game: the bytes, and — for a reply that came from upstream —
/// the room they occupy in the budget, which is given back when this body is dropped (the
/// game has read all of it, or its connection is gone). Handed out in small frames: the
/// HTTP layer asks for the next one only when the previous ones are on their way.
pub struct ReplyBody {
    bytes: Bytes,
    _room: Option<Room>,
}

impl ReplyBody {
    fn local(bytes: Bytes) -> Self {
        ReplyBody { bytes, _room: None }
    }
}

impl Body for ReplyBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<std::result::Result<Frame<Bytes>, Infallible>>> {
        if self.bytes.is_empty() {
            return std::task::Poll::Ready(None);
        }
        let take = self.bytes.len().min(REPLY_FRAME);
        std::task::Poll::Ready(Some(Ok(Frame::data(self.bytes.split_to(take)))))
    }

    fn is_end_stream(&self) -> bool {
        self.bytes.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.bytes.len() as u64)
    }
}

/// What the game is given instead of a token: recognisable, unique per launch, worthless.
pub fn new_placeholder() -> Result<String> {
    Ok(format!("bastion-placeholder-{}", util::hex_encode(&util::random_bytes::<16>()?)))
}

impl Broker {
    /// `http` is the launcher's own client (system roots, built-in pins, TLS 1.3); the
    /// broker widens its host allowlist to [`DOMAINS`] and nothing else.
    pub fn new(
        authority: Authority,
        config: &BrokerConfig,
        http: &HttpClient,
        placeholder: String,
        credential: Option<Credential>,
    ) -> Result<Self> {
        if let Some(c) = &credential {
            // The token is spliced into a hand-built JSON body and a header value.
            auth::require_json_safe(&c.token, "Minecraft access token")?;
            if c.profile_id.len() != 32 || !c.profile_id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::Auth("credential broker: malformed profile id".into()));
            }
        }
        let mut endpoints = builtin_endpoints();
        for spec in &config.extra_token_endpoints {
            let (method, host, path) = parse_endpoint(spec)?;
            endpoints.push(Endpoint { host, method, path, capability: Capability::Extra });
        }
        let enabled = [
            (config.join, Capability::Join),
            (config.chat_signing, Capability::ChatSigning),
            (config.account_read, Capability::AccountRead),
            (config.friends, Capability::Friends),
            (config.settings, Capability::Settings),
            (config.reports, Capability::Reports),
            (config.telemetry, Capability::Telemetry),
            (true, Capability::Extra),
        ]
        .into_iter()
        .filter_map(|(on, capability)| on.then_some(capability))
        .collect();
        Ok(Broker {
            authority,
            tls: Mutex::new(HashMap::new()),
            placeholder,
            credential,
            enabled,
            endpoints,
            http: http.clone().with_domains(&DOMAINS),
            buffers: Budget::new(BUFFER_BUDGET, BUDGET_WAIT),
            realms: None,
        })
    }

    /// Broker Realms as well (`[realms] approve` is on). The authority must have been made
    /// for it ([`Authority::generate_for`]).
    pub fn with_realms(mut self, realms: RealmsBroker) -> Result<Self> {
        if !self.authority.realms {
            return Err(Error::Tls("the credential broker's authority was not made for the Realms hosts".into()));
        }
        let domains: Vec<&str> = DOMAINS.iter().chain(&crate::realms::table::DOMAINS).copied().collect();
        self.http = self.http.clone().with_domains(&domains);
        self.realms = Some(realms);
        Ok(self)
    }

    /// The Realms part, when Realms is brokered.
    pub fn realms(&self) -> Option<&RealmsBroker> {
        self.realms.as_ref()
    }

    /// Is HTTPS to this name terminated by this broker? Mojang's API domains always; the
    /// Realms hosts when Realms is brokered.
    pub fn terminates(&self, name: &str) -> bool {
        Self::intercepts(name) || (self.realms.is_some() && crate::realms::table::is_realms_host(name))
    }

    /// Another budget for buffered bodies, and another wait for room in it. For the tests,
    /// which cannot hold 64 MiB of replies or wait ten seconds to see a refusal; never less
    /// than one request and one reply of the largest size.
    #[doc(hidden)]
    pub fn with_buffer_budget(mut self, bytes: usize, wait: Duration) -> Self {
        self.buffers = Budget::new(bytes.max(MAX_REQUEST_BODY + MAX_RESPONSE_BODY as usize), wait);
        self
    }

    /// Bytes of request and reply bodies held (or reserved for a reply on its way) right now.
    pub fn buffered_bytes(&self) -> usize {
        self.buffers.in_use()
    }

    /// Is HTTPS to this name terminated by the broker (rather than passed through)?
    pub fn intercepts(name: &str) -> bool {
        DOMAINS.iter().any(|d| name == *d || name.strip_suffix(d).is_some_and(|prefix| prefix.ends_with('.')))
    }

    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// One line for the launch log.
    pub fn describe(&self) -> String {
        let on: Vec<&str> = self.enabled.iter().filter(|c| **c != Capability::Extra).map(|c| c.key()).collect();
        format!(
            "credential broker: the game holds a placeholder; the launcher authenticates [{}] on its behalf{}",
            on.join(", "),
            if self.credential.is_none() { " (no account in this session: nothing is authenticated)" } else { "" }
        ) + match &self.realms {
            Some(r) => match r.approvals.mode() {
                crate::realms::Approve::Totp => "; Realms: brokered, joins need your approval (TOTP code in the phone)",
                _ => "; Realms: brokered, joins need your approval (the launcher's code, typed into the phone)",
            },
            None => "",
        }
    }

    /// The decision table. Pure: it looks at nothing but its arguments and the configuration.
    pub fn classify(
        &self,
        host: &str,
        method: &Method,
        path: &str,
        has_query: bool,
        presents_placeholder: bool,
    ) -> Verdict {
        if ![Method::GET, Method::POST, Method::PUT, Method::DELETE].contains(method) {
            return refuse(405, format!("method {method} is not relayed"));
        }
        if !path_is_canonical(path) {
            return refuse(400, "the request path is not in canonical form");
        }
        let lowered = path.to_ascii_lowercase();
        let known = self.endpoints.iter().find(|e| e.host == host && e.method == *method && e.path == path);
        if known.is_none()
            && self.endpoints.iter().any(|e| e.host == host && e.path.eq_ignore_ascii_case(path) && e.path != path)
        {
            // Same endpoint, different spelling: never let that reach a server that might
            // fold case, with or without a credential.
            return refuse(400, "non-canonical spelling of a credentialed endpoint");
        }
        if let Some(endpoint) = known {
            if has_query {
                return refuse(400, "credentialed endpoints take no query string");
            }
            let capability = endpoint.capability;
            if !self.enabled.contains(&capability) {
                return refuse(
                    403,
                    format!(
                        "{} {path} needs your account and is switched off (network.broker.{} = false)",
                        method,
                        capability.key()
                    ),
                );
            }
            if capability == Capability::Join {
                return Verdict::Join;
            }
            if !presents_placeholder {
                // Whatever the game sent instead is not ours to vouch for.
                return if *method == Method::GET {
                    Verdict::Anonymous
                } else {
                    refuse(401, "no credential the launcher recognises was presented")
                };
            }
            if self.credential.is_none() {
                return refuse(401, "there is no account in this session (demo mode)");
            }
            return Verdict::Credentialed(capability);
        }
        let changes = *method != Method::GET;
        if changes
            && ACCOUNT_PATHS.iter().any(|p| lowered == *p || lowered.starts_with(&format!("{p}/")))
            && !ANONYMOUS_POSTS.iter().any(|(h, p)| *h == host && *p == path && *method == Method::POST)
        {
            return Verdict::Refuse {
                status: 403,
                reason: format!("{method} {path} would change your account; the game is never allowed to"),
                security: true,
            };
        }
        if !changes || ANONYMOUS_POSTS.iter().any(|(h, p)| *h == host && *p == path && *method == Method::POST) {
            return Verdict::Anonymous;
        }
        Verdict::Refuse {
            status: 403,
            reason: format!(
                "{method} {path} is not an endpoint the broker knows; state-changing requests are denied by default"
            ),
            // Presenting the placeholder to an unknown endpoint is worth a closer look.
            security: presents_placeholder,
        }
    }

    /// Validate a join request and build the one that is actually sent. Nothing of the
    /// game's body is forwarded: the three fields are checked and written out again.
    pub fn join_body(&self, gate: &JoinGate, body: &[u8]) -> std::result::Result<(Bytes, String), Verdict> {
        struct JoinRequest<'a> {
            access_token: &'a str,
            selected_profile: &'a str,
            server_id: &'a str,
        }
        let Some(credential) = &self.credential else {
            return Err(refuse(401, "there is no account in this session (demo mode)"));
        };
        // Exactly a JSON object with exactly these three string members. (A derived
        // deserialiser would also take an array, in field order.)
        let malformed = || refuse(400, "the join request is not the expected JSON object");
        if body.len() > 4096 {
            return Err(malformed());
        }
        let object = match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(serde_json::Value::Object(object)) if object.len() == 3 => object,
            _ => return Err(malformed()),
        };
        let member = |name: &str| object.get(name).and_then(serde_json::Value::as_str);
        let request = match (member("accessToken"), member("selectedProfile"), member("serverId")) {
            (Some(access_token), Some(selected_profile), Some(server_id)) => {
                JoinRequest { access_token, selected_profile, server_id }
            }
            _ => return Err(malformed()),
        };
        if request.access_token != self.placeholder {
            return Err(refuse(401, "the join request does not carry the credential the launcher handed out"));
        }
        let profile: String = request.selected_profile.chars().filter(|c| *c != '-').collect();
        if !profile.eq_ignore_ascii_case(&credential.profile_id) {
            return Err(Verdict::Refuse {
                status: 403,
                reason: "the join request is for a different profile than the one signed in".into(),
                security: true,
            });
        }
        // Minecraft's server hash: a signed hexadecimal SHA-1.
        let hash = request.server_id.strip_prefix('-').unwrap_or(request.server_id);
        if hash.is_empty() || hash.len() > 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(refuse(400, "the join request's server hash is malformed"));
        }
        let connection = gate.authorize().map_err(|why| Verdict::Refuse {
            status: 403,
            reason: format!("server join refused: {why}"),
            security: true,
        })?;

        let token = credential.token.expose();
        let mut out = Vec::with_capacity(token.len() + credential.profile_id.len() + request.server_id.len() + 64);
        out.extend_from_slice(br#"{"accessToken":""#);
        out.extend_from_slice(token);
        out.extend_from_slice(br#"","selectedProfile":""#);
        out.extend_from_slice(credential.profile_id.as_bytes());
        out.extend_from_slice(br#"","serverId":""#);
        out.extend_from_slice(request.server_id.as_bytes());
        out.extend_from_slice(br#""}"#);
        Ok((Bytes::from(out), connection))
    }

    fn tls_config(&self, name: &str) -> std::result::Result<Arc<ServerConfig>, String> {
        let mut cache = self.tls.lock().expect("broker tls lock");
        if let Some(config) = cache.get(name) {
            return Ok(config.clone());
        }
        if cache.len() >= MAX_NAMES {
            return Err("too many distinct service names in one session".into());
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
}

// ---------------------------------------------------------------- serving

/// What a connection handler needs from the relay around it.
pub struct Context<'a> {
    pub gate: &'a JoinGate,
    pub log: &'a Logger,
    pub stats: &'a Stats,
}

fn local_reply(status: u16, message: &str) -> Response<ReplyBody> {
    let body =
        serde_json::json!({ "error": "bastion-mc", "errorMessage": format!("bastion-mc: {message}") }).to_string();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(ReplyBody::local(Bytes::from(body)))
        .expect("static response")
}

fn forwardable(headers: &HeaderMap) -> Headers {
    REQUEST_HEADERS.iter().filter_map(|name| headers.get(name).map(|value| (name.clone(), value.clone()))).collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}

impl Broker {
    /// Serve one connection from the game that was addressed to `name`: TLS handshake with a
    /// certificate for exactly that name, then HTTP/1.1 requests until either side is done.
    /// Returns the number of requests handled.
    pub async fn serve(&self, ctx: &Context<'_>, inbound: TcpStream, name: &str) -> std::result::Result<u64, String> {
        let start = timeout(HANDSHAKE_TIMEOUT, LazyConfigAcceptor::new(Acceptor::default(), inbound))
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
        let stream = timeout(HANDSHAKE_TIMEOUT, start.into_stream(config))
            .await
            .map_err(|_| "TLS handshake with the game timed out".to_string())?
            .map_err(|e| format!("TLS handshake with the game failed ({e})"))?;

        let upstream = tokio::sync::Mutex::new(None);
        let requests = std::sync::atomic::AtomicU64::new(0);
        let service = service_fn(|request| {
            requests.fetch_add(1, Ordering::Relaxed);
            let (upstream, name) = (&upstream, name);
            async move { Ok::<_, Infallible>(self.handle(ctx, upstream, name, request).await) }
        });
        let connection = http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(REQUEST_IDLE)
            .max_buf_size(64 * 1024)
            .keep_alive(true)
            .serve_connection(TokioIo::new(stream), service);
        // Errors here are the game hanging up mid-request or sending garbage; both simply
        // end the connection.
        let _ = timeout(CONNECTION_LIFETIME, connection).await;
        Ok(requests.load(Ordering::Relaxed))
    }

    async fn handle(
        &self,
        ctx: &Context<'_>,
        upstream: &tokio::sync::Mutex<Option<Connection>>,
        host: &str,
        request: Request<Incoming>,
    ) -> Response<ReplyBody> {
        let (parts, body) = request.into_parts();
        let method = parts.method.clone();
        let path = parts.uri.path().to_string();
        let shown = util::sanitize_line(path.as_bytes(), 160);
        let say = |outcome: &str| (ctx.log)(format!("broker: {host} {method} {shown} → {outcome}"));
        let deny = |status: u16, reason: &str, security: bool| {
            ctx.stats.broker_refused.fetch_add(1, Ordering::Relaxed);
            say(&format!("{status} here{}: {reason}", if security { " [SECURITY]" } else { "" }));
            local_reply(status, reason)
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
        let query = parts.uri.query();
        if query.is_some_and(|q| !query_is_plain(q)) {
            return deny(400, "the query string contains characters that are not relayed", false);
        }
        if self.realms.is_some() && crate::realms::table::is_realms_host(host) {
            return self.handle_realms(ctx, upstream, host, &parts, body).await;
        }
        let presents_placeholder = parts
            .headers
            .get(header::AUTHORIZATION)
            .is_some_and(|value| value.as_bytes() == format!("Bearer {}", self.placeholder).as_bytes());

        let verdict = self.classify(host, &method, &path, query.is_some(), presents_placeholder);
        if let Verdict::Refuse { status, reason, security } = &verdict {
            return deny(*status, reason, *security);
        }
        // The body is held in memory from here until the request has been sent on: out of
        // the budget, like everything the broker holds for the game. Its declared size is
        // what is reserved; a body of undeclared size (chunked) may turn out to be the
        // largest allowed.
        let no_room = "the launcher is already holding as much of this game's traffic with Mojang as it will: \
                       replies the game has not read yet are in the way";
        let declared = body.size_hint().upper().unwrap_or(MAX_REQUEST_BODY as u64);
        if declared > MAX_REQUEST_BODY as u64 {
            return deny(413, "the request body is too large or was cut short", false);
        }
        let request_room = match declared {
            0 => None,
            bytes => match self.buffers.reserve(bytes as usize).await {
                Some(room) => Some(room),
                None => return deny(503, no_room, false),
            },
        };
        let body = match Limited::new(body, MAX_REQUEST_BODY).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return deny(413, "the request body is too large or was cut short", false),
        };

        let mut headers = forwardable(&parts.headers);
        let (body, how) = match verdict {
            Verdict::Anonymous => (body, String::new()),
            Verdict::Credentialed(capability) => {
                let token = self.credential.as_ref().expect("classified as credentialed").token.expose();
                let mut value = Vec::with_capacity(token.len() + 7);
                value.extend_from_slice(b"Bearer ");
                value.extend_from_slice(token);
                match sensitive_header("authorization", &value) {
                    Ok(header) => headers.push(header),
                    Err(_) => return deny(500, "the credential could not be attached", false),
                }
                (body, format!(" [credential attached: {}]", capability.key()))
            }
            Verdict::Join => match self.join_body(ctx.gate, &body) {
                Ok((rebuilt, connection)) => {
                    headers.retain(|(name, _)| name != header::CONTENT_TYPE);
                    headers.push((
                        header::CONTENT_TYPE,
                        header::HeaderValue::from_static("application/json; charset=utf-8"),
                    ));
                    (rebuilt, format!(" [join authorised for game connection {connection}]"))
                }
                Err(Verdict::Refuse { status, reason, security }) => return deny(status, &reason, security),
                Err(_) => return deny(500, "internal error", false),
            },
            Verdict::Refuse { .. } => unreachable!("handled above"),
        };
        let credentialed = !how.is_empty();

        let target = match query {
            Some(q) => format!("{path}?{q}"),
            None => path.clone(),
        };
        let body = (!body.is_empty() || method != Method::GET).then_some(body);
        // Room for the reply before it is asked for: how large it is is only known once it
        // is here, so the largest possible one is reserved and the rest given back.
        let Some(mut reply_room) = self.buffers.reserve(MAX_RESPONSE_BODY as usize).await else {
            return deny(503, no_room, false);
        };
        let (head, mut payload) = match self.forward(upstream, host, method.clone(), &target, &headers, body).await {
            Ok(reply) => reply,
            Err(e) => {
                ctx.stats.broker_refused.fetch_add(1, Ordering::Relaxed);
                say(&format!("502 (upstream: {})", util::sanitize_line(e.to_string().as_bytes(), 200)));
                return local_reply(502, "the request could not be relayed to Mojang");
            }
        };
        drop(request_room);
        payload.shrink_to_fit();
        reply_room.shrink_to(payload.len());
        // A reply to a request that carried the token is checked for it below — which only
        // means something for a reply that arrived as it is. The launcher asked for exactly
        // that (`Accept-Encoding: identity`); an answer that is encoded all the same cannot
        // be checked, and is not passed on.
        if credentialed {
            let encoding = head.headers.get(header::CONTENT_ENCODING).map(|v| v.as_bytes().trim_ascii());
            if encoding.is_some_and(|e| !e.is_empty() && !e.eq_ignore_ascii_case(b"identity")) {
                return deny(
                    502,
                    "the reply to a request that carried your credential came back encoded; it cannot be checked \
                     for the access token and was withheld",
                    true,
                );
            }
        }
        // No endpoint on the list echoes the token. If one ever does, it stays here.
        if let Some(credential) = &self.credential {
            let token = credential.token.expose();
            let leaked = contains(&payload, token)
                || RESPONSE_HEADERS
                    .iter()
                    .any(|name| head.headers.get(name).is_some_and(|v| contains(v.as_bytes(), token)));
            if leaked {
                return deny(502, "the reply contained the access token and was withheld", true);
            }
        }
        ctx.stats.brokered.fetch_add(1, Ordering::Relaxed);
        if credentialed {
            ctx.stats.credentialed.fetch_add(1, Ordering::Relaxed);
        }
        say(&format!("{}{how}", head.status));

        let mut response = Response::builder().status(head.status);
        for name in &RESPONSE_HEADERS {
            if let Some(value) = head.headers.get(name) {
                response = response.header(name, value);
            }
        }
        response
            .body(ReplyBody { bytes: Bytes::from(payload), _room: Some(reply_room) })
            .unwrap_or_else(|_| local_reply(502, "malformed upstream reply"))
    }

    /// One request to the real server, on a connection kept open between requests of the
    /// same inbound connection.
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
        match connection.call(method, target, headers, body, MAX_RESPONSE_BODY, &mut payload).await {
            Ok(head) => Ok((head, payload)),
            Err(e) => {
                *slot = None;
                Err(e)
            }
        }
    }
}

// ---------------------------------------------------------------- Realms

/// Realms requests carry these besides the usual ones (26.x: which client is asking).
const REALMS_HEADERS: [&str; 3] = ["client-ref", "x-networkprotocolversion", "is-prerelease"];
/// The largest body of a Realms request the broker takes (marking notifications, region pings).
const REALMS_MAX_BODY: usize = 64 * 1024;

/// A reply in the form the Realms client shows (`errorMsg`).
fn realms_reply(status: u16, message: &str, retry_after: Option<u64>) -> Response<ReplyBody> {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(seconds) = retry_after {
        builder = builder.header(header::RETRY_AFTER, seconds.to_string());
    }
    builder
        .body(ReplyBody::local(Bytes::from(crate::realms::table::error_body(status, message))))
        .expect("static response")
}

impl Broker {
    /// One request to a Realms host (see `realms::table` for what is on the table, and
    /// `realms::approval` for the join).
    async fn handle_realms(
        &self,
        ctx: &Context<'_>,
        upstream: &tokio::sync::Mutex<Option<Connection>>,
        host: &str,
        parts: &hyper::http::request::Parts,
        body: Incoming,
    ) -> Response<ReplyBody> {
        use crate::realms::{approval::JoinStep, table};
        let realms = self.realms.as_ref().expect("checked by the caller");
        let method = parts.method.clone();
        let path = parts.uri.path().to_string();
        let shown = util::sanitize_line(path.as_bytes(), 160);
        let say = |outcome: &str| (ctx.log)(format!("broker: {host} {method} {shown} → {outcome}"));
        let deny = |status: u16, reason: &str, security: bool| {
            ctx.stats.broker_refused.fetch_add(1, Ordering::Relaxed);
            say(&format!("{status} here{}: {reason}", if security { " [SECURITY]" } else { "" }));
            realms_reply(status, reason, None)
        };
        let query = parts.uri.query();
        let verdict = table::classify(host, &method, &path, query.is_some());
        if let table::Verdict::Refuse { status, reason, security } = &verdict {
            return deny(*status, reason, *security);
        }
        // A join may be answered here, without asking Realms again.
        let join = match verdict {
            table::Verdict::Join(world) => match realms.approvals.join_requested(world) {
                JoinStep::Forward => Some(world),
                JoinStep::Wait(seconds) => {
                    say("503 here: waiting for your approval outside the game");
                    return realms_reply(
                        503,
                        "this Realms join waits for your approval: type the code into the phone's Realms app",
                        Some(seconds),
                    );
                }
                JoinStep::Deliver(answer) => {
                    say("200 here: approved — the game is given the one-time pass");
                    return Response::builder()
                        .status(200)
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::CACHE_CONTROL, "no-store")
                        .body(ReplyBody::local(Bytes::from(answer)))
                        .unwrap_or_else(|_| local_reply(500, "internal error"));
                }
                JoinStep::Refuse(reason) => return deny(403, &reason, false),
            },
            _ => None,
        };

        // The credential: the session cookie (and, for the frontend, x-mojangauth) rebuilt
        // around the real token, or nothing at all.
        let mut headers = forwardable(&parts.headers);
        for name in REALMS_HEADERS {
            if let Some(value) = parts.headers.get(name) {
                headers.push((HeaderName::from_static(name), value.clone()));
            }
        }
        let credentialed = !matches!(verdict, table::Verdict::Anonymous);
        let what = match &verdict {
            table::Verdict::Cookie(what) | table::Verdict::MojangAuth(what) => *what,
            table::Verdict::Join(_) => "join a Realm",
            _ => "",
        };
        if credentialed {
            let Some(credential) = &self.credential else {
                return deny(401, "there is no account in this session (demo mode)", false);
            };
            let token = credential.token.expose();
            if let table::Verdict::MojangAuth(_) = verdict {
                let presented = parts.headers.get("x-mojangauth").map(|v| v.as_bytes());
                if presented != Some(self.placeholder.as_bytes()) {
                    return deny(401, "no credential the launcher recognises was presented", false);
                }
                match sensitive_header("x-mojangauth", token) {
                    Ok(header) => headers.push(header),
                    Err(_) => return deny(500, "the credential could not be attached", false),
                }
            }
            match parts.headers.get(header::COOKIE) {
                Some(cookie) => {
                    match table::rebuild_cookie(cookie.as_bytes(), &self.placeholder, token, &credential.profile_id) {
                        Ok(rebuilt) => match sensitive_header("cookie", &rebuilt) {
                            Ok(header) => headers.push(header),
                            Err(_) => return deny(500, "the credential could not be attached", false),
                        },
                        Err(why) => {
                            let security = why.contains("different profile");
                            return deny(if security { 403 } else { 401 }, why, security);
                        }
                    }
                }
                None if matches!(verdict, table::Verdict::MojangAuth(_)) => {}
                None => return deny(401, "no Realms session cookie was presented", false),
            }
        }

        // The body, out of the budget like everything the broker holds.
        let no_room = "the launcher is already holding as much of this game's traffic as it will";
        let declared = body.size_hint().upper().unwrap_or(REALMS_MAX_BODY as u64);
        if declared > REALMS_MAX_BODY as u64 {
            return deny(413, "the request body is too large", false);
        }
        let request_room = match declared {
            0 => None,
            bytes => match self.buffers.reserve(bytes as usize).await {
                Some(room) => Some(room),
                None => return deny(503, no_room, false),
            },
        };
        let body = match Limited::new(body, REALMS_MAX_BODY).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return deny(413, "the request body is too large or was cut short", false),
        };
        let target = match query {
            Some(q) => format!("{path}?{q}"),
            None => path.clone(),
        };
        let body = (!body.is_empty() || method != Method::GET).then_some(body);
        let Some(mut reply_room) = self.buffers.reserve(MAX_RESPONSE_BODY as usize).await else {
            return deny(503, no_room, false);
        };
        let (head, mut payload) = match self.forward(upstream, host, method.clone(), &target, &headers, body).await {
            Ok(reply) => reply,
            Err(e) => {
                ctx.stats.broker_refused.fetch_add(1, Ordering::Relaxed);
                say(&format!("502 (upstream: {})", util::sanitize_line(e.to_string().as_bytes(), 200)));
                return realms_reply(502, "the request could not be relayed to Realms", None);
            }
        };
        drop(request_room);
        payload.shrink_to_fit();
        reply_room.shrink_to(payload.len());
        if credentialed {
            let encoding = head.headers.get(header::CONTENT_ENCODING).map(|v| v.as_bytes().trim_ascii());
            if encoding.is_some_and(|e| !e.is_empty() && !e.eq_ignore_ascii_case(b"identity")) {
                return deny(
                    502,
                    "the reply to a request that carried your credential came back encoded and was withheld",
                    true,
                );
            }
        }
        if let Some(credential) = &self.credential {
            let token = credential.token.expose();
            let leaked = contains(&payload, token)
                || RESPONSE_HEADERS
                    .iter()
                    .any(|name| head.headers.get(name).is_some_and(|v| contains(v.as_bytes(), token)));
            if leaked {
                return deny(502, "the reply contained the access token and was withheld", true);
            }
        }
        ctx.stats.brokered.fetch_add(1, Ordering::Relaxed);
        if credentialed {
            ctx.stats.credentialed.fetch_add(1, Ordering::Relaxed);
        }

        // The join: the address stays here; the game waits for the approval.
        if let (Some(world), 200) = (join, head.status) {
            let (address, object) = match table::parse_join_reply(&payload) {
                Ok(read) => read,
                Err(why) => return deny(502, why, false),
            };
            if let Ok(ip) = address.host.parse::<std::net::IpAddr>()
                && !realms.permits(ip)
            {
                return deny(502, &format!("Realms named a server at a non-public address ({address})"), true);
            }
            say(&format!(
                "200 from Realms [credential attached: {what}]; server {address} held back until you approve"
            ));
            let answer = table::rewrite_join_reply(object, realms.approvals.pass_address());
            return match realms.approvals.prompt(world, address, answer) {
                JoinStep::Wait(seconds) => realms_reply(
                    503,
                    "this Realms join waits for your approval: type the code into the phone's Realms app",
                    Some(seconds),
                ),
                JoinStep::Refuse(reason) => deny(403, &reason, false),
                _ => deny(500, "internal error", false),
            };
        }
        if head.status == 200
            && method == Method::GET
            && matches!(path.as_str(), "/worlds" | "/worlds/listUserWorldsOfType/any")
        {
            realms.approvals.note_names(&payload);
        }
        say(&format!(
            "{}{}",
            head.status,
            if credentialed { format!(" [credential attached: {what}]") } else { String::new() }
        ));
        let mut response = Response::builder().status(head.status);
        for name in &RESPONSE_HEADERS {
            if let Some(value) = head.headers.get(name) {
                response = response.header(name, value);
            }
        }
        response
            .body(ReplyBody { bytes: Bytes::from(payload), _room: Some(reply_room) })
            .unwrap_or_else(|_| realms_reply(502, "malformed upstream reply", None))
    }
}

/// For the robustness tests and fuzz targets: classify arbitrary request lines and read an
/// arbitrary join body, with and without an open game connection. Whatever the bytes are,
/// a join that is accepted is one this broker built: the real token, the session's own
/// profile, and a server id made of the characters a server id can have.
#[doc(hidden)]
pub fn fuzz_requests(data: &[u8]) {
    use std::sync::OnceLock;
    static BROKER: OnceLock<Option<Broker>> = OnceLock::new();
    let broker = BROKER.get_or_init(|| {
        let roots = tls::load_system_roots().ok()?;
        let http = HttpClient::new(tls::client_config(roots, tls::PinSet::default()).ok()?, &[]);
        let credential = Credential {
            token: Secret::from_slice(b"FUZZ.REAL.TOKEN").ok()?,
            profile_id: "069a79f444e94726a5befca90e38aaf5".into(),
        };
        Broker::new(
            Authority::generate().ok()?,
            &BrokerConfig::default(),
            &http,
            "bastion-placeholder-fuzz".into(),
            Some(credential),
        )
        .ok()
    });
    let Some(broker) = broker else { return };
    let text = String::from_utf8_lossy(data);
    for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        for host in ["api.minecraftservices.com", "sessionserver.mojang.com"] {
            for placeholder in [true, false] {
                let verdict = broker.classify(host, &method, &text, false, placeholder);
                // The token is only ever attached to a path that is exactly a known one.
                if let Verdict::Credentialed(_) | Verdict::Join = verdict {
                    assert!(path_is_canonical(&text), "credential for a non-canonical path {text:?}");
                    assert!(!text.to_ascii_lowercase().starts_with("/minecraft/profile"), "{text:?}");
                }
            }
        }
    }
    let gate = JoinGate::default();
    assert!(broker.join_body(&gate, data).is_err(), "a join without a game connection was accepted");
    let _connection = gate.opened("fuzz");
    if let Ok((body, connection)) = broker.join_body(&gate, data) {
        assert_eq!(connection, "fuzz", "the join was tied to the open game connection");
        // What goes out is exactly the object the broker wrote: the real token, the
        // session's profile, and a server hash made of hash characters only.
        let sent = std::str::from_utf8(&body).expect("ASCII");
        let prefix =
            r#"{"accessToken":"FUZZ.REAL.TOKEN","selectedProfile":"069a79f444e94726a5befca90e38aaf5","serverId":""#;
        let server_id = sent.strip_prefix(prefix).and_then(|rest| rest.strip_suffix("\"}")).unwrap_or_else(|| {
            panic!("the forwarded join is not the broker's own object: {sent}");
        });
        let hash = server_id.strip_prefix('-').unwrap_or(server_id);
        assert!((1..=40).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit()), "{server_id:?}");
        // One join per game connection.
        assert!(broker.join_body(&gate, data).is_err(), "the same game connection paid for two joins");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http() -> HttpClient {
        let config = tls::client_config(tls::load_system_roots().unwrap(), tls::PinSet::default()).unwrap();
        HttpClient::new(config, &[])
    }

    fn broker_with(config: BrokerConfig, signed_in: bool) -> Broker {
        let credential = signed_in.then(|| Credential {
            token: Secret::from_slice(b"REAL.ACCESS.TOKEN").unwrap(),
            profile_id: "069a79f444e94726a5befca90e38aaf5".into(),
        });
        Broker::new(Authority::generate().unwrap(), &config, &http(), "bastion-placeholder-test".into(), credential)
            .unwrap()
    }

    fn broker() -> Broker {
        broker_with(BrokerConfig::default(), true)
    }

    fn status(verdict: &Verdict) -> Option<(u16, bool)> {
        if let Verdict::Refuse { status, security, .. } = verdict { Some((*status, *security)) } else { None }
    }

    /// J13: nothing in the sandbox can change the skin, the cape or the player name —
    /// with every optional capability switched on, with or without the placeholder, in any
    /// spelling, on any path below the profile. No configuration switch opens these.
    #[test]
    fn skin_cape_and_name_changes_are_refused_under_every_configuration() {
        let everything =
            BrokerConfig { friends: true, settings: true, reports: true, telemetry: true, ..BrokerConfig::default() };
        for (config, signed_in) in [(BrokerConfig::default(), true), (everything.clone(), true), (everything, false)] {
            let b = broker_with(config, signed_in);
            for method in [Method::POST, Method::PUT, Method::DELETE] {
                for path in [
                    "/minecraft/profile",
                    "/minecraft/profile/skins",
                    "/minecraft/profile/skins/active",
                    "/minecraft/profile/capes",
                    "/minecraft/profile/capes/active",
                    "/minecraft/profile/name",
                    "/minecraft/profile/name/Hijacked",
                    "/minecraft/profile/anything/new",
                    "/Minecraft/Profile/Skins",
                    "/MINECRAFT/PROFILE/NAME/Hijacked",
                    "/minecraft/profile/capes/ACTIVE",
                ] {
                    for placeholder in [true, false] {
                        let verdict = b.classify(API, &method, path, false, placeholder);
                        assert_eq!(
                            status(&verdict),
                            Some((403, true)),
                            "{method} {path} (placeholder: {placeholder}, signed in: {signed_in}): {verdict:?}"
                        );
                    }
                }
                // Spellings a server might fold into the same path never get that far.
                for path in [
                    "/minecraft/profile/skins/",
                    "/minecraft//profile/skins",
                    "/minecraft/profile/./skins",
                    "/minecraft/x/../profile/skins",
                    "/minecraft/profile%2Fskins",
                    "/minecraft/profile/skins;x=1",
                ] {
                    let verdict = b.classify(API, &method, path, false, true);
                    let refused = matches!(status(&verdict), Some((400 | 403, _)));
                    assert!(refused, "{method} {path}: {verdict:?}");
                }
            }
            // An extra endpoint cannot be used to open one either: configuration parsing
            // is where that is decided.
        }
        for spec in [
            "PUT api.minecraftservices.com/minecraft/profile/name/x",
            "POST api.minecraftservices.com/minecraft/profile/skins",
            "DELETE api.minecraftservices.com/minecraft/profile/capes/active",
        ] {
            let config = BrokerConfig { extra_token_endpoints: vec![spec.to_string()], ..BrokerConfig::default() };
            let built =
                Broker::new(Authority::generate().unwrap(), &config, &http(), "bastion-placeholder-t".into(), None);
            match built {
                Err(_) => {}
                Ok(b) => {
                    let (method, _, path) = parse_endpoint(spec).unwrap();
                    let verdict = b.classify(API, &method, &path, false, true);
                    assert!(
                        !matches!(verdict, Verdict::Credentialed(_)),
                        "{spec}: an extra endpoint must not hand the token to a profile-changing request"
                    );
                }
            }
        }
    }

    /// The budget for buffered bodies: room is counted in whole units, the unused part of a
    /// reservation goes back, a request for more than is free waits and is then refused, and
    /// dropping a share returns it.
    #[tokio::test]
    async fn buffered_bodies_come_out_of_one_budget() {
        const MIB: usize = 1 << 20;
        let budget = Budget::new(10 * MIB, Duration::from_millis(50));
        assert_eq!(budget.in_use(), 0);
        let mut reply = budget.reserve(MAX_RESPONSE_BODY as usize).await.expect("room for one reply");
        assert_eq!(budget.in_use(), 8 * MIB);
        reply.shrink_to(5000);
        assert_eq!(budget.in_use(), 2 * BUDGET_UNIT, "5000 bytes occupy two units; the rest was given back");
        let second = budget.reserve(MAX_RESPONSE_BODY as usize).await.expect("room again");
        let started = Instant::now();
        assert!(budget.reserve(4 * MIB).await.is_none(), "8 MiB + 8 KiB are in use: 4 MiB more do not fit");
        assert!(started.elapsed() >= Duration::from_millis(50), "refused only after the wait");
        drop(second);
        let mut empty = budget.reserve(MIB).await.expect("room once a share was dropped");
        empty.shrink_to(0);
        drop(reply);
        assert_eq!(budget.in_use(), 0, "an empty reply occupies nothing");
        drop(empty);
        // More than the whole budget can never be had, and is not waited for for ever.
        assert!(budget.reserve(11 * MIB).await.is_none());

        // A reply is handed over in small frames, all of it, and says how long it is.
        let mut body = ReplyBody { bytes: Bytes::from(vec![7u8; 3 * REPLY_FRAME + 5]), _room: budget.reserve(1).await };
        assert_eq!(body.size_hint().exact(), Some((3 * REPLY_FRAME + 5) as u64));
        let mut frames = Vec::new();
        while let Some(frame) = body.frame().await {
            frames.push(frame.unwrap().into_data().unwrap().len());
        }
        assert_eq!(frames, [REPLY_FRAME, REPLY_FRAME, REPLY_FRAME, 5]);
        assert!(body.is_end_stream());
        assert_eq!(budget.in_use(), BUDGET_UNIT, "its room is held for as long as the body exists");
        drop(body);
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn interception_is_label_exact() {
        for yes in ["api.minecraftservices.com", "sessionserver.mojang.com", "mojang.com", "a.b.mojang.com"] {
            assert!(Broker::intercepts(yes), "{yes}");
        }
        for no in ["textures.minecraft.net", "pc.realms.minecraft.net", "evilmojang.com", "mojang.com.evil.example", ""]
        {
            assert!(!Broker::intercepts(no), "{no}");
        }
    }

    #[test]
    fn the_default_table_gives_the_game_what_it_needs_to_play_and_nothing_else() {
        let b = broker();
        let c = |host: &str, method: Method, path: &str| b.classify(host, &method, path, false, true);
        // On by default.
        assert_eq!(c(SESSION, Method::POST, JOIN_PATH), Verdict::Join);
        assert_eq!(c(API, Method::POST, "/player/certificates"), Verdict::Credentialed(Capability::ChatSigning));
        assert_eq!(c(API, Method::GET, "/player/attributes"), Verdict::Credentialed(Capability::AccountRead));
        assert_eq!(c(API, Method::GET, "/privacy/blocklist"), Verdict::Credentialed(Capability::AccountRead));
        // Off by default: they act on the account beyond playing.
        for (method, path, key) in [
            (Method::GET, "/friends", "friends"),
            (Method::PUT, "/friends", "friends"),
            (Method::POST, "/presence", "friends"),
            (Method::POST, "/player/attributes", "settings"),
            (Method::POST, "/player/report", "reports"),
            (Method::POST, "/events", "telemetry"),
        ] {
            let Verdict::Refuse { status: 403, reason, security: false } = c(API, method.clone(), path) else {
                panic!("{method} {path} should be refused by default")
            };
            assert!(reason.contains(&format!("network.broker.{key} = false")), "{reason}");
        }
        // Reads without credentials, and the two lookup POSTs, pass through anonymously.
        assert_eq!(c(API, Method::GET, "/publickeys"), Verdict::Anonymous);
        assert_eq!(c("discovery.minecraftservices.com", Method::GET, "/minecraft/client"), Verdict::Anonymous);
        assert_eq!(
            c(SESSION, Method::GET, "/session/minecraft/profile/069a79f444e94726a5befca90e38aaf5"),
            Verdict::Anonymous
        );
        assert_eq!(c("api.mojang.com", Method::POST, "/profiles/minecraft"), Verdict::Anonymous);
        assert_eq!(c(API, Method::POST, "/minecraft/profile/lookup/bulk/byname"), Verdict::Anonymous);
    }

    #[test]
    fn the_account_itself_cannot_be_changed_from_inside_the_game() {
        let b = broker_with(
            BrokerConfig { friends: true, settings: true, reports: true, telemetry: true, ..Default::default() },
            true,
        );
        for (method, path) in [
            (Method::POST, "/minecraft/profile/skins"),          // upload a skin
            (Method::DELETE, "/minecraft/profile/skins/active"), // reset it
            (Method::PUT, "/minecraft/profile/capes/active"),    // switch cape
            (Method::DELETE, "/minecraft/profile/capes/active"),
            (Method::PUT, "/minecraft/profile/name/Someone"),  // rename
            (Method::POST, "/minecraft/profile"),              // create a profile
            (Method::POST, "/authentication/login_with_xbox"), // mint a token
            (Method::POST, "/productvoucher/abc"),             // redeem a code
            (Method::POST, "/MINECRAFT/Profile/skins"),        // …in another spelling
        ] {
            for presents in [true, false] {
                let verdict = b.classify(API, &method, path, false, presents);
                assert_eq!(
                    status(&verdict),
                    Some((403, true)),
                    "{method} {path} (placeholder: {presents}): {verdict:?}"
                );
            }
        }
        // Reading the same paths carries no credential, so it can reveal nothing.
        assert_eq!(b.classify(API, &Method::GET, "/minecraft/profile", false, true), Verdict::Anonymous);
        assert_eq!(b.classify(API, &Method::GET, "/entitlements/mcstore", false, true), Verdict::Anonymous);
    }

    #[test]
    fn everything_unknown_that_changes_state_is_denied() {
        let b = broker();
        for (method, path) in [
            (Method::POST, "/some/new/endpoint"),
            (Method::PUT, "/player/attributes"),
            (Method::DELETE, "/friends"),
            (Method::DELETE, "/privacy/blocklist"),
        ] {
            assert_eq!(status(&b.classify(API, &method, path, false, false)), Some((403, false)), "{method} {path}");
            // …and doing so *with* the placeholder is flagged.
            assert_eq!(status(&b.classify(API, &method, path, false, true)), Some((403, true)), "{method} {path}");
        }
        for method in [Method::PATCH, Method::OPTIONS, Method::CONNECT, Method::TRACE, Method::HEAD] {
            assert_eq!(status(&b.classify(API, &method, "/publickeys", false, false)), Some((405, false)), "{method}");
        }
        // The same path on another host is not the credentialed endpoint.
        assert_eq!(b.classify("api.mojang.com", &Method::GET, "/player/attributes", false, true), Verdict::Anonymous);
        assert_eq!(
            status(&b.classify("api.mojang.com", &Method::POST, "/player/certificates", false, true)),
            Some((403, true))
        );
    }

    #[test]
    fn credentials_need_the_placeholder_an_exact_path_and_an_account() {
        let b = broker();
        // Without the placeholder: a read goes out bare, a write is refused.
        assert_eq!(b.classify(API, &Method::GET, "/player/attributes", false, false), Verdict::Anonymous);
        assert_eq!(status(&b.classify(API, &Method::POST, "/player/certificates", false, false)), Some((401, false)));
        // Query strings, other spellings and non-canonical paths never get a credential.
        assert_eq!(status(&b.classify(API, &Method::GET, "/player/attributes", true, true)), Some((400, false)));
        assert_eq!(status(&b.classify(API, &Method::GET, "/Player/Attributes", false, true)), Some((400, false)));
        for path in [
            "/player//attributes",
            "/player/./attributes",
            "/player/../player/attributes",
            "/player/attributes/..",
            "/player/%61ttributes",
            "/player/attributes;x=1",
            "/player/attributes\\",
            "player/attributes",
            "/player/attributes ",
        ] {
            assert_eq!(status(&b.classify(API, &Method::GET, path, false, true)), Some((400, false)), "{path:?}");
        }
        assert_eq!(
            b.classify(API, &Method::GET, "/player/attributes/", false, true),
            Verdict::Anonymous,
            "a different path"
        );
        // Demo mode: the table is the same, but there is nothing to attach.
        let demo = broker_with(BrokerConfig::default(), false);
        assert_eq!(status(&demo.classify(API, &Method::GET, "/player/attributes", false, true)), Some((401, false)));
        assert_eq!(demo.classify(API, &Method::GET, "/publickeys", false, false), Verdict::Anonymous);
    }

    #[test]
    fn capabilities_follow_the_configuration() {
        let b = broker_with(
            BrokerConfig {
                join: false,
                chat_signing: false,
                friends: true,
                telemetry: true,
                extra_token_endpoints: vec!["GET api.minecraftservices.com/player/mystery".into()],
                ..Default::default()
            },
            true,
        );
        assert_eq!(status(&b.classify(SESSION, &Method::POST, JOIN_PATH, false, true)), Some((403, false)));
        assert_eq!(status(&b.classify(API, &Method::POST, "/player/certificates", false, true)), Some((403, false)));
        assert_eq!(b.classify(API, &Method::GET, "/friends", false, true), Verdict::Credentialed(Capability::Friends));
        assert_eq!(
            b.classify(API, &Method::POST, "/events", false, true),
            Verdict::Credentialed(Capability::Telemetry)
        );
        assert_eq!(
            b.classify(API, &Method::GET, "/player/mystery", false, true),
            Verdict::Credentialed(Capability::Extra)
        );
        assert!(b.describe().contains("account_read") && !b.describe().contains("join,"));
    }

    #[test]
    fn extra_endpoints_are_validated() {
        assert_eq!(
            parse_endpoint("POST api.minecraftservices.com/player/thing").unwrap(),
            (Method::POST, "api.minecraftservices.com".to_string(), "/player/thing".to_string())
        );
        for bad in [
            "api.minecraftservices.com/player/thing", // no method
            "PATCH api.minecraftservices.com/x",      // method
            "GET example.com/x",                      // not a brokered domain
            "GET pc.realms.minecraft.net/worlds",     // passes through, cannot be brokered
            "GET api.minecraftservices.com",          // no path
            "GET api.minecraftservices.com/a?b=c",    // query
            "GET api.minecraftservices.com/a/../b",
            "PUT api.minecraftservices.com/minecraft/profile/capes/active",
            "POST sessionserver.mojang.com/session/minecraft/join",
        ] {
            assert!(parse_endpoint(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_join_is_rebuilt_around_the_real_token_and_needs_a_fresh_game_connection() {
        let b = broker();
        let gate = JoinGate::default();
        let request = br#"{"accessToken":"bastion-placeholder-test","selectedProfile":"069a79f4-44e9-4726-a5be-fca90e38aaf5","serverId":"-3f9a01bc"}"#;

        // No game connection: refused, and it is a security event.
        assert_eq!(status(&b.join_body(&gate, request).unwrap_err()), Some((403, true)));

        let connection = gate.opened("play.example.org:25565");
        let (body, label) = b.join_body(&gate, request).unwrap();
        assert_eq!(label, "play.example.org:25565");
        assert_eq!(
            &body[..],
            br#"{"accessToken":"REAL.ACCESS.TOKEN","selectedProfile":"069a79f444e94726a5befca90e38aaf5","serverId":"-3f9a01bc"}"#
        );
        // One join per connection.
        assert_eq!(status(&b.join_body(&gate, request).unwrap_err()), Some((403, true)));
        // A second connection allows a second join; closing it takes the allowance away.
        let second = gate.opened("play.example.org:25565");
        drop(second);
        assert!(b.join_body(&gate, request).is_err());
        drop(connection);

        let _open = gate.opened("x:1");
        for (bad, expected) in [
            (&br#"{"accessToken":"something-else","selectedProfile":"069a79f444e94726a5befca90e38aaf5","serverId":"ab"}"#[..], (401, false)),
            (br#"{"accessToken":"bastion-placeholder-test","selectedProfile":"00000000000000000000000000000000","serverId":"ab"}"#, (403, true)),
            (br#"{"accessToken":"bastion-placeholder-test","selectedProfile":"069a79f444e94726a5befca90e38aaf5","serverId":"zz"}"#, (400, false)),
            (br#"{"accessToken":"bastion-placeholder-test","selectedProfile":"069a79f444e94726a5befca90e38aaf5","serverId":""}"#, (400, false)),
            (br#"{"accessToken":"bastion-placeholder-test","selectedProfile":"069a79f444e94726a5befca90e38aaf5","serverId":"ab\",\"x\":\"y"}"#, (400, false)),
            (br#"{"accessToken":"bastion-placeholder-test","selectedProfile":"069a79f444e94726a5befca90e38aaf5","serverId":"ab","extra":1}"#, (400, false)),
            (br#"["not","an","object"]"#, (400, false)),
            (b"", (400, false)),
        ] {
            assert_eq!(status(&b.join_body(&gate, bad).unwrap_err()), Some(expected), "{}", String::from_utf8_lossy(bad));
        }
        // None of the rejected attempts used up the open connection.
        assert!(b.join_body(&gate, request).is_ok());

        let demo = broker_with(BrokerConfig::default(), false);
        let _c = gate.opened("y:1");
        assert_eq!(status(&demo.join_body(&gate, request).unwrap_err()), Some((401, false)));
    }

    #[test]
    fn the_authority_is_constrained_and_issues_leaves_for_single_names() {
        let authority = Authority::generate().unwrap();
        let (leaf, _key) = authority.issue("api.minecraftservices.com").unwrap();
        // The leaf validates against the authority for its own name only.
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(authority.certificate_der().to_vec())).unwrap();
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), tls::provider())
            .build()
            .unwrap();
        let check = |cert: &CertificateDer<'_>, name: &str| {
            use rustls::client::danger::ServerCertVerifier;
            let name = rustls::pki_types::ServerName::try_from(name.to_string()).unwrap();
            verifier.verify_server_cert(cert, &[], &name, &[], rustls::pki_types::UnixTime::now())
        };
        assert!(check(&leaf, "api.minecraftservices.com").is_ok());
        assert!(check(&leaf, "sessionserver.mojang.com").is_err(), "a leaf is for one name");
        // The name constraint: even a leaf this authority signed for another domain is void.
        let (stray, _key) = authority.issue("login.microsoftonline.com").unwrap();
        assert!(check(&stray, "login.microsoftonline.com").is_err(), "outside the permitted domains");
        // Two launches never share an authority.
        assert_ne!(authority.certificate_der(), Authority::generate().unwrap().certificate_der());
    }

    #[test]
    fn placeholders_are_unique_and_obviously_not_tokens() {
        let (a, b) = (new_placeholder().unwrap(), new_placeholder().unwrap());
        assert_ne!(a, b);
        assert!(a.starts_with("bastion-placeholder-") && a.len() == 52);
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'));
    }

    #[test]
    fn canonical_paths() {
        for ok in ["/", "/a", "/session/minecraft/profile/069a79f444e94726a5befca90e38aaf5", "/a.b/c-d_e~f", "/a/"] {
            assert!(path_is_canonical(ok), "{ok}");
        }
        for bad in ["", "a", "//", "/a//b", "/./a", "/a/..", "/a/%2e%2e/b", "/a?b", "/a#b", "/a b", "/a\\b", "/é"] {
            assert!(!path_is_canonical(bad), "{bad:?}");
        }
        assert!(!path_is_canonical(&format!("/{}", "a".repeat(600))));
    }
}
