//! Microsoft identity platform, consumer tenant, public client (no client secret).
//!
//! Authorization Code flow hardening:
//!  * PKCE with S256; the verifier never leaves this process except in the token request.
//!  * The redirect listener binds loopback only, on a kernel-chosen ephemeral port, *before*
//!    the authorization URL exists.
//!  * `state` is 256 bits from the kernel CSPRNG and is compared in constant time **before**
//!    any other parameter of a callback is looked at. Any local process (of any user) can
//!    connect to the port; without the state value it can neither inject a code nor end the
//!    wait with a forged error.
//!  * The callback response is a constant page; nothing from the request is reflected.

use std::time::Duration;

use bytes::Bytes;
use hyper::Method;
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Instant, timeout},
};

use super::{Api, ApiRequest, clean};
use crate::{
    config::AuthFlow,
    error::{Context, Error, Result},
    events::{Event, Reporter},
    net::http::{form_body, header},
    secret::Secret,
    util::{self, HashAlg},
};

const AUTHORITY: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const SCOPE: &str = "XboxLive.signin offline_access";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_STRAY_CONNECTIONS: usize = 200;

pub struct MsaTokens {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Secret,
    refresh_token: Option<Secret>,
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
    error_description: Option<String>,
}

impl OAuthError {
    fn describe(&self) -> String {
        // Only the first line: the rest is trace/correlation ids.
        let desc = self.error_description.as_deref().unwrap_or("").lines().next().unwrap_or("");
        format!("{} {}", clean(&self.error), clean(desc))
    }
}

async fn token_request(api: &impl Api, form: &[(&str, &[u8])]) -> Result<std::result::Result<MsaTokens, OAuthError>> {
    let url = format!("{AUTHORITY}/token");
    let headers =
        vec![header("content-type", "application/x-www-form-urlencoded")?, header("accept", "application/json")?];
    let reply = api.call(ApiRequest { method: Method::POST, url: &url, headers, body: Some(form_body(form)) }).await?;
    if reply.status == 200 {
        let t: TokenReply = serde_json::from_slice(reply.body.expose())
            .map_err(|_| Error::Auth("Microsoft token endpoint: unexpected response format".into()))?;
        return Ok(Ok(MsaTokens { access_token: t.access_token, refresh_token: t.refresh_token }));
    }
    match serde_json::from_slice::<OAuthError>(reply.body.expose()) {
        Ok(e) => Ok(Err(e)),
        Err(_) => Err(Error::Auth(format!("Microsoft token endpoint: HTTP {}", reply.status))),
    }
}

fn explain(e: &OAuthError) -> Error {
    let hint = match e.error.as_str() {
        "invalid_grant" => " — the stored sign-in has expired or was revoked; run `bastion-mc login`",
        "invalid_client" | "unauthorized_client" => {
            " — check the Azure app: it must allow personal Microsoft accounts and be a public client \
             (\"Allow public client flows\" for device code; redirect URI http://localhost under \
             \"Mobile and desktop applications\" for PKCE)"
        }
        _ => "",
    };
    Error::Auth(format!("Microsoft sign-in failed: {}{hint}", e.describe()))
}

/// Refresh-token grant.
pub async fn refresh(api: &impl Api, client_id: &str, refresh_token: &Secret) -> Result<MsaTokens> {
    token_request(
        api,
        &[
            ("client_id", client_id.as_bytes()),
            ("grant_type", b"refresh_token"),
            ("refresh_token", refresh_token.expose()),
            ("scope", SCOPE.as_bytes()),
        ],
    )
    .await?
    .map_err(|e| explain(&e))
}

// ---------------------------------------------------------------- authorization code + PKCE

pub struct Pkce {
    pub verifier: Secret,
    pub challenge: String,
}

pub fn pkce() -> Result<Pkce> {
    let raw = util::random_bytes::<32>()?;
    let verifier = util::b64url_encode(&raw);
    let challenge = util::b64url_encode(&util::hash_bytes(HashAlg::Sha256, verifier.as_bytes()));
    Ok(Pkce { verifier: Secret::from_slice(verifier.as_bytes())?, challenge })
}

pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let query = form_body(&[
        ("client_id", client_id.as_bytes()),
        ("response_type", b"code"),
        ("redirect_uri", redirect_uri.as_bytes()),
        ("response_mode", b"query"),
        ("scope", SCOPE.as_bytes()),
        ("state", state.as_bytes()),
        ("code_challenge", challenge.as_bytes()),
        ("code_challenge_method", b"S256"),
        ("prompt", b"select_account"),
    ]);
    format!("{AUTHORITY}/authorize?{}", String::from_utf8_lossy(&query))
}

#[derive(Debug)]
pub enum Callback {
    Code(Secret),
    /// The identity provider reported an error for *our* request (state matched).
    Denied(String),
    /// Not our callback: wrong or missing state, favicon request, port scanner, …
    Unrelated(&'static str),
}

fn percent_decode(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(out)
}

/// Interpret the head of an HTTP request received on the loopback port.
pub fn parse_callback(head: &[u8], expected_state: &str) -> Callback {
    let Ok(text) = std::str::from_utf8(head) else { return Callback::Unrelated("not text") };
    let mut parts = text.lines().next().unwrap_or_default().split(' ');
    let (Some("GET"), Some(target), Some(_version)) = (parts.next(), parts.next(), parts.next()) else {
        return Callback::Unrelated("not a GET request");
    };
    let Some((path, query)) = target.split_once('?') else { return Callback::Unrelated("no query") };
    if path != "/" {
        return Callback::Unrelated("unexpected path");
    }
    let (mut state, mut code, mut error, mut description) = (None, None, None, None);
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else { continue };
        let Some(value) = percent_decode(value) else { return Callback::Unrelated("bad encoding") };
        match key {
            "state" if state.is_none() => state = Some(value),
            "code" if code.is_none() => code = Some(value),
            "error" if error.is_none() => error = Some(value),
            "error_description" if description.is_none() => description = Some(value),
            // A repeated parameter is ambiguous; refuse to pick one.
            "state" | "code" | "error" | "error_description" => return Callback::Unrelated("duplicate parameter"),
            _ => {}
        }
    }
    // State first, in constant time, before anything else is believed.
    match state {
        Some(s) if util::ct_eq(&s, expected_state.as_bytes()) => {}
        _ => return Callback::Unrelated("state mismatch"),
    }
    if let Some(e) = error {
        let d = description.unwrap_or_default();
        return Callback::Denied(format!("{} {}", util::sanitize_line(&e, 80), util::sanitize_line(&d, 300)));
    }
    match code {
        Some(c) if !c.is_empty() && c.len() <= 8192 => match Secret::from_slice(&c) {
            Ok(s) => Callback::Code(s),
            Err(_) => Callback::Unrelated("cannot store code"),
        },
        _ => Callback::Unrelated("no code"),
    }
}

fn page(status: &str, body: &str) -> Vec<u8> {
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>bastion-mc</title><body style=\"font-family:sans-serif;margin:3em\"><p>{body}</p>"
    );
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\n\r\n{html}",
        html.len()
    )
    .into_bytes()
}

pub struct Loopback {
    v4: TcpListener,
    /// Browsers may resolve `localhost` to ::1 first.
    v6: Option<TcpListener>,
    pub port: u16,
    pub state: String,
}

impl Loopback {
    pub async fn bind() -> Result<Self> {
        let v4 = TcpListener::bind(("127.0.0.1", 0)).await.ctx(|| "binding the loopback redirect listener")?;
        let port = v4.local_addr().ctx(|| "reading listener address")?.port();
        let v6 = TcpListener::bind(("::1", port)).await.ok();
        let state = util::b64url_encode(&util::random_bytes::<32>()?);
        Ok(Loopback { v4, v6, port, state })
    }

    /// Microsoft matches loopback redirect URIs on scheme, host and path, ignoring the port,
    /// so registering `http://localhost` covers any ephemeral port.
    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Wait for the browser to deliver the authorization code.
    pub async fn wait(self, limit: Duration) -> Result<Secret> {
        let deadline = Instant::now() + limit;
        let mut stray = 0usize;
        loop {
            let accept = async {
                match &self.v6 {
                    Some(v6) => tokio::select! { a = self.v4.accept() => a, b = v6.accept() => b },
                    None => self.v4.accept().await,
                }
            };
            let (mut stream, _) = match tokio::time::timeout_at(deadline, accept).await {
                Err(_) => return Err(Error::Auth("timed out waiting for the browser sign-in".into())),
                Ok(Err(_)) => continue,
                Ok(Ok(conn)) => conn,
            };
            if let Some(head) = timeout(Duration::from_secs(5), read_head(&mut stream)).await.ok().flatten() {
                match parse_callback(&head, &self.state) {
                    Callback::Code(code) => {
                        let _ = stream
                            .write_all(&page("200 OK", "Signed in. You can close this tab and return to the terminal."))
                            .await;
                        let _ = stream.shutdown().await;
                        return Ok(code);
                    }
                    Callback::Denied(why) => {
                        let _ = stream
                            .write_all(&page(
                                "200 OK",
                                "Sign-in was not completed. Return to the terminal for details.",
                            ))
                            .await;
                        let _ = stream.shutdown().await;
                        return Err(Error::Auth(format!("Microsoft sign-in was refused: {why}")));
                    }
                    Callback::Unrelated(_) => {
                        let _ = stream
                            .write_all(&page(
                                "400 Bad Request",
                                "This is not the sign-in response bastion-mc is waiting for.",
                            ))
                            .await;
                    }
                }
            }
            stray += 1;
            if stray > MAX_STRAY_CONNECTIONS {
                return Err(Error::Auth("too many unrelated connections on the sign-in port; aborting".into()));
            }
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(2048);
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            buf.truncate(end);
            return Some(buf);
        }
        if buf.len() > 16 * 1024 {
            return None;
        }
        let mut chunk = [0u8; 2048];
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

async fn redeem_code(
    api: &impl Api,
    client_id: &str,
    code: &Secret,
    redirect_uri: &str,
    verifier: &Secret,
) -> Result<MsaTokens> {
    token_request(
        api,
        &[
            ("client_id", client_id.as_bytes()),
            ("grant_type", b"authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", redirect_uri.as_bytes()),
            ("code_verifier", verifier.expose()),
            ("scope", SCOPE.as_bytes()),
        ],
    )
    .await?
    .map_err(|e| explain(&e))
}

// ---------------------------------------------------------------- device code

#[derive(Deserialize)]
struct DeviceReply {
    device_code: Secret,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    interval: u64,
}

async fn device_flow(api: &impl Api, client_id: &str, reporter: &Reporter) -> Result<MsaTokens> {
    let url = format!("{AUTHORITY}/devicecode");
    let headers =
        vec![header("content-type", "application/x-www-form-urlencoded")?, header("accept", "application/json")?];
    let body = form_body(&[("client_id", client_id.as_bytes()), ("scope", SCOPE.as_bytes())]);
    let reply = api.call(ApiRequest { method: Method::POST, url: &url, headers, body: Some(body) }).await?;
    if reply.status != 200 {
        return Err(match serde_json::from_slice::<OAuthError>(reply.body.expose()) {
            Ok(e) => explain(&e),
            Err(_) => Error::Auth(format!("device code request failed: HTTP {}", reply.status)),
        });
    }
    let device: DeviceReply = serde_json::from_slice(reply.body.expose())
        .map_err(|_| Error::Auth("device code request: unexpected response format".into()))?;
    if !device.verification_uri.starts_with("https://") {
        return Err(Error::Auth("device code request returned a non-HTTPS verification URL".into()));
    }
    // The code is shown to the user and may be put on a clipboard by a front end: accept
    // only the shape Microsoft issues.
    let code_ok = (4..=32).contains(&device.user_code.len())
        && device.user_code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !code_ok {
        return Err(Error::Auth("device code request returned a malformed user code".into()));
    }
    reporter.emit(Event::DeviceCode {
        user_code: device.user_code.clone(),
        verification_uri: clean(&device.verification_uri),
        expires_in: device.expires_in.clamp(60, 1800),
    });

    let mut interval = Duration::from_secs(device.interval.clamp(1, 60));
    let deadline = Instant::now() + Duration::from_secs(device.expires_in.clamp(60, 1800));
    loop {
        tokio::time::sleep(interval).await;
        if Instant::now() >= deadline {
            return Err(Error::Auth("the device code expired before sign-in completed".into()));
        }
        let outcome = token_request(
            api,
            &[
                ("client_id", client_id.as_bytes()),
                ("grant_type", b"urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device.device_code.expose()),
            ],
        )
        .await?;
        match outcome {
            Ok(tokens) => return Ok(tokens),
            Err(e) if e.error == "authorization_pending" => {}
            Err(e) if e.error == "slow_down" => interval += Duration::from_secs(5),
            Err(e) => return Err(explain(&e)),
        }
    }
}

/// Interactive sign-in. What the user has to do (a code to type, a link to open) is handed
/// to `reporter` as an event; a terminal prints it, a front end displays it.
pub async fn interactive_login(
    api: &impl Api,
    client_id: &str,
    flow: AuthFlow,
    reporter: &Reporter,
) -> Result<MsaTokens> {
    match flow {
        AuthFlow::Device => device_flow(api, client_id, reporter).await,
        AuthFlow::Pkce => {
            let loopback = Loopback::bind().await?;
            let pkce = pkce()?;
            let redirect = loopback.redirect_uri();
            reporter.emit(Event::BrowserLogin {
                url: authorize_url(client_id, &redirect, &loopback.state, &pkce.challenge),
                timeout_secs: LOGIN_TIMEOUT.as_secs(),
            });
            let code = loopback.wait(LOGIN_TIMEOUT).await?;
            redeem_code(api, client_id, &code, &redirect, &pkce.verifier).await
        }
    }
}

/// Keep `Bytes` in the public surface honest for callers that build bodies themselves.
#[allow(dead_code)]
fn _assert_body_type(_: Bytes) {}

#[cfg(test)]
mod tests {
    use super::{super::testing::Scripted, *};

    #[test]
    fn pkce_follows_rfc7636() {
        let p = pkce().unwrap();
        let v = p.verifier.expose_str().unwrap();
        assert_eq!(v.len(), 43);
        assert!(v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_eq!(p.challenge, util::b64url_encode(&util::hash_bytes(HashAlg::Sha256, v.as_bytes())));
        // RFC 7636 appendix B test vector.
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            util::b64url_encode(&util::hash_bytes(HashAlg::Sha256, v.as_bytes())),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_ne!(pkce().unwrap().challenge, p.challenge);
    }

    #[test]
    fn authorize_url_shape() {
        let url = authorize_url("01234567-89ab-cdef-0123-456789abcdef", "http://localhost:43210", "ST4TE", "CH4LL");
        assert!(url.starts_with("https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?"));
        for part in [
            "client_id=01234567-89ab-cdef-0123-456789abcdef",
            "response_type=code",
            "redirect_uri=http%3A%2F%2Flocalhost%3A43210",
            "scope=XboxLive.signin%20offline_access",
            "state=ST4TE",
            "code_challenge=CH4LL",
            "code_challenge_method=S256",
        ] {
            assert!(url.contains(part), "missing {part} in {url}");
        }
        assert!(!url.contains("client_secret"));
    }

    #[test]
    fn callback_state_is_checked_before_everything_else() {
        let ok = parse_callback(b"GET /?code=M.C5_BAY.abc%2Fdef&state=GOOD HTTP/1.1\r\nHost: localhost", "GOOD");
        assert!(matches!(&ok, Callback::Code(c) if c.expose() == b"M.C5_BAY.abc/def"), "{ok:?}");
        // wrong / missing state: never a code, never an error that would end the wait
        for forged in [
            &b"GET /?code=EVIL&state=BAD HTTP/1.1"[..],
            b"GET /?code=EVIL HTTP/1.1",
            b"GET /?error=access_denied&state=BAD HTTP/1.1",
            b"GET /?error=access_denied&error_description=stop HTTP/1.1",
            b"GET /?code=EVIL&state=GOOD&state=BAD HTTP/1.1",
            b"GET /?code=A&code=B&state=GOOD HTTP/1.1",
            b"GET /?state=GOO HTTP/1.1",
            b"GET /?state=GOODX&code=EVIL HTTP/1.1",
            b"GET /favicon.ico HTTP/1.1",
            b"GET /other?code=EVIL&state=GOOD HTTP/1.1",
            b"POST /?code=EVIL&state=GOOD HTTP/1.1",
            b"GET /?code=%ZZ&state=GOOD HTTP/1.1",
            b"GET /?state=GOOD HTTP/1.1",
            b"\xff\xfe\xfd",
            b"",
        ] {
            assert!(
                matches!(parse_callback(forged, "GOOD"), Callback::Unrelated(_)),
                "{:?}",
                String::from_utf8_lossy(forged)
            );
        }
        // a genuine provider error for our state ends the flow, with sanitised text
        let denied = parse_callback(
            b"GET /?error=access_denied&error_description=The+user%1b%5b31m+declined&state=GOOD HTTP/1.1",
            "GOOD",
        );
        match denied {
            Callback::Denied(msg) => {
                assert!(msg.contains("access_denied") && msg.contains("declined"));
                assert!(!msg.contains('\x1b'), "escape sequences must be stripped");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn loopback_ignores_forgeries_and_accepts_the_real_callback() {
        let lb = Loopback::bind().await.unwrap();
        let (port, state) = (lb.port, lb.state.clone());
        assert!(lb.redirect_uri().starts_with("http://localhost:"));
        let waiter = tokio::spawn(lb.wait(Duration::from_secs(20)));

        async fn hit(port: u16, request: String) -> String {
            let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            s.write_all(request.as_bytes()).await.unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out).await;
            out
        }
        // A local attacker without the state value cannot complete or abort the flow.
        let r = hit(port, "GET /?code=ATTACKER&state=guess HTTP/1.1\r\nHost: localhost\r\n\r\n".into()).await;
        assert!(r.starts_with("HTTP/1.1 400"), "{r}");
        let r = hit(port, "GET /?error=access_denied&state=guess HTTP/1.1\r\n\r\n".into()).await;
        assert!(r.starts_with("HTTP/1.1 400"));
        assert!(!waiter.is_finished());
        // The genuine redirect.
        let r =
            hit(port, format!("GET /?code=REAL-CODE&state={state} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n")).await;
        assert!(r.starts_with("HTTP/1.1 200"));
        assert!(!r.contains("REAL-CODE"), "nothing from the request may be reflected");
        assert!(r.contains("default-src 'none'"));
        let code = waiter.await.unwrap().unwrap();
        assert_eq!(code.expose(), b"REAL-CODE");
    }

    #[tokio::test]
    async fn device_flow_polls_until_authorised() {
        let api = Scripted::new(vec![
            (
                "/devicecode",
                200,
                r#"{"device_code":"DEV-CODE","user_code":"ABCD-EFGH","verification_uri":"https://www.microsoft.com/link","expires_in":900,"interval":1}"#,
            ),
            ("/token", 400, r#"{"error":"authorization_pending","error_description":"waiting"}"#),
            ("/token", 200, r#"{"access_token":"MSA-ACCESS","refresh_token":"MSA-REFRESH"}"#),
        ]);
        let reporter = Reporter::capture();
        let tokens = device_flow(&api, "01234567-89ab-cdef-0123-456789abcdef", &reporter).await.unwrap();
        assert_eq!(tokens.refresh_token.unwrap().expose(), b"MSA-REFRESH");
        assert_eq!(
            reporter.captured(),
            vec![Event::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://www.microsoft.com/link".into(),
                expires_in: 900,
            }],
            "the prompt is structured: a front end shows the code, it does not parse prose"
        );
        let reqs = api.requests();
        assert_eq!(reqs.len(), 3);
        assert!(
            reqs[1].2.contains("device_code=DEV-CODE")
                && reqs[1].2.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code")
        );
    }

    #[tokio::test]
    async fn device_flow_stops_on_refusal_and_rejects_plain_http_links() {
        let api = Scripted::new(vec![
            (
                "/devicecode",
                200,
                r#"{"device_code":"D","user_code":"USER-1","verification_uri":"https://microsoft.com/link","expires_in":900,"interval":1}"#,
            ),
            ("/token", 400, r#"{"error":"authorization_declined","error_description":"no"}"#),
        ]);
        assert!(device_flow(&api, "c", &Reporter::capture()).await.is_err());
        for (code, uri) in [
            ("USER-1", "http://evil.example/link"),           // not HTTPS
            ("\\u001b[2JFAKE", "https://microsoft.com/link"), // escape sequence in the code
            ("A", "https://microsoft.com/link"),              // implausibly short
        ] {
            let body = format!(r#"{{"device_code":"D","user_code":"{code}","verification_uri":"{uri}"}}"#);
            let api = Scripted::new(vec![("/devicecode", 200, body.as_str())]);
            let reporter = Reporter::capture();
            assert!(device_flow(&api, "c", &reporter).await.is_err(), "{code:?} {uri:?}");
            assert!(reporter.captured().is_empty(), "nothing may be shown for {code:?} {uri:?}");
        }
    }
}
