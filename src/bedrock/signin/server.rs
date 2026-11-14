//! The launcher's side of the bridge: from a request in the sandbox to a page shown outside
//! the game, and from the user's answer back to the game.
//!
//! ```text
//!  sandbox                         launcher (confined, step 10)            outside the game
//!  ───────                         ────────────────────────────            ────────────────
//!  client ─exec─▶ stub ──frame──▶  check_request ─▶ prompt ──event/text──▶ front end, terminal
//!                                       (one at a time,                     │ the user's browser
//!         stub ◀──frame── address ◀─ check_final ◀── answer ◀──trusted input┘ (Microsoft's page)
//! ```
//!
//! [`serve`] runs on the listener [`Bridge`] bound before the launcher confined itself; it
//! only accepts on it. The rules:
//!
//! * **What is shown**: only a request that passes [`check::check_request`]. Anything else is
//!   answered with an error, written to the log once per reason, and — because a refusal of
//!   the real client's request would mean that the user cannot sign in — said to the user
//!   once per reason.
//! * **How much**: one prompt at a time; at most [`MAX_REQUESTS`] per session; after a prompt
//!   that ended without an address, [`COOLDOWN`] before the next (a game cannot reopen a
//!   prompt the moment the user cancelled it); connections are taken at a bounded rate and
//!   read by at most [`MAX_READERS`] tasks at once, each for at most [`READ_TIMEOUT`].
//! * **How long**: a prompt waits [`PROMPT_TTL`], takes up to [`MAX_ANSWERS`] answers that
//!   are not its address (each says why), and ends when the user cancels, when the
//!   connection closes (the game stopped waiting), or when the launch ends.
//! * **What goes back**: only an address that passes [`check::check_final`], rebuilt from the
//!   code and the state. It is never logged and never sent as an event.
//!
//! Answers come from [`Reporter::answers`] (kind `signin`): the front end's control pipe or
//! the terminal, never the game — and, when this session holds it, from the desktop's handler
//! for the address's scheme (`handback`): an address the browser was sent to, delivered by
//! `bastion-mc bedrock hand-back` on a socket bound before the launcher confined itself. It is
//! checked exactly as a pasted answer is and has the same outcome; only a refused one does not
//! use up one of the prompt's answers (see `handback`).

use std::{
    collections::BTreeMap,
    os::unix::net::UnixListener as StdUnixListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Semaphore, mpsc, oneshot},
};

use super::{
    Bridge, REDIRECT_URI,
    check::{self, Checked},
    handback::{self, Handback, Held},
    wire::{self, Reply},
};
use crate::{
    error::{Error, Result},
    events::{Answer, AnswerKind, AnswerReceiver, Event, Reporter, SignInPage, SignInState},
};

/// How long a prompt waits for its answer (see `stub` for why this long).
pub const PROMPT_TTL: Duration = Duration::from_secs(15 * 60);
/// Prompts per session. Signing in takes one; a few more allow for a wrong account, a
/// cancelled attempt or a sign-in the game asks for again later.
pub const MAX_REQUESTS: u32 = 6;
/// After a prompt ended without an address, how long until the next is shown.
pub const COOLDOWN: Duration = Duration::from_secs(10);
/// Answers per prompt that are not its address.
pub const MAX_ANSWERS: u32 = 10;
/// Connections whose request is being read at once.
pub const MAX_READERS: usize = 4;
/// How long a request may take to arrive once connected. The stub sends it at once.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections taken per second on average, and in a burst: the real client connects once
/// per sign-in; a game that connects in a loop is slowed down, not served.
const ACCEPT_RATE: f64 = 4.0;
const ACCEPT_BURST: f64 = 8.0;
/// How long writing a reply may take. A reply is small and fits the socket's buffer.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the server writes its lines (the launch log). Never handed an address or a code.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The limits of one server. [`Options::default`] is what a launch uses; tests shorten it.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub ttl: Duration,
    pub max_requests: u32,
    pub cooldown: Duration,
    pub max_answers: u32,
    pub read_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            ttl: PROMPT_TTL,
            max_requests: MAX_REQUESTS,
            cooldown: COOLDOWN,
            max_answers: MAX_ANSWERS,
            read_timeout: READ_TIMEOUT,
        }
    }
}

/// The running server. [`Serving::shutdown`] (or dropping it) ends it: a waiting prompt is
/// withdrawn and the game told that the sign-in failed.
pub struct Serving {
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
    signed_in: Arc<AtomicBool>,
    /// The desktop's handler, when this session holds it: released (its name removed, if the
    /// launcher may still do that, then its lock) once the server has stopped.
    handback: Option<Held>,
}

impl Serving {
    /// End the server; whether a sign-in was handed to the game in this session (an address
    /// that passed every check reached the stub), for what the launch says at its end.
    pub async fn shutdown(mut self) -> bool {
        drop(self.stop.take());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut self.task).await;
        drop(self.handback.take());
        self.signed_in.load(Ordering::SeqCst)
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        // Dropping the sender ends the loop on its own; nothing to wait for here.
        drop(self.stop.take());
    }
}

/// Serve the bridge — and the desktop's handler, when this session took it
/// (`handback::take`) — for the rest of the launch. Call it inside the launch's runtime, after
/// the launcher has confined itself (it binds nothing and opens nothing; it only accepts).
pub fn serve(bridge: Bridge, handback: Option<Handback>, reporter: &Reporter, log: Log) -> Result<Serving> {
    serve_with_handback(bridge.into_listener(), handback, reporter, log, Options::default())
}

/// [`serve`] with a listener and limits of the caller's choosing, and no handler (tests).
pub fn serve_with(listener: StdUnixListener, reporter: &Reporter, log: Log, options: Options) -> Result<Serving> {
    serve_with_handback(listener, None, reporter, log, options)
}

/// [`serve`] with a listener, the handler and limits of the caller's choosing (tests).
pub fn serve_with_handback(
    listener: StdUnixListener,
    handback: Option<Handback>,
    reporter: &Reporter,
    log: Log,
    options: Options,
) -> Result<Serving> {
    let tokio_listener = |listener: StdUnixListener, what: &str| -> Result<UnixListener> {
        listener.set_nonblocking(true).map_err(|e| Error::io(format!("preparing {what}"), e))?;
        UnixListener::from_std(listener).map_err(|e| Error::io(format!("preparing {what}"), e))
    };
    let listener = tokio_listener(listener, "the sign-in bridge")?;
    let (handback_listener, held) = match handback {
        Some(handback) => {
            let (listener, held) = handback.into_parts();
            (Some(tokio_listener(listener, "the desktop's handler for sign-in addresses")?), Some(held))
        }
        None => (None, None),
    };
    let answers = reporter.answers(AnswerKind::SignIn)?;
    let (stop, stopped) = oneshot::channel();
    let signed_in = Arc::new(AtomicBool::new(false));
    let session = Session {
        reporter: reporter.clone(),
        log,
        options,
        answers,
        pending: None,
        accepted: 0,
        next_allowed: None,
        refusals: BTreeMap::new(),
        signed_in: signed_in.clone(),
        handback: handback_listener.is_some(),
        handback_refusals: BTreeMap::new(),
    };
    let task = tokio::spawn(session.run(listener, handback_listener, stopped));
    Ok(Serving { stop: Some(stop), task, signed_in, handback: held })
}

/// A prompt that waits.
struct Pending {
    stream: UnixStream,
    request: Checked,
    expires: Instant,
    answers_left: u32,
}

struct Session {
    reporter: Reporter,
    log: Log,
    options: Options,
    answers: AnswerReceiver,
    pending: Option<Pending>,
    accepted: u32,
    next_allowed: Option<Instant>,
    /// Refused requests per reason (each reason is logged once, with the count at the end).
    refusals: BTreeMap<&'static str, u32>,
    /// Set once an address was handed to the game.
    signed_in: Arc<AtomicBool>,
    /// This session holds the desktop's handler (the prompt says so).
    handback: bool,
    /// Addresses from the handler that were refused, per reason (logged once each).
    handback_refusals: BTreeMap<&'static str, u32>,
}

/// An address from the desktop's handler, read from a connection, or why none could be.
type Handed = (UnixStream, std::result::Result<String, &'static str>);

/// Read the one line `bastion-mc bedrock hand-back` sends: the address, bounded, printable.
async fn read_handed(mut stream: UnixStream, timeout: Duration, to: mpsc::Sender<Handed>) {
    let ours = stream.peer_cred().is_ok_and(|cred| cred.uid() == nix::unistd::getuid().as_raw());
    let line = if !ours {
        Err("the program that delivered it is not the user's")
    } else {
        match tokio::time::timeout(timeout, read_line(&mut stream)).await {
            Err(_) => Err("no address arrived in time"),
            Ok(line) => line,
        }
    };
    let _ = to.send((stream, line)).await;
}

async fn read_line(stream: &mut UnixStream) -> std::result::Result<String, &'static str> {
    let mut line = Vec::with_capacity(256);
    let mut chunk = [0u8; 512];
    loop {
        let n = stream.read(&mut chunk).await.map_err(|_| "the connection failed")?;
        if n == 0 {
            return Err("the connection ended before a whole line");
        }
        line.extend_from_slice(&chunk[..n]);
        if let Some(end) = line.iter().position(|b| *b == b'\n') {
            line.truncate(end);
            let text = String::from_utf8(line).map_err(|_| "the address is not text")?;
            bastion_mc_proto::answer::check_answer_text(&text)?;
            return Ok(text);
        }
        if line.len() >= handback::MAX_LINE {
            return Err("the address is too long");
        }
    }
}

/// A request read from a connection, or why none could be.
type Incoming = (UnixStream, std::result::Result<wire::Request, &'static str>);

/// Resolves when the peer of `stream` closes its end or sends anything (the stub never does
/// after its request): either way the game is no longer waiting for this prompt.
async fn peer_gone(stream: &UnixStream) {
    let mut scrap = [0u8; 64];
    loop {
        if stream.readable().await.is_err() {
            return;
        }
        match stream.try_read(&mut scrap) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            _ => return,
        }
    }
}

async fn read_request(mut stream: UnixStream, timeout: Duration, to: mpsc::Sender<Incoming>) {
    let parsed = match tokio::time::timeout(timeout, wire::read_frame(&mut stream)).await {
        Err(_) => Err("no request arrived in time"),
        Ok(Err(why)) => Err(why),
        Ok(Ok(body)) => wire::parse_request(&body),
    };
    let _ = to.send((stream, parsed)).await;
}

async fn reply(stream: &mut UnixStream, reply: &Reply) -> bool {
    let frame = wire::reply_frame(reply);
    matches!(tokio::time::timeout(WRITE_TIMEOUT, stream.write_all(&frame)).await, Ok(Ok(())))
}

/// A token bucket for accepting connections.
struct Pace {
    tokens: f64,
    at: Instant,
}

impl Pace {
    /// How long to wait before the next connection may be taken (and take it).
    fn take(&mut self) -> Duration {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * ACCEPT_RATE).min(ACCEPT_BURST);
        self.at = now;
        self.tokens -= 1.0;
        if self.tokens >= 0.0 { Duration::ZERO } else { Duration::from_secs_f64(-self.tokens / ACCEPT_RATE) }
    }
}

impl Session {
    async fn run(mut self, listener: UnixListener, handback: Option<UnixListener>, mut stopped: oneshot::Receiver<()>) {
        let (to_session, mut requests) = mpsc::channel::<Incoming>(MAX_READERS);
        let readers = Arc::new(Semaphore::new(MAX_READERS));
        let mut pace = Pace { tokens: ACCEPT_BURST, at: Instant::now() };
        // While set, no connection is taken (the pace, or a failing accept).
        let mut accept_after: Option<tokio::time::Instant> = None;
        let mut answers_open = true;
        // The desktop's handler: paced and bounded like the bridge, on its own budget.
        let (to_handed, mut handed) = mpsc::channel::<Handed>(MAX_READERS);
        let handback_readers = Arc::new(Semaphore::new(MAX_READERS));
        let mut handback_pace = Pace { tokens: ACCEPT_BURST, at: Instant::now() };
        let mut handback_after: Option<tokio::time::Instant> = None;
        loop {
            let waiting = self.pending.as_ref().map(|p| (&p.stream, tokio::time::Instant::from_std(p.expires)));
            tokio::select! {
                _ = &mut stopped => break,
                accepted = async { handback.as_ref().expect("guarded").accept().await },
                    if handback.is_some() && handback_after.is_none() => {
                    let wait = handback_pace.take();
                    match accepted {
                        Ok((stream, _)) => match handback_readers.clone().try_acquire_owned() {
                            Ok(permit) => {
                                let (to, timeout) = (to_handed.clone(), self.options.read_timeout);
                                tokio::spawn(async move {
                                    read_handed(stream, timeout, to).await;
                                    drop(permit);
                                });
                            }
                            Err(_) => self.refused_handback("too many deliveries at once"),
                        },
                        Err(_) => handback_after = Some(tokio::time::Instant::now() + Duration::from_millis(100)),
                    }
                    if !wait.is_zero() {
                        handback_after = Some(handback_after.unwrap_or_else(tokio::time::Instant::now) + wait);
                    }
                }
                _ = async { tokio::time::sleep_until(handback_after.expect("guarded")).await }, if handback_after.is_some() => {
                    handback_after = None;
                }
                Some((stream, line)) = handed.recv() => self.handed(stream, line).await,
                accepted = listener.accept(), if accept_after.is_none() => {
                    let wait = pace.take();
                    match accepted {
                        Ok((stream, _)) => match readers.clone().try_acquire_owned() {
                            Ok(permit) => {
                                let (to, timeout) = (to_session.clone(), self.options.read_timeout);
                                tokio::spawn(async move {
                                    read_request(stream, timeout, to).await;
                                    drop(permit);
                                });
                            }
                            Err(_) => self.refused("too many connections at once", false),
                        },
                        // Out of descriptors or similar: wait rather than spin.
                        Err(_) => accept_after = Some(tokio::time::Instant::now() + Duration::from_millis(100)),
                    }
                    if !wait.is_zero() {
                        accept_after = Some(accept_after.unwrap_or_else(tokio::time::Instant::now) + wait);
                    }
                }
                _ = async { tokio::time::sleep_until(accept_after.expect("guarded")).await }, if accept_after.is_some() => {
                    accept_after = None;
                }
                Some((stream, parsed)) = requests.recv() => self.request(stream, parsed).await,
                answer = self.answers.recv(), if answers_open => match answer {
                    Some(answer) => self.answer(answer).await,
                    None => answers_open = false,
                },
                _ = async { peer_gone(waiting.expect("guarded").0).await }, if waiting.is_some() => {
                    self.end(SignInState::Cancelled, "the game stopped waiting for the sign-in").await;
                }
                _ = async { tokio::time::sleep_until(waiting.expect("guarded").1).await }, if waiting.is_some() => {
                    self.end(SignInState::Expired, "no answer in time").await;
                }
            }
        }
        if self.pending.is_some() {
            self.end(SignInState::Cancelled, "the launch is ending").await;
        }
        let total: u32 = self.refusals.values().sum();
        if total > 0 {
            let reasons: Vec<String> = self.refusals.iter().map(|(why, n)| format!("{why} ({n}×)")).collect();
            (self.log)(&format!("signin: {total} request(s) refused in this session: {}", reasons.join("; ")));
        }
        let total: u32 = self.handback_refusals.values().sum();
        if total > 0 {
            let reasons: Vec<String> = self.handback_refusals.iter().map(|(why, n)| format!("{why} ({n}×)")).collect();
            (self.log)(&format!(
                "signin: {total} address(es) from the desktop's handler refused in this session: {}",
                reasons.join("; ")
            ));
        }
    }

    /// Count a refused delivery of the desktop's handler; log its reason the first time.
    fn refused_handback(&mut self, why: &'static str) {
        let count = self.handback_refusals.entry(why).or_insert(0);
        *count += 1;
        if *count == 1 {
            (self.log)(&format!("signin: refused an address from the desktop's handler: {why}"));
        }
    }

    /// An address from the desktop's handler: checked as a pasted answer is, and answered on the
    /// handler's connection with the verdict (`ok …` or `no …`, one line).
    async fn handed(&mut self, mut stream: UnixStream, line: std::result::Result<String, &'static str>) {
        let (accepted, said) = match line {
            Err(why) => {
                self.refused_handback(why);
                (false, why.to_string())
            }
            Ok(text) => self.handed_address(&text).await,
        };
        let verdict = handback::verdict_line(accepted, &said);
        let _ = tokio::time::timeout(WRITE_TIMEOUT, stream.write_all(verdict.as_bytes())).await;
    }

    async fn handed_address(&mut self, text: &str) -> (bool, String) {
        let Some(pending) = self.pending.as_mut() else {
            self.refused_handback("no sign-in was waiting");
            return (false, "no Bedrock sign-in is waiting in the running session".into());
        };
        match check::check_final(text, &pending.request) {
            Ok(url) => {
                if reply(&mut pending.stream, &Reply::Url(url)).await {
                    (self.log)(
                        "signin: the address the sign-in ended on came from the desktop's handler and was handed to \
                         the game",
                    );
                    self.signed_in.store(true, Ordering::SeqCst);
                    self.finish(SignInState::Done, None);
                    (true, "handed to the game".into())
                } else {
                    self.end(SignInState::Cancelled, "the game stopped waiting before the address arrived").await;
                    (false, "the game stopped waiting for this sign-in".into())
                }
            }
            Err(why) => {
                // The prompt stays, and keeps its answers: any web page can make the browser call
                // the handler, and none can make an address of this sign-in.
                self.refused_handback(why);
                let note = format!("an address your browser handed back was refused: {why}");
                let page = self.page(self.pending.as_ref().expect("checked"), SignInState::Waiting, Some(&note));
                self.reporter.emit(Event::SignInPage(page));
                (false, format!("the running Bedrock session refused it: {why}"))
            }
        }
    }

    /// Count a refusal; log it (and, for `tell`, say it to the user) the first time its
    /// reason comes up.
    fn refused(&mut self, why: &'static str, tell: bool) {
        let count = self.refusals.entry(why).or_insert(0);
        *count += 1;
        if *count == 1 {
            (self.log)(&format!("signin: refused a request from the sandbox: {why}"));
            if tell {
                self.reporter.warn(format!(
                    "Bedrock sign-in: a request from the game was refused ({why}); the game was told that the \
                     sign-in failed"
                ));
            }
        }
    }

    async fn request(&mut self, mut stream: UnixStream, parsed: std::result::Result<wire::Request, &'static str>) {
        let now = Instant::now();
        let outcome = parsed.map_err(|why| (why, false)).and_then(|request| {
            let checked = check::check_request(&request.start, &request.end).map_err(|r| (r.0, true))?;
            if self.pending.is_some() {
                Err(("a sign-in is already waiting", true))
            } else if self.accepted >= self.options.max_requests {
                Err(("this session has asked for the most sign-ins it may", true))
            } else if self.next_allowed.is_some_and(|t| now < t) {
                Err(("the last sign-in ended a moment ago; try again in a few seconds", true))
            } else {
                Ok(checked)
            }
        });
        match outcome {
            Err((why, tell)) => {
                self.refused(why, tell);
                reply(&mut stream, &Reply::Error(format!("refused by bastion-mc: {why}"))).await;
            }
            Ok(request) => {
                self.accepted += 1;
                let pending = Pending {
                    stream,
                    request,
                    expires: now + self.options.ttl,
                    answers_left: self.options.max_answers,
                };
                self.answers.set_pending(true);
                (self.log)(
                    "signin: the game asked for a Microsoft sign-in; the page (login.live.com, Minecraft's client) is \
                     shown outside the game",
                );
                self.reporter.emit(Event::SignInPage(self.page(&pending, SignInState::Waiting, None)));
                self.pending = Some(pending);
            }
        }
    }

    fn page(&self, pending: &Pending, state: SignInState, note: Option<&str>) -> SignInPage {
        SignInPage {
            url: pending.request.url.clone(),
            end: REDIRECT_URI.to_string(),
            expires_in: match state {
                SignInState::Waiting => pending.expires.saturating_duration_since(Instant::now()).as_secs(),
                _ => 0,
            },
            attempts_left: pending.answers_left,
            state,
            note: note.map(str::to_string),
            handback: self.handback,
        }
    }

    async fn answer(&mut self, answer: Answer) {
        let Some(pending) = self.pending.as_mut() else {
            self.reporter.status("no Bedrock sign-in is waiting: that answer was ignored");
            return;
        };
        let text = match answer {
            Answer::Cancel => return self.end(SignInState::Cancelled, "cancelled outside the game").await,
            Answer::Text(text) => text,
        };
        match check::check_final(&text, &pending.request) {
            Ok(url) => {
                if reply(&mut pending.stream, &Reply::Url(url)).await {
                    (self.log)("signin: the address the sign-in ended on was handed to the game");
                    self.signed_in.store(true, Ordering::SeqCst);
                    self.finish(SignInState::Done, None);
                } else {
                    self.end(SignInState::Cancelled, "the game stopped waiting before the address arrived").await;
                }
            }
            Err(why) => {
                pending.answers_left = pending.answers_left.saturating_sub(1);
                if pending.answers_left == 0 {
                    return self
                        .end(SignInState::Failed, "too many answers that were not the address this sign-in ended on")
                        .await;
                }
                let page = self.page(self.pending.as_ref().expect("checked"), SignInState::Waiting, Some(why));
                self.reporter.emit(Event::SignInPage(page));
            }
        }
    }

    /// End the waiting prompt without an address: the game is told so.
    async fn end(&mut self, state: SignInState, why: &str) {
        if let Some(pending) = self.pending.as_mut() {
            reply(&mut pending.stream, &Reply::Error(format!("the sign-in was not completed: {why}"))).await;
            (self.log)(&format!("signin: the sign-in prompt ended without an address: {why}"));
        }
        self.finish(state, Some(why));
    }

    fn finish(&mut self, state: SignInState, note: Option<&str>) {
        let Some(pending) = self.pending.take() else { return };
        self.answers.set_pending(false);
        if state != SignInState::Done {
            self.next_allowed = Some(Instant::now() + self.options.cooldown);
        }
        self.reporter.emit(Event::SignInPage(self.page(&pending, state, note)));
        // Dropping the stream closes the connection: the stub has its reply.
    }
}

#[cfg(test)]
mod tests;
