//! The launcher's self-confinement, applied to a real multi-threaded process.
//!
//! Neither Landlock nor a seccomp filter can be taken off again, so the confined process is
//! a child: this test binary re-executes itself to run `confined_child`, which confines
//! itself and then reports, from several threads, what it can still do. The threads matter:
//! the restriction is only worth anything if it also covers the ones that existed before it
//! was applied.
//!
//! Two questions are asked of every thread: what is gone (files, programs, ports, sockets,
//! and — the reason for the system-call filter — the kernel keyring), and whether everything
//! a supervising launcher still has to do keeps working (relaying, writing the settings
//! back, removing its scratch directory, signalling and reaping the sandbox, starting
//! threads, resolving names, allocating token memory).
//!
//! The keyring probes never touch a key of the user: the child first joins a new, empty
//! session keyring and creates its own key there, which is gone when the child exits.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{
            net::{SocketAddr, UnixListener, UnixStream},
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use bastion_mc::{
    confine::{self, Confinement, Demand},
    sandbox::landlock::kernel_abi,
};

const CHILD_DIR: &str = "BASTION_CONFINE_CHILD_DIR";
const CHILD_MODE: &str = "BASTION_CONFINE_CHILD_MODE";
const ALLOWED_PORT: &str = "BASTION_CONFINE_ALLOWED_PORT";
const DENIED_PORT: &str = "BASTION_CONFINE_DENIED_PORT";
const ABSTRACT_NAME: &str = "BASTION_CONFINE_ABSTRACT";
const PATH_SOCKET: &str = "BASTION_CONFINE_PATH_SOCKET";
const INBOUND_FD: &str = "BASTION_CONFINE_INBOUND_FD";

const KEYCTL_JOIN_SESSION_KEYRING: libc::c_long = 1;
const KEYCTL_SEARCH: libc::c_long = 10;
const KEYCTL_READ: libc::c_long = 11;
const KEY_SPEC_SESSION_KEYRING: libc::c_long = -3;
const TIOCSTI: libc::c_ulong = 0x5412;
const X32: libc::c_long = 0x4000_0000;
const KEY_PAYLOAD: &[u8] = b"REFRESH-TOKEN-STAND-IN";

fn outcome<T>(result: std::io::Result<T>) -> String {
    match result {
        Ok(_) => "ok".into(),
        Err(e) => match e.raw_os_error() {
            Some(libc::EACCES) => "EACCES".into(),
            Some(libc::EPERM) => "EPERM".into(),
            Some(libc::ENOSYS) => "ENOSYS".into(),
            Some(libc::EBADF) => "EBADF".into(),
            Some(libc::ECONNREFUSED) => "ECONNREFUSED".into(),
            Some(libc::ENOENT) => "ENOENT".into(),
            _ => format!("error({e})"),
        },
    }
}

/// The result of a raw system call: its value, or the error.
fn sys(value: libc::c_long) -> std::io::Result<libc::c_long> {
    if value < 0 { Err(std::io::Error::last_os_error()) } else { Ok(value) }
}

/// The child's own key: its description and its serial number.
#[derive(Clone)]
struct TestKey {
    description: std::ffi::CString,
    id: libc::c_long,
}

/// A session keyring of this process's own, with one key in it. Called before confinement.
fn private_keyring() -> TestKey {
    // SAFETY: plain system calls on this process's own keyrings; a null name asks for a new
    // anonymous session keyring, and every pointer is to a live, NUL-terminated buffer.
    unsafe {
        let ring = libc::syscall(libc::SYS_keyctl, KEYCTL_JOIN_SESSION_KEYRING, std::ptr::null::<libc::c_char>());
        assert!(ring > 0, "joining a new session keyring: {}", std::io::Error::last_os_error());
        let description = std::ffi::CString::new(format!("bastion-mc-test-confine:{}", std::process::id())).unwrap();
        let id = libc::syscall(
            libc::SYS_add_key,
            c"user".as_ptr(),
            description.as_ptr(),
            KEY_PAYLOAD.as_ptr(),
            KEY_PAYLOAD.len(),
            KEY_SPEC_SESSION_KEYRING,
        );
        assert!(id > 0, "add_key: {}", std::io::Error::last_os_error());
        TestKey { description, id }
    }
}

/// What `auth::store` does to load a token: search the session keyring, read the key.
fn keyring_probes(key: &TestKey, entry: libc::c_long) -> Vec<(&'static str, String)> {
    let mut buffer = [0u8; 64];
    // SAFETY: raw system calls with pointers to live buffers of the stated sizes. `entry`
    // selects the ordinary or the x32 entry point of the same call.
    unsafe {
        let search = sys(libc::syscall(
            entry | libc::SYS_keyctl,
            KEYCTL_SEARCH,
            KEY_SPEC_SESSION_KEYRING,
            c"user".as_ptr(),
            key.description.as_ptr(),
            0,
        ));
        let read = sys(libc::syscall(entry | libc::SYS_keyctl, KEYCTL_READ, key.id, buffer.as_mut_ptr(), buffer.len()))
            .map(|n| assert_eq!(&buffer[..n as usize], KEY_PAYLOAD));
        let add = sys(libc::syscall(
            entry | libc::SYS_add_key,
            c"user".as_ptr(),
            c"bastion-mc-test-confine:second".as_ptr(),
            KEY_PAYLOAD.as_ptr(),
            KEY_PAYLOAD.len(),
            KEY_SPEC_SESSION_KEYRING,
        ));
        let request = sys(libc::syscall(
            entry | libc::SYS_request_key,
            c"user".as_ptr(),
            key.description.as_ptr(),
            std::ptr::null::<libc::c_char>(),
            0,
        ));
        vec![
            ("keyctl_search", outcome(search)),
            ("keyctl_read", outcome(read)),
            ("add_key", outcome(add)),
            ("request_key", outcome(request)),
        ]
    }
}

/// Everything a compromised launcher might try, attempted from the calling thread.
fn probes(
    label: &str,
    dir: &Path,
    allowed_port: u16,
    denied_port: u16,
    abstract_name: &str,
    key: &TestKey,
    memfd: RawFd,
) -> Vec<String> {
    let allowed = dir.join("allowed");
    let mine = allowed.join(format!("{label}.txt"));
    let abstract_addr = <SocketAddr as std::os::linux::net::SocketAddrExt>::from_abstract_name(abstract_name).unwrap();
    let mut results = vec![
        ("read_secret", outcome(std::fs::read(dir.join("secret.txt")))),
        ("list_outside", outcome(std::fs::read_dir(dir))),
        ("create_outside", outcome(std::fs::write(dir.join(format!("{label}-escape.txt")), b"x"))),
        ("write_allowed", outcome(std::fs::write(&mine, b"settings"))),
        ("rename_allowed", outcome(std::fs::rename(&mine, allowed.join(format!("{label}.done"))))),
        ("remove_allowed", outcome(std::fs::remove_file(allowed.join(format!("{label}.done"))))),
        ("symlink_allowed", outcome(std::os::unix::fs::symlink("/etc/passwd", allowed.join(format!("{label}.link"))))),
        ("read_etc", outcome(std::fs::read("/etc/passwd"))),
        // /proc stays readable (the supervisor's own bookkeeping). What that is worth for
        // another process of the user: its command line is public; its environment is not.
        ("read_other_process_cmdline", outcome(std::fs::read(format!("/proc/{}/cmdline", nix::unistd::getppid())))),
        ("read_other_process_environ", outcome(std::fs::read(format!("/proc/{}/environ", nix::unistd::getppid())))),
        ("write_etc_like", outcome(std::fs::OpenOptions::new().append(true).open("/etc/passwd"))),
        ("exec", outcome(Command::new("/usr/bin/true").status())),
        // The same program as an in-memory file: no path a Landlock rule could name.
        ("exec_memfd", outcome(Command::new(format!("/proc/self/fd/{memfd}")).status())),
        ("connect_allowed_port", outcome(TcpStream::connect((Ipv4Addr::LOCALHOST, allowed_port)))),
        ("connect_other_port", outcome(TcpStream::connect((Ipv4Addr::LOCALHOST, denied_port)))),
        ("bind", outcome(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))),
        ("connect_abstract_socket", outcome(UnixStream::connect_addr(&abstract_addr))),
        (
            "connect_path_socket",
            outcome(UnixStream::connect(std::env::var_os(PATH_SOCKET).expect("set by the parent"))),
        ),
        (
            "signal_parent",
            outcome(
                nix::sys::signal::kill(nix::unistd::getppid(), None)
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32)),
            ),
        ),
        ("resolve_localhost", outcome(std::net::ToSocketAddrs::to_socket_addrs(&("localhost", 80)).map(|_| ()))),
        // Token memory (memfd_secret or mlock) can still be allocated and wiped.
        (
            "secret_memory",
            outcome(
                bastion_mc::secret::Secret::from_slice(b"token")
                    .map(|_| ())
                    .map_err(|e| std::io::Error::other(e.to_string())),
            ),
        ),
    ];
    results.extend(keyring_probes(key, 0));
    results.extend(keyring_probes(key, X32).into_iter().map(|(name, result)| {
        let name: &'static str = match name {
            "keyctl_search" => "x32_keyctl_search",
            "keyctl_read" => "x32_keyctl_read",
            "add_key" => "x32_add_key",
            _ => "x32_request_key",
        };
        (name, result)
    }));
    let mut byte = b'x' as libc::c_char;
    // SAFETY: raw system calls that are expected to fail; null or invalid arguments are part
    // of the probe (an unfiltered call fails with EFAULT, EINVAL or EBADF instead). The one
    // that can succeed without the filter is the clone: it then behaves like fork, and the
    // new process leaves at once, without running anything of this program.
    unsafe {
        let cloned = libc::syscall(libc::SYS_clone, libc::CLONE_NEWUSER | libc::SIGCHLD, 0, 0, 0, 0);
        if cloned == 0 {
            libc::_exit(0);
        }
        let cloned = sys(cloned).inspect(|pid| {
            libc::waitpid(*pid as libc::pid_t, std::ptr::null_mut(), 0);
        });
        results.extend([
            ("x32_getpid", outcome(sys(libc::syscall(X32 | libc::SYS_getpid)))),
            ("unshare_user_namespace", outcome(sys(libc::syscall(libc::SYS_unshare, libc::CLONE_NEWUSER)))),
            ("clone_with_namespace", outcome(cloned)),
            ("clone3", outcome(sys(libc::syscall(libc::SYS_clone3, std::ptr::null::<u8>(), 0)))),
            ("io_uring_setup", outcome(sys(libc::syscall(libc::SYS_io_uring_setup, 0, std::ptr::null::<u8>())))),
            ("userfaultfd", outcome(sys(libc::syscall(libc::SYS_userfaultfd, -1)))),
            (
                "perf_event_open",
                outcome(sys(libc::syscall(libc::SYS_perf_event_open, std::ptr::null::<u8>(), 0, -1, -1, 0))),
            ),
            ("pidfd_getfd", outcome(sys(libc::syscall(libc::SYS_pidfd_getfd, -1, 0, 0)))),
            ("tiocsti", outcome(sys(libc::ioctl(-1, TIOCSTI, &mut byte) as libc::c_long))),
            // An ordinary ioctl is not the filter's business: the kernel answers.
            ("other_ioctl", outcome(sys(libc::ioctl(-1, libc::FIONREAD, &mut byte) as libc::c_long))),
        ]);
    }
    results.into_iter().map(|(name, result)| format!("{label}.{name}={result}")).collect()
}

/// The re-executed half. Does nothing in a normal test run.
#[test]
fn confined_child() {
    let Some(dir) = std::env::var_os(CHILD_DIR).map(PathBuf::from) else { return };
    let mode = std::env::var(CHILD_MODE).unwrap();
    let port = |name: &str| std::env::var(name).unwrap().parse::<u16>().unwrap();
    let (allowed_port, denied_port) = (port(ALLOWED_PORT), port(DENIED_PORT));
    let abstract_name = std::env::var(ABSTRACT_NAME).unwrap();
    let key = private_keyring();
    // An in-memory copy of a program, made while files can still be read.
    // SAFETY: memfd_create with a static name; the descriptor is owned by this process.
    let memfd = unsafe { libc::memfd_create(c"bastion-confine-probe".as_ptr(), 0) };
    assert!(memfd >= 0);
    let image = std::fs::read("/usr/bin/true").unwrap();
    // SAFETY: writes `image.len()` bytes from a live buffer to the descriptor above.
    assert_eq!(unsafe { libc::write(memfd, image.as_ptr().cast(), image.len()) }, image.len() as isize);

    // (libtest writes "test confined_child ... " without a newline first.)
    println!("\nbefore.read_secret={}", outcome(std::fs::read(dir.join("secret.txt"))));
    println!(
        "before.read_other_process_environ={}",
        outcome(std::fs::read(format!("/proc/{}/environ", nix::unistd::getppid())))
    );
    for (name, result) in keyring_probes(&key, 0) {
        println!("before.{name}={result}");
    }

    // What a supervisor holds when it confines itself: a log file, a child process, a
    // listening socket of the relay, an async runtime with its workers, and other threads.
    let mut log = std::fs::File::create(dir.join("launch.log")).unwrap();
    let scratch = dir.join("allowed/scratch");
    std::fs::create_dir_all(scratch.join("saves/world/region")).unwrap();
    std::fs::write(scratch.join("saves/world/region/r.0.0.mca"), b"chunk").unwrap();
    std::fs::write(scratch.join("options.txt"), b"fov:70\n\x1b[31mbad:1\n").unwrap();
    // What a game can leave behind to keep its root from being deleted: a directory without
    // any permission bits.
    std::fs::create_dir_all(scratch.join("locked/inner")).unwrap();
    std::fs::write(scratch.join("locked/inner/kept"), b"x").unwrap();
    std::fs::set_permissions(scratch.join("locked"), std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    std::fs::create_dir_all(dir.join("allowed/persist")).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let mut sandbox = runtime.block_on(async {
        tokio::process::Command::new("/usr/bin/sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    });
    let inbound = std::env::var(INBOUND_FD).ok().map(|fd| {
        // SAFETY: the parent passed this listening socket for exactly this purpose.
        let listener = unsafe { TcpListener::from_raw_fd(fd.parse().unwrap()) };
        listener.set_nonblocking(true).unwrap();
        listener
    });

    let (go_plain, wait_plain) = mpsc::channel::<()>();
    let (plain_out, plain_results) = mpsc::channel::<Vec<String>>();
    let (d, a, k) = (dir.clone(), abstract_name.clone(), key.clone());
    let plain = std::thread::spawn(move || {
        wait_plain.recv().unwrap();
        plain_out.send(probes("old_thread", &d, allowed_port, denied_port, &a, &k, memfd)).unwrap();
    });
    let (go_worker, wait_worker) = tokio::sync::oneshot::channel::<()>();
    let (d, a, k) = (dir.clone(), abstract_name.clone(), key.clone());
    let worker = runtime.spawn(async move {
        wait_worker.await.unwrap();
        probes("runtime_worker", &d, allowed_port, denied_port, &a, &k, memfd)
    });
    // Make sure the worker task is parked on a worker thread, not still queued.
    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });

    let mut confinement = Confinement::supervisor();
    confinement.write.push(dir.join("allowed"));
    confinement.tcp_connect.push(allowed_port);
    let applied = match mode.as_str() {
        "whole" | "tripwire" => confine::apply(&confinement, Demand::Whole),
        "abi8" => confine::apply_as(&confinement, Demand::Possible, 8),
        "abi7" => confine::apply_as(&confinement, Demand::Possible, 7),
        "require-abi8" => confine::apply_as(&confinement, Demand::Whole, 8),
        other => panic!("unknown mode {other}"),
    };
    match &applied {
        Ok(applied) => {
            println!("confine=ok");
            println!("gaps={}", applied.gaps.join(" | "));
        }
        Err(e) => println!("confine=failed: {e}"),
    }
    if mode == "tripwire" {
        // (This process is about to be killed: its stand-in sandbox would be left behind.)
        let _ = sandbox.start_kill();
        // An exploit-only call, from a thread that existed before the filter did.
        let (go, wait) = mpsc::channel::<()>();
        let old = std::thread::spawn(move || {
            wait.recv().unwrap();
            // SAFETY: PTRACE_TRACEME takes no pointers. With the filter the call never returns.
            unsafe { libc::syscall(libc::SYS_ptrace, libc::PTRACE_TRACEME, 0, 0, 0) };
        });
        go.send(()).unwrap();
        old.join().unwrap();
        println!("tripwire=survived");
        return;
    }

    let mut lines = probes("main", &dir, allowed_port, denied_port, &abstract_name, &key, memfd);
    go_plain.send(()).unwrap();
    lines.extend(plain_results.recv().unwrap());
    plain.join().unwrap();
    go_worker.send(()).unwrap();
    lines.extend(runtime.block_on(worker).unwrap());
    let (d, a, k) = (dir.clone(), abstract_name.clone(), key.clone());
    lines.extend(
        std::thread::spawn(move || probes("new_thread", &d, allowed_port, denied_port, &a, &k, memfd)).join().unwrap(),
    );

    // The supervisor's remaining work.
    // A descriptor opened earlier keeps working although its path is out of reach now.
    lines.push(format!("work.log_write={}", outcome(log.write_all(b"[launcher] still logging\n"))));
    // Settings carried back through the sanitiser, into a kept directory.
    let carried = bastion_mc::persist::carry(&scratch, &dir.join("allowed/persist"), "options.txt");
    lines.push(format!(
        "work.settings_back={}",
        match carried {
            Ok(Some(report)) => format!("kept{}dropped{}", report.kept, report.dropped),
            other => format!("{other:?}"),
        }
    ));
    // The scratch directory, the way the launcher removes it (`tree::remove`: it changes
    // modes, opens directories one by one and unlinks relative to them).
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    lines.push(format!("work.remove_scratch={}", outcome(bastion_mc::tree::remove(&scratch, Some(deadline)))));
    // A blocking task: the runtime starts a new thread for it (clone, since clone3 is "absent").
    let blocking = runtime.block_on(async { tokio::task::spawn_blocking(|| std::thread::current().id()).await });
    lines.push(format!("work.blocking_task={}", if blocking.is_ok() { "ok" } else { "failed" }));
    // Relaying: accept on a socket bound earlier, connect out to a kept port, copy both ways.
    if let Some(listener) = inbound {
        let relayed = runtime.block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            let (mut game, _) = tokio::time::timeout(Duration::from_secs(20), listener.accept()).await??;
            let mut upstream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, allowed_port)).await?;
            let (to_server, to_game) = tokio::io::copy_bidirectional(&mut game, &mut upstream).await?;
            Ok::<_, std::io::Error>(format!("{to_server}+{to_game}"))
        });
        lines.push(format!("work.relay={}", relayed.unwrap_or_else(|e| format!("error({e})"))));
    }
    // Signalling and reaping the sandbox.
    let pid = nix::unistd::Pid::from_raw(sandbox.id().unwrap() as i32);
    let signalled = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
    let status = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(20), sandbox.wait()).await });
    lines.push(format!(
        "work.stop_sandbox={}",
        match (signalled, status) {
            (Ok(()), Ok(Ok(status))) if status.signal() == Some(libc::SIGTERM) => "ok".to_string(),
            other => format!("{other:?}"),
        }
    ));
    for line in lines {
        println!("{line}");
    }
    std::io::stdout().flush().unwrap();
}

/// Things outside the child for it to reach, or not.
struct World {
    dir: PathBuf,
    allowed: TcpListener,
    denied: TcpListener,
    abstract_name: String,
    _abstract_listener: UnixListener,
    _path_listener: UnixListener,
    /// Where the pathname socket lives. A socket address holds 107 bytes of path, so this is
    /// a short directory of its own and not one under the build directory, whose length is
    /// whatever the checkout's is (with a seven-digit process id in the name it stopped
    /// fitting in a deep checkout).
    socket_dir: PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.socket_dir);
    }
}

impl World {
    fn new(tag: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("confine-{tag}-{}", std::process::id()));
        let _ = bastion_mc::tree::remove(&dir, None);
        std::fs::create_dir_all(dir.join("allowed")).unwrap();
        std::fs::write(dir.join("secret.txt"), b"id_ed25519").unwrap();
        let socket_dir = std::env::temp_dir().join(format!("bcf-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&socket_dir);
        std::fs::create_dir(&socket_dir).unwrap();
        let abstract_name = format!("bastion-confine-{tag}-{}", std::process::id());
        let abstract_addr =
            <SocketAddr as std::os::linux::net::SocketAddrExt>::from_abstract_name(&abstract_name).unwrap();
        World {
            allowed: TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap(),
            denied: TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap(),
            _abstract_listener: UnixListener::bind_addr(&abstract_addr).unwrap(),
            _path_listener: UnixListener::bind(socket_dir.join("s")).unwrap(),
            socket_dir,
            abstract_name,
            dir,
        }
    }

    fn child(&self, mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["confined_child", "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_DIR, &self.dir)
            .env(CHILD_MODE, mode)
            .env(ALLOWED_PORT, self.allowed.local_addr().unwrap().port().to_string())
            .env(DENIED_PORT, self.denied.local_addr().unwrap().port().to_string())
            .env(ABSTRACT_NAME, &self.abstract_name)
            .env(PATH_SOCKET, self.socket_dir.join("s"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Delete what the test left. (A child that was killed before it removed its scratch
    /// directory leaves the mode-000 directory in it: the plain recursive delete stops there.)
    fn remove(&self) {
        bastion_mc::tree::remove(&self.dir, None).unwrap();
    }

    /// Run the child to its end; its output as `key=value` facts.
    fn run(&self, mut command: Command) -> (std::process::ExitStatus, String) {
        let mut child = command.spawn().unwrap();
        let mut stdout = String::new();
        child.stdout.take().unwrap().read_to_string(&mut stdout).unwrap();
        let mut stderr = String::new();
        child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
        (child.wait().unwrap(), format!("{stdout}\n--- stderr ---\n{stderr}"))
    }
}

fn parse_facts(output: &str) -> BTreeMap<&str, &str> {
    output.lines().filter_map(|l| l.split_once('=')).collect()
}

const THREADS: [&str; 4] = ["main", "old_thread", "runtime_worker", "new_thread"];

#[test]
fn a_confined_process_keeps_only_what_it_was_left() {
    if confine::support() != confine::Support::Whole {
        eprintln!("skipped: {:?}", confine::support());
        return;
    }
    let world = World::new("whole");

    // The other end of the relay: a "game" that connects to a socket the child holds, and a
    // "server" on the kept port that answers what it is sent, in upper case.
    let inbound = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let inbound_port = inbound.local_addr().unwrap().port();
    let upstream = world.allowed.try_clone().unwrap();
    let server = std::thread::spawn(move || {
        // The probes connect here too (and hang up at once); the relay's connection carries text.
        for stream in upstream.incoming() {
            let Ok(mut stream) = stream else { break };
            stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            let mut request = Vec::new();
            let _ = stream.read_to_end(&mut request);
            if request == b"stop" {
                break;
            }
            let _ = stream.write_all(&request.to_ascii_uppercase());
        }
    });
    let game = std::thread::spawn(move || {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, inbound_port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(40))).unwrap();
        stream.write_all(b"through the relay").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        answer
    });

    let mut command = world.child("whole");
    let inbound_fd = inbound.as_raw_fd();
    command.env(INBOUND_FD, inbound_fd.to_string());
    // SAFETY: runs between fork and exec and only calls fcntl(2), which is async-signal-safe:
    // the listening socket loses close-on-exec so that the child inherits it.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(inbound_fd, libc::F_SETFD, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let (status, output) = world.run(command);
    drop(inbound);
    let _ = TcpStream::connect(world.allowed.local_addr().unwrap()).and_then(|mut s| s.write_all(b"stop"));
    server.join().unwrap();
    assert!(status.success(), "child failed:\n{output}");

    let facts = parse_facts(&output);
    let get = |key: &str| *facts.get(key).unwrap_or_else(|| panic!("no `{key}` in:\n{output}"));
    assert_eq!(get("before.read_secret"), "ok", "the file is readable until the process confines itself");
    assert_eq!(get("before.read_other_process_environ"), "ok", "and so is the environment of the user's processes");
    for probe in ["keyctl_search", "keyctl_read", "add_key", "request_key"] {
        assert_eq!(get(&format!("before.{probe}")), "ok", "the keyring works until the process confines itself");
    }
    assert_eq!(get("confine"), "ok");
    assert_eq!(get("gaps"), "", "on this kernel the whole confinement is in force");

    // The same answers on every thread, including the two that existed beforehand.
    for thread in THREADS {
        let on = |probe: &str| get(&format!("{thread}.{probe}"));
        assert_eq!(on("read_secret"), "EACCES", "{thread}: files outside the kept trees");
        assert_eq!(on("list_outside"), "EACCES", "{thread}");
        assert_eq!(on("create_outside"), "EACCES", "{thread}");
        assert_eq!(on("write_allowed"), "ok", "{thread}: the kept tree stays writable");
        assert_eq!(on("rename_allowed"), "ok", "{thread}: atomic replace still works");
        assert_eq!(on("remove_allowed"), "ok", "{thread}");
        assert_eq!(on("symlink_allowed"), "EACCES", "{thread}: no symlinks, even where writing is allowed");
        assert_eq!(on("read_etc"), "ok", "{thread}: resolver configuration stays readable");
        assert_eq!(on("read_other_process_cmdline"), "ok", "{thread}: /proc stays readable: who runs what is visible");
        assert_eq!(
            on("read_other_process_environ"),
            "EACCES",
            "{thread}: but not what needs access to another process (its environment, its memory)"
        );
        assert_eq!(on("write_etc_like"), "EACCES", "{thread}: read-only means read-only");
        assert_eq!(on("exec"), "EPERM", "{thread}: nothing can be executed (the filter answers first)");
        assert_eq!(on("exec_memfd"), "EPERM", "{thread}: not even a program that has no path");
        assert_eq!(on("connect_allowed_port"), "ok", "{thread}: the relay's ports");
        assert_eq!(on("connect_other_port"), "EACCES", "{thread}: any other port");
        assert_eq!(on("bind"), "EACCES", "{thread}: no new listeners");
        assert_eq!(on("connect_abstract_socket"), "EPERM", "{thread}: other processes' abstract sockets");
        assert_eq!(on("connect_path_socket"), "EACCES", "{thread}: pathname sockets that were not kept");
        assert_eq!(on("signal_parent"), "ok", "{thread}: signals are not scoped (the game must be stoppable)");
        assert_eq!(on("resolve_localhost"), "ok", "{thread}: name resolution keeps working");
        assert_eq!(on("secret_memory"), "ok", "{thread}: token memory can still be allocated");

        // The kernel keyring: what `auth::store` does to load a refresh token, and the two
        // other calls that reach it — through the ordinary and the x32 entry points.
        for probe in ["keyctl_search", "keyctl_read", "add_key", "request_key"] {
            assert_eq!(on(probe), "EPERM", "{thread}: the kernel keyring is out of reach");
            assert_eq!(on(&format!("x32_{probe}")), "ENOSYS", "{thread}: also under its x32 number");
        }
        assert_eq!(on("x32_getpid"), "ENOSYS", "{thread}: the x32 table is gone as a whole");
        assert_eq!(on("unshare_user_namespace"), "EPERM", "{thread}");
        assert_eq!(on("clone_with_namespace"), "EPERM", "{thread}");
        assert_eq!(on("clone3"), "ENOSYS", "{thread}: so that thread creation goes through clone");
        assert_eq!(on("io_uring_setup"), "EPERM", "{thread}");
        assert_eq!(on("userfaultfd"), "EPERM", "{thread}");
        assert_eq!(on("perf_event_open"), "EPERM", "{thread}");
        assert_eq!(on("pidfd_getfd"), "EPERM", "{thread}");
        assert_eq!(on("tiocsti"), "EPERM", "{thread}: no typing into the terminal");
        assert_eq!(on("other_ioctl"), "EBADF", "{thread}: other ioctls are answered by the kernel");
    }

    // Everything the supervisor still has to do.
    assert_eq!(get("work.log_write"), "ok", "the open log file");
    assert_eq!(get("work.settings_back"), "kept1dropped1", "settings are carried back, through the sanitiser");
    assert_eq!(
        std::fs::read(world.dir.join("allowed/persist/options.txt")).unwrap(),
        b"fov:70\n",
        "what arrived is the sanitised file"
    );
    assert_eq!(get("work.remove_scratch"), "ok", "the scratch directory is removed, mode-000 directory and all");
    assert!(!world.dir.join("allowed/scratch").exists());
    assert_eq!(get("work.blocking_task"), "ok", "new threads can be started");
    assert_eq!(get("work.relay"), "17+17", "bytes are relayed in both directions");
    assert_eq!(game.join().unwrap(), "THROUGH THE RELAY");
    assert_eq!(get("work.stop_sandbox"), "ok", "the sandbox is signalled and reaped");
    assert_eq!(std::fs::read(world.dir.join("launch.log")).unwrap(), b"[launcher] still logging\n");

    // Nothing leaked out of the kept tree while trying.
    let left: Vec<_> = std::fs::read_dir(&world.dir).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(left.iter().all(|n| !n.to_string_lossy().contains("escape")), "{left:?}");
    world.remove();
}

/// An exploit-only system call ends the process — also when it comes from a thread that was
/// running before the filter was installed.
#[test]
fn an_exploit_only_system_call_kills_the_confined_launcher() {
    if confine::support() != confine::Support::Whole {
        eprintln!("skipped: {:?}", confine::support());
        return;
    }
    let world = World::new("tripwire");
    let (status, output) = world.run(world.child("tripwire"));
    assert_eq!(status.signal(), Some(libc::SIGSYS), "killed by the filter:\n{output}");
    assert!(output.contains("confine=ok") && !output.contains("tripwire=survived"), "{output}");
    world.remove();
}

/// A kernel whose Landlock cannot mediate pathname sockets (ABI 8): the rest is applied,
/// the gap is real, and it is reported — or, where the whole is demanded, nothing is applied.
#[test]
fn a_kernel_without_pathname_socket_control_is_confined_partly_and_says_so() {
    if kernel_abi() < confine::MIN_ABI {
        eprintln!("skipped: Landlock ABI {}", kernel_abi());
        return;
    }
    let world = World::new("abi8");
    let (status, output) = world.run(world.child("abi8"));
    assert!(status.success(), "child failed:\n{output}");
    let facts = parse_facts(&output);
    let get = |key: &str| *facts.get(key).unwrap_or_else(|| panic!("no `{key}` in:\n{output}"));
    assert_eq!(get("confine"), "ok");
    assert!(get("gaps").contains("pathname UNIX sockets") && get("gaps").contains("D-Bus"), "{}", get("gaps"));
    for thread in THREADS {
        let on = |probe: &str| get(&format!("{thread}.{probe}"));
        assert_eq!(on("connect_path_socket"), "ok", "{thread}: the gap is real — a pathname socket is reachable");
        assert_eq!(on("read_secret"), "EACCES", "{thread}: files are confined all the same");
        assert_eq!(on("read_other_process_environ"), "EACCES", "{thread}");
        assert_eq!(on("connect_other_port"), "EACCES", "{thread}");
        assert_eq!(on("connect_abstract_socket"), "EPERM", "{thread}");
        assert_eq!(on("keyctl_read"), "EPERM", "{thread}: and so is the keyring");
        assert_eq!(on("exec"), "EPERM", "{thread}");
    }
    world.remove();

    // "require" on such a kernel: refused before anything is restricted.
    let world = World::new("require");
    let (status, output) = world.run(world.child("require-abi8"));
    assert!(status.success(), "child failed:\n{output}");
    let facts = parse_facts(&output);
    let get = |key: &str| *facts.get(key).unwrap_or_else(|| panic!("no `{key}` in:\n{output}"));
    assert!(get("confine").starts_with("failed") && get("confine").contains("pathname UNIX sockets"), "{output}");
    for thread in THREADS {
        assert_eq!(get(&format!("{thread}.read_secret")), "ok", "{thread}: nothing was restricted");
        assert_eq!(get(&format!("{thread}.keyctl_read")), "ok", "{thread}: nothing was filtered");
    }
    world.remove();
}

/// A kernel that cannot apply Landlock to all threads (ABI below 8): only the system-call
/// filter is installed, and the result says that files and network are not restricted.
#[test]
fn a_kernel_without_thread_synchronised_landlock_gets_the_filter_alone() {
    let world = World::new("abi7");
    let (status, output) = world.run(world.child("abi7"));
    assert!(status.success(), "child failed:\n{output}");
    let facts = parse_facts(&output);
    let get = |key: &str| *facts.get(key).unwrap_or_else(|| panic!("no `{key}` in:\n{output}"));
    assert_eq!(get("confine"), "ok");
    assert!(get("gaps").contains("NOT restricted"), "{}", get("gaps"));
    for thread in THREADS {
        let on = |probe: &str| get(&format!("{thread}.{probe}"));
        assert_eq!(on("read_secret"), "ok", "{thread}: files are not confined, as reported");
        assert_eq!(on("read_other_process_environ"), "ok", "{thread}: nor is access to other processes");
        assert_eq!(on("keyctl_search"), "EPERM", "{thread}: the keyring is out of reach");
        assert_eq!(on("x32_keyctl_search"), "ENOSYS", "{thread}");
        assert_eq!(on("exec"), "EPERM", "{thread}: no program execution");
    }
    world.remove();
}
