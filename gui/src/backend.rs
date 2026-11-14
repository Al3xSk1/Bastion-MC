//! The backend worker.
//!
//! The launcher is not a library to this program: it is a separate, hardened process, and
//! tokens only ever exist there. One [`Request`] becomes one `bastion-mc --events …` child;
//! a tokio task reads the child's event lines and forwards them, typed, over a bounded
//! `tokio::sync::mpsc` channel. Nothing in here knows about widgets, and nothing in the UI
//! ever waits on the network, the disk or the game: it only receives [`Update`]s.
//!
//! ```text
//!   UI thread (iced)                tokio task (this file)            launcher process
//!   ────────────────                ──────────────────────            ────────────────
//!   Request ──────── spawn ───────▶ Command::spawn ──────────────────▶ bastion-mc --events …
//!   Message::Backend ◀── mpsc ───── parse line ◀──────── stdout ────── {"event":"progress",…}
//!   Handle::stop() ─── mpsc ──────▶ write "stop\n" ───── stdin ──────▶ graceful shutdown
//! ```
//!
//! If the UI goes away (window closed, crash) the channel closes, the task closes the
//! child's standard input, and the launcher shuts the game down on its own.
//!
//! The emergency stop ([`scram`]) is the one command that is handled the other way round:
//! nothing this program does — a stop, a closed window, a time limit — ever ends the
//! launcher that is removing credentials. It is started, listened to, and otherwise left
//! alone.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use bastion_mc_proto::{EVENTS_FLAG, Event, MAX_LINE_BYTES, STOP_COMMAND};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
    time::Instant,
};

/// Updates buffered between the worker and the UI. When the UI falls behind, the worker
/// waits; the launcher then drops game output on its side instead of stalling the game.
const CHANNEL_CAPACITY: usize = 1024;
/// How long the launcher gets to shut the game down after a stop before it is killed. The
/// launcher itself gives the game 15 seconds.
const STOP_GRACE: Duration = Duration::from_secs(30);
/// How long output may keep arriving after the launcher itself has exited. Its pipes close
/// with it; this only matters if some stray descendant still holds one open.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
const MAX_STRAY_CHARS: usize = 2000;
/// How long the window waits for a scram before it says "still running" and stops waiting.
/// Reaching it ends nothing: the launcher is left to finish. A scram normally takes a
/// second or two; the slow part is deleting what killed games left behind, which is
/// bounded by the size of that data, not by a clock.
pub const SCRAM_WAIT_LIMIT: Duration = Duration::from_secs(30 * 60);

/// Everything the UI can ask of the launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Sign-in state and the instance list.
    Status {
        account: String,
    },
    /// Make `account` the one the launcher uses when none is named.
    DefaultAccount {
        account: String,
    },
    SignIn {
        account: String,
        device_code: bool,
    },
    SignOut {
        account: String,
    },
    Versions {
        snapshots: bool,
    },
    CreateInstance {
        id: String,
        minecraft: String,
    },
    Launch {
        instance: String,
        account: String,
        demo: bool,
    },
}

impl Request {
    /// The launcher command line for this request (after the `--events` switch).
    pub fn args(&self) -> Vec<String> {
        let owned = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        // An empty label means "whichever account the launcher uses by default".
        let with_account = |mut args: Vec<String>, account: &String| {
            if !account.is_empty() {
                args.extend(["--account".to_string(), account.clone()]);
            }
            args
        };
        match self {
            Request::Status { account } => with_account(owned(&["status"]), account),
            Request::DefaultAccount { account } => owned(&["account", "default", account]),
            Request::SignIn { account, device_code } => {
                let mut args = with_account(owned(&["login"]), account);
                if *device_code {
                    args.push("--device".into());
                }
                args
            }
            Request::SignOut { account } => with_account(owned(&["logout"]), account),
            Request::Versions { snapshots } => {
                let mut args = owned(&["versions", "--limit", "400"]);
                if *snapshots {
                    args.push("--snapshots".into());
                }
                args
            }
            Request::CreateInstance { id, minecraft } => owned(&["instance", "new", id, "--minecraft", minecraft]),
            Request::Launch { instance, account, demo } => {
                let mut args = with_account(owned(&["launch", instance]), account);
                if *demo {
                    args.push("--demo".into());
                }
                args
            }
        }
    }
}

/// What the worker tells the UI.
#[derive(Debug, Clone)]
pub enum Update {
    /// The launcher is running. Keep the handle to be able to stop it.
    Started(Handle),
    Event(Event),
    /// Output that is not an event (a diagnostic on standard error, for instance), cleaned
    /// of control characters.
    Stray(String),
    /// The launcher is gone. Always the last update of a request.
    Finished(Outcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Exited(i32),
    /// Killed by a signal (including by this worker after [`STOP_GRACE`]).
    Killed,
    FailedToStart(String),
}

/// Asks the launcher to stop what it is doing. Cheap to clone; dropping every clone has the
/// same effect as calling [`Handle::stop`].
#[derive(Debug, Clone)]
pub struct Handle {
    /// Each request carries the time the launcher gets before it is killed.
    stop: mpsc::Sender<Duration>,
}

impl Handle {
    pub fn stop(&self) {
        self.stop_within(STOP_GRACE);
    }

    /// Like [`Handle::stop`], with `grace` instead of the default before the launcher is
    /// killed. Of several requests the earliest deadline counts: asking again can shorten
    /// the wait, never lengthen it.
    pub fn stop_within(&self, grace: Duration) {
        // Full or closed both mean a stop is already under way.
        let _ = self.stop.try_send(grace);
    }
}

/// A handle wired to a channel the caller holds, for tests of the UI logic.
#[doc(hidden)]
pub fn test_handle(stop: mpsc::Sender<Duration>) -> Handle {
    Handle { stop }
}

/// What the launcher running a scram says, as it says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScramUpdate {
    /// The launcher is running: from here on it does its work whatever becomes of this
    /// program. The first update, unless the launcher could not be started.
    Started,
    /// One line of its report, cleaned of control characters. May be empty (the report has
    /// paragraphs).
    Line(String),
    /// Its own error message.
    Error(String),
    /// Always the last update.
    Finished(ScramEnd),
}

/// How a scram ended, as far as this program can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScramEnd {
    /// The launcher's exit status. Its contract: 0 only if everything was removed, 1 (and
    /// lines starting "COULD NOT") if something is still there.
    Exited(i32),
    /// Ended by a signal — not one from this program, which never sends a scram any.
    Killed,
    FailedToStart(String),
    /// [`SCRAM_WAIT_LIMIT`] passed. The launcher was left running.
    StillRunning,
}

/// Run `bastion-mc --events scram --yes` and report what it says, line by line, then how it
/// ended. Must be called inside a tokio runtime.
///
/// This is deliberately not a [`Request`]: it must work while another command (a running
/// game) holds the one job slot — stopping that game is part of what it does.
///
/// **Nothing here ever ends the launcher.** Removing credentials has to be allowed to
/// finish: there is no stop channel, no kill when the receiver or this task is dropped
/// (closing the window mid-scram leaves the launcher to complete on its own), and the time
/// limit only ends the *waiting* ([`ScramEnd::StillRunning`]). Its standard error goes to
/// `/dev/null` rather than a pipe for the same reason: a pipe whose reader has gone away
/// makes the writer fail, and a launcher that fails to print must not thereby fail to
/// remove. And it is put into a process group of its own, so that a signal aimed at this
/// program's group — Ctrl-C in the terminal the window was started from — is not aimed at
/// it as well.
pub fn scram(launcher: &Path) -> mpsc::Receiver<ScramUpdate> {
    let mut command = Command::new(launcher);
    command.arg(EVENTS_FLAG).args(["scram", "--yes"]);
    scram_with(command, SCRAM_WAIT_LIMIT)
}

fn scram_with(mut command: Command, limit: Duration) -> mpsc::Receiver<ScramUpdate> {
    let (updates, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(false).process_group(0);
    tokio::spawn(watch_scram(command, updates, limit));
    receiver
}

async fn watch_scram(mut command: Command, updates: mpsc::Sender<ScramUpdate>, limit: Duration) {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = updates.send(ScramUpdate::Finished(ScramEnd::FailedToStart(error.to_string()))).await;
            return;
        }
    };
    let _ = updates.send(ScramUpdate::Started).await;
    let (lines, mut output) = mpsc::channel(CHANNEL_CAPACITY);
    // If this function returns first (the time limit), the reader ends with it and the
    // pipe closes. The launcher is built for that — it is what a front end that exits
    // looks like: its event writer notes the broken pipe and stops writing, and the
    // command carries on.
    let reader = tokio::spawn(forward(child.stdout.take().expect("piped stdout"), lines, true));

    // A send that fails means nobody is watching any more. That changes nothing for the
    // launcher: it is still waited for, its output still read.
    let relay = async |update: Update| {
        let update = match update {
            Update::Event(Event::Status { text }) => ScramUpdate::Line(clean(&text)),
            Update::Event(Event::Error { message, .. }) => ScramUpdate::Error(clean(&message)),
            Update::Stray(text) => ScramUpdate::Line(text),
            _ => return,
        };
        let _ = updates.send(update).await;
    };
    let deadline = Instant::now() + limit;
    let mut output_open = true;
    let end = loop {
        tokio::select! {
            line = output.recv(), if output_open => match line {
                Some(update) => relay(update).await,
                None => output_open = false,
            },
            status = child.wait() => break match status {
                Ok(status) => status.code().map_or(ScramEnd::Killed, ScramEnd::Exited),
                Err(error) => ScramEnd::FailedToStart(error.to_string()),
            },
            _ = tokio::time::sleep_until(deadline) => break ScramEnd::StillRunning,
        }
    };
    if end != ScramEnd::StillRunning {
        // Everything it wrote before it exited is delivered before `Finished`.
        let drained = async {
            while let Some(update) = output.recv().await {
                relay(update).await;
            }
        };
        if tokio::time::timeout(DRAIN_GRACE, drained).await.is_err() {
            reader.abort();
        }
    }
    let _ = updates.send(ScramUpdate::Finished(end)).await;
}

/// Where the launcher binary is: next to this program (true both for `/usr/bin` and for a
/// cargo target directory), else the packaged location. `$PATH` is deliberately not
/// searched.
pub fn default_launcher() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("bastion-mc")))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("/usr/bin/bastion-mc"))
}

/// Start the launcher for `request`. Must be called inside a tokio runtime. The returned
/// channel yields [`Update::Started`] (unless the program cannot be started), then events,
/// then exactly one [`Update::Finished`].
pub fn spawn(launcher: &Path, request: &Request) -> mpsc::Receiver<Update> {
    let (updates, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    let mut command = Command::new(launcher);
    command.arg(EVENTS_FLAG).args(request.args()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    supervise_in_background(command, updates);
    receiver
}

fn supervise_in_background(command: Command, updates: mpsc::Sender<Update>) {
    tokio::spawn(supervise(command, updates));
}

async fn supervise(mut command: Command, updates: mpsc::Sender<Update>) {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = updates.send(Update::Finished(Outcome::FailedToStart(error.to_string()))).await;
            return;
        }
    };
    let (stop, mut stop_requests) = mpsc::channel(4);
    let _ = updates.send(Update::Started(Handle { stop })).await;

    let mut control = child.stdin.take();
    let stdout = tokio::spawn(forward(child.stdout.take().expect("piped stdout"), updates.clone(), true));
    let stderr = tokio::spawn(forward(child.stderr.take().expect("piped stderr"), updates.clone(), false));

    let mut kill_at: Option<Instant> = None;
    let (mut handles_alive, mut listened_to, mut killed) = (true, true, false);
    let status = loop {
        // An explicit stop, every handle dropped, or nobody listening any more: all of
        // them end the command the same, orderly way. A further request can only bring
        // the deadline forward.
        let grace = tokio::select! {
            status = child.wait() => break status,
            request = stop_requests.recv(), if handles_alive => {
                handles_alive = request.is_some();
                request.unwrap_or(STOP_GRACE)
            }
            _ = updates.closed(), if listened_to => {
                listened_to = false;
                STOP_GRACE
            }
            _ = tokio::time::sleep_until(kill_at.unwrap_or_else(Instant::now)), if kill_at.is_some() && !killed => {
                let _ = child.start_kill();
                killed = true;
                continue;
            }
        };
        if let Some(mut pipe) = control.take() {
            let _ = pipe.write_all(format!("{STOP_COMMAND}\n").as_bytes()).await;
            let _ = pipe.shutdown().await;
        }
        let deadline = Instant::now() + grace;
        kill_at = Some(kill_at.map_or(deadline, |earlier| earlier.min(deadline)));
    };
    // Everything the launcher wrote before it exited is delivered before `Finished` — but a
    // pipe that never closes must not keep the UI waiting for that `Finished` forever.
    let (stdout_abort, stderr_abort) = (stdout.abort_handle(), stderr.abort_handle());
    let drained = async {
        let _ = stdout.await;
        let _ = stderr.await;
    };
    if tokio::time::timeout(DRAIN_GRACE, drained).await.is_err() {
        stdout_abort.abort();
        stderr_abort.abort();
    }
    let outcome = match status {
        Ok(status) => status.code().map_or(Outcome::Killed, Outcome::Exited),
        Err(error) => Outcome::FailedToStart(error.to_string()),
    };
    let _ = updates.send(Update::Finished(outcome)).await;
}

/// Printable text only, bounded. The launcher already sanitises what comes out of the
/// sandbox; this covers everything else that can end up on its output.
pub fn clean(text: &str) -> String {
    clean_to(text, MAX_STRAY_CHARS)
}

/// Characters that are not shown but change how the text around them is: line and
/// paragraph separators, and the marks that reorder text (a label must not read
/// differently from what it contains).
fn is_layout_control(c: char) -> bool {
    matches!(c, '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// [`clean`] with the bound given: for the launcher's words that end up in a label.
pub fn clean_to(text: &str, max_chars: usize) -> String {
    let mut printable = text
        .chars()
        .filter(|c| *c == '\t' || !(c.is_control() || is_layout_control(*c)))
        .map(|c| if c == '\t' { ' ' } else { c });
    let mut out: String = printable.by_ref().take(max_chars).collect();
    if printable.next().is_some() {
        out.push('…');
    }
    out
}

/// `text` in at most `chars` characters, printable: all of it, or its beginning and the mark
/// of the cut. For what has to fit on one line.
pub fn fit(text: &str, chars: usize) -> String {
    let all = clean_to(text, chars);
    if all.chars().count() <= chars { all } else { clean_to(text, chars.saturating_sub(1)) }
}

/// Read `stream` line by line, with a hard bound on line length, and forward it.
async fn forward(stream: impl AsyncRead + Unpin, updates: mpsc::Sender<Update>, events: bool) {
    let mut reader = BufReader::with_capacity(16 * 1024, stream);
    let mut line = Vec::with_capacity(1024);
    loop {
        line.clear();
        let mut bounded = (&mut reader).take(MAX_LINE_BYTES as u64 + 1);
        match bounded.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let complete = line.last() == Some(&b'\n');
        if !complete && line.len() > MAX_LINE_BYTES {
            // Not something the launcher produces. Skip to the end of the line.
            loop {
                line.clear();
                let mut rest = (&mut reader).take(MAX_LINE_BYTES as u64);
                match rest.read_until(b'\n', &mut line).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) if line.last() == Some(&b'\n') => break,
                    Ok(_) => {}
                }
            }
            if updates.send(Update::Stray("(an over-long line of launcher output was dropped)".into())).await.is_err() {
                return;
            }
            continue;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        let update = match events.then(|| Event::parse_line(text)).flatten() {
            Some(event) => Update::Event(event),
            None if text.trim().is_empty() => continue,
            None => Update::Stray(clean(text)),
        };
        if updates.send(update).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_mc_proto::{Phase, Source};

    /// Run a stand-in launcher: a shell script that plays back a session and receives the
    /// arguments the real one would (`$1` is `--events`). Nothing executable is written to
    /// disk, so parallel tests cannot trip over each other's half-written scripts.
    fn spawn_fake(script: &str, request: &Request) -> mpsc::Receiver<Update> {
        let (updates, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script, "bastion-mc", EVENTS_FLAG])
            .args(request.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        supervise_in_background(command, updates);
        receiver
    }

    fn scratch_file(name: &str) -> PathBuf {
        let dir = std::env::current_exe().unwrap().parent().unwrap().join("gui-test-scratch");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}-{}", std::process::id()))
    }

    async fn collect(mut receiver: mpsc::Receiver<Update>) -> Vec<Update> {
        let mut all = Vec::new();
        while let Some(update) = tokio::time::timeout(Duration::from_secs(20), receiver.recv()).await.unwrap() {
            all.push(update);
        }
        all
    }

    fn launch() -> Request {
        Request::Launch { instance: "vanilla".into(), account: "default".into(), demo: true }
    }

    #[test]
    fn requests_become_launcher_command_lines() {
        let args = |r: Request| r.args().join(" ");
        assert_eq!(args(Request::Status { account: "default".into() }), "status --account default");
        assert_eq!(args(Request::SignIn { account: "a".into(), device_code: true }), "login --account a --device");
        assert_eq!(args(Request::SignIn { account: "a".into(), device_code: false }), "login --account a");
        assert_eq!(args(Request::SignOut { account: "a".into() }), "logout --account a");
        assert_eq!(args(Request::Status { account: String::new() }), "status", "the launcher's default account");
        assert_eq!(args(Request::DefaultAccount { account: "work".into() }), "account default work");
        assert_eq!(args(Request::Versions { snapshots: true }), "versions --limit 400 --snapshots");
        assert_eq!(
            args(Request::CreateInstance { id: "survival".into(), minecraft: "26.3".into() }),
            "instance new survival --minecraft 26.3"
        );
        assert_eq!(args(launch()), "launch vanilla --account default --demo");
    }

    #[tokio::test]
    async fn events_strays_and_exit_status_arrive_in_order() {
        let script = r#"[ "$1" = "--events" ] || exit 99
echo '{"event":"hello","protocol":1,"version":"0.0.0"}'
echo '{"event":"phase","phase":"preparing"}'
echo 'a stray line with an escape: '"$(printf '\033')"'[31mred'
echo '{"event":"log","source":"game","line":"hello from the game"}'
echo '{"event":"from_the_future","x":1}'
echo "diagnostic on stderr: $2 $3" >&2
echo '{"event":"exited","code":0}'
exit 7"#;
        let updates = collect(spawn_fake(script, &launch())).await;
        assert!(matches!(updates.first(), Some(Update::Started(_))), "{updates:?}");
        assert!(matches!(updates.last(), Some(Update::Finished(Outcome::Exited(7)))), "{updates:?}");
        let events: Vec<&Event> =
            updates.iter().filter_map(|u| if let Update::Event(e) = u { Some(e) } else { None }).collect();
        assert_eq!(
            events,
            vec![
                &Event::Hello { protocol: 1, version: "0.0.0".into() },
                &Event::Phase { phase: Phase::Preparing },
                &Event::Log { source: Source::Game, line: "hello from the game".into() },
                &Event::Unknown,
                &Event::Exited { code: 0 },
            ]
        );
        let strays: Vec<&str> =
            updates.iter().filter_map(|u| if let Update::Stray(s) = u { Some(s.as_str()) } else { None }).collect();
        assert!(strays.contains(&"a stray line with an escape: [31mred"), "control characters are removed: {strays:?}");
        assert!(strays.contains(&"diagnostic on stderr: launch vanilla"), "{strays:?}");
    }

    #[tokio::test]
    async fn stop_is_delivered_on_standard_input() {
        let script = r#"echo '{"event":"phase","phase":"running"}'
read -r command
echo "{\"event\":\"status\",\"text\":\"got $command\"}"
exit 130"#;
        let mut receiver = spawn_fake(script, &launch());
        let Some(Update::Started(handle)) = receiver.recv().await else { panic!("no handle") };
        assert!(matches!(receiver.recv().await, Some(Update::Event(Event::Phase { phase: Phase::Running }))));
        handle.stop();
        handle.stop(); // idempotent
        let rest = collect(receiver).await;
        assert!(matches!(&rest[0], Update::Event(Event::Status { text }) if text == "got stop"), "{rest:?}");
        assert!(matches!(rest.last(), Some(Update::Finished(Outcome::Exited(130)))));
    }

    #[tokio::test]
    async fn a_vanished_ui_stops_the_launcher() {
        let marker = scratch_file("stopped");
        let _ = std::fs::remove_file(&marker);
        let script = format!("read -r command; echo \"${{command:-eof}}\" > '{}'; exit 130", marker.display());
        let mut receiver = spawn_fake(&script, &launch());
        assert!(matches!(receiver.recv().await, Some(Update::Started(_))));
        drop(receiver); // the window is gone; the handle inside `Started` went with it
        for _ in 0..200 {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let seen = std::fs::read_to_string(&marker).expect("the launcher was told to stop");
        assert_eq!(seen.trim(), "stop");
        let _ = std::fs::remove_file(&marker);
    }

    #[tokio::test]
    async fn a_descendant_holding_the_pipe_open_cannot_delay_the_end() {
        // The stand-in exits at once but leaves a background process with its stdout.
        let script = r#"echo '{"event":"exited","code":0}'
sleep 30 &
exit 0"#;
        let started = std::time::Instant::now();
        let updates = collect(spawn_fake(script, &launch())).await;
        assert!(matches!(updates.last(), Some(Update::Finished(Outcome::Exited(0)))), "{updates:?}");
        assert!(updates.iter().any(|u| matches!(u, Update::Event(Event::Exited { code: 0 }))));
        assert!(started.elapsed() < Duration::from_secs(15), "took {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn a_missing_launcher_is_reported_not_panicked_on() {
        let updates = collect(spawn(Path::new("/nonexistent/bastion-mc"), &launch())).await;
        assert!(matches!(updates.as_slice(), [Update::Finished(Outcome::FailedToStart(_))]), "{updates:?}");
    }

    #[tokio::test]
    async fn over_long_lines_are_dropped_without_buffering_them() {
        let script = r#"head -c 300000 /dev/zero | tr '\0' 'x'; echo
echo '{"event":"exited","code":0}'"#;
        let updates = collect(spawn_fake(script, &launch())).await;
        let dropped = updates.iter().filter(|u| matches!(u, Update::Stray(s) if s.contains("over-long"))).count();
        assert_eq!(dropped, 1, "{updates:?}");
        assert!(updates.iter().any(|u| matches!(u, Update::Event(Event::Exited { code: 0 }))), "the stream recovers");
        assert!(updates.iter().all(|u| !matches!(u, Update::Stray(s) if s.len() > 3 * MAX_STRAY_CHARS)));
    }

    #[test]
    fn cleaning_removes_controls_and_bounds_length() {
        assert_eq!(clean("a\tb\u{1b}[2Jc\u{7}"), "a b[2Jc");
        let long = clean(&"é".repeat(MAX_STRAY_CHARS + 10));
        assert_eq!(long.chars().count(), MAX_STRAY_CHARS + 1);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn fitting_never_returns_more_characters_than_asked_for() {
        assert_eq!(fit("vanilla", 7), "vanilla");
        assert_eq!(fit("vanilla", 20), "vanilla");
        assert_eq!(fit("vanilla-latest", 7), "vanill…");
        assert_eq!(fit("van\u{1b}illa", 7), "vanilla", "what is not shown does not count");
        for chars in 1..40 {
            let fitted = fit(&"é".repeat(30), chars);
            assert!(fitted.chars().count() <= chars, "{chars}: {fitted}");
            assert_eq!(fitted.ends_with('…'), chars < 30, "{chars}: the cut is marked, and only a cut");
        }
        // No room at all still says that something was left out.
        assert_eq!(fit("vanilla", 0), "…");
        assert_eq!(fit("", 0), "");
    }

    // ------------------------------------------------------------------ the emergency stop
    //
    // Never against the real launcher: `scram --yes` removes real sign-ins from the kernel
    // keyring, whatever directories a test points it at.

    /// Run a stand-in for the launcher the way [`scram`] runs the real one.
    fn scram_fake(script: &str, limit: Duration) -> mpsc::Receiver<ScramUpdate> {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script, "bastion-mc", EVENTS_FLAG, "scram", "--yes"]);
        scram_with(command, limit)
    }

    async fn collect_scram(mut receiver: mpsc::Receiver<ScramUpdate>) -> Vec<ScramUpdate> {
        let mut all = Vec::new();
        while let Some(update) = tokio::time::timeout(Duration::from_secs(20), receiver.recv()).await.unwrap() {
            all.push(update);
        }
        all
    }

    fn line(text: &str) -> ScramUpdate {
        ScramUpdate::Line(text.into())
    }

    /// Wait until a stand-in has written its last word to `marker`: it was not killed.
    fn finished(marker: &Path) -> bool {
        for _ in 0..400 {
            if std::fs::read_to_string(marker).is_ok_and(|said| said.trim() == "finished") {
                let _ = std::fs::remove_file(marker);
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    /// A stand-in that, like the launcher (and every Rust program), does not die of a
    /// closed pipe: it reports, takes `seconds`, reports again and then leaves its mark.
    fn slow_scram(marker: &Path, seconds: &str) -> String {
        let _ = std::fs::remove_file(marker);
        [
            "trap '' PIPE".to_string(),
            r#"printf '%s\n' '{"event":"status","text":"started"}'"#.to_string(),
            format!("sleep {seconds}"),
            r#"printf '%s\n' '{"event":"status","text":"still removing"}'"#.to_string(),
            format!("echo finished > '{}'", marker.display()),
        ]
        .join("\n")
    }

    #[tokio::test]
    async fn a_scram_report_arrives_line_by_line_and_ends_with_the_launchers_exit_status() {
        // What the launcher prints when something could not be removed, and its status 1.
        let script = r#"[ "$1 $2 $3" = "--events scram --yes" ] || exit 99
printf '%s\n' '{"event":"hello","protocol":1,"version":"0.0.0"}'
printf '%s\n' '{"event":"status","text":"no launch was in progress"}'
printf '%s\n' '{"event":"status","text":"COULD NOT delete the vault file /v/alt.age: Permission denied"}'
printf '%s\n' '{"event":"status","text":""}'
printf '%s\n' '{"event":"status","text":"an escape: \u001b[31mred"}'
printf '%s\n' 'not an event'
printf '%s\n' 'on standard error' >&2
printf '%s\n' '{"event":"accounts","default":"default","accounts":[]}'
exit 1"#;
        let updates = collect_scram(scram_fake(script, SCRAM_WAIT_LIMIT)).await;
        assert_eq!(
            updates,
            [
                ScramUpdate::Started,
                line("no launch was in progress"),
                line("COULD NOT delete the vault file /v/alt.age: Permission denied"),
                line(""),
                line("an escape: [31mred"),
                line("not an event"),
                ScramUpdate::Finished(ScramEnd::Exited(1)),
            ],
            "the report in order, cleaned; the status is the launcher's, not turned into a success"
        );

        let clean = r#"printf '%s\n' '{"event":"status","text":"no launch was in progress"}'"#;
        let updates = collect_scram(scram_fake(clean, SCRAM_WAIT_LIMIT)).await;
        assert_eq!(
            updates,
            [ScramUpdate::Started, line("no launch was in progress"), ScramUpdate::Finished(ScramEnd::Exited(0))]
        );
    }

    #[tokio::test]
    async fn a_scram_runs_in_a_process_group_of_its_own() {
        // Field five of /proc/<pid>/stat is the process group: the stand-in leads its own,
        // and that is not the group of the program that started it.
        let script = r#"read -r _ _ _ _ group _ < /proc/$$/stat
read -r _ _ _ _ parents _ < /proc/$PPID/stat
[ "$group" = "$$" ] && [ "$group" != "$parents" ] && printf '%s\n' '{"event":"status","text":"a group of its own"}'
exit 0"#;
        let updates = collect_scram(scram_fake(script, SCRAM_WAIT_LIMIT)).await;
        assert_eq!(
            updates,
            [ScramUpdate::Started, line("a group of its own"), ScramUpdate::Finished(ScramEnd::Exited(0))]
        );
    }

    #[tokio::test]
    async fn a_scram_that_fails_is_killed_or_cannot_start_ends_as_exactly_that() {
        let failing = r#"printf '%s\n' '{"event":"error","message":"cannot create /run/user/1000/x","code":3}'
exit 3"#;
        let updates = collect_scram(scram_fake(failing, SCRAM_WAIT_LIMIT)).await;
        assert_eq!(
            updates,
            [
                ScramUpdate::Started,
                ScramUpdate::Error("cannot create /run/user/1000/x".into()),
                ScramUpdate::Finished(ScramEnd::Exited(3))
            ]
        );

        let updates = collect_scram(scram_fake("kill -9 $$", SCRAM_WAIT_LIMIT)).await;
        assert_eq!(updates, [ScramUpdate::Started, ScramUpdate::Finished(ScramEnd::Killed)]);

        // Nothing was started, and nothing says that something was.
        let updates = collect_scram(scram(Path::new("/nonexistent/bastion-mc"))).await;
        assert!(matches!(updates.as_slice(), [ScramUpdate::Finished(ScramEnd::FailedToStart(_))]), "{updates:?}");
    }

    #[tokio::test]
    async fn a_scram_that_outlasts_the_wait_limit_is_reported_as_still_running_and_is_not_killed() {
        let marker = scratch_file("scram-outlasts-the-limit");
        let script = slow_scram(&marker, "3");
        let updates = collect_scram(scram_fake(&script, Duration::from_millis(300))).await;
        assert_eq!(
            updates,
            [ScramUpdate::Started, line("started"), ScramUpdate::Finished(ScramEnd::StillRunning)],
            "the waiting ends at the limit, and what ended is said"
        );
        assert!(!marker.exists(), "the stand-in was still at work when the waiting ended");
        assert!(
            tokio::task::spawn_blocking(move || finished(&marker)).await.unwrap(),
            "nothing killed it: it ran to its end, with nobody reading its output"
        );
    }

    #[tokio::test]
    async fn a_scram_nobody_listens_to_any_more_runs_to_its_end() {
        let marker = scratch_file("scram-unwatched");
        let mut receiver = scram_fake(&slow_scram(&marker, "0.5"), SCRAM_WAIT_LIMIT);
        assert_eq!(receiver.recv().await, Some(ScramUpdate::Started));
        assert_eq!(receiver.recv().await, Some(line("started")));
        drop(receiver); // the window stopped listening
        assert!(tokio::task::spawn_blocking(move || finished(&marker)).await.unwrap());
    }

    #[test]
    fn a_scram_survives_the_front_end_going_away() {
        let marker = scratch_file("scram-outlives-the-runtime");
        let script = slow_scram(&marker, "1");
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let first = runtime.block_on(async {
            let mut receiver = scram_fake(&script, SCRAM_WAIT_LIMIT);
            receiver.recv().await
        });
        // The earliest moment the window lets itself be closed during a scram (see
        // `app`): the launcher exists, and has perhaps not said a word yet.
        assert_eq!(first, Some(ScramUpdate::Started));
        // Everything this program had running is gone, as when its window is closed: the
        // task that watched the launcher, the handle to the child, the pipe it wrote to.
        drop(runtime);
        assert!(finished(&marker), "the launcher was neither killed nor stopped by its output going nowhere");
    }
}
