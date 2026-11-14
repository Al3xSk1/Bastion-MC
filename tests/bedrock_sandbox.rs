//! The Bedrock client's system-call filter (`SeccompProfile::BedrockClient`), in the real
//! bubblewrap → init chain with a Python payload in place of the client — the profile's two
//! modes and the promise of the log mode: profiling the real client once must not open what
//! the filter is there to keep shut.
//!
//! Requirements as for `tests/sandbox.rs`: bubblewrap, python3, unprivileged user namespaces,
//! Landlock ABI >= 6. The log-mode cases make the kernel log a few `type=1326` lines.

use std::{net::Shutdown, path::Path, time::Duration};

use bastion_mc::{
    config::SeccompMode,
    sandbox::{
        self, BaseOptions, Network, Resolv, SandboxSpec,
        policy::{InitPolicy, SeccompPolicy, SeccompProfile, TcpPolicy},
    },
};
use tokio::io::AsyncReadExt;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

const PRELUDE: &str = r#"
import ctypes, errno, os
libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long
def sc(nr, *a):
    ctypes.set_errno(0)
    r = libc.syscall(ctypes.c_long(nr), *[ctypes.c_long(x) for x in a])
    return "ok" if r >= 0 else errno.errorcode.get(ctypes.get_errno(), str(ctypes.get_errno()))
def show(k, v): print(f"{k}={v}", flush=True)
"#;

/// What every case below asks: calls the client's allowlist does not have (`munlockall`,
/// harmless), the always-refused ones, and an ordinary allowed one.
const PROBES: &str = r#"
show("getpid", sc(39))
show("munlockall", sc(152))
show("keyctl", sc(250, 0, -4, 0))
show("add_key", sc(248, 0, 0, 0, 0, -3))
show("unshare_user", sc(272, 0x10000000))
show("unshare_net", sc(272, 0x40000000))
show("clone3", sc(435, 0, 0))
show("io_uring_setup", sc(425, 1, 0))
show("userfaultfd", sc(323, 0))
show("perf_event_open", sc(298, 0, 0, -1, -1, 0))
show("clone_newuser", sc(56, 0x10000000 | 17, 0, 0, 0, 0))
"#;

struct Outcome {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Outcome {
    fn line(&self, key: &str) -> &str {
        self.stdout.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('='))).unwrap_or_else(|| {
            panic!("no `{key}=` line.\n--- stdout ---\n{}\n--- stderr ---\n{}", self.stdout, self.stderr)
        })
    }
}

async fn run(seccomp: SeccompPolicy, body: &str) -> Outcome {
    let python = std::fs::canonicalize("/usr/bin/python3").expect("python3");
    let base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: std::slice::from_ref(&python),
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })
    .expect("base");
    let mut landlock = base.landlock;
    landlock.tcp = TcpPolicy::Ports(vec![]);
    landlock.udp_send = Some(vec![]);
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::None,
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: python,
            argv: vec!["python3".into(), "-I".into(), "-c".into(), format!("{PRELUDE}\n{body}")],
            env: base.env,
            landlock,
            seccomp,
        },
    };
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn sandbox");
    running.channel.shutdown(Shutdown::Both).unwrap();
    drop(std::mem::take(&mut running.egress));
    let mut out = running.child.stdout.take().unwrap();
    let mut err = running.child.stderr.take().unwrap();
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let work = async {
        let (a, b) = tokio::join!(out.read_to_string(&mut stdout), err.read_to_string(&mut stderr));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    };
    let status = tokio::time::timeout(Duration::from_secs(60), work).await.expect("payload timed out");
    Outcome { code: status.code(), stdout, stderr }
}

fn bedrock(mode: SeccompMode) -> SeccompPolicy {
    SeccompPolicy { mode, profile: SeccompProfile::BedrockClient, ..Default::default() }
}

fn requirements_met() -> bool {
    let ok = sandbox::check_bwrap().is_ok()
        && sandbox::landlock::kernel_abi() >= sandbox::landlock::MIN_KERNEL_ABI
        && Path::new("/usr/bin/python3").exists();
    if !ok {
        eprintln!("skipped: needs bubblewrap, python3 and Landlock ABI >= 6");
    }
    ok
}

const ALWAYS_REFUSED: [&str; 9] = [
    "keyctl",
    "add_key",
    "unshare_user",
    "unshare_net",
    "clone3",
    "io_uring_setup",
    "userfaultfd",
    "perf_event_open",
    "clone_newuser",
];

#[tokio::test(flavor = "multi_thread")]
async fn the_enforcing_client_profile_refuses_what_its_allowlist_lacks() {
    if !requirements_met() {
        return;
    }
    let o = run(bedrock(SeccompMode::Enforce), PROBES).await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.line("getpid"), "ok");
    assert_eq!(o.line("munlockall"), "ENOSYS", "not on the client's list (the JVM's has it)");
    for key in ALWAYS_REFUSED {
        assert_eq!(o.line(key), "ENOSYS", "{key}");
    }
    // The same probes under the JVM's profile: the filters really differ in the kernel.
    let jvm = run(SeccompPolicy::default(), PROBES).await;
    assert_eq!(jvm.code, Some(0), "stderr: {}", jvm.stderr);
    assert_eq!(jvm.line("munlockall"), "ok", "the JVM's list allows it");
    assert_eq!(jvm.line("keyctl"), "ENOSYS");
}

/// Log mode is for taking the real client's profile once. What lies outside the allowlist is
/// let through (and logged by the kernel), but the always-refused calls stay refused.
#[tokio::test(flavor = "multi_thread")]
async fn log_mode_lets_the_rest_through_but_keeps_refusing_namespaces_keyrings_and_io_uring() {
    if !requirements_met() {
        return;
    }
    let o = run(bedrock(SeccompMode::Log), PROBES).await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.line("getpid"), "ok");
    assert_eq!(o.line("munlockall"), "ok", "outside the allowlist: logged and allowed");
    for key in ALWAYS_REFUSED {
        assert_eq!(o.line(key), "ENOSYS", "{key} must stay refused in log mode");
    }
}

/// The tripwire is the JVM's, unchanged, and it is not switched off by log mode (the JVM's
/// log mode does switch it off).
#[tokio::test(flavor = "multi_thread")]
async fn the_tripwire_kills_the_client_in_both_modes() {
    if !requirements_met() {
        return;
    }
    for mode in [SeccompMode::Enforce, SeccompMode::Log] {
        for (name, call) in [("ptrace", "sc(101, 0, 0, 0, 0)"), ("setns", "sc(308, -1, 0)")] {
            let o = run(bedrock(mode), &format!("show('{name}', {call})\nshow('survived', 'yes')")).await;
            assert_eq!(o.code, Some(128 + libc::SIGSYS), "{mode:?}/{name}: stderr: {}", o.stderr);
            assert!(!o.stdout.contains("survived"), "{mode:?}/{name}: the process went on");
        }
    }
}
