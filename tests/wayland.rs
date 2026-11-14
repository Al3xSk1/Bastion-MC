//! The security-context setup against a real Wayland compositor.
//!
//! Opt-in, because it needs one: `BASTION_WAYLAND_COMPOSITOR=/run/user/1000/wayland-1`.
//! (The unit tests in `src/sandbox/wayland.rs` cover the wire format against a stand-in.)

use std::{
    collections::BTreeSet,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use bastion_mc::{
    config::{AudioPref, DisplayPref, IsolationMode, SandboxConfig},
    paths::Paths,
    sandbox::{
        self, BaseOptions, Network, Resolv, SandboxSpec, display,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
        wayland::{self, Setup},
    },
};
use tokio::io::AsyncReadExt;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

/// A Wayland client in two dozen lines: list what the compositor offers on the socket the
/// environment names, after giving the launcher a moment to remove that socket's name.
const CLIENT: &str = r#"
import os, socket, struct, time
time.sleep(1.5)
path = os.path.join(os.environ["XDG_RUNTIME_DIR"], os.environ["WAYLAND_DISPLAY"])
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(10)
s.connect(path)
def msg(obj, op, payload=b""):
    return struct.pack("=II", obj, ((8 + len(payload)) << 16) | op) + payload
s.sendall(msg(1, 1, struct.pack("=I", 2)) + msg(1, 0, struct.pack("=I", 3)))
buf, names, done = b"", [], False
while not done:
    chunk = s.recv(65536)
    if not chunk:
        break
    buf += chunk
    while len(buf) >= 8:
        obj, word = struct.unpack_from("=II", buf)
        size, op = word >> 16, word & 0xffff
        if len(buf) < size:
            break
        body, buf = buf[8:size], buf[size:]
        if obj == 2 and op == 0:
            n = struct.unpack_from("=I", body, 4)[0]
            names.append(body[8:8 + n - 1].decode())
        elif obj == 3 and op == 0:
            done = True
print("globals=" + ",".join(sorted(names)))
print("runtime=" + ",".join(sorted(os.listdir(os.environ["XDG_RUNTIME_DIR"]))))
"#;

const MANAGER: &str = "wp_security_context_manager_v1";

/// Interfaces that give a client power over other clients or the session. A compositor
/// that implements security contexts is expected to keep whichever of these it has away
/// from a sandboxed client — and the launcher checks exactly this list at every launch.
const PRIVILEGED: &[&str] = wayland::PRIVILEGED;

#[test]
fn a_real_compositor_withholds_its_privileged_interfaces_from_the_sandbox_socket() {
    let Some(compositor) = std::env::var_os("BASTION_WAYLAND_COMPOSITOR").map(PathBuf::from) else {
        eprintln!("skipped: set BASTION_WAYLAND_COMPOSITOR to a compositor's socket");
        return;
    };
    let names = |socket: &std::path::Path| -> BTreeSet<String> {
        wayland::globals(socket).expect("listing globals").into_iter().map(|g| g.interface).collect()
    };
    let trusted = names(&compositor);
    assert!(trusted.contains(MANAGER), "this compositor does not implement security contexts: {trusted:?}");

    let dir = std::env::temp_dir().join(format!("bastion-wl-it-{}", std::process::id()));
    let Setup::Secured(context, offered) = wayland::secure(&compositor, &dir, "integration-test").expect("setup")
    else {
        panic!("the manager is advertised but the setup reported no support")
    };
    // What the launcher itself saw on the game's socket while setting it up…
    let offered = offered.expect("the launcher listed the game's socket");
    println!("the launcher's own look: {}", offered.describe());
    assert!(offered.privileged.is_empty(), "{:?}", offered.privileged);
    assert!(!offered.manager);
    // …and the same thing, asked again from here.
    let sandboxed = names(&context.socket);
    assert_eq!(offered.interfaces, sandboxed.iter().cloned().collect::<Vec<_>>());
    assert_eq!(offered.withheld, trusted.difference(&sandboxed).count());

    let withheld: Vec<&String> = trusted.difference(&sandboxed).collect();
    println!("offered to a trusted client: {}", trusted.len());
    println!("offered through the security context: {}", sandboxed.len());
    println!("withheld: {withheld:?}");
    println!("still offered: {sandboxed:?}");

    assert!(sandboxed.is_subset(&trusted), "a sandboxed client is never offered more");
    assert!(!sandboxed.contains(MANAGER), "a sandboxed client cannot create further contexts");
    let leaked: Vec<&&str> = PRIVILEGED.iter().filter(|name| sandboxed.contains(**name)).collect();
    assert!(leaked.is_empty(), "privileged interfaces offered to the sandbox: {leaked:?}");
    let had: Vec<&&str> = PRIVILEGED.iter().filter(|name| trusted.contains(**name)).collect();
    assert!(!had.is_empty(), "the comparison is vacuous: the compositor has none of the known privileged interfaces");
    // What a game needs is still there.
    for needed in ["wl_compositor", "wl_shm", "wl_seat", "wl_output", "xdg_wm_base"] {
        assert!(sandboxed.contains(needed), "{needed} is missing from the sandbox socket");
    }
    // …including mouse capture, where the compositor has it at all.
    for wanted in ["zwp_pointer_constraints_v1", "zwp_relative_pointer_manager_v1", "zwp_linux_dmabuf_v1"] {
        assert!(!trusted.contains(wanted) || sandboxed.contains(wanted), "{wanted} was withheld");
    }

    // The socket lives exactly as long as the launcher's end of the pipe.
    let socket = context.socket.clone();
    assert!(UnixStream::connect(&socket).is_ok());
    drop(context);
    let closed = (0..50).any(|_| {
        std::thread::sleep(Duration::from_millis(100));
        UnixStream::connect(&socket).is_err()
    });
    assert!(closed, "the compositor kept accepting after the launcher let go");
    let _ = std::fs::remove_dir_all(dir);
}

/// The whole path the launcher takes: plan the display on a Wayland session, start the real
/// sandbox with what the plan says, remove the socket's name once the sandbox has it, and
/// let a client inside ask the compositor what it may have.
///
/// Needs `WAYLAND_DISPLAY` to name a compositor that implements the protocol (an absolute
/// path is fine) and `BASTION_WAYLAND_TESTS=1`.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_client_is_offered_only_what_the_security_context_allows() {
    if std::env::var_os("BASTION_WAYLAND_TESTS").is_none() {
        eprintln!("skipped: set BASTION_WAYLAND_TESTS=1 and WAYLAND_DISPLAY to a compositor's socket");
        return;
    }
    let python = std::fs::canonicalize("/usr/bin/python3").expect("python3");
    let paths = Paths::from_env().unwrap();
    let cfg = SandboxConfig {
        display: DisplayPref::Wayland,
        audio: AudioPref::None,
        gpu: false,
        wayland_security_context: IsolationMode::Require,
        ..Default::default()
    };
    let mut desktop = display::plan(&cfg, &paths, "integration-test").expect("display plan");
    let context = desktop.wayland_context.take().expect("the plan created a security context");
    let session_socket = std::env::var("WAYLAND_DISPLAY").unwrap();
    assert!(
        desktop
            .mounts
            .iter()
            .all(|m| !matches!(m, sandbox::Mount::RoBind { src, .. } if src.to_str() == Some(session_socket.as_str()))),
        "the session's own socket must not be what the sandbox gets"
    );
    assert!(desktop.notes.iter().any(|n| n.contains("security context")), "{:?}", desktop.notes);
    // The plan does not say "secured" on the strength of the setup alone: it looked.
    let seen = desktop.wayland_isolation.clone().expect("a security context was set up");
    let seen = seen.expect("none of the known privileged interfaces is offered on the game's socket");
    println!("display plan: {seen}");
    assert!(seen.contains("interface(s) offered on the game's socket") && seen.contains("withheld"), "{seen}");

    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: std::slice::from_ref(&python),
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })
    .unwrap();
    base.mounts.extend(desktop.mounts);
    base.landlock.unix_sockets.extend(desktop.sockets);
    base.landlock.tcp = TcpPolicy::Ports(vec![]);
    let mut env = base.env;
    env.extend(desktop.env);
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::None,
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: python.clone(),
            argv: vec!["python3".into(), "-I".into(), "-c".into(), CLIENT.into()],
            env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    // As the launcher does: the name goes away as soon as the sandbox holds the socket.
    assert!(running.payload_pid().await.is_some(), "the sandbox started");
    let name = context.socket.clone();
    context.unlink();
    assert!(!name.exists());

    let mut out = running.child.stdout.take().unwrap();
    let mut err = running.child.stderr.take().unwrap();
    let (mut so, mut se) = (String::new(), String::new());
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut so), err.read_to_string(&mut se));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("timeout");
    assert!(status.success(), "client failed: {status:?}\n{so}\n{se}");
    let line = |key: &str| {
        so.lines().find_map(|l| l.strip_prefix(key)).unwrap_or_else(|| panic!("no {key} line in:\n{so}\n{se}"))
    };
    let offered: BTreeSet<&str> = line("globals=").split(',').collect();
    println!("offered inside the sandbox: {offered:?}");
    assert!(offered.contains("wl_compositor") && offered.contains("xdg_wm_base"), "{offered:?}");
    assert!(!offered.contains(MANAGER));
    let leaked: Vec<&&str> = PRIVILEGED.iter().filter(|name| offered.contains(**name)).collect();
    assert!(leaked.is_empty(), "privileged interfaces reached the sandbox: {leaked:?}");
    // One socket, nothing else of the session's runtime directory.
    assert_eq!(line("runtime="), "wayland-0");
    drop(context);
}
