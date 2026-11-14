//! A deliberately small HTTPS/1.1 client built on hyper's connection-level API.
//!
//! What it does not have is the point: no redirects (a 3xx is an error), no proxy support
//! (environment proxies are ignored), no cookies, no content decoding (so no decompression
//! bombs), no HTTP/2, no plaintext HTTP, no ports other than 443, and no host that is not on
//! the allowlist. Every response body is read under a caller-supplied size limit.

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{
    Method, Request, Uri,
    client::conn::http1,
    header::{self, HeaderName, HeaderValue},
};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, ProtocolVersion, pki_types::ServerName};
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::TlsConnector;

use crate::{
    error::{Error, Result},
    secret::Secret,
};

/// Every host the launcher itself will ever contact. Nothing else — no update checks, no
/// telemetry, no analytics endpoints exist in this binary.
pub const BUILTIN_HOSTS: &[&str] = &[
    // Microsoft identity platform (consumer tenant) and Xbox Live token services
    "login.microsoftonline.com",
    "user.auth.xboxlive.com",
    "xsts.auth.xboxlive.com",
    // Minecraft services: login_with_xbox, entitlements, profile
    "api.minecraftservices.com",
    // Mojang metadata and content
    "piston-meta.mojang.com",
    "launchermeta.mojang.com",
    "piston-data.mojang.com",
    "launcher.mojang.com",
    "libraries.minecraft.net",
    "resources.download.minecraft.net",
    "textures.minecraft.net",
    // Mod loader metadata and artifacts (only used for curated profiles)
    "meta.fabricmc.net",
    "maven.fabricmc.net",
    "meta.quiltmc.org",
    "maven.quiltmc.org",
];

const USER_AGENT: &str = "bastion-mc";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

pub type Headers = Vec<(HeaderName, HeaderValue)>;

/// Receives response body chunks.
pub trait Sink {
    fn accept(&mut self, chunk: &[u8]) -> Result<()>;
}

impl Sink for Vec<u8> {
    fn accept(&mut self, chunk: &[u8]) -> Result<()> {
        self.extend_from_slice(chunk);
        Ok(())
    }
}

/// Token-bearing responses are accumulated directly in locked memory.
impl Sink for Secret {
    fn accept(&mut self, chunk: &[u8]) -> Result<()> {
        self.push(chunk)
    }
}

/// Status line and headers of a response.
#[derive(Debug)]
pub struct Head {
    pub status: u16,
    pub headers: hyper::HeaderMap,
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub struct SecretReply {
    pub status: u16,
    pub body: Secret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    /// Origin-form request target: path plus optional query.
    pub path: String,
}

#[derive(Clone)]
pub struct HttpClient {
    tls: TlsConnector,
    hosts: Arc<BTreeSet<String>>,
    /// Domains whose every sub-domain is acceptable (the credential broker's upstreams).
    suffixes: Arc<Vec<String>>,
    /// Test hook: dial this address instead of resolving the host. Certificate validation
    /// against the host name is unaffected.
    dial_override: Option<SocketAddr>,
    /// Reach every host through this SOCKS5 proxy, which also resolves the name.
    socks5: Option<SocketAddr>,
}

/// Open a tunnel to `host:port` through the SOCKS5 proxy on `stream` (RFC 1928, no
/// authentication, CONNECT, address type "domain name"). The name is sent to the proxy as
/// it is: this process does not resolve it, so nothing about the destination is visible to
/// the local resolver or the local network.
async fn socks5_connect(stream: &mut TcpStream, host: &str, port: u16) -> std::result::Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let name = host.as_bytes();
    if name.is_empty() || name.len() > 255 {
        return Err("host name does not fit a SOCKS5 request".into());
    }
    let io = |e: std::io::Error| format!("talking to the SOCKS5 proxy: {e}");
    // Greeting: version 5, one method offered: 0 (no authentication).
    stream.write_all(&[5, 1, 0]).await.map_err(io)?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).await.map_err(io)?;
    if choice != [5, 0] {
        return Err("the proxy is not a SOCKS5 proxy that accepts unauthenticated clients".into());
    }
    let mut request = vec![5, 1, 0, 3, name.len() as u8];
    request.extend_from_slice(name);
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await.map_err(io)?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await.map_err(io)?;
    if head[0] != 5 {
        return Err("malformed reply from the SOCKS5 proxy".into());
    }
    if head[1] != 0 {
        let why = match head[1] {
            1 => "general failure",
            2 => "connection not allowed by the proxy's rules",
            3 => "network unreachable",
            4 => "host unreachable",
            5 => "connection refused",
            6 => "timed out",
            _ => "request rejected",
        };
        return Err(format!("the SOCKS5 proxy could not connect: {why}"));
    }
    // The bound address that follows is of no use here; it only has to be consumed.
    let rest = match head[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await.map_err(io)?;
            usize::from(len[0]) + 2
        }
        _ => return Err("malformed reply from the SOCKS5 proxy".into()),
    };
    let mut skip = [0u8; 257];
    stream.read_exact(&mut skip[..rest]).await.map_err(io)?;
    Ok(())
}

/// Connect to the SOCKS5 proxy at `proxy` and have it open a tunnel to `host:port`; what
/// comes back is a byte stream to that host. The one SOCKS5 client in this program: the
/// HTTPS client above uses it for the launcher's own requests, and the egress relay for the
/// connections it passes through to Mojang's services (`egress::relay_service`), so that
/// "through the proxy" means the same thing — name resolved by the proxy, never here — in
/// both places.
pub(crate) async fn socks5_dial(proxy: SocketAddr, host: &str, port: u16) -> std::result::Result<TcpStream, String> {
    let mut stream = match timeout(CONNECT_TIMEOUT, TcpStream::connect(proxy)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return Err(format!("cannot connect ({proxy}: {e})")),
        Err(_) => return Err(format!("cannot connect ({proxy}: connect timed out)")),
    };
    let _ = stream.set_nodelay(true);
    timeout(CONNECT_TIMEOUT, socks5_connect(&mut stream, host, port))
        .await
        .map_err(|_| "the SOCKS5 proxy did not answer in time".to_string())??;
    Ok(stream)
}

impl HttpClient {
    pub fn new(tls: Arc<ClientConfig>, extra_hosts: &[String]) -> Self {
        let hosts = BUILTIN_HOSTS
            .iter()
            .map(|h| h.to_string())
            .chain(extra_hosts.iter().map(|h| h.to_ascii_lowercase()))
            .collect();
        HttpClient {
            tls: TlsConnector::from(tls),
            hosts: Arc::new(hosts),
            suffixes: Arc::new(Vec::new()),
            dial_override: None,
            socks5: None,
        }
    }

    /// Route every connection through a SOCKS5 proxy (Tor, for instance). Names are
    /// resolved by the proxy. Everything else is unchanged: the TLS session is still end
    /// to end with the named host, TLS 1.3 only, with the same pins.
    pub fn with_socks5(mut self, proxy: Option<SocketAddr>) -> Self {
        self.socks5 = proxy;
        self
    }

    pub fn socks5(&self) -> Option<SocketAddr> {
        self.socks5
    }

    /// Also accept any host at or below these domains. Used by the credential broker, whose
    /// upstream is whatever Mojang service name the game resolved through the relay; every
    /// other user of this client keeps the exact-host allowlist.
    pub fn with_domains(mut self, domains: &[&str]) -> Self {
        self.suffixes = Arc::new(domains.iter().map(|d| d.to_ascii_lowercase()).collect());
        self
    }

    #[doc(hidden)]
    pub fn with_dial_override(mut self, addr: SocketAddr) -> Self {
        self.dial_override = Some(addr);
        self
    }

    /// Accepts exactly `https://<allowlisted-host>[:443]/path[?query]`.
    pub fn parse_url(&self, url: &str) -> Result<Target> {
        let uri: Uri = url.parse().map_err(|_| Error::Policy(format!("unparseable URL {url:?}")))?;
        if uri.scheme_str() != Some("https") {
            return Err(Error::Policy(format!("refusing non-HTTPS URL {url:?}")));
        }
        let authority = uri.authority().ok_or_else(|| Error::Policy(format!("URL without host: {url:?}")))?;
        if authority.as_str().contains('@') {
            return Err(Error::Policy(format!("URL with embedded credentials: {url:?}")));
        }
        if !matches!(uri.port_u16(), None | Some(443)) {
            return Err(Error::Policy(format!("refusing non-443 port in {url:?}")));
        }
        let host = authority.host().to_ascii_lowercase();
        self.check_host(&host)?;
        let path = uri.path_and_query().map(|p| p.as_str()).filter(|p| !p.is_empty()).unwrap_or("/").to_string();
        Ok(Target { host, path })
    }

    pub fn check_host(&self, host: &str) -> Result<()> {
        let under_domain = |domain: &String| {
            host == domain || host.strip_suffix(domain.as_str()).is_some_and(|prefix| prefix.ends_with('.'))
        };
        if self.hosts.contains(host) || self.suffixes.iter().any(under_domain) {
            Ok(())
        } else {
            Err(Error::Policy(format!(
                "host {host:?} is not on the launcher's outbound allowlist (add it to downloads.extra_hosts if intended)"
            )))
        }
    }

    /// Open a keep-alive connection. TLS 1.3 is asserted after the handshake.
    pub async fn connect(&self, host: &str) -> Result<Connection> {
        self.check_host(host)?;
        let net = |m: String| Error::Net(format!("{host}: {m}"));

        let tcp = match (self.socks5, self.dial_override) {
            // The proxy resolves the name; this process must not.
            (Some(proxy), _) => socks5_dial(proxy, host, 443).await.map_err(net)?,
            (None, dial) => {
                let addrs: Vec<SocketAddr> = match dial {
                    Some(addr) => vec![addr],
                    None => timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, 443)))
                        .await
                        .map_err(|_| net("DNS lookup timed out".into()))?
                        .map_err(|e| net(format!("DNS lookup failed: {e}")))?
                        .collect(),
                };
                let mut tcp = None;
                let mut last_err = "no addresses".to_string();
                for addr in addrs {
                    match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
                        Ok(Ok(s)) => {
                            tcp = Some(s);
                            break;
                        }
                        Ok(Err(e)) => last_err = format!("{addr}: {e}"),
                        Err(_) => last_err = format!("{addr}: connect timed out"),
                    }
                }
                let tcp = tcp.ok_or_else(|| net(format!("cannot connect ({last_err})")))?;
                let _ = tcp.set_nodelay(true);
                tcp
            }
        };

        let name = ServerName::try_from(host.to_owned()).map_err(|_| net("not a valid TLS server name".into()))?;
        let tls = timeout(CONNECT_TIMEOUT, self.tls.connect(name, tcp))
            .await
            .map_err(|_| Error::Tls(format!("{host}: handshake timed out")))?
            .map_err(|e| Error::Tls(format!("{host}: {e}")))?;
        if tls.get_ref().1.protocol_version() != Some(ProtocolVersion::TLSv1_3) {
            return Err(Error::Tls(format!("{host}: negotiated something other than TLS 1.3")));
        }

        let (sender, conn) = http1::Builder::new()
            .handshake::<_, Full<Bytes>>(TokioIo::new(tls))
            .await
            .map_err(|e| net(format!("HTTP handshake failed: {e}")))?;
        // The driver ends when the sender is dropped or the peer closes.
        tokio::spawn(async move {
            let _ = conn.await;
        });
        Ok(Connection { host: host.to_owned(), sender })
    }

    /// One request on a fresh connection.
    pub async fn request(
        &self,
        method: Method,
        url: &str,
        headers: &Headers,
        body: Option<Bytes>,
        limit: u64,
    ) -> Result<Reply> {
        let target = self.parse_url(url)?;
        let mut out = Vec::new();
        let head = self.connect(&target.host).await?.call(method, &target.path, headers, body, limit, &mut out).await?;
        Ok(Reply { status: head.status, body: out })
    }

    /// Like [`request`](Self::request) but the response body never touches ordinary heap
    /// memory that we own: it is collected straight into an mlock'd [`Secret`].
    pub async fn request_secret(
        &self,
        method: Method,
        url: &str,
        headers: &Headers,
        body: Option<Bytes>,
        limit: usize,
    ) -> Result<SecretReply> {
        let target = self.parse_url(url)?;
        let mut out = Secret::with_capacity(limit)?;
        let head =
            self.connect(&target.host).await?.call(method, &target.path, headers, body, limit as u64, &mut out).await?;
        Ok(SecretReply { status: head.status, body: out })
    }

    /// GET that must answer 200.
    pub async fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let reply = self.request(Method::GET, url, &Vec::new(), None, limit).await?;
        if reply.status != 200 {
            return Err(Error::Net(format!("GET {url}: HTTP {}", reply.status)));
        }
        Ok(reply.body)
    }
}

pub struct Connection {
    host: String,
    sender: http1::SendRequest<Full<Bytes>>,
}

impl Connection {
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Has the peer (or an error) closed this connection?
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// Issue one request and stream the body into `sink`, enforcing `limit` bytes.
    /// Returns status and headers; the caller decides what a non-200 means. After an error
    /// the connection must be dropped.
    pub async fn call(
        &mut self,
        method: Method,
        path: &str,
        headers: &Headers,
        body: Option<Bytes>,
        limit: u64,
        sink: &mut (dyn Sink + Send),
    ) -> Result<Head> {
        let host = self.host.clone();
        let net = |m: String| Error::Net(format!("{host}{path}: {m}"));

        timeout(IDLE_TIMEOUT, self.sender.ready())
            .await
            .map_err(|_| net("connection stalled".into()))?
            .map_err(|e| net(format!("connection closed: {e}")))?;

        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, self.host.as_str())
            // No transfer coding we would have to decode; a server that ignores this and
            // compresses anyway simply fails hash verification.
            .header(header::ACCEPT_ENCODING, "identity");
        // The launcher's own requests identify as the launcher; a request relayed for the
        // game keeps the game's identification.
        if !headers.iter().any(|(k, _)| k == header::USER_AGENT) {
            builder = builder.header(header::USER_AGENT, USER_AGENT);
        }
        for (k, v) in headers {
            builder = builder.header(k, v);
        }
        let request =
            builder.body(Full::new(body.unwrap_or_default())).map_err(|e| net(format!("invalid request: {e}")))?;

        let response = timeout(IDLE_TIMEOUT, self.sender.send_request(request))
            .await
            .map_err(|_| net("timed out waiting for response".into()))?
            .map_err(|e| net(format!("request failed: {e}")))?;
        let status = response.status().as_u16();
        let response_headers = response.headers().clone();

        let declared = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if declared.is_some_and(|len| len > limit) {
            return Err(net(format!("response of {} bytes exceeds the {limit} byte limit", declared.unwrap_or(0))));
        }

        let mut body = response.into_body();
        let mut total = 0u64;
        loop {
            let frame = match timeout(IDLE_TIMEOUT, body.frame()).await {
                Err(_) => return Err(net("response body stalled".into())),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(net(format!("response body error: {e}"))),
                Ok(Some(Ok(f))) => f,
            };
            if let Ok(data) = frame.into_data() {
                total += data.len() as u64;
                if total > limit {
                    return Err(net(format!("response exceeds the {limit} byte limit")));
                }
                sink.accept(&data)?;
            }
        }
        Ok(Head { status, headers: response_headers })
    }
}

/// `application/x-www-form-urlencoded` body. Values may be secrets, so the buffer is built
/// once at its final size; it is wiped by the allocator when freed.
pub fn form_body(pairs: &[(&str, &[u8])]) -> Bytes {
    fn enc(out: &mut Vec<u8>, raw: &[u8]) {
        for &b in raw {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b),
                _ => {
                    const HEX: &[u8; 16] = b"0123456789ABCDEF";
                    out.extend_from_slice(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
                }
            }
        }
    }
    let cap: usize = pairs.iter().map(|(k, v)| k.len() * 3 + v.len() * 3 + 2).sum();
    let mut out = Vec::with_capacity(cap);
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(b'&');
        }
        enc(&mut out, k.as_bytes());
        out.push(b'=');
        enc(&mut out, v);
    }
    Bytes::from(out)
}

pub fn header(name: &'static str, value: &str) -> Result<(HeaderName, HeaderValue)> {
    let v = HeaderValue::from_str(value).map_err(|_| Error::Other(format!("invalid value for header {name}")))?;
    Ok((HeaderName::from_static(name), v))
}

/// A header whose value is a credential: flagged so hyper never prints it in debug output.
pub fn sensitive_header(name: &'static str, value: &[u8]) -> Result<(HeaderName, HeaderValue)> {
    let mut v = HeaderValue::from_bytes(value).map_err(|_| Error::Other(format!("invalid value for header {name}")))?;
    v.set_sensitive(true);
    Ok((HeaderName::from_static(name), v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::tls;

    fn client() -> HttpClient {
        let cfg = tls::client_config(tls::load_system_roots().unwrap(), tls::PinSet::default()).unwrap();
        HttpClient::new(cfg, &["extra.example.org".to_string()])
    }

    #[test]
    fn url_policy() {
        let c = client();
        let t = c.parse_url("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json").unwrap();
        assert_eq!(t.host, "piston-meta.mojang.com");
        assert_eq!(t.path, "/mc/game/version_manifest_v2.json");
        assert_eq!(c.parse_url("https://LIBRARIES.minecraft.net:443/a?b=c").unwrap().path, "/a?b=c");
        assert!(c.parse_url("https://extra.example.org/x").is_ok());
        // Domain allowances are opt-in and label-exact.
        assert!(c.check_host("sessionserver.mojang.com").is_err());
        let broker = c.clone().with_domains(&["mojang.com", "minecraftservices.com"]);
        for ok in ["sessionserver.mojang.com", "mojang.com", "a.b.minecraftservices.com", "piston-meta.mojang.com"] {
            assert!(broker.check_host(ok).is_ok(), "{ok}");
        }
        for bad in ["evilmojang.com", "mojang.com.evil.example", "minecraft.net.evil.example", "example.com"] {
            assert!(broker.check_host(bad).is_err(), "{bad}");
        }
        for bad in [
            "http://piston-meta.mojang.com/x",              // plaintext
            "https://evil.example/x",                       // not allowlisted
            "https://piston-meta.mojang.com:8443/x",        // odd port
            "https://user:pw@piston-meta.mojang.com/x",     // credentials
            "https://piston-meta.mojang.com.evil.example/", // suffix trick
            "ftp://piston-meta.mojang.com/x",
            "/relative",
            "",
        ] {
            assert!(c.parse_url(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn form_encoding() {
        let b = form_body(&[
            ("grant_type", b"refresh_token"),
            ("scope", b"XboxLive.signin offline_access"),
            ("x", b"a&b=c/+"),
        ]);
        assert_eq!(&b[..], b"grant_type=refresh_token&scope=XboxLive.signin%20offline_access&x=a%26b%3Dc%2F%2B");
    }
}
