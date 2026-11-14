//! How the launcher reports what it is doing — to a person, or to a program.
//!
//! Core code never prints. It hands a [`Reporter`] status notes, progress, sign-in prompts
//! and game output, and the reporter decides what that becomes:
//!
//! * **human** (the default): the text the command line has always printed;
//! * **events** (`bastion-mc --events …`): one JSON [`Event`] per line on standard output,
//!   for a front end running as a separate process (see the `bastion-mc-proto` crate).
//!
//! The event stream is credential-free, and nearly one-way: a front end can ask for the
//! current command to stop (see [`Reporter::stop_requested`]), and answer the prompts the
//! launcher asked it to show — a Bedrock sign-in's final address, a Realms approval without a
//! phone — which is *trusted input*, from the user and never from the game ([`input`]; on a
//! terminal launch the same answers are typed on the terminal).
//!
//! A slow or dead reader must never slow the game down, so in event mode nothing here
//! blocks: lines go through a bounded queue drained by a writer thread, and when the queue
//! is full game output is dropped (and counted) rather than waited for. The log file on
//! disk is unaffected.

pub mod input;

use std::{
    collections::VecDeque,
    io::Write,
    os::fd::AsFd,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub use bastion_mc_proto::{
    Account, Answer, AnswerKind, ApprovalState, EVENTS_FLAG, Event, Instance, PROTOCOL, Phase, Protection,
    RealmsApproval, RealmsMode, STOP_COMMAND, Shield, SignInPage, SignInState, Source, Unit, Version,
};

pub use self::input::{AnswerReceiver, Delivery, PendingHandle};

/// Game output beyond this many queued lines is dropped until the reader catches up.
const MAX_QUEUED_LINES: usize = 4096;
/// Longest text carried in one event; anything longer is cut (lines stay far below the
/// protocol's line limit even after JSON escaping).
const MAX_TEXT_CHARS: usize = 6000;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub struct Reporter {
    inner: Arc<Inner>,
}

struct Inner {
    mode: Mode,
    /// Who takes answers to prompts in this command (trusted input, see [`input`]).
    answers: Arc<input::Registry>,
    /// The terminal reader of a human-mode command has been started.
    terminal_reading: AtomicBool,
}

impl Inner {
    fn new(mode: Mode) -> Self {
        Inner { mode, answers: input::Registry::new(), terminal_reading: AtomicBool::new(false) }
    }
}

enum Mode {
    Human {
        quiet: bool,
    },
    Events(Queue),
    /// Keeps events in memory; for tests.
    Capture(Mutex<Vec<Event>>),
}

struct Queue {
    state: Mutex<QueueState>,
    /// Signalled when lines are queued or the queue is closed.
    wake: Condvar,
    /// Signalled when the writer has nothing left to do.
    idle: Condvar,
}

#[derive(Default)]
struct QueueState {
    lines: VecDeque<String>,
    dropped: u64,
    writing: bool,
    closed: bool,
    /// Standard output is gone (the front end exited); stop producing.
    broken: bool,
}

fn clip(text: &str) -> String {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

fn human_amount(done: u64, total: u64, unit: Unit) -> String {
    match unit {
        Unit::Files => format!("{done}/{total} files"),
        Unit::Bytes => format!("{:.1}/{:.1} MiB", done as f64 / 1048576.0, total as f64 / 1048576.0),
    }
}

impl Reporter {
    /// Plain text for a terminal. `quiet` suppresses everything but errors.
    pub fn human(quiet: bool) -> Self {
        Reporter { inner: Arc::new(Inner::new(Mode::Human { quiet })) }
    }

    /// JSON events on standard output. Starts the writer thread and sends [`Event::Hello`].
    pub fn events() -> Self {
        let reporter = Reporter {
            inner: Arc::new(Inner::new(Mode::Events(Queue {
                state: Mutex::new(QueueState::default()),
                wake: Condvar::new(),
                idle: Condvar::new(),
            }))),
        };
        let for_writer = reporter.clone();
        // A thread rather than a task: it must keep draining while the async runtime is
        // busy or already shut down, and it is the only place that may block on the pipe.
        let _ = std::thread::Builder::new().name("event-writer".into()).spawn(move || for_writer.write_loop());
        reporter.emit(Event::Hello { protocol: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string() });
        reporter
    }

    #[doc(hidden)]
    pub fn capture() -> Self {
        Reporter { inner: Arc::new(Inner::new(Mode::Capture(Mutex::new(Vec::new())))) }
    }

    #[doc(hidden)]
    pub fn captured(&self) -> Vec<Event> {
        match &self.inner.mode {
            Mode::Capture(events) => events.lock().expect("capture lock").clone(),
            _ => Vec::new(),
        }
    }

    pub fn is_events(&self) -> bool {
        matches!(self.inner.mode, Mode::Events(_))
    }

    fn write_loop(&self) {
        let Mode::Events(queue) = &self.inner.mode else { return };
        loop {
            let batch: Vec<String> = {
                let mut state = queue.state.lock().expect("event queue lock");
                loop {
                    if !state.lines.is_empty() {
                        break;
                    }
                    state.writing = false;
                    queue.idle.notify_all();
                    if state.closed {
                        return;
                    }
                    state = queue.wake.wait(state).expect("event queue lock");
                }
                state.writing = true;
                state.lines.drain(..).collect()
            };
            let mut out = std::io::stdout().lock();
            let written = batch.iter().try_for_each(|line| out.write_all(line.as_bytes())).and_then(|_| out.flush());
            if written.is_err() {
                let mut state = queue.state.lock().expect("event queue lock");
                state.broken = true;
                state.lines.clear();
            }
        }
    }

    fn push(&self, event: Event, droppable: bool) {
        match &self.inner.mode {
            Mode::Human { .. } => {}
            Mode::Capture(events) => events.lock().expect("capture lock").push(event),
            Mode::Events(queue) => {
                let mut state = queue.state.lock().expect("event queue lock");
                if state.broken || state.closed {
                    return;
                }
                if droppable && state.lines.len() >= MAX_QUEUED_LINES {
                    state.dropped += 1;
                    return;
                }
                if state.dropped > 0 && state.lines.len() < MAX_QUEUED_LINES / 2 {
                    let note = Event::Status {
                        text: format!(
                            "{} line(s) of output were not forwarded because the reader fell behind (the log file has them)",
                            state.dropped
                        ),
                    };
                    state.dropped = 0;
                    state.lines.push_back(note.to_line());
                }
                state.lines.push_back(event.to_line());
                queue.wake.notify_one();
            }
        }
    }

    /// Send a structured event. In human mode the events that need the user's attention are
    /// printed as text; the rest have no terminal representation.
    pub fn emit(&self, event: Event) {
        if let Mode::Human { quiet } = &self.inner.mode {
            match &event {
                Event::DeviceCode { user_code, verification_uri, .. } => {
                    println!("To sign in, open {verification_uri} in any browser and enter the code {user_code}");
                }
                Event::BrowserLogin { url, timeout_secs } => {
                    println!(
                        "Open this URL in your browser to sign in (waiting up to {} minutes):\n\n  {url}\n",
                        timeout_secs / 60
                    );
                }
                // A Realms join waiting for approval is shown whatever else is silenced: it is
                // the one place the code appears.
                Event::RealmsApproval(prompt) => {
                    eprintln!("{}", realms_text(prompt, self.inner.answers.is_open(AnswerKind::Realms)))
                }
                // So is a sign-in page the game asked for: nothing else shows it.
                Event::SignInPage(page) => eprintln!("{}", sign_in_text(page)),
                Event::Status { text } if !quiet => eprintln!("{text}"),
                Event::Log { source, line } if !quiet => println!("[{}] {line}", source.tag()),
                _ => {}
            }
            return;
        }
        let event = match event {
            Event::Status { text } => Event::Status { text: clip(&text) },
            Event::Log { source, line } => Event::Log { source, line: clip(&line) },
            Event::Error { message, code } => Event::Error { message: clip(&message), code },
            Event::SignInPage(page) => Event::SignInPage(SignInPage { note: page.note.map(|n| clip(&n)), ..page }),
            other => other,
        };
        let droppable = matches!(event, Event::Log { .. } | Event::Progress { .. });
        self.push(event, droppable);
    }

    /// A progress note from the launcher itself.
    pub fn status(&self, text: impl AsRef<str>) {
        self.emit(Event::Status { text: text.as_ref().to_string() });
    }

    /// A note the user must not miss: like [`Reporter::status`], but on a terminal it is
    /// printed even when everything else is silenced (`launch --quiet`).
    pub fn warn(&self, text: impl AsRef<str>) {
        match &self.inner.mode {
            Mode::Human { quiet: true } => eprintln!("{}", text.as_ref()),
            _ => self.status(text),
        }
    }

    pub fn phase(&self, phase: Phase) {
        self.emit(Event::Phase { phase });
    }

    /// One line of output that is also in the log file. `line` must already be sanitised if
    /// it came from the sandbox.
    pub fn log(&self, source: Source, line: &str) {
        self.emit(Event::Log { source, line: line.to_string() });
    }

    /// Start counting one batch of measurable work.
    pub fn progress(&self, label: &str, total: u64, unit: Unit) -> Arc<Progress> {
        Arc::new(Progress {
            reporter: self.clone(),
            label: label.to_string(),
            unit,
            total,
            done: AtomicU64::new(0),
            last: Mutex::new(None),
        })
    }

    /// Flush what is queued (bounded wait) and stop accepting events. Call once, last.
    pub fn finish(&self) {
        let Mode::Events(queue) = &self.inner.mode else { return };
        let mut state = queue.state.lock().expect("event queue lock");
        state.closed = true;
        queue.wake.notify_all();
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        while (state.writing || !state.lines.is_empty()) && !state.broken {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
            state = queue.idle.wait_timeout(state, left).expect("event queue lock").0;
        }
    }

    /// Resolves when the front end asks the current command to stop: a [`STOP_COMMAND`] line
    /// on standard input, or standard input reaching end-of-file (the front end is gone).
    /// Meanwhile `answer <kind> <text>` lines on the same input go to whoever takes answers of
    /// that kind ([`Reporter::answers`]); every other line is ignored, as it always was.
    ///
    /// Only in event mode and only when standard input is a pipe; otherwise this never
    /// resolves and Ctrl-C / SIGTERM remain the way to stop.
    pub async fn stop_requested(&self) {
        if !self.is_events() {
            return std::future::pending().await;
        }
        // A duplicate, so that dropping the receiver does not close descriptor 0.
        let pipe =
            std::io::stdin().as_fd().try_clone_to_owned().and_then(tokio::net::unix::pipe::Receiver::from_owned_fd);
        let Ok(pipe) = pipe else { return std::future::pending().await };
        let note = |text: String| self.status(text);
        input::control_loop(&self.inner.answers, pipe, &note).await;
    }

    /// Take the answers to prompts of `kind` from the trusted input — the front end's control
    /// pipe, or the terminal of a terminal launch — for as long as the receiver lives. At most
    /// one taker per kind; a second is refused.
    ///
    /// Event mode: the front end is told once ([`Event::Answerable`]). Terminal: a reader of
    /// the terminal starts (once per command) and routes typed lines to the prompt that waits.
    pub fn answers(&self, kind: AnswerKind) -> crate::Result<AnswerReceiver> {
        let receiver = self.inner.answers.subscribe(kind).ok_or_else(|| {
            crate::Error::Other(format!("answers of the kind `{}` are already taken in this command", kind.as_str()))
        })?;
        match &self.inner.mode {
            Mode::Events(_) | Mode::Capture(_) => self.emit(Event::Answerable { kind }),
            Mode::Human { .. } => {
                if !self.inner.terminal_reading.swap(true, Ordering::SeqCst) {
                    match std::io::stdin().as_fd().try_clone_to_owned() {
                        Ok(fd) => input::spawn_terminal(
                            fd,
                            Arc::downgrade(&self.inner.answers),
                            Arc::new(|line: &str| eprintln!("{line}")),
                        ),
                        Err(e) => eprintln!("bastion-mc: prompts cannot be answered on this terminal: {e}"),
                    }
                }
            }
        }
        Ok(receiver)
    }

    /// Test support: hand an answer over as if the user had given it. Only a capturing
    /// reporter (tests) takes it — the launcher's own reporters accept answers from the
    /// control pipe and the terminal alone, so nothing else in the program can answer a
    /// prompt through this.
    #[doc(hidden)]
    pub fn answer_for_test(&self, kind: AnswerKind, answer: Answer) -> Delivery {
        match &self.inner.mode {
            Mode::Capture(_) => self.inner.answers.deliver(kind, answer),
            _ => Delivery::NobodyAsks,
        }
    }
}

/// How a Realms approval prompt reads on a terminal. `here`: such prompts are answered on
/// this terminal (a session without the phone), not in the game.
fn realms_text(prompt: &RealmsApproval, here: bool) -> String {
    match prompt.state {
        ApprovalState::Waiting => {
            let what = match (&prompt.code, here) {
                (Some(code), false) if code.len() == 6 => {
                    format!("type this code into the phone's Realms app:   {} {}", &code[..3], &code[3..])
                }
                (Some(code), false) => format!("type this code into the phone's Realms app:   {code}"),
                (None, false) => "type the code your authenticator app shows into the phone's Realms app".to_string(),
                (Some(code), true) => format!(
                    "type this code here and press Enter (`cancel` refuses):   {code}\n                    (if a sign-in waits too, type `realms {code}`)"
                ),
                (None, true) => "type the code your authenticator app shows here and press Enter (`cancel` refuses)\n                    (if a sign-in waits too, start the line with `realms `)".to_string(),
            };
            format!(
                "\n== Realms: a join waits for your approval ==\n   Realm:   {}\n   Server:  {}\n   To join, {what}\n                    ({} s, {} attempt(s); if you did not ask to join this Realm, do nothing and it is refused)\n",
                prompt.world, prompt.address, prompt.expires_in, prompt.attempts_left
            )
        }
        ApprovalState::Approved => format!("Realms: the join to {} ({}) was approved", prompt.world, prompt.address),
        ApprovalState::Refused => format!("Realms: the join to {} was refused", prompt.world),
        ApprovalState::Expired => format!("Realms: the join to {} was not approved in time", prompt.world),
    }
}

/// How a sign-in prompt reads on a terminal. The address is on a line of its own, so that it
/// can be selected and copied whole.
fn sign_in_text(page: &SignInPage) -> String {
    let note = page.note.as_deref().map(|n| format!(" ({n})")).unwrap_or_default();
    match page.state {
        SignInState::Waiting => format!(
            "\n== Bedrock: the game asks you to sign in to your Microsoft account ==\n\
             {retry}   Open this page in your web browser (it is Microsoft's own sign-in page, on login.live.com):\n\n\
             {url}\n\n   \
             The sign-in ends on an address your browser cannot open and does not show ({end}?code=…).\n   \
             Before the last step, open the browser's developer tools (F12) on the Network tab; afterwards select\n   \
             the last request (it failed, status 302), copy the Location header of its response, paste it here\n   \
             and press Enter. `cancel` refuses the sign-in. ({mins} min, {left} attempt(s); only that address goes\n   \
             to the game, never your password; if a Realms join waits too, start the line with `signin `)\n",
            retry = match &page.note {
                Some(n) => format!("   That was not accepted: {n}.\n"),
                None => String::new(),
            },
            url = page.url,
            end = page.end,
            mins = page.expires_in.div_ceil(60),
            left = page.attempts_left,
        ),
        SignInState::Done => "Bedrock sign-in: the address was handed to the game".to_string(),
        SignInState::Cancelled => format!("Bedrock sign-in: cancelled{note}"),
        SignInState::Expired => "Bedrock sign-in: not answered in time; the game was told it failed".to_string(),
        SignInState::Failed => format!("Bedrock sign-in: refused{note}"),
    }
}

/// "Please stop", shared between whoever can ask (a front end, through
/// [`Reporter::stop_requested`]) and the code that knows how to stop cleanly.
///
/// Until a command declares that it is supervising something ([`StopSignal::supervise`]),
/// a stop cancels it from outside: its future is dropped, which is all the cleanup a
/// download or a pending sign-in needs (temporary files and scratch directories are removed
/// by their owners' destructors). From then on the command is expected to watch
/// [`StopSignal::wait`] itself and shut down what it started.
#[derive(Clone, Default)]
pub struct StopSignal {
    inner: Arc<StopInner>,
}

#[derive(Default)]
struct StopInner {
    requested: AtomicBool,
    supervised: AtomicBool,
    notify: tokio::sync::Notify,
}

impl StopSignal {
    /// A signal nobody will ever raise.
    pub fn never() -> Self {
        Self::default()
    }

    pub fn trigger(&self) {
        self.inner.requested.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn is_requested(&self) -> bool {
        self.inner.requested.load(Ordering::SeqCst)
    }

    /// From now on the caller handles stop requests (by awaiting [`wait`](Self::wait)).
    pub fn supervise(&self) {
        self.inner.supervised.store(true, Ordering::SeqCst);
    }

    pub fn is_supervised(&self) -> bool {
        self.inner.supervised.load(Ordering::SeqCst)
    }

    /// Resolves when a stop is requested while nothing is being supervised; never resolves
    /// otherwise. This is what cancels a command from outside.
    pub async fn wait_unsupervised(&self) {
        self.wait().await;
        if self.is_supervised() {
            std::future::pending::<()>().await;
        }
    }

    /// Resolves once a stop has been requested (immediately if it already was).
    pub async fn wait(&self) {
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            // Register before looking at the flag, so a trigger in between is not missed.
            notified.as_mut().enable();
            if self.is_requested() {
                return;
            }
            notified.await;
        }
    }
}

/// A counter for one batch of work, shared by the workers doing it. Reports are rate-limited
/// so that thousands of small files do not become thousands of events.
pub struct Progress {
    reporter: Reporter,
    label: String,
    unit: Unit,
    total: u64,
    done: AtomicU64,
    last: Mutex<Option<Instant>>,
}

impl Progress {
    pub fn unit(&self) -> Unit {
        self.unit
    }

    pub fn add(&self, amount: u64) {
        let done = self.done.fetch_add(amount, Ordering::Relaxed) + amount;
        self.report(done.min(self.total), false);
    }

    /// Take back work that turned out not to count (a failed attempt that will be retried).
    pub fn retract(&self, amount: u64) {
        let mut current = self.done.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_sub(amount);
            match self.done.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(seen) => current = seen,
            }
        }
    }

    /// Report completion. Sent exactly once per batch, whatever was counted before.
    pub fn complete(&self) {
        self.done.store(self.total, Ordering::Relaxed);
        self.report(self.total, true);
    }

    fn report(&self, done: u64, last: bool) {
        let human = matches!(self.reporter.inner.mode, Mode::Human { .. });
        if !last {
            // Completion is announced by `complete` only, so a reader sees it exactly once.
            if done >= self.total {
                return;
            }
            let gap = if human { Duration::from_secs(2) } else { Duration::from_millis(100) };
            let mut stamp = self.last.lock().expect("progress lock");
            let now = Instant::now();
            // The first report is delayed by one interval too: quick batches stay silent.
            let since = *stamp.get_or_insert(now);
            if now.duration_since(since) < gap {
                return;
            }
            *stamp = Some(now);
        }
        if human {
            if let Mode::Human { quiet: false } = self.reporter.inner.mode
                && !last
            {
                eprintln!("  … {}: {}", self.label, human_amount(done, self.total, self.unit));
            }
            return;
        }
        self.reporter.emit(Event::Progress { label: self.label.clone(), done, total: self.total, unit: self.unit });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_rate_limited_and_completes_exactly_once() {
        let reporter = Reporter::capture();
        let progress = reporter.progress("assets", 1000, Unit::Bytes);
        for _ in 0..100 {
            progress.add(1);
        }
        assert!(reporter.captured().is_empty(), "a burst inside one interval is not reported");
        std::thread::sleep(Duration::from_millis(120));
        progress.add(100);
        progress.add(800); // reaches the total: left to `complete`
        progress.add(50); // over-count must not exceed the total either
        progress.complete();
        assert_eq!(
            reporter.captured(),
            vec![
                Event::Progress { label: "assets".into(), done: 200, total: 1000, unit: Unit::Bytes },
                Event::Progress { label: "assets".into(), done: 1000, total: 1000, unit: Unit::Bytes },
            ]
        );
    }

    #[test]
    fn retracting_a_failed_attempt() {
        let reporter = Reporter::capture();
        let progress = reporter.progress("game files", 10, Unit::Files);
        progress.add(4);
        progress.retract(3);
        progress.retract(100); // never underflows
        std::thread::sleep(Duration::from_millis(120));
        progress.add(2);
        assert_eq!(
            reporter.captured(),
            vec![Event::Progress { label: "game files".into(), done: 2, total: 10, unit: Unit::Files }]
        );
    }

    #[test]
    fn long_text_is_clipped_and_stays_one_line() {
        let reporter = Reporter::capture();
        reporter.status("x".repeat(MAX_TEXT_CHARS * 3));
        reporter.log(Source::Game, &"é".repeat(MAX_TEXT_CHARS + 5));
        for event in reporter.captured() {
            let line = event.to_line();
            assert!(line.len() < bastion_mc_proto::MAX_LINE_BYTES, "{} bytes", line.len());
            assert_eq!(Event::parse_line(&line), Some(event));
        }
    }

    #[test]
    fn the_terminal_shows_the_sign_in_page_on_a_line_of_its_own_and_how_to_answer() {
        let page = SignInPage {
            url: "https://login.live.com/oauth20_authorize.srf?client_id=0000000048183522&state=s".into(),
            end: "ms-xal-0000000048183522://auth".into(),
            expires_in: 899,
            attempts_left: 10,
            state: SignInState::Waiting,
            note: None,
        };
        let text = sign_in_text(&page);
        assert!(text.lines().any(|l| l == page.url), "the address alone on its line, to be copied whole: {text}");
        assert!(text.contains("Network tab") && text.contains("Location header") && text.contains("`cancel`"));
        assert!(text.contains("(15 min, 10 attempt(s);"), "{text}");
        let retry = sign_in_text(&SignInPage { note: Some("it holds no code".into()), ..page.clone() });
        assert!(retry.contains("That was not accepted: it holds no code."), "{retry}");
        let ended =
            SignInPage { state: SignInState::Cancelled, note: Some("cancelled outside the game".into()), ..page };
        assert_eq!(sign_in_text(&ended), "Bedrock sign-in: cancelled (cancelled outside the game)");
    }

    #[test]
    fn a_realms_prompt_answered_here_says_to_type_here_not_into_the_phone() {
        let prompt = RealmsApproval {
            world: "Our Realm".into(),
            address: "203.0.113.9:19132".into(),
            mode: RealmsMode::Code,
            code: Some("123456".into()),
            expires_in: 120,
            attempts_left: 3,
            state: ApprovalState::Waiting,
        };
        assert!(realms_text(&prompt, false).contains("into the phone's Realms app:   123 456"));
        let here = realms_text(&prompt, true);
        assert!(here.contains("type this code here and press Enter") && here.contains("`realms 123456`"), "{here}");
        assert!(!here.contains("phone"), "{here}");
        let totp = realms_text(&RealmsApproval { mode: RealmsMode::Totp, code: None, ..prompt }, true);
        assert!(totp.contains("authenticator app shows here"), "{totp}");
    }

    #[test]
    fn human_amounts() {
        assert_eq!(human_amount(3, 10, Unit::Files), "3/10 files");
        assert_eq!(human_amount(1 << 20, 3 << 19, Unit::Bytes), "1.0/1.5 MiB");
    }

    #[tokio::test]
    async fn stop_signal_wakes_waiters_and_stays_set() {
        let stop = StopSignal::never();
        assert!(tokio::time::timeout(Duration::from_millis(30), stop.wait()).await.is_err());
        let waiter = tokio::spawn({
            let stop = stop.clone();
            async move { stop.wait().await }
        });
        tokio::task::yield_now().await;
        assert!(!stop.is_supervised());
        stop.supervise();
        stop.trigger();
        tokio::time::timeout(Duration::from_secs(5), waiter).await.unwrap().unwrap();
        assert!(stop.is_requested() && stop.is_supervised());
        // Late waiters return at once.
        tokio::time::timeout(Duration::from_secs(5), stop.wait()).await.unwrap();
        // ...but once something is supervised, nobody cancels it from outside.
        assert!(tokio::time::timeout(Duration::from_millis(30), stop.wait_unsupervised()).await.is_err());

        let early = StopSignal::never();
        early.trigger();
        tokio::time::timeout(Duration::from_secs(5), early.wait_unsupervised()).await.unwrap();
    }

    #[tokio::test]
    async fn stop_is_never_requested_outside_event_mode() {
        let reporter = Reporter::human(true);
        let waited = tokio::time::timeout(Duration::from_millis(50), reporter.stop_requested()).await;
        assert!(waited.is_err());
    }
}
