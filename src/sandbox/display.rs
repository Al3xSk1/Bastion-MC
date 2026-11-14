//! What the game needs from the desktop: a display socket, an audio socket, the GPU.
//! Each is a hole in the sandbox, so each is individually switchable and as narrow as it
//! can be made: single socket files, never whole runtime directories.
//!
//! * **Wayland** — one socket. Clients are isolated from each other by the protocol.
//! * **X11** — one socket plus a cookie reduced to this display. X11 has *no* client
//!   isolation: any client can read all keyboard input, inject input (XTEST) into other
//!   windows and capture the screen. A compromised game on X11 can therefore type into your
//!   terminal — a full sandbox escape. It is refused unless `sandbox.allow_x11 = true`.
//! * **Audio** — the PulseAudio-protocol socket only (also served by pipewire-pulse). The
//!   native PipeWire socket is never exposed: it would also offer cameras and screen capture.
//!   The Pulse protocol still allows microphone capture; `audio = "none"` removes that. It
//!   also has a "load module" command, which both servers honour by default and which leads
//!   out of the sandbox: the socket is exposed only after the server itself has refused such
//!   a request (see `pulse`).
//! * **GPU** — `/dev/dri` and, read-only, exactly the sysfs device directories Mesa needs to
//!   identify the driver. Kernel GPU drivers are attack surface that cannot be filtered here.

use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

use super::{
    Mount, RUNTIME_DIR, nested,
    policy::SeccompPolicy,
    pulse::{self, ModuleLoading},
    wayland,
};
use crate::{
    config::{AudioPref, DisplayPref, IsolationMode, SandboxConfig},
    error::{Error, Result},
    paths::Paths,
    util,
};

#[derive(Default)]
pub struct Desktop {
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    /// Sandbox paths of UNIX sockets the game may connect to.
    pub sockets: Vec<PathBuf>,
    /// Sandbox paths of device directories needing ioctl.
    pub devices: Vec<PathBuf>,
    /// Human-readable summary lines, shown before launch.
    pub notes: Vec<String>,
    pub uses_x11: bool,
    /// The game's own Wayland socket. The compositor accepts connections on it for as long
    /// as this is kept.
    pub wayland_context: Option<wayland::SecurityContext>,
    /// The game's display does not exist yet: it is the inner display of a nested
    /// compositor, to be started (from this) just before the game.
    pub nested: Option<nested::Plan>,
}

pub(super) fn is_socket(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

fn set(env: &mut Vec<(String, String)>, k: &str, v: impl Into<String>) {
    env.push((k.to_string(), v.into()));
}

// ---------------------------------------------------------------- display

pub fn wayland_socket(paths: &Paths) -> Option<PathBuf> {
    let name = std::env::var_os("WAYLAND_DISPLAY").filter(|v| !v.is_empty())?;
    let path = PathBuf::from(&name);
    let path = if path.is_absolute() { path } else { paths.runtime.as_ref()?.join(name) };
    is_socket(&path).then_some(path)
}

/// Local displays only: `:N`, `:N.S`, `unix:N`. Anything naming a host is TCP and refused.
pub(super) fn x11_display_number(display: &str) -> Option<u32> {
    let rest = display.strip_prefix("unix:").or_else(|| display.strip_prefix(':'))?;
    let number = rest.split('.').next()?;
    (!number.is_empty() && number.len() <= 5 && number.bytes().all(|b| b.is_ascii_digit()))
        .then(|| number.parse().ok())
        .flatten()
}

pub fn x11_socket() -> Option<(u32, PathBuf)> {
    let number = x11_display_number(&std::env::var("DISPLAY").ok()?)?;
    let path = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
    is_socket(&path).then_some((number, path))
}

/// Reduce an Xauthority file to the one MIT cookie a *local* client of this display would
/// use, rewritten as `FamilyWild` with no display number, so that it still matches under
/// the sandbox's different host name and for a client that was given a socket path.
///
/// Selection mirrors libXau for a UNIX-socket connection: a `FamilyLocal` entry whose address
/// is this machine's host name, or an existing wildcard entry, for this display number.
/// Entries of other families (stale TCP-era records for old addresses are common and often
/// carry a *different*, dead cookie) must not be offered: the client would send the first
/// match and be refused.
///
/// Record layout, all integers big-endian u16: family, then four length-prefixed strings:
/// address, display number, auth name, auth data.
pub fn filter_xauthority(raw: &[u8], display: u32, hostname: &str) -> Vec<u8> {
    const FAMILY_LOCAL: u16 = 256;
    const FAMILY_WILD: u16 = 0xFFFF;
    fn take<'a>(buf: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        if buf.len() < n {
            return None;
        }
        let (head, tail) = buf.split_at(n);
        *buf = tail;
        Some(head)
    }
    fn u16be(buf: &mut &[u8]) -> Option<u16> {
        take(buf, 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    fn field<'a>(buf: &mut &'a [u8]) -> Option<&'a [u8]> {
        let len = u16be(buf)? as usize;
        take(buf, len)
    }
    let wanted = display.to_string();
    let mut rest = raw;
    while !rest.is_empty() {
        let parsed = (|| {
            let family = u16be(&mut rest)?;
            Some((family, field(&mut rest)?, field(&mut rest)?, field(&mut rest)?, field(&mut rest)?))
        })();
        // A truncated or corrupt file: stop, never guess.
        let Some((family, address, number, name, data)) = parsed else { break };
        let for_us = match family {
            FAMILY_LOCAL => address == hostname.as_bytes(),
            FAMILY_WILD => true,
            _ => false,
        };
        if !for_us || name != b"MIT-MAGIC-COOKIE-1" || !(number.is_empty() || number == wanted.as_bytes()) {
            continue;
        }
        // Exactly one record: the first usable one, as the client library would choose.
        let mut out = Vec::with_capacity(32 + data.len());
        out.extend_from_slice(&FAMILY_WILD.to_be_bytes());
        // An empty display number matches any (libXau): the sandbox contains one display,
        // and a client given the socket's path as DISPLAY does not know its number.
        for f in [&b""[..], &b""[..], name, data] {
            out.extend_from_slice(&(f.len() as u16).to_be_bytes());
            out.extend_from_slice(f);
        }
        return out;
    }
    Vec::new()
}

/// Wayland keeps clients apart, but a compositor offers every client everything it has —
/// including, on many compositors, screen capture and input injection. Where the compositor
/// implements `wp_security_context_v1` the game therefore does not get the session's
/// socket at all, but one created for it and declared to the compositor as a sandbox.
fn add_wayland(d: &mut Desktop, compositor: PathBuf, cfg: &SandboxConfig, paths: &Paths, instance: &str) -> Result<()> {
    let (socket, how) = match cfg.wayland_security_context {
        IsolationMode::Off => (
            compositor,
            "the session's own socket (wayland_security_context = \"off\"): the game is offered whatever the \
             compositor offers a trusted client"
                .to_string(),
        ),
        mode => {
            // A directory of its own, so that the socket's name can go away again without
            // touching anything else in the runtime directory.
            let home = paths.runtime.as_deref().or(compositor.parent()).unwrap_or(Path::new("/tmp"));
            let dir =
                home.join(crate::paths::APP).join(format!("wl-{}", util::hex_encode(&util::random_bytes::<6>()?)));
            match wayland::secure(&compositor, &dir, instance)? {
                wayland::Setup::Secured(context) => {
                    let socket = context.socket.clone();
                    d.wayland_context = Some(context);
                    (socket, "a socket of its own, declared to the compositor as a sandbox (security context)".into())
                }
                wayland::Setup::Unsupported if mode == IsolationMode::Require => {
                    return Err(Error::Sandbox(
                        "sandbox.wayland_security_context = \"require\", but this compositor does not implement \
                         wp_security_context_v1"
                            .into(),
                    ));
                }
                wayland::Setup::Unsupported => (
                    compositor,
                    "WARNING: this compositor does not implement wp_security_context_v1, so it cannot tell the \
                     game from a trusted client; whatever privileged interfaces it has (screen capture, virtual \
                     input, clipboard managers) are offered to the game too"
                        .to_string(),
                ),
            }
        }
    };
    let dst = PathBuf::from(RUNTIME_DIR).join("wayland-0");
    d.mounts.push(Mount::RoBind { src: socket, dst: dst.clone() });
    d.sockets.push(dst);
    set(&mut d.env, "WAYLAND_DISPLAY", "wayland-0");
    set(&mut d.env, "XDG_SESSION_TYPE", "wayland");
    d.notes.push(format!("display: Wayland — {how}"));
    Ok(())
}

/// Hand `d` the session's X display `number`. `display_as_path`: how `DISPLAY` names it.
///
/// * `true` (the game, when it is given the session's display directly): the socket's
///   path. libxcb then opens the file and never tries the abstract socket of the same
///   name, so Landlock's abstract-socket scoping can stay on in every network mode.
/// * `false` (the nested compositor's own sandbox): the classic `:N`. gamescope's SDL
///   front end does not come up with a path there (its display was never ready, in
///   `tests/nested.rs`), and it does not need one: that sandbox always has a private
///   network namespace, where the abstract name does not exist and libxcb falls back to
///   the socket file.
fn add_x11(d: &mut Desktop, number: u32, socket: PathBuf, paths: &Paths, display_as_path: bool) {
    let dst = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
    d.mounts.push(Mount::RoBind { src: socket, dst: dst.clone() });
    if display_as_path {
        set(&mut d.env, "DISPLAY", dst.to_string_lossy());
    } else {
        set(&mut d.env, "DISPLAY", format!(":{number}"));
    }
    d.sockets.push(dst);
    set(&mut d.env, "XDG_SESSION_TYPE", "x11");

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let source = std::env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".Xauthority")));
    let cookie = source
        .and_then(|p| fs::metadata(&p).ok().filter(|m| m.len() <= 256 * 1024).and(fs::read(&p).ok()))
        .map(|raw| {
            let host = fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
            filter_xauthority(&raw, number, host.trim())
        })
        .filter(|c| !c.is_empty());
    match cookie {
        Some(data) => {
            let dst = PathBuf::from(RUNTIME_DIR).join("Xauthority");
            set(&mut d.env, "XAUTHORITY", dst.to_string_lossy());
            d.mounts.push(Mount::RoData { data, dst, mode: 0o400 });
        }
        None => d.notes.push("display: no X11 cookie found; the X server must allow this user via xhost".into()),
    }
    let _ = paths;
    d.uses_x11 = true;
    d.notes.push(format!(
        "display: X11 :{number} — WARNING: X11 lets the game observe and inject input into every other X client; \
         this is a sandbox escape path. Prefer a Wayland session."
    ));
}

/// The session's display is X11. In order of preference: a nested compositor, so that the
/// game never touches the session's X server; the X server itself, if the user has accepted
/// what that means; a refusal that says what the choices are.
fn use_x11(d: &mut Desktop, cfg: &SandboxConfig, paths: &Paths, number: u32, socket: PathBuf) -> Result<()> {
    let nested_unavailable = match cfg.nested_compositor {
        IsolationMode::Off => "sandbox.nested_compositor = \"off\"".to_string(),
        mode => {
            // What the compositor — not the game — is given of the session.
            let mut host = Desktop::default();
            let available = nested::locate(&cfg.nested_compositor_layers).and_then(|()| {
                plan_gpu(&mut host).map_err(|e| e.to_string())?;
                if host.devices.is_empty() {
                    return Err("gamescope renders with Vulkan and this machine exposes no GPU (/dev/dri)".into());
                }
                Ok(())
            });
            match available {
                Ok(()) => {
                    add_x11(&mut host, number, socket, paths, false);
                    d.nested = Some(nested::Plan {
                        mounts: host.mounts,
                        env: host.env,
                        sockets: host.sockets,
                        devices: host.devices,
                        args: cfg.nested_compositor_args.clone(),
                        dir: paths.cache.join(format!("display-{}", util::hex_encode(&util::random_bytes::<6>()?))),
                        layers: cfg.nested_compositor_layers.clone(),
                        limits: None,
                        seccomp: SeccompPolicy { mode: cfg.seccomp, tripwire: cfg.tripwire, extra_allow: Vec::new() },
                    });
                    d.notes.push(format!(
                        "display: X11 :{number} through a nested compositor (gamescope, in a sandbox of its own): \
                         the game gets a display server to itself and nothing of the session's, so it cannot \
                         observe or inject input into your other windows"
                    ));
                    return Ok(());
                }
                Err(why) if mode == IsolationMode::Require => {
                    return Err(Error::Sandbox(format!("sandbox.nested_compositor = \"require\", but {why}")));
                }
                Err(why) => why,
            }
        }
    };
    if !cfg.allow_x11 {
        return Err(Error::Policy(format!(
            "this session only offers an X11 display. X11 has no isolation between clients: a compromised game \
             could log your keystrokes and type into other windows, escaping the sandbox. Choose one: (1) give \
             the game a display server of its own — install gamescope (`pacman -S gamescope`) and the launcher \
             runs the game inside it (not used now: {nested_unavailable}); (2) launch from a Wayland session; \
             (3) accept the risk: `allow_x11 = true` under [sandbox] in config.toml."
        )));
    }
    add_x11(d, number, socket, paths, true);
    d.notes.push(format!("display: no nested compositor ({nested_unavailable})"));
    Ok(())
}

fn plan_display(d: &mut Desktop, cfg: &SandboxConfig, paths: &Paths, instance: &str) -> Result<()> {
    let wayland = wayland_socket(paths);
    let x11 = x11_socket();
    match cfg.display {
        DisplayPref::Wayland => match wayland {
            Some(s) => add_wayland(d, s, cfg, paths, instance)?,
            None => return Err(Error::Config("sandbox.display = \"wayland\" but no Wayland socket was found".into())),
        },
        DisplayPref::X11 => match x11 {
            Some((n, s)) => use_x11(d, cfg, paths, n, s)?,
            None => return Err(Error::Config("sandbox.display = \"x11\" but no local X11 socket was found".into())),
        },
        DisplayPref::Auto => match (wayland, x11) {
            (Some(s), _) => add_wayland(d, s, cfg, paths, instance)?,
            (None, Some((n, s))) => use_x11(d, cfg, paths, n, s)?,
            (None, None) => {
                return Err(Error::Config(
                    "no display found (neither WAYLAND_DISPLAY nor a local DISPLAY is usable)".into(),
                ));
            }
        },
    }
    Ok(())
}

// ---------------------------------------------------------------- audio

/// What to tell the user when the sound server would load modules for the game.
fn module_loading_refusal(found: &str) -> Error {
    Error::Policy(format!(
        "refusing to give the game the audio socket: {found}. A sound server that loads modules for its clients \
         lets a compromised game run code outside the sandbox. Choose one: (1) keep sound and close the hole — \
         with PipeWire, create ~/.config/pipewire/pipewire-pulse.conf.d/90-no-module-loading.conf containing \
         `pulse.properties = {{ pulse.allow-module-loading = false }}` and run \
         `systemctl --user restart pipewire-pulse`; (2) play without sound: `audio = \"none\"` under [sandbox]; \
         (3) accept the risk: `allow_pulse_module_loading = true` under [sandbox]."
    ))
}

fn plan_audio(d: &mut Desktop, cfg: &SandboxConfig, paths: &Paths) -> Result<()> {
    match cfg.audio {
        AudioPref::None => d.notes.push("audio: disabled".into()),
        AudioPref::Pulse => {
            let socket = paths.runtime.as_ref().map(|r| r.join("pulse/native")).filter(|p| is_socket(p));
            match socket {
                Some(src) => {
                    // Asked of the server itself, every time: configuration files only say
                    // what a server would do if it had read them.
                    let modules = if cfg.allow_pulse_module_loading {
                        "module loading by clients not checked (allow_pulse_module_loading = true)"
                    } else {
                        match pulse::probe(&src) {
                            ModuleLoading::Denied => "the server refuses module loading by clients",
                            ModuleLoading::Allowed => {
                                return Err(module_loading_refusal(
                                    "the sound server accepts module-loading requests from its clients (it was asked)",
                                ));
                            }
                            ModuleLoading::Unknown(why) => {
                                return Err(module_loading_refusal(&format!(
                                    "it could not be established that the sound server refuses module loading ({why})"
                                )));
                            }
                        }
                    };
                    let dst = PathBuf::from(RUNTIME_DIR).join("pulse/native");
                    set(&mut d.env, "PULSE_SERVER", format!("unix:{}", dst.display()));
                    d.mounts.push(Mount::RoBind { src, dst: dst.clone() });
                    d.sockets.push(dst);
                    d.notes.push(format!(
                        "audio: PulseAudio-protocol socket (playback; it also permits recording); {modules}"
                    ));
                }
                None => d.notes.push("audio: no PulseAudio/pipewire-pulse socket found; running silent".into()),
            }
        }
        AudioPref::Alsa => {
            if Path::new("/dev/snd").is_dir() {
                d.mounts.push(Mount::DevBind { src: "/dev/snd".into(), dst: "/dev/snd".into() });
                d.devices.push("/dev/snd".into());
                d.notes.push("audio: raw ALSA devices (/dev/snd)".into());
            } else {
                d.notes.push("audio: /dev/snd not present; running silent".into());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- GPU

/// Expose `/dev/dri` plus, read-only, the sysfs directory of each DRM device and the three
/// symlinks libdrm/Mesa follow to find it. Nothing else of `/sys` exists in the sandbox.
fn plan_gpu(d: &mut Desktop) -> Result<()> {
    let dri = Path::new("/dev/dri");
    if !dri.is_dir() {
        d.notes.push("gpu: /dev/dri not present; software rendering only".into());
        return Ok(());
    }
    d.mounts.push(Mount::DevBind { src: dri.into(), dst: dri.into() });
    d.devices.push(dri.into());

    let mut bound: Vec<PathBuf> = Vec::new();
    let mut names = Vec::new();
    for entry in fs::read_dir(dri).map_err(|e| Error::io("reading /dev/dri", e))?.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.file_type().is_char_device() {
            continue;
        }
        let rdev = meta.rdev();
        let (major, minor) = (libc::major(rdev), libc::minor(rdev));
        let char_link = PathBuf::from(format!("/sys/dev/char/{major}:{minor}"));
        let (Ok(target), Ok(real)) = (fs::read_link(&char_link), fs::canonicalize(&char_link)) else { continue };
        // …/<pci device>/drm/cardN → the PCI (or platform) device directory.
        let Some(device_dir) =
            real.ancestors().find(|a| a.file_name().is_some_and(|n| n == "drm")).and_then(Path::parent)
        else {
            continue;
        };
        if !device_dir.starts_with("/sys/devices") {
            continue;
        }
        if !bound.iter().any(|b| b == device_dir) {
            bound.push(device_dir.to_path_buf());
            d.mounts.push(Mount::RoBind { src: device_dir.into(), dst: device_dir.into() });
            // /sys/bus/pci/devices/<addr>, used by Vulkan device enumeration.
            if let Some(addr) = device_dir.file_name() {
                let bus_link = Path::new("/sys/bus/pci/devices").join(addr);
                if let Ok(t) = fs::read_link(&bus_link) {
                    d.mounts.push(Mount::Symlink { target: t, link: bus_link });
                }
            }
        }
        d.mounts.push(Mount::Symlink { target, link: char_link });
        let name = entry.file_name();
        let class_link = Path::new("/sys/class/drm").join(&name);
        if let Ok(t) = fs::read_link(&class_link) {
            d.mounts.push(Mount::Symlink { target: t, link: class_link });
        }
        names.push(name.to_string_lossy().into_owned());
    }
    names.sort();

    // Proprietary NVIDIA driver nodes, when present. (Not exercised on the development
    // machine, which has an AMD GPU.)
    let mut nvidia = 0;
    if let Ok(entries) = fs::read_dir("/dev") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let is_nvidia = name.to_string_lossy().starts_with("nvidia");
            if is_nvidia && entry.metadata().is_ok_and(|m| m.file_type().is_char_device()) {
                let p = entry.path();
                d.mounts.push(Mount::DevBind { src: p.clone(), dst: p.clone() });
                d.devices.push(p);
                nvidia += 1;
            }
        }
    }
    d.notes.push(format!(
        "gpu: /dev/dri [{}]{}",
        names.join(", "),
        if nvidia > 0 { format!(" + {nvidia} nvidia node(s)") } else { String::new() }
    ));
    Ok(())
}

/// Decide what of the desktop session the game gets. `instance` names the sandbox towards
/// the compositor.
pub fn plan(cfg: &SandboxConfig, paths: &Paths, instance: &str) -> Result<Desktop> {
    let mut d = Desktop::default();
    plan_display(&mut d, cfg, paths, instance)?;
    plan_audio(&mut d, cfg, paths)?;
    if cfg.gpu {
        plan_gpu(&mut d)?;
    } else {
        d.notes.push("gpu: disabled".into());
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(family: u16, addr: &[u8], num: &[u8], name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = family.to_be_bytes().to_vec();
        for f in [addr, num, name, data] {
            v.extend_from_slice(&(f.len() as u16).to_be_bytes());
            v.extend_from_slice(f);
        }
        v
    }

    #[test]
    fn xauthority_is_reduced_to_the_cookie_a_local_client_would_use() {
        // Shaped like a real file: a stale TCP-family entry with a dead cookie comes first.
        let mut raw = record(0, b"\x7f\x00\x00\x02", b"0", b"MIT-MAGIC-COOKIE-1", b"STALE-COOKIE-000");
        raw.extend(record(256, b"otherhost", b"0", b"MIT-MAGIC-COOKIE-1", b"OTHER-HOST-COOKI"));
        raw.extend(record(256, b"myhost", b"1", b"MIT-MAGIC-COOKIE-1", b"COOKIE-FOR-DPY-1"));
        raw.extend(record(256, b"myhost", b"0", b"XDM-AUTHORIZATION-1", b"other-mechanism!"));
        raw.extend(record(256, b"myhost", b"0", b"MIT-MAGIC-COOKIE-1", b"GOOD-COOKIE-DPY0"));
        raw.extend(record(0, b"\xc0\xa8\x58\xfc", b"0", b"MIT-MAGIC-COOKIE-1", b"GOOD-COOKIE-DPY0"));
        let out = filter_xauthority(&raw, 0, "myhost");
        assert_eq!(out, record(0xFFFF, b"", b"", b"MIT-MAGIC-COOKIE-1", b"GOOD-COOKIE-DPY0"));
        assert!(!out.windows(6).any(|w| w == b"myhost"), "the real host name must not leak into the sandbox");
        assert_eq!(
            filter_xauthority(&raw, 1, "myhost"),
            record(0xFFFF, b"", b"", b"MIT-MAGIC-COOKIE-1", b"COOKIE-FOR-DPY-1")
        );
        assert!(filter_xauthority(&raw, 7, "myhost").is_empty());
        assert!(filter_xauthority(&raw, 0, "unknownhost").is_empty(), "no entry for this host: offer nothing");
        // An existing wildcard entry is honoured.
        let wild = record(0xFFFF, b"", b"0", b"MIT-MAGIC-COOKIE-1", b"WILDCARD-COOKIE!");
        assert_eq!(
            filter_xauthority(&wild, 0, "anything"),
            record(0xFFFF, b"", b"", b"MIT-MAGIC-COOKIE-1", b"WILDCARD-COOKIE!")
        );
    }

    #[test]
    fn xauthority_parser_survives_garbage() {
        assert!(filter_xauthority(b"", 0, "h").is_empty());
        assert!(filter_xauthority(&[0xff; 7], 0, "h").is_empty());
        let good = record(256, b"h", b"0", b"MIT-MAGIC-COOKIE-1", b"0123456789abcdef");
        let mut truncated = vec![0x01, 0x00, 0xff, 0xff, 0x41]; // length claims more than exists
        assert!(filter_xauthority(&truncated, 0, "h").is_empty());
        truncated = good.clone();
        truncated.extend_from_slice(&[0x01, 0x00, 0xff, 0xff, 0x41]);
        assert_eq!(filter_xauthority(&truncated, 0, "h"), filter_xauthority(&good, 0, "h"));
        for cut in 0..good.len() {
            let _ = filter_xauthority(&good[..cut], 0, "h");
        }
    }

    #[test]
    fn only_local_x11_displays_are_accepted() {
        assert_eq!(x11_display_number(":0"), Some(0));
        assert_eq!(x11_display_number(":12.0"), Some(12));
        assert_eq!(x11_display_number("unix:3"), Some(3));
        for bad in ["", "host:0", "192.0.2.1:0", ":", ":x", "tcp/host:0", ":0;rm"] {
            assert_eq!(x11_display_number(bad), None, "{bad:?}");
        }
    }
}
