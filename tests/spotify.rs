//! Spotify from the phone, against stand-ins: never the user's session bus, never the real
//! Spotify app, never Spotify's servers.
//!
//! * **MPRIS**: a private `dbus-daemon` (its own configuration, its own socket in a scratch
//!   directory) and `tests/support/mpris-standin.py`, an MPRIS player written with GDBus — an
//!   independent implementation of the protocol — that owns `org.mpris.MediaPlayer2.spotify`
//!   there. The launcher's side is the real thing: `xdg-dbus-proxy` in its bubblewrap sandbox,
//!   the hand-written client, and (in the confined-child test) a process that has confined
//!   itself the way the launcher does in step 9.
//! * **Web API**: see the `web_api_*` tests (a stand-in HTTPS server with a test CA).

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bastion_mc::{
    confine::{self, Confinement, Demand, Support},
    sandbox::policy::SeccompPolicy,
    spotify::{
        dbus::{Message, Value},
        mpris::{self, Event, Mpris, Player, Proxy, Repeat},
    },
    util,
};

fn scratch(tag: &str) -> PathBuf {
    // Short: socket paths are limited to 107 bytes.
    let dir = PathBuf::from(format!("/tmp/bmc-{tag}-{}", util::hex_encode(&util::random_bytes::<4>().unwrap())));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[path = "support/mpris_bus.rs"]
mod mpris_bus;
use mpris_bus::{PrivateBus, Standin, available};

fn sink() -> mpris::Sink {
    Arc::new(|line: String| eprintln!("{line}"))
}

async fn next_event(signals: &mut tokio::sync::mpsc::Receiver<Message>, owner: Option<&str>) -> Event {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let signal = tokio::time::timeout_at(deadline, signals.recv()).await.expect("a signal in time").expect("open");
        if let Some(event) = Mpris::interpret(&signal, owner) {
            return event;
        }
    }
}

async fn open(proxy: &Proxy) -> (Mpris, tokio::sync::mpsc::Receiver<Message>) {
    let stream = proxy.connect().await.unwrap();
    proxy.release_name();
    Mpris::open(stream).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_player_is_driven_through_the_proxy_and_its_signals_arrive() {
    if !available() {
        return;
    }
    let dir = scratch("mpris");
    let bus = PrivateBus::start(&dir);
    let mut standin = Standin::start(&bus, mpris::PLAYER);
    let proxy =
        Proxy::start(&bus.socket, &dir, Path::new(env!("CARGO_BIN_EXE_bastion-mc")), SeccompPolicy::default(), sink())
            .await
            .expect("xdg-dbus-proxy in its sandbox");
    let (client, mut signals) = open(&proxy).await;
    assert!(
        !dir.read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("mpris-")),
        "name released"
    );

    let owner = client.owner().await.unwrap().expect("the stand-in owns the player's name");
    let mut player = Player::default();
    let properties = client.properties().await.unwrap();
    let refs: Vec<(&str, &Value)> = properties.iter().map(|(k, v)| (k.as_str(), v)).collect();
    player.apply(&refs, Instant::now());
    assert_eq!(player.title, "Harder, Better, Faster, Stronger");
    assert_eq!((player.artist.as_str(), player.album.as_str()), ("Daft Punk", "Discovery"));
    assert_eq!((player.length_ms, player.volume, player.shuffle), (Some(224_000), Some(50), Some(false)));
    assert_eq!(player.repeat, Some(Repeat::Off));
    assert_eq!(player.art.as_deref(), Some("ab67616d0000b2734ce8b4e42588bf18182a1ad2"));
    assert_eq!(player.track_id.as_deref(), Some("/com/spotify/track/4uLU6hMCjMI75M1A2tKUQC"));

    client.play().await.unwrap();
    assert_eq!(
        next_event(&mut signals, Some(&owner)).await,
        Event::Changed(vec![("PlaybackStatus".into(), Value::Str("Playing".into()))])
    );
    client.volume(40).await.unwrap();
    client.shuffle(true).await.unwrap();
    client.repeat(Repeat::Track).await.unwrap();
    client.seek(61_000, &player).await.unwrap();
    client.open_uri("spotify:track:7ouMYWpwJ422jRcDASZB7P").await.unwrap();
    client.next().await.unwrap();
    client.toggle().await.unwrap();
    client.pause().await.unwrap();
    client.previous().await.unwrap();
    let mut seeked = false;
    let mut titles = Vec::new();
    for _ in 0..12 {
        match next_event(&mut signals, Some(&owner)).await {
            Event::Seeked(us) => seeked = us == 61_000_000,
            Event::Changed(changed) => {
                let refs: Vec<(&str, &Value)> = changed.iter().map(|(k, v)| (k.as_str(), v)).collect();
                player.apply(&refs, Instant::now());
                if refs.iter().any(|(k, _)| *k == "Metadata") {
                    titles.push(player.title.clone());
                }
            }
            Event::Owner(_) => panic!("the owner did not change"),
        }
        if titles.len() == 3 {
            break;
        }
    }
    assert!(seeked, "the Seeked signal arrived with the new position");
    assert_eq!(titles, ["Knights of Cydonia", "Harder, Better, Faster, Stronger", "Knights of Cydonia"]);
    let said = standin.said();
    for expected in [
        "call Play",
        "set Volume 0.4",
        "set Shuffle True",
        "set LoopStatus Track",
        "call SetPosition /com/spotify/track/4uLU6hMCjMI75M1A2tKUQC 61000000",
        "call OpenUri spotify:track:7ouMYWpwJ422jRcDASZB7P",
        "call Next",
        "call PlayPause",
        "call Pause",
        "call Previous",
    ] {
        assert!(said.iter().any(|l| l == expected), "the stand-in did not get {expected:?}: {said:?}");
    }

    // The filter. The player's root interface (Raise, Quit) is not reachable…
    let raw = client.raw();
    let quit = raw.call(Message::call(mpris::PLAYER, mpris::OBJECT, "org.mpris.MediaPlayer2", "Quit", vec![])).await;
    assert!(quit.is_err(), "Quit must be refused by the proxy");
    // …nor any other object of the player…
    let other = raw.call(Message::call(mpris::PLAYER, "/other", mpris::PLAYER_INTERFACE, "Play", vec![])).await;
    assert!(other.is_err());
    // …nor any other name: another player on the same bus is neither listed nor callable.
    let mut vlc = Standin::start(&bus, "org.mpris.MediaPlayer2.vlc");
    let names = raw
        .call(Message::call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "ListNames",
            vec![],
        ))
        .await
        .unwrap();
    let listed: Vec<&str> = names.body[0].as_str_list().unwrap();
    assert!(!listed.contains(&"org.mpris.MediaPlayer2.vlc"), "{listed:?}");
    let play_vlc = raw
        .call(Message::call("org.mpris.MediaPlayer2.vlc", mpris::OBJECT, mpris::PLAYER_INTERFACE, "Play", vec![]))
        .await;
    assert!(play_vlc.is_err(), "another name must be unreachable");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!vlc.said().iter().any(|l| l.starts_with("call")), "the other player was reached: {:?}", vlc.said());
    assert!(!standin.said().iter().any(|l| l.starts_with("call Quit")), "Quit reached the player");
    vlc.tell("quit");
    vlc.quit();

    // The app quits and starts again: the client hears both.
    standin.tell("quit");
    standin.quit();
    assert_eq!(next_event(&mut signals, Some(&owner)).await, Event::Owner(None));
    let _again = Standin::start(&bus, mpris::PLAYER);
    match next_event(&mut signals, None).await {
        Event::Owner(Some(name)) => assert_ne!(name, owner, "a new connection, a new unique name"),
        other => panic!("{other:?}"),
    }
    drop(client);
    drop(proxy);
    let _ = std::fs::remove_dir_all(&dir);
}

const CONFINED_CHILD: &str = "BASTION_SPOTIFY_CONFINED_CHILD";

/// The re-executed half of the next test: does what the launcher does before step 9 (start
/// the proxy, connect, register for signals), confines itself as the launcher does in step
/// 9, and then drives the player. Nothing in a normal run.
#[tokio::test(flavor = "multi_thread")]
async fn confined_child() {
    let Ok(bus) = std::env::var(CONFINED_CHILD) else { return };
    let dir = Path::new(&bus).parent().unwrap().to_path_buf();
    let proxy = Proxy::start(
        Path::new(&bus),
        &dir,
        Path::new(env!("CARGO_BIN_EXE_bastion-mc")),
        SeccompPolicy::default(),
        sink(),
    )
    .await
    .unwrap();
    let (client, mut signals) = open(&proxy).await;
    let applied = confine::apply(&Confinement::supervisor(), Demand::Whole).expect("confining the child");
    assert!(applied.is_whole());
    let owner = client.owner().await.unwrap();
    println!("\nowner={}", owner.is_some());
    println!("play={:?}", client.play().await.map_err(|e| e.to_string()));
    let event = next_event(&mut signals, owner.as_deref()).await;
    println!("event={event:?}");
    println!("volume={:?}", client.volume(65).await.map_err(|e| e.to_string()));
    // Control: the confinement is in force — no new connection to the bus can be made.
    let fresh = std::os::unix::net::UnixStream::connect(&bus);
    println!("fresh_connection={}", if fresh.is_err() { "refused" } else { "made" });
}

/// MPRIS needs nothing after step 9 that a confined launcher cannot do: the connection to
/// the proxy was made before, and calls and signals travel on it.
#[test]
fn mpris_keeps_working_after_the_launcher_has_confined_itself() {
    if !available() {
        return;
    }
    if confine::support() != Support::Whole {
        eprintln!("skipped: {:?}", confine::support());
        return;
    }
    let dir = scratch("mprisc");
    let bus = PrivateBus::start(&dir);
    let standin = Standin::start(&bus, mpris::PLAYER);
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["confined_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CONFINED_CHILD, &bus.socket)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    for expected in ["owner=true", "play=Ok(())", "volume=Ok(())", "fresh_connection=refused"] {
        assert!(stdout.contains(expected), "{expected}:\n{stdout}");
    }
    assert!(stdout.contains("event=Changed([(\"PlaybackStatus\""), "{stdout}");
    let said = standin.said();
    assert!(said.contains(&"call Play".to_string()) && said.contains(&"set Volume 0.65".to_string()), "{said:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ the Web API, against a stand-in

mod standin_https {
    //! A stand-in for Spotify's three hosts: one HTTPS server with a certificate for
    //! accounts.spotify.com, api.spotify.com and i.scdn.co from a test CA that only the
    //! test's client trusts, answering from a script and recording every request.

    use std::{
        collections::VecDeque,
        net::SocketAddr,
        sync::{Arc, Mutex},
    };

    use bastion_mc::net::{
        http::HttpClient,
        tls::{self, PinSet},
    };
    use http_body_util::{BodyExt, Full};
    use hyper::{Request, Response, body::Incoming};
    use hyper_util::rt::TokioIo;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
    use rustls::{
        RootCertStore, ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    };

    #[derive(Debug, Clone)]
    pub struct Seen {
        pub method: String,
        pub host: String,
        pub path: String,
        pub authorization: String,
        pub content_length: Option<String>,
        pub body: String,
    }

    #[derive(Debug, Clone)]
    pub struct Reply {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    pub fn reply(status: u16, body: &str) -> Reply {
        Reply { status, headers: Vec::new(), body: body.as_bytes().to_vec() }
    }

    type Script = Vec<(String, String, VecDeque<Reply>)>;

    #[derive(Clone)]
    pub struct Standin {
        pub addr: SocketAddr,
        ca: CertificateDer<'static>,
        pub seen: Arc<Mutex<Vec<Seen>>>,
        script: Arc<Mutex<Script>>,
    }

    impl Standin {
        /// Answer `method` requests to paths starting with `prefix` with these replies, in
        /// order; the last one repeats. Later entries for the same route come first.
        pub fn script(&self, method: &str, prefix: &str, replies: Vec<Reply>) {
            self.script.lock().unwrap().insert(0, (method.into(), prefix.into(), replies.into()));
        }

        pub fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }

        pub fn count(&self, method: &str, prefix: &str) -> usize {
            self.seen().iter().filter(|s| s.method == method && s.path.starts_with(prefix)).count()
        }

        /// The launcher's HTTPS client, trusting only the test CA, dialling the stand-in
        /// for every host, and restricted to Spotify's hosts as in a launch.
        pub fn client(&self) -> HttpClient {
            let mut roots = RootCertStore::empty();
            roots.add(self.ca.clone()).unwrap();
            let config = tls::client_config(roots, PinSet::default()).unwrap();
            HttpClient::new(config, &[]).only(&bastion_mc::spotify::web::HOSTS).with_dial_override(self.addr)
        }
    }

    pub async fn start() -> Standin {
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.distinguished_name.push(DnType::CommonName, "bastion-mc test CA (Spotify stand-in)");
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
        let ca_key = KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap().der().clone();
        let names = bastion_mc::spotify::web::HOSTS.iter().map(|h| h.to_string()).collect::<Vec<_>>();
        let mut leaf_params = CertificateParams::new(names).unwrap();
        leaf_params.distinguished_name.push(DnType::CommonName, "api.spotify.com");
        let leaf_key = KeyPair::generate().unwrap();
        let leaf = leaf_params.signed_by(&leaf_key, &Issuer::from_params(&ca_params, &ca_key)).unwrap().der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
        let config = ServerConfig::builder_with_provider(tls::provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![leaf, ca.clone()], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let standin = Standin { addr, ca, seen: Arc::default(), script: Arc::default() };
        let shared = standin.clone();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { break };
                let (acceptor, shared) = (acceptor.clone(), shared.clone());
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(tcp).await else { return };
                    let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                        let shared = shared.clone();
                        async move { Ok::<_, std::convert::Infallible>(shared.answer(request).await) }
                    });
                    let _ =
                        hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(tls), service).await;
                });
            }
        });
        standin
    }

    impl Standin {
        async fn answer(&self, request: Request<Incoming>) -> Response<Full<bytes::Bytes>> {
            let header =
                |name: &str| request.headers().get(name).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
            let (method, path) = (request.method().to_string(), request.uri().to_string());
            let (host, authorization, content_length) = (
                header("host").unwrap_or_default(),
                header("authorization").unwrap_or_default(),
                header("content-length"),
            );
            let body = request.into_body().collect().await.map(|b| b.to_bytes()).unwrap_or_default();
            self.seen.lock().unwrap().push(Seen {
                method: method.clone(),
                host,
                path: path.clone(),
                authorization,
                content_length,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
            let reply = {
                let mut script = self.script.lock().unwrap();
                match script.iter_mut().find(|(m, prefix, _)| *m == method && path.starts_with(prefix.as_str())) {
                    Some((_, _, replies)) if replies.len() > 1 => replies.pop_front().unwrap(),
                    Some((_, _, replies)) => replies.front().cloned().unwrap(),
                    None => super::standin_https::reply(404, r#"{"error":{"status":404,"message":"no script"}}"#),
                }
            };
            let mut response = Response::builder().status(reply.status);
            for (k, v) in &reply.headers {
                response = response.header(k.as_str(), v.as_str());
            }
            response.body(Full::new(bytes::Bytes::from(reply.body))).unwrap()
        }
    }
}

use bastion_mc::{
    phone::{Command as PhoneCommand, FrameFormat, Outbound, Outbox},
    secret::Secret,
    spotify::{
        self, picture,
        web::{self as spotify_web, Web, WebError},
    },
};
use standin_https::{Reply, reply};

const TOKEN_1: &str = r#"{"access_token":"BQ-ACCESS-1","token_type":"Bearer","expires_in":3600,"scope":"user-read-playback-state user-modify-playback-state","refresh_token":"AQ-REFRESH-2"}"#;
const TOKEN_2: &str = r#"{"access_token":"BQ-ACCESS-2","token_type":"Bearer","expires_in":3600,"scope":"user-read-playback-state user-modify-playback-state"}"#;
const CLIENT_ID: &str = "0123456789abcdef0123456789abcdef";

fn player_state(title: &str, playing: bool) -> String {
    format!(
        r#"{{"device":{{"id":"5fbb3ba6aa454b5534c4ba43a8c7e8e45a63ad0e","is_active":true,"name":"Kitchen speaker","type":"Speaker","volume_percent":59}},"shuffle_state":false,"repeat_state":"off","progress_ms":1000,"item":{{"album":{{"images":[{{"url":"https://i.scdn.co/image/ab67616d00001e024ce8b4e42588bf18182a1ad2","width":300}}],"name":"Discovery"}},"artists":[{{"name":"Daft Punk"}}],"duration_ms":224693,"name":"{title}","type":"track","uri":"spotify:track:5W3cjX2J3tjhG8zb6u0qHn"}},"is_playing":{playing}}}"#
    )
}

/// A refreshing token source as a launch's keeper uses it, with what it stores recorded.
fn refreshing(standin: &standin_https::Standin, stored: Arc<Mutex<Vec<String>>>) -> Arc<spotify_web::Refreshing> {
    Arc::new(spotify_web::Refreshing::new(
        standin.client(),
        CLIENT_ID,
        Secret::from_slice(b"AQ-REFRESH-1").unwrap(),
        Box::new(move |next: &Secret| {
            stored.lock().unwrap().push(String::from_utf8_lossy(next.expose()).into_owned());
            Ok(())
        }),
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn web_api_a_refused_token_is_refreshed_once_and_rotations_are_stored() {
    let standin = standin_https::start().await;
    standin.script("POST", "/api/token", vec![reply(200, TOKEN_1), reply(200, TOKEN_2)]);
    standin.script(
        "GET",
        "/v1/me/player?",
        vec![
            reply(401, r#"{"error":{"status":401,"message":"The access token expired"}}"#),
            reply(200, &player_state("Harder", true)),
        ],
    );
    let stored = Arc::new(Mutex::new(Vec::new()));
    let api = Web::new(standin.client(), refreshing(&standin, stored.clone()));
    let state = api.state().await.unwrap().expect("something plays");
    assert_eq!((state.title.as_str(), state.playing), ("Harder", true));
    let seen = standin.seen();
    let tokens: Vec<&standin_https::Seen> = seen.iter().filter(|s| s.path == "/api/token").collect();
    assert_eq!(tokens.len(), 2, "one refresh at the start, one after the 401: {seen:#?}");
    assert!(
        tokens[0]
            .body
            .contains("grant_type=refresh_token&refresh_token=AQ-REFRESH-1&client_id=0123456789abcdef0123456789abcdef")
    );
    assert!(
        tokens[1].body.contains("refresh_token=AQ-REFRESH-2"),
        "the rotated token is the one used next: {}",
        tokens[1].body
    );
    assert!(!tokens[0].body.contains("client_secret"));
    assert_eq!(tokens[0].host, "accounts.spotify.com");
    let reads: Vec<&standin_https::Seen> = seen.iter().filter(|s| s.path.starts_with("/v1/me/player")).collect();
    assert_eq!(
        reads.iter().map(|s| s.authorization.as_str()).collect::<Vec<_>>(),
        ["Bearer BQ-ACCESS-1", "Bearer BQ-ACCESS-2"]
    );
    assert_eq!(reads[0].host, "api.spotify.com");
    assert_eq!(*stored.lock().unwrap(), ["AQ-REFRESH-2"], "the rotation was handed over for storing");
    // 204: nothing plays anywhere.
    standin.script("GET", "/v1/me/player?", vec![reply(204, "")]);
    assert_eq!(api.state().await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn web_api_premium_no_device_and_429_are_recognised() {
    let standin = standin_https::start().await;
    standin.script("POST", "/api/token", vec![reply(200, TOKEN_2)]);
    standin.script(
        "PUT",
        "/v1/me/player/play",
        vec![reply(403, r#"{"error":{"status":403,"message":"Player command failed: Premium required","reason":"PREMIUM_REQUIRED"}}"#)],
    );
    standin.script(
        "POST",
        "/v1/me/player/next",
        vec![reply(404, r#"{"error":{"status":404,"message":"Player command failed: No active device found","reason":"NO_ACTIVE_DEVICE"}}"#)],
    );
    let mut limited = reply(429, "");
    limited.headers.push(("retry-after".into(), "2".into()));
    standin.script("GET", "/v1/me/player/devices", vec![limited, reply(200, r#"{"devices":[]}"#)]);
    let api = Web::new(standin.client(), refreshing(&standin, Arc::default()));
    assert_eq!(api.play().await, Err(WebError::Premium));
    assert_eq!(api.next().await, Err(WebError::NoDevice));
    assert_eq!(api.devices().await, Err(WebError::RateLimited { seconds: 2 }));
    // During the pause nothing is sent at all — not even other kinds of call.
    let before = standin.seen().len();
    assert!(matches!(api.devices().await, Err(WebError::RateLimited { .. })));
    assert!(matches!(api.pause().await, Err(WebError::RateLimited { .. })));
    assert_eq!(standin.seen().len(), before, "nothing left during Spotify's pause");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert_eq!(api.devices().await, Ok(vec![]));
    // A body-less PUT carries a zero length (Spotify answers 411 otherwise).
    let play = standin.seen().into_iter().find(|s| s.path == "/v1/me/player/play").unwrap();
    assert_eq!(play.content_length.as_deref(), Some("0"));
}

#[tokio::test(flavor = "multi_thread")]
async fn web_api_requests_have_the_shape_spotify_documents_and_local_limits_hold() {
    let standin = standin_https::start().await;
    standin.script("POST", "/api/token", vec![reply(200, TOKEN_2)]);
    for (method, prefix) in [("PUT", "/v1/me/player"), ("POST", "/v1/me/player/queue")] {
        standin.script(method, prefix, vec![reply(204, "")]);
    }
    standin.script(
        "GET",
        "/v1/search",
        vec![reply(200, r#"{"tracks":{"items":[{"uri":"spotify:track:5W3cjX2J3tjhG8zb6u0qHn","name":"Harder","artists":[{"name":"Daft Punk"}],"album":{"name":"Discovery"}}]}}"#)],
    );
    let api = Web::new(standin.client(), refreshing(&standin, Arc::default()));
    let album = spotify::uri::Uri::parse("spotify:album:2noRn2Aes5aoNVsU6iWThc").unwrap();
    let track = spotify::uri::Uri::parse("spotify:track:5W3cjX2J3tjhG8zb6u0qHn").unwrap();
    api.open(&album).await.unwrap();
    api.open(&track).await.unwrap();
    api.queue(&track).await.unwrap();
    api.seek(61_000).await.unwrap();
    api.volume(40).await.unwrap();
    api.repeat(spotify::mpris::Repeat::Track).await.unwrap();
    let device = spotify_web::Device {
        id: "5fbb3ba6aa454b5534c4ba43a8c7e8e45a63ad0e".into(),
        name: "Kitchen".into(),
        active: false,
    };
    api.transfer(&device).await.unwrap();
    let found = api.search("daft punk & friends").await.unwrap();
    assert_eq!(found[0].title, "Harder");
    let seen = standin.seen();
    let has = |method: &str, path: &str, body: &str| {
        assert!(
            seen.iter().any(|s| s.method == method && s.path == path && s.body == body),
            "{method} {path} {body:?} not in {seen:#?}"
        )
    };
    has("PUT", "/v1/me/player/play", r#"{"context_uri":"spotify:album:2noRn2Aes5aoNVsU6iWThc"}"#);
    has("PUT", "/v1/me/player/play", r#"{"uris":["spotify:track:5W3cjX2J3tjhG8zb6u0qHn"]}"#);
    has("POST", "/v1/me/player/queue?uri=spotify%3Atrack%3A5W3cjX2J3tjhG8zb6u0qHn", "");
    has("PUT", "/v1/me/player/seek?position_ms=61000", "");
    has("PUT", "/v1/me/player/volume?volume_percent=40", "");
    has("PUT", "/v1/me/player/repeat?state=track", "");
    has("PUT", "/v1/me/player", r#"{"device_ids":["5fbb3ba6aa454b5534c4ba43a8c7e8e45a63ad0e"],"play":true}"#);
    has("GET", "/v1/search?q=daft%20punk%20%26%20friends&type=track,album,artist,playlist&limit=5", "");
    // Three searches a burst; the fourth is held back here, without a request.
    let searches = standin.count("GET", "/v1/search");
    api.search("b").await.unwrap();
    api.search("c").await.unwrap();
    assert!(matches!(api.search("d").await, Err(WebError::Limited { .. })));
    assert_eq!(standin.count("GET", "/v1/search"), searches + 2);
}

fn test_jpeg() -> Option<Vec<u8>> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=640x640:rate=1"])
        .args(["-frames:v", "1", "-q:v", "4", "-f", "mjpeg", "-"])
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// States, frames (slot, width, height, format, bytes) and notices the app sent.
type Drained = (Vec<serde_json::Value>, Vec<(u8, u16, u16, FrameFormat, Vec<u8>)>, Vec<String>);

fn states(rx: &mut tokio::sync::mpsc::Receiver<Outbound>) -> Drained {
    let (mut states, mut frames, mut notices) = (Vec::new(), Vec::new(), Vec::new());
    while let Ok(message) = rx.try_recv() {
        match message {
            Outbound::State { app: "spotify", json } => states.push(serde_json::from_str(&json).unwrap()),
            Outbound::Frame { app: "spotify", slot, width, height, format, bytes } => {
                frames.push((slot, width, height, format, bytes))
            }
            Outbound::Notice { app: "spotify", text } => notices.push(text),
            other => panic!("not the spotify app's: {other:?}"),
        }
    }
    (states, frames, notices)
}

async fn settle(
    rx: &mut tokio::sync::mpsc::Receiver<Outbound>,
    wanted: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while let Ok(message) = rx.try_recv() {
            if let Outbound::State { json, .. } = message {
                let value: serde_json::Value = serde_json::from_str(&json).unwrap();
                if wanted(&value) {
                    return value;
                }
            }
        }
        assert!(Instant::now() < deadline, "the state never became what was wanted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The whole app as the phone uses it: the desktop app over MPRIS while it runs (through
/// the real proxy, on a private bus), album art from the stand-in image host, and — once the
/// desktop app has quit — the Web API on the stand-in, Premium failure included.
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_app_uses_the_desktop_app_then_the_web_api() {
    if !available() {
        return;
    }
    let Some(jpeg) = test_jpeg() else {
        eprintln!("skipped: needs ffmpeg for a test cover");
        return;
    };
    let dir = scratch("app");
    let bus = PrivateBus::start(&dir);
    let mut standin = Standin::start(&bus, mpris::PLAYER);
    let https = standin_https::start().await;
    https.script("POST", "/api/token", vec![reply(200, TOKEN_2)]);
    https.script(
        "GET",
        "/image/ab67616d0000b2734ce8b4e42588bf18182a1ad2",
        vec![Reply { status: 200, headers: vec![], body: jpeg.clone() }],
    );
    https.script("GET", "/image/", vec![Reply { status: 200, headers: vec![], body: jpeg }]);

    let paths = bastion_mc::paths::Paths {
        config: dir.join("config"),
        data: dir.join("data"),
        cache: dir.join("cache"),
        state: dir.join("state"),
        runtime: Some(dir.join("run")),
    };
    paths.ensure().unwrap();
    let cfg = bastion_mc::config::Config::default();
    let api = Arc::new(Web::new(https.client(), refreshing(&https, Arc::default())));
    let setup = spotify::Setup {
        cfg: &cfg,
        paths: &paths,
        program: Path::new(env!("CARGO_BIN_EXE_bastion-mc")),
        bus_address: Some(bus.address()),
        offline: false,
        log: Arc::new(|line| eprintln!("{line}")),
    };
    let service = spotify::prepare_with(setup, Some(api), Some(https.client())).await;
    let mut keep = Confinement::default();
    service.confinement_needs(&mut keep);
    assert_eq!(keep.tcp_connect, [443], "the API and the image server: HTTPS, nothing else");
    assert!(keep.write.is_empty() && keep.unix_sockets.is_empty() && keep.read.is_empty());
    let mut app = service.into_app();
    assert_eq!(app.name(), "spotify");
    let (out, mut rx) = Outbox::new(64);

    app.hello(&out);
    let first = settle(&mut rx, |s| s["source"] == "mpris").await;
    assert_eq!(first["track"], "Harder, Better, Faster, Stronger");
    assert_eq!(
        (first["artist"].as_str(), first["volume"].as_u64(), first["repeat"].as_str()),
        (Some("Daft Punk"), Some(50), Some("off"))
    );
    assert_eq!(first["desktop"], "running");

    let cmd = |line: &str| PhoneCommand::parse(line.as_bytes()).unwrap();
    app.command(cmd("spotify volume 40"), &out);
    settle(&mut rx, |s| s["volume"] == 40).await;
    app.command(cmd("spotify art"), &out);
    let deadline = Instant::now() + Duration::from_secs(10);
    let frame = loop {
        let (_, frames, _) = states(&mut rx);
        if let Some(frame) = frames.into_iter().next() {
            break frame;
        }
        assert!(Instant::now() < deadline, "no cover arrived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let (slot, width, height, format, png) = frame;
    assert_eq!((slot, width, height, format), (0, 320, 320, FrameFormat::Png), "the cover, re-encoded to fit 320×320");
    assert_eq!(picture::png_dimensions(&png), Some((320, 320)));
    assert_eq!(https.count("GET", "/image/ab67616d0000b2734ce8b4e42588bf18182a1ad2"), 1);
    // A track change brings its cover by itself, once the phone has asked for covers.
    app.command(cmd("spotify next"), &out);
    settle(&mut rx, |s| s["track"] == "One More Time").await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while https.count("GET", "/image/ab67616d0000b2734ce8b4e42588bf18182a1ad3") == 0 {
        assert!(Instant::now() < deadline, "the next cover was not fetched");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Web-only verbs need the Web API; they are refused with a reason when it says no.
    https.script(
        "GET",
        "/v1/me/player/devices",
        vec![reply(200, r#"{"devices":[{"id":"abc123","is_active":false,"name":"Phone"}]}"#)],
    );
    app.command(cmd("spotify devices"), &out);
    let listed = settle(&mut rx, |s| s["devices"].as_array().is_some_and(|d| !d.is_empty())).await;
    assert_eq!(listed["devices"][0]["name"], "Phone");
    app.command(cmd("spotify transfer not-listed"), &out);
    let refused = settle(&mut rx, |s| s["error"].as_str().is_some_and(|e| e.contains("unknown device"))).await;
    assert_eq!(refused["source"], "mpris");
    assert_eq!(https.count("PUT", "/v1/me/player"), 0, "nothing was sent for a device the phone made up");

    // The desktop app quits: the Web API takes over, and says when Premium is missing.
    https.script("GET", "/v1/me/player?", vec![reply(200, &player_state("Web Track", false))]);
    https.script(
        "PUT",
        "/v1/me/player/play",
        vec![reply(403, r#"{"error":{"status":403,"message":"Player command failed: Premium required","reason":"PREMIUM_REQUIRED"}}"#)],
    );
    standin.tell("quit");
    standin.quit();
    settle(&mut rx, |s| s["desktop"] == "not running").await;
    app.command(cmd("spotify play"), &out);
    let premium = settle(&mut rx, |s| s["error"].as_str().is_some_and(|e| e.contains("Premium"))).await;
    assert_eq!((premium["source"].as_str(), premium["track"].as_str()), (Some("web"), Some("Web Track")));
    let (_, _, notices) = states(&mut rx);
    let _ = notices;
    assert_eq!(https.count("PUT", "/v1/me/player/play"), 1);
    // Malformed commands are refused before anything is sent.
    let before = https.seen().len();
    for line in [
        "spotify volume 400",
        "spotify open https://evil.example/x",
        "spotify queue spotify:album:2noRn2Aes5aoNVsU6iWThc",
    ] {
        app.command(cmd(line), &out);
    }
    settle(&mut rx, |s| s["error"].as_str().is_some_and(|e| e.contains("queue takes"))).await;
    assert_eq!(https.seen().len(), before);
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scratch XDG layout with this configuration (never the user's own directories).
fn scratch_roots(dir: &Path, config: &str) -> Vec<(String, PathBuf)> {
    let roots: Vec<(String, PathBuf)> = ["CONFIG", "DATA", "CACHE", "STATE"]
        .iter()
        .map(|r| (format!("XDG_{r}_HOME"), dir.join(r.to_ascii_lowercase())))
        .collect();
    for (_, path) in &roots {
        std::fs::create_dir_all(path.join("bastion-mc")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path.join("bastion-mc"), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let file = dir.join("config/bastion-mc/config.toml");
    std::fs::write(&file, config).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    roots
}

/// `bastion-mc spotify status` is the launch's MPRIS path from the command line: the proxy
/// in its sandbox, the client, the player's properties.
#[test]
fn spotify_status_finds_the_desktop_app_through_the_proxy() {
    if !available() {
        return;
    }
    let dir = scratch("status");
    let bus = PrivateBus::start(&dir);
    let _standin = Standin::start(&bus, mpris::PLAYER);
    let roots = scratch_roots(&dir, "[auth]\ntoken_store = \"none\"\n");
    let output = Command::new(env!("CARGO_BIN_EXE_bastion-mc"))
        .args(["spotify", "status"])
        .env_clear()
        .envs(roots)
        .env("HOME", &dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("Spotify is running — Harder, Better, Faster, Stronger — Daft Punk (paused)"), "{text}");
    assert!(text.contains("Web API: off (no [spotify] client_id)"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The token keeper as a launch starts it — this program in its keeper mode, its
/// configuration from the XDG directories it inherits — answers over the socket pair. With
/// nothing stored (token_store = "none") the answer says why there is no token.
#[tokio::test(flavor = "multi_thread")]
async fn the_token_keeper_process_answers_on_its_socket() {
    let dir = scratch("keeper");
    let roots =
        scratch_roots(&dir, &format!("[auth]\ntoken_store = \"none\"\n[spotify]\nclient_id = \"{CLIENT_ID}\"\n"));
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bastion-mc"))
        .arg(spotify::keeper::KEEPER_ARG)
        .env_clear()
        .envs(roots)
        .env("HOME", &dir)
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs)))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    ours.set_nonblocking(true).unwrap();
    let client = spotify::keeper::Client::over(tokio::net::UnixStream::from_std(ours).unwrap());
    use spotify_web::TokenSource;
    assert_eq!(client.token(false).await.unwrap_err(), WebError::SignedOut("not signed in".into()));
    // It registered itself for scram while it ran …
    let markers = std::fs::read_dir(dir.join("state/bastion-mc/running")).map(|d| d.count()).unwrap_or(0);
    assert_eq!(markers, 0, "without a token it has nothing to store, and no marker");
    // … and ends when the launcher's end closes.
    drop(client);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the keeper did not end with its socket");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `spotify login` up to the tokens, against the stand-in: the URL a browser is shown, the
/// callback on the loopback port, the code exchange with the PKCE verifier.
#[tokio::test(flavor = "multi_thread")]
async fn the_sign_in_exchanges_the_code_with_its_verifier() {
    let https = standin_https::start().await;
    https.script("POST", "/api/token", vec![reply(200, TOKEN_1)]);
    let reporter = bastion_mc::events::Reporter::capture();
    let client = https.client();
    let signing_in = {
        let reporter = reporter.clone();
        tokio::spawn(async move { spotify::login::sign_in(&client, CLIENT_ID, None, false, &reporter).await })
    };
    // The browser: open the URL the launcher showed and come back with a code.
    let deadline = Instant::now() + Duration::from_secs(10);
    let url = loop {
        let shown = reporter.captured().into_iter().find_map(|e| match e {
            bastion_mc_proto::Event::BrowserLogin { url, .. } => Some(url),
            _ => None,
        });
        if let Some(url) = shown {
            break url;
        }
        assert!(Instant::now() < deadline, "no sign-in URL was shown");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let param = |name: &str| {
        let start = url.find(&format!("{name}=")).unwrap() + name.len() + 1;
        url[start..].split('&').next().unwrap().to_string()
    };
    assert!(url.starts_with("https://accounts.spotify.com/authorize?"));
    let redirect = param("redirect_uri").replace("%3A", ":").replace("%2F", "/");
    assert!(redirect.starts_with("http://127.0.0.1:") && redirect.ends_with("/callback"), "{redirect}");
    let port: u16 = redirect["http://127.0.0.1:".len()..redirect.len() - "/callback".len()].parse().unwrap();
    let (state, challenge) = (param("state"), param("code_challenge"));
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut browser = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    browser
        .write_all(
            format!("GET /callback?code=AQC-the-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut page = String::new();
    let _ = browser.read_to_string(&mut page).await;
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("Signed in to Spotify"), "{page}");
    let tokens = signing_in.await.unwrap().unwrap();
    assert_eq!(tokens.access.expose(), b"BQ-ACCESS-1");
    assert_eq!(tokens.refresh.as_ref().unwrap().expose(), b"AQ-REFRESH-2");
    // The exchange: the code, the same redirect URI, and the verifier whose hash is the
    // challenge the browser was given.
    let exchange = https.seen().into_iter().find(|s| s.path == "/api/token").unwrap();
    let form: std::collections::BTreeMap<String, String> = exchange
        .body
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.replace("%3A", ":").replace("%2F", "/")))
        .collect();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], "AQC-the-code");
    assert_eq!(form["redirect_uri"], redirect);
    assert_eq!(form["client_id"], CLIENT_ID);
    let verifier = &form["code_verifier"];
    let hashed = util::b64url_encode(&util::hash_bytes(util::HashAlg::Sha256, verifier.as_bytes()));
    assert_eq!(hashed, challenge, "S256(verifier) is the challenge");
    assert!(!exchange.body.contains("client_secret"));
}
