# bastion-mc threat model

This document says what the launcher defends, against whom, with which mechanism, and how
each claim is checked. It also says, without softening, what is **not** defended. Where a
row cites a test, that test lives in this repository and passes on the development machine
(Arch Linux, kernel 7.2, Landlock ABI 10, bubblewrap 0.13.1, OpenJDK 27).

## 1. Assets

| Asset | Lifetime | Why it matters |
|---|---|---|
| Microsoft **refresh token** | weeks–months | Mints new sessions for the Microsoft account's Xbox/Minecraft identity. The most valuable secret. |
| Minecraft **access token** | 24 h | Lets its holder join servers as you and change skin/name. What "session stealers" take. |
| **Signing key** for mod profiles | long | Decides what code may run in a curated instance. |
| The **user's files and session** | — | Everything in `$HOME`, the desktop session (keystrokes, clipboard, screen), SSH/GPG agents, browser profiles. |
| **Integrity of what runs** | — | Client jar, libraries, loader, mods. |

## 2. Adversaries

| ID | Adversary | Capability assumed |
|---|---|---|
| **N** | Network attacker | Controls DNS answers and can intercept/modify traffic (hostile Wi-Fi, compromised resolver, BGP). Cannot break TLS 1.3 or forge certificates from the system trust store — except in row N3. |
| **M** | Malicious or compromised mod | Arbitrary Java **and native** code inside the game JVM from the first instruction. |
| **S** | Malicious game server / content | Sends data that triggers a JVM-side RCE (Log4Shell-class lookups, deserialisation gadgets, parser bugs). Ends up with the same capability as **M**. |
| **O** | Other local user | A different UID on the same machine. Reads everything world-readable: `/proc/*/cmdline`, `/proc/*/status`, world-readable files, loopback ports. |
| **U** | Same-user software | A process running as *your* UID outside the sandbox (a compromised editor plugin, an npm post-install script). |
| **C** | Supply chain | A tampered file on a CDN/mirror, or a swapped file in the local store. |

Out of scope: root, the kernel, firmware, physical access, and Mojang/Microsoft themselves
acting maliciously (they can simply hand out a malicious client).

## 3. Trust boundaries

```
 ┌──────────────────────────── host, your UID ────────────────────────────────┐
 │                                                                            │
 │  bastion-mc (launcher)            non-dumpable, no_new_privs, RLIMIT_CORE=1│
 │  ├─ auth: MSA → XBL → XSTS → MC   tokens only in mlock'd, self-wiping pages│
 │  ├─ verified store                every file hashed; bound into sandbox by │
 │  │                                the descriptor that was hashed           │
 │  ├─ egress relay + DNS responder  holds the ONLY sockets that lead out     │
 │  └─ log relay                     sanitises every byte from the sandbox    │
 │        │ fd 0: AF_UNIX socketpair (launch frame incl. access token)        │
 │ ═══════╪══════════ user ns ▸ user ns ▸ pid/ipc/uts/cgroup/mount/net ns ════│
 │        ▼                                                                   │
 │  bwrap ─▶ init stage ─▶ JVM       Landlock + seccomp(allowlist+tripwire)   │
 │  empty tmpfs "/"; ro /usr; synthetic /etc; fresh /proc,/dev; no route      │
 │  rw: /game (scratch) + saves/ logs/ config/ resourcepacks/ of ONE instance │
 │  holes: 1 display socket, 1 audio socket, /dev/dri (+ its sysfs dir)       │
 └────────────────────────────────────────────────────────────────────────────┘
```

The JVM is treated as hostile from its first instruction. The launcher is the trusted
computing base: about 12k lines of Rust including its unit tests. `unsafe` appears only
around raw syscalls, the locked-memory mapping, the wiping allocator and the fork/exec hook.

The **graphical front end** (`bastion-mc-gui`) is deliberately *not* part of that base. It
is a different program in a different process: it starts the launcher with `--events` and
reads typed progress events from its standard output (§4.5). The launcher binary links no
GUI, font or GPU code, and the event protocol has no field that could carry a credential.

## 4. Attack surface matrix

"Verified by" names a test (`tests/…` or a unit test) or an experiment in §7.

### 4.1 `/proc` snooping and local observation (adversaries O, U)

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| P1 | **O** reads the access token from `/proc/<jvm>/cmdline` (`--accessToken …`), `ps`, audit logs | The JVM is started with **no program arguments at all**. Main class and the whole argument vector travel in a frame over an inherited `AF_UNIX` socket to a bootstrap class. Player name and UUID are kept off the command line too. | `jvm::jvm_starts_sandboxed_and_receives_the_token_out_of_band`; real client: cmdline ends in `… BastionBootstrap`, 0 matching args | none for O |
| P2 | Token in `/proc/<jvm>/environ` | Environment is built from scratch (11 synthetic variables); nothing secret is ever exported. | same test (`token_in_environ=false`), `sandbox::identity_environment_and_filesystem_view` | — |
| P3 | Token via `sun.java.command`, `hsperfdata_*`, `jcmd`, `hs_err_pid*.log` | Arguments passed reflectively never enter those; `-XX:-UsePerfData`, `-XX:+DisableAttachMechanism`, `-XX:-EnableDynamicAgentLoading`. | same test (`sun_java_command=BastionBootstrap`) | — |
| P4 | **U** re-opens the launch channel through `/proc/<pid>/fd/0` before the JVM reads it | The channel is a **socketpair**. A pipe or a memfd *can* be re-opened that way and was stolen in an experiment; a socket cannot (`ENXIO`). | §7 E3; `sandbox::launch_channel_is_not_observable` | see P7 |
| P5 | bubblewrap's command line reveals `$HOME` paths, instance names, mod list | All bwrap options are passed over a sealed memfd (`--args N`); visible command line is `bwrap --args N -- /bastion/init __sandbox-init M`. Jars appear inside as `/bastion/lib/NNNN-name.jar`. | real client snapshot (§7 E7) | The JVM's `-cp` lists library file names (public information). |
| P6 | **O/U** attach to or dump the **launcher** to read tokens | `PR_SET_DUMPABLE=0` (also blocks `/proc/<pid>/{mem,environ,maps,fd}` and ancestors' ptrace), Yama, `RLIMIT_CORE=1`, tokens in `mlock`+`MADV_DONTDUMP`+`MADV_WIPEONFORK` pages, heap wiped on free, launcher holds **no** credential while the game runs. | §7 E1 (unrelated process: denied), E9 (own parent: denied once non-dumpable); `secret::tests::*`, `hardening::tests::*` | root |
| P7 | **U** reads the **JVM's** memory (`process_vm_readv`, ptrace) and takes the access token | **Not defended, and cannot be by an unprivileged sandbox.** The JVM lives in a user namespace *owned by you*; the kernel grants any process with your UID in the parent namespace full capabilities over it, which bypasses both Yama `ptrace_scope` 1/2 and non-dumpable status. | §7 E1: read succeeded with `ptrace_scope=1`, with and without `PR_SET_DUMPABLE=0` | Only `kernel.yama.ptrace_scope=3`, or running the launcher under a dedicated UNIX account, closes this. `doctor` reports it. |
| P8 | Core dump of the JVM lands on disk (systemd-coredump) with the token inside | `RLIMIT_CORE` is set to **1**, not 0. With a piped `core_pattern`, 0 still pipes the full image to the handler; 1 makes the kernel abort the dump. Plus `-XX:-CreateCoredumpOnCrash`. | §7 E4; `sandbox::identity…` (`core=(1, 1)`) | — |
| P9 | Token paged to swap | Launcher: locked pages. JVM: not controllable (Java heap). | `secret::tests::pages_are_locked` | JVM heap may be swapped; use encrypted or RAM-backed swap. |

### 4.2 Malicious mod / in-JVM code execution (adversaries M, S)

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| J1 | Read `~/.ssh`, browser profiles, wallets, other instances | Mount namespace with an empty tmpfs root. `$HOME`, `/run`, `/var`, `/opt`, real `/etc` do not exist. Landlock independently allows only the listed trees. | `sandbox::identity…` (`host_paths` empty), `sandbox::landlock_denies…` | Information leak: `/proc/self/mountinfo` lists the *source path* of every bind mount, so in-sandbox code can learn host directory names (including the user name in `$HOME`). It cannot open them. |
| J2 | Persist: drop a jar, edit launcher config, plant autostart | Instance root is not mounted; the game root is a per-launch scratch directory deleted on exit. Only `saves/ logs/ config/ resourcepacks/` are writable. `mods/` is a sealed read-only tmpfs of verified jars. No symlink/FIFO/socket/device creation anywhere (Landlock + seccomp). Launcher never follows links in sandbox-writable trees. | `sandbox::landlock_denies…`, `sandbox::verified_files…`, `instance::tests::*` | A mod that *executes scripts from `config/`* can persist inside the sandbox. `options.txt`/`servers.dat` are copied back as size-capped regular files. |
| J3 | Spawn a shell / download-and-run (`Runtime.exec("sh -c curl …")`) | Landlock grants `execve` only on the JVM home and `ld.so`. | `jvm::…` (`exec_shell=denied`), `sandbox::landlock_denies…` | In-process native code is inherent to a JVM with JNI; it gains nothing the JVM lacks. |
| J4 | Escape via kernel attack surface | seccomp **allowlist** (248 entries, 7 of them argument-filtered): everything else → `ENOSYS`. Denied incl. `io_uring_*`, `userfaultfd`, `bpf`, `perf_event_open`, `keyctl`, `unshare`, `clone3`, `open_by_handle_at`, netlink/packet/AF_ALG sockets, set*id, `mknod`, `splice`. Namespaces flags masked on `clone`. bwrap `--disable-userns`. Zero capabilities, `no_new_privs`. | `sandbox::seccomp_allowlist…`, `sandbox::landlock_denies…` (Landlock alone, seccomp in log mode) | Bugs in allowed syscalls and in the GPU driver ioctl surface. |
| J5 | Bypass a syscall filter through the x32 ABI | The Arch kernel has `CONFIG_X86_X32_ABI=y`; `nr \| 0x40000000` reaches the same handlers. An allowlist does not list those numbers; a denylist would have been bypassed. | `sandbox::x32_entry_point_is_live_on_the_host`, `sandbox::seccomp_allowlist…` (`x32_getpid=ENOSYS`) | — |
| J6 | Exploit-only syscalls (`ptrace`, `process_vm_*`, `bpf`, `mount`, `kexec_*`, `setns`, module loading…) | **Tripwire**: `KILL_PROCESS`. The launcher reports the SIGSYS death as a compromise indicator. | `sandbox::tripwire_kills…` | — |
| J7 | Read the **refresh token** from the kernel keyring | Keyrings are not namespaced: an unhardened bwrap sandbox read both test keys (§7 E2). Defence in two independent layers: init joins a fresh session keyring; seccomp denies `keyctl`/`add_key`/`request_key`. | `sandbox::kernel_keyring_is_unreachable_from_the_sandbox` (each layer alone) | — |
| J8 | Inject keystrokes into the launching terminal (`TIOCSTI`), read/poison terminal | No tty: stdio are a socket and two pipes; `--new-session`; ioctl filter; Landlock device-ioctl control. Output is stripped of control and bidi characters before it reaches a terminal or log. | `sandbox::seccomp_allowlist…` (`tiocsti`), `util::tests::sanitize_strips_escape_sequences` | — |
| J9 | Signal, trace or connect to host processes | PID namespace (sees PIDs 1 and 2); Landlock signal + abstract-socket scoping; private IPC namespace; no D-Bus, no portal, no agent sockets. | `sandbox::identity…`, `sandbox::landlock_denies…` (`signal_pid1=EPERM`), `sandbox::procfs_does_not_hand_over_the_supervisor` (bubblewrap's init: descriptors, environ, memory, root all `EACCES`) | Display and audio sockets (D1–D3). |
| J10 | **Exfiltrate** to an attacker host (webhook, C2) | Private network namespace with **no route**. The only exits are sockets held by the launcher: DNS answers only allowlisted names (synthetic addresses), TCP is relayed only to Mojang service domains on 443 *with matching SNI* and to allowlisted game servers. | `sandbox::filtered_network…`, `egress::*` (5 tests), real client: 5 relayed / 0 denied | Covert channels through the *allowed* endpoints: see J13. |
| J11 | Exfiltrate through DNS (`<data>.attacker.tld`) | The responder never forwards. Non-allowlisted names are answered `NXDOMAIN` locally and logged. | `egress::names_are_resolved_only_if_allowlisted…` | — |
| J12 | Domain fronting / SSRF: reuse an allowed CDN address for another site; reach router or `127.0.0.1` services | SNI must equal the name the address was handed out for; non-TLS on 443 is dropped; plain HTTP only as `GET /texture/<hex>` re-fetched by the launcher over TLS; outbound relay refuses private/loopback/link-local targets unless `allow_private`. | `egress::https_relay_requires…`, `egress::game_server_relay…`, `egress::tests::public_address_classification` | `Host`-header fronting inside TLS is invisible to any non-terminating relay (the large CDNs reject SNI/Host mismatch). |
| J13 | **Use or leak the access token from inside** | **Not prevented.** The game must hold the token to join servers; any code in the JVM can read it and can call Mojang's API with it (e.g. encode it into a skin upload). Limited by: 24 h lifetime, refresh token never enters the sandbox, egress policy. | — | The real fix is a credential-injecting broker so the JVM holds only a placeholder (§8). Not implemented. |
| J14 | Log4Shell (CVE-2021-44228 family), JNDI codebase loading | Mojang's per-version patched Log4j config is fetched, hash-verified and mandatory; `-Dlog4j2.formatMsgNoLookups=true`; `trustURLCodebase=false` ×3; no route to attacker LDAP anyway. | `java::tests::flags…`; real client log (`-Dlog4j.configurationFile=/bastion/log4j2.xml`) | — |
| J15 | Java deserialisation gadgets (e.g. "BleedingPipe") | `-Djdk.serialFilter=!*` JVM-wide; opt-out only via config or a *signed* profile option. | `jvm::…` (`deserialize=rejected`) | Mods that install their own stream filter. |
| J16 | Attach a Java agent at runtime | `-XX:+DisableAttachMechanism`, `-XX:-EnableDynamicAgentLoading`; `-javaagent`/`-agentlib`/`-Xbootclasspath` refused from version files and signed manifests alike. | `profile::manifest::tests::rejects_dangerous…`, `java::tests::flags…` | — |
| J17 | Cross-process Spectre v2/v4 from JIT-compiled code | `PR_SET_SPECULATION_CTRL` force-disable for indirect branches and store bypass, inherited by the JVM. | init code path (best effort; not measured) | CPU/kernel dependent. |

### 4.3 Desktop integration (the deliberate holes)

| # | Attack | Defence | Residual |
|---|---|---|---|
| D1 | **X11**: keylog every X client, inject input (XTEST) into a terminal, screenshot | Refused by default; needs `sandbox.allow_x11 = true`. Only the display's socket file and the one cookie a local client would use are exposed; in filtered mode the server's abstract socket is unreachable. (In `open` network mode abstract-socket scoping has to be switched off for X11: libxcb does not fall back after a permission error — `sandbox::x11_client_connects_in_each_network_mode`.) | **With X11 allowed this is a sandbox escape path.** No user-space sandbox fixes X11; use a Wayland session, or a nested compositor (gamescope). |
| D2 | **Wayland**: clipboard while focused; privileged protocols on wlroots compositors (screencopy, virtual keyboard) | One socket file only. | `wp_security_context_v1` is not used yet (§8). GNOME and KDE do not offer those protocols to ordinary clients; wlroots compositors (sway, Hyprland) do. Untested here. |
| D3 | **Audio**: record the microphone, capture other apps' audio; make the sound server load code | Only the PulseAudio-protocol socket (never the native PipeWire socket, which also offers cameras/screen capture). That protocol has a "load module" command which PulseAudio *and* pipewire-pulse honour by default; with it a client can make the server — outside the sandbox — load a plugin or touch files it names. So before the socket is exposed the launcher asks the server itself to load a module that does not exist: only a clean "access denied" lets the launch continue (`allow_pulse_module_loading = true` overrides, `audio = "none"` removes the socket). Tested by `sandbox::pulse::tests::*` (stand-in servers: refused, accepted, garbage, hang-up) and against real PipeWire in both settings (§7 E10). | Recording is possible with `audio = "pulse"`; exfiltrating it still needs J10. The answer is checked at launch, not continuously. |
| D4 | **GPU**: driver ioctl bugs | `/dev/dri` only (+ NVIDIA nodes if present), device-ioctl right limited to those nodes, sysfs limited to the GPU's own device directory. | Kernel GPU drivers are large; this surface cannot be filtered generically. |

### 4.4 Network and supply chain (adversaries N, C)

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| N1 | MITM / downgrade of launcher traffic | TLS 1.3 only (TLS 1.2 is not compiled into rustls), hybrid post-quantum key exchange (X25519MLKEM768) offered first, system CA store from a fixed root-owned path (`SSL_CERT_FILE` ignored), no redirects, no proxies, no plaintext, fixed host allowlist, bounded bodies, no decompression. | `tls::*` (5 tests), `net::http::tests::url_policy`; hybrid key exchange observed against Mojang's hosts (Microsoft's sign-in hosts still pick a classical group) | Recorded traffic to hosts that do not negotiate the hybrid group stays exposed to a future quantum adversary. |
| N2 | Poisoned DNS for launcher or relay | TLS for everything the launcher fetches; relay refuses private targets; game traffic is integrity-protected by Minecraft's own session handshake. | — | DoS; being steered to an attacker-run *game server* (contained by §4.2). |
| N3 | A trusted CA mis-issues a certificate for a Mojang/Microsoft host | SPKI pins, matched against the **validated** chain (a decoy copy of the pinned certificate does not help the attacker). Built in and on by default for Microsoft/Mojang hosts: the seven root and long-lived intermediate keys those services chain to, so ~120 other CAs no longer count there. `tls.builtin_pins = false` turns them off; `[tls.pins]` adds or replaces. | `tls::presenting_the_pinned_certificate_as_an_unused_extra…`, `net::tls::tests::builtin_pins_cover_microsoft_hosts_and_match_the_system_store` | If Microsoft moves to a root outside the list, connections fail closed until the list is updated or the pins are disabled. Other hosts (Fabric, Quilt) are unpinned unless you pin them. |
| C1 | Tampered client jar / library / asset (CDN or local store) | Hash chain from `version_manifest_v2.json`; verified on download (never reaches its final path otherwise) and again on every launch. Jars, asset index, logging config and mods are hashed **from the descriptor that is then bind-mounted**; asset objects are re-hashed and mounted as a read-only directory. Mismatch aborts; no automatic re-download without `--repair`. | `mojang::install::tests::*`, real store: bit-flip → exit 4 (§7 E6), `sandbox::verified_files…` | Mojang publishes SHA-1 only: second-preimage resistance is what is relied on. |
| C2 | Unsigned, modified or extra mod jar | Curated instances need an Ed25519 signature (`verify_strict`, domain-separated) by a key in the trust store over the manifest; `mods/` must equal the signed list exactly — no extra file of any kind, no symlinks, exact size and SHA-256. | `profile::*` (15 tests), `instance::tests::curated…`, §7 E8 | On filesystems with fs-verity (btrfs; ext4/f2fs with the feature) every store file and mod jar is made immutable *before* it is hashed, so there is no window. Elsewhere: in-place modification by **U** between check and mount of a file U can write (U can do far worse). |
| C3 | Replay an old signed manifest; move a manifest to another instance | Monotonic `serial` remembered per instance; manifest is bound to the instance id. | `profile::signing::tests::rollback…`, `instance::tests::curated…` | State file is user-writable. |
| C4 | **U** adds its own signing key | If `/etc/bastion-mc/trusted-keys.d` (root-owned) holds keys, per-user keys are ignored. | `profile::signing::tests::*` | Without system keys, **U** can add a key. |
| C5 | Malicious JVM | Only root-owned runtimes under `/usr/lib/jvm`; nothing is ever downloaded. | `java::tests::*` | — |
| C6 | Loader library with no published hash | `profile new` pins what it downloads (TLS) and tells you which hashes were cross-checked and which are trust-on-first-use; your signature freezes them. | `profile::author::tests::*` | TOFU at authoring time. |
| C7 | Malicious dependency of the launcher | 111 crates in the launcher, no OpenSSL/native-tls; the only C is AWS-LC (the rustls crypto provider, built from the pinned sources, never a system copy); `Cargo.lock` + `--locked/--frozen` builds; the Java bootstrap is compiled from source at build time, no binary blobs. The graphical front end's larger tree (about 220 crates) is linked only into its own, secret-free binary (§4.5). | `cargo tree -p bastion-mc` | Inherent to using crates.io. |

### 4.5 Graphical front end

The window, its fonts and its GPU context live in `bastion-mc-gui`, a separate process that
only ever sees what `bastion-mc --events` prints.

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| G1 | A bug in the GUI toolkit, text shaping or a GPU driver (reachable by hostile text: game output is displayed) is used to read tokens | Tokens never exist in the GUI process. The launcher is not linked into it and links none of its dependencies (iced, winit, wgpu, … about 220 crates); the wire protocol (`crates/proto`) has no credential field in either direction; the GUI crate forbids `unsafe`. | `cargo tree -p bastion-mc` contains no `iced`/`winit`/`wgpu`; `proto::tests::the_wire_format_is_stable`; `events::*` tests | The GUI process can still be made to *click* (start a launch, sign out): an integrity, not a confidentiality, problem. |
| G2 | Hostile game output attacks the console: escape sequences, megabyte-long lines, floods | Sanitised and length-bounded in the launcher; the GUI strips control characters again, refuses lines over 64 KiB without buffering them, keeps 800 lines. A reader that falls behind makes the launcher *drop* output (counted, still in the log file) rather than stall the game. | `backend::tests::over_long_lines_are_dropped_without_buffering_them`, `…::events_strays_and_exit_status_arrive_in_order`, `console::tests::*`, `events::tests::long_text_is_clipped_and_stays_one_line` | — |
| G3 | The sign-in prompt is used to make the desktop open an attacker's URL | "Open link" exists only for `https://` links to four Microsoft sign-in hosts (exact host, no port, no user part, printable ASCII) and passes the URL as one argument to `xdg-open`. The launcher refuses device codes that are not 4–32 alphanumerics and verification links that are not HTTPS. | `app::tests::only_microsoft_sign_in_links_are_opened`, `view::tests::an_untrusted_link_gets_no_open_button`, `msa::tests::device_flow_stops_on_refusal_and_rejects_plain_http_links` | — |
| G4 | The GUI crashes or is killed while a game runs, leaving an unsupervised sandbox | The launcher's standard input is its control channel; end-of-file means "stop". The game is shut down gracefully (15 s, then killed) and the launcher exits. Closing the window does the same and waits for it. | `backend::tests::a_vanished_ui_stops_the_launcher`, `events::a_pending_sign_in_is_abandoned_on_stop_and_when_the_front_end_disappears`, `app::tests::closing_the_window_stops_a_running_game_first` | — |
| G5 | **U**, or any other X11 client, reads the device code or the sign-in link off the window, or injects clicks | Not defended (same-user software, and X11 by design). The code and the link are useless without your Microsoft credentials, and the browser sign-in's `state` only protects against *other users*. | — | See §5.1 and §5.2. |

## 5. Not defended — read this before relying on it

1. **Other software running as you (U).** It can read the game's memory (P7), read the
   refresh token from the kernel keyring (same login session), replace the launcher binary
   in `$PATH`, or edit `config.toml`. On this development machine the account is also in
   the `docker` group, which makes any same-user process root-equivalent. A user-level
   launcher cannot defend the user from themself; a dedicated UNIX account for gaming can.
2. **X11 sessions** once `allow_x11` is set (D1).
3. **The session token inside the JVM** (J13).
4. **Microphone** with Pulse audio enabled (D3). (Module loading through the same socket is
   closed: the launcher refuses the socket unless the server denies it.)
5. **Kernel and GPU-driver vulnerabilities** reachable through allowed syscalls/ioctls.
6. **Denial of service**: a hostile mod can burn CPU/RAM, fill `saves/` or the scratch
   directory. No cgroup limits are applied. (The launcher's own log and the relay's
   connection count are bounded.)
7. **Realms and UDP**: Realms servers are handed out as bare IPs and are not reachable in
   filtered mode; UDP (voice-chat mods, LAN discovery) does not exist in the sandbox.

## 6. Where the implementation deviates from the written requirements

| Requirement as written | What was built instead | Reason (observed on the target system) |
|---|---|---|
| `-XX:+DisableAttachAgent` | `-XX:+DisableAttachMechanism` (+ `-XX:-EnableDynamicAgentLoading` on 21+) | The named flag does not exist; HotSpot exits with "Unrecognized VM option". |
| seccomp filter *blocking* dangerous syscalls | allowlist (default `ENOSYS`) + kill-tripwire | `CONFIG_X86_X32_ABI=y` on Arch makes denylists bypassable; new syscalls stay denied. |
| SOCKS5/HTTP proxy or slirp4netns for egress | route-less netns + fake-IP DNS + transparent relay on launcher-held sockets | Minecraft's game connection (Netty) ignores Java proxy settings, so a proxy cannot both work and enforce. No extra binaries needed. |
| Token via "anonymous fd" / memfd | `AF_UNIX` socketpair on fd 0 | memfd and pipe were re-openable through `/proc/<pid>/fd` (§7 E3). |
| Refresh token "strictly in the kernel keyring" | yes (default), **plus** keyring isolation of the sandbox; optional `age` vault | An unhardened sandbox can read the keyring (§7 E2). |
| `--unshare-all --share-net` | explicit `--unshare-{user,ipc,pid,uts,cgroup}` + `--disable-userns`; net: private by default | `--unshare-all` only *tries* user/cgroup namespaces; `--disable-userns` needs explicit `--unshare-user`. |
| Hide `/sys` entirely | `/sys` contains only the GPU's own device directory and the few symlinks leading to it | Mesa cannot identify the driver without them (no hardware acceleration). |
| Instance dir read-only except four dirs | game root is a throw-away scratch dir; the four dirs are the only instance paths mounted writable; `options.txt`/`servers.dat` copied back by the launcher | A read-only root breaks settings, server list and server resource packs. The on-disk instance is still only writable in those four dirs. Set `persist_files = []` for the literal behaviour. |
| "mlock … never swapped" | per-secret `mlock`; whole-process `mlockall` only when `RLIMIT_MEMLOCK` allows | Default limit is 8 MiB; `MCL_FUTURE` would make allocations fail. Transient TLS buffers are wiped on free instead. |
| CET/IBT, `-fstack-protector-strong`, `_FORTIFY_SOURCE=3` for the binary | applied to the C objects; Rust objects get PIE, full RELRO, BIND_NOW, NX, overflow checks | Stable rustc rejects `stack-protector`/`cf-protection`; a stable Rust binary cannot be CET-marked (`-z cet-report=error` fails to link). The PKGBUILD documents the nightly path. |
| ratatui TUI / iced GUI | plain CLI | Smallest attack surface in the process that handles tokens. |

## 7. Experiments behind the claims (development machine)

| ID | Experiment | Result |
|---|---|---|
| E1 | Unrelated same-UID process reads another process's memory (`/proc/pid/mem`, `process_vm_readv`, `PTRACE_SEIZE`), `ptrace_scope=1` | Ordinary host process: **denied**. Process in a bwrap user namespace: **allowed**, also when non-dumpable. |
| E2 | `keyctl print` of keys in `@s` and `@u` from inside `bwrap --unshare-all` | Both secrets **read**. After joining a new session keyring: `EACCES`. |
| E3 | Another process opens `/proc/<pid>/fd/0` of a victim holding the token channel | pipe: **stolen**; memfd: **stolen**; socketpair: `ENXIO`. |
| E4 | SIGSEGV with `core_pattern=\|systemd-coredump` at `RLIMIT_CORE` unlimited / 0 / 1 | unlimited: core stored. 0: image still piped to the handler ("Resource limits disable core dumping"). 1: kernel "RLIMIT_CORE is set to 1, aborting core". |
| E5 | TLS 1.3-only handshake with all 17 endpoints | All succeed (including `user.auth.xboxlive.com`, `xsts.auth.xboxlive.com`). |
| E6 | Flip one bit in a stored library jar / append a byte to an asset | `INTEGRITY VIOLATION`, exit 4, launch refused; `--repair` restores a byte-identical file. |
| E7 | Launch real Minecraft 26.3 (demo mode) in the sandbox, vanilla and as a signed Fabric profile | Vanilla: runs with hardware OpenGL (radeonsi, RX 6900 XT), SDL/X11 window, audio via Pulse socket; 5 HTTPS connections relayed, 0 denied. Host view: JVM cmdline ends `… BastionBootstrap`, `Seccomp: 2`, `Seccomp_filters: 2`, `CapEff: 0`, bwrap cmdline `--args 90 -- /bastion/init __sandbox-init 91`. Fabric 0.20.1 + Fabric API: "Loading 49 mods", `/game/mods` is a read-only tmpfs holding the one verified jar; stop on SIGINT is graceful (exit 130). |
| E8 | Real signed profile: extra jar, hidden file, sub-directory, one flipped byte in the mod, mod replaced by a symlink to an identical copy, manifest edited after signing, signature removed, key untrusted, loader library tampered | All nine refused before the JVM starts (policy → exit 5, signature/integrity → exit 4). |
| E9 | A *parent* process reads its child's memory (`process_vm_readv`, `/proc/<child>/environ`), `ptrace_scope=1` | Dumpable child: **read**. Child after `PR_SET_DUMPABLE=0` (what the launcher does to itself): denied. So the shell or IDE that started the launcher cannot read its tokens either. |
| E10 | Ordinary client asks the sound server to load a module (`pactl load-module module-null-sink`), PipeWire 1.6.10 as packaged by Arch | Default configuration: **loaded** (and unloaded again). A second, private `pipewire-pulse` with `pulse.allow-module-loading = false`: **"Access denied"**; with `= true`: loaded. The launcher's probe gave `Allowed`, `Denied`, `Allowed` for the same three servers and left no module or client behind. So the default has to be treated as an open door: `audio = "pulse"` is refused until the server says no (D3). |

**Not exercised:** the live Microsoft/Xbox/Mojang sign-in (needs your Azure client ID,
Mojang's approval of it, and an interactive login — the chain is tested against scripted
replies only); joining a real multiplayer server (needs a signed-in session; the relay is
tested with a stand-in server and with live TLS to Mojang); NVIDIA; Wayland; launching Java
8-era versions (only Java 27 is installed here; their install, verification and natives
extraction are tested against the real 1.12.2 and 1.5.2); the real `age` binary (the vault
is tested against a stand-in).

## 8. Hardening not yet built

* **Credential broker**: terminate TLS to Mojang services in the launcher with a per-session
  CA, allow only the endpoints the game needs, and substitute the real token for a
  placeholder. The JVM would then never hold a usable token (closes J13 and most of P7).
  A broker alone still lets code inside the JVM request one server join per relayed game
  connection; only a login proxy that speaks the game protocol removes that too.
* **Wayland `wp_security_context_v1`** so wlroots compositors withhold privileged protocols.
* **Nested compositor for X11 sessions** (gamescope) to close D1.
* **cgroup limits** through a transient systemd scope.
* **Self-sandboxing the launcher** (Landlock) once the game is running.
* **Strict re-serialisation** of `options.txt` and `servers.dat` when the launcher copies
  them back (they are copied byte for byte today, size-limited).

Built since this list was first written: fs-verity on store files and mods (C2), hybrid
post-quantum key exchange and built-in pins for launcher TLS (N1, N3), the sound-server
module-loading check (D3; a first version that read configuration files was replaced by
one that asks the server, after it turned out to miss PipeWire's default), and the event
interface with its separate graphical front end (§4.5).
