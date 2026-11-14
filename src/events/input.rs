//! Trusted input: answers to prompts, from whoever started the launcher — never from the game.
//!
//! Some prompts can only be answered by a person outside the game: the address a Bedrock
//! sign-in page ended on (`bedrock::signin`), the approval of a Realms join in a session that
//! has no phone to type a code into. Code in the game must not be able to give those answers
//! itself, so they are read from exactly two places, both of which only the user controls:
//!
//! * **event mode**: the front end's control pipe (the launcher's standard input), as lines
//!   `answer <kind> <text>` beside the `stop` line ([`bastion_mc_proto::answer`] has the
//!   grammar; [`control_loop`] reads them);
//! * **a terminal launch**: the terminal it was started from, while something in the
//!   command takes answers ([`spawn_terminal`]). There a bare line is enough when one prompt
//!   waits; with two (a sign-in and a Realms join at once) the line starts with the kind.
//!
//! The game cannot write to either: its standard input is a socket of the launcher's own,
//! the sandbox starts a new session (no controlling terminal, so no `TIOCSTI`), and both the
//! game's system-call filter and the confined launcher's deny the terminal-injection ioctls.
//!
//! Who asks for answers subscribes per kind ([`Registry::subscribe`], through
//! `Reporter::answers`) and gets them in order on an [`AnswerReceiver`]; at most one
//! subscriber per kind, for the rest of the command or until it drops the receiver. Each
//! answer is checked against the grammar here (printable ASCII, bounded) and against the
//! prompt by the subscriber, which also says what became of it.
//!
//! What this does **not** do: decide whether an answer is right (the subscriber does), or
//! make an answer secret — the terminal echoes what is typed, and a front end shows it in its
//! input field. The answers this carries are not secrets that outlive their prompt: a sign-in
//! code is single-use and useless without the game's PKCE verifier, a Realms code is good for
//! one prompt.

use std::{
    io::Read,
    os::fd::{AsFd, OwnedFd},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc,
};

pub use bastion_mc_proto::{Answer, AnswerKind, ControlLine, MAX_ANSWER_TEXT, MAX_CONTROL_LINE};
use bastion_mc_proto::{CANCEL, parse_control_line};

/// Answers queued for one subscriber before further ones are turned away as "busy". Prompts
/// take one answer at a time; this only absorbs a burst.
const QUEUE: usize = 4;

/// How often the terminal reader looks whether the command it reads for is still there.
const TERMINAL_POLL: u16 = 1000;

/// What became of an answer handed to the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    /// Nothing in this command takes answers of this kind.
    NobodyAsks,
    /// The subscriber has not taken the answers before this one yet.
    Busy,
}

#[derive(Default)]
struct Slot {
    /// The subscriber's number; 0 when there is none.
    id: u64,
    sender: Option<mpsc::Sender<Answer>>,
    /// A prompt of this kind waits (for routing a bare terminal line).
    pending: bool,
}

/// Who takes answers of which kind, in this command.
#[derive(Default)]
pub struct Registry {
    slots: Mutex<[Slot; 2]>,
    next_id: AtomicU64,
}

impl Registry {
    pub fn new() -> Arc<Registry> {
        Arc::new(Registry::default())
    }

    /// Take the answers of `kind` from now on. `None` if something else already takes them.
    pub fn subscribe(self: &Arc<Self>, kind: AnswerKind) -> Option<AnswerReceiver> {
        let mut slots = self.slots.lock().expect("answer registry");
        let slot = &mut slots[kind.index()];
        if slot.sender.as_ref().is_some_and(|s| !s.is_closed()) {
            return None;
        }
        let (sender, rx) = mpsc::channel(QUEUE);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        *slot = Slot { id, sender: Some(sender), pending: false };
        Some(AnswerReceiver { kind, id, rx, registry: Arc::downgrade(self) })
    }

    /// Hand `answer` to whoever takes answers of `kind`.
    pub fn deliver(&self, kind: AnswerKind, answer: Answer) -> Delivery {
        let slots = self.slots.lock().expect("answer registry");
        match &slots[kind.index()].sender {
            None => Delivery::NobodyAsks,
            Some(sender) => match sender.try_send(answer) {
                Ok(()) => Delivery::Delivered,
                Err(mpsc::error::TrySendError::Full(_)) => Delivery::Busy,
                Err(mpsc::error::TrySendError::Closed(_)) => Delivery::NobodyAsks,
            },
        }
    }

    /// Something takes answers of `kind` in this command.
    pub fn is_open(&self, kind: AnswerKind) -> bool {
        self.slots.lock().expect("answer registry")[kind.index()].sender.as_ref().is_some_and(|s| !s.is_closed())
    }

    /// The kinds whose prompt waits right now.
    pub fn pending(&self) -> Vec<AnswerKind> {
        let slots = self.slots.lock().expect("answer registry");
        AnswerKind::ALL.into_iter().filter(|kind| slots[kind.index()].pending).collect()
    }

    fn set_pending(&self, kind: AnswerKind, id: u64, pending: bool) {
        let mut slots = self.slots.lock().expect("answer registry");
        let slot = &mut slots[kind.index()];
        if slot.id == id {
            slot.pending = pending;
        }
    }

    fn unsubscribe(&self, kind: AnswerKind, id: u64) {
        let mut slots = self.slots.lock().expect("answer registry");
        if slots[kind.index()].id == id {
            slots[kind.index()] = Slot::default();
        }
    }
}

/// The answers of one kind, for the one subscriber. Dropping it ends the subscription.
pub struct AnswerReceiver {
    kind: AnswerKind,
    id: u64,
    rx: mpsc::Receiver<Answer>,
    registry: Weak<Registry>,
}

impl AnswerReceiver {
    pub fn kind(&self) -> AnswerKind {
        self.kind
    }

    /// The next answer, in the order given. `None` once nothing can deliver any more.
    pub async fn recv(&mut self) -> Option<Answer> {
        self.rx.recv().await
    }

    /// Say whether a prompt of this kind waits now. On a terminal, a bare line goes to the one
    /// prompt that waits.
    pub fn set_pending(&self, pending: bool) {
        if let Some(registry) = self.registry.upgrade() {
            registry.set_pending(self.kind, self.id, pending);
        }
    }

    /// The same switch as [`AnswerReceiver::set_pending`], for whoever learns first that a
    /// prompt opened or ended (an announcer), while the receiver lives elsewhere.
    pub fn pending_handle(&self) -> PendingHandle {
        PendingHandle { kind: self.kind, id: self.id, registry: self.registry.clone() }
    }
}

/// Says whether a prompt of one kind waits (see [`AnswerReceiver::pending_handle`]). It does
/// nothing once its subscription has ended.
#[derive(Clone)]
pub struct PendingHandle {
    kind: AnswerKind,
    id: u64,
    registry: Weak<Registry>,
}

impl PendingHandle {
    pub fn set(&self, pending: bool) {
        if let Some(registry) = self.registry.upgrade() {
            registry.set_pending(self.kind, self.id, pending);
        }
    }
}

impl Drop for AnswerReceiver {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.unsubscribe(self.kind, self.id);
        }
    }
}

/// What to tell the user about an answer line that could not be delivered, if anything.
pub fn delivery_note(kind: AnswerKind, delivery: Delivery) -> Option<String> {
    match delivery {
        Delivery::Delivered => None,
        Delivery::NobodyAsks => {
            Some(format!("an answer of the kind `{}` arrived, but nothing asks for one", kind.as_str()))
        }
        Delivery::Busy => Some(format!(
            "an answer of the kind `{}` was not taken: the ones before it are still being looked at",
            kind.as_str()
        )),
    }
}

/// Read a front end's control lines from `input` until it says `stop` or ends (both mean
/// "stop"). Answers go to `registry`; `note` is told, in a few words, about every answer
/// line that breaks the grammar or that nothing asked for. Every other line is ignored, as it
/// always was.
///
/// A line longer than [`MAX_CONTROL_LINE`] is dropped whole: never cut down to a shorter
/// line that might then mean something.
pub async fn control_loop(registry: &Registry, mut input: impl AsyncRead + Unpin, note: &(dyn Fn(String) + Sync)) {
    let mut line: Vec<u8> = Vec::with_capacity(256);
    let mut overlong = false;
    let mut buf = [0u8; 1024];
    loop {
        let n = match input.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        for &byte in &buf[..n] {
            if byte != b'\n' {
                if line.len() < MAX_CONTROL_LINE {
                    line.push(byte);
                } else {
                    overlong = true;
                }
                continue;
            }
            if overlong {
                if line.starts_with(b"answer ") {
                    note(format!("a control line longer than {MAX_CONTROL_LINE} bytes was ignored"));
                }
            } else {
                match parse_control_line(&line) {
                    ControlLine::Stop => return,
                    ControlLine::Answer { kind, answer } => {
                        if let Some(text) = delivery_note(kind, registry.deliver(kind, answer)) {
                            note(text);
                        }
                    }
                    ControlLine::Invalid(why) => note(format!("an answer line was ignored: {why}")),
                    ControlLine::Other => {}
                }
            }
            line.clear();
            overlong = false;
        }
    }
}

/// Route one line typed on the terminal. Returns what to tell the user, if anything (an
/// answer that was delivered is acknowledged by whoever asked for it).
pub fn terminal_line(registry: &Registry, raw: &[u8]) -> Option<String> {
    let text = raw.trim_ascii();
    if text.is_empty() {
        return None;
    }
    if text.len() > MAX_ANSWER_TEXT + 16 {
        return Some(format!("that line is longer than {MAX_ANSWER_TEXT} characters and was ignored"));
    }
    // The event-mode form works here too.
    if text.starts_with(b"answer ") {
        return match parse_control_line(text) {
            ControlLine::Answer { kind, answer } => delivery_note(kind, registry.deliver(kind, answer)),
            ControlLine::Invalid(why) => Some(format!("ignored: {why}")),
            ControlLine::Stop | ControlLine::Other => None,
        };
    }
    // `<kind> <text>`, for when two prompts wait.
    if let Some((word, rest)) = std::str::from_utf8(text).ok().and_then(|t| t.split_once(' '))
        && let Some(kind) = AnswerKind::parse(word)
    {
        return deliver_typed(registry, kind, rest.trim_start_matches(' '));
    }
    let Ok(text) = std::str::from_utf8(text) else {
        return Some("ignored: an answer may only contain printable ASCII characters".into());
    };
    match registry.pending().as_slice() {
        [] => Some("nothing is waiting for an answer right now; that line was ignored".into()),
        [kind] => deliver_typed(registry, *kind, text),
        _ => Some(
            "a sign-in and a Realms join both wait: start the line with `signin ` or `realms ` to say which one it answers"
                .into(),
        ),
    }
}

fn deliver_typed(registry: &Registry, kind: AnswerKind, text: &str) -> Option<String> {
    let answer = if text == CANCEL {
        Answer::Cancel
    } else {
        if let Err(why) = bastion_mc_proto::answer::check_answer_text(text) {
            return Some(format!("ignored: {why}"));
        }
        Answer::Text(text.to_string())
    };
    delivery_note(kind, registry.deliver(kind, answer))
}

/// Read answers typed on the terminal, on a thread of their own, until `input` ends or the
/// command's registry is gone. `input` is a duplicate of the launcher's standard input; it
/// stays in blocking mode (the terminal is shared with the shell, which must not inherit a
/// non-blocking terminal), so the thread waits with `poll` and a timeout instead.
///
/// It reads only while the launcher is the terminal's foreground job: a read from the
/// controlling terminal by a background job (`bastion-mc launch … &`) stops the whole
/// process with `SIGTTIN` — the relay and the supervisor with it. In the background the
/// thread waits instead, and what is typed stays with whatever has the terminal. (A job that
/// is put into the background between the check and the read is stopped by the kernel as any
/// other program would be, until it is brought back.)
///
/// `say` prints what the user should know about a line (it is the terminal's own output).
pub fn spawn_terminal(input: OwnedFd, registry: Weak<Registry>, say: Arc<dyn Fn(&str) + Send + Sync>) {
    let _ = std::thread::Builder::new().name("terminal-answers".into()).spawn(move || {
        terminal_loop(input, &registry, &*say);
    });
}

fn terminal_loop(input: OwnedFd, registry: &Weak<Registry>, say: &dyn Fn(&str)) {
    use nix::poll::{PollFd, PollFlags, poll};
    let mut file = std::fs::File::from(input);
    let mut line: Vec<u8> = Vec::with_capacity(256);
    let mut overlong = false;
    let mut buf = [0u8; 1024];
    loop {
        if registry.strong_count() == 0 {
            return;
        }
        let mut fds = [PollFd::new(file.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, TERMINAL_POLL) {
            Ok(0) | Err(nix::errno::Errno::EINTR) => continue,
            Ok(_) => {}
            Err(_) => return,
        }
        if !foreground(&file) {
            std::thread::sleep(TERMINAL_EXIT_WITHIN);
            continue;
        }
        let n = match file.read(&mut buf) {
            Ok(0) => {
                if registry.upgrade().is_some_and(|r| AnswerKind::ALL.into_iter().any(|k| r.is_open(k))) {
                    say("bastion-mc: standard input ended: prompts can no longer be answered here");
                }
                return;
            }
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) => {
                continue;
            }
            Err(_) => return,
        };
        let Some(registry) = registry.upgrade() else { return };
        for &byte in &buf[..n] {
            if byte != b'\n' {
                if line.len() <= MAX_ANSWER_TEXT + 16 {
                    line.push(byte);
                } else {
                    overlong = true;
                }
                continue;
            }
            let note = if overlong {
                Some(format!("that line is longer than {MAX_ANSWER_TEXT} characters and was ignored"))
            } else {
                terminal_line(&registry, &line)
            };
            if let Some(note) = note {
                say(&format!("bastion-mc: {note}"));
            }
            line.clear();
            overlong = false;
        }
    }
}

/// How long [`spawn_terminal`]'s thread may take to notice that its command is gone.
pub const TERMINAL_EXIT_WITHIN: Duration = Duration::from_millis(TERMINAL_POLL as u64);

/// May `input` be read without the risk of `SIGTTIN`? Yes unless it is this process's
/// controlling terminal and another job is in its foreground. (`tcgetpgrp` fails for anything
/// that is not the caller's controlling terminal — a pipe, a file, another terminal — and job
/// control does not apply to those.)
fn foreground(input: &impl AsFd) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: tcgetpgrp(3) is ioctl(TIOCGPGRP) on a descriptor the borrow keeps open; it
    // reads nothing from the caller's memory and writes nothing to it.
    let group = unsafe { libc::tcgetpgrp(input.as_fd().as_raw_fd()) };
    group < 0 || nix::unistd::Pid::from_raw(group) == nix::unistd::getpgrp()
}

/// Fuzz harness: bytes on the control pipe and lines typed on a terminal. The control loop
/// ends whatever arrives, delivers the same answers however the bytes are cut into reads, and
/// every answer it or the terminal router delivers follows the grammar.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    fn drain(receivers: &mut [AnswerReceiver]) -> Vec<(AnswerKind, Answer)> {
        let mut out = Vec::new();
        for receiver in receivers.iter_mut() {
            while let Ok(answer) = receiver.rx.try_recv() {
                if let Answer::Text(text) = &answer {
                    assert!(bastion_mc_proto::answer::check_answer_text(text).is_ok(), "{text:?}");
                    assert_ne!(text, CANCEL);
                }
                out.push((receiver.kind, answer));
            }
        }
        out
    }
    let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a runtime");
    let mut outcomes = Vec::new();
    for piece in [data.len().max(1), 7, 1] {
        let registry = Registry::new();
        // Queues large enough for every line of the input, so that "busy" never differs.
        let mut receivers: Vec<AnswerReceiver> = AnswerKind::ALL
            .into_iter()
            .map(|kind| {
                let (sender, rx) = mpsc::channel(data.len() / 8 + QUEUE);
                registry.slots.lock().expect("answer registry")[kind.index()] =
                    Slot { id: 1, sender: Some(sender), pending: false };
                AnswerReceiver { kind, id: 1, rx, registry: Arc::downgrade(&registry) }
            })
            .collect();
        let chunks: Vec<&[u8]> = data.chunks(piece).collect();
        let reader = tokio_test_reader(chunks);
        let quiet = |_n: String| {};
        runtime.block_on(control_loop(&registry, reader, &quiet));
        outcomes.push(drain(&mut receivers));
    }
    assert!(outcomes.windows(2).all(|w| w[0] == w[1]), "the cut into reads changed the answers");
    // The same bytes typed on a terminal, line by line, with one prompt waiting and then two.
    let registry = Registry::new();
    let mut receivers: Vec<AnswerReceiver> =
        AnswerKind::ALL.into_iter().map(|kind| registry.subscribe(kind).expect("free")).collect();
    for (n, line) in data.split(|b| *b == b'\n').take(64).enumerate() {
        receivers[0].set_pending(true);
        receivers[1].set_pending(n % 2 == 1);
        if let Some(note) = terminal_line(&registry, line) {
            assert!(!note.chars().any(char::is_control), "{note:?}");
        }
        drain(&mut receivers);
    }
}

/// An `AsyncRead` that hands out the given chunks one read at a time.
fn tokio_test_reader(chunks: Vec<&[u8]>) -> impl AsyncRead + Unpin + '_ {
    struct Chunks<'a>(std::collections::VecDeque<&'a [u8]>);
    impl AsyncRead for Chunks<'_> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if let Some(chunk) = self.0.pop_front() {
                let n = chunk.len().min(buf.remaining());
                buf.put_slice(&chunk[..n]);
                if n < chunk.len() {
                    self.0.push_front(&chunk[n..]);
                }
            }
            std::task::Poll::Ready(Ok(()))
        }
    }
    Chunks(chunks.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Answer {
        Answer::Text(s.to_string())
    }

    #[tokio::test]
    async fn answers_reach_the_one_subscriber_of_their_kind_in_order() {
        let registry = Registry::new();
        assert_eq!(registry.deliver(AnswerKind::SignIn, text("x")), Delivery::NobodyAsks);
        let mut signin = registry.subscribe(AnswerKind::SignIn).unwrap();
        assert!(registry.subscribe(AnswerKind::SignIn).is_none(), "one subscriber per kind");
        assert!(registry.is_open(AnswerKind::SignIn) && !registry.is_open(AnswerKind::Realms));
        assert_eq!(registry.deliver(AnswerKind::SignIn, text("a")), Delivery::Delivered);
        assert_eq!(registry.deliver(AnswerKind::SignIn, Answer::Cancel), Delivery::Delivered);
        assert_eq!(registry.deliver(AnswerKind::Realms, text("1")), Delivery::NobodyAsks, "not crossed over");
        assert_eq!(signin.recv().await, Some(text("a")));
        assert_eq!(signin.recv().await, Some(Answer::Cancel));
        // A burst beyond the queue is turned away rather than buffered without bound.
        for _ in 0..QUEUE {
            assert_eq!(registry.deliver(AnswerKind::SignIn, text("b")), Delivery::Delivered);
        }
        assert_eq!(registry.deliver(AnswerKind::SignIn, text("c")), Delivery::Busy);
        // Dropping the receiver ends the subscription; the kind can be taken again.
        drop(signin);
        assert!(!registry.is_open(AnswerKind::SignIn));
        assert_eq!(registry.deliver(AnswerKind::SignIn, text("d")), Delivery::NobodyAsks);
        let mut again = registry.subscribe(AnswerKind::SignIn).unwrap();
        assert_eq!(registry.deliver(AnswerKind::SignIn, text("e")), Delivery::Delivered);
        assert_eq!(again.recv().await, Some(text("e")), "nothing of the old subscription is left over");
    }

    #[tokio::test]
    async fn the_control_loop_routes_answers_and_still_stops_exactly_as_before() {
        let registry = Registry::new();
        let mut signin = registry.subscribe(AnswerKind::SignIn).unwrap();
        let notes = Mutex::new(Vec::new());
        let note = |n: String| notes.lock().unwrap().push(n);
        let long = format!("answer signin {}\n", "y".repeat(MAX_CONTROL_LINE));
        let input = [
            "not a command\n",
            "answer signin ms-xal-0000000048183522://auth?code=a&state=b\n",
            "answer realms 123456\n",
            "answer phone 1\n",
            "answer signin cancel\n",
            long.as_str(),
            "stop\r\n",
            "answer signin after-the-noise\n",
            "stop\n",
            "answer signin never-read\n",
        ]
        .concat();
        control_loop(&registry, input.as_bytes(), &note).await;
        assert_eq!(signin.recv().await, Some(text("ms-xal-0000000048183522://auth?code=a&state=b")));
        assert_eq!(signin.recv().await, Some(Answer::Cancel));
        assert_eq!(signin.recv().await, Some(text("after-the-noise")));
        assert!(signin.rx.try_recv().is_err(), "nothing after the stop line is read");
        let notes = notes.into_inner().unwrap();
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes[0].contains("`realms`") && notes[0].contains("nothing asks"), "{notes:?}");
        assert!(notes[1].contains("`signin` or `realms`"), "{notes:?}");
        assert!(notes[2].contains("longer than"), "{notes:?}");
    }

    #[tokio::test]
    async fn the_end_of_input_stops_and_a_line_split_across_reads_is_one_line() {
        let registry = Registry::new();
        let mut realms = registry.subscribe(AnswerKind::Realms).unwrap();
        let (mut writer, reader) = tokio::io::duplex(8);
        let feeder = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            for piece in ["answ", "er rea", "lms 65", "4321\n"] {
                writer.write_all(piece.as_bytes()).await.unwrap();
                tokio::task::yield_now().await;
            }
            // Dropping the writer is the front end going away.
        });
        let silent = |_n: String| {};
        tokio::time::timeout(Duration::from_secs(5), control_loop(&registry, reader, &silent)).await.unwrap();
        feeder.await.unwrap();
        assert_eq!(realms.recv().await, Some(text("654321")));
    }

    #[test]
    fn a_bare_terminal_line_answers_the_one_prompt_that_waits() {
        let registry = Registry::new();
        let mut signin = registry.subscribe(AnswerKind::SignIn).unwrap();
        let mut realms = registry.subscribe(AnswerKind::Realms).unwrap();
        assert!(terminal_line(&registry, b"   \t").is_none(), "blank lines are nothing");
        assert!(terminal_line(&registry, b"ms-xal-x").unwrap().contains("nothing is waiting"));
        signin.set_pending(true);
        assert_eq!(terminal_line(&registry, b"  ms-xal-0000000048183522://auth?code=a  \n"), None);
        assert_eq!(signin.rx.try_recv().unwrap(), text("ms-xal-0000000048183522://auth?code=a"));
        realms.set_pending(true);
        assert!(terminal_line(&registry, b"123456").unwrap().contains("start the line with"));
        assert_eq!(terminal_line(&registry, b"realms 123456"), None);
        assert_eq!(realms.rx.try_recv().unwrap(), text("123456"));
        assert_eq!(terminal_line(&registry, b"signin cancel"), None);
        assert_eq!(signin.rx.try_recv().unwrap(), Answer::Cancel);
        assert_eq!(terminal_line(&registry, b"answer realms cancel"), None);
        assert_eq!(realms.rx.try_recv().unwrap(), Answer::Cancel);
        signin.set_pending(false);
        assert_eq!(terminal_line(&registry, b"cancel"), None, "the one waiting prompt is the Realms join");
        assert_eq!(realms.rx.try_recv().unwrap(), Answer::Cancel);
        assert!(terminal_line(&registry, "caf\u{e9}".as_bytes()).unwrap().contains("printable ASCII"));
        assert!(terminal_line(&registry, b"a\x1b[2Jb").unwrap().contains("printable ASCII"));
        assert!(terminal_line(&registry, &vec![b'x'; MAX_ANSWER_TEXT + 17]).unwrap().contains("longer than"));
        drop(realms);
        assert!(terminal_line(&registry, b"realms 1").unwrap().contains("nothing asks"));
    }

    #[test]
    fn the_terminal_reader_delivers_typed_lines_and_ends_with_its_input() {
        let registry = Registry::new();
        let mut signin = registry.subscribe(AnswerKind::SignIn).unwrap();
        signin.set_pending(true);
        let (reader, writer) = std::io::pipe().unwrap();
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = said.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let weak = Arc::downgrade(&registry);
        std::thread::spawn(move || {
            terminal_loop(OwnedFd::from(reader), &weak, &|line: &str| sink.lock().unwrap().push(line.to_string()));
            let _ = done_tx.send(());
        });
        let mut writer = std::fs::File::from(OwnedFd::from(writer));
        use std::io::Write;
        writer.write_all(b"ms-xal-0000000048183522://auth?code=c&state=d\nunknown words\n").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let first = loop {
            if let Ok(answer) = signin.rx.try_recv() {
                break answer;
            }
            assert!(std::time::Instant::now() < deadline, "the typed line did not arrive");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(first, text("ms-xal-0000000048183522://auth?code=c&state=d"));
        drop(writer);
        done_rx.recv_timeout(Duration::from_secs(5)).expect("the reader ends with its input");
        let said = said.lock().unwrap().clone();
        // "unknown words" went to the waiting sign-in as well (it decides); the end of input
        // is said because a subscriber was still there.
        assert_eq!(signin.rx.try_recv().unwrap(), text("unknown words"));
        assert_eq!(said, ["bastion-mc: standard input ended: prompts can no longer be answered here"]);
    }

    #[test]
    fn only_a_controlling_terminal_owned_by_another_job_is_left_unread() {
        // A pipe (a terminal launch fed by a script) has no job control: always read.
        let (reader, _writer) = std::io::pipe().unwrap();
        assert!(foreground(&reader));
        // Whatever the test runner's standard input is, the answer is a plain yes or no and
        // asking has no side effect.
        let _ = foreground(&std::io::stdin());
    }

    /// For a manual check with a real terminal (`docs/notes/bedrock-signin.md` says how it was
    /// run): prints whether standard input may be read without `SIGTTIN`.
    #[test]
    #[ignore = "a probe for a pseudo-terminal experiment, not a test"]
    fn probe_foreground_of_standard_input() {
        println!("foreground={}", foreground(&std::io::stdin()));
    }

    #[test]
    fn the_terminal_reader_stops_when_its_command_is_gone() {
        let registry = Registry::new();
        let (reader, _writer) = std::io::pipe().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let weak = Arc::downgrade(&registry);
        std::thread::spawn(move || {
            terminal_loop(OwnedFd::from(reader), &weak, &|_: &str| {});
            let _ = done_tx.send(());
        });
        drop(registry);
        done_rx.recv_timeout(TERMINAL_EXIT_WITHIN * 3).expect("the reader noticed");
    }
}
