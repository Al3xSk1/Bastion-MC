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
    stop: mpsc::Sender<()>,
}

impl Handle {
    pub fn stop(&self) {
        // Full or closed both mean a stop is already under way.
        let _ = self.stop.try_send(());
    }
}

/// A handle wired to a channel the caller holds, for tests of the UI logic.
#[doc(hidden)]
pub fn test_handle(stop: mpsc::Sender<()>) -> Handle {
    Handle { stop }
}

/// Where the launcher binary is: next to this program (true both for `/usr/bin` and for a
/// cargo target directory), else the packaged location. `$PATH` is deliberately not
/// searched.
/// Run `bastion-mc --events scram --yes` to its end and return what it reported, one line
/// per entry. This is deliberately not a [`Request`]: it must work while another command
/// (a running game) holds the one job slot — stopping that game is part of what it does.
pub async fn scram(launcher: PathBuf) -> Result<Vec<String>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new(&launcher)
            .arg(EVENTS_FLAG)
            .args(["scram", "--yes"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "the launcher did not finish within a minute".to_string())?
    .map_err(|e| format!("cannot run {}: {e}", launcher.display()))?;
    let mut lines = Vec::new();
    let mut failure = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        match Event::parse_line(line) {
            Some(Event::Status { text }) => lines.push(text),
            Some(Event::Error { message, .. }) => failure = Some(message),
            _ => {}
        }
    }
    match failure {
        Some(message) => Err(message),
        None if lines.is_empty() => Err(format!("the launcher reported nothing ({})", output.status)),
        None => Ok(lines),
    }
}

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
    let (stop, mut stop_requests) = mpsc::channel(1);
    let _ = updates.send(Update::Started(Handle { stop })).await;

    let mut control = child.stdin.take();
    let stdout = tokio::spawn(forward(child.stdout.take().expect("piped stdout"), updates.clone(), true));
    let stderr = tokio::spawn(forward(child.stderr.take().expect("piped stderr"), updates.clone(), false));

    let mut kill_at: Option<Instant> = None;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            // An explicit stop, every handle dropped, or nobody listening any more: all of
            // them end the command the same, orderly way.
            _ = async { tokio::select! { _ = stop_requests.recv() => {}, _ = updates.closed() => {} } }, if control.is_some() => {
                if let Some(mut pipe) = control.take() {
                    let _ = pipe.write_all(format!("{STOP_COMMAND}\n").as_bytes()).await;
                    let _ = pipe.shutdown().await;
                }
                kill_at = Some(Instant::now() + STOP_GRACE);
            }
            _ = tokio::time::sleep_until(kill_at.unwrap_or_else(Instant::now)), if kill_at.is_some() => {
                let _ = child.start_kill();
                kill_at = None;
            }
        }
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
    let mut out: String = text
        .chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .map(|c| if c == '\t' { ' ' } else { c })
        .take(MAX_STRAY_CHARS)
        .collect();
    if text.chars().count() > MAX_STRAY_CHARS {
        out.push('…');
    }
    out
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
}
