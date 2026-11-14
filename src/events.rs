//! How the launcher reports what it is doing — to a person, or to a program.
//!
//! Core code never prints. It hands a [`Reporter`] status notes, progress, sign-in prompts
//! and game output, and the reporter decides what that becomes:
//!
//! * **human** (the default): the text the command line has always printed;
//! * **events** (`bastion-mc --events …`): one JSON [`Event`] per line on standard output,
//!   for a front end running as a separate process (see the `bastion-mc-proto` crate).
//!
//! The event stream is deliberately one-way and credential-free. A front end can only ask
//! for the current command to stop (see [`Reporter::stop_requested`]).
//!
//! A slow or dead reader must never slow the game down, so in event mode nothing here
//! blocks: lines go through a bounded queue drained by a writer thread, and when the queue
//! is full game output is dropped (and counted) rather than waited for. The log file on
//! disk is unaffected.

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
    Account, EVENTS_FLAG, Event, Instance, PROTOCOL, Phase, Protection, STOP_COMMAND, Shield, Source, Unit, Version,
};

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
        Reporter { inner: Arc::new(Inner { mode: Mode::Human { quiet } }) }
    }

    /// JSON events on standard output. Starts the writer thread and sends [`Event::Hello`].
    pub fn events() -> Self {
        let reporter = Reporter {
            inner: Arc::new(Inner {
                mode: Mode::Events(Queue {
                    state: Mutex::new(QueueState::default()),
                    wake: Condvar::new(),
                    idle: Condvar::new(),
                }),
            }),
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
        Reporter { inner: Arc::new(Inner { mode: Mode::Capture(Mutex::new(Vec::new())) }) }
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
            other => other,
        };
        let droppable = matches!(event, Event::Log { .. } | Event::Progress { .. });
        self.push(event, droppable);
    }

    /// A progress note from the launcher itself.
    pub fn status(&self, text: impl AsRef<str>) {
        self.emit(Event::Status { text: text.as_ref().to_string() });
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
    ///
    /// Only in event mode and only when standard input is a pipe; otherwise this never
    /// resolves and Ctrl-C / SIGTERM remain the way to stop.
    pub async fn stop_requested(&self) {
        if !self.is_events() {
            return std::future::pending().await;
        }
        use tokio::io::AsyncReadExt;
        // A duplicate, so that dropping the receiver does not close descriptor 0.
        let pipe =
            std::io::stdin().as_fd().try_clone_to_owned().and_then(tokio::net::unix::pipe::Receiver::from_owned_fd);
        let Ok(mut pipe) = pipe else { return std::future::pending().await };
        let mut line = Vec::new();
        let mut buf = [0u8; 128];
        loop {
            match pipe.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    for &byte in &buf[..n] {
                        if byte == b'\n' {
                            if line == STOP_COMMAND.as_bytes() {
                                return;
                            }
                            line.clear();
                        } else if line.len() < 64 {
                            line.push(byte);
                        }
                    }
                }
            }
        }
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
