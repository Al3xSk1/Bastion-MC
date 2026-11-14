# Bedrock launch (round 5, launch package)

The Bedrock instance kind, its launch, and the sandbox of `mcpelauncher-client`. Written for
the integrator and for the user documentation (README, `docs/BEDROCK.md`). Code:
`src/bedrock/launch.rs`, `src/bedrock/launch/{instance,profile}.rs`, the Bedrock profile in
`src/sandbox/seccomp.rs`; tests: `tests/bedrock_launch.rs` (with the stand-in client
`tests/support/bedrock_standin.rs`), `tests/bedrock_sandbox.rs`, `tests/bedrock_cli.rs`.

**Honest limits first.** There is no `mcpelauncher-client`, no game file, no Google Play
purchase and no Microsoft account on this machine, and none was installed. Everything below
that says "verified" was run against a stand-in client written for the tests, on Linux
7.2.14-arch1-1 with bubblewrap 0.13.1, curl 8.23.0 (OpenSSL 3.6.5) and glibc 2.44. Nothing
here has run the real client or the real game.

## 1. User documentation (ready for the README and docs/BEDROCK.md)

### Creating a Bedrock instance

```sh
bastion-mc instance new phone --bedrock 1.26.62.1
```

The version is an id in bastion-mc's store (`bastion-mc bedrock versions`); it may be imported
later (`bastion-mc bedrock import <directory>`), and the command says so when it is not there
yet. The instance directory gets an `instance.toml` and an empty `bedrock-data/`:

```toml
kind = "bedrock"
# width = 1280          # the window size the client opens with (-ww/-wh), both or neither
# height = 720

[bedrock]
version = "1.26.62.1"
# servers = [{ host = "play.example.org", port = 19132 }]   # added to [bedrock] servers

[network]
# mode = "filtered"     # or "none": offline single-player, nothing reachable ("open" is refused)
```

Java keys mean nothing in a Bedrock instance and are refused with the reason, not ignored:
`minecraft = …`, `memory_mb` (the session's memory ceiling is `[sandbox] memory_max_mb`),
`[network] servers` (Bedrock servers go under `[bedrock] servers`), `[companion]`. The Java-only
commands — `install`, `profile …`, `pack …` — refuse a Bedrock instance with a sentence that
says what to do instead. `instance list` shows it as `bedrock`.

### Launching

```sh
bastion-mc launch phone
```

Needs `[bedrock] accept_licence_substitution = true` (the gate of the risk package says what
that accepts). What happens, and what the launch says:

- **No sign-in by bastion-mc.** No Microsoft or Minecraft token of yours exists in a Bedrock
  session; the token store and the vault are not opened. The game signs in to Xbox itself; its
  sign-in pages are meant to be shown outside the game by the sign-in bridge (sign-in package).
- **What is kept, what is not.** The game's worlds and its own settings are kept in the
  instance's `bedrock-data/` (`games/com.mojang/minecraftWorlds/`,
  `games/com.mojang/minecraftpe/`, and the client's own `mcpelauncher-client-settings.txt`).
  Everything else the session writes — the game's Xbox sign-in included, wherever the game
  puts it — is in a data directory of that session alone and is deleted when the session
  ends. A session that could not delete it (killed, power lost) has it deleted by the next
  start of the same instance, by any later launch after a minute, and by `bastion-mc scram`.
  `[bedrock] persist_worlds = false` keeps nothing at all: every session starts empty.
- **Copying worlds in or out:** `bedrock-data/games/com.mojang/minecraftWorlds/` has the
  layout of mcpelauncher's own data directory. Only those two folders and the one file are
  ever shown to the game; anything else copied into `bedrock-data/` is not.
- **One session per instance at a time** (the worlds are one database); a second launch of
  the same instance is refused while the first runs.
- **Network.** `filtered` (the default): HTTPS by name to the Bedrock hosts of the network
  plan, through the launcher's relay; nothing else. `none`: nothing at all (single-player;
  the game cannot sign in). `open` is refused for Bedrock: the game holds the account's Xbox
  sign-in, and only the relay keeps it from reaching anything else. The instance's
  `[network] mode` wins over the global `[network] mode`.
- **Options of `launch`.** `--burner`, `--join` and `--repair` are Java features and are
  refused with what to do instead. `--demo` changes nothing for Bedrock (nobody is signed in
  by the launcher in any Bedrock session) and is accepted with a note, because the front end
  passes it whenever its demo switch is on. `--account` has no meaning for Bedrock.
- **No native mods.** The client's mod directory is empty and read-only in every session.
- **`[sandbox]` settings that apply to Bedrock:** `display`, `allow_x11`, `audio`,
  `allow_pulse_module_loading`, `gpu`, `tripwire`, `expose_timezone`, `tmp_size_mb`, `cgroup`,
  `memory_max_mb` (default for Bedrock: 6144 MiB plus the sizes of `/tmp` and the client's
  512 MiB cache), `tasks_max`, `cpu_quota_percent`, `wayland_security_context`,
  `nested_compositor`, `nested_compositor_args`. Not: `seccomp` and `extra_syscalls` (Bedrock
  has its own, under `[bedrock]`), `exec_allow`, `persist_files`, `extra_rw_dirs` (Java's).

### Taking the client's system-call profile (once)

The client has an allowlist of its own. It was derived from the client's source, not measured
on the real client (there is none here). To find out what the real client needs, run one
session with

```toml
[bedrock]
seccomp = "log"
```

The launch says loudly that the filter is in log mode. Calls outside the allowlist are then
let through and logged by the kernel: `journalctl -k -g 'type=1326'` lists them (`syscall=`
gives the x86-64 number). What still holds in log mode: the tripwire (ptrace, bpf, mount,
setns, … kill the client) and the refusal of new namespaces, the kernel keyrings, io_uring,
userfaultfd, perf events and pidfd_getfd. Put what the real client needs into

```toml
[bedrock]
extra_syscalls = ["name", …]   # names as in sandbox.extra_syscalls
```

and set `seccomp` back to `"enforce"`. The tripwire, the always-refused calls and the
argument-checked calls (`clone`, `socket`, `ioctl`, `prctl`, …) cannot be named there.

### What the protection read-out says for Bedrock

Six items (the front end shows unknown aspects under their own name):

| aspect | filtered | none |
|---|---|---|
| display | as for Java | as for Java |
| network | On — "filtered: only allowlisted Bedrock hosts, by name, through the launcher" | On — "offline: no network at all" |
| credential | **Partial** — "the game signs in to Xbox itself and holds that sign-in while it runs (deleted with the session's data at the end); no Java token exists in this session" | On — "offline: the game cannot sign in, and no Java token exists in this session" |
| sign-in | Off in this branch — "the bridge is not served in this build" (the sign-in package's server sets it) | On — "offline: no sign-in pages at all" |
| limits | as for Java | as for Java |
| launcher | as for Java | as for Java |

## 2. Threat model: rows and changes it implies

New asset (research §5.1): **the game's Xbox sign-in** — the MSA refresh token for client
`0000000048183522`, XSTS tokens, the Minecraft service token, the PlayFab session. The
client's ECDSA device key is not persisted by the client at all (`Ecdsa::restoreKeyAndId`
returns nothing in `src/jni/ecdsa.cpp`): it is generated per process.

| # | Threat | Mitigation (this package) | Residual risk |
|---|---|---|---|
| BL1 | The game's sign-in outlives the session (a stolen laptop, a later hostile session) | The whole data root (`-dd`) is the session's: a fresh directory, deleted at the end; leftovers deleted by the next start of the instance (any age), any launch after 60 s, `scram`. Only `minecraftWorlds/`, `minecraftpe/` and the client's settings file are kept. Why the whole root: the client maps every storage API of the game to that one directory (`Interop.GetLocalStoragePath`, `LocalStorage.getPath`, `getFilesDir`, `getCacheDir`, external storage, `/data/data/com.mojang.minecraftpe`), so where XAL keeps its tokens is the game's choice — `games/com.mojang/xal/` until 1.20.70.20, elsewhere since (Mojang report MCPE-178354). | Unverified that nothing in the two kept folders is sign-in material (no game here). Symptom if it were: the next session does not ask for a sign-in. Deleting a file is not a secure wipe (SSD, copy-on-write). What the game writes is bounded only by the file system's free space (as for Java). |
| BL2 | A session plants native code that the next session's client loads | The client looks for its helper libraries (`PathHelper::findDataFile("lib/<abi>/…")`) and native mods (`<data>/mods/`) in the data directory **first**; the data root is fresh every session, and `mods/` is an empty read-only tmpfs in every session. Landlock gives no write grant the execute right; nothing below a sandbox-made directory may be executed (the client is refused there). | Within one session, code already running in the client can `mmap(PROT_EXEC)` a file it wrote (Landlock governs `execve`, not mappings) — that code is the game, already running arbitrary code. Worlds are game-written content parsed by the next session (LevelDB, behaviour-pack scripts), as with Java worlds. |
| BL3 | Sign-in pages run inside the game's sandbox | The launcher's own executable is mounted at `/bastion/bin/mcpelauncher-webview` (first in `PATH`) and over every existing `Client::webview_paths` entry; Landlock lets the client execute exactly those, its own binary and libraries, and the loader. | The stub is the launcher binary: inside the sandbox it can be executed with any arguments — with exactly the game's rights (namespaces, Landlock, seccomp, no network beyond the relay), so nothing is gained. |
| BL4 | A Java credential reaches the Bedrock session | No auth step, no token store or vault opened; keyring system calls refused by the client's filter in both modes, plus the init stage's fresh session keyring; neither the launcher's data, config nor state directory is mounted. | — |
| BL5 | The client creates namespaces (escape surface, or a browser sandbox) | `unshare`, `clone3` and `clone` with any namespace flag refused in both modes; `setns` is on the tripwire; bubblewrap's `--disable-userns`. | — |
| BL6 | Log mode, meant for profiling, opens the filter | Log mode keeps the tripwire and a filter of always-refused calls (namespaces, keyrings, io_uring, userfaultfd, perf_event_open, pidfd_getfd) in front of the logging allowlist; the launch says it loudly. | Everything else outside the allowlist is allowed during a log-mode session. |
| BL7 | Two sessions of one instance corrupt the worlds or sweep each other's data | An exclusive lock on `bedrock-data/` for the whole session; session directories are locked while in use and never swept then. | — |
| BL8 | The bridge socket is reached by other programs of the user | The socket file (not its directory) is bound into the sandbox; its name on the host is removed as soon as the program in the sandbox exists (before step 9); names left by killed launches are swept by the next launch (older than a minute, nobody listening). | If the program cannot be identified in time, the name stays for the session (said as a warning). Requests through it are checked like the game's (sign-in package). |

Changes to existing rows: the "game holds the access token" class of rows does not apply
(no Java token); the self-confinement row applies unchanged (verified, see 3); the
tripwire row applies unchanged, with one caveat written into the Bedrock SIGSYS message:
the client's profile has not been taken on a real game, and a crash reporter that inspects
processes would also trip it.

## 3. What was verified by running it, and against what

All runs on this machine, with `DISPLAY=:77 XAUTHORITY=/dev/null`, no session bus, XDG
directories under the clone's `target/`.

- **The end-to-end launch** (`tests/bedrock_launch.rs`, one test, three sessions of one
  instance through `bedrock::launch::launch_with`: the real bubblewrap → init → Landlock →
  seccomp chain, the relay, no display, no systemd scope). The stand-in (a std-only program
  compiled by the test with `rustc`, started as the client) reported, and the test asserted:
  - its arguments were exactly `-dg /bastion/game -dd /bastion/data -dc /bastion/cache`;
  - the game directory reads (4096 bytes) and is read-only (EROFS);
  - the data root is writable; `mods/` is not (EROFS) and is empty; a copy of itself written
    into the data root cannot be executed (EACCES);
  - the webview the client finds first (next to its own executable, as `xal_webview_qt.cpp`
    searches) is the stub: same device and inode as `/bastion/init`, and the "installed
    webview" planted there (a program that prints a marker) never ran; the stub exited with
    status 2 (the CLI; the sign-in package's stub is not in this branch);
  - `/home` is not listable (EACCES); the launcher's data, config, state and vault
    directories and the test's directory do not exist inside (ENOENT); `/run/user/1000/bus`
    and the X socket do not exist; the abstract X socket name has no listener in the
    sandbox's network namespace (ECONNREFUSED);
  - `keyctl`, `add_key`, `unshare(CLONE_NEWUSER)`, `unshare(CLONE_NEWNET)` and
    `clone(CLONE_NEWUSER)` fail with ENOSYS;
  - an unlisted name does not resolve; `https://client.discovery.minecraft-services.net/`
    fetched with the **system's libcurl** (via `dlopen`, as the client links it) returned 200
    and the stand-in host's body — DNS through the relay, SNI checked, spliced through the
    configured SOCKS5 proxy (the test's endpoint terminates TLS there), and the certificate
    verified by libcurl/OpenSSL against the sandbox's bundle **with the stand-in CA appended
    through `NetworkAdditions::trust_pem`**; `https://api.minecraftservices.com/` did not
    resolve;
  - the bridge socket is mounted and reachable from inside (`connect` succeeded while the
    launcher still listened); its name on the host was gone after the session;
  - after the session, the world, `options.txt` and the client's settings were on the host
    under `bedrock-data/`; no `xal`, no `token.json` and no planted library anywhere under it;
    no session directory left.
  - Session 2 (after a crashed session's data root with a token was planted, and with
    `[network] mode = "none"`): the launch said it deleted 1 earlier session's data and the
    directory was gone; the world, options and settings were there; both token files and the
    planted library were not; no bridge, nothing resolved, no HTTPS; protection read-out
    network and credential "offline".
  - Session 3 (filtered, `launcher.confine = "auto"`): the launcher confined itself on all
    threads (TCP only to 53 and the proxy's port); HTTPS still worked through the relay; the
    data root was still deleted afterwards (the cache stays writable); protection read-out:
    credential Partial with "Xbox" and "no Java token", network On "filtered…", sign-in Off,
    launcher not Off.
- **The client's filter in the kernel** (`tests/bedrock_sandbox.rs`, Python payload under the
  Bedrock profile): enforcing — `munlockall` (not on the client's list) ENOSYS, the nine
  always-refused probes ENOSYS, and under the JVM's profile the same `munlockall` succeeds
  (the filters differ in the kernel); log mode — `munlockall` let through, all nine
  always-refused probes still ENOSYS; `ptrace` and `setns` kill with SIGSYS in both modes.
  One deliberately wrong expectation was run to see the test fail (it did), then restored.
- **The command line** (`tests/bedrock_cli.rs`, the real binary, isolated XDG): `instance new
  --bedrock` writes the instance (and no Java directory), `instance list` (text and
  `--events`) shows `bedrock`; `install`, `profile verify|lock|audit|check`, `pack
  list|adopt` refuse with the sentence and create nothing; `launch` is gated, `--burner`,
  `--repair`, `--join` are refused, `--demo` reaches the gate; a Java key in a Bedrock
  instance makes it "NOT LAUNCHABLE" with the reason.
- **Unit tests**: the instance shape and its refusals; the profile (mount order — data root,
  kept binds, then the sealed mod directory; the client before the stub; nothing writable is
  executable; the environment and arguments; the lock held as long as the session holds it;
  the targeted sweep deletes only this instance's unlocked sessions; deletion of a root whose
  permission bits were taken; a client below a sandbox-made directory refused); the launch
  (network mode, refused options, relay rules from the plan — the Java services unreachable,
  telemetry by its switch, a plan that would cover a denied name refused; the trust store
  with the extra CA first, garbage and oversize refused; the protection wording; the seccomp
  policy); the seccomp tables (disjoint from tripwire and refusals; the difference from the
  JVM's list written down: `mlockall`, `munlockall`; log mode keeps the tripwire program
  byte-identical to the JVM's; `extra_syscalls` cannot reopen what is always refused).
- `cargo fmt --all`, `cargo clippy --workspace --all-targets` (no warnings; the stand-in,
  which is not a Cargo target, linted with `clippy-driver`), `cargo test --workspace` (799
  passed, 0 failed, 40 binaries, before the last small commit; the Bedrock suites again
  after it), `cargo deny --offline --locked check` (ok; no dependency added).

**Not verified, and why:**

- The real `mcpelauncher-client` and the game in this sandbox: neither exists here. In
  particular not: that the client starts with these mounts, environment and arguments (the
  argument names come from `main.cpp`); that its allowlist is complete (the log mode exists to
  find out); that it accepts a read-only empty `mods/`; that SDL3/GLFW, EGL/GL and its audio
  work in here (the stand-in drew nothing and played nothing — the display path is
  `display::plan`, the same code as for Java, but no Bedrock session ran with a display, a
  GPU, PulseAudio, the nested compositor or a Wayland security context); that the real
  client's libcurl is built to use the system CA paths (the research says it uses the host's
  libcurl with no CA override; the manifest's default is a static curl with the system
  OpenSSL); where this game version keeps its XAL tokens (see BL1).
- Resource limits for a Bedrock session (the tests used `cgroup = "off"`: no systemd scope
  was created from these tests); the default ceiling (6144 MiB plus the tmpfs sizes) was
  never measured against the game.
- The sign-in itself (the bridge server is the sign-in package's, not in this branch), RakNet
  servers and Realms (the network package's).
- The front end with a Bedrock instance.

## 4. Open items

- **Integration (sign-in package):** wire the bridge server into `SignIn::prepare` (anything
  that must start before step 9) and `SignIn::serve` (step 10, the listener). Until then the
  listener is closed at step 10 and the `sign-in` item says so.
- **Integration (network package):** replace `relay_policy` with the Bedrock SNI profile
  (it is written with the relay's `service_domains`, which match a name and its subdomains:
  an exact name lets its subdomains through, a suffix its own apex); fill
  `network_additions` (CA PEM, RakNet, Realms/gatherings); pass the broker in step 10
  (`egress::start(…, None)`, marked); RakNet's UDP ports go into the netns spec and Landlock's
  `udp_send` through the policy, outbound ports into the confinement through
  `policy.outbound_ports`.
- **Integration (store package):** `runtime::discover` and `store::open` are placeholders here;
  `launch` calls them, `launch_with` takes their results (tests).
- **Front end:** for an instance of kind `bedrock`, Play should not require a Java sign-in or
  demo mode (`App::play_blocker`), and `--demo` need not be passed (it is accepted).
- **Profile the real client** once in log mode (user only), then fold the result into
  `BEDROCK_ALLOW` (or the user's `extra_syscalls`).
- Persisting more than worlds and settings (global resource/behaviour/skin packs, world
  templates) was deliberately left out; imports into the sandboxed client do not work anyway
  (its file picker and file-handler socket cannot run in here). Revisit if users ask.
- `--join` for Bedrock (the client takes `-u <minecraft:// URI>`; the URI form for joining a
  server was not verified).

## Contract notes (where the contract did not fit; for the integrator)

1. `/bastion/data` is **this session's** directory, and the kept parts are bound into it — not
   the persistent directory with a per-session token store inside it. Reason: BL1 and BL2 (one
   storage root for everything the game keeps; tokens at a place of the game's choosing; the
   client loading libraries and mods from the data directory first).
2. What is kept: `games/com.mojang/minecraftWorlds`, `games/com.mojang/minecraftpe`,
   `mcpelauncher-client-settings.txt` — "worlds and the game's settings", nothing more.
3. The bridge socket is bound as a file at `/bastion/run/signin.sock` (not the directory) so
   that its host name can be removed before step 9; the sandbox path is the contract's.
4. New key `[bedrock] extra_syscalls` (contiguous block at the end of `BedrockConfig`, default
   empty, line in `examples/config.toml`).
5. A sixth protection aspect, `sign-in`.
6. `--demo` is accepted (with a note) instead of refused.
7. The relay rules are wider than `NetPlan` until the network package's profile replaces
   `relay_policy` (see 4).
