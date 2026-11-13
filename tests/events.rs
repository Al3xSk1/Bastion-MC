//! The `--events` contract, exercised through the real binary the way a front end uses it:
//! isolated XDG directories, events parsed from standard output, control over standard input.

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

use bastion_mc::events::{Account, Event, PROTOCOL, Phase};

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("events-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Sandbox { root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bastion-mc"));
        command
            .arg("--events")
            .args(args)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Run to completion with no control channel; return the events and the exit status.
    fn run(&self, args: &[&str]) -> (Vec<Event>, i32) {
        let output = self.command(args).stdin(Stdio::null()).output().unwrap();
        let text = String::from_utf8(output.stdout).expect("events are UTF-8");
        let events = text
            .lines()
            .map(|line| Event::parse_line(line).unwrap_or_else(|| panic!("not an event: {line:?}")))
            .collect();
        (events, output.status.code().expect("exit status"))
    }

    fn write_config(&self, text: &str) {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let dir = self.root.join("config/bastion-mc");
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(dir.join("config.toml"))
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn hello() -> Event {
    Event::Hello { protocol: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string() }
}

fn next_event(lines: &mut std::io::Lines<BufReader<ChildStdout>>) -> Event {
    let line = lines.next().expect("the stream ended early").unwrap();
    Event::parse_line(&line).unwrap_or_else(|| panic!("not an event: {line:?}"))
}

fn wait_with_deadline(child: &mut Child, limit: Duration) -> i32 {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code().expect("exit status");
        }
        assert!(Instant::now() < deadline, "the launcher did not exit within {limit:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn status_reports_account_and_instances_without_touching_a_token() {
    let sandbox = Sandbox::new("status");
    let (events, code) = sandbox.run(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(events[0], hello(), "the stream always starts with the protocol version");
    let [_, Event::Account(account), Event::Instances { instances }] = events.as_slice() else {
        panic!("unexpected stream: {events:?}")
    };
    assert_eq!(
        account,
        &Account {
            account: "default".into(),
            signed_in: false,
            player: None,
            store: account.store.clone(),
            client_id_configured: false,
            device_flow: false,
        }
    );
    assert!(instances.is_empty());

    // Create an instance through the same interface and see it listed.
    let (events, code) = sandbox.run(&["instance", "new", "survival", "--minecraft", "1.21.4"]);
    assert_eq!(code, 0, "{events:?}");
    let listed =
        events.iter().find_map(|e| if let Event::Instances { instances } = e { Some(instances) } else { None });
    let listed = listed.expect("the new list follows the creation");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].id.as_str(), listed[0].minecraft.as_deref(), listed[0].kind.as_str()),
        ("survival", Some("1.21.4"), "vanilla")
    );
    assert!(listed[0].error.is_none());

    let (events, _) = sandbox.run(&["status", "--account", "second"]);
    assert!(matches!(&events[1], Event::Account(a) if a.account == "second"));
    assert!(matches!(&events[2], Event::Instances { instances } if instances.len() == 1));
}

#[test]
fn failures_are_events_with_the_exit_status_and_nothing_else_on_stdout() {
    let sandbox = Sandbox::new("errors");
    // Unknown instance: a usage error (2), reported after the launch had begun.
    let (events, code) = sandbox.run(&["launch", "nope", "--demo"]);
    assert_eq!(code, 2);
    assert_eq!(events[..2], [hello(), Event::Phase { phase: Phase::Preparing }]);
    assert!(matches!(events.last(), Some(Event::Error { message, code: 2 }) if message.contains("no instance")));

    // No client ID: a configuration error (3) that says what to do.
    let (events, code) = sandbox.run(&["login", "--device"]);
    assert_eq!(code, 3);
    assert!(matches!(events.last(), Some(Event::Error { message, code: 3 }) if message.contains("auth.client_id")));

    // A bad name never reaches the filesystem.
    let (events, code) = sandbox.run(&["instance", "new", "../escape", "--minecraft", "1.21.4"]);
    assert_eq!(code, 2);
    assert!(matches!(events.last(), Some(Event::Error { code: 2, .. })));
    assert!(!sandbox.root.join("data/bastion-mc/escape").exists());

    // Commands with no machine interface refuse instead of mixing prose into the stream.
    for command in [&["doctor"][..], &["key", "list"], &["config", "path"]] {
        let (events, code) = sandbox.run(command);
        assert_eq!(code, 2, "{command:?}");
        assert!(
            matches!(events.as_slice(), [_, Event::Error { message, code: 2 }] if message.contains("--events is not available")),
            "{command:?}: {events:?}"
        );
    }
}

#[test]
fn signing_out_clears_the_remembered_identity() {
    let sandbox = Sandbox::new("logout");
    // The kernel keyring is not part of the XDG sandbox: use an account name nobody has, so
    // that this test can never sign a real account out.
    let account = format!("test-{}", std::process::id());
    let identities = sandbox.root.join("state/bastion-mc/accounts");
    let remembered = identities.join(format!("{account}.json"));
    std::fs::create_dir_all(&identities).unwrap();
    std::fs::write(&remembered, r#"{"player":"Alex","uuid":"069a79f444e94726a5befca90e38aaf5"}"#).unwrap();

    // No token is stored, so the remembered name is not presented as a signed-in player.
    let (events, _) = sandbox.run(&["status", "--account", &account]);
    assert!(matches!(&events[1], Event::Account(a) if !a.signed_in && a.player.is_none()), "{events:?}");

    let (events, code) = sandbox.run(&["logout", "--account", &account]);
    if code != 0 && std::env::var_os("BASTION_ALLOW_NO_KEYRING").is_some() {
        eprintln!("skipped: kernel keyring unavailable in this environment ({events:?})");
        return;
    }
    assert_eq!(code, 0, "{events:?}");
    assert!(
        matches!(events.last(), Some(Event::Account(a)) if a.account == account && !a.signed_in && a.player.is_none())
    );
    assert!(!remembered.exists());
}

/// A browser sign-in waits for the user for minutes. A front end must be able to abandon it:
/// by saying `stop`, or simply by going away.
#[test]
fn a_pending_sign_in_is_abandoned_on_stop_and_when_the_front_end_disappears() {
    let sandbox = Sandbox::new("cancel");
    sandbox.write_config("[auth]\nclient_id = \"00000000-0000-4000-8000-000000000000\"\nflow = \"pkce\"\n");

    for explicit in [true, false] {
        let mut child = sandbox.command(&["login"]).stdin(Stdio::piped()).spawn().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        assert_eq!(next_event(&mut lines), hello());
        let Event::BrowserLogin { url, timeout_secs } = next_event(&mut lines) else { panic!("no sign-in prompt") };
        assert!(url.starts_with("https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?"), "{url}");
        assert!(url.contains("code_challenge_method=S256") && url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A"));
        assert!(timeout_secs >= 60);

        let mut control = child.stdin.take().unwrap();
        if explicit {
            control.write_all(b"not a command\nstop\n").unwrap();
            control.flush().unwrap();
        } else {
            drop(control); // the front end crashed or was closed
        }
        assert_eq!(next_event(&mut lines), Event::Error { message: "cancelled".into(), code: 130 });
        assert!(lines.next().is_none(), "the error is the last event");
        assert_eq!(wait_with_deadline(&mut child, Duration::from_secs(10)), 130, "explicit stop: {explicit}");
    }
}

/// Without `--events` nothing changes for people: text, on the usual streams.
#[test]
fn the_human_interface_is_unchanged() {
    let sandbox = Sandbox::new("human");
    let output = Command::new(env!("CARGO_BIN_EXE_bastion-mc"))
        .args(["status"])
        .env("XDG_CONFIG_HOME", sandbox.root.join("config"))
        .env("XDG_DATA_HOME", sandbox.root.join("data"))
        .env("XDG_CACHE_HOME", sandbox.root.join("cache"))
        .env("XDG_STATE_HOME", sandbox.root.join("state"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success());
    assert!(text.contains("account default: not signed in"), "{text}");
    assert!(!text.contains("{\"event\""), "no JSON unless asked for: {text}");
}
