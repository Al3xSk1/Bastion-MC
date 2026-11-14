//! The Spotify token keeper: a small process of this program (`bastion-mc
//! __spotify-keeper`) that holds the Spotify refresh token for a launch, outside the
//! launcher.
//!
//! **Why a process of its own.** An access token lasts an hour; a game session lasts
//! longer, so the Web API needs refreshing while the game runs. Spotify rotates refresh
//! tokens (a refresh can return a new one; the old one may then be spent), so each new one
//! has to be stored — and the launcher, once it has confined itself in step 9, can reach
//! neither the kernel keyring nor the vault. It also must not hold a refresh token while
//! the game runs: that is the launcher's standing promise (`auth`), and the launcher is the
//! process that handles what the game sends.
//!
//! So before step 9 the launcher starts this keeper. The keeper loads the refresh token
//! from the token store, refreshes when the launcher asks (one byte on a socket pair),
//! stores each rotated refresh token, and answers with an access token and its lifetime —
//! nothing else crosses. It never reads anything the game sends. It refreshes at most once
//! per [`MIN_INTERVAL`] and [`MAX_REFRESHES`] times in all, hands out the same access token
//! until shortly before it runs out (unless the launcher says the API refused it), and ends
//! when the launcher's end of the socket closes. A compromised launcher can therefore get
//! fresh access tokens from it — which it holds anyway — but never the refresh token.
//!
//! It registers itself for `scram` like a sign-in does (it stores tokens), so an emergency
//! stop ends it before credentials are removed.

use std::{
    os::{fd::FromRawFd, unix::net::UnixStream},
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::web::{self, TokenSource, WebError};
use crate::{
    auth::store::TokenStore,
    config::Config,
    error::{Error, Result},
    paths::Paths,
    secret::Secret,
};

/// `bastion-mc __spotify-keeper`.
pub const KEEPER_ARG: &str = "__spotify-keeper";
/// The shortest time between two refreshes.
pub const MIN_INTERVAL: Duration = Duration::from_secs(20);
/// Refreshes in one session (an hour's token: a day and a half of play, with margin).
pub const MAX_REFRESHES: u32 = 60;
/// A held access token is handed out again until this long before it runs out.
const REUSE_MARGIN: Duration = Duration::from_secs(300);
const REQUEST_TOKEN: u8 = 1;
const REQUEST_REFUSED: u8 = 2;
const REPLY_OK: u8 = 0;
const REPLY_ERROR: u8 = 1;
const MAX_TOKEN: usize = 4096;
const MAX_TEXT: usize = 300;
/// How long the launcher waits for an answer (a refresh is one HTTPS round trip).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------- the wire

fn error_kind(e: &WebError) -> (u8, u32, String) {
    match e {
        WebError::SignedOut(why) => (1, 0, why.clone()),
        WebError::Net(why) => (2, 0, why.clone()),
        WebError::Api { status, message } => (3, u32::from(*status), message.clone()),
        WebError::Limited { seconds } => (4, (*seconds).min(u64::from(u32::MAX)) as u32, String::new()),
        WebError::RateLimited { seconds } => (5, (*seconds).min(u64::from(u32::MAX)) as u32, String::new()),
        WebError::Premium => (6, 0, String::new()),
        WebError::NoDevice => (7, 0, String::new()),
    }
}

/// An answer on the wire: `0, u32 seconds, u16 length, token` or `1, u8 kind, u32 number,
/// u16 length, text`.
pub fn encode_reply(reply: &std::result::Result<(Secret, Duration), WebError>) -> Result<Secret> {
    let mut out = Secret::with_capacity(MAX_TOKEN + 16)?;
    match reply {
        Ok((token, valid)) => {
            if token.is_empty() || token.len() > MAX_TOKEN {
                return Err(Error::Auth("token of an impossible size".into()));
            }
            out.push(&[REPLY_OK])?;
            out.push(&(valid.as_secs().min(u64::from(u32::MAX)) as u32).to_be_bytes())?;
            out.push(&(token.len() as u16).to_be_bytes())?;
            out.push(token.expose())?;
        }
        Err(e) => {
            let (kind, number, text) = error_kind(e);
            let text = crate::util::sanitize_line(text.as_bytes(), MAX_TEXT);
            let text = &text.as_bytes()[..text.len().min(MAX_TEXT * 4)];
            out.push(&[REPLY_ERROR, kind])?;
            out.push(&number.to_be_bytes())?;
            out.push(&(text.len() as u16).to_be_bytes())?;
            out.push(text)?;
        }
    }
    Ok(out)
}

/// Read one answer, strictly.
async fn read_reply(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
) -> std::result::Result<(Secret, Duration), WebError> {
    let io = |e: std::io::Error| WebError::Net(format!("the token keeper: {e}"));
    match stream.read_u8().await.map_err(io)? {
        REPLY_OK => {
            let seconds = stream.read_u32().await.map_err(io)?;
            let len = usize::from(stream.read_u16().await.map_err(io)?);
            if len == 0 || len > MAX_TOKEN {
                return Err(WebError::Net("the token keeper sent a token of an impossible size".into()));
            }
            let mut token = Secret::with_capacity(len).map_err(|e| WebError::Net(e.to_string()))?;
            let mut chunk = [0u8; 256];
            let mut left = len;
            while left > 0 {
                let n = left.min(chunk.len());
                stream.read_exact(&mut chunk[..n]).await.map_err(io)?;
                token.push(&chunk[..n]).map_err(|e| WebError::Net(e.to_string()))?;
                left -= n;
            }
            zeroize::Zeroize::zeroize(&mut chunk);
            Ok((token, Duration::from_secs(u64::from(seconds).clamp(60, 86_400))))
        }
        REPLY_ERROR => {
            let kind = stream.read_u8().await.map_err(io)?;
            let number = stream.read_u32().await.map_err(io)?;
            let len = usize::from(stream.read_u16().await.map_err(io)?);
            if len > MAX_TEXT * 4 {
                return Err(WebError::Net("the token keeper sent an impossible answer".into()));
            }
            let mut text = vec![0u8; len];
            stream.read_exact(&mut text).await.map_err(io)?;
            let text = crate::util::sanitize_line(&text, MAX_TEXT);
            Err(match kind {
                1 => WebError::SignedOut(text),
                2 => WebError::Net(text),
                3 => WebError::Api { status: number.min(999) as u16, message: text },
                4 => WebError::Limited { seconds: u64::from(number).clamp(1, 3600) },
                5 => WebError::RateLimited { seconds: u64::from(number).clamp(1, 3600) },
                6 => WebError::Premium,
                7 => WebError::NoDevice,
                _ => WebError::Net("the token keeper sent an unknown answer".into()),
            })
        }
        _ => Err(WebError::Net("the token keeper sent an unknown answer".into())),
    }
}

// ---------------------------------------------------------------- the launcher's side

/// The launcher's end: a [`TokenSource`] that asks the keeper.
pub struct Client {
    stream: tokio::sync::Mutex<tokio::net::UnixStream>,
    child: std::sync::Mutex<Option<std::process::Child>>,
}

impl Client {
    /// Start the keeper (before step 9: this executes a program). `program` is this program.
    pub fn start(program: &Path) -> Result<Client> {
        let (ours, theirs) = UnixStream::pair().map_err(|e| Error::io("creating the token keeper's socket", e))?;
        let mut command = Command::new(program);
        command.arg(KEEPER_ARG).env_clear();
        // What finding the configuration and the token store takes; nothing else.
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "XDG_RUNTIME_DIR",
            "PATH",
            "TERM",
            "LANG",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let child = command
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::io("starting the Spotify token keeper", e))?;
        ours.set_nonblocking(true).map_err(|e| Error::io("the token keeper's socket", e))?;
        let stream = tokio::net::UnixStream::from_std(ours).map_err(|e| Error::io("the token keeper's socket", e))?;
        Ok(Client { stream: tokio::sync::Mutex::new(stream), child: std::sync::Mutex::new(Some(child)) })
    }

    /// Wrap a stream to a keeper served elsewhere (tests: `serve` in the same process).
    #[doc(hidden)]
    pub fn over(stream: tokio::net::UnixStream) -> Client {
        Client { stream: tokio::sync::Mutex::new(stream), child: std::sync::Mutex::new(None) }
    }
}

impl TokenSource for Client {
    fn token(&self, refused: bool) -> web::TokenFuture<'_> {
        Box::pin(async move {
            let mut stream = self.stream.lock().await;
            let ask = async {
                stream
                    .write_all(&[if refused { REQUEST_REFUSED } else { REQUEST_TOKEN }])
                    .await
                    .map_err(|e| WebError::Net(format!("the token keeper is gone ({e})")))?;
                read_reply(&mut *stream).await
            };
            tokio::time::timeout(ANSWER_TIMEOUT, ask)
                .await
                .map_err(|_| WebError::Net("the token keeper did not answer in time".into()))?
        })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Its socket closes with us, which ends it; this makes sure.
        if let Ok(mut slot) = self.child.lock()
            && let Some(mut child) = slot.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------- the keeper's side

/// What the keeper holds while it serves.
pub struct Keeper {
    source: std::result::Result<Arc<dyn TokenSource>, WebError>,
    held: Option<(Secret, Instant)>,
    last_refresh: Option<Instant>,
    refreshes: u32,
}

impl Keeper {
    /// `source`: the refresh token's [`TokenSource`], or why there is none (the answer to
    /// every request then).
    pub fn new(source: std::result::Result<Arc<dyn TokenSource>, WebError>) -> Keeper {
        Keeper { source, held: None, last_refresh: None, refreshes: 0 }
    }

    async fn answer(&mut self, refused: bool, now: Instant) -> std::result::Result<(Secret, Duration), WebError> {
        let source = self.source.as_ref().map_err(Clone::clone)?.clone();
        if !refused
            && let Some((token, until)) = &self.held
            && now + REUSE_MARGIN < *until
        {
            return Ok((
                token.try_clone().map_err(|e| WebError::Net(e.to_string()))?,
                until.saturating_duration_since(now),
            ));
        }
        if let Some(last) = self.last_refresh
            && now.saturating_duration_since(last) < MIN_INTERVAL
        {
            return Err(WebError::Limited {
                seconds: (MIN_INTERVAL - now.saturating_duration_since(last)).as_secs().max(1),
            });
        }
        if self.refreshes >= MAX_REFRESHES {
            return Err(WebError::SignedOut("the keeper's refreshes for this session are used up".into()));
        }
        self.last_refresh = Some(now);
        self.refreshes += 1;
        let (token, valid) = source.token(refused).await?;
        self.held = Some((token.try_clone().map_err(|e| WebError::Net(e.to_string()))?, now + valid));
        Ok((token, valid))
    }

    /// Answer requests on `stream` until it closes or says something that is not a request.
    pub async fn serve(mut self, mut stream: tokio::net::UnixStream) {
        loop {
            let refused = match stream.read_u8().await {
                Ok(REQUEST_TOKEN) => false,
                Ok(REQUEST_REFUSED) => true,
                _ => return,
            };
            let answer = self.answer(refused, Instant::now()).await;
            let Ok(bytes) = encode_reply(&answer) else { return };
            if stream.write_all(bytes.expose()).await.is_err() {
                return;
            }
        }
    }
}

/// Entry point for `bastion-mc __spotify-keeper`, with the launcher's socket as standard
/// input. Never returns.
pub fn keeper_main() -> ! {
    // SAFETY: the launcher started this process with its socket as fd 0 and nothing else
    // uses fd 0 here.
    let std_stream = unsafe { UnixStream::from_raw_fd(0) };
    let code = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime.block_on(async {
            let (source, _marker) = load();
            let _ = std_stream.set_nonblocking(true);
            match tokio::net::UnixStream::from_std(std_stream) {
                Ok(stream) => {
                    Keeper::new(source).serve(stream).await;
                    0
                }
                Err(_) => 1,
            }
        }),
        Err(_) => 1,
    };
    std::process::exit(code)
}

/// The refresh token and how to refresh it, from the configuration and the token store
/// this process finds in its environment (the launcher's XDG directories).
fn load() -> (std::result::Result<Arc<dyn TokenSource>, WebError>, Option<crate::running::Marker>) {
    let setup = || -> std::result::Result<(Arc<dyn TokenSource>, crate::running::Marker), WebError> {
        let fail = |e: Error| WebError::SignedOut(crate::util::sanitize_line(e.to_string().as_bytes(), 200));
        let paths = Paths::from_env().map_err(fail)?;
        let cfg = Config::load(&paths).map_err(fail)?;
        let client_id = cfg
            .spotify
            .client_id
            .clone()
            .ok_or_else(|| WebError::SignedOut("[spotify] client_id is not set".into()))?;
        let marker = crate::running::Marker::for_command(&paths, "spotify-keeper").map_err(fail)?;
        let store = Arc::new(TokenStore::open(&cfg.auth, &paths).map_err(fail)?);
        let refresh = store.load_spotify().map_err(fail)?.ok_or_else(|| WebError::SignedOut("not signed in".into()))?;
        let http = super::client(&cfg).map_err(fail)?;
        let saver = store.clone();
        let source = web::Refreshing::new(
            http,
            &client_id,
            refresh,
            Box::new(move |next: &Secret| saver.save_spotify(next).map_err(fail)),
        );
        Ok((Arc::new(source), marker))
    };
    match setup() {
        Ok((source, marker)) => (Ok(source), Some(marker)),
        Err(e) => (Err(e), None),
    }
}

/// The reply reader on arbitrary bytes.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().build() else { return };
    runtime.block_on(async {
        let mut input = data;
        if let Ok((token, valid)) = read_reply(&mut input).await {
            assert!(!token.is_empty() && token.len() <= MAX_TOKEN);
            assert!(valid >= Duration::from_secs(60) && valid <= Duration::from_secs(86_400));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokens numbered by refresh; counts what it was asked.
    struct Counting(std::sync::Mutex<u32>);

    impl TokenSource for Counting {
        fn token(&self, _refused: bool) -> web::TokenFuture<'_> {
            Box::pin(async move {
                let mut n = self.0.lock().unwrap();
                *n += 1;
                Ok((Secret::from_slice(format!("ACCESS-{n}").as_bytes()).unwrap(), Duration::from_secs(3600)))
            })
        }
    }

    #[tokio::test]
    async fn the_keeper_reuses_refreshes_sparingly_and_says_why_not() {
        let counting = Arc::new(Counting(std::sync::Mutex::new(0)));
        let mut keeper = Keeper::new(Ok(counting.clone()));
        let t0 = Instant::now();
        let (first, valid) = keeper.answer(false, t0).await.unwrap();
        assert_eq!((first.expose(), valid), (&b"ACCESS-1"[..], Duration::from_secs(3600)));
        // Asked again: the same token, no refresh.
        let (again, _) = keeper.answer(false, t0 + Duration::from_secs(600)).await.unwrap();
        assert_eq!(again.expose(), b"ACCESS-1");
        // Refused by the API, but too soon after the last refresh: wait.
        assert!(matches!(keeper.answer(true, t0 + Duration::from_secs(5)).await, Err(WebError::Limited { .. })));
        let (fresh, _) = keeper.answer(true, t0 + Duration::from_secs(30)).await.unwrap();
        assert_eq!(fresh.expose(), b"ACCESS-2");
        // Close to expiry: refreshed without being asked to.
        let (late, _) = keeper.answer(false, t0 + Duration::from_secs(30 + 3600 - 200)).await.unwrap();
        assert_eq!(late.expose(), b"ACCESS-3");
        assert_eq!(*counting.0.lock().unwrap(), 3);
        // The session's refreshes run out.
        keeper.refreshes = MAX_REFRESHES;
        assert!(matches!(keeper.answer(true, t0 + Duration::from_secs(9000)).await, Err(WebError::SignedOut(_))));
        // Without a refresh token, every answer says why.
        let mut none = Keeper::new(Err(WebError::SignedOut("not signed in".into())));
        assert_eq!(none.answer(false, t0).await.unwrap_err(), WebError::SignedOut("not signed in".into()));
    }

    #[tokio::test]
    async fn the_launcher_and_the_keeper_understand_each_other() {
        let (ours, theirs) = tokio::net::UnixStream::pair().unwrap();
        let counting = Arc::new(Counting(std::sync::Mutex::new(0)));
        let served = tokio::spawn(Keeper::new(Ok(counting)).serve(theirs));
        let client = Client::over(ours);
        let (token, valid) = client.token(false).await.unwrap();
        assert_eq!((token.expose(), valid), (&b"ACCESS-1"[..], Duration::from_secs(3600)));
        assert!(matches!(client.token(true).await, Err(WebError::Limited { .. })));
        drop(client);
        served.await.unwrap();
        // Every error kind survives the wire.
        for e in [
            WebError::SignedOut("revoked".into()),
            WebError::Net("down".into()),
            WebError::Api { status: 503, message: "busy".into() },
            WebError::Limited { seconds: 7 },
            WebError::RateLimited { seconds: 30 },
            WebError::Premium,
            WebError::NoDevice,
        ] {
            let (mut a, mut b) = tokio::net::UnixStream::pair().unwrap();
            a.write_all(encode_reply(&Err(e.clone())).unwrap().expose()).await.unwrap();
            assert_eq!(read_reply(&mut b).await.unwrap_err(), e);
        }
    }

    #[test]
    fn the_reply_reader_survives_its_fuzz_seeds() {
        fuzz(&[0, 0, 0, 14, 16, 0, 3, b'a', b'b', b'c']);
        fuzz(&[1, 9, 0, 0, 0, 0, 0xff, 0xff]);
        fuzz(&[0, 0, 0, 0, 0, 0, 0]);
    }
}
