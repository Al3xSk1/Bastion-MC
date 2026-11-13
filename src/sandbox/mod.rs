//! The JVM sandbox: bubblewrap namespaces, then (inside) Landlock and seccomp.
//!
//! Layers, outermost first:
//!
//! | layer | mechanism | what it removes |
//! |---|---|---|
//! | namespaces | `bwrap` user/pid/ipc/uts/cgroup (+net) | the host filesystem, other processes, SysV IPC, host name; `/` is an empty tmpfs populated only by the mounts below |
//! | network | private netns owned by the launcher's helper | every route; see `netns` and `egress` |
//! | Landlock | `init` stage | exec outside the JVM, device ioctls, creation of symlinks/sockets/FIFOs, TCP to other ports, signals and abstract sockets beyond the sandbox |
//! | seccomp | `init` stage | every syscall outside a 248-entry allowlist: keyrings, io_uring, userfaultfd, bpf, ptrace, mount API, namespaces, … |
//! | process | `init` stage | inherited keyrings, core dumps, inherited descriptors, the environment |
//!
//! Nothing here trusts a path twice: jars are bound by the descriptor they were hashed from,
//! generated files are passed as sealed memfds, and bwrap's own option list travels over a
//! descriptor so the host-visible command line is just `bwrap --args N -- /bastion/init …`.

pub mod cgroup;
pub mod display;
pub mod init;
pub mod landlock;
pub mod nested;
pub mod netns;
pub mod policy;
pub mod pulse;
pub mod seccomp;
pub mod trust;
pub mod wayland;

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::Write,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd, RawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nix::{
    fcntl::{FcntlArg, FdFlag, OFlag, SealFlag, fcntl},
    sys::memfd::{MFdFlags, memfd_create},
};

use self::{
    netns::NetnsSpec,
    policy::{InitPolicy, LandlockPolicy},
};
use crate::{
    error::{Context, Error, Result},
    java::Jvm,
    util,
};

pub const BWRAP: &str = "/usr/bin/bwrap";
/// Identity inside the sandbox, whatever the real UID is.
pub const SANDBOX_UID: u32 = 1000;
pub const SANDBOX_GID: u32 = 1000;
pub const SANDBOX_USER: &str = "player";
pub const HOSTNAME: &str = "bastion";
pub const RUNTIME_DIR: &str = "/run/user/1000";
pub const GAME_DIR: &str = "/game";
pub const SUPPORT_DIR: &str = "/bastion";
pub const INIT_PATH: &str = "/bastion/init";
const DYNAMIC_LOADER: &str = "/usr/lib/ld-linux-x86-64.so.2";

/// One filesystem operation for bubblewrap, applied in order.
pub enum Mount {
    RoBind {
        src: PathBuf,
        dst: PathBuf,
    },
    RoBindTry {
        src: PathBuf,
        dst: PathBuf,
    },
    /// Read-write bind (instance save/log/config directories, the scratch game root).
    Bind {
        src: PathBuf,
        dst: PathBuf,
    },
    /// Bind that keeps device nodes usable (GPU, sound).
    DevBind {
        src: PathBuf,
        dst: PathBuf,
    },
    /// Bind the exact file behind an open descriptor (the one that was verified).
    RoBindFd {
        file: File,
        dst: PathBuf,
    },
    /// Materialise bytes as a read-only file that exists only inside the sandbox.
    RoData {
        data: Vec<u8>,
        dst: PathBuf,
        mode: u32,
    },
    Tmpfs {
        dst: PathBuf,
        size_bytes: Option<u64>,
        mode: u32,
    },
    Dir {
        dst: PathBuf,
        mode: u32,
    },
    Symlink {
        target: PathBuf,
        link: PathBuf,
    },
    /// Make an existing mount point (e.g. a populated tmpfs) read-only.
    RemountRo {
        dst: PathBuf,
    },
    Proc {
        dst: PathBuf,
    },
    Dev {
        dst: PathBuf,
    },
    /// A read-only stack of directories, later ones on top. Not used for the game: it lets
    /// the tests show a sandbox an `/usr` with packages that are not installed.
    RoOverlay {
        layers: Vec<PathBuf>,
        dst: PathBuf,
    },
}

pub enum Network {
    /// Share the host network namespace.
    Open,
    /// A private namespace with nothing in it but loopback.
    None,
    /// A private namespace whose only exits are sockets held by the launcher.
    Filtered(NetnsSpec),
}

pub struct SandboxSpec {
    pub mounts: Vec<Mount>,
    pub network: Network,
    pub policy: InitPolicy,
    pub cwd: PathBuf,
    /// Run the whole sandbox inside a transient systemd scope with these resource limits.
    pub limits: Option<cgroup::Limits>,
}

pub struct Running {
    pub child: tokio::process::Child,
    /// Our end of the socketpair connected to the sandboxed program's stdin.
    pub channel: UnixStream,
    /// Sockets created inside the private network namespace: DNS first (if requested), then
    /// one TCP listener per requested port, in order.
    pub egress: Vec<OwnedFd>,
    /// bubblewrap's `--json-status-fd` stream (`child-pid`, later `exit-code`).
    status: Option<File>,
    init_pid: Option<i32>,
}

impl Running {
    /// Host PID of the sandboxed program itself (the JVM).
    ///
    /// bubblewrap reports the PID of its in-namespace init (PID 1 of the sandbox). Signals
    /// sent to that process from outside are ignored by the kernel unless it installed a
    /// handler, so it is useless as a target; the program is its first child.
    pub async fn payload_pid(&mut self) -> Option<i32> {
        if self.init_pid.is_none() {
            // Read through a duplicate: the original read end must stay open for as long as
            // the sandbox lives, or bubblewrap dies of SIGPIPE when it writes its final
            // `exit-code` document and the game's real exit status is lost.
            let pipe = self.status.as_ref()?.try_clone().ok()?;
            self.init_pid = tokio::task::spawn_blocking(move || {
                use std::io::BufRead;
                let mut line = String::new();
                std::io::BufReader::new(pipe).read_line(&mut line).ok()?;
                let value: serde_json::Value = serde_json::from_str(&line).ok()?;
                i32::try_from(value.get("child-pid")?.as_i64()?).ok()
            })
            .await
            .ok()
            .flatten();
        }
        let init = self.init_pid?;
        // The init stage execs into the program, so the child PID is stable once it exists.
        for _ in 0..50 {
            if let Some(pid) = first_child_of(init) {
                return Some(pid);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        None
    }

    /// Ask the sandboxed program to exit (SIGTERM, so the JVM runs its shutdown hooks) and
    /// wait; after `grace`, kill the whole sandbox.
    pub async fn stop(&mut self, grace: Duration) -> std::io::Result<std::process::ExitStatus> {
        if let Some(pid) = self.payload_pid().await {
            let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM);
        }
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                self.child.kill().await?;
                self.child.wait().await
            }
        }
    }
}

fn first_child_of(parent: i32) -> Option<i32> {
    // Fast path (CONFIG_PROC_CHILDREN).
    if let Ok(text) = std::fs::read_to_string(format!("/proc/{parent}/task/{parent}/children"))
        && let Some(pid) = text.split_whitespace().next().and_then(|p| p.parse().ok())
    {
        return Some(pid);
    }
    // Portable path: scan for a process whose parent is `parent`. Field 4 of /proc/<pid>/stat
    // follows the parenthesised command name, which may itself contain spaces or parentheses.
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let ppid = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse::<i32>().ok());
        if ppid == Some(parent) {
            return Some(pid);
        }
    }
    None
}

/// bubblewrap must be the distribution binary and must not be setuid: the design relies on
/// unprivileged user namespaces, and the launcher sets `no_new_privs` on itself.
pub fn check_bwrap() -> Result<PathBuf> {
    let path = util::require_root_owned(Path::new(BWRAP))
        .map_err(|e| Error::Sandbox(format!("bubblewrap is required (pacman -S bubblewrap): {e}")))?;
    let mode = std::fs::metadata(&path).ctx(|| "inspecting bubblewrap")?.mode();
    if mode & 0o6000 != 0 {
        return Err(Error::Sandbox(format!(
            "{} is setuid/setgid; this launcher only supports the unprivileged user-namespace mode",
            path.display()
        )));
    }
    Ok(path)
}

/// An anonymous, sealed, in-memory file: immutable once created, never on disk.
pub fn sealed_memfd(name: &str, data: &[u8]) -> Result<OwnedFd> {
    let fd = memfd_create(name, MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING).ctx(|| "memfd_create")?;
    let mut file = File::from(fd);
    file.write_all(data).ctx(|| "filling memfd")?;
    let fd: OwnedFd = file.into();
    nix::unistd::lseek(&fd, 0, nix::unistd::Whence::SeekSet).ctx(|| "rewinding memfd")?;
    fcntl(
        &fd,
        FcntlArg::F_ADD_SEALS(
            SealFlag::F_SEAL_WRITE | SealFlag::F_SEAL_GROW | SealFlag::F_SEAL_SHRINK | SealFlag::F_SEAL_SEAL,
        ),
    )
    .ctx(|| "sealing memfd")?;
    Ok(fd)
}

#[derive(Default)]
struct Args {
    list: Vec<OsString>,
    /// Descriptors the child chain must inherit (close-on-exec is cleared after fork).
    inherit: Vec<OwnedFd>,
}

impl Args {
    fn push(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.list.push(arg.as_ref().to_os_string());
        self
    }
    fn fd(&mut self, fd: OwnedFd) -> &mut Self {
        self.list.push(fd.as_raw_fd().to_string().into());
        self.inherit.push(fd);
        self
    }
    fn perms(&mut self, mode: u32) -> &mut Self {
        self.push("--perms").push(format!("{mode:04o}"))
    }
}

fn mount_args(args: &mut Args, mount: Mount) -> Result<()> {
    match mount {
        Mount::RoBind { src, dst } => args.push("--ro-bind").push(src).push(dst),
        Mount::RoBindTry { src, dst } => args.push("--ro-bind-try").push(src).push(dst),
        Mount::Bind { src, dst } => args.push("--bind").push(src).push(dst),
        Mount::DevBind { src, dst } => args.push("--dev-bind").push(src).push(dst),
        Mount::RoBindFd { file, dst } => args.push("--ro-bind-fd").fd(file.into()).push(dst),
        Mount::RoData { data, dst, mode } => {
            let fd = sealed_memfd("bastion-data", &data)?;
            args.perms(mode).push("--ro-bind-data").fd(fd).push(dst)
        }
        Mount::Tmpfs { dst, size_bytes, mode } => {
            args.perms(mode);
            if let Some(size) = size_bytes {
                args.push("--size").push(size.to_string());
            }
            args.push("--tmpfs").push(dst)
        }
        Mount::Dir { dst, mode } => args.perms(mode).push("--dir").push(dst),
        Mount::Symlink { target, link } => args.push("--symlink").push(target).push(link),
        Mount::RemountRo { dst } => args.push("--remount-ro").push(dst),
        Mount::Proc { dst } => args.push("--proc").push(dst),
        Mount::Dev { dst } => args.push("--dev").push(dst),
        Mount::RoOverlay { layers, dst } => {
            for layer in layers {
                args.push("--overlay-src").push(layer);
            }
            args.push("--ro-overlay").push(dst)
        }
    };
    Ok(())
}

/// Start the sandbox. `launcher_exe` is this program (it is bound into the sandbox as the
/// init stage and, in filtered mode, re-executed as the network-namespace helper).
pub fn spawn(spec: SandboxSpec, launcher_exe: &Path) -> Result<Running> {
    let bwrap = check_bwrap()?;
    let init_exe = File::open(launcher_exe).ctx(|| format!("opening {}", launcher_exe.display()))?;
    // The same binary again, for the case where it has to be executed through another
    // program (see below): a descriptor of its own, because the one above is consumed by
    // bubblewrap's argument list.
    let helper_exe = OwnedFd::from(init_exe.try_clone().ctx(|| "duplicating the launcher descriptor")?);

    let mut args = Args::default();
    // Explicit flags instead of --unshare-all: that one silently *tries* user and cgroup
    // namespaces; these fail hard if the kernel cannot provide them.
    for flag in ["--unshare-user", "--unshare-ipc", "--unshare-pid", "--unshare-uts", "--unshare-cgroup"] {
        args.push(flag);
    }
    if matches!(spec.network, Network::None) {
        args.push("--unshare-net");
    }
    args.push("--disable-userns") // no nested user namespaces, even if seccomp were bypassed
        .push("--die-with-parent")
        .push("--new-session") // no controlling terminal: TIOCSTI-style injection has no target
        .push("--clearenv")
        .push("--cap-drop")
        .push("ALL")
        .push("--hostname")
        .push(HOSTNAME)
        .push("--uid")
        .push(SANDBOX_UID.to_string())
        .push("--gid")
        .push(SANDBOX_GID.to_string());

    let (status_r, status_w) = nix::unistd::pipe2(OFlag::O_CLOEXEC).ctx(|| "creating status pipe")?;
    args.push("--json-status-fd").fd(status_w);

    for mount in spec.mounts {
        mount_args(&mut args, mount)?;
    }
    args.push("--ro-bind-fd").fd(init_exe.into()).push(INIT_PATH);
    args.push("--chdir").push(&spec.cwd);

    // bwrap reads its options from a descriptor; the host-visible command line carries none
    // of the paths above.
    let mut blob = Vec::new();
    for a in &args.list {
        if a.as_bytes().contains(&0) {
            return Err(Error::Sandbox("sandbox argument contains NUL".into()));
        }
        blob.extend_from_slice(a.as_bytes());
        blob.push(0);
    }
    let args_fd = sealed_memfd("bastion-bwrap-args", &blob)?;
    let policy_json =
        serde_json::to_vec(&spec.policy).map_err(|e| Error::Sandbox(format!("serialising policy: {e}")))?;
    let policy_fd = sealed_memfd("bastion-policy", &policy_json)?;

    let mut bwrap_argv: Vec<OsString> =
        vec![bwrap.clone().into(), "--args".into(), args_fd.as_raw_fd().to_string().into()];
    bwrap_argv.extend(["--".into(), INIT_PATH.into(), init::INIT_ARG.into(), policy_fd.as_raw_fd().to_string().into()]);
    let mut inherit = args.inherit;
    inherit.push(args_fd);
    inherit.push(policy_fd);

    // The launch channel: a socketpair, because unlike a pipe or memfd it cannot be re-opened
    // by another process through /proc/<pid>/fd/0.
    let (ours, theirs) = UnixStream::pair().ctx(|| "creating launch channel")?;

    // What to execute: bubblewrap, or (filtered network) this program as the namespace
    // helper, which then executes bubblewrap.
    let mut ctrl_parent = None;
    let (program, program_args): (OsString, Vec<OsString>) = match &spec.network {
        Network::Filtered(ns) => {
            let (ctrl_ours, ctrl_theirs) = UnixStream::pair().ctx(|| "creating helper channel")?;
            let spec_json =
                serde_json::to_vec(ns).map_err(|e| Error::Sandbox(format!("serialising namespace spec: {e}")))?;
            let spec_fd = sealed_memfd("bastion-netns", &spec_json)?;
            let mut helper_args: Vec<OsString> = vec![
                netns::NETNS_ARG.into(),
                spec_fd.as_raw_fd().to_string().into(),
                ctrl_theirs.as_raw_fd().to_string().into(),
            ];
            helper_args.extend(bwrap_argv.iter().cloned());
            inherit.push(spec_fd);
            inherit.push(ctrl_theirs.into());
            ctrl_parent = Some((ctrl_ours, ns.socket_count()));
            // With a scope in between, the helper is executed by systemd-run, not by a fork
            // of this process: "/proc/self/exe" would then name systemd-run. An inherited
            // descriptor names the same file wherever it is opened from.
            let program: OsString = match &spec.limits {
                Some(_) => format!("/proc/self/fd/{}", helper_exe.as_raw_fd()).into(),
                None => launcher_exe.into(),
            };
            (program, helper_args)
        }
        Network::Open | Network::None => (bwrap_argv[0].clone(), bwrap_argv[1..].to_vec()),
    };
    inherit.push(helper_exe);

    let mut cmd = match &spec.limits {
        Some(limits) => {
            let systemd_run = cgroup::locate().map_err(|e| Error::Sandbox(format!("resource limits: {e}")))?;
            let mut cmd = tokio::process::Command::new(systemd_run);
            cmd.args(limits.command(&program, &program_args));
            // Only what systemd-run needs to reach the user manager. bubblewrap clears the
            // environment again for the sandbox itself.
            cmd.env_clear().envs(cgroup::manager_env());
            cmd
        }
        None => {
            let mut cmd = tokio::process::Command::new(&program);
            cmd.args(&program_args);
            cmd.env_clear();
            cmd
        }
    };

    let raw: Vec<RawFd> = inherit.iter().map(|f| f.as_raw_fd()).collect();
    cmd.stdin(Stdio::from(OwnedFd::from(theirs)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Own process group: a terminal Ctrl-C reaches only the launcher, which then stops the
        // game deliberately instead of bubblewrap dying mid-write. If the launcher itself is
        // killed, bubblewrap's --die-with-parent still tears the sandbox down.
        .process_group(0)
        .kill_on_drop(true);
    // SAFETY: the closure runs between fork and exec and only calls fcntl(2), which is
    // async-signal-safe; `raw` was allocated before the fork.
    unsafe {
        cmd.pre_exec(move || {
            for fd in &raw {
                // Clear FD_CLOEXEC so exactly these descriptors survive into bubblewrap.
                if libc::fcntl(*fd, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let child = cmd.spawn().ctx(|| "starting the sandbox")?;
    drop(inherit); // the child has its own copies now

    let egress = match ctrl_parent {
        Some((ctrl, expected)) => netns::recv_fds(ctrl.as_fd(), expected, Duration::from_secs(20))?,
        None => Vec::new(),
    };
    // Keep close-on-exec on everything we hold.
    let _ = fcntl(&status_r, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC));
    Ok(Running { child, channel: ours, egress, status: Some(File::from(status_r)), init_pid: None })
}

// ---------------------------------------------------------------- base system view

pub enum Resolv {
    /// No resolver configuration at all.
    None,
    /// Point the resolver at the launcher's in-sandbox DNS responder.
    Loopback,
    /// Copy the host's resolver configuration (open network mode).
    Host,
}

pub struct BaseOptions<'a> {
    pub tmp_size_mb: u32,
    pub jvm: Option<&'a Jvm>,
    pub exec_allow: &'a [PathBuf],
    pub resolv: Resolv,
    pub expose_timezone: bool,
    /// TLS trust anchors for the sandbox. `None`: there are none (nothing in it speaks TLS).
    pub trust: Option<&'a trust::TrustStore>,
}

pub struct Base {
    pub mounts: Vec<Mount>,
    pub landlock: LandlockPolicy,
    pub env: Vec<(String, String)>,
}

fn data(dst: &str, text: String) -> Mount {
    Mount::RoData { data: text.into_bytes(), dst: dst.into(), mode: 0o444 }
}

/// The minimal system every sandboxed program gets: read-only `/usr`, a fresh `/proc` and
/// `/dev`, private `/tmp`, and a synthetic `/etc` that reveals neither the user name, the
/// host name, the machine id nor (by default) the time zone.
pub fn base(opts: &BaseOptions<'_>) -> Result<Base> {
    let mut mounts = vec![
        Mount::RoBind { src: "/usr".into(), dst: "/usr".into() },
        // Arch's merged-/usr layout.
        Mount::Symlink { target: "usr/lib".into(), link: "/lib".into() },
        Mount::Symlink { target: "usr/lib".into(), link: "/lib64".into() },
        Mount::Symlink { target: "usr/bin".into(), link: "/bin".into() },
        Mount::Symlink { target: "usr/bin".into(), link: "/sbin".into() },
        // A new procfs for the new PID namespace: only sandbox processes are visible.
        Mount::Proc { dst: "/proc".into() },
        // null, zero, full, random, urandom, tty, a private devpts and /dev/shm. Nothing else.
        Mount::Dev { dst: "/dev".into() },
        Mount::Tmpfs { dst: "/tmp".into(), size_bytes: Some(u64::from(opts.tmp_size_mb) << 20), mode: 0o1777 },
        Mount::Dir { dst: "/run".into(), mode: 0o755 },
        Mount::Dir { dst: "/run/user".into(), mode: 0o755 },
        Mount::Dir { dst: RUNTIME_DIR.into(), mode: 0o700 },
        Mount::RoBindTry { src: "/etc/ld.so.cache".into(), dst: "/etc/ld.so.cache".into() },
        data(
            "/etc/passwd",
            format!(
                "root:x:0:0::/:/usr/bin/nologin\n{SANDBOX_USER}:x:{SANDBOX_UID}:{SANDBOX_GID}::{GAME_DIR}:/usr/bin/nologin\nnobody:x:65534:65534::/:/usr/bin/nologin\n"
            ),
        ),
        data("/etc/group", format!("root:x:0:\n{SANDBOX_USER}:x:{SANDBOX_GID}:\nnobody:x:65534:\n")),
        data("/etc/nsswitch.conf", "passwd: files\ngroup: files\nhosts: files dns\n".into()),
        data("/etc/hosts", format!("127.0.0.1 localhost {HOSTNAME}\n::1 localhost\n")),
        // Fresh per launch: the real machine-id is a stable identifier of the installation.
        data("/etc/machine-id", format!("{}\n", util::hex_encode(&util::random_bytes::<16>()?))),
    ];
    // TLS trust anchors: generated for this launch, not the host's directories.
    if let Some(trust) = opts.trust {
        mounts.extend(trust.mounts());
    }
    match opts.resolv {
        Resolv::None => {}
        Resolv::Loopback => {
            mounts.push(data("/etc/resolv.conf", "nameserver 127.0.0.1\noptions timeout:3 attempts:2\n".into()))
        }
        Resolv::Host => {
            let text = std::fs::read("/etc/resolv.conf").ctx(|| "reading /etc/resolv.conf")?;
            if text.len() > 64 * 1024 {
                return Err(Error::Sandbox("/etc/resolv.conf is implausibly large".into()));
            }
            mounts.push(Mount::RoData { data: text, dst: "/etc/resolv.conf".into(), mode: 0o444 });
        }
    }
    if opts.expose_timezone
        && let Ok(zone) = std::fs::canonicalize("/etc/localtime")
        && zone.starts_with("/usr/share/zoneinfo")
    {
        mounts.push(Mount::Symlink { target: zone, link: "/etc/localtime".into() });
    }

    let loader = std::fs::canonicalize(DYNAMIC_LOADER).ctx(|| format!("resolving {DYNAMIC_LOADER}"))?;
    let mut landlock = LandlockPolicy {
        read: vec!["/usr".into(), "/etc".into(), "/proc".into(), SUPPORT_DIR.into()],
        read_exec: vec![loader],
        write: vec!["/tmp".into(), "/dev/shm".into(), RUNTIME_DIR.into()],
        plain_devices: vec!["/dev".into()],
        read_if_present: vec!["/sys".into()],
        scope_abstract_unix: true,
        ..Default::default()
    };
    landlock.read_exec.extend(opts.exec_allow.iter().cloned());

    if let Some(jvm) = opts.jvm {
        landlock.read_exec.push(jvm.home.clone());
        for dir in &jvm.external_dirs {
            // The JDK reaches its `cacerts` through /etc/ssl; that path is provided by the
            // generated trust store above, never by the host's directory.
            if dir.starts_with("/etc/ssl") || dir.starts_with("/etc/ca-certificates") {
                continue;
            }
            if !mounts
                .iter()
                .any(|m| matches!(m, Mount::RoBind { dst, .. } | Mount::RoBindTry { dst, .. } if dst == dir))
            {
                mounts.push(Mount::RoBind { src: dir.clone(), dst: dir.clone() });
            }
        }
    }

    let mut env: Vec<(String, String)> = [
        ("PATH", "/usr/bin"),
        ("HOME", GAME_DIR),
        ("USER", SANDBOX_USER),
        ("LOGNAME", SANDBOX_USER),
        ("TMPDIR", "/tmp"),
        ("XDG_RUNTIME_DIR", RUNTIME_DIR),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    // The UI language is the one piece of host locale worth carrying over; only if it looks
    // like a locale name.
    let lang = std::env::var("LANG").ok().filter(|l| {
        !l.is_empty()
            && l.len() <= 32
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'@'))
    });
    env.push(("LANG".into(), lang.unwrap_or_else(|| "C.UTF-8".into())));

    Ok(Base { mounts, landlock, env })
}
