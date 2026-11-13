//! The launcher's self-confinement, applied to a real multi-threaded process.
//!
//! Landlock cannot be taken off again, so the confined process is a child: this test binary
//! re-executes itself to run `confined_child`, which confines itself and then reports, from
//! several threads, what it can still do. The threads matter: the restriction is only worth
//! anything if it also covers the ones that existed before it was applied.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    os::unix::net::{SocketAddr, UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
};

use bastion_mc::{
    confine::{self, Confinement},
    sandbox::landlock::kernel_abi,
};

const CHILD_DIR: &str = "BASTION_CONFINE_CHILD_DIR";
const ALLOWED_PORT: &str = "BASTION_CONFINE_ALLOWED_PORT";
const DENIED_PORT: &str = "BASTION_CONFINE_DENIED_PORT";
const ABSTRACT_NAME: &str = "BASTION_CONFINE_ABSTRACT";

fn outcome<T>(result: std::io::Result<T>) -> String {
    match result {
        Ok(_) => "ok".into(),
        Err(e) => match e.raw_os_error() {
            Some(libc::EACCES) => "EACCES".into(),
            Some(libc::EPERM) => "EPERM".into(),
            Some(libc::ECONNREFUSED) => "ECONNREFUSED".into(),
            Some(libc::ENOENT) => "ENOENT".into(),
            _ => format!("error({e})"),
        },
    }
}

/// Everything a compromised launcher might try, attempted from the calling thread.
fn probes(label: &str, dir: &Path, allowed_port: u16, denied_port: u16, abstract_name: &str) -> Vec<String> {
    let allowed = dir.join("allowed");
    let mine = allowed.join(format!("{label}.txt"));
    let abstract_addr = <SocketAddr as std::os::linux::net::SocketAddrExt>::from_abstract_name(abstract_name).unwrap();
    let results = [
        ("read_secret", outcome(std::fs::read(dir.join("secret.txt")))),
        ("list_outside", outcome(std::fs::read_dir(dir))),
        ("create_outside", outcome(std::fs::write(dir.join(format!("{label}-escape.txt")), b"x"))),
        ("write_allowed", outcome(std::fs::write(&mine, b"settings"))),
        ("rename_allowed", outcome(std::fs::rename(&mine, allowed.join(format!("{label}.done"))))),
        ("remove_allowed", outcome(std::fs::remove_file(allowed.join(format!("{label}.done"))))),
        ("symlink_allowed", outcome(std::os::unix::fs::symlink("/etc/passwd", allowed.join(format!("{label}.link"))))),
        ("read_etc", outcome(std::fs::read("/etc/passwd"))),
        ("write_etc_like", outcome(std::fs::OpenOptions::new().append(true).open("/etc/passwd"))),
        ("exec", outcome(Command::new("/usr/bin/true").status())),
        ("connect_allowed_port", outcome(TcpStream::connect((Ipv4Addr::LOCALHOST, allowed_port)))),
        ("connect_other_port", outcome(TcpStream::connect((Ipv4Addr::LOCALHOST, denied_port)))),
        ("bind", outcome(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))),
        ("connect_abstract_socket", outcome(UnixStream::connect_addr(&abstract_addr))),
        ("connect_path_socket", outcome(UnixStream::connect(dir.join("other.sock")))),
        (
            "signal_parent",
            outcome(
                nix::sys::signal::kill(nix::unistd::getppid(), None)
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32)),
            ),
        ),
        ("resolve_localhost", outcome(std::net::ToSocketAddrs::to_socket_addrs(&("localhost", 80)).map(|_| ()))),
    ];
    results.into_iter().map(|(name, result)| format!("{label}.{name}={result}")).collect()
}

/// The re-executed half. Does nothing in a normal test run.
#[test]
fn confined_child() {
    let Some(dir) = std::env::var_os(CHILD_DIR).map(PathBuf::from) else { return };
    let port = |name: &str| std::env::var(name).unwrap().parse::<u16>().unwrap();
    let (allowed_port, denied_port) = (port(ALLOWED_PORT), port(DENIED_PORT));
    let abstract_name = std::env::var(ABSTRACT_NAME).unwrap();

    // (libtest writes "test confined_child ... " without a newline first.)
    println!("\nbefore.read_secret={}", outcome(std::fs::read(dir.join("secret.txt"))));

    // Threads that exist before the restriction: a plain one, and an async runtime's worker.
    let (go_plain, wait_plain) = mpsc::channel::<()>();
    let (plain_out, plain_results) = mpsc::channel::<Vec<String>>();
    let (d, a) = (dir.clone(), abstract_name.clone());
    let plain = std::thread::spawn(move || {
        wait_plain.recv().unwrap();
        plain_out.send(probes("old_thread", &d, allowed_port, denied_port, &a)).unwrap();
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let (go_worker, wait_worker) = tokio::sync::oneshot::channel::<()>();
    let (d, a) = (dir.clone(), abstract_name.clone());
    let worker = runtime.spawn(async move {
        wait_worker.await.unwrap();
        probes("runtime_worker", &d, allowed_port, denied_port, &a)
    });
    // Make sure the worker task is parked on a worker thread, not still queued.
    runtime.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(50)).await });

    let mut confinement = Confinement::supervisor();
    confinement.write.push(dir.join("allowed"));
    confinement.tcp_connect.push(allowed_port);
    match confine::apply(&confinement) {
        Ok(()) => println!("confine=ok"),
        Err(e) => {
            println!("confine=failed: {e}");
            return;
        }
    }

    let mut lines = probes("main", &dir, allowed_port, denied_port, &abstract_name);
    go_plain.send(()).unwrap();
    lines.extend(plain_results.recv().unwrap());
    plain.join().unwrap();
    go_worker.send(()).unwrap();
    lines.extend(runtime.block_on(worker).unwrap());
    let (d, a) = (dir.clone(), abstract_name.clone());
    lines.extend(std::thread::spawn(move || probes("new_thread", &d, allowed_port, denied_port, &a)).join().unwrap());
    for line in lines {
        println!("{line}");
    }
    std::io::stdout().flush().unwrap();
}

#[test]
fn a_confined_process_keeps_only_what_it_was_left() {
    if let Err(why) = confine::supported() {
        eprintln!("skipped: {why}");
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("confine-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("allowed")).unwrap();
    std::fs::write(dir.join("secret.txt"), b"id_ed25519").unwrap();

    // Things for the child to reach, or not: two TCP listeners and two UNIX sockets.
    let allowed = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let denied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let abstract_name = format!("bastion-confine-{}", std::process::id());
    let abstract_addr = <SocketAddr as std::os::linux::net::SocketAddrExt>::from_abstract_name(&abstract_name).unwrap();
    let _abstract_listener = UnixListener::bind_addr(&abstract_addr).unwrap();
    let _path_listener = UnixListener::bind(dir.join("other.sock")).unwrap();

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["confined_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_DIR, &dir)
        .env(ALLOWED_PORT, allowed.local_addr().unwrap().port().to_string())
        .env(DENIED_PORT, denied.local_addr().unwrap().port().to_string())
        .env(ABSTRACT_NAME, &abstract_name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = String::new();
    child.stdout.take().unwrap().read_to_string(&mut stdout).unwrap();
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    assert!(child.wait().unwrap().success(), "child failed:\n{stdout}\n{stderr}");

    let facts: BTreeMap<&str, &str> = stdout.lines().filter_map(|l| l.split_once('=')).collect();
    let get = |key: &str| *facts.get(key).unwrap_or_else(|| panic!("no `{key}` in:\n{stdout}\n{stderr}"));
    assert_eq!(get("before.read_secret"), "ok", "the file is readable until the process confines itself");
    assert_eq!(get("confine"), "ok");

    // The same answers on every thread, including the two that existed beforehand.
    for thread in ["main", "old_thread", "runtime_worker", "new_thread"] {
        let on = |probe: &str| get(&format!("{thread}.{probe}"));
        assert_eq!(on("read_secret"), "EACCES", "{thread}: files outside the kept trees");
        assert_eq!(on("list_outside"), "EACCES", "{thread}");
        assert_eq!(on("create_outside"), "EACCES", "{thread}");
        assert_eq!(on("write_allowed"), "ok", "{thread}: the kept tree stays writable");
        assert_eq!(on("rename_allowed"), "ok", "{thread}: atomic replace still works");
        assert_eq!(on("remove_allowed"), "ok", "{thread}");
        assert_eq!(on("symlink_allowed"), "EACCES", "{thread}: no symlinks, even where writing is allowed");
        assert_eq!(on("read_etc"), "ok", "{thread}: resolver configuration stays readable");
        assert_eq!(on("write_etc_like"), "EACCES", "{thread}: read-only means read-only");
        assert_eq!(on("exec"), "EACCES", "{thread}: nothing can be executed");
        assert_eq!(on("connect_allowed_port"), "ok", "{thread}: the relay's ports");
        assert_eq!(on("connect_other_port"), "EACCES", "{thread}: any other port");
        assert_eq!(on("bind"), "EACCES", "{thread}: no new listeners");
        assert_eq!(on("connect_abstract_socket"), "EPERM", "{thread}: other processes' abstract sockets");
        assert_eq!(on("signal_parent"), "ok", "{thread}: signals are not scoped (the game must be stoppable)");
        assert_eq!(on("resolve_localhost"), "ok", "{thread}: name resolution keeps working");
        // Mediating pathname sockets is newer than the rest.
        if kernel_abi() >= 9 {
            assert_eq!(on("connect_path_socket"), "EACCES", "{thread}: pathname sockets that were not kept");
        }
    }
    // Nothing leaked out of the kept tree while trying.
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(left.iter().all(|n| !n.to_string_lossy().contains("escape")), "{left:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
