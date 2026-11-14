//! A private dbus-daemon and the stand-in MPRIS player (`mpris-standin.py`) on it, for the
//! tests that need "the session bus": never the user's own, never the real Spotify app.
#![allow(dead_code)]

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// A dbus-daemon of the test's own: its configuration allows everything on it (nothing else
/// uses it), it starts no services, and it listens on a socket in `dir`.
pub struct PrivateBus {
    child: Child,
    pub socket: PathBuf,
}

impl PrivateBus {
    pub fn start(dir: &Path) -> PrivateBus {
        let socket = dir.join("bus");
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                socket.display()
            ),
        )
        .unwrap();
        let child = Command::new("/usr/bin/dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .env_clear()
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "the private bus did not come up");
            std::thread::sleep(Duration::from_millis(20));
        }
        PrivateBus { child, socket }
    }

    pub fn address(&self) -> String {
        format!("unix:path={}", self.socket.display())
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The stand-in player: what it prints, and a way to tell it things.
pub struct Standin {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Standin {
    pub fn start(bus: &PrivateBus, name: &str) -> Standin {
        let mut child = Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/mpris-standin.py"))
            .arg(name)
            .env_clear()
            .env("BASTION_TEST_BUS", bus.address())
            .env("PATH", "/usr/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the stand-in player (python3 with gi)");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let stdout = child.stdout.take().unwrap();
        let sink = lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                sink.lock().unwrap().push(line);
            }
        });
        let stdin = child.stdin.take();
        let standin = Standin { child, stdin, lines };
        standin.wait_for("ready");
        standin
    }

    pub fn wait_for(&self, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.lines.lock().unwrap().iter().any(|l| l == line) {
            assert!(Instant::now() < deadline, "the stand-in never said {line:?}: {:?}", self.lines.lock().unwrap());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn said(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    pub fn tell(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    pub fn quit(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Standin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn available() -> bool {
    let tools = ["/usr/bin/dbus-daemon", "/usr/bin/xdg-dbus-proxy", "/usr/bin/bwrap", "/usr/bin/python3"];
    let gi = Command::new("/usr/bin/python3")
        .args(["-I", "-c", "import gi; gi.require_version('Gio','2.0'); from gi.repository import Gio"])
        .status()
        .is_ok_and(|s| s.success());
    let ok = tools.iter().all(|t| Path::new(t).exists()) && gi;
    if !ok {
        eprintln!("skipped: needs dbus-daemon, xdg-dbus-proxy, bwrap and python3 with gi (Gio)");
    }
    ok
}
