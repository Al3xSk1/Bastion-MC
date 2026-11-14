//! The server's rules, over a real socket: requests as the stub (or a hostile game) sends
//! them, answers as the trusted input delivers them, and what the user is shown.

use std::{
    os::unix::net::UnixListener as StdUnixListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::{io::AsyncWriteExt, net::UnixStream};

use super::{Log, Options, Serving, serve_with};
use crate::{
    bedrock::signin::{
        REDIRECT_URI, START_PREFIX,
        wire::{self, Reply},
    },
    events::{Answer, AnswerKind, Delivery, Event, Reporter, SignInPage, SignInState},
};

const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const CODE: &str = "M.C507_BL2.2.U.0e5b2bbb-1b34-4e1b-9d29-2f7c0a4f9d4b";

fn start_url(state: &str) -> String {
    format!(
        "{START_PREFIX}code_challenge={CHALLENGE}&code_challenge_method=S256&state={state}\
         &client_id=0000000048183522&response_type=code&scope=service%3A%3Auser.auth.xboxlive.com%3A%3AMBI_SSL\
         &redirect_uri=ms-xal-0000000048183522%3A%2F%2Fauth&display=android_phone"
    )
}

fn final_url(state: &str) -> String {
    format!("{REDIRECT_URI}?code={CODE}&state={state}&lc=1033")
}

/// A socket directory of its own (short: socket paths are limited to 108 bytes).
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("bmc-si-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("signin.sock")
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    dir: Dir,
    reporter: Reporter,
    log: Arc<Mutex<Vec<String>>>,
    serving: Option<Serving>,
}

impl Harness {
    fn start(tag: &str, options: Options) -> Harness {
        let dir = Dir::new(tag);
        let listener = StdUnixListener::bind(dir.socket()).unwrap();
        let reporter = Reporter::capture();
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let logger: Log = Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_string()));
        let serving = serve_with(listener, &reporter, logger, options).unwrap();
        Harness { dir, reporter, log, serving: Some(serving) }
    }

    /// Connect and send a request; the connection, for the reply.
    async fn request(&self, start: &str, end: &str) -> UnixStream {
        let mut stream = UnixStream::connect(self.dir.socket()).await.unwrap();
        stream.write_all(&wire::request_frame(start, end).unwrap()).await.unwrap();
        stream
    }

    fn pages(&self) -> Vec<SignInPage> {
        self.reporter
            .captured()
            .into_iter()
            .filter_map(|e| if let Event::SignInPage(p) = e { Some(p) } else { None })
            .collect()
    }

    /// Wait until `n` prompt events have been sent; the last one.
    async fn page(&self, n: usize) -> SignInPage {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let pages = self.pages();
            if pages.len() >= n {
                return pages[n - 1].clone();
            }
            assert!(Instant::now() < deadline, "prompt event {n} did not come: {:?}", self.reporter.captured());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn answer(&self, answer: Answer) {
        assert_eq!(self.reporter.answer_for_test(AnswerKind::SignIn, answer), Delivery::Delivered);
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

async fn reply(stream: &mut UnixStream) -> Reply {
    let body = tokio::time::timeout(Duration::from_secs(10), wire::read_frame(stream)).await.expect("a reply in time");
    wire::parse_reply(&body.expect("a whole frame")).expect("a reply")
}

fn error_text(reply: Reply) -> String {
    match reply {
        Reply::Error(text) => text,
        Reply::Url(url) => panic!("an address came back: {url}"),
    }
}

#[tokio::test]
async fn a_sign_in_is_shown_outside_the_game_and_only_its_rebuilt_address_goes_back() {
    let h = Harness::start("ok", Options::default());
    assert_eq!(h.reporter.captured()[0], Event::Answerable { kind: AnswerKind::SignIn }, "the front end is told");
    let mut stream = h.request(&start_url("st4te"), REDIRECT_URI).await;
    let page = h.page(1).await;
    assert_eq!(page.state, SignInState::Waiting);
    assert_eq!(page.url, start_url("st4te"));
    assert_eq!(page.end, REDIRECT_URI);
    assert!(page.expires_in > 14 * 60 && page.attempts_left == 10, "{page:?}");
    h.answer(Answer::Text(final_url("st4te")));
    assert_eq!(reply(&mut stream).await, Reply::Url(format!("{REDIRECT_URI}?code={CODE}&state=st4te")));
    let done = h.page(2).await;
    assert_eq!((done.state, done.note), (SignInState::Done, None));
    // Neither the address nor the code was logged or sent as an event.
    let everything = format!("{:?}{:?}", h.log(), h.reporter.captured());
    assert!(!everything.contains(CODE), "{everything}");
    assert!(h.log().iter().any(|l| l.contains("handed to the game")), "{:?}", h.log());
}

#[tokio::test]
async fn pages_bastion_does_not_show_are_refused_logged_once_and_said_once() {
    let h = Harness::start("refuse", Options::default());
    let evil = start_url("s").replace("0000000048183522&", "00000000402b5328&");
    for _ in 0..3 {
        let mut stream = h.request(&evil, REDIRECT_URI).await;
        assert!(error_text(reply(&mut stream).await).contains("Minecraft's client"));
    }
    let mut stream =
        h.request(&start_url("s").replace("https://login.live.com", "https://evil.example"), REDIRECT_URI).await;
    assert!(error_text(reply(&mut stream).await).contains("login.live.com"));
    let mut stream = h.request(&start_url("s"), "ms-xal-0000000048183522://other").await;
    assert!(error_text(reply(&mut stream).await).contains("redirect"));
    assert!(h.pages().is_empty(), "nothing was shown");
    let refusals: Vec<String> = h.log().into_iter().filter(|l| l.contains("refused")).collect();
    assert_eq!(refusals.len(), 3, "once per reason: {refusals:?}");
    let warnings = h
        .reporter
        .captured()
        .into_iter()
        .filter(|e| matches!(e, Event::Status { text } if text.contains("was refused")))
        .count();
    assert_eq!(warnings, 3, "the user hears of each reason once");
    // The totals are logged when the session ends.
    let Harness { serving, log, .. } = h;
    serving.unwrap().shutdown().await;
    assert!(log.lock().unwrap().iter().any(|l| l.contains("5 request(s) refused") && l.contains("(3×)")));
}

#[tokio::test]
async fn broken_frames_and_silent_connections_get_nothing_shown() {
    let options = Options { read_timeout: Duration::from_millis(200), ..Options::default() };
    let h = Harness::start("frames", options);
    // An announced length beyond the limit: refused before anything is read or allocated.
    let mut stream = UnixStream::connect(h.dir.socket()).await.unwrap();
    stream.write_all(&(1u32 << 30).to_be_bytes()).await.unwrap();
    assert!(error_text(reply(&mut stream).await).contains("16 KiB"));
    // Not the request's shape.
    let mut stream = UnixStream::connect(h.dir.socket()).await.unwrap();
    stream.write_all(&wire::frame(br#"{"start":"a","end":"b","more":1}"#).unwrap()).await.unwrap();
    assert!(error_text(reply(&mut stream).await).contains("start"));
    // Nothing at all: given up on after the read timeout.
    let mut stream = UnixStream::connect(h.dir.socket()).await.unwrap();
    assert!(error_text(reply(&mut stream).await).contains("in time"));
    assert!(h.pages().is_empty());
}

#[tokio::test]
async fn one_prompt_at_a_time_and_a_bounded_number_per_session() {
    let options = Options { max_requests: 2, cooldown: Duration::ZERO, ..Options::default() };
    let h = Harness::start("bounds", options);
    let mut first = h.request(&start_url("one"), REDIRECT_URI).await;
    h.page(1).await;
    let mut second = h.request(&start_url("two"), REDIRECT_URI).await;
    assert!(error_text(reply(&mut second).await).contains("already waiting"));
    h.answer(Answer::Cancel);
    assert!(error_text(reply(&mut first).await).contains("cancelled outside the game"));
    assert_eq!(h.page(2).await.state, SignInState::Cancelled);
    let mut third = h.request(&start_url("three"), REDIRECT_URI).await;
    assert_eq!(h.page(3).await.state, SignInState::Waiting);
    h.answer(Answer::Text(final_url("three")));
    assert!(matches!(reply(&mut third).await, Reply::Url(_)));
    let mut fourth = h.request(&start_url("four"), REDIRECT_URI).await;
    assert!(error_text(reply(&mut fourth).await).contains("most sign-ins"));
}

#[tokio::test]
async fn a_cancelled_prompt_cannot_be_reopened_at_once() {
    let options = Options { cooldown: Duration::from_millis(400), ..Options::default() };
    let h = Harness::start("cooldown", options);
    let mut first = h.request(&start_url("a"), REDIRECT_URI).await;
    h.page(1).await;
    h.answer(Answer::Cancel);
    error_text(reply(&mut first).await);
    let mut again = h.request(&start_url("b"), REDIRECT_URI).await;
    assert!(error_text(reply(&mut again).await).contains("a moment ago"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _later = h.request(&start_url("c"), REDIRECT_URI).await;
    assert_eq!(h.page(3).await.state, SignInState::Waiting);
}

#[tokio::test]
async fn a_prompt_nobody_answers_expires_and_the_game_is_told() {
    let options = Options { ttl: Duration::from_millis(300), ..Options::default() };
    let h = Harness::start("expire", options);
    let mut stream = h.request(&start_url("s"), REDIRECT_URI).await;
    h.page(1).await;
    assert!(error_text(reply(&mut stream).await).contains("no answer in time"));
    assert_eq!(h.page(2).await.state, SignInState::Expired);
    // A late answer reaches nothing.
    h.answer(Answer::Text(final_url("s")));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !h
        .reporter
        .captured()
        .iter()
        .any(|e| matches!(e, Event::Status { text } if text.contains("no Bedrock sign-in is waiting")))
    {
        assert!(Instant::now() < deadline, "{:?}", h.reporter.captured());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn the_prompt_is_withdrawn_when_the_game_stops_waiting() {
    let h = Harness::start("gone", Options::default());
    let stream = h.request(&start_url("s"), REDIRECT_URI).await;
    h.page(1).await;
    drop(stream);
    let ended = h.page(2).await;
    assert_eq!(ended.state, SignInState::Cancelled);
    assert!(ended.note.unwrap().contains("stopped waiting"));
}

#[tokio::test]
async fn wrong_answers_say_why_and_too_many_end_the_prompt() {
    let options = Options { max_answers: 3, ..Options::default() };
    let h = Harness::start("wrong", options);
    let mut stream = h.request(&start_url("right"), REDIRECT_URI).await;
    h.page(1).await;
    h.answer(Answer::Text(final_url("wrong")));
    let retry = h.page(2).await;
    assert_eq!((retry.state, retry.attempts_left), (SignInState::Waiting, 2));
    assert!(retry.note.unwrap().contains("earlier sign-in"));
    h.answer(Answer::Text("https://login.live.com/".into()));
    let retry = h.page(3).await;
    assert_eq!(retry.attempts_left, 1);
    assert!(retry.note.unwrap().contains("does not start with"));
    h.answer(Answer::Text(format!("{REDIRECT_URI}?state=right")));
    assert!(error_text(reply(&mut stream).await).contains("too many answers"));
    let failed = h.page(4).await;
    assert_eq!(failed.state, SignInState::Failed);
}

#[tokio::test]
async fn ending_the_launch_withdraws_a_waiting_prompt() {
    let mut h = Harness::start("shutdown", Options::default());
    let mut stream = h.request(&start_url("s"), REDIRECT_URI).await;
    h.page(1).await;
    h.serving.take().unwrap().shutdown().await;
    assert!(error_text(reply(&mut stream).await).contains("the launch is ending"));
    assert_eq!(h.page(2).await.state, SignInState::Cancelled);
    // The answers of this kind are no longer taken by anyone.
    assert_eq!(h.reporter.answer_for_test(AnswerKind::SignIn, Answer::Cancel), Delivery::NobodyAsks);
}

#[tokio::test]
async fn a_flood_of_connections_is_paced_and_the_prompt_still_works() {
    let h = Harness::start("flood", Options { read_timeout: Duration::from_millis(100), ..Options::default() });
    let mut stream = h.request(&start_url("s"), REDIRECT_URI).await;
    h.page(1).await;
    // Forty connections that send nothing: they are taken at the paced rate, and the prompt
    // keeps taking answers meanwhile.
    let mut idle = Vec::new();
    for _ in 0..40 {
        idle.push(UnixStream::connect(h.dir.socket()).await.unwrap());
    }
    let begun = Instant::now();
    h.answer(Answer::Text(final_url("s")));
    assert!(matches!(reply(&mut stream).await, Reply::Url(_)));
    assert!(begun.elapsed() < Duration::from_secs(2), "the answer waited for the flood: {:?}", begun.elapsed());
}

/// The stub's code, in this process, against the server: the whole round trip as the client
/// sees it (exit status, one line on standard output, nothing on standard error).
#[tokio::test]
async fn the_stub_prints_the_address_and_exits_zero_or_prints_nothing_and_exits_one() {
    let h = Harness::start("stub", Options::default());
    let socket = h.dir.socket();
    let run_stub = |start: String, socket: PathBuf| {
        tokio::task::spawn_blocking(move || {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let args = [start.into(), std::ffi::OsString::from(REDIRECT_URI)];
            let code = crate::bedrock::signin::stub::run(&args, &socket, Duration::from_secs(20), &mut out, &mut err);
            (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
        })
    };
    let client = run_stub(start_url("st"), socket.clone());
    h.page(1).await;
    h.answer(Answer::Text(final_url("st")));
    let (code, out, err) = client.await.unwrap();
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, format!("{REDIRECT_URI}?code={CODE}&state=st\n"));

    let client = run_stub(start_url("st2"), socket.clone());
    h.page(3).await;
    h.answer(Answer::Cancel);
    let (code, out, err) = client.await.unwrap();
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.starts_with("mcpelauncher-webview (bastion-mc): the sign-in was not completed"), "{err}");

    let refused = run_stub(start_url("x").replace("response_type=code", "response_type=token"), socket.clone());
    let (code, out, err) = refused.await.unwrap();
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.contains("refused by bastion-mc") && err.contains("response_type"), "{err}");
}

#[test]
fn a_socket_directory_path_stays_short() {
    // The sockets above live in the temporary directory; a long one would not bind.
    assert!(Dir::new("len").socket().as_os_str().len() < 100, "{}", Path::new(&std::env::temp_dir()).display());
}

// ---------------------------------------------------------------- the desktop's handler

/// A runtime directory of its own for the handler's socket (short, private).
fn handback_runtime(tag: &str) -> Dir {
    let dir = Dir::new(&format!("hb{tag}"));
    std::fs::set_permissions(&dir.0, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    dir
}

/// The server with the desktop's handler taken in `runtime`.
fn with_handback(tag: &str, runtime: &Path) -> Harness {
    use super::super::handback::{Taken, take};
    let dir = Dir::new(tag);
    let listener = StdUnixListener::bind(dir.socket()).unwrap();
    let Taken::Bound(handback) = take(Some(runtime)).unwrap() else { panic!("the handler is free") };
    let reporter = Reporter::capture();
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    let logger: Log = Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_string()));
    let serving = super::serve_with_handback(listener, Some(handback), &reporter, logger, Options::default()).unwrap();
    Harness { dir, reporter, log, serving: Some(serving) }
}

/// `bastion-mc bedrock hand-back <address>`, as the browser runs it (in-process here).
async fn hand_back(runtime: &Path, address: String) -> super::super::handback::Outcome {
    let runtime = runtime.to_path_buf();
    tokio::task::spawn_blocking(move || super::super::handback::hand_back(Some(&runtime), &address)).await.unwrap()
}

/// What the browser hands back is checked as a pasted answer is: an address of another sign-in
/// is refused — the prompt stays and keeps its answers (a web page can make the browser call the
/// handler) — and this sign-in's goes to the game, rebuilt.
#[tokio::test]
async fn an_address_the_browser_hands_back_is_checked_like_a_pasted_one_and_handed_to_the_game() {
    use super::super::handback::Outcome;
    let runtime = handback_runtime("ok");
    let h = with_handback("hbok", &runtime.0);
    // Nothing waits yet.
    let early = hand_back(&runtime.0, final_url("st4te")).await;
    assert!(matches!(&early, Outcome::Refused(why) if why.contains("no Bedrock sign-in is waiting")), "{early:?}");
    let mut stream = h.request(&start_url("st4te"), REDIRECT_URI).await;
    let page = h.page(1).await;
    assert!(page.handback && page.state == SignInState::Waiting, "the prompt says the handler is this session's");
    // Another sign-in's address: refused with the reason, the prompt stays, its answers are kept.
    let other = hand_back(&runtime.0, final_url("another")).await;
    assert!(matches!(&other, Outcome::Refused(why) if why.contains("state")), "{other:?}");
    let still = h.page(2).await;
    assert_eq!(still.state, SignInState::Waiting);
    assert_eq!(still.attempts_left, 10, "a refused hand-back does not use up an answer");
    assert!(still.note.as_deref().is_some_and(|n| n.contains("your browser handed back")), "{still:?}");
    // This sign-in's: handed to the game, rebuilt from code and state.
    assert_eq!(hand_back(&runtime.0, final_url("st4te")).await, Outcome::Accepted);
    assert_eq!(reply(&mut stream).await, Reply::Url(format!("{REDIRECT_URI}?code={CODE}&state=st4te")));
    assert_eq!(h.page(3).await.state, SignInState::Done);
    let log = h.log().join("\n");
    assert!(log.contains("came from the desktop's handler and was handed to the game"), "{log}");
    assert!(!log.contains(CODE) && !format!("{:?}", h.reporter.captured()).contains(CODE), "the code is never shown");
    let mut h = h;
    assert!(h.serving.take().unwrap().shutdown().await, "a sign-in was handed to the game");
    // The session is over: its handler is gone with it.
    let after = hand_back(&runtime.0, final_url("st4te")).await;
    assert!(matches!(&after, Outcome::NoSession(_)), "{after:?}");
}

/// A server without the handler says so in its prompts; one that holds it releases it at its end.
#[tokio::test]
async fn a_prompt_says_whether_the_browser_hands_the_result_back() {
    let h = Harness::start("nohb", Options::default());
    let _stream = h.request(&start_url("s"), REDIRECT_URI).await;
    assert!(!h.page(1).await.handback, "no handler in this session: the prompt asks for a paste");
}
