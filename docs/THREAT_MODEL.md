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
 │  ├─ auth: MSA → XBL → XSTS → MC   tokens only in locked, self-wiping pages │
 │  │                                (memfd_secret where the kernel has it)   │
 │  ├─ verified store                every file hashed; bound into sandbox by │
 │  │                                the descriptor that was hashed           │
 │  ├─ egress relay + DNS responder  holds the ONLY sockets that lead out     │
 │  ├─ credential broker             terminates the game's TLS to Mojang's API│
 │  │                                and holds the access token: the game has │
 │  │                                a placeholder                            │
 │  └─ log relay                     sanitises every byte from the sandbox    │
 │        │ fd 0: AF_UNIX socketpair (launch frame; placeholder, not token)   │
 │   before it reads anything from the game the launcher confines ITSELF      │
 │   (Landlock + a system-call filter, on all its threads)                    │
 │ ═══════╪══════════ user ns ▸ user ns ▸ pid/ipc/uts/cgroup/mount/net ns ════│
 │        ▼                                                                   │
 │  bwrap ─▶ init stage ─▶ JVM       Landlock + seccomp(allowlist+tripwire)   │
 │  empty tmpfs "/"; ro /usr; synthetic /etc; fresh /proc,/dev; no route      │
 │  rw: /game (scratch) + saves/ logs/ config/ of ONE instance                │
 │  ro: mods/ resourcepacks/ shaderpacks/ (verified files, sealed tmpfs)      │
 │  holes: 1 display socket (the game's own: security context or nested       │
 │  compositor), 1 audio socket, /dev/dri (+ its sysfs dir)                   │
 │  the whole sandbox sits in a systemd scope: memory, no swap, task limit    │
 └────────────────────────────────────────────────────────────────────────────┘
```

The JVM is treated as hostile from its first instruction. The launcher is the trusted
computing base: about 28k lines of Rust including its unit tests. `unsafe` appears only
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
| P6 | **O/U** attach to or dump the **launcher** to read tokens | `PR_SET_DUMPABLE=0` (also blocks `/proc/<pid>/{mem,environ,maps,fd}` and ancestors' ptrace), Yama, `RLIMIT_CORE=1`, tokens in `memfd_secret` pages where the kernel offers them (locked, absent from the kernel's direct map: `/proc/<pid>/mem`, `process_vm_readv` and ptrace reads fail on them even for a process allowed to attach), otherwise `mlock`+`MADV_DONTDUMP`+`MADV_WIPEONFORK` pages; a forked child never has the mapping; heap wiped on free. With the broker the launcher holds the access token for the session, in such pages; without it, no credential while the game runs. | §7 E1 (unrelated process: denied), E9 (own parent: denied once non-dumpable); `secret::tests::*` (`secretmem_pages_cannot_be_read_through_the_kernel`, `a_forked_child_does_not_see_the_secret`), `hardening::tests::*` | root; code in the kernel; cold boot. `secretmem.enable=0` falls back to locked anonymous memory (`doctor` says which). |
| P7 | **U** reads the **JVM's** memory (`process_vm_readv`, ptrace) and takes the access token | **Not defended, and cannot be by an unprivileged sandbox.** The JVM lives in a user namespace *owned by you*; the kernel grants any process with your UID in the parent namespace full capabilities over it, which bypasses both Yama `ptrace_scope` 1/2 and non-dumpable status. | §7 E1: read succeeded with `ptrace_scope=1`, with and without `PR_SET_DUMPABLE=0` | With the broker (default) the JVM's memory holds a placeholder, not the token, so there is nothing of the account to take from it (J13). For the rest of the game's memory: only `kernel.yama.ptrace_scope=3`, or running the launcher under a dedicated UNIX account, closes this — both are described in `MC_VAULT_ACCOUNT.md`. `doctor` reports the setting. |
| P8 | Core dump of the JVM lands on disk (systemd-coredump) with the token inside | `RLIMIT_CORE` is set to **1**, not 0. With a piped `core_pattern`, 0 still pipes the full image to the handler; 1 makes the kernel abort the dump. Plus `-XX:-CreateCoredumpOnCrash`. | §7 E4; `sandbox::identity…` (`core=(1, 1)`) | — |
| P9 | Token paged to swap | Launcher: locked pages. JVM: not controllable (Java heap). | `secret::tests::pages_are_locked` | JVM heap may be swapped; use encrypted or RAM-backed swap. |

### 4.2 Malicious mod / in-JVM code execution (adversaries M, S)

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| J1 | Read `~/.ssh`, browser profiles, wallets, other instances | Mount namespace with an empty tmpfs root. `$HOME`, `/run`, `/var`, `/opt`, real `/etc` do not exist. Landlock independently allows only the listed trees. | `sandbox::identity…` (`host_paths` empty), `sandbox::landlock_denies…` | Information leak: `/proc/self/mountinfo` lists the *source path* of every bind mount, so in-sandbox code can learn host directory names (including the user name in `$HOME`). It cannot open them. |
| J2 | Persist: drop a jar, edit launcher config, plant autostart | Instance root is not mounted; the game root is a per-launch scratch directory deleted on exit. Only `saves/ logs/ config/` are writable. `mods/` is a sealed read-only tmpfs of verified jars; `resourcepacks/` and `shaderpacks/` likewise hold exactly the packs the user imported (`bastion-mc pack import`: a zip archive of the one shape every reader takes for the same thing — see `profile::zipdir` —, hashed, recorded; the source is read once and what is checked and recorded is the launcher's own copy), so a pack the game or a server drops cannot install itself for the next session. No symlink/FIFO/socket/device creation anywhere (Landlock + seccomp). Launcher never follows links in sandbox-writable trees. `options.txt`/`servers.dat` are never copied: each is parsed against a strict grammar and re-encoded, in both directions (`persist`); a persisted `.json` file comes out as printable ASCII. The per-launch game root is deleted by a walk that the game cannot stop with permission bits or depth, for a bounded time, and a deletion that did not complete is reported, not assumed. | `sandbox::landlock_denies…`, `sandbox::verified_files…`, `instance::tests::*`, `profile::packs::tests::*`, `profile::zipdir::tests::*`, `persist::tests::*`, `tree::tests::*`, `robustness::settings_sanitisers`, `robustness::zip_directory_reader` | A mod that *executes scripts from `config/`* can persist inside the sandbox. A hostile *world* in `saves/` is data the game parses on the next start; `launch --burner` runs untrusted content on copies that are deleted. |
| J3 | Spawn a shell / download-and-run (`Runtime.exec("sh -c curl …")`) | Landlock grants `execve` only on the JVM home and `ld.so`. | `jvm::…` (`exec_shell=denied`), `sandbox::landlock_denies…` | In-process native code is inherent to a JVM with JNI; it gains nothing the JVM lacks. |
| J4 | Escape via kernel attack surface | seccomp **allowlist** (248 entries, 7 of them argument-filtered): everything else → `ENOSYS`. Denied incl. `io_uring_*`, `userfaultfd`, `bpf`, `perf_event_open`, `keyctl`, `unshare`, `clone3`, `open_by_handle_at`, netlink/packet/AF_ALG sockets, set*id, `mknod`, `splice`. Namespaces flags masked on `clone`. bwrap `--disable-userns`. Zero capabilities, `no_new_privs`. | `sandbox::seccomp_allowlist…`, `sandbox::landlock_denies…` (Landlock alone, seccomp in log mode) | Bugs in allowed syscalls and in the GPU driver ioctl surface. |
| J5 | Bypass a syscall filter through the x32 ABI | The Arch kernel has `CONFIG_X86_X32_ABI=y`; `nr \| 0x40000000` reaches the same handlers. An allowlist does not list those numbers; a denylist would have been bypassed. | `sandbox::x32_entry_point_is_live_on_the_host`, `sandbox::seccomp_allowlist…` (`x32_getpid=ENOSYS`) | — |
| J6 | Exploit-only syscalls (`ptrace`, `process_vm_*`, `bpf`, `mount`, `kexec_*`, `setns`, module loading…) | **Tripwire**: `KILL_PROCESS`. The launcher reports the SIGSYS death as a compromise indicator. | `sandbox::tripwire_kills…` | — |
| J7 | Read the **refresh token** from the kernel keyring | Keyrings are not namespaced: an unhardened bwrap sandbox read both test keys (§7 E2). Defence in two independent layers: init joins a fresh session keyring; seccomp denies `keyctl`/`add_key`/`request_key`. | `sandbox::kernel_keyring_is_unreachable_from_the_sandbox` (each layer alone) | — |
| J8 | Inject keystrokes into the launching terminal (`TIOCSTI`), read/poison terminal | No tty: stdio are a socket and two pipes; `--new-session`; ioctl filter; Landlock device-ioctl control. Output is stripped of control and bidi characters before it reaches a terminal or log. | `sandbox::seccomp_allowlist…` (`tiocsti`), `util::tests::sanitize_strips_escape_sequences` | — |
| J9 | Signal, trace or connect to host processes | PID namespace (sees PIDs 1 and 2); Landlock signal + abstract-socket scoping; private IPC namespace; no D-Bus, no portal, no agent sockets. | `sandbox::identity…`, `sandbox::landlock_denies…` (`signal_pid1=EPERM`), `sandbox::procfs_does_not_hand_over_the_supervisor` (bubblewrap's init: descriptors, environ, memory, root all `EACCES`) | Display and audio sockets (D1–D3). |
| J10 | **Exfiltrate** to an attacker host (webhook, C2) | Private network namespace with **no route**. The only exits are sockets held by the launcher: DNS answers only allowlisted names (synthetic addresses), TCP is relayed only to Mojang service domains on 443 *with matching SNI* and to allowlisted game servers. Behind the namespace, Landlock repeats it: TCP only to the allowlisted ports, and (Landlock ABI 10) UDP only to the launcher's DNS port, from an ephemeral port. Interface enumeration over netlink is refused; loopback is the only interface and has no hardware address. | `sandbox::filtered_network…` (incl. UDP and interface checks), `sandbox::network_none…`, `egress::*` (5 tests), real client: 5 relayed / 0 denied | Covert channels through the *allowed* endpoints: an allowlisted game server sees whatever the game sends it (chat, movement, timing), and that cannot be told apart from play. |
| J11 | Exfiltrate through DNS (`<data>.attacker.tld`) | The responder never forwards. Non-allowlisted names are answered `NXDOMAIN` locally and logged. | `egress::names_are_resolved_only_if_allowlisted…` | — |
| J12 | Domain fronting / SSRF: reuse an allowed CDN address for another site; reach router or `127.0.0.1` services | SNI must equal the name the address was handed out for; non-TLS on 443 is dropped; plain HTTP only as `GET /texture/<hex>` re-fetched by the launcher over TLS; outbound relay refuses private/loopback/link-local targets unless `allow_private`. | `egress::https_relay_requires…`, `egress::game_server_relay…`, `egress::tests::public_address_classification` | `Host`-header fronting inside TLS is invisible to any non-terminating relay (the large CDNs reject SNI/Host mismatch). |
| J13 | **Use or leak the access token from inside** | **Credential broker** (filtered mode, on by default). The game is given a per-launch placeholder. The launcher terminates the game's TLS to `*.minecraftservices.com` / `*.mojang.com` with a per-session CA that exists only in the sandbox's trust store, and attaches the real token only to an explicit list of requests (join, chat-signing key, account reads; friends, settings, reports and telemetry are separate switches, off by default). Everything state-changing that is not on the list is refused; skin, cape and name changes are refused under every configuration. A join is honoured once per open relayed game connection, for the session's own profile, and the forwarded body is rebuilt by the launcher. Replies that contain the token are withheld, and so is a reply to a credentialed request that arrives in an encoding (it could not be checked). Everything the broker holds in memory for the game — request bodies, replies the game has not read — comes out of one budget of 64 MiB over all connections. | `broker::*` (12 tests incl. a sandboxed JVM holding only the placeholder, `replies_the_game_does_not_read_come_out_of_one_bounded_budget`, `an_encoded_reply_to_a_credentialed_request_is_withheld`), `egress::broker::tests::*` (incl. `skin_cape_and_name_changes_are_refused_under_every_configuration`), `robustness::credential_broker_requests` | Code in the JVM can still *ask for* what is switched on: the account reads, and server joins. A join is spent on a game connection the relay has just opened to an allowlisted server — one join per connection — but **it is not tied to that server**: the server hash in the request is only checked for its shape. So a mod that learns the hash some *other* server is waiting for (over chat on an allowlisted server, for instance) can authorise a login as you on that third server; it can do that once for every game connection it opens, and nothing limits how many it opens over time. What it needs is at least one allowlisted game server; without one no join is ever honoured. Removing this needs a login-protocol proxy (§8). In `open` network mode, or with `network.broker.enabled = false`, the token itself is in the JVM and this row is "not prevented" as before — the launch log and the front end say so. |
| J14 | Log4Shell (CVE-2021-44228 family), JNDI codebase loading | Mojang's per-version patched Log4j config is fetched, hash-verified and mandatory; `-Dlog4j2.formatMsgNoLookups=true`; `trustURLCodebase=false` ×3; no route to attacker LDAP anyway. | `java::tests::flags…`; real client log (`-Dlog4j.configurationFile=/bastion/log4j2.xml`) | — |
| J15 | Java deserialisation gadgets (e.g. "BleedingPipe") | `-Djdk.serialFilter=!*` JVM-wide; opt-out only via config or a *signed* profile option. | `jvm::…` (`deserialize=rejected`) | Mods that install their own stream filter. |
| J16 | Attach a Java agent at runtime | `-XX:+DisableAttachMechanism`, `-XX:-EnableDynamicAgentLoading`; `-javaagent`/`-agentlib`/`-Xbootclasspath` refused from version files and signed manifests alike. | `profile::manifest::tests::rejects_dangerous…`, `java::tests::flags…` | — |
| J17 | Cross-process Spectre v2/v4 from JIT-compiled code | `PR_SET_SPECULATION_CTRL` force-disable for indirect branches and store bypass, inherited by the JVM. | init code path (best effort; not measured) | CPU/kernel dependent. |
| J18 | Code execution in the **launcher**, through a bug in what it parses from the game (DNS questions, TLS hellos, HTTP requests to the broker, output lines, what the game makes its display server print) | **The launcher confines itself before it reads any of that**, on every thread. Landlock: no file outside the instance's `persist/` directory (the settings it carries back) and its cache — not `instance.toml`, not the manifest, not the pack list, nothing of the instance at all in a burner session —, no `execve`, TCP only to the ports the relay serves, no abstract and (Landlock ABI 9) no other pathname sockets. A system-call filter: no kernel keyring (where the refresh token rests by default), no new namespaces, no `io_uring`/`userfaultfd`/`perf`/`pidfd_getfd`, no terminal-injection ioctls, the game's tripwire list kills, and the x32 and foreign-architecture entry points are closed so that the list cannot be walked around. `launcher.confine = "require"` insists on all of it; `auto` applies what the kernel can and says — in the log, in `doctor` and in the front end's shield — what is missing. | `confine::*` (4 tests, each around a real multi-threaded child process that confines itself: files, exec incl. from a memfd, ports, sockets, keyring through the ordinary and the x32 entry points, another process's command line and environment — on four threads; relaying, settings write-back, scratch removal, signalling and reaping still work; emulated ABI 8 and ABI 7), `confine::filter::tests::*`, `instance::tests::settings_files_of_an_existing_instance…`, `sandbox::nested::tests::held_output_is_not_read…`; real launches (§7 E16) | What a compromised launcher still has: **the session's access token** (the broker needs it; valid up to a day), UDP, TCP to any host on the allowed ports, **signals to every process of yours** (they are not scoped: it has to be able to stop the game, which is outside its Landlock domain — so it can also kill or freeze your other programs), read access to `/etc`, `/usr` and `/proc` (who runs what, with which command line; not another process's environment or memory: `EACCES`, tested), writing text to the terminal it was started from, the settings files in `persist/` (parsed again before the next game gets them), the launch log, and its own markers and directories in the cache (so it can hide from `scram`'s marker search — its locked game directory still gives it away, see A2). On Landlock ABI 8 the pathname-socket rule is missing (D-Bus and systemd stay reachable: a way to start a program outside the confinement), below ABI 8 only the system-call filter applies; both are reported, neither is called "confined". A system call added by a future kernel is allowed until it is put on the list. |

### 4.3 Desktop integration (the deliberate holes)

| # | Attack | Defence | Residual |
|---|---|---|---|
| D1 | **X11**: keylog every X client, inject input (XTEST) into a terminal, screenshot, read and replace the clipboard | **Nested compositor.** With gamescope installed the game is not a client of the session's X server at all: gamescope runs in a sandbox of its own (same namespaces, Landlock, seccomp, its own resource scope; no home, no network) holding the session's display, and the game gets only the inner display's socket. Its root window is gamescope's; the session's windows, properties and cookie do not exist for it. gamescope would bridge CLIPBOARD and PRIMARY both ways and has no switch for it; a small preloaded library (`native/noclip.c`, built and embedded with the launcher) makes the four SDL calls that carry the bridge do nothing. A preload can fail without a sound, so the library reports from inside the compositor and the launcher claims the cut only for the report "active"; without it `auto` says that the clipboard is not known to be cut (shield: partial) and `require` stops the launch. The game's `DISPLAY` is the socket's *path*, so libxcb never tries an abstract socket and Landlock's abstract-socket scoping stays on in every network mode. Without gamescope the launch is refused unless `sandbox.allow_x11 = true` hands over the session's display (one socket file, one cookie). | `nested::the_game_talks_to_a_display_server_of_its_own` (both network namespaces), `nested::no_selection_crosses_the_nested_compositor`, `nested::a_compositor_without_the_cut_off_is_noticed_and_not_called_cut` (the control run: without the library CLIPBOARD crossed both ways, the launcher reported "was not loaded" and `require` refused), `nested::a_compositor_that_goes_away_is_noticed`, `nested_game` (real Minecraft 26.3), `sandbox::x11_client_connects_in_each_network_mode` | A bug in gamescope or Xwayland reachable from the game yields the compositor's sandbox, i.e. the session's display — the thing that used to be handed over. The report "active" establishes that the bridge's calls land in the cut-off, not that a given build of gamescope has no other way of moving a selection: that is a property of the compositor, tested for gamescope 3.17 as packaged by Arch and for nothing else. **With `allow_x11` and no gamescope this is a sandbox escape path**, as before. Playability inside gamescope (pointer capture, full screen) is not assessed. |
| D2 | **Wayland**: privileged protocols on wlroots compositors (screencopy, virtual keyboard and pointer, data control = clipboard managers, layer shell) | **`wp_security_context_v1`.** The game does not get the session's socket but one created for it and registered with the compositor as a sandbox (engine `org.bastion-mc`); a compositor that implements the protocol is expected to withhold the interfaces meant for trusted desktop components from it — the protocol says "should", so the launcher does not assume it: it connects to the game's socket itself and lists what is offered there. If one of 16 known privileged interfaces is on that list (or the list cannot be had), `auto` says so and the shield is not shown as on; `require` refuses. The socket's name is removed once the sandbox has it. `wayland_security_context = "require"` also refuses compositors without the protocol; `auto` says so and falls back. The setup conversation with the compositor is bounded in time, events and bytes. | `sandbox::wayland::tests::*` (stand-in compositors, incl. one that accepts the context and filters nothing, one that never stops sending, one that never accepts), `wayland::*` against a real headless sway (37 interfaces offered on the game's socket, none of the known privileged ones; 20 withheld), real Minecraft started through it (§7 E16), `robustness::wayland_event_reader`; `doctor` shows the same listing | The ordinary clipboard while the game has focus (that is Wayland). A privileged interface that is not on the launcher's list — a compositor's private protocol — is not noticed; nor is what a compositor does with requests on the interfaces it does offer. Compositors without the protocol — Mutter (GNOME), Weston, Wayfire, per the protocol registry's support table on 2026-11-13; KWin, Sway, Hyprland, niri, COSMIC, labwc and river are listed as implementing it — get the session socket, with whatever that compositor offers every client (GNOME offers ordinary clients no capture or injection interfaces, but that is GNOME's policy, not this launcher's enforcement). Tested against sway only, headless; not on a real desktop session. |
| D3 | **Audio**: record the microphone, capture other apps' audio; make the sound server load code | Only the PulseAudio-protocol socket (never the native PipeWire socket, which also offers cameras/screen capture). That protocol has a "load module" command which PulseAudio *and* pipewire-pulse honour by default; with it a client can make the server — outside the sandbox — load a plugin or touch files it names. So before the socket is exposed the launcher asks the server itself to load a module that does not exist: only a clean "access denied" lets the launch continue (`allow_pulse_module_loading = true` overrides, `audio = "none"` removes the socket). Tested by `sandbox::pulse::tests::*` (stand-in servers: refused, accepted, garbage, hang-up) and against real PipeWire in both settings (§7 E10). | Recording is possible with `audio = "pulse"`; exfiltrating it still needs J10. The answer is checked at launch, not continuously. |
| D4 | **GPU**: driver ioctl bugs | `/dev/dri` only (+ NVIDIA nodes if present), device-ioctl right limited to those nodes, sysfs limited to the GPU's own device directory. | Kernel GPU drivers are large; this surface cannot be filtered generically. |

### 4.4 Network and supply chain (adversaries N, C)

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| N1 | MITM / downgrade of launcher traffic | TLS 1.3 only (TLS 1.2 is not compiled into rustls), hybrid post-quantum key exchange (X25519MLKEM768) offered first, system CA store from a fixed root-owned path (`SSL_CERT_FILE` ignored), no redirects, no plaintext, fixed host allowlist, bounded bodies, no decompression. No proxy unless `network.launcher_proxy` names a SOCKS5 proxy by literal address; then the proxy resolves the names and carries the bytes, and TLS (same version, same pins) is still end to end with the named host. | `tls::*` (7 tests incl. `a_socks5_proxy_carries_the_connection_and_resolves_the_name`), `net::http::tests::url_policy`; hybrid key exchange observed against Mojang's hosts (Microsoft's sign-in hosts still pick a classical group) | Recorded traffic to hosts that do not negotiate the hybrid group stays exposed to a future quantum adversary. |
| N2 | Poisoned DNS for launcher or relay | TLS for everything the launcher fetches; relay refuses private targets; game traffic is integrity-protected by Minecraft's own session handshake. | — | DoS; being steered to an attacker-run *game server* (contained by §4.2). |
| N3 | A trusted CA mis-issues a certificate for a Mojang/Microsoft host | SPKI pins, matched against the **validated** chain (a decoy copy of the pinned certificate does not help the attacker). Built in and on by default for Microsoft/Mojang hosts: the seven root and long-lived intermediate keys those services chain to, so ~120 other CAs no longer count there. `tls.builtin_pins = false` turns them off; `[tls.pins]` adds or replaces. | `tls::presenting_the_pinned_certificate_as_an_unused_extra…`, `net::tls::tests::builtin_pins_cover_microsoft_hosts_and_match_the_system_store` | If Microsoft moves to a root outside the list, connections fail closed until the list is updated or the pins are disabled. Other hosts (Fabric, Quilt) are unpinned unless you pin them. |
| C1 | Tampered client jar / library / asset (CDN or local store) | Hash chain from `version_manifest_v2.json`; verified on download (never reaches its final path otherwise) and again on every launch. Jars, asset index, logging config and mods are hashed **from the descriptor that is then bind-mounted**; asset objects are re-hashed and mounted as a read-only directory. Mismatch aborts; no automatic re-download without `--repair`. | `mojang::install::tests::*`, real store: bit-flip → exit 4 (§7 E6), `sandbox::verified_files…` | Mojang publishes SHA-1 only: second-preimage resistance is what is relied on. |
| C2 | Unsigned, modified or extra mod jar | Curated instances need an Ed25519 signature (`verify_strict`, domain-separated) by a key in the trust store over the manifest; `mods/` must equal the signed list exactly — no extra file of any kind, no symlinks, exact size and SHA-256. | `profile::*` (15 tests), `instance::tests::curated…`, §7 E8 | On filesystems with fs-verity (btrfs; ext4/f2fs with the feature) every store file and mod jar is made immutable *before* it is hashed, so there is no window. Elsewhere: in-place modification by **U** between check and mount of a file U can write (U can do far worse). |
| C3 | Replay an old signed manifest; move a manifest to another instance | Monotonic `serial` remembered per instance; manifest is bound to the instance id. | `profile::signing::tests::rollback…`, `instance::tests::curated…` | State file is user-writable. |
| C4 | **U** adds its own signing key | If `/etc/bastion-mc/trusted-keys.d` (root-owned) holds keys, per-user keys are ignored. | `profile::signing::tests::*` | Without system keys, **U** can add a key. |
| C5 | Malicious JVM | Only root-owned runtimes under `/usr/lib/jvm`; nothing is ever downloaded. | `java::tests::*` | — |
| C6 | Loader library with no published hash | `profile new` pins what it downloads (TLS) and tells you which hashes were cross-checked and which are trust-on-first-use; your signature freezes them. | `profile::author::tests::*` | TOFU at authoring time. |
| C7 | Malicious dependency of the launcher | About 100 crates in the launcher, no OpenSSL/native-tls; the only C is AWS-LC (the rustls crypto provider: C plus pre-generated assembly compiled here from the pinned crate, which ships no prebuilt objects; never a system copy) and the 40-line clipboard cut-off; `Cargo.lock` + `--locked/--frozen` builds; the Java bootstrap is compiled from source at build time, no binary blobs. `deny.toml` makes the mechanical part repeatable (`cargo deny check`: advisories, permissive licences only, crates.io only, OpenSSL/native-tls/ring/reqwest banned); the package embeds its dependency list in the binaries (`cargo auditable`). The graphical front end's larger tree (about 200 crates) is linked only into its own, secret-free binary (§4.5). | `cargo tree -p bastion-mc`; `cargo deny check` (2026-11-13: advisories ok, bans ok, licenses ok, sources ok; two build-time crates in two versions) | Inherent to using crates.io. |

### 4.5 Graphical front end

The window, its fonts and its GPU context live in `bastion-mc-gui`, a separate process that
only ever sees what `bastion-mc --events` prints.

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| G1 | A bug in the GUI toolkit, text shaping or a GPU driver (reachable by hostile text: game output is displayed) is used to read tokens | Tokens never exist in the GUI process. The launcher is not linked into it and links none of its dependencies (iced, winit, wgpu, … about 200 crates); the wire protocol (`crates/proto`) has no credential field in either direction; the GUI crate forbids `unsafe`. | `cargo tree -p bastion-mc` contains no `iced`/`winit`/`wgpu`; `proto::tests::the_wire_format_is_stable`; `events::*` tests | The GUI process can still be made to *click* (start a launch, sign out): an integrity, not a confidentiality, problem. |
| G2 | Hostile game output attacks the console: escape sequences, megabyte-long lines, floods | Sanitised and length-bounded in the launcher; the GUI strips control characters again, refuses lines over 64 KiB without buffering them, keeps 800 lines. A reader that falls behind makes the launcher *drop* output (counted, still in the log file) rather than stall the game. | `backend::tests::over_long_lines_are_dropped_without_buffering_them`, `…::events_strays_and_exit_status_arrive_in_order`, `console::tests::*`, `events::tests::long_text_is_clipped_and_stays_one_line` | — |
| G3 | The sign-in prompt is used to make the desktop open an attacker's URL | "Open link" exists only for `https://` links to four Microsoft sign-in hosts (exact host, no port, no user part, printable ASCII) and passes the URL as one argument to `xdg-open`. The launcher refuses device codes that are not 4–32 alphanumerics and verification links that are not HTTPS. | `app::tests::only_microsoft_sign_in_links_are_opened`, `view::tests::an_untrusted_link_gets_no_open_button`, `msa::tests::device_flow_stops_on_refusal_and_rejects_plain_http_links` | — |
| G4 | The GUI crashes or is killed while a game runs, leaving an unsupervised sandbox | The launcher's standard input is its control channel; end-of-file means "stop". The game is shut down gracefully (15 s, then killed) and the launcher exits. Closing the window does the same and waits for it. | `backend::tests::a_vanished_ui_stops_the_launcher`, `events::a_pending_sign_in_is_abandoned_on_stop_and_when_the_front_end_disappears`, `app::tests::closing_the_window_stops_a_running_game_first` | — |
| G6 | The emergency panel is used to open an attacker's page, or scram is triggered by a stray click | Links: a compiled-in list of twelve exact addresses (`proto::runbook::LINKS`), compared for equality — no host or prefix matching; a test keeps the list and the runbook text identical. Scram needs two different buttons in sequence and removes things only. | `proto::runbook::tests::*`, `app::emergency_tests::*`, `view::tests::the_emergency_panel_asks_before_scram_and_offers_only_the_runbook_links` | Scram can be triggered by anything that can click in the window (G1): a denial of service — you have to sign in again. |
| G5 | **U**, or any other X11 client, reads the device code or the sign-in link off the window, or injects clicks | Not defended (same-user software, and X11 by design). The code and the link are useless without your Microsoft credentials, and the browser sign-in's `state` only protects against *other users*. | — | See §5.1 and §5.2. |

### 4.6 The account itself (after a compromise elsewhere)

The rows above are about this machine. An account is more often lost somewhere else — a
phished password, a "verification" app that was given consent. The launcher cannot prevent
that; it can notice it and make the response quick.

| # | Situation | What the launcher does | Verified by | Limits |
|---|---|---|---|---|
| A1 | Someone else changed the account: player name, skin, cape | **Profile canary.** The profile Mojang returns at every sign-in is compared with the pinned one before the token is used for anything. A difference that was not made through this launcher stops the launch (`auth.canary = "enforce"`, the default) until `bastion-mc canary accept`; a damaged pin file is an alarm, not a first sight. A change made with `skin set` / `cape set` pins exactly the part that command changed — not whatever else the profile has become in the meantime, which stays a difference. With `auth.canary = "warn"` every command that opens a session says so (also under `--quiet`, and in the launch log) and the pin stays where it was. `canary_wipe` also deletes the stored refresh token. | `auth::canary::tests::*` (incl. `an_own_change_pins_its_own_aspect_and_leaves_foreign_changes_unpinned`), `launch::canary_tests::*` | A tripwire for *visible* tampering only: an intruder who just plays trips nothing. The pin is a file of the user: it guards against the account changing elsewhere, not against software on this machine. Read-only towards Mojang. |
| A2 | The user suspects a compromise and wants every credential off this machine now | **`bastion-mc scram`** (and the front end's emergency panel). First it stops everything that could still *write* a credential: running launches (which takes the game, its display server and the in-memory session token with it) and pending sign-ins and skin, cape, canary or vault commands — each found by pid *and* process start time, signalled through a pidfd, and reported as killed only when the kernel confirmed its end. Then the credentials: every refresh token in the kernel keyring (the kernel's own list is read before and after) and every age vault file (the directory is read again afterwards to confirm that none is left); identities are forgotten. Last, and for at most 15 seconds, the game directories that killed launches left behind. Whatever could not be done or confirmed is a line starting `COULD NOT`, and the exit status is then 1 — also when the markers of running commands could not be read, which is not taken for "nothing was running". No network needed. | `scram::*` (9 tests, incl. two login sessions, a pending sign-in, an undeletable directory, a launch without a marker, a command that cannot be stopped, markers that cannot be read), `running::tests::*`, `tree::tests::*`; real launch: SIGKILL of the launcher ended the JVM and the scope (§7 E13) | **Revokes nothing at Microsoft or Mojang**, and deleted files are not securely wiped — its output says both every time. A token copied earlier works until "sign out everywhere"; an issued session token lives up to ~24 h. A key that only another login session of yours still holds cannot be removed from here (reported as such). Markers are files in the cache: a program running as you — or a launcher that was compromised — can delete one; scram then does not find that launch by its marker, sees that its game directory is still locked, and reports a launch it could not stop instead of killing it. |
| A3 | Under pressure the user searches the web for recovery pages and lands on phishing | **Built-in runbook** (`bastion-mc wiki scram`, `docs/SCRAM_RUNBOOK.md`): the steps and the exact Microsoft addresses, taken from Microsoft's own help articles; the front end opens those addresses and no others (G6). The warning to use another device if this one is in doubt comes first. | `proto::runbook::tests::*` | Links were checked on the date in the file and can go stale; Minecraft's help centre could not be read by a program, so only the existence of its request form was checked. A compromised machine can show a different text. |
| A4 | Ownership dispute with support | **`bastion-mc export-proofs`**: the player id, every name/skin/cape state the canary accepted with dates, and when the account signed in and played here. | `proofs::tests::*` (asserts no credential in the output) | Only what this launcher saw. No IP addresses or hardware details are kept, deliberately. |
| A5 | The game changes the skin/cape/name (J13) or a skin file leaks data about the user | From the game: refused by the broker under every configuration. From the launcher: `skin set` decodes the image to its pixels and encodes it again; what is uploaded is that new file — the image's height and pixel values, and nothing else that was in the original (no text, time stamps, EXIF or private chunks, no palette, no bytes behind the pixel data). | `egress::broker::tests::skin_cape_and_name_changes…`, `auth::minecraft::tests::*` (incl. `nothing_but_the_pixels_can_ride_along`), `robustness::profile_reply_and_skin_files` | The colour values of fully transparent pixels are part of the image and are uploaded with it (the game shows some of them). |

## 5. Not defended — read this before relying on it

1. **Other software running as you (U).** It can read the game's memory (P7), read the
   refresh token from the kernel keyring (same login session), replace the launcher binary
   in `$PATH`, or edit `config.toml`. That includes the **age vault**: a hardware-backed
   identity makes a *copy* of the vault file useless without the device, but it does not
   stop such software from asking the connected device to decrypt (a touch or PIN policy
   on the device is all that stands in the way), and it has no say over the *recipients* —
   a recipients file of your own can be extended, and the token stored at the next launch
   is then encrypted to the added recipient too, without any touch. Only a root-owned
   `/etc/bastion-mc/age-recipients` takes the recipients (and the choice of store) out of
   its reach; `doctor` says which case applies. That file closes the quiet, lasting
   variant — one line added once — and nothing more: software that puts itself between you
   and the launcher (another `bastion-mc` earlier in `$PATH`, a wrapper, a preloaded
   library) has the token in the moment you launch, whatever the vault is encrypted to.
   On this development machine the account is also in
   the `docker` group, which makes any same-user process root-equivalent. A user-level
   launcher cannot defend the user from themself; a dedicated UNIX account for gaming can
   (`MC_VAULT_ACCOUNT.md`).
2. **X11 sessions without gamescope** once `allow_x11` is set (D1). With gamescope the
   game is on a display of its own.
3. **The session token inside the JVM** when the broker is not in use: `open` network mode
   or `network.broker.enabled = false` (J13). With the broker, what remains is that code in
   the game can request what is switched on: account reads, and one join per game connection
   it opens — a join that is good for a login on *any* server, not only the one the
   connection goes to (J13).
4. **Microphone** with Pulse audio enabled (D3). (Module loading through the same socket is
   closed: the launcher refuses the socket unless the server denies it.)
5. **Kernel and GPU-driver vulnerabilities** reachable through allowed syscalls/ioctls.
6. **Denial of service, partly**: memory, swap and the number of tasks are limited by a
   systemd scope (the game is killed inside its own group; the limits are read back from
   the kernel once the game runs, and `sandbox.cgroup = "require"` stops a launch whose
   limits are not in force), and CPU time can be (`cpu_quota_percent`). A hostile mod
   can still fill `saves/` and the scratch directory up to the free space of those file
   systems, and use all CPUs unless a quota is set. On the launcher's side: its log is
   bounded per source (the game cannot crowd the relay's or the launcher's lines out of
   it), the number of launch logs and of relay connections is bounded, and what the
   credential broker holds in memory for the game is bounded at 64 MiB in all. Not under
   that budget: the texture gateway, which holds up to 4 MiB per connection while a
   texture is fetched and handed over (at most 256 connections).
7. **Realms and UDP**: Realms servers are handed out as bare IPs and are not reachable in
   filtered mode; UDP (voice-chat mods, LAN discovery) does not exist in the sandbox.
8. **Covert channels to an allowlisted server.** Whatever the game may send to a server you
   allowlisted can carry data out; the relay sees an encrypted stream and cannot tell.
9. **Hardware fingerprinting.** Host name, machine id and network interfaces are synthetic
   or absent, but the GPU's renderer string, the CPU model in `/proc/cpuinfo` and the
   amount of memory are the real ones.
10. **The account at Microsoft.** Nothing here revokes a session or secures the Microsoft
    account; §4.6 is about noticing and reacting.

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
| Instance dir read-only except four dirs | game root is a throw-away scratch dir; three dirs (`saves/ logs/ config/`) are the only instance paths mounted writable, `resourcepacks/` became read-only with an import command; `options.txt`/`servers.dat` are re-encoded by the launcher in both directions | A read-only root breaks settings, server list and server resource packs. A game-writable pack directory lets a pack install itself. Set `persist_files = []` for the literal behaviour. |
| "mlock … never swapped" | per-secret locked mapping (`memfd_secret`, or `mlock`); whole-process `mlockall` only when `RLIMIT_MEMLOCK` allows | Default limit is 8 MiB; `MCL_FUTURE` would make allocations fail. Transient TLS buffers are wiped on free instead. |
| Launcher "holds no credential while the game runs" | with the broker it holds the access token for the session (in secret memory, in a process that has confined itself) and the game holds none | The alternative is the token inside the JVM, readable by every mod. |
| Landlock UDP rules for the launcher itself | sandbox only | The launcher's DNS responder answers to the game's ephemeral ports; a port rule cannot express "any port on this socket". |
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

| E11 | An X client in the game's sandbox reads/owns CLIPBOARD and PRIMARY while a session client does the same, through gamescope 3.17 | Without the preloaded library: CLIPBOARD crossed in both directions, PRIMARY from the game to the session. With it: nothing crossed, in 4 of 4 directions; the game can still own its own selections. |
| E12 | `xprop -root` in the game's sandbox with `DISPLAY=:0` and the host's network namespace, inner display also `:0` | "unable to open display": libxcb tried the session's abstract socket of the same name, Landlock refused it, libxcb did not fall back. With `DISPLAY=/tmp/.X11-unix/X0`: connects to the inner display. Same result for a client of the session's own X server (cookie written without a display number). |
| E13 | `SIGKILL` to the launcher of a running demo launch (what `scram` does) | The JVM and the systemd scope were gone within two seconds; the game root left in the cache was removed by the next launch's sweep. |
| E14 | `process_vm_readv` on a token buffer of the same process | `memfd_secret` backing: refused, buffer untouched (an ordinary buffer in the same call: read). A forked child reading the address: `SIGSEGV`. |
| E15 | UDP from the sandbox in filtered mode, kernel with Landlock ABI 10 | Port 53 on loopback: answered by the launcher. Any other port (`sendto`, `connect`), IPv4 and IPv6: `EACCES`. `bind` to a chosen port: `EACCES`; to an ephemeral one: ok. |
| E16 | Real demo launches of Minecraft 26.3 after the start-up was reordered (confinement before anything the game sends is read): through gamescope 3.17 on a private X display (driven with `--events`), through a security context on headless sway (the game's window came from SDL's Wayland driver), as a `--burner` session, and the signed Fabric profile through gamescope | In each: the scope's limits were read back from the kernel; the launcher reported itself confined and `/proc/<launcher>/status` showed `Seccomp: 2`, `Seccomp_filters: 3`, `NoNewPrivs: 1` (13 to 17 threads); no line from the game, the relay or the compositor was logged between the game's start and that point. In the launch driven with `--events` the `Protection` event came after it: display, network, credential, limits, launcher — each `on`, with what was seen as its detail. Afterwards the broker and the relay served the game's requests (5 connections relayed, 0 denied; none in the burner session, which has no network). SIGINT ended the game (exit status 130); `persist/options.txt` was written back by the confined launcher (not by the burner session, which reported everything the game wrote as deleted); the game root and the marker were gone. The Fabric profile loaded its 49 mods; its `options.txt`, still in the instance directory from before `persist/` existed, was moved there at that launch and said so. |
| E17 | The same two archives given to the zip crate (9.5.1) and to `java.util.zip.ZipFile` (OpenJDK 27): one whose end record declares 1 member in front of a directory with 3 headers, one with the same name in two headers | Declared 1 of 3: the zip crate lists 1 member, Java lists 3 (including a `../../` name). Same name twice: the zip crate keeps 1, Java lists 2. Minecraft 26.3 opens resource packs with `java.util.zip.ZipFile` (`FilePackResources`, from its bytecode). The strict reader refuses both archives. |
| E18 | A process exceeds `MemoryMax` in a transient scope under a wrapper process (systemd 262, `DefaultOOMPolicy=stop`) | The kernel kills the process; the user manager then stops the scope, and its SIGTERM reached the wrapper before the wrapper had reported its child's death. So after a group OOM kill the launcher may see the sandbox end by a signal instead of with status 137; which of the two it sees is a race. |

**Not exercised:** a kernel with Landlock ABI 8 or older (the partial states are produced
on ABI 10 by asking for less, not observed on such a kernel); a launch whose resource
limits are *not* in force, or whose Wayland socket still offers a privileged interface
(the decisions are tested against a fake control-group hierarchy and a stand-in
compositor; neither situation exists on this machine); a signed-in launch after the
start-up was reordered (that the launcher needs neither the token store nor a new program
once it has confined itself was established by reading the code — every launch run here
was a demo or burner launch); the root-owned recipients file with real root ownership
(tested with a stand-in path; it takes root); the
live Microsoft/Xbox/Mojang sign-in (needs your Azure client ID,
Mojang's approval of it, and an interactive login — the chain is tested against scripted
replies only); joining a real multiplayer server (needs a signed-in session; the relay is
tested with a stand-in server and with live TLS to Mojang); NVIDIA; launching Java
8-era versions (only Java 27 is installed here; their install, verification and natives
extraction are tested against the real 1.12.2 and 1.5.2); the real `age` binary and its
hardware plugins (the vault is tested against a stand-in); everything that needs a
signed-in account — the broker against the real Mojang API (it is tested against a stand-in
Mojang and a real sandboxed JVM), `skin`/`cape` changes, the canary against a real profile;
a real Wayland desktop session (Wayland was exercised against a headless sway only);
playability inside gamescope; the libFuzzer targets (no
nightly toolchain here — the same harnesses run as generated-input tests instead); a
`mc-vault` account and `ptrace_scope=3` (`MC_VAULT_ACCOUNT.md`).

## 8. Hardening: what was built, what is not

**Built** (each was on this list as "not yet"): the credential broker (J13), Wayland
`wp_security_context_v1` (D2), the nested compositor for X11 with its clipboard cut-off
(D1), resource limits through a systemd scope (§5.6), the launcher confining itself once
the game runs, strict re-serialisation of `options.txt`/`servers.dat` (J2). Before those:
fs-verity on store files and mods (C2), hybrid post-quantum key exchange and built-in pins
(N1, N3), the sound-server module-loading check (D3), the event interface with its separate
front end (§4.5). Since: read-only packs with an import step (J2), Landlock UDP rules
(J10), `memfd_secret` token memory (P6), the profile canary, scram and the runbook (§4.6),
an optional SOCKS5 upstream (N1), `launch --burner`, per-account vault keys and
`vault reseal`. After the two independent reviews of 2026-11-13 (`docs/notes/review-fixes.md`):
the launcher's system-call filter and the reordered start-up (J18), limits read back from
the kernel, the listing of the security-context socket and the clipboard cut-off's report
(D1, D2), a strict zip directory reader for packs (J2), decoded-and-re-encoded skin
uploads (A5), the broker's memory budget (J13), scram's order and its "nothing is reported
that was not confirmed" (A2), and the root-owned recipients file for the age vault (§5.1).

**Not built**, with the reason each is still open:

* **Login-protocol proxy.** With the broker, code in the JVM can still spend one join per
  game connection it opens, on a server of its choosing (J13). Removing that means the launcher performs the join itself,
  which requires terminating Minecraft's protocol encryption (the shared secret is
  encrypted to the server's key, so a relay cannot compute the server hash by watching):
  being the client towards the server and the server towards the game, per protocol
  version. Large and version-sensitive. It would also be the only place a packet-rate
  tripwire on game traffic could look at more than sizes and timing.
* **Signed server allowlist / out-of-band approval of new servers and mods.** A second
  factor kept on this machine is readable by the user it is meant to stop. The meaningful
  form is the mod-manifest model applied to the allowlist: signed with a key that is off
  the machine or in hardware.
* **In-game companion (Fabric) for approvals.** Enforcement would stay in the launcher —
  a mod runs in the same JVM as what it guards against — so the mod is only an interface;
  the gates it would front (packs: built; servers, account requests: per-switch today) do
  not depend on it.
* **Hardware normalisation** (CPU count, `/proc/cpuinfo`, memory size): the GPU renderer
  string comes from the real driver and cannot be normalised by mounts, so the result
  would still be fingerprintable.
* **An immutable, verified `/usr` and JVM for the sandbox** (EROFS/composefs): needs a
  root-side component, against the unprivileged design.
* **A security-key dead man's switch**, **shadow stacks for the Rust code** (nightly-only
  flags; the kernel enforces user-space shadow stacks only when glibc opts the process in,
  and not IBT at all), **seccomp user notification** (must not decide on pointer
  arguments; Landlock already enforces paths and ports), **padding of sign-in traffic**
  (the observer sees the destination hosts anyway; rustls 0.23 has no record-padding setting),
  **cross-registry hash consensus and a bytecode capability diff for mod updates** (aids
  for the person who signs a profile, not enforcement).
* **Wayland compositors without the security-context protocol**: `auto` falls back to the
  session socket. Running gamescope there too (`--backend wayland`) is the candidate fix.
