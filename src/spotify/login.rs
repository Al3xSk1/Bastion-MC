//! `bastion-mc spotify login | logout | status`.
//!
//! The sign-in is OAuth 2.0 authorization code with PKCE, as for Microsoft (`auth::msa`):
//! the user's own Spotify application, no client secret, a loopback redirect on
//! `127.0.0.1` (Spotify refuses `localhost`), `state` checked in constant time before
//! anything else of a callback is believed, the minimal scopes (`web::SCOPES`). The refresh
//! token goes to the launcher's token store under its own name
//! (`auth::store::SPOTIFY_KEY_DESCRIPTION`); the access token is dropped (wiped) here — a
//! launch gets its own from the token keeper.
//!
//! Register `http://127.0.0.1/callback` (no port: Spotify then accepts whatever port the
//! sign-in uses) or, with `[spotify] redirect_port`, `http://127.0.0.1:<port>/callback`.

use std::{path::Path, sync::Arc, time::Duration};

use tokio::{
    io::AsyncWriteExt,
    net::TcpListener,
    time::{Instant, timeout},
};

use super::{
    mpris::{self, Mpris, Player, Proxy},
    web::{self, Web},
};
use crate::{
    auth::{
        msa::{self, Callback},
        store::TokenStore,
    },
    config::Config,
    error::{Context, Error, Result},
    events::{Event, Reporter},
    net::http::{HttpClient, form_body},
    paths::Paths,
    sandbox::policy::SeccompPolicy,
    secret::Secret,
    util,
};

const AUTHORIZE: &str = "https://accounts.spotify.com/authorize";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_STRAY_CONNECTIONS: usize = 200;
const CALLBACK_PATH: &str = "/callback";

pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}{CALLBACK_PATH}")
}

pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let query = form_body(&[
        ("client_id", client_id.as_bytes()),
        ("response_type", b"code"),
        ("redirect_uri", redirect_uri.as_bytes()),
        ("code_challenge_method", b"S256"),
        ("code_challenge", challenge.as_bytes()),
        ("state", state.as_bytes()),
        ("scope", web::SCOPES.as_bytes()),
    ]);
    format!("{AUTHORIZE}?{}", String::from_utf8_lossy(&query))
}

/// Wait on `listener` for the browser to bring the authorization code back.
pub async fn wait_for_code(listener: TcpListener, state: &str, limit: Duration) -> Result<Secret> {
    let deadline = Instant::now() + limit;
    let mut stray = 0usize;
    loop {
        let (mut stream, _) = match tokio::time::timeout_at(deadline, listener.accept()).await {
            Err(_) => return Err(Error::Auth("timed out waiting for the Spotify sign-in in the browser".into())),
            Ok(Err(_)) => continue,
            Ok(Ok(connection)) => connection,
        };
        if let Some(head) = timeout(Duration::from_secs(5), msa::read_head(&mut stream)).await.ok().flatten() {
            match msa::parse_callback_at(&head, state, CALLBACK_PATH) {
                Callback::Code(code) => {
                    let _ = stream
                        .write_all(&msa::page(
                            "200 OK",
                            "Signed in to Spotify. You can close this tab and return to the terminal.",
                        ))
                        .await;
                    let _ = stream.shutdown().await;
                    return Ok(code);
                }
                Callback::Denied(why) => {
                    let _ = stream
                        .write_all(&msa::page(
                            "200 OK",
                            "The Spotify sign-in was not completed. Return to the terminal for details.",
                        ))
                        .await;
                    let _ = stream.shutdown().await;
                    return Err(Error::Auth(format!("the Spotify sign-in was refused: {why}")));
                }
                Callback::Unrelated(_) => {
                    let _ = stream
                        .write_all(&msa::page(
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

/// The sign-in up to the tokens: listen, show the URL (and open it, if asked), wait,
/// redeem the code.
pub async fn sign_in(
    http: &HttpClient,
    client_id: &str,
    port: Option<u16>,
    open_browser: bool,
    reporter: &Reporter,
) -> Result<web::Tokens> {
    let listener = TcpListener::bind(("127.0.0.1", port.unwrap_or(0)))
        .await
        .ctx(|| "binding the loopback redirect listener on 127.0.0.1")?;
    let port = listener.local_addr().ctx(|| "reading the listener's address")?.port();
    let redirect = redirect_uri(port);
    let pkce = msa::pkce()?;
    let state = util::b64url_encode(&util::random_bytes::<32>()?);
    let url = authorize_url(client_id, &redirect, &state, &pkce.challenge);
    reporter.emit(Event::BrowserLogin { url: url.clone(), timeout_secs: LOGIN_TIMEOUT.as_secs() });
    if open_browser {
        // The user's own browser, as the user asked; nothing of the launcher's goes with it.
        let _ = std::process::Command::new("xdg-open")
            .arg(&url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    let code = wait_for_code(listener, &state, LOGIN_TIMEOUT).await?;
    web::redeem(http, client_id, &code, &redirect, &pkce.verifier).await.map_err(|e| Error::Auth(e.to_string()))
}

fn client_id(cfg: &Config, paths: &Paths) -> Result<String> {
    cfg.spotify.client_id.clone().ok_or_else(|| {
        Error::Config(format!(
            "[spotify] client_id is not set in {}. The Spotify Web API needs your own Spotify application: create one \
             at https://developer.spotify.com/dashboard, register the redirect URI http://127.0.0.1/callback, and put \
             its client ID under [spotify]. (The desktop app on this machine works without it, over MPRIS.)",
            paths.config_file().display()
        ))
    })
}

/// `bastion-mc spotify login`.
pub async fn login(paths: &Paths, cfg: &Config, open_browser: bool, reporter: &Reporter) -> Result<()> {
    let client_id = client_id(cfg, paths)?;
    let store = TokenStore::open(&cfg.auth, paths)?;
    let http = super::client(cfg)?;
    let tokens = sign_in(&http, &client_id, cfg.spotify.redirect_port, open_browser, reporter).await?;
    drop(tokens.access);
    let refresh = tokens.refresh.ok_or_else(|| Error::Auth("Spotify returned no refresh token".into()))?;
    store.save_spotify(&refresh)?;
    drop(refresh);
    if matches!(store, TokenStore::None) {
        reporter.warn(
            "auth.token_store = \"none\": the Spotify sign-in worked, but nothing was stored, so a launch cannot use the \
             Web API",
        );
    }
    let granted = if tokens.scope.is_empty() { "none named".to_string() } else { tokens.scope };
    reporter.status(format!("signed in to Spotify — refresh token stored in: {}; scopes: {granted}", store.describe()));
    Ok(())
}

/// `bastion-mc spotify logout`: forget the stored refresh token. (Spotify keeps the grant
/// until it is removed under "Manage apps" in the account's settings.)
pub fn logout(paths: &Paths, cfg: &Config, reporter: &Reporter) -> Result<()> {
    TokenStore::open(&cfg.auth, paths)?.delete_spotify()?;
    reporter.status(
        "the stored Spotify refresh token is gone (to revoke the grant itself: Spotify account settings, Manage apps)",
    );
    Ok(())
}

/// `bastion-mc spotify status`: the configuration, and — unless `offline` — whether the
/// desktop app answers over MPRIS (through the same proxy a launch uses) and whether the
/// Web API sign-in works (one refresh, one read of the player).
pub async fn status(paths: &Paths, cfg: &Config, program: &Path, offline: bool, reporter: &Reporter) -> Result<()> {
    let s = &cfg.spotify;
    let say = |line: String| reporter.status(line);
    if !s.mpris {
        say("desktop app (MPRIS): off ([spotify] mpris = false)".into());
    } else if offline {
        say("desktop app (MPRIS): on (not checked: --offline)".into());
    } else {
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        match mpris::session_bus(address.as_deref(), paths.runtime.as_deref()) {
            Err(why) => say(format!("desktop app (MPRIS): on, but unusable — {why}")),
            Ok(bus) => {
                let base =
                    paths.runtime.as_deref().map(|r| r.join(crate::paths::APP)).unwrap_or_else(|| paths.cache.clone());
                util::create_private_dir_all(&base)?;
                let seccomp = SeccompPolicy {
                    mode: cfg.sandbox.seccomp,
                    tripwire: cfg.sandbox.tripwire,
                    extra_allow: Vec::new(),
                };
                let sink: mpris::Sink = Arc::new(|_| {});
                let checked = async {
                    let proxy = Proxy::start(&bus, &base, program, seccomp, sink).await?;
                    let stream = proxy.connect().await?;
                    proxy.release_name();
                    let (client, _signals) = Mpris::open(stream).await?;
                    let line = match client.owner().await? {
                        None => "not running".to_string(),
                        Some(_) => {
                            let properties = client.properties().await?;
                            let refs: Vec<_> = properties.iter().map(|(k, v)| (k.as_str(), v)).collect();
                            let mut player = Player::default();
                            player.apply(&refs, std::time::Instant::now());
                            format!(
                                "running — {} — {}{}",
                                if player.title.is_empty() { "(no track)" } else { &player.title },
                                if player.artist.is_empty() { "?" } else { &player.artist },
                                match player.playback {
                                    mpris::Playback::Playing => " (playing)",
                                    mpris::Playback::Paused => " (paused)",
                                    mpris::Playback::Stopped => " (stopped)",
                                }
                            )
                        }
                    };
                    Ok::<_, Error>(line)
                };
                match checked.await {
                    Ok(line) => {
                        say(format!("desktop app (MPRIS): on; session bus {}; Spotify is {line}", bus.display()))
                    }
                    Err(e) => say(format!("desktop app (MPRIS): on, but the check failed — {e}")),
                }
            }
        }
    }
    say(format!("album art: {}", if s.album_art { "on (fetched from i.scdn.co when the phone asks)" } else { "off" }));
    let Some(client_id) = s.client_id.as_deref() else {
        say("Web API: off (no [spotify] client_id)".into());
        return Ok(());
    };
    let store = TokenStore::open(&cfg.auth, paths)?;
    if !store.has_spotify()? {
        say(format!("Web API: client ID set; not signed in (`bastion-mc spotify login`; store: {})", store.describe()));
        return Ok(());
    }
    if offline {
        say(format!("Web API: signed in (store: {}; not checked: --offline)", store.describe()));
        return Ok(());
    }
    let refresh = store.load_spotify()?.ok_or_else(|| Error::Auth("the stored Spotify token disappeared".into()))?;
    let http = super::client(cfg)?;
    let store = Arc::new(store);
    let saver = store.clone();
    let source = web::Refreshing::new(
        http.clone(),
        client_id,
        refresh,
        Box::new(move |next: &Secret| saver.save_spotify(next).map_err(|e| web::WebError::Net(e.to_string()))),
    );
    let api = Web::new(http, Arc::new(source));
    match api.state().await {
        Ok(Some(state)) => say(format!(
            "Web API: signed in and working — {} {} — {}{}",
            if state.playing { "playing" } else { "paused:" },
            state.title,
            state.artist,
            state.device.map(|d| format!(" on {d}")).unwrap_or_default()
        )),
        Ok(None) => say("Web API: signed in and working — nothing is playing on any device".into()),
        Err(e) => say(format!("Web API: signed in, but: {e}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authorization_url_asks_for_exactly_what_is_needed() {
        let url = authorize_url("0123456789abcdef0123456789abcdef", &redirect_uri(43117), "ST4TE", "CH4LLENGE");
        assert!(url.starts_with("https://accounts.spotify.com/authorize?client_id=0123456789abcdef0123456789abcdef&"));
        for part in [
            "response_type=code",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A43117%2Fcallback",
            "code_challenge_method=S256",
            "code_challenge=CH4LLENGE",
            "state=ST4TE",
            "scope=user-read-playback-state%20user-modify-playback-state",
        ] {
            assert!(url.contains(part), "{part} missing from {url}");
        }
        assert!(!url.contains("client_secret") && !url.contains("show_dialog"));
    }

    #[tokio::test]
    async fn the_loopback_ignores_forgeries_and_takes_the_real_callback() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let waiting = tokio::spawn(wait_for_code(listener, "GOOD", Duration::from_secs(20)));
        async fn hit(port: u16, line: &str) -> String {
            use tokio::io::AsyncReadExt;
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            s.write_all(format!("{line}\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).await.unwrap();
            let mut answer = String::new();
            let _ = s.read_to_string(&mut answer).await;
            answer
        }
        for forged in [
            "GET /callback?code=EVIL&state=BAD HTTP/1.1",
            "GET /?code=EVIL&state=GOOD HTTP/1.1",
            "GET /callback?error=access_denied&state=BAD HTTP/1.1",
            "GET /callback?code=A&code=B&state=GOOD HTTP/1.1",
        ] {
            assert!(hit(port, forged).await.starts_with("HTTP/1.1 400"), "{forged}");
        }
        assert!(hit(port, "GET /callback?code=AQB-code&state=GOOD HTTP/1.1").await.starts_with("HTTP/1.1 200"));
        assert_eq!(waiting.await.unwrap().unwrap().expose(), b"AQB-code");
    }
}
