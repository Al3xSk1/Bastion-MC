//! The sign-in bridge's stub as the game's client runs it: the real launcher binary, mounted
//! read-only at `/bastion/bin/mcpelauncher-webview` inside bubblewrap and started under that
//! name, with the bridge's socket directory bound **read-only** at `/bastion/run` — against
//! the launcher's server in this process.
//!
//! What this stands in for: `mcpelauncher-client`, which runs the webview as
//! `execv(path, [path, start, end])` and reads its standard output once. The client itself is
//! not on this machine; the sandbox here is a minimal one (empty root, `/usr`, no network),
//! not the launch package's Bedrock profile.
//!
//! Requirements: bubblewrap and unprivileged user namespaces (as for `tests/sandbox.rs`).

use std::{
    io::Write,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use bastion_mc::{
    bedrock::signin::{
        Bridge, REDIRECT_URI, SANDBOX_RUN_DIR, SANDBOX_STUB, START_PREFIX,
        server::{self, Options},
    },
    events::{Answer, AnswerKind, Delivery, Event, Reporter, SignInState},
};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");
const CODE: &str = "M.C507_BL2.2.U.0e5b2bbb-1b34-4e1b-9d29-2f7c0a4f9d4b";

fn start_url(state: &str) -> String {
    format!(
        "{START_PREFIX}lw=1&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256\
         &state={state}&client_id=0000000048183522&response_type=code\
         &scope=service%3A%3Auser.auth.xboxlive.com%3A%3AMBI_SSL&redirect_uri=ms-xal-0000000048183522%3A%2F%2Fauth"
    )
}

/// A runtime directory of the launch's kind: private, short (socket paths are limited to 108
/// bytes), holding nothing but the socket.
struct RunDir(PathBuf);

impl RunDir {
    fn new(tag: &str) -> RunDir {
        use std::os::unix::fs::DirBuilderExt;
        let dir = std::env::temp_dir().join(format!("bmc-stub-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        RunDir(dir)
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `bwrap … /bastion/bin/mcpelauncher-webview <start> <end>` with the socket directory bound
/// read-only, as the launch package mounts it.
fn stub_command(run_dir: &Path, start: &str, seccomp_fd: Option<i32>) -> Command {
    let mut command = Command::new("bwrap");
    command.args(["--unshare-all", "--die-with-parent", "--new-session", "--clearenv"]);
    command.args(["--ro-bind", "/usr", "/usr", "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib", "/lib64"]);
    command.args(["--symlink", "usr/bin", "/bin", "--proc", "/proc", "--dev", "/dev"]);
    command.arg("--ro-bind").arg(LAUNCHER).arg(SANDBOX_STUB);
    command.arg("--ro-bind").arg(run_dir).arg(SANDBOX_RUN_DIR);
    if let Some(fd) = seccomp_fd {
        // Without a reaper of bubblewrap's own as pid 1, the filter applies to the stub alone.
        command.args(["--as-pid-1", "--seccomp", &fd.to_string()]);
    }
    command.args([SANDBOX_STUB, start, REDIRECT_URI]);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

struct Session {
    run_dir: RunDir,
    reporter: Reporter,
    runtime: tokio::runtime::Runtime,
    serving: Option<server::Serving>,
}

impl Session {
    fn start(tag: &str) -> Session {
        let run_dir = RunDir::new(tag);
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let reporter = Reporter::capture();
        let bridge = Bridge::prepare(&run_dir.0).unwrap();
        let log: server::Log = Arc::new(|_: &str| {});
        let serving = {
            let _guard = runtime.enter();
            server::serve_with(bridge.into_listener(), &reporter, log, Options::default()).unwrap()
        };
        Session { run_dir, reporter, runtime, serving: Some(serving) }
    }

    /// The `n`th prompt event, if it has been sent.
    fn prompt(&self, n: usize) -> Option<bastion_mc::events::SignInPage> {
        self.reporter
            .captured()
            .into_iter()
            .filter_map(|e| if let Event::SignInPage(p) = e { Some(p) } else { None })
            .nth(n - 1)
    }

    fn wait_for_prompt(&self, n: usize) -> bastion_mc::events::SignInPage {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(page) = self.prompt(n) {
                return page;
            }
            assert!(Instant::now() < deadline, "no prompt: {:?}", self.reporter.captured());
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(serving) = self.serving.take() {
            self.runtime.block_on(serving.shutdown());
        }
    }
}

fn finish(child: std::process::Child) -> Output {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    rx.recv_timeout(Duration::from_secs(30)).expect("the stub ended").unwrap()
}

#[test]
fn the_launcher_binary_answers_as_mcpelauncher_webview_inside_bubblewrap() {
    let session = Session::start("ok");
    let child = stub_command(&session.run_dir.0, &start_url("st4te"), None).spawn().expect("bwrap");
    let page = session.wait_for_prompt(1);
    assert_eq!((page.state, page.url.as_str()), (SignInState::Waiting, start_url("st4te").as_str()));
    let pasted = format!("{REDIRECT_URI}?code={CODE}&state=st4te&lc=1033");
    assert_eq!(session.reporter.answer_for_test(AnswerKind::SignIn, Answer::Text(pasted)), Delivery::Delivered);
    let output = finish(child);
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(output.status.code(), Some(0));
    // One line, exactly what the client checks: the prefix, then only URL characters.
    assert_eq!(String::from_utf8(output.stdout).unwrap(), format!("{REDIRECT_URI}?code={CODE}&state=st4te\n"));
    assert_eq!(session.wait_for_prompt(2).state, SignInState::Done);
}

#[test]
fn a_page_bastion_does_not_show_leaves_the_stub_silent_on_standard_output() {
    let session = Session::start("refused");
    let evil = start_url("s").replace("scope=service%3A%3Auser.auth", "scope=service%3A%3Aoutlook.office");
    let output = finish(stub_command(&session.run_dir.0, &evil, None).spawn().expect("bwrap"));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "nothing for the client to take as an address");
    let err = String::from_utf8(output.stderr).unwrap();
    assert!(
        err.starts_with("mcpelauncher-webview (bastion-mc): refused by bastion-mc:") && err.contains("scope"),
        "{err}"
    );
    assert!(!session.reporter.captured().iter().any(|e| matches!(e, Event::SignInPage(_))), "nothing was shown");
}

#[test]
fn killing_the_stub_withdraws_the_prompt() {
    let session = Session::start("killed");
    let mut child = stub_command(&session.run_dir.0, &start_url("s"), None).spawn().expect("bwrap");
    session.wait_for_prompt(1);
    child.kill().unwrap();
    let _ = child.wait();
    let ended = session.wait_for_prompt(2);
    assert_eq!(ended.state, SignInState::Cancelled);
}

/// The system calls the stub's module header lists — with those a dynamically linked Rust
/// program needs to start and exit — and nothing else: every other call fails with `ENOSYS`,
/// as under the game's allowlist. If the stub needed one more, it could not complete.
#[test]
fn the_stub_needs_no_system_call_beyond_its_documented_list() {
    use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};
    // The list the Bedrock client's filter is held to (`stub::SYSCALLS`), and nothing else.
    let allowed: Vec<i64> = bastion_mc::bedrock::signin::stub::SYSCALLS
        .iter()
        .map(|name| bastion_mc::sandbox::seccomp::syscall_number(name).unwrap_or_else(|| panic!("{name}")))
        .collect();
    let expected: &[i64] =
        &[libc::SYS_execve, libc::SYS_socket, libc::SYS_connect, libc::SYS_recvfrom, libc::SYS_exit_group];
    assert!(expected.iter().all(|nr| allowed.contains(nr)) && allowed.len() == 30, "{allowed:?}");
    let rules = allowed.iter().map(|nr| (*nr, Vec::new())).collect();
    let filter =
        SeccompFilter::new(rules, SeccompAction::Errno(libc::ENOSYS as u32), SeccompAction::Allow, TargetArch::x86_64)
            .unwrap();
    let program: BpfProgram = filter.try_into().unwrap();
    let bytes: Vec<u8> = program
        .iter()
        .flat_map(|i| {
            let mut b = Vec::with_capacity(8);
            b.extend_from_slice(&i.code.to_ne_bytes());
            b.push(i.jt);
            b.push(i.jf);
            b.extend_from_slice(&i.k.to_ne_bytes());
            b
        })
        .collect();
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(&bytes).unwrap();
    drop(writer);

    let session = Session::start("seccomp");
    let fd = reader.as_raw_fd();
    let mut command = stub_command(&session.run_dir.0, &start_url("s3"), Some(fd));
    // SAFETY: between fork and exec only fcntl(2) is called, which is async-signal-safe; it
    // clears close-on-exec on the one descriptor bubblewrap is told to read the filter from.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().expect("bwrap");
    drop(reader);
    let deadline = Instant::now() + Duration::from_secs(20);
    let page = loop {
        if let Some(page) = session.prompt(1) {
            break page;
        }
        if let Some(status) = child.try_wait().unwrap() {
            let out = finish(child);
            panic!("the stub ended before its request arrived ({status}): {}", String::from_utf8_lossy(&out.stderr));
        }
        assert!(Instant::now() < deadline, "no prompt");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(page.state, SignInState::Waiting);
    let pasted = format!("{REDIRECT_URI}?code={CODE}&state=s3");
    assert_eq!(session.reporter.answer_for_test(AnswerKind::SignIn, Answer::Text(pasted)), Delivery::Delivered);
    let out = finish(child);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{REDIRECT_URI}?code={CODE}&state=s3\n"));
}
