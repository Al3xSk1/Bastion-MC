//! A stand-in for `mcpelauncher-client`, for `tests/bedrock_launch.rs`. Not a Cargo target:
//! the test compiles this one file with `rustc` (standard library only), so that the program
//! the sandbox runs is a native executable of its own — as the real client is — and not a
//! test harness that would choke on the client's arguments.
//!
//! It does what the real client does where the sandbox has to be right: it is started with
//! the client's arguments; it reads the game directory; it writes the data root the way the
//! game does (worlds, settings, sign-in tokens where Bedrock has kept them); it fetches HTTPS
//! with the system's libcurl, as the client does (`lib_http_client.cpp`), so name resolution,
//! the relay and the sandbox's CA bundle are those the real client gets; and it finds and runs
//! `mcpelauncher-webview` the way `xal_webview_qt.cpp` does (a path relative to its own
//! directory first, then `PATH`; stdout read back). Then it tries what must fail and says what
//! happened. Every finding is one `key=value` line on standard output.
//!
//! Started under the name `mcpelauncher-webview` it is the *installed* webview the stub must
//! cover: it says so loudly, so a cover that failed cannot pass unnoticed.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_long, c_void},
    io::Read,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

unsafe extern "C" {
    fn syscall(number: c_long, ...) -> c_long;
    fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn unshare(flags: c_int) -> c_int;
    fn _exit(code: c_int) -> !;
}

const SYS_CLONE: c_long = 56;
const SYS_ADD_KEY: c_long = 248;
const SYS_KEYCTL: c_long = 250;
const CLONE_NEWNET: c_int = 0x4000_0000;
const CLONE_NEWUSER: c_int = 0x1000_0000;
const SIGCHLD: c_long = 17;

/// A sign-in page's address in the shape the game's sign-in library asks for one (the bridge
/// checks every part: Minecraft's client, its redirect, a code, Xbox sign-in only, a PKCE
/// challenge and a state).
const START: &str = "https://login.live.com/oauth20_authorize.srf?client_id=0000000048183522&redirect_uri=ms-xal-\
                     0000000048183522%3A%2F%2Fauth&response_type=code&scope=service%3A%3Auser.auth.xboxlive.com%3A%3A\
                     MBI_SSL&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256\
                     &state=standin-state-1&display=android_phone";
const END: &str = "ms-xal-0000000048183522://auth";

fn say(key: &str, value: impl std::fmt::Display) {
    println!("{key}={value}");
}

/// The outcome of an operation that is expected to fail: `ok`, or the error's kind/number.
fn outcome<T>(result: std::io::Result<T>) -> String {
    match result {
        Ok(_) => "ok".into(),
        Err(e) => match e.raw_os_error() {
            Some(code) => format!("errno{code}"),
            None => format!("{:?}", e.kind()),
        },
    }
}

fn errno_of(rc: c_long) -> String {
    if rc >= 0 { "ok".into() } else { format!("errno{}", std::io::Error::last_os_error().raw_os_error().unwrap_or(0)) }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = Path::new(&args[0]).file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    if name == "mcpelauncher-webview" {
        // The installed webview. The sandbox must never let this run.
        println!("FAKE-WEBVIEW-RAN");
        println!("{END}?code=fake");
        return;
    }
    say("standin", "started");
    say("argv", args[1..].join(" "));
    let probes = read_probes();
    let session: u32 = probes.iter().find(|(k, _)| k == "session").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    say("session", session);

    game_dir();
    data(session);
    executing_what_was_written();
    webview();
    forbidden(&probes);
    network(&probes);
    say("standin", "done");
}

/// The test's instructions, put into the (test-made) game directory — or, for a version the
/// store imported (which holds nothing unlisted), into its `assets/`.
fn read_probes() -> Vec<(String, String)> {
    let text = std::fs::read_to_string("/bastion/game/standin-probes.txt")
        .or_else(|_| std::fs::read_to_string("/bastion/game/assets/standin-probes.txt"))
        .unwrap_or_default();
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn game_dir() {
    match std::fs::read("/bastion/game/lib/x86_64/libminecraftpe.so") {
        Ok(bytes) => say("game.read", bytes.len()),
        Err(e) => say("game.read", outcome::<()>(Err(e))),
    }
    say("game.write", outcome(std::fs::write("/bastion/game/planted", b"x")));
}

const WORLD: &str = "/bastion/data/games/com.mojang/minecraftWorlds/standin-world/level.dat";
const OPTIONS: &str = "/bastion/data/games/com.mojang/minecraftpe/options.txt";
const SETTINGS: &str = "/bastion/data/mcpelauncher-client-settings.txt";
/// Where Bedrock's sign-in library has kept its tokens: the external-storage folder of older
/// versions, and (an assumption standing for "anywhere else in internal storage") the root.
const TOKENS: [(&str, &str); 2] = [
    ("token.external", "/bastion/data/games/com.mojang/xal/token.json"),
    ("token.root", "/bastion/data/xal/token.json"),
];
/// What a session could leave for the client to load into the next one.
const PLANTED_LIB: &str = "/bastion/data/lib/x86_64/libjnivmsupport.so";
const PLANTED_MOD: &str = "/bastion/data/mods/evil.so";

fn data(session: u32) {
    for (key, path) in [("world.present", WORLD), ("options.present", OPTIONS)] {
        say(key, Path::new(path).exists());
    }
    say("settings.content", std::fs::read_to_string(SETTINGS).map(|t| t.trim().to_string()).unwrap_or_default());
    for (key, path) in TOKENS {
        say(&format!("{key}.present"), Path::new(path).exists());
    }
    say("planted_lib.present", Path::new(PLANTED_LIB).exists());
    say(
        "mods.entries",
        std::fs::read_dir("/bastion/data/mods")
            .map(|d| d.count().to_string())
            .unwrap_or_else(|e| outcome::<()>(Err(e))),
    );
    if session != 1 {
        return;
    }
    let write = |path: &str, text: &str| {
        let parent = Path::new(path).parent().expect("absolute");
        std::fs::create_dir_all(parent).and_then(|()| std::fs::write(path, text))
    };
    say("write.world", outcome(write(WORLD, "world-from-session-1")));
    say("write.options", outcome(write(OPTIONS, "gfx_viewdistance:8")));
    say("write.settings", outcome(std::fs::write(SETTINGS, "fullscreen=false")));
    for (key, path) in TOKENS {
        say(&format!("write.{key}"), outcome(write(path, "{\"refresh_token\":\"standin\"}")));
    }
    say("write.planted_lib", outcome(write(PLANTED_LIB, "\x7fELF")));
    say("write.planted_mod", outcome(std::fs::write(PLANTED_MOD, "\x7fELF")));
}

/// A copy of this program, written into the data root, then executed: Landlock must refuse.
fn executing_what_was_written() {
    let copy = Path::new("/bastion/data/written-exe");
    let made = std::fs::copy("/proc/self/exe", copy)
        .and_then(|_| std::fs::set_permissions(copy, std::fs::Permissions::from_mode(0o755)));
    say("written_exe.made", outcome(made));
    say("written_exe.exec", outcome(Command::new(copy).stdout(Stdio::null()).stderr(Stdio::null()).status()));
}

/// As `XalWebViewQt::findWebView` and `exec_get_stdout`.
fn webview() {
    let app_dir = std::fs::read_link("/proc/self/exe").ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let candidates = app_dir
        .iter()
        .map(|dir| dir.join("mcpelauncher-webview"))
        .chain(
            std::env::var("PATH").unwrap_or_default().split(':').map(|d| PathBuf::from(d).join("mcpelauncher-webview")),
        )
        .collect::<Vec<_>>();
    let init = std::fs::metadata("/bastion/init").ok().map(|m| (m.dev(), m.ino()));
    let same_as_init = |path: &Path| std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino())) == init && init.is_some();
    say("webview.bin_same_as_init", same_as_init(Path::new("/bastion/bin/mcpelauncher-webview")));
    if let Some(dir) = &app_dir {
        let installed = dir.join("mcpelauncher-webview");
        say("webview.installed_present", installed.exists());
        say("webview.installed_same_as_init", same_as_init(&installed));
    }
    let Some(found) = candidates.into_iter().find(|p| p.is_file()) else {
        say("webview.path", "none");
        return;
    };
    say("webview.path", found.display());
    say("webview.same_as_init", same_as_init(&found));
    let run = Command::new(&found)
        .args([START, END])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    match run {
        Ok(mut child) => {
            let mut out = Vec::new();
            if let Some(stdout) = child.stdout.take() {
                let _ = stdout.take(64 * 1024).read_to_end(&mut out);
            }
            let status = child.wait().map(|s| s.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()));
            say("webview.exit", status.unwrap_or_else(|e| format!("{e}")));
            say("webview.fake_ran", String::from_utf8_lossy(&out).contains("FAKE-WEBVIEW-RAN"));
            say("webview.stdout_bytes", out.len());
            // What the client hands its sign-in library: the first line, as it reads it.
            let first = String::from_utf8_lossy(&out).lines().next().unwrap_or("").to_string();
            say("webview.answer", first.chars().filter(|c| c.is_ascii_graphic()).collect::<String>());
        }
        Err(e) => say("webview.exit", outcome::<()>(Err(e))),
    }
}

fn forbidden(probes: &[(String, String)]) {
    say("home", outcome(std::fs::read_dir("/home")));
    for (key, value) in probes {
        if key == "host_path" {
            say(&format!("host_path[{value}]"), outcome(std::fs::metadata(value)));
        }
    }
    say("session_bus", outcome(UnixStream::connect("/run/user/1000/bus")));
    say("x11.path", outcome(UnixStream::connect("/tmp/.X11-unix/X0")));
    {
        use std::os::linux::net::SocketAddrExt;
        let abstract_x = std::os::unix::net::SocketAddr::from_abstract_name(b"/tmp/.X11-unix/X0")
            .and_then(|addr| UnixStream::connect_addr(&addr));
        say("x11.abstract", outcome(abstract_x));
    }
    // SAFETY: plain system calls with integer arguments; the strings outlive the calls.
    unsafe {
        say("keyctl", errno_of(syscall(SYS_KEYCTL, 0 as c_long, -4 as c_long, 0 as c_long)));
        let (kind, desc, payload) = (c"user", c"bastion-standin", c"x");
        say(
            "add_key",
            errno_of(syscall(SYS_ADD_KEY, kind.as_ptr(), desc.as_ptr(), payload.as_ptr(), 1 as c_long, -3 as c_long)),
        );
        say("unshare.user", errno_of(c_long::from(unshare(CLONE_NEWUSER))));
        say("unshare.net", errno_of(c_long::from(unshare(CLONE_NEWNET))));
        let rc = syscall(
            SYS_CLONE,
            c_long::from(CLONE_NEWUSER) | SIGCHLD,
            0 as c_long,
            0 as c_long,
            0 as c_long,
            0 as c_long,
        );
        if rc == 0 {
            // A child in a new user namespace: must not exist; leave at once.
            _exit(0);
        }
        say("clone.newuser", errno_of(rc));
    }
}

fn network(probes: &[(String, String)]) {
    say("bridge.present", Path::new("/bastion/run/signin.sock").exists());
    // Reachable means: connected (the launcher still listens) or refused (it has closed the
    // listener). Not reachable would be ENOENT (not mounted) or EACCES/EPERM (Landlock).
    say("bridge.connect", outcome(UnixStream::connect("/bastion/run/signin.sock")));
    say("dns.unlisted", outcome(std::net::ToSocketAddrs::to_socket_addrs(&("example.org", 443))));
    for (key, value) in probes {
        if key == "https" || key == "denied" {
            match fetch(value) {
                Ok((code, body)) => {
                    say(&format!("{key}.code"), code);
                    say(
                        &format!("{key}.body"),
                        body.chars().filter(|c| c.is_ascii_graphic()).take(40).collect::<String>(),
                    );
                }
                Err(e) => say(
                    &format!("{key}.error"),
                    e.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).collect::<String>(),
                ),
            }
        }
    }
}

type CurlInit = unsafe extern "C" fn() -> *mut c_void;
type CurlSetopt = unsafe extern "C" fn(*mut c_void, c_int, ...) -> c_int;
type CurlPerform = unsafe extern "C" fn(*mut c_void) -> c_int;
type CurlGetinfo = unsafe extern "C" fn(*mut c_void, c_int, ...) -> c_int;
type CurlCleanup = unsafe extern "C" fn(*mut c_void);
type WriteCallback = extern "C" fn(*const c_char, usize, usize, *mut c_void) -> usize;

extern "C" fn collect(data: *const c_char, size: usize, count: usize, sink: *mut c_void) -> usize {
    let len = size.saturating_mul(count);
    // SAFETY: libcurl passes `len` readable bytes and the `Vec` given as CURLOPT_WRITEDATA.
    let (bytes, sink) = unsafe { (std::slice::from_raw_parts(data.cast::<u8>(), len), &mut *sink.cast::<Vec<u8>>()) };
    if sink.len() + len > 64 * 1024 {
        return 0;
    }
    sink.extend_from_slice(bytes);
    len
}

/// GET `url` with the system's libcurl, as the client does: its own name resolution, its own
/// default CA bundle (the sandbox's), no options that weaken the TLS check.
fn fetch(url: &str) -> Result<(c_long, String), String> {
    const CURLOPT_WRITEDATA: c_int = 10001;
    const CURLOPT_URL: c_int = 10002;
    const CURLOPT_ERRORBUFFER: c_int = 10010;
    const CURLOPT_WRITEFUNCTION: c_int = 20011;
    const CURLOPT_TIMEOUT: c_int = 13;
    const CURLOPT_NOSIGNAL: c_int = 99;
    const CURLINFO_RESPONSE_CODE: c_int = 0x0020_0002;
    // SAFETY: dlopen/dlsym with valid C strings; the symbols are cast to their documented
    // libcurl signatures; every pointer handed to libcurl outlives the transfer.
    unsafe {
        let lib = dlopen(c"libcurl.so.4".as_ptr(), 2);
        if lib.is_null() {
            return Err("no libcurl".into());
        }
        let sym = |name: &CStr| dlsym(lib, name.as_ptr());
        let (init, setopt, perform, getinfo, cleanup) = (
            sym(c"curl_easy_init"),
            sym(c"curl_easy_setopt"),
            sym(c"curl_easy_perform"),
            sym(c"curl_easy_getinfo"),
            sym(c"curl_easy_cleanup"),
        );
        if [init, setopt, perform, getinfo, cleanup].iter().any(|p| p.is_null()) {
            return Err("libcurl without the easy interface".into());
        }
        let init: CurlInit = std::mem::transmute(init);
        let setopt: CurlSetopt = std::mem::transmute(setopt);
        let perform: CurlPerform = std::mem::transmute(perform);
        let getinfo: CurlGetinfo = std::mem::transmute(getinfo);
        let cleanup: CurlCleanup = std::mem::transmute(cleanup);
        let handle = init();
        if handle.is_null() {
            return Err("curl_easy_init failed".into());
        }
        let url = CString::new(url).map_err(|_| "bad url")?;
        let mut body: Vec<u8> = Vec::new();
        let mut error = [0 as c_char; 256];
        let callback: WriteCallback = collect;
        setopt(handle, CURLOPT_URL, url.as_ptr());
        setopt(handle, CURLOPT_WRITEFUNCTION, callback);
        setopt(handle, CURLOPT_WRITEDATA, (&mut body as *mut Vec<u8>).cast::<c_void>());
        setopt(handle, CURLOPT_ERRORBUFFER, error.as_mut_ptr());
        setopt(handle, CURLOPT_TIMEOUT, 20 as c_long);
        setopt(handle, CURLOPT_NOSIGNAL, 1 as c_long);
        let rc = perform(handle);
        let mut code: c_long = 0;
        getinfo(handle, CURLINFO_RESPONSE_CODE, &mut code as *mut c_long);
        cleanup(handle);
        if rc != 0 {
            let text = CStr::from_ptr(error.as_ptr()).to_string_lossy().into_owned();
            return Err(format!("curl {rc}: {text}"));
        }
        Ok((code, String::from_utf8_lossy(&body).into_owned()))
    }
}
