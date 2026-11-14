//! The nested compositor, for real: gamescope in its own sandbox on this X11 session, and
//! clients in a game-shaped sandbox talking to its inner display.
//!
//! Opt-in: `BASTION_NESTED_TESTS=1`, on an X11 session with a GPU. It opens a window for a
//! few seconds. If gamescope is not installed, `BASTION_NESTED_LAYERS` can name directories
//! (colon-separated, each the `usr` of an extracted package) to layer over `/usr` inside
//! the compositor's sandbox.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use bastion_mc::{
    config::{AudioPref, DisplayPref, IsolationMode, SandboxConfig},
    paths::Paths,
    sandbox::{
        self, BaseOptions, Network, Resolv, SandboxSpec, display, nested,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
};
use tokio::io::AsyncReadExt;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");
/// One compositor window at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn enabled() -> bool {
    if std::env::var_os("BASTION_NESTED_TESTS").is_none() {
        eprintln!("skipped: set BASTION_NESTED_TESTS=1 (X11 session, GPU, gamescope or BASTION_NESTED_LAYERS)");
        return false;
    }
    true
}

fn config() -> SandboxConfig {
    let layers = std::env::var_os("BASTION_NESTED_LAYERS")
        .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
        .unwrap_or_default();
    SandboxConfig {
        display: DisplayPref::X11,
        audio: AudioPref::None,
        nested_compositor: IsolationMode::Require,
        nested_compositor_args: ["-W", "640", "-H", "360"].map(String::from).to_vec(),
        nested_compositor_layers: layers,
        ..Default::default()
    }
}

/// State directories under `target/`, never the user's own.
fn paths(tag: &str) -> Paths {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("nested-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = Paths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        state: root.join("state"),
        runtime: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    };
    paths.ensure().unwrap();
    paths
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Run one program the way the game is run — same base system, same Landlock and seccomp —
/// with the nested session's display attached.
async fn client(session: &nested::Session, cfg: &SandboxConfig, paths: &Paths, program: &str, argv: &[&str]) -> Output {
    client_in(session, cfg, paths, program, argv, Network::None).await
}

async fn client_in(
    session: &nested::Session,
    cfg: &SandboxConfig,
    paths: &Paths,
    program: &str,
    argv: &[&str],
    network: Network,
) -> Output {
    let exec = std::fs::canonicalize(program).unwrap_or_else(|e| panic!("{program}: {e}"));
    // The game's share of the desktop: the GPU. Its display is the nested session's.
    let desktop = display::plan(cfg, paths, "nested-test").expect("display plan");
    assert!(desktop.env.iter().all(|(k, _)| k != "DISPLAY" && k != "XAUTHORITY"), "nothing of the session's display");
    assert!(desktop.sockets.is_empty(), "no socket of the session: {:?}", desktop.sockets);
    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: std::slice::from_ref(&exec),
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })
    .unwrap();
    base.mounts.extend(desktop.mounts);
    base.landlock.devices.extend(desktop.devices);
    base.landlock.tcp = TcpPolicy::Ports(vec![]);
    let mut env = base.env;
    env.extend(desktop.env);
    session.attach(&mut base.mounts, &mut base.landlock.unix_sockets, &mut env);
    let spec = SandboxSpec {
        mounts: base.mounts,
        network,
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec,
            argv: argv.iter().map(|a| a.to_string()).collect(),
            env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    let mut out = running.child.stdout.take().unwrap();
    let mut err = running.child.stderr.take().unwrap();
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut stdout), err.read_to_string(&mut stderr));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("client timed out");
    Output { ok: status.success(), stdout, stderr }
}

const INSIDE: &str = r#"
import os, socket
print("x11_dir=" + ",".join(sorted(os.listdir("/tmp/.X11-unix"))))
print("runtime=" + ",".join(sorted(os.listdir(os.environ["XDG_RUNTIME_DIR"]))))
print("display=" + os.environ.get("DISPLAY", ""))
print("xauthority=" + os.environ.get("XAUTHORITY", ""))
def reach(address):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect(address)
        return "connected"
    except OSError as e:
        return type(e).__name__
    finally:
        s.close()
host = "@HOST@"
print("session_socket_file=" + reach("/tmp/.X11-unix/X" + host))
print("session_socket_abstract=" + reach("\0/tmp/.X11-unix/X" + host))
"#;

#[tokio::test(flavor = "multi_thread")]
async fn the_game_talks_to_a_display_server_of_its_own() {
    if !enabled() {
        return;
    }
    let _serial = SERIAL.lock().await;
    let cfg = config();
    let paths = paths("own");
    let host_display = std::env::var("DISPLAY").expect("an X11 session");
    let host_number: String = host_display.trim_start_matches(':').split('.').next().unwrap().into();

    let mut desktop = display::plan(&cfg, &paths, "nested-test").expect("display plan");
    let plan = desktop.nested.take().expect("on X11 with gamescope available, the plan is a nested compositor");
    assert!(!desktop.uses_x11, "the game is not a client of the session's X server");
    assert!(desktop.notes.iter().any(|n| n.contains("nested compositor")), "{:?}", desktop.notes);
    let dir = plan.dir.clone();
    assert!(dir.starts_with(&paths.cache));

    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink: nested::Sink = {
        let log = log.clone();
        Arc::new(move |line| log.lock().unwrap().push(line))
    };
    let started = nested::start(plan, Path::new(LAUNCHER), sink).await;
    let mut session = match started {
        Ok(session) => session,
        Err(e) => panic!("{e}\n--- compositor output ---\n{}", log.lock().unwrap().join("\n")),
    };
    println!("inner display :{}", session.display);
    assert_eq!(
        session.clipboard,
        nested::ClipboardCut::Active,
        "the cut-off library reported from inside this compositor that the bridge calls land in it"
    );
    assert_ne!(session.display.to_string(), host_number, "a display of its own, not the session's");
    assert!(dir.join(format!("X{}", session.display)).exists());
    // The inner display's socket lives in the compositor's private directory. (Whether the
    // same *number* exists in the host's `/tmp/.X11-unix` says nothing: when this test runs
    // on a private display, the inner `:0` and an unrelated host `:0` can coexist.)
    assert!(!dir.starts_with("/tmp/.X11-unix"));

    // 1. What the game's sandbox contains.
    let script = INSIDE.replace("@HOST@", &host_number);
    let inside = client(&session, &cfg, &paths, "/usr/bin/python3", &["python3", "-I", "-c", &script]).await;
    assert!(inside.ok, "{}\n{}", inside.stdout, inside.stderr);
    let line = |key: &str| {
        inside
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or_else(|| panic!("no {key} in {}", inside.stdout))
    };
    assert_eq!(line("x11_dir="), format!("X{}", session.display), "one display socket: the inner one");
    assert_eq!(line("runtime="), "", "nothing of the compositor's or the session's runtime directory");
    assert_eq!(
        line("display="),
        format!("/tmp/.X11-unix/X{}", session.display),
        "a socket path: no abstract socket is tried"
    );
    assert_eq!(line("xauthority="), "", "no cookie of the session");
    assert_ne!(line("session_socket_file="), "connected");
    assert_ne!(line("session_socket_abstract="), "connected");

    // 2. A real X client: the root window it sees is gamescope's, not the session's.
    let root = client(&session, &cfg, &paths, "/usr/bin/xprop", &["xprop", "-root"]).await;
    assert!(root.ok, "xprop failed:\n{}\n{}", root.stdout, root.stderr);
    assert!(root.stdout.contains("GAMESCOPE_"), "not gamescope's root window:\n{}", root.stdout);
    // A marker on the *session's* root window: every client of the session's X server can
    // read it (that is X11), and the game cannot, because it is not one.
    let marker = format!("mark-{}", std::process::id());
    let xprop = |args: &[&str]| {
        let out = std::process::Command::new("/usr/bin/xprop").args(args).output().expect("xprop on the session");
        assert!(out.status.success(), "xprop {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    xprop(&["-root", "-f", "BASTION_MC_TEST", "8s", "-set", "BASTION_MC_TEST", &marker]);
    let session_root = xprop(&["-root"]);
    let game_root = client(&session, &cfg, &paths, "/usr/bin/xprop", &["xprop", "-root"]).await;
    xprop(&["-root", "-remove", "BASTION_MC_TEST"]);
    assert!(session_root.contains(&marker), "the marker is visible to clients of the session's server");
    assert!(!session_root.contains("GAMESCOPE_"), "the comparison needs a session that is not itself gamescope");
    assert!(game_root.ok && game_root.stdout.contains("GAMESCOPE_"));
    assert!(!game_root.stdout.contains(&marker), "the game read a property of the session's root window");
    assert!(!game_root.stdout.contains("_NET_CLIENT_LIST"), "the game can enumerate windows:\n{}", game_root.stdout);

    // 2b. The same with the host's network namespace (`network.mode = "open"`). There the
    // abstract socket `@/tmp/.X11-unix/X<n>` that libxcb tries first can belong to a
    // display of the *session* with the same number; the game must still end up on the
    // inner display, and only there.
    let open = client_in(&session, &cfg, &paths, "/usr/bin/xprop", &["xprop", "-root"], Network::Open).await;
    assert!(open.ok, "xprop with the host network failed:\n{}\n{}", open.stdout, open.stderr);
    assert!(open.stdout.contains("GAMESCOPE_"), "not gamescope's root window:\n{}", open.stdout);

    // 3. Hardware rendering through the inner display.
    if Path::new("/usr/bin/glxinfo").exists() {
        let glx = client(&session, &cfg, &paths, "/usr/bin/glxinfo", &["glxinfo", "-B"]).await;
        assert!(glx.ok, "glxinfo failed:\n{}\n{}", glx.stdout, glx.stderr);
        let renderer = glx.stdout.lines().find(|l| l.contains("OpenGL renderer string")).unwrap_or("").to_string();
        println!("{}", renderer.trim());
        assert!(glx.stdout.contains("direct rendering: Yes"), "{}", glx.stdout);
        assert!(!renderer.contains("llvmpipe"), "software rendering: {renderer}");
    }

    // 4. The compositor is alive until told otherwise, and leaves nothing behind.
    assert!(tokio::time::timeout(Duration::from_millis(300), session.exited()).await.is_err(), "still running");
    session.stop().await;
    assert!(!dir.exists(), "the shared directory is removed with the session");
    let log = log.lock().unwrap();
    assert!(log.iter().any(|l| l.contains("Starting Xwayland")), "compositor output was relayed: {log:?}");
    assert!(log.iter().all(|l| !l.contains('\u{1b}')), "no terminal escapes in the log");
    let _ = std::fs::remove_dir_all(paths.cache.parent().unwrap());
}

/// Closing the compositor's window (here: it is killed) is noticed.
#[tokio::test(flavor = "multi_thread")]
async fn a_compositor_that_goes_away_is_noticed() {
    if !enabled() {
        return;
    }
    let _serial = SERIAL.lock().await;
    let cfg = config();
    let paths = paths("gone");
    let mut desktop = display::plan(&cfg, &paths, "nested-test").expect("display plan");
    let plan = desktop.nested.take().expect("nested plan");
    let dir = plan.dir.clone();
    let mut session = nested::start(plan, Path::new(LAUNCHER), Arc::new(|_| {})).await.expect("start");
    // Stand-in for "its window was closed" or "it crashed": the compositor's sandbox is
    // killed from outside.
    let pid = session.process_id().expect("the compositor's process") as i32;
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL).unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), session.exited()).await.expect("exit was noticed");
    assert!(!status.unwrap().success());
    drop(session);
    assert!(!dir.exists());
    let _ = std::fs::remove_dir_all(paths.cache.parent().unwrap());
}

/// An X11 client in Python (ctypes + libX11) that either owns a selection (CLIPBOARD or
/// PRIMARY) with a given text for a while (`own <selection> <text> <seconds>`) or reads it
/// (`read <selection> <seconds>`).
const SELECTION: &str = r#"
import ctypes, sys, time
from ctypes import POINTER, byref, c_char_p, c_int, c_long, c_uint, c_ulong, c_void_p
X = ctypes.CDLL("libX11.so.6")
def fn(name, result, *args):
    f = getattr(X, name)
    f.restype, f.argtypes = result, list(args)
    return f
XOpenDisplay = fn("XOpenDisplay", c_void_p, c_char_p)
XDefaultRootWindow = fn("XDefaultRootWindow", c_ulong, c_void_p)
XCreateSimpleWindow = fn("XCreateSimpleWindow", c_ulong, c_void_p, c_ulong, c_int, c_int, c_uint, c_uint, c_uint, c_ulong, c_ulong)
XInternAtom = fn("XInternAtom", c_ulong, c_void_p, c_char_p, c_int)
XSetSelectionOwner = fn("XSetSelectionOwner", c_int, c_void_p, c_ulong, c_ulong, c_ulong)
XGetSelectionOwner = fn("XGetSelectionOwner", c_ulong, c_void_p, c_ulong)
XConvertSelection = fn("XConvertSelection", c_int, c_void_p, c_ulong, c_ulong, c_ulong, c_ulong, c_ulong)
XNextEvent = fn("XNextEvent", c_int, c_void_p, c_void_p)
XPending = fn("XPending", c_int, c_void_p)
XChangeProperty = fn("XChangeProperty", c_int, c_void_p, c_ulong, c_ulong, c_ulong, c_int, c_int, c_void_p, c_int)
XSendEvent = fn("XSendEvent", c_int, c_void_p, c_ulong, c_int, c_long, c_void_p)
XFlush = fn("XFlush", c_int, c_void_p)
XGetWindowProperty = fn("XGetWindowProperty", c_int, c_void_p, c_ulong, c_ulong, c_long, c_long, c_int, c_ulong,
                        POINTER(c_ulong), POINTER(c_int), POINTER(c_ulong), POINTER(c_ulong), POINTER(c_void_p))
COMMON = [("type", c_int), ("serial", c_ulong), ("send_event", c_int), ("display", c_void_p)]
class Request(ctypes.Structure):
    _fields_ = COMMON + [(n, c_ulong) for n in ("owner", "requestor", "selection", "target", "property", "time")]
class Notify(ctypes.Structure):
    _fields_ = COMMON + [(n, c_ulong) for n in ("requestor", "selection", "target", "property", "time")]

d = XOpenDisplay(None)
if not d:
    print("display=unreachable")
    sys.exit(1)
w = XCreateSimpleWindow(d, XDefaultRootWindow(d), 0, 0, 1, 1, 0, 0, 0)
atom = lambda name: XInternAtom(d, name, 0)
CLIPBOARD, UTF8, TARGETS, PROP = atom(sys.argv[2].encode()), atom(b"UTF8_STRING"), atom(b"TARGETS"), atom(b"BASTION_PROBE")
TEXT = [UTF8, 31, atom(b"TEXT"), atom(b"text/plain;charset=utf-8"), atom(b"text/plain")]
event = ctypes.create_string_buffer(192)

def pump(seconds, handle):
    end = time.time() + seconds
    while time.time() < end:
        while XPending(d):
            XNextEvent(d, event)
            if handle(c_int.from_buffer(event).value):
                return True
        time.sleep(0.02)
    return False

if sys.argv[1] == "own":
    data = sys.argv[3].encode()
    XSetSelectionOwner(d, CLIPBOARD, w, 0)
    XFlush(d)
    print("owning=" + str(XGetSelectionOwner(d, CLIPBOARD) == w), flush=True)
    def serve(kind):
        if kind != 30:  # SelectionRequest
            return False
        r = Request.from_buffer_copy(event)
        prop = r.property or r.target
        if r.target == TARGETS:
            offered = (c_ulong * (len(TEXT) + 1))(TARGETS, *TEXT)
            XChangeProperty(d, r.requestor, prop, 4, 32, 0, offered, len(offered))
        elif r.target in TEXT:
            XChangeProperty(d, r.requestor, prop, r.target, 8, 0, data, len(data))
        else:
            prop = 0
        answer = ctypes.create_string_buffer(192)
        n = Notify(31, 0, 1, d, r.requestor, r.selection, r.target, prop, r.time)
        ctypes.memmove(answer, byref(n), ctypes.sizeof(n))
        XSendEvent(d, r.requestor, 0, 0, answer)
        XFlush(d)
        return False
    pump(float(sys.argv[4]), serve)
else:
    found, owner, end = "", 0, time.time() + float(sys.argv[3])
    while time.time() < end and not found:
        owner = XGetSelectionOwner(d, CLIPBOARD)
        if owner:
            XConvertSelection(d, CLIPBOARD, UTF8, PROP, w, 0)
            XFlush(d)
            got = []
            def notified(kind):
                if kind != 31:  # SelectionNotify
                    return False
                got.append(Notify.from_buffer_copy(event).property)
                return True
            if pump(2, notified) and got[0]:
                kind, fmt, count, rest, ptr = c_ulong(), c_int(), c_ulong(), c_ulong(), c_void_p()
                XGetWindowProperty(d, w, PROP, 0, 4096, 1, 0, byref(kind), byref(fmt), byref(count), byref(rest), byref(ptr))
                if ptr.value:
                    found = ctypes.string_at(ptr.value, count.value).decode("utf-8", "replace")
        if not found:
            time.sleep(0.3)
    print("clipboard_owner=" + ("yes" if owner else "no"))
    print("clipboard_text=" + found)
"#;

fn value<'a>(output: &'a str, key: &str) -> &'a str {
    output.lines().find_map(|l| l.strip_prefix(key)).unwrap_or_else(|| panic!("no {key} in:\n{output}"))
}

/// What crosses the nested compositor besides pixels and input: the selections.
///
/// gamescope bridges CLIPBOARD and PRIMARY between the session and its inner display, in
/// both directions; the launcher preloads `native/noclip.c` into it to cut that. This test
/// checks that neither selection crosses in either direction. It takes over the selections
/// of the display it runs against, so it has its own switch
/// (`BASTION_NESTED_CLIPBOARD_TEST=1`) and belongs on a throw-away display.
#[tokio::test(flavor = "multi_thread")]
async fn no_selection_crosses_the_nested_compositor() {
    if !enabled() {
        return;
    }
    if std::env::var_os("BASTION_NESTED_CLIPBOARD_TEST").is_none() {
        eprintln!("skipped: set BASTION_NESTED_CLIPBOARD_TEST=1 (replaces the display's selection owners)");
        return;
    }
    let _serial = SERIAL.lock().await;
    let cfg = config();
    let paths = paths("clip");
    let mut desktop = display::plan(&cfg, &paths, "nested-test").expect("display plan");
    let plan = desktop.nested.take().expect("nested plan");
    let session = nested::start(plan, Path::new(LAUNCHER), Arc::new(|_| {})).await.expect("start");
    assert_eq!(session.clipboard, nested::ClipboardCut::Active);
    let crossed = selections_that_cross(&session, &cfg, &paths, &["CLIPBOARD", "PRIMARY"]).await;
    session.stop().await;
    let _ = std::fs::remove_dir_all(paths.cache.parent().unwrap());
    assert!(crossed.is_empty(), "selections crossed the nested compositor: {crossed:?}");
}

/// The launcher must know whether the cut-off is in the compositor it started, not assume
/// it. Started without the library (which is what a preload that the dynamic linker skips
/// amounts to), the same compositor says nothing — and then the launcher does not claim a
/// cut clipboard, "require" refuses to go on, and the claim would indeed have been false:
/// the session's clipboard reaches the game.
#[tokio::test(flavor = "multi_thread")]
async fn a_compositor_without_the_cut_off_is_noticed_and_not_called_cut() {
    if !enabled() {
        return;
    }
    let _serial = SERIAL.lock().await;
    let cfg = config();
    let paths = paths("nocut");
    let plan_without = |require: bool| {
        let mut desktop = display::plan(&cfg, &paths, "nested-test").expect("display plan");
        let mut plan = desktop.nested.take().expect("nested plan");
        assert!(plan.require_clipboard_cut, "nested_compositor = \"require\" asks for the cut as well");
        plan.preload_clipboard_cut = false;
        plan.require_clipboard_cut = require;
        plan
    };

    // "require": the launch stops, and the compositor with it.
    let refused = match nested::start(plan_without(true), Path::new(LAUNCHER), Arc::new(|_| {})).await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a compositor without the cut-off was accepted under \"require\""),
    };
    assert!(refused.contains("not established") && refused.contains("was not loaded"), "{refused}");

    // "auto": it runs, and the session knows what it does not know.
    let session = nested::start(plan_without(false), Path::new(LAUNCHER), Arc::new(|_| {})).await.expect("start");
    match &session.clipboard {
        nested::ClipboardCut::Unconfirmed(why) => assert!(why.contains("was not loaded"), "{why}"),
        nested::ClipboardCut::Active => panic!("reported as cut without the library"),
    }
    if std::env::var_os("BASTION_NESTED_CLIPBOARD_TEST").is_some() {
        let crossed = selections_that_cross(&session, &cfg, &paths, &["CLIPBOARD"]).await;
        println!("without the cut-off, crossed: {crossed:?}");
        assert!(
            crossed.iter().any(|c| c.contains("session → game")),
            "the control run: without the library gamescope bridges the clipboard ({crossed:?})"
        );
    }
    session.stop().await;
    let _ = std::fs::remove_dir_all(paths.cache.parent().unwrap());
}

/// Which of `selections` cross the compositor of `session`, and in which direction.
async fn selections_that_cross(
    session: &nested::Session,
    cfg: &SandboxConfig,
    paths: &Paths,
    selections: &[&str],
) -> Vec<String> {
    let python = ["python3", "-I", "-c", SELECTION];
    let mut crossed = Vec::new();

    for selection in selections.iter().copied() {
        // 1. Session → game: something is copied in the session while the game runs.
        let secret = format!("copied-in-the-session-{selection}-{}", std::process::id());
        let mut owner = std::process::Command::new("/usr/bin/python3")
            .args(["-I", "-c", SELECTION, "own", selection, &secret, "12"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("selection owner on the session");
        {
            use std::io::BufRead;
            let mut line = String::new();
            std::io::BufReader::new(owner.stdout.take().unwrap()).read_line(&mut line).unwrap();
            assert_eq!(line.trim(), "owning=True", "could not take the session's {selection}");
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let mut argv = python.to_vec();
        argv.extend(["read", selection, "6"]);
        let seen = client(session, cfg, paths, "/usr/bin/python3", &argv).await;
        let _ = owner.kill();
        let _ = owner.wait();
        assert!(seen.ok, "{}\n{}", seen.stdout, seen.stderr);
        let into_game = value(&seen.stdout, "clipboard_text=").to_string();
        println!(
            "{selection} session → game: owner in the inner display: {}, text read by the game: {into_game:?}",
            value(&seen.stdout, "clipboard_owner=")
        );
        if into_game.contains(&secret) {
            crossed.push(format!("{selection}: session → game"));
        }

        // 2. Game → session: the game puts something on its display's selection.
        let planted = format!("planted-by-the-game-{selection}-{}", std::process::id());
        let mut argv = python.to_vec();
        argv.extend(["own", selection, &planted, "8"]);
        let (inside, outside) = tokio::join!(client(session, cfg, paths, "/usr/bin/python3", &argv), async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            tokio::process::Command::new("/usr/bin/python3")
                .args(["-I", "-c", SELECTION, "read", selection, "4"])
                .output()
                .await
                .expect("selection reader on the session")
        });
        assert!(inside.ok, "{}\n{}", inside.stdout, inside.stderr);
        assert_eq!(value(&inside.stdout, "owning="), "True", "the game could not own its own {selection}");
        let outside = String::from_utf8_lossy(&outside.stdout).into_owned();
        let into_session = value(&outside, "clipboard_text=").to_string();
        println!("{selection} game → session: text read in the session: {into_session:?}");
        if into_session.contains(&planted) {
            crossed.push(format!("{selection}: game → session"));
        }
    }
    crossed
}
