//! End-to-end sandbox tests: the real bubblewrap → init (Landlock + seccomp) chain is started
//! with a Python payload in place of the JVM, and the payload reports what it could and could
//! not do. Every claim in docs/THREAT_MODEL.md that can be checked mechanically is checked here.
//!
//! Requirements: bubblewrap, python3, unprivileged user namespaces, Landlock ABI >= 6.

use std::{
    io::{Read, Write},
    net::Shutdown,
    path::{Path, PathBuf},
    time::Duration,
};

use bastion_mc::sandbox::landlock;
use bastion_mc::{
    config::SeccompMode,
    sandbox::{
        self, BaseOptions, Mount, Network, Resolv, SandboxSpec,
        cgroup::{self, Limits},
        netns::NetnsSpec,
        policy::{InitPolicy, SeccompPolicy, TcpPolicy},
    },
};
use tokio::io::AsyncReadExt;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_bastion-mc");

struct Outcome {
    code: Option<i32>,
    /// The signal that ended the sandbox's outermost process, if one did.
    signal: Option<i32>,
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

struct Probe {
    network: Network,
    seccomp: SeccompPolicy,
    stdin: Vec<u8>,
    extra_mounts: Vec<Mount>,
    write_paths: Vec<PathBuf>,
    tcp: TcpPolicy,
    scope_abstract: bool,
    udp_send: Option<Vec<u16>>,
    limits: Option<Limits>,
}

impl Default for Probe {
    fn default() -> Self {
        Probe {
            network: Network::None,
            seccomp: SeccompPolicy::default(),
            stdin: Vec::new(),
            extra_mounts: Vec::new(),
            write_paths: Vec::new(),
            tcp: TcpPolicy::Ports(vec![]),
            scope_abstract: true,
            udp_send: None,
            limits: None,
        }
    }
}

impl Probe {
    /// The sandbox description for a Python payload.
    fn spec(self, script: &str) -> (SandboxSpec, Vec<u8>) {
        let python = std::fs::canonicalize("/usr/bin/python3").expect("python3");
        let resolv = match self.network {
            Network::Filtered(_) => Resolv::Loopback,
            _ => Resolv::None,
        };
        let mut base = sandbox::base(&BaseOptions {
            tmp_size_mb: 64,
            jvm: None,
            exec_allow: std::slice::from_ref(&python),
            resolv,
            expose_timezone: false,
            trust: None,
        })
        .expect("base");
        base.mounts.extend(self.extra_mounts);
        base.landlock.write.extend(self.write_paths);
        base.landlock.tcp = self.tcp;
        base.landlock.scope_abstract_unix = self.scope_abstract;
        base.landlock.udp_send = self.udp_send.clone();
        // /bastion only exists because the init binary is bound there.
        let spec = SandboxSpec {
            mounts: base.mounts,
            network: self.network,
            cwd: "/tmp".into(),
            limits: self.limits,
            policy: InitPolicy {
                exec: python.clone(),
                argv: vec!["python3".into(), "-I".into(), "-c".into(), script.into()],
                env: base.env,
                landlock: base.landlock,
                seccomp: self.seccomp,
            },
        };
        (spec, self.stdin)
    }

    async fn run(self, script: &str) -> Outcome {
        let (spec, stdin) = self.spec(script);
        let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn sandbox");
        running.channel.write_all(&stdin).unwrap();
        running.channel.shutdown(Shutdown::Write).unwrap();
        drop(std::mem::take(&mut running.egress));

        let mut out = running.child.stdout.take().unwrap();
        let mut err = running.child.stderr.take().unwrap();
        let (mut so, mut se) = (String::new(), String::new());
        let work = async {
            let (a, b) = tokio::join!(out.read_to_string(&mut so), err.read_to_string(&mut se));
            a.unwrap();
            b.unwrap();
            running.child.wait().await.unwrap()
        };
        let status = tokio::time::timeout(Duration::from_secs(60), work).await.expect("sandbox payload timed out");
        let signal = std::os::unix::process::ExitStatusExt::signal(&status);
        Outcome { code: status.code(), signal, stdout: so, stderr: se }
    }
}

/// Shared Python helpers: raw syscalls with errno reporting.
const PRELUDE: &str = r#"
import ctypes, errno, os, socket, sys
libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long
def sc(nr, *a):
    ctypes.set_errno(0)
    r = libc.syscall(ctypes.c_long(nr), *[ctypes.c_long(x) if isinstance(x, int) else x for x in a])
    return "ok" if r >= 0 else errno.errorcode.get(ctypes.get_errno(), str(ctypes.get_errno()))
def attempt(f):
    try:
        f(); return "ok"
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
def show(k, v): print(f"{k}={v}", flush=True)
"#;

fn script(body: &str) -> String {
    format!("{PRELUDE}\n{body}")
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_environment_and_filesystem_view() {
    let o = Probe::default()
        .run(&script(
            r#"
show("uid", os.getuid()); show("gid", os.getgid())
show("hostname", socket.gethostname())
show("env", ",".join(sorted(os.environ)))
show("list_root", attempt(lambda: os.listdir("/")))
show("host_paths", ",".join(p for p in ("/home", "/root", "/var", "/opt", "/mnt", "/srv", "/boot", "/sys", "/etc/fstab", "/etc/shadow", "/etc/machine-id.real", "/run/user/1000/bus", "/run/dbus", "/tmp/.X11-unix") if os.path.lexists(p)))
show("top_level", ",".join(p for p in ("/bastion", "/bin", "/dev", "/etc", "/lib", "/lib64", "/proc", "/run", "/sbin", "/tmp", "/usr") if os.path.lexists(p)))
show("etc", ",".join(sorted(os.listdir("/etc"))))
show("passwd_users", ",".join(l.split(":")[0] for l in open("/etc/passwd")))
show("home_exists", os.path.exists("/home"))
show("shadow", attempt(lambda: open("/etc/shadow")))
show("runtime", ",".join(os.listdir("/run/user/1000")))
show("pids", ",".join(sorted(p for p in os.listdir("/proc") if p.isdigit())))
st = {l.split(":")[0]: l.split(":")[1].strip() for l in open("/proc/self/status") if ":" in l}
for k in ("NoNewPrivs", "Seccomp", "Seccomp_filters", "CapEff", "CapPrm", "CapBnd", "CapAmb"): show(k, st[k])
import resource
show("core", resource.getrlimit(resource.RLIMIT_CORE))
show("fds", ",".join(sorted(os.listdir("/proc/self/fd"), key=int)))
show("cmdline_of_pid1", open("/proc/1/cmdline","rb").read().replace(b"\0", b" ").decode()[:60])
"#,
        ))
        .await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.line("uid"), "1000");
    assert_eq!(o.line("gid"), "1000");
    assert_eq!(o.line("hostname"), "bastion");
    // The whole environment: nothing inherited from the launcher's.
    assert_eq!(o.line("env"), "HOME,LANG,LOGNAME,PATH,TMPDIR,USER,XDG_RUNTIME_DIR");
    assert_eq!(o.line("list_root"), "EACCES", "the root directory itself is not even listable");
    assert_eq!(o.line("host_paths"), "", "no host path may exist in the sandbox");
    assert_eq!(o.line("top_level"), "/bastion,/bin,/dev,/etc,/lib,/lib64,/proc,/run,/sbin,/tmp,/usr");
    assert_eq!(o.line("home_exists"), "False");
    assert_eq!(o.line("shadow"), "ENOENT");
    // Synthetic /etc: no real account names, no real machine-id (it is random per launch).
    assert_eq!(o.line("passwd_users"), "root,player,nobody");
    for entry in o.line("etc").split(',') {
        assert!(
            ["ca-certificates", "group", "hosts", "ld.so.cache", "machine-id", "nsswitch.conf", "passwd", "ssl"]
                .contains(&entry),
            "unexpected /etc entry {entry}"
        );
    }
    assert_eq!(o.line("runtime"), "", "runtime dir must start empty");
    // Fresh PID namespace: bwrap is 1, the payload is 2.
    assert_eq!(o.line("pids"), "1,2");
    assert_eq!(o.line("NoNewPrivs"), "1");
    assert_eq!(o.line("Seccomp"), "2");
    assert_eq!(o.line("Seccomp_filters"), "2", "tripwire + allowlist");
    for cap in ["CapEff", "CapPrm", "CapBnd", "CapAmb"] {
        assert_eq!(o.line(cap), "0000000000000000", "{cap}");
    }
    assert_eq!(o.line("core"), "(1, 1)", "RLIMIT_CORE must be exactly 1");
    // stdin/stdout/stderr plus the directory handle used for the listing itself.
    assert_eq!(o.line("fds"), "0,1,2,3", "no launcher descriptor may leak into the payload");
    assert!(o.line("cmdline_of_pid1").starts_with("/usr/bin/bwrap --args "), "{}", o.line("cmdline_of_pid1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn landlock_denies_what_file_permissions_would_allow() {
    let o = Probe::default()
        .run(&script(
            r##"
# "/" and "/run" are tmpfs owned by us: ordinary permissions would allow all of these.
show("mkdir_root", attempt(lambda: os.mkdir("/planted")))
show("write_run", attempt(lambda: open("/run/x", "w")))
show("write_tmp", attempt(lambda: open("/tmp/ok", "w").write("x")))
show("symlink_tmp", attempt(lambda: os.symlink("/etc/passwd", "/tmp/link")))
show("fifo_tmp", attempt(lambda: os.mkfifo("/tmp/fifo")))
s = socket.socket(socket.AF_UNIX); show("unix_sock_tmp", attempt(lambda: s.bind("/tmp/sock")))
show("write_usr", attempt(lambda: open("/usr/planted", "w")))
show("exec_sh", attempt(lambda: os.execv("/usr/bin/sh", ["sh", "-c", "true"])))
show("exec_tmp", attempt(lambda: (open("/tmp/x.sh","w").write("#!/usr/bin/sh\n"), os.chmod("/tmp/x.sh", 0o755), os.execv("/tmp/x.sh", ["x"]))))
import subprocess
show("subprocess", attempt(lambda: subprocess.run(["/usr/bin/id"], check=True)))
show("tty_ioctl", attempt(lambda: __import__("fcntl").ioctl(os.open("/dev/tty", os.O_RDWR), 0x5413, b"\0"*8)))
show("null_rw", attempt(lambda: os.write(os.open("/dev/null", os.O_WRONLY), b"x")))
show("signal_pid1", attempt(lambda: os.kill(1, 0)))
show("signal_self", attempt(lambda: os.kill(os.getpid(), 0)))
t = socket.socket(); show("tcp_bind", attempt(lambda: t.bind(("127.0.0.1", 9999))))
c = socket.socket(); show("tcp_connect", attempt(lambda: c.connect(("127.0.0.1", 8080))))
"##,
        ))
        .await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.line("mkdir_root"), "EACCES");
    assert_eq!(o.line("write_run"), "EACCES");
    assert_eq!(o.line("write_tmp"), "ok");
    // seccomp answers these before Landlock is consulted; Landlock alone is checked below.
    assert_eq!(o.line("symlink_tmp"), "ENOSYS", "no symlink creation anywhere");
    assert_eq!(o.line("fifo_tmp"), "ENOSYS");
    assert_eq!(o.line("unix_sock_tmp"), "EACCES");
    assert!(matches!(o.line("write_usr"), "EACCES" | "EROFS"));
    assert_eq!(o.line("exec_sh"), "EACCES", "a shell must not be executable");
    assert_eq!(o.line("exec_tmp"), "EACCES", "nothing written by the payload may be executed");
    assert_eq!(o.line("subprocess"), "EACCES");
    assert_ne!(o.line("tty_ioctl"), "ok");
    assert_eq!(o.line("null_rw"), "ok");
    assert_eq!(o.line("signal_pid1"), "EPERM", "no signals to processes outside the Landlock domain");
    assert_eq!(o.line("signal_self"), "ok");
    assert_eq!(o.line("tcp_bind"), "EACCES");
    assert_eq!(
        o.line("tcp_connect"),
        "EACCES",
        "TCP to a port that is not allowlisted is refused before it is attempted"
    );

    // Same restrictions with seccomp reduced to logging: Landlock must hold on its own.
    let log = SeccompPolicy { mode: SeccompMode::Log, ..Default::default() };
    let o = Probe { seccomp: log, ..Default::default() }
        .run(&script(
            r#"
show("symlink_tmp", attempt(lambda: os.symlink("/etc/passwd", "/tmp/link")))
show("fifo_tmp", attempt(lambda: os.mkfifo("/tmp/fifo")))
show("mkdir_root", attempt(lambda: os.mkdir("/planted")))
show("exec_sh", attempt(lambda: os.execv("/usr/bin/sh", ["sh", "-c", "true"])))
show("unshare_user", sc(272, 0x10000000))
"#,
        ))
        .await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    for k in ["symlink_tmp", "fifo_tmp", "mkdir_root", "exec_sh"] {
        assert_eq!(o.line(k), "EACCES", "{k}: Landlock alone must deny this");
    }
    // …and bubblewrap's --disable-userns holds without seccomp, too.
    assert_ne!(o.line("unshare_user"), "ok", "nested user namespaces must be impossible even without seccomp");
}

#[tokio::test(flavor = "multi_thread")]
async fn seccomp_allowlist_blocks_dangerous_syscalls_but_not_normal_operation() {
    let o = Probe::default()
        .run(&script(
            r#"
import threading, mmap
show("unshare_user", sc(272, 0x10000000))
show("unshare_net", sc(272, 0x40000000))
show("clone_newuser", sc(56, 0x10000000 | 17, 0, 0, 0, 0))
show("clone3", sc(435, 0, 0))
show("io_uring_setup", sc(425, 4, 0))
show("userfaultfd", sc(323, 0))
show("perf_event_open", sc(298, 0, 0, -1, -1, 0))
show("keyctl", sc(250, 0, -3, 0))            # KEYCTL_GET_KEYRING_ID, session keyring
show("add_key", sc(248, 0, 0, 0, 0, -3))
show("request_key", sc(249, 0, 0, 0, -3))
show("open_by_handle_at", "skipped-tripwire")
show("setuid", sc(105, 0))
show("mknod", sc(133, 0, 0, 0))
show("seccomp", sc(317, 0, 0, 0))
show("landlock_create", sc(444, 0, 0, 1))
show("pidfd_getfd", sc(438, 0, 0, 0))
show("netlink", attempt(lambda: socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0)))
show("packet", attempt(lambda: socket.socket(socket.AF_PACKET, socket.SOCK_RAW, 0)))
show("af_alg", attempt(lambda: socket.socket(38, socket.SOCK_SEQPACKET, 0)))
show("inet", attempt(lambda: socket.socket(socket.AF_INET, socket.SOCK_STREAM, 0)))
show("unix", attempt(lambda: socket.socket(socket.AF_UNIX, socket.SOCK_STREAM, 0)))
show("tiocsti", sc(16, 0, 0x5412, 0))
# x32 ABI: same kernel entry, syscall number with bit 30 set. getpid is 39.
show("x32_getpid", sc(0x40000000 | 39))
show("x32_ptrace_nr", sc(0x40000000 | 521, 0, 0, 0, 0))
# Normal operation must be unaffected: threads (glibc falls back from clone3), mmap, exec memory.
t = threading.Thread(target=lambda: None); t.start(); t.join(); show("thread", "ok")
m = mmap.mmap(-1, 4096, prot=mmap.PROT_READ | mmap.PROT_WRITE | mmap.PROT_EXEC); show("mmap_exec", "ok")
show("prctl_set_name", sc(157, 15, ctypes.c_char_p(b"probe")))
show("prctl_set_ptracer", sc(157, 0x59616d61, -1))   # PR_SET_PTRACER, PR_SET_PTRACER_ANY
show("getrandom", "ok" if os.getrandom(8) else "fail")
"#,
        ))
        .await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    for denied in [
        "unshare_user",
        "unshare_net",
        "clone_newuser",
        "clone3",
        "io_uring_setup",
        "userfaultfd",
        "perf_event_open",
        "keyctl",
        "add_key",
        "request_key",
        "setuid",
        "mknod",
        "seccomp",
        "landlock_create",
        "pidfd_getfd",
        "tiocsti",
        "x32_getpid",
        "x32_ptrace_nr",
        "prctl_set_ptracer",
    ] {
        assert_eq!(o.line(denied), "ENOSYS", "{denied} must be refused by the allowlist");
    }
    for denied in ["netlink", "packet", "af_alg"] {
        assert_eq!(o.line(denied), "ENOSYS", "socket family {denied}");
    }
    for fine in ["inet", "unix", "thread", "mmap_exec", "prctl_set_name", "getrandom"] {
        assert_eq!(o.line(fine), "ok", "{fine} must keep working");
    }
}

/// The x32 path really exists on this kernel; without the allowlist it would bypass a denylist.
#[test]
fn x32_entry_point_is_live_on_the_host() {
    let nr: libc::c_long = 0x4000_0000 | 39; // x32 getpid
    // SAFETY: getpid has no arguments and no side effects.
    let r = unsafe { libc::syscall(nr) };
    if r < 0 {
        eprintln!("note: kernel built without CONFIG_X86_X32_ABI; x32 bypass not applicable here");
    } else {
        assert_eq!(r as u32, std::process::id(), "x32 getpid should return our pid");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tripwire_kills_the_process_on_exploit_only_syscalls() {
    for (name, call) in [
        ("ptrace", "sc(101, 0, 0, 0, 0)"),
        ("bpf", "sc(321, 0, 0, 0)"),
        ("mount", "sc(165, 0, 0, 0, 0, 0)"),
        ("process_vm_readv", "sc(310, 1, 0, 0, 0, 0, 0)"),
        ("setns", "sc(308, 0, 0)"),
        ("kexec_load", "sc(246, 0, 0, 0, 0)"),
    ] {
        let o =
            Probe::default().run(&script(&format!("show('before', 'ok')\n{call}\nshow('after', 'survived')"))).await;
        assert_eq!(o.line("before"), "ok");
        assert!(!o.stdout.contains("after=survived"), "{name}: process survived a tripwire syscall");
        // bubblewrap reports death-by-signal as 128 + signal; SIGSYS is 31.
        assert_eq!(o.code, Some(128 + libc::SIGSYS), "{name}: expected SIGSYS, stderr: {}", o.stderr);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tripwire_can_be_disabled_and_log_mode_is_permissive() {
    let quiet = SeccompPolicy { tripwire: false, ..Default::default() };
    let o = Probe { seccomp: quiet, ..Default::default() }.run(&script("show('ptrace', sc(101, 0, 0, 0, 0))")).await;
    assert_eq!(o.code, Some(0));
    assert_eq!(o.line("ptrace"), "ENOSYS", "without the tripwire the call is still denied");

    let log = SeccompPolicy { mode: SeccompMode::Log, ..Default::default() };
    let o = Probe { seccomp: log, ..Default::default() }.run(&script("show('unshare_files', sc(272, 0x400))")).await;
    assert_eq!(o.code, Some(0));
    assert_eq!(o.line("unshare_files"), "ok", "log mode only records");
}

/// The trap in "keep the refresh token in the kernel keyring": keyrings are not namespaced.
/// Both defences are tested separately so that neither silently carries the other.
#[tokio::test(flavor = "multi_thread")]
async fn kernel_keyring_is_unreachable_from_the_sandbox() {
    use std::process::Command;
    let secret = format!("bastion-test-secret-{}", std::process::id());
    let add = Command::new("keyctl").args(["add", "user", "bastion-test-key", &secret, "@s"]).output().expect("keyctl");
    assert!(add.status.success(), "keyctl add failed: {}", String::from_utf8_lossy(&add.stderr));
    let key_id = String::from_utf8(add.stdout).unwrap().trim().to_string();
    let read_key = format!(
        r#"
buf = ctypes.create_string_buffer(256)
ctypes.set_errno(0)
n = libc.syscall(ctypes.c_long(250), ctypes.c_long(11), ctypes.c_long({key_id}), buf, ctypes.c_long(256))  # KEYCTL_READ
show("read", buf.raw[:n].decode() if n > 0 else errno.errorcode.get(ctypes.get_errno()))
"#
    );

    // Layer 1 (seccomp): the syscall does not exist.
    let o = Probe::default().run(&script(&read_key)).await;
    assert_eq!(o.line("read"), "ENOSYS");

    // Layer 2 (fresh session keyring): with seccomp only logging, the key is still not ours.
    let log = SeccompPolicy { mode: SeccompMode::Log, ..Default::default() };
    let o = Probe { seccomp: log, ..Default::default() }.run(&script(&read_key)).await;
    assert_ne!(o.line("read"), secret, "the sandbox read a key from the user's session keyring");
    assert_eq!(o.line("read"), "EACCES");

    // Control: outside the sandbox the same key is readable, so the test is meaningful.
    let host = Command::new("keyctl").args(["print", &key_id]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&host.stdout).trim(), secret);
    let _ = Command::new("keyctl").args(["unlink", &key_id, "@s"]).output();
}

#[tokio::test(flavor = "multi_thread")]
async fn verified_files_are_bound_by_descriptor_and_immutable() {
    let dir = std::env::temp_dir().join(format!("bastion-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let jar = dir.join("lib.jar");
    std::fs::write(&jar, b"VERIFIED-CONTENT").unwrap();
    let file = std::fs::File::open(&jar).unwrap();
    // After "verification", the path is swapped for something else. The sandbox must still
    // see the descriptor that was verified.
    std::fs::rename(&jar, dir.join("old.jar")).unwrap();
    std::fs::write(&jar, b"SWAPPED-BY-ATTACKER").unwrap();

    let o = Probe {
        extra_mounts: vec![
            Mount::RoBindFd { file, dst: "/bastion/lib/0001-lib.jar".into() },
            Mount::RoData { data: b"generated".to_vec(), dst: "/bastion/boot/gen.txt".into(), mode: 0o444 },
        ],
        ..Default::default()
    }
    .run(&script(
        r#"
show("jar", open("/bastion/lib/0001-lib.jar").read())
show("gen", open("/bastion/boot/gen.txt").read())
show("write_jar", attempt(lambda: open("/bastion/lib/0001-lib.jar", "w")))
show("write_gen", attempt(lambda: open("/bastion/boot/gen.txt", "a")))
show("unlink_jar", attempt(lambda: os.unlink("/bastion/lib/0001-lib.jar")))
show("init_readable", os.path.getsize("/bastion/init") > 100000)
show("write_init", attempt(lambda: open("/bastion/init", "ab")))
"#,
    ))
    .await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.line("jar"), "VERIFIED-CONTENT");
    assert_eq!(o.line("gen"), "generated");
    // Which refusal wins depends on kernel check order: read-only mount (EROFS), Landlock
    // (EACCES), busy mount point (EBUSY), or "text file busy" when the launcher binary is
    // being executed by another sandbox at that instant (ETXTBSY). All are refusals.
    for w in ["write_jar", "write_gen", "unlink_jar", "write_init"] {
        assert!(matches!(o.line(w), "EACCES" | "EROFS" | "EBUSY" | "ETXTBSY"), "{w} = {}", o.line(w));
    }
    assert_eq!(o.line("init_readable"), "True");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn network_none_has_no_way_out() {
    let o = Probe { tcp: TcpPolicy::AnyConnect, ..Default::default() }
        .run(&script(
            r#"
show("ifaces", ",".join(sorted(l.split(":")[0].strip() for l in open("/proc/net/dev").read().splitlines()[2:])))
# What java.net.NetworkInterface would report: names via if_nameindex, hardware addresses via ioctl.
import fcntl, struct
try:
    names = sorted(n for _, n in socket.if_nameindex())
    show("if_names", ",".join(names))
except OSError as e:
    names = ["lo"]          # netlink is not available at all (seccomp): ask about loopback by name
    show("if_names", errno.errorcode.get(e.errno, str(e.errno)))
names += ["eth0", "enp1s0", "wlan0"]
q = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
def mac(n):
    try:
        return fcntl.ioctl(q.fileno(), 0x8927, struct.pack("256s", n.encode()))[18:24].hex()
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
show("macs", ",".join(sorted(set(mac(n) for n in names))))
c = socket.socket(); c.settimeout(3); show("connect_public", attempt(lambda: c.connect(("1.1.1.1", 443))))
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); show("udp_public", attempt(lambda: u.sendto(b"x", ("1.1.1.1", 53))))
show("resolve", attempt(lambda: socket.getaddrinfo("example.com", 443)))
"#,
        ))
        .await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.line("ifaces"), "lo");
    // Enumeration over netlink (what getifaddrs and java.net.NetworkInterface use) is not
    // available at all; where it is, it must show loopback only.
    assert!(matches!(o.line("if_names"), "lo" | "ENOSYS" | "EPERM" | "EAFNOSUPPORT"), "{}", o.line("if_names"));
    // Asked by name: loopback has the all-zero address, and no other interface exists.
    assert_eq!(o.line("macs"), "000000000000,ENODEV", "no hardware address to fingerprint");
    assert_eq!(o.line("connect_public"), "ENETUNREACH");
    assert_eq!(o.line("udp_public"), "ENETUNREACH");
    assert_ne!(o.line("resolve"), "ok");
}

/// Filtered mode: the sandbox's only exits are sockets that the launcher holds.
#[tokio::test(flavor = "multi_thread")]
async fn filtered_network_delivers_only_to_launcher_held_sockets() {
    use std::net::{Ipv4Addr, TcpListener, UdpSocket};
    let python = std::fs::canonicalize("/usr/bin/python3").unwrap();
    let ns = NetnsSpec { tcp_ports: vec![443, 25565], dns: true, local_addrs: vec![Ipv4Addr::new(203, 0, 113, 7)] };
    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: std::slice::from_ref(&python),
        resolv: Resolv::Loopback,
        expose_timezone: false,
        trust: None,
    })
    .unwrap();
    base.landlock.tcp = TcpPolicy::Ports(vec![443, 25565]);
    base.landlock.udp_send = Some(vec![53]);
    let payload = script(
        r#"
show("ifaces", ",".join(sorted(l.split(":")[0].strip() for l in open("/proc/net/dev").read().splitlines()[2:])))
# What java.net.NetworkInterface would report: names via if_nameindex, hardware addresses via ioctl.
import fcntl, struct
try:
    names = sorted(n for _, n in socket.if_nameindex())
    show("if_names", ",".join(names))
except OSError as e:
    names = ["lo"]          # netlink is not available at all (seccomp): ask about loopback by name
    show("if_names", errno.errorcode.get(e.errno, str(e.errno)))
names += ["eth0", "enp1s0", "wlan0"]
q = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
def mac(n):
    try:
        return fcntl.ioctl(q.fileno(), 0x8927, struct.pack("256s", n.encode()))[18:24].hex()
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
show("macs", ",".join(sorted(set(mac(n) for n in names))))
def talk(ip, port):
    c = socket.socket(); c.settimeout(5)
    try:
        c.connect((ip, port)); c.sendall(b"hello"); return c.recv(64).decode()
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
show("fake_ip_443", talk("127.64.0.9", 443))
show("fake_ip_game", talk("127.99.1.2", 25565))
show("literal_ip", talk("203.0.113.7", 25565))
show("other_literal", talk("203.0.113.8", 25565))
show("public", talk("1.1.1.1", 443))
show("port_not_allowlisted", talk("127.64.0.9", 8443))
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); u.settimeout(5)
u.sendto(b"query", ("127.0.0.1", 53)); show("dns", u.recv(64).decode())
show("resolv_conf", open("/etc/resolv.conf").read().split()[1])
t = socket.socket(); show("bind_443", attempt(lambda: t.bind(("0.0.0.0", 443))))
# UDP beyond the DNS port (Landlock ABI 10): not to another port, not from a chosen port.
v = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
show("udp_other_port", attempt(lambda: v.sendto(b"x", ("127.0.0.1", 5353))))
show("udp_connect_other", attempt(lambda: v.connect(("127.0.0.1", 4445))))
w = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
show("udp_bind_chosen", attempt(lambda: w.bind(("127.0.0.1", 4445))))
show("udp_bind_ephemeral", attempt(lambda: w.bind(("127.0.0.1", 0))))
show("udp6_other_port", attempt(lambda: socket.socket(socket.AF_INET6, socket.SOCK_DGRAM).sendto(b"x", ("::1", 5353))))
"#,
    );
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::Filtered(ns),
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: python,
            argv: vec!["python3".into(), "-I".into(), "-c".into(), payload],
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
    running.channel.shutdown(Shutdown::Write).unwrap();
    let mut socks = std::mem::take(&mut running.egress).into_iter();
    let dns = UdpSocket::from(socks.next().unwrap());
    let listeners: Vec<TcpListener> = socks.map(TcpListener::from).collect();
    assert_eq!(listeners.len(), 2);

    // Host-side "proxy": report which destination the sandbox thought it was talking to.
    for l in listeners {
        std::thread::spawn(move || {
            while let Ok((mut c, _)) = l.accept() {
                let mut buf = [0u8; 5];
                let _ = c.read_exact(&mut buf);
                let _ = c.write_all(format!("dst={}", c.local_addr().unwrap()).as_bytes());
            }
        });
    }
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        while let Ok((_, from)) = dns.recv_from(&mut buf) {
            let _ = dns.send_to(b"answered-by-launcher", from);
        }
    });

    let mut so = String::new();
    let mut se = String::new();
    let mut out = running.child.stdout.take().unwrap();
    let mut err = running.child.stderr.take().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        let (a, b) = tokio::join!(out.read_to_string(&mut so), err.read_to_string(&mut se));
        a.unwrap();
        b.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("timeout");
    let o = Outcome { code: status.code(), signal: None, stdout: so, stderr: se };
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.line("ifaces"), "lo", "no interface but loopback exists");
    // Enumeration over netlink (what getifaddrs and java.net.NetworkInterface use) is not
    // available at all; where it is, it must show loopback only.
    assert!(matches!(o.line("if_names"), "lo" | "ENOSYS" | "EPERM" | "EAFNOSUPPORT"), "{}", o.line("if_names"));
    // Asked by name: loopback has the all-zero address, and no other interface exists.
    assert_eq!(o.line("macs"), "000000000000,ENODEV", "no hardware address to fingerprint");
    // Any 127/8 address on an allowlisted port reaches the launcher, which learns the address.
    assert_eq!(o.line("fake_ip_443"), "dst=127.64.0.9:443");
    assert_eq!(o.line("fake_ip_game"), "dst=127.99.1.2:25565");
    // An allowlisted literal IP is captured too…
    assert_eq!(o.line("literal_ip"), "dst=203.0.113.7:25565");
    // …while its neighbour and the public internet are simply unreachable.
    assert_eq!(o.line("other_literal"), "ENETUNREACH");
    assert_eq!(o.line("public"), "ENETUNREACH");
    assert_eq!(o.line("port_not_allowlisted"), "EACCES");
    assert_eq!(o.line("dns"), "answered-by-launcher");
    assert_eq!(o.line("resolv_conf"), "127.0.0.1");
    assert_eq!(o.line("bind_443"), "EACCES", "the payload itself has no capability over the namespace");
    if landlock::kernel_abi() >= landlock::UDP_ABI {
        assert_eq!(o.line("udp_other_port"), "EACCES", "datagrams go to the DNS port only");
        assert_eq!(o.line("udp_connect_other"), "EACCES");
        assert_eq!(o.line("udp_bind_chosen"), "EACCES", "no listening on a chosen UDP port");
        assert_eq!(o.line("udp_bind_ephemeral"), "ok", "an ephemeral source port is all a client needs");
        assert_eq!(o.line("udp6_other_port"), "EACCES");
    } else {
        eprintln!("note: Landlock ABI {} has no UDP rules; only the namespace was tested", landlock::kernel_abi());
    }
}

/// The launch frame arrives on fd 0, which is a socket: not visible in the command line, the
/// environment, or re-openable through /proc by another process.
#[tokio::test(flavor = "multi_thread")]
async fn launch_channel_is_not_observable() {
    let token = "eyJhbGciOi.SECRET-ACCESS-TOKEN.signature";
    let o = Probe { stdin: token.as_bytes().to_vec(), ..Default::default() }
        .run(&script(
            r#"
import stat
got = sys.stdin.buffer.read().decode()
show("received", got)
show("fd0_is_socket", stat.S_ISSOCK(os.fstat(0).st_mode))
show("in_cmdline", got in open("/proc/self/cmdline", "rb").read().decode(errors="replace"))
show("in_environ", got in open("/proc/self/environ", "rb").read().decode(errors="replace"))
show("reopen_fd0", attempt(lambda: os.open("/proc/self/fd/0", os.O_RDONLY)))
"#,
        ))
        .await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.line("received"), token);
    assert_eq!(o.line("fd0_is_socket"), "True");
    assert_eq!(o.line("in_cmdline"), "False");
    assert_eq!(o.line("in_environ"), "False");
    assert_eq!(o.line("reopen_fd0"), "ENXIO", "a socket cannot be re-opened through procfs");
}

#[tokio::test(flavor = "multi_thread")]
async fn init_refuses_to_run_with_a_broken_policy() {
    // A policy naming a path that does not exist must stop the launch, not skip the rule.
    let python = std::fs::canonicalize("/usr/bin/python3").unwrap();
    let mut base = sandbox::base(&BaseOptions {
        tmp_size_mb: 64,
        jvm: None,
        exec_allow: std::slice::from_ref(&python),
        resolv: Resolv::None,
        expose_timezone: false,
        trust: None,
    })
    .unwrap();
    base.landlock.write.push("/does/not/exist".into());
    let spec = SandboxSpec {
        mounts: base.mounts,
        network: Network::None,
        cwd: "/tmp".into(),
        limits: None,
        policy: InitPolicy {
            exec: python,
            argv: vec!["python3".into(), "-c".into(), "print('RAN')".into()],
            env: base.env,
            landlock: base.landlock,
            seccomp: SeccompPolicy::default(),
        },
    };
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).unwrap();
    let mut so = String::new();
    running.child.stdout.take().unwrap().read_to_string(&mut so).await.unwrap();
    let status = running.child.wait().await.unwrap();
    assert!(!so.contains("RAN"), "payload ran despite an unenforceable policy");
    assert_eq!(status.code(), Some(126));
}

/// Stopping: SIGTERM must reach the sandboxed program itself (not bubblewrap's in-namespace
/// init, which ignores it), and a program that refuses to exit is killed after the grace period.
#[tokio::test(flavor = "multi_thread")]
async fn stop_is_graceful_first_and_forceful_after_the_grace_period() {
    async fn start(body: &str) -> sandbox::Running {
        let python = std::fs::canonicalize("/usr/bin/python3").unwrap();
        let base = sandbox::base(&BaseOptions {
            tmp_size_mb: 64,
            jvm: None,
            exec_allow: std::slice::from_ref(&python),
            resolv: Resolv::None,
            expose_timezone: false,
            trust: None,
        })
        .unwrap();
        let spec = SandboxSpec {
            mounts: base.mounts,
            network: Network::None,
            cwd: "/tmp".into(),
            limits: None,
            policy: InitPolicy {
                exec: python,
                argv: vec!["python3".into(), "-I".into(), "-u".into(), "-c".into(), body.into()],
                env: base.env,
                landlock: base.landlock,
                seccomp: SeccompPolicy::default(),
            },
        };
        sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn")
    }
    async fn wait_ready(running: &mut sandbox::Running) -> tokio::process::ChildStdout {
        let mut out = running.child.stdout.take().unwrap();
        let mut buf = [0u8; 6];
        tokio::time::timeout(Duration::from_secs(30), out.read_exact(&mut buf)).await.expect("payload start").unwrap();
        assert_eq!(&buf, b"ready\n");
        out
    }

    // Cooperative program: handles SIGTERM like a JVM running its shutdown hooks.
    let mut running = start(
        "import signal, sys, time\n\
         def bye(*_):\n    print('clean shutdown', flush=True); sys.exit(0)\n\
         signal.signal(signal.SIGTERM, bye)\nprint('ready', flush=True)\ntime.sleep(120)\n",
    )
    .await;
    let mut out = wait_ready(&mut running).await;
    let pid = running.payload_pid().await.expect("payload pid");
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap();
    assert!(comm.starts_with("python"), "payload_pid must name the program, got {comm:?}");
    let began = std::time::Instant::now();
    let status = running.stop(Duration::from_secs(20)).await.unwrap();
    let mut rest = String::new();
    out.read_to_string(&mut rest).await.unwrap();
    assert!(rest.contains("clean shutdown"), "the program never saw SIGTERM: {rest:?}");
    assert_eq!(status.code(), Some(0), "status: {status:?}");
    assert!(began.elapsed() < Duration::from_secs(10), "graceful stop took {:?}", began.elapsed());

    // Uncooperative program: ignores SIGTERM, so the sandbox is killed after the grace period.
    let mut running = start("import signal, time\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\nprint('ready', flush=True)\ntime.sleep(120)\n").await;
    let _out = wait_ready(&mut running).await;
    let began = std::time::Instant::now();
    let status = running.stop(Duration::from_secs(2)).await.unwrap();
    assert!(!status.success());
    assert!(began.elapsed() >= Duration::from_secs(2) && began.elapsed() < Duration::from_secs(15));
}

/// What the payload can learn about the host from procfs. bubblewrap's in-namespace init
/// (PID 1) still holds descriptors whose contents name host paths; the payload must not be
/// able to reach them. (`mountinfo` is a known, documented leak: see the assertion text.)
#[tokio::test(flavor = "multi_thread")]
async fn procfs_does_not_hand_over_the_supervisor() {
    let dir = std::env::temp_dir().join(format!("bastion-procfs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let o = Probe {
        extra_mounts: vec![Mount::Bind { src: dir.clone(), dst: "/game".into() }],
        write_paths: vec!["/game".into()],
        ..Default::default()
    }
    .run(&script(
        r#"
fds = os.listdir("/proc/1/fd")   # the numbers are listable; nothing behind them may be
show("pid1_fd_links", ",".join(sorted({attempt(lambda: os.readlink(f"/proc/1/fd/{n}")) for n in fds})))
show("pid1_fd_open", ",".join(sorted({attempt(lambda: open(f"/proc/1/fd/{n}", "rb")) for n in fds})))
show("pid1_environ", attempt(lambda: open("/proc/1/environ", "rb").read()))
show("pid1_mem", attempt(lambda: open("/proc/1/mem", "rb")))
show("pid1_maps", attempt(lambda: open("/proc/1/maps").read()))
show("pid1_root", attempt(lambda: os.listdir("/proc/1/root")))
show("pid1_cwd", attempt(lambda: os.readlink("/proc/1/cwd")))
show("self_maps", attempt(lambda: open("/proc/self/maps").read()))
show("kcore", attempt(lambda: open("/proc/kcore", "rb")))
show("sysrq", attempt(lambda: open("/proc/sysrq-trigger", "w")))
show("write_proc_sys", attempt(lambda: open("/proc/sys/kernel/hostname", "w")))
show("kallsyms_zeroed", all(l.split()[0].strip("0") == "" for l in open("/proc/kallsyms").readlines()[:50]))
mi = open("/proc/self/mountinfo").read()
show("mountinfo_names_bind_source", "bastion-procfs-" in mi)
"#,
    ))
    .await;
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    // PID 1 is outside the payload's Landlock domain, so every ptrace-class access to it is
    // refused: what its descriptors point to, its environment, memory, maps, root and cwd.
    for k in ["pid1_fd_links", "pid1_fd_open", "pid1_environ", "pid1_mem", "pid1_maps", "pid1_root", "pid1_cwd"] {
        assert!(
            matches!(o.line(k), "EACCES" | "EPERM"),
            "{k} = {} — the payload reached bubblewrap's init process",
            o.line(k)
        );
    }
    assert_eq!(o.line("self_maps"), "ok");
    assert_ne!(o.line("kcore"), "ok");
    assert_ne!(o.line("sysrq"), "ok");
    assert_ne!(o.line("write_proc_sys"), "ok");
    assert_eq!(o.line("kallsyms_zeroed"), "True", "kernel addresses must not be visible");
    // Documented information leak (THREAT_MODEL J1): the kernel lists the source path of
    // every bind mount in mountinfo, so code in the sandbox can learn host directory names.
    assert_eq!(o.line("mountinfo_names_bind_source"), "True", "if this ever becomes False, update the threat model");
    std::fs::remove_dir_all(dir).unwrap();
}

/// X11 clients in each network mode (needs an X session and `glxinfo`; `BASTION_X11_TESTS=1`).
/// libxcb tries the server's *abstract* socket before the socket file when the display is
/// given as `:N`, and gives up if Landlock's abstract-socket scoping refuses that. The game
/// is therefore given the socket's path as its DISPLAY, which is only ever opened as a file:
/// an X11 client then connects in both kinds of network namespace with scoping on.
#[tokio::test(flavor = "multi_thread")]
async fn x11_client_connects_in_each_network_mode() {
    if std::env::var_os("BASTION_X11_TESTS").is_none() {
        eprintln!("skipped: set BASTION_X11_TESTS=1 (needs a running X session and glxinfo)");
        return;
    }
    use bastion_mc::{config::SandboxConfig, paths::Paths, sandbox::display};
    let glxinfo = std::fs::canonicalize("/usr/bin/glxinfo").expect("glxinfo");
    let paths = Paths::from_env().unwrap();
    let cfg = SandboxConfig {
        allow_x11: true,
        display: bastion_mc::config::DisplayPref::X11,
        audio: bastion_mc::config::AudioPref::None,
        // This test is about the session's display handed over directly. Where gamescope is
        // installed the plan would otherwise be a nested compositor — which is not started
        // here, so the client would have no display at all.
        nested_compositor: bastion_mc::config::IsolationMode::Off,
        ..Default::default()
    };

    for (label, open_network, scope, expect_ok) in [
        ("filtered network, abstract-socket scoping on", false, true, true),
        ("open network, abstract-socket scoping on", true, true, true),
    ] {
        let desktop = display::plan(&cfg, &paths, "test").expect("display plan");
        let mut base = sandbox::base(&BaseOptions {
            tmp_size_mb: 64,
            jvm: None,
            exec_allow: std::slice::from_ref(&glxinfo),
            resolv: Resolv::None,
            expose_timezone: false,
            trust: None,
        })
        .unwrap();
        base.mounts.extend(desktop.mounts);
        base.landlock.devices.extend(desktop.devices);
        base.landlock.unix_sockets.extend(desktop.sockets);
        base.landlock.scope_abstract_unix = scope;
        base.landlock.tcp = if open_network { TcpPolicy::AnyConnect } else { TcpPolicy::Ports(vec![]) };
        let mut env = base.env;
        env.extend(desktop.env);
        let spec = SandboxSpec {
            mounts: base.mounts,
            network: if open_network { Network::Open } else { Network::None },
            cwd: "/tmp".into(),
            limits: None,
            policy: InitPolicy {
                exec: glxinfo.clone(),
                argv: vec!["glxinfo".into(), "-B".into()],
                env,
                landlock: base.landlock,
                seccomp: SeccompPolicy::default(),
            },
        };
        let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn");
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
        let renderer = so.lines().find(|l| l.contains("OpenGL renderer")).unwrap_or("").trim().to_string();
        println!("{label}: exit {:?}; {renderer}; stderr: {}", status.code(), se.lines().next().unwrap_or(""));
        assert_eq!(status.success(), expect_ok, "{label}\nstdout: {so}\nstderr: {se}");
        if expect_ok {
            assert!(so.contains("direct rendering: Yes"), "{label}: no direct rendering\n{so}");
        }
    }
}

// ---------------------------------------------------------------- resource limits

/// Skip (with a note) only where there is no systemd user manager at all, as in a package
/// build chroot. Where there is one, failing to get a scope is a failure, not a skip.
async fn scopes_available() -> bool {
    if let Err(why) = cgroup::locate() {
        eprintln!("skipped: {why}");
        return false;
    }
    cgroup::preflight(true).await.expect("a user manager is running, so a scope with limits must be obtainable");
    true
}

fn small_limits(tag: &str, memory_mb: u64, tasks: u32) -> Limits {
    Limits {
        memory_max_bytes: memory_mb << 20,
        tasks_max: tasks,
        cpu_quota_percent: Some(150),
        unit: format!("bastion-mc-test-{tag}-{}", std::process::id()),
    }
}

/// The control group of `pid`, as a path under /sys/fs/cgroup.
fn cgroup_dir(pid: u32) -> PathBuf {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).expect("reading the process's cgroup");
    let path = text.lines().find_map(|l| l.strip_prefix("0::")).expect("cgroup v2");
    PathBuf::from(format!("/sys/fs/cgroup{path}"))
}

/// With limits requested, the sandbox runs inside a transient systemd scope: the limits
/// can be read back from the kernel, and a thread bomb stops at the task limit.
#[tokio::test]
async fn resource_limits_put_the_sandbox_in_a_scope_and_cap_its_tasks() {
    if !scopes_available().await {
        return;
    }
    let limits = small_limits("tasks", 256, 48);
    let unit = limits.unit.clone();
    let body = r#"
import threading, time
show("ready", 1)
sys.stdin.readline()                       # until the test has looked at the cgroup
started = 0
try:
    while started < 400:
        threading.Thread(target=time.sleep, args=(20,), daemon=True).start()
        started += 1
except RuntimeError:
    pass
show("threads", started)
"#;
    let (spec, _) = Probe { limits: Some(limits.clone()), ..Default::default() }.spec(&script(body));
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn sandbox in a scope");
    let mut out = tokio::io::BufReader::new(running.child.stdout.take().unwrap());
    let mut err = running.child.stderr.take().unwrap();

    use tokio::io::AsyncBufReadExt;
    let mut first = String::new();
    tokio::time::timeout(Duration::from_secs(60), out.read_line(&mut first)).await.expect("payload start").unwrap();
    assert_eq!(first.trim(), "ready=1", "stderr: {}", {
        let mut text = String::new();
        let _ = tokio::time::timeout(Duration::from_secs(2), err.read_to_string(&mut text)).await;
        text
    });

    // Seen from outside: the scope exists under that name and carries exactly these limits.
    let dir = cgroup_dir(running.child.id().expect("sandbox pid"));
    assert_eq!(dir.file_name().unwrap().to_str().unwrap(), format!("{unit}.scope"), "{}", dir.display());
    let knob = |name: &str| {
        std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}")).trim().to_string()
    };
    assert_eq!(knob("pids.max"), "48");
    assert_eq!(knob("memory.max"), (256u64 << 20).to_string());
    assert_eq!(knob("memory.swap.max"), "0", "nothing of the game may be written to swap");
    // The cpu controller is delegated to user managers on current systemd; where it is not,
    // the file does not exist and systemd ignores the property.
    if dir.join("cpu.max").exists() {
        assert_eq!(knob("cpu.max"), "150000 100000", "CPUQuota=150%");
    }
    let members: Vec<u32> = knob("cgroup.procs").lines().filter_map(|l| l.parse().ok()).collect();
    assert!(members.len() >= 2, "bubblewrap and the payload are both in the scope: {members:?}");

    // What the launcher itself does after starting the game: it reads the limits back, for
    // the program inside the sandbox, and believes nothing else.
    let payload = running.payload_pid().await.expect("the sandboxed program's host pid") as u32;
    assert_eq!(cgroup::verify(payload, &limits), Ok(()), "the payload is in the scope with these limits");
    assert_eq!(cgroup::verify(running.child.id().unwrap(), &limits), Ok(()));
    // Limits that were asked for but are not the ones in force are not "verified"…
    let other = Limits { tasks_max: 49, ..limits.clone() };
    assert!(
        matches!(cgroup::verify(payload, &other), Err(cgroup::NotVerified::NotEnforced(why)) if why.contains("pids.max"))
    );
    // …and neither is a process that is not in the scope at all.
    assert!(matches!(cgroup::verify(std::process::id(), &limits), Err(cgroup::NotVerified::NotInScope(_))));

    running.channel.write_all(b"go\n").unwrap();
    running.channel.shutdown(Shutdown::Write).unwrap();
    let mut rest = String::new();
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        out.read_to_string(&mut rest).await.unwrap();
        running.child.wait().await.unwrap()
    })
    .await
    .expect("payload end");
    let threads: u32 = rest.lines().find_map(|l| l.strip_prefix("threads=")).expect("thread count").parse().unwrap();
    assert!((5..48).contains(&threads), "a thread bomb must stop below the task limit, got {threads}");
    assert_eq!(status.code(), Some(0));
    // The scope is gone with its last process.
    for _ in 0..100 {
        if !dir.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!dir.exists(), "the transient scope should not outlive the sandbox");
}

/// Exceeding the memory ceiling kills the game inside its own group; it does not get to
/// push the rest of the session into swap or the system-wide OOM killer.
#[tokio::test]
async fn resource_limits_kill_a_memory_hog_inside_the_scope() {
    if !scopes_available().await {
        return;
    }
    let body = r#"
block = bytearray(600 << 20)
for i in range(0, len(block), 4096):       # touch every page
    block[i] = 1
show("survived", len(block))
"#;
    let o = Probe { limits: Some(small_limits("memory", 128, 64)), ..Default::default() }.run(&script(body)).await;
    assert!(!o.stdout.contains("survived"), "600 MiB were allocated under a 128 MiB ceiling: {}", o.stdout);
    // The kernel kills the hog (SIGKILL) inside its group. What the launcher then sees of the
    // sandbox is one of two things, and which one is a race this test must not depend on:
    // bubblewrap reports its child's death (exit status 137) — or the user manager, whose
    // default for a unit struck by the OOM killer is to stop it (`OOMPolicy=stop`), has
    // terminated bubblewrap before it got that far. (Observed on systemd 262: the manager
    // wins while the hog's memory is still being torn down.) Either way the sandbox is gone.
    let relayed = o.code == Some(137);
    let scope_stopped = matches!(o.signal, Some(libc::SIGTERM | libc::SIGKILL));
    assert!(
        relayed || scope_stopped,
        "ended by the kernel's per-group OOM handling: exit status {:?}, signal {:?}; stderr: {}",
        o.code,
        o.signal,
        o.stderr
    );
}

/// The network-namespace helper is this program, executed here by systemd-run rather than
/// by a fork of the launcher. It has to be found through an inherited descriptor.
#[tokio::test]
async fn resource_limits_work_with_the_filtered_network_helper() {
    if !scopes_available().await {
        return;
    }
    let ns = NetnsSpec { tcp_ports: vec![443], dns: true, local_addrs: vec![] };
    let body = r#"
show("interfaces", ",".join(sorted(l.split(":")[0].strip() for l in open("/proc/net/dev").read().splitlines()[2:])))
"#;
    let limits = small_limits("netns", 256, 64);
    let unit = limits.unit.clone();
    let (spec, _) =
        Probe { network: Network::Filtered(ns), limits: Some(limits), ..Default::default() }.spec(&script(body));
    let mut running = sandbox::spawn(spec, Path::new(LAUNCHER)).expect("spawn sandbox in a scope");
    assert_eq!(running.egress.len(), 2, "DNS socket and one listener came back from the helper");
    let dir = cgroup_dir(running.child.id().unwrap());
    running.channel.shutdown(Shutdown::Write).unwrap();
    let mut out = String::new();
    running.child.stdout.take().unwrap().read_to_string(&mut out).await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(60), running.child.wait()).await.expect("exit").unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(out.contains("interfaces=lo"), "{out}");
    assert!(dir.ends_with(format!("{unit}.scope")), "{}", dir.display());
}
