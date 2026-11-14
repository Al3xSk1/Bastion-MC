# Round 5, the integration: Minecraft Bedrock (the Android build) in bastion-mc's sandbox

Written by the integrator on 2026-11-14, in the clone `target/agents/r5-int` (branch
`integration`), from the user's `main` at `c25c3c4` (rounds 1–4, the website, CI). Inputs: the
contract (`round5-contract.md`), the research (`bedrock-research.md`), the five packages' notes
(`bedrock-{store,net,signin,risk,launch}.md` and their `-progress.md` hand-over files) and their
structured hand-ins. The user guide is `docs/BEDROCK.md` (the risk package's, kept true to the
merged behaviour here).

**Honest limits first.** There is no `mcpelauncher-client`, no game file, no Google Play purchase
and no Microsoft account on this machine, and none was installed or contacted. Every "verified"
below ran against stand-ins: a stand-in client (a program `tests/bedrock_launch.rs` compiles and
starts as the client, in the real sandbox), stand-in versions, stand-in RakNet servers, stand-in
Realms and gatherings services over TLS, a stand-in sign-in request and final address. Section 6
lists what ran, with the numbers; section 7 what did not.

Contents: 1. the merge · 2. the reconciliations, decision by decision · 3. phase 2: the sign-in
scheme handler · 4. the wiring of a Bedrock launch · 5. contract changes and what became of them ·
6. verified by running · 7. not verified · 8. open · 9. threat-model rows, consolidated · 10. the
risk package's re-check list, answered · 11. for the documents pass.

---

## 1. The merge

Order and commits (`git merge --no-ff`; each merge built, and its package's own tests were run
before the next merge):

| Commit | Package | Conflicts | Its tests after the merge |
|---|---|---|---|
| `711bee9` | store (`r5/store` 222bcba) | none | `--lib bedrock` 49, `--test bedrock_store` 3, `--test robustness bedrock` 1 — all passed |
| `dc8c97f` | net (`r5/net` e637e0a) | `src/fuzzing.rs` (harness list), `tests/robustness.rs` (seeds, names): both kept | `--lib bedrock egress realms` 149, `--test bedrock_net` 7, `--test robustness` 36, `--test egress/voice/realms/broker` 12/6/5/15 — all passed |
| `a75bee0` | sign-in (`r5/signin` edf97e9) | `fuzz/Cargo.toml`, `src/fuzzing.rs`, `tests/robustness.rs`: both kept (see below) | `--lib bedrock::signin events realms` 66 (1 ignored: the pty probe), `--test signin_stub` 4, `--test events` 6, `--test robustness` 40, proto 13, GUI 129+5+2 — all passed |
| `370a3ee` | risk (`r5/risk` 69be7dd) | none | `--lib bedrock running auth::canary canary_tests` 118, `--test scram` 15 — all passed |
| `bf0b4ec` | launch (`r5/launch` 9e4ab4f) | none | `--lib bedrock sandbox` 171 passed, **1 failed**; `--test bedrock_cli` 4 and `--test bedrock_sandbox` 3 passed; **`--test bedrock_launch` failed** |

The conflicts were all appended lists. In `fuzz/Cargo.toml` git aligned the `[[bin]]` blocks of
both sides line by line, so "keep both" was done by hand: the store/net side with the three
sign-in bins appended. In `tests/robustness.rs` the sign-in package's seeds, names and tests were
applied onto the merged file at their places (a naive "ours then theirs" would have left the
gatherings seeds' `vec![` open).

The launch package's two failures had one cause, expected at that point: its placeholder
`relay_policy()` refuses a plan whose suffix `.minecraft-services.net` covers a denied name, and
the network package's refined deny list holds two Java Realms hosts under that suffix. The next
commit (decision 4) replaced the placeholder; from then on every commit below was green.

## 2. The reconciliations, decision by decision

One commit per step (`git log --oneline c25c3c4..HEAD` lists them all):

| Commit | What |
|---|---|
| `f4a34c6` | decisions 4 and 5: the network package's prepare/start; Realms answers through one path |
| `a8fe870` | decision 2: the version held for the session, installed webviews covered, PATH and working directory |
| `812633e` | decisions 1 and 6: gate first, `bedrock:` marker, the risk package's session directories and leftover removal, doctor on the store's listing |
| `86b246d` | decision 3: the bridge served in step 10; the stub's system calls held to the client's filter; the sign-in end to end |
| `7a2ced2` | decision 7: telemetry off sets the client's own switch |
| `417fdc0` | decision 8: the front end |
| `5ad9356` | the `bedrock_store` harness's libFuzzer target (found missing) |
| `5e04e50` | `tests/bedrock_cli.rs` follows the gate-first order |
| this commit | doctor's telemetry and TOTP lines; `docs/BEDROCK.md`; these notes |

### Decision 1 — the per-session data directory

**Done as decided.** The client's whole `-dd` root is per-session: `policy::SessionDir`
(`$XDG_CACHE_HOME/bastion-mc/bedrock-sessions/<instance>-<16 hex>`, 0700, locked by `flock` for
the session, removed at its end within 30 s by `SessionDir::remove`, and on drop). It is bound at
`/bastion/data` by a descriptor of the directory created (`Mount::BindFd`), with the kept parts
bound into it read-write from `instances/<id>/bedrock-data/` when `persist_worlds`:
`games/com.mojang/minecraftWorlds`, `games/com.mojang/minecraftpe`,
`mcpelauncher-client-settings.txt`; nothing when it is off. `mods/` is an empty, read-only tmpfs in
every session. The launch package's `Session`, `sweep_instance`, `session_dirs` and its
`scratch/<id>+bedrock-<hex>` location are gone: scram, doctor and the leftover removal each go
through the risk package's one function (`sessions_dir`, `look_at_sessions`, `remove_leftovers`).

The confined launcher still removes the directory: its `Confinement` keeps the cache writable, as
the Java launch's does, and `sessions_dir` lies in it. The confinement is now built by one function
(`bedrock::launch::confinement`) with a unit test that it covers `sessions_dir` and not the data
directory; `tests/bedrock_launch.rs` session 3 (launcher confined) shows the directory gone
afterwards.

**`mcpelauncher-client-settings.txt` checked before it was allowed to persist** (the client's
`src/settings.cpp` at the research clone's commit 7890498). Its sixteen keys: `enable_imgui`,
`enable_menubar`, `enable_fps_hud`, `fps_hud_x`, `fps_hud_y`, `enable_keystroke_hud`,
`keystroke_hud_x`, `keystroke_hud_y`, `scale`, `fullscreen`, `vsync` (UI switches and positions),
`videoMode` (compared with the display's mode list, `imgui_ui.cpp`), `menubarFocusKey` (compared
with two fixed strings), and three switches of the client's own built-in game patches:
`enable_keyboard_autofocus_patches_1_20_60`, `enable_keyboard_autofocus_paste_patches_1_20_60`,
`enable_intel_sprint_strafe_patch`. No other source file reads or writes that property list. **None
names a path, a library, a directory or a program**, so the file persists. What a session can do
with it: switch the client's own patches on or off for the next session (the patch code is the
client's), and hand the next session's parser hostile text — the same class as the worlds and
`options.txt`, which the game and the client parse too. Written into `profile.rs`'s
`CLIENT_SETTINGS` documentation.

### Decision 2 — store ↔ launch

* `runtime::discover` and `store::open` are the store package's; the launch calls them after the
  gate (they were already wired in the launch package's `launch()`).
* **The version is held for the whole session.** Before: the `OpenedVersion`'s descriptor itself
  was moved into the mount, and with it the shared `flock`, which then ended with bubblewrap's copy
  (the init stage closes every descriptor above 2) — `bedrock remove` could have removed a version
  under a running game. Now the launch keeps the `OpenedVersion` and gives the sandbox a duplicate
  (`dir.try_clone()`); it is dropped after the data root is gone. Shown end to end: while the
  stand-in waits for its sign-in, `store::remove` is refused ("in use by a running launch (it holds
  the version open)"), and it succeeds after the session (`tests/bedrock_launch.rs`, session 1,
  which now runs on a version imported with `store::import` and opened with `store::open`).
* The store's layout `versions/<id>/{manifest, game/}`, `OpenedVersion.dir` `O_RDONLY` (for the
  flock): accepted as they are.
* **Every `Client::installed_webviews()` entry is covered by the stub** (it was the raw candidate
  list before; the installed ones are the paths that exist, links resolved to the file a bind mount
  lands on).
* **PATH is exactly `/bastion/bin:/usr/bin`**, built from `runtime::SANDBOX_PATH_DIRS` (a unit test:
  no empty or relative entry). `XDG_DATA_DIRS` and `XDG_DATA_HOME` stay unset (tested), so the
  client's `findDataFile` searches the session's data root, then `/usr/local/share` and
  `/usr/share` (read-only).
* **The working directory is `/`** (read-only, as on Android) instead of the writable data root.
  Deviation from the launch package, following the store package's requirement: `PathHelper` takes
  a working directory that holds `lib/<abi>/libminecraftpe.so` for its data directory at static
  initialisation, before `-dd` is applied. `/bastion/game` would hold one — and then `main.cpp`'s
  `defaultDataDir` would point at the read-only game directory, where the client creates its
  `file_handler` socket — so `/` it is; it holds nothing of the kind. (With a fresh data root the
  old choice was harmless at start; read-only is the safer rule.)
* `store::instance_version` reads the `instance.toml` the launch package writes (`kind =
  "bedrock"`, `[bedrock] version`): tested with `instance::create_bedrock`'s file.
* **Integration fix:** `store::remove`'s marker check (`launches_using`) read only `launch:`
  markers; a Bedrock launch now writes `bedrock:` ones (decision 6), so it reads both. Tested.

### Decision 3 — sign-in ↔ launch

* The bridge is bound before the sandbox exists (`signin::Bridge::prepare` in a 0700 directory of
  this launch: `$XDG_RUNTIME_DIR/bastion-mc/signin-<12 hex>/`, or the cache without a runtime
  directory), bound into the sandbox as a **file** at `/bastion/run/signin.sock`, and its host name
  removed once the payload's pid is known. It is served with `bedrock::signin::serve(bridge,
  &reporter, log)` right after the launcher confined itself (it only accepts on that listener), and
  `Serving::shutdown()` ends it at the session's end; `shutdown` now also says whether a sign-in was
  handed to the game (that drives the after-session lines, decision 6).
* The protection read-out's `sign-in` item: On while served; Partial if the host name had to stay
  (the program in the sandbox was not identified in time); Off if the server could not start; On
  offline ("no sign-in pages at all").
* **The client's filter allows the stub.** `bedrock::signin::stub::SYSCALLS` is now the one list
  of the stub's 30 calls: `tests/signin_stub.rs` runs the stub under exactly that allowlist, and a
  unit test (`sandbox::seccomp::tests::the_bedrock_client_allows_every_system_call_of_the_sign_in_stub`)
  holds `BEDROCK_ALLOW` to it (`socket` through its argument filter, which lets `AF_UNIX` through).
* **Landlock** lets the client execute the stub (the launcher's own executable, read-only at
  `/bastion/bin/mcpelauncher-webview` and over every installed webview) and connect to the socket
  (`unix_sockets`).
* **End to end, proven** (`tests/bedrock_launch.rs`): the stand-in, doing what
  `xal_webview_qt.cpp` does, finds `mcpelauncher-webview` next to its own executable (the stub over
  the "installed webview") and runs it with a start address in the shape the game's sign-in library
  makes (PKCE S256, a state) — inside the real sandbox, under the client's Landlock and seccomp
  profile. The stub reaches the bridge; the server checks the request and announces the page
  (`SignInPage`, 10 attempts, the `login.live.com` address); at the trusted input
  (`Reporter::answer_for_test`, the path the front end's `answer signin` line takes) the address of
  another sign-in is refused with the reason and the prompt stays; the right one is accepted; the
  stand-in reads, from the stub's standard output, exactly
  `ms-xal-0000000048183522://auth?code=…&state=standin-state-1` — rebuilt from the code and the
  state, the `lc` parameter of the pasted address dropped. No launcher line holds the code. Session
  3 does it with the launcher confined and the prompt cancelled at the input (the stub prints
  nothing and exits 1); session 2 (offline) has no bridge.

### Decision 4 — network ↔ launch

The launch package's `relay_policy`, `network_additions`, `NetworkAdditions` and the
`egress::start(…, None)` placeholder are gone. In filtered mode the launch calls
`bedrock::net::prepare(Options { cfg, instance_servers, approve, proxied, announce })` before the
sandbox exists and uses what it prepared: `netns_spec()` for `Network::Filtered` (resolver on
loopback), `sandbox_tcp_ports()` for Landlock's TCP rule, `sandbox_udp_ports()` for its UDP rule,
`trust_anchor()` first in the sandbox's CA bundle, `describe()` (plus `describe_proxy` when a proxy
is set) to the status lines and the launch log, `confinement_needs()` in step 9. `start(spec,
sockets, http, log)` runs in step 10, where the Java launch calls `egress::start`; its `summary()`
goes to the log at the end, then `shutdown()`. Mode `none` calls neither; `open` stays refused.

A test's stand-in CA (the SOCKS5 stand-in host of `tests/bedrock_launch.rs`) now comes through
`Setup::extra_trust_pem` (doc-hidden; empty in every real launch), after the broker's authority.

### Decision 5 — Realms answers

One path. When joins are brokered (`[realms] approve` not `off`), the launch takes
`reporter.answers(AnswerKind::Realms)`, wraps the Java launch's announcer (moved into
`launch::realms_announcer`, shared by both launches) with `realms::answers::tracking`, and passes
it as the network package's `Options.announce`; in step 10 `realms::answers::serve(answers,
prepared.approvals(), reporter)` runs for the session and is aborted at its end.
`bedrock::net::answer_join` had no caller left and is removed; `tests/bedrock_net.rs` now approves
its joins through the trusted input and `realms::answers`, exactly as a launch does (its Realms and
event-server joins and the confined-launcher test pass that way). One thing `answer_join` did that
`realms::answers` did not: take a spaced code ("123 456", as the window and the phone show a code);
`realms::answers` now removes whitespace before it asks the approvals (tested).

`[realms] approve = "totp"` acts as `"code"` in a Bedrock session (the TOTP secret is in the token
store, which a Bedrock session never opens): `net::prepare` says so in `describe()`, which goes to
the launch log; doctor's Realms line says so too (changed here); the guide says so.

### Decision 6 — risk ↔ launch

`policy::gate` is the first thing of `launch()` and `launch_with()`; `running::Marker::bedrock`
replaces `Marker::create`; `policy::remove_leftovers(paths, Some(now + 20 s))` runs before the
session (removed ones said, failures warned, and a directory of sessions that is a link or open to
others stops the launch: no sign-in goes into it); `policy::report_after_session(reporter,
signed_in)` runs at the end with `signed_in` from the bridge server (shown: the cape lines follow
the signed-in session and not the others). Doctor's versions line uses `store::list` (manifests
read and checked; damaged entries named in a warning). `tests/bedrock_cli.rs` was adapted to the
gate-first order.

### Decision 7 — telemetry off

Verified in the source before wiring: `mcpelauncher-core`'s `minecraft_utils.cpp` —
`ReadEnvFlag("MCPELAUNCHER_DISABLE_TELEMETRY", false)` takes "true", "1" or "on" and hooks the game's
`Microsoft::Applications::Events::TelemetrySystemBase::start` with an empty function. (Read in the
copy the store package fetched, `target/agents/r5-store/target/r5-research/core/` — tree 8ada1e6;
the research clone of the client has mcpelauncher-core as a submodule that was not cloned.) With
`[bedrock] telemetry = false` the profile sets `MCPELAUNCHER_DISABLE_TELEMETRY=1` and the launch
says so; the relay refuses the telemetry hosts as before. Unit test; `tests/bedrock_launch.rs`
session 3 runs with telemetry off and the stand-in reports the switch (`1`; session 1: unset).

### Decision 8 — the front end

`App::play_blocker` asks a Bedrock instance (`kind = "bedrock"` in `instance list --events`) for
neither a Java sign-in nor demo mode, and `App::launch_request` starts it as `launch <id>`, with
neither `--account` nor `--demo` (the launcher accepts `--demo` with a note that it changes nothing;
the front end no longer sends it). The sixth protection aspect, `sign-in`: the protocol carries
aspects as strings, so older front ends show it under its own name (backward compatible); this one
knows what it covers (`kit::MORE_ASPECTS`), and the security page's lead names it. Tests: unit
(playable signed out, its command line), view (the sixth aspect with its meaning), a layout state
with a Bedrock session's six aspects at every size (snapshot looked at), and the emulated
application playing a Bedrock instance end to end against the stand-in launcher (status lists a
`bedrock` instance first, the account is signed out, demo off; the stand-in is called as `launch
phone`; the six aspects, the sign-in prompt and the Realms panel appear; answers go back as answer
lines).

Not done (polish, for later): the demo switch stays visible beside a Bedrock instance, where it
changes nothing.

### Decision 9 — CI

The test job runs `cargo test --lib`, the protocol, the front end and `--test site` in an Arch
container without bubblewrap, ffmpeg, xdg-dbus-proxy, Firefox, a systemd user manager or
`XDG_RUNTIME_DIR`, as an ordinary user. **Read:** every new or changed unit test in `src/` (49
files changed in round 5). The ones that start a program start the test binary itself
(`bedrock::policy::tests`'s locking child, `running::tests`) or `/usr/bin/sleep`
(`bedrock::store::tests`, `running::tests`) — coreutils, in the container. None starts bubblewrap,
ffmpeg, xdg-dbus-proxy, Firefox or `systemd-run`, none needs a display, a session bus or
`XDG_RUNTIME_DIR` (the bridge directory falls back to the cache, and the tests give `Paths` no
runtime directory). `bedrock::runtime::tests` read `/usr/bin/true` and pacman's database, both in
an Arch container.

**Run:** the four test steps with those programs made unavailable — `target/int-logs/ci-hide.sh`
covers `/usr/bin/{bwrap,ffmpeg,xdg-dbus-proxy,firefox,systemd-run}` and `/usr/lib/firefox/firefox`
with an empty file in a mount namespace of their own (made by bubblewrap before it is itself
covered), unsets `XDG_RUNTIME_DIR`, the display and the bus, and gives a fresh `HOME`
(`target/int-logs/ci-tests-hidden.sh`). Result: `--lib` 590 passed, 20 failed, 1 ignored; proto 13,
GUI 137, `--test site` 5 passed. **Every one of the 20 failures is an artefact of the method**, not
of a missing program: in an unprivileged user namespace root is not mapped, so root-owned files read
as uid 65534 and every test that checks root ownership fails ("must be owned by root … uid 65534":
the CA bundle for 17 of them, `/` for the store package's two production-policy tests, the JDK for
one) — 7 of them are Java-side tests from earlier rounds. The same 20 pass outside the namespace
(the full workspace run, section 6, and the clean CI simulation).

### Decision 10 — contract changes

Accepted unless they conflicted with the decisions; each is listed in section 5 with what became
of it.

### Also found and fixed while integrating

* `bedrock_store` was the one harness in `fuzzing::ALL` without a libFuzzer target; added (and the
  fuzz README names the Bedrock harnesses).
* The launch package's own unit tests of the removed pieces went with them (the placeholder relay
  rules' two tests, the scratch-directory sweep's test, `Session::remove`'s test — the risk
  package's `SessionDir` tests cover removal, permissions taken and drop); new ones were added for
  what replaced them (section 6).

## 3. Phase 2: the sign-in scheme handler

(Section written with phase 2; see below in this file once phase 2 is committed.)

## 4. The wiring of a Bedrock launch (in order)

`launch::launch` loads the instance and hands a Bedrock instance to `bedrock::launch::launch`
before anything of Java happens (no `Marker::create`, no Java sweep of its own, no sign-in, no
token store, no vault).

1. `policy::gate` (risk) — refused without `accept_licence_substitution`.
2. The Java launch options refused (`--burner`, `--join`, `--repair`; `--demo` accepted with a
   note); `runtime::discover` (store); `store::open` (store) — the `OpenedVersion`, held to the end.
3. `running::Marker::bedrock` (risk); the Java sweep of `scratch/` (as before); `launcher.confine =
   "require"` checked; the network mode (the instance's, else the global; `open` refused); the
   client's seccomp policy built (a bad `extra_syscalls` ends the launch here); the desktop planned.
4. `profile::Kept::open` (launch: the instance lock, the kept directories opened by descriptor);
   `policy::remove_leftovers` (risk); `policy::SessionDir::create` (risk).
5. The launch log opened.
6. Network, filtered: `reporter.answers(Realms)` and `realms::answers::tracking` (sign-in) when
   joins are brokered; `net::prepare` (net). Offline: nothing.
7. The trust store (the broker's authority first); the bridge (`signin::Bridge::prepare`); the
   stub opened for `/bastion/bin` and every installed webview; the base view; the profile
   (`profile::build`: game by descriptor, data root by descriptor, kept binds, sealed `mods/`, cache
   tmpfs, client, libraries, stub, bridge socket, PATH, HOME, telemetry switch, cwd `/`).
8. Limits asked of the user manager; the nested compositor (X11 sessions); the sandbox spawned;
   the Wayland context's and the bridge socket's host names removed once the payload exists.
9. The limits read back; the launcher confines itself (`confinement`: cache writable, the network
   package's outbound ports, the proxy's).
10. `signin::serve` (sign-in); the protection read-out (six aspects); `Prepared::start` (net) and
    `realms::answers::serve` (sign-in); the game's output relayed until it exits.
11. `Serving::shutdown` (whether a sign-in happened); the Realms pump aborted; the relay's summary
    and shutdown; `SessionDir::remove` (said only if true); the instance lock released; the version
    released; `policy::report_after_session` (risk); the exit code (SIGSYS: the tripwire's text).

## 5. Contract changes, and what became of them

From the five hand-ins (`contract_changes`). "Accepted" means kept as the package built it.

**Store**
* `OpenedVersion::dir` is `O_RDONLY|O_DIRECTORY|O_NOFOLLOW`, not `O_PATH` (flock needs it) —
  accepted; the launch keeps the `OpenedVersion` and mounts a duplicate (decision 2).
* Layout `versions/<id>/{manifest, game/}`, `game/` mounted — accepted.
* `Client::lib_dirs` holds the client's data directories (`/usr/share/mcpelauncher`) — accepted; bound
  read-only at their own paths.
* `Client::installed_webviews()` added — accepted; it is what the stub covers.
* `runtime::SANDBOX_PATH_DIRS` assumes PATH `/bastion/bin:/usr/bin` — accepted; PATH is built from it.
* Commands `bedrock client` and `bedrock versions --verify` added; `bedrock` not offered with
  `--events` — accepted (open item for a front end that lists versions).
* `remove` refuses through the flock and through running markers — accepted; the marker check now
  reads `bedrock:` markers too (integration fix).

**Network**
* The client's HTTP is a static curl 8.21.0 its manifest builds (downloaded without a checksum),
  not the system's libcurl; its compiled-in CA path on Arch is `/etc/ssl/certs/ca-certificates.crt`,
  which the generated trust store covers — accepted; threat row B3.
* Two calls (`prepare` before the sandbox, `start` after step 9) instead of one before step 9 —
  accepted (decision 4).
* "A one-time pass": one approval is one join, up to four flows to that server within 30 s —
  accepted (unverified with the real client).
* `[realms] approve = "totp"` acts as `"code"` — accepted (decision 5); doctor and the guide say so.
* The deny list refined (Java Realms hosts, NetherNet signalling; two under
  `.minecraft-services.net`) — accepted; it is what failed the launch package's placeholder at the
  merge.
* `BedrockServer` has no `allow_private`: LAN and same-machine servers are refused — accepted for
  this version; left open (section 8), as it needs a product decision.
* Terminated hosts: the Realms API, `pocket.realms.minecraft.net`, two gatherings hosts — accepted.
* `approve = "off"`: nothing terminated, no authority — accepted.

**Sign-in**
* `signin.rs` gained submodules (`check`, `wire`, `server`, `stub`) and re-exports — accepted.
* Checks beyond the contract's list (`response_type=code`, scope exactly Xbox sign-in, PKCE S256, a
  state; `request`/`request_uri`/`resource`/return-address parameters refused; parameter names plain
  and unique ignoring case; no `;`) — accepted. They supersede the contract's list.
* The final address goes back rebuilt from code and state, at most 1023 bytes (the client's one
  1024-byte read) — accepted; shown end to end.
* On failure the stub writes a short reason to standard error — accepted.
* "Logged once" = once per reason, totals at the end — accepted.
* New event `Answerable { kind }` — accepted (backward compatible).
* `realms::answers` lives in the sign-in package — accepted; it is the one path (decision 5).
* The optional sandboxed browser helper was not built — accepted; phase 2 builds the scheme handler
  the package recommended instead.
* Firefox shows the final address neither in the address bar nor in the console — accepted; the
  guide said "copy it from the address bar" and was corrected; phase 2 makes the paste a fallback.

**Risk**
* The token store is the data directory's root (`XboxLocalStorage.getPath` →
  `getPrimaryDataDirectory()`), so the whole per-session data directory is treated as the token
  store — accepted; it agrees with the launch package's finding (decision 1).
* `sessions_dir` and `SessionDir`, not under `scratch/` — accepted; the launch uses them.
* A marker kind of its own (`bedrock:<instance>`, `Activity::is_launch`) — accepted; the launch
  writes it; the store reads it.
* Scram's order (Bedrock leftovers after the second stop pass) and "no launch was in progress" no
  longer printed next to an unmarked launch — accepted.
* The research's canary premise was wrong (a gained cape was never a difference): the after-session
  text says a new cape needs nothing; a note names gained capes — accepted.
* There is no `cape list`; `skin show` lists capes — accepted.
* Worlds kept by scram, and said — accepted.
* The runbook's "sign out everywhere" link is already in `runbook::LINKS` — accepted (no change).

**Launch**
* `/bastion/data` is the session's directory, kept parts bound into it — accepted (decision 1),
  with the location moved to `sessions_dir`.
* Kept: worlds, `minecraftpe`, the client's settings file — accepted, the settings file after the
  check in decision 1.
* The bridge socket bound as a file at `/bastion/run/signin.sock`, its host name removed — accepted
  (decision 3; stricter than the read-only directory bind the sign-in package tested).
* New key `[bedrock] extra_syscalls` — accepted.
* A sixth protection aspect, `sign-in` — accepted; the front end shows it (decision 8).
* `--demo` accepted with a note — accepted; the front end no longer sends it.
* The network mode is the instance's, else the global; `open` refused — accepted.
* "Relay rules wider than `NetPlan` until the network package's profile replaces `relay_policy`" —
  resolved: the network package's exact/suffix/deny rules are in force.
* The Bedrock log mode keeps the tripwire and an always-refused filter — accepted.
* Additions: one session per instance (a lock on `bedrock-data/`), a client below directories the
  sandbox makes refused, `HOME=/bastion/data`, `XDG_CACHE_HOME=/bastion/cache`, `-ww/-wh` —
  accepted; **`cwd=/bastion/data` changed to `/`** (decision 2).

## 6. Verified by running

All on this machine (Arch Linux, kernel 7.2.14, Landlock ABI 10, bubblewrap 0.13.1, rustc
1.100.0), with the four XDG roots under the clone's `target/`, a private runtime directory
(`/run/user/1000/bmc-r5int-rt`), no display unless named (`:77`, the private invisible one), no
session bus.

Phase 1 (before the scheme handler), on the tree of the phase-1 commit:

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets` | no warnings |
| `cargo test --workspace --no-fail-fast` (fresh XDG roots) | 44 result lines, **978 passed, 0 failed, 2 ignored** (an earlier run on `7a2ced2`: 977 passed, 1 failed — `tests/bedrock_cli.rs` expecting the Java options before the gate, fixed in `5e04e50`) |
| the Bedrock suites (in that run) | `bedrock_launch` 1 (three sessions, section 2), `bedrock_sandbox` 3, `signin_stub` 4, `bedrock_store` 3, `bedrock_net` 7, `bedrock_cli` 4 — all passed |
| opt-in suites on the Java launch path (`target/int-logs/optin-r5.sh`, on `:77`) | `nested` 4, `wayland` 2 (headless sway), `nested_game` 1, `companion_game` ×3 (fab with a world, fab1214, fab1201) 1 each, `browser_game` 1, `clips_game` 2 — all passed; demo launches of `fabvoice`, `fabv3`, `v1214` from `target/e2e` reached the texture atlases with the launcher confined on all threads, then stopped with SIGINT (exit 130) |
| the CI test steps with the CI-absent programs hidden | decision 9: `--lib` 590 passed, 20 failed (all "owned by root … uid 65534", an artefact of the user namespace; they pass outside it), proto 13, GUI 137, site 5 |
| `cargo deny --offline --locked check` | advisories ok, bans ok, licenses ok, sources ok (the one warning is the existing unmatched `OpenSSL` allowance in `deny.toml`) |
| `python3 -I site/build.py --check` | passed: 12 pages, 916 internal links, 81 outbound links, two runs byte-identical |

`optin-r5.sh` leaves out `optin-r4.sh`'s first step (the whole workspace with
`BASTION_NET_TESTS=1`), which reaches Mojang's and Microsoft's live services.

## 7. Not verified

* **Anything with the real client, the game, a Google Play purchase or a Microsoft account** —
  none exists here. In particular: that `mcpelauncher-client` starts with these mounts,
  environment (PATH, HOME, cwd `/`, the telemetry switch) and arguments; that its allowlist is
  complete (`[bedrock] seccomp = "log"` exists to find out); that it accepts an empty read-only
  `mods/`; SDL3/EGL, sound, the nested compositor or a Wayland security context with a Bedrock
  session (no Bedrock session ran with a display, a GPU or sound); where this game version keeps
  its Xbox tokens, and that nothing of the sign-in lands in the kept folders; that the client execs
  the stub and takes its line, and that the game's sign-in library accepts the rebuilt address;
  Microsoft's real sign-in, two-factor, passkeys; real RakNet servers, Realms and event servers, the
  game's TLS stack accepting the per-launch authority; that a challenge registers with telemetry off.
* **A terminal launch of Bedrock end to end**: `bastion-mc launch` finds the client by
  `runtime::discover`, which accepts only a root-owned client in `/usr` — a stand-in cannot be put
  there. The terminal reader (answers typed on the terminal) was tested by the sign-in package's
  unit tests and a pty probe; the launch tests use a capturing reporter.
* **Resource limits** for a Bedrock session (the tests use `cgroup = "off"`), and the default memory
  ceiling against the real game.
* **Chromium-based browsers**: where they show the final address.
* **The CI on GitHub** with this tree (simulated locally, section 6).
* The libFuzzer targets were compiled (`cargo check` of `fuzz/`), not run (no nightly toolchain).

## 8. Open

* **The real client, once** (user): run a session with `[bedrock] seccomp = "log"`, fold what the
  kernel log names into `BEDROCK_ALLOW` or `[bedrock] extra_syscalls`; check that a second session
  asks for the sign-in again (if not, the token split failed and the kept folders hold sign-in
  material); measure a real version's size and verification time against `store::LIMITS`.
* **LAN and same-machine Bedrock servers** need `allow_private` in the shared `BedrockServer`: a
  product decision.
* **Featured servers** come by host name from the gatherings discovery: the user must list them; the
  broker could learn and report their names (not built).
* **Unlisted IP literals** typed into the game never reach the relay (no route), so they cannot be
  reported; a local route for all addresses in the namespace would let the relay see them (a
  `sandbox::netns` change, not made).
* **`bedrock` with `--events`** for a front end that lists or imports versions (not built).
* **The risk package's proposals**: a tripwire for a failed token split (the relay sees
  `xsts`/`sisu.xboxlive.com` reached without a bridge sign-in in that session) and `scram
  --bedrock-data` (worlds too). Neither built.
* **`--join` for Bedrock** (the client takes `-u <minecraft:// URI>`; the URI form for a server was
  not verified).
* **The front end's demo switch** beside a Bedrock instance (it changes nothing there).

## 9. Threat-model rows, consolidated (for `docs/THREAT_MODEL.md`, a new section "Minecraft: Bedrock Edition")

The packages proposed overlapping ids (store B-S1…8, network BN1…10, sign-in B-S1…8 and B-R1, risk
B1…13, launch BL1…8). Deduplicated and renumbered below as one table, B1…B39; the "from" column
names the rows merged into each. Test names are those of the merged tree. The risk package's other
additions — the asset row (§1), the adversary paragraph (§2), the trust-boundary sentence (§3),
"not defended" items 17–19 (§5), the canary deviation (§6), experiments E33 and E34 (§7) — are in
`bedrock-risk.md` §2 and fold in as they are; the integration's own experiments are proposed as
E35–E37 below the table.

(The table is in the next commit of these notes, with phase 2's rows.)

## 10. The risk package's re-check list (`bedrock-risk.md` §5), answered

1. The commands are `bedrock client | import <dir> [--id ID] | versions [--verify] | remove <id>`;
   `import` prints notes and the id; `versions` lists imported versions and what mcpelauncher-ui has.
   The guide now names all of them.
2. mcpelauncher-ui's directories: `~/.local/share/mcpelauncher/versions/<version>` (or
   `$XDG_DATA_HOME/mcpelauncher/versions/`), Flatpak `~/.var/app/io.mrarm.mcpelauncher/data/
   mcpelauncher/versions/` — as the guide says.
3. Import refuses links, hard links, FIFOs, sockets, devices, non-plain names, oversize trees, any APK;
   each launch re-hashes every file and mounts the very directory it checked. **No signer is checked**
   (no APK is kept by mcpelauncher-ui): threat row B5's residual says so.
4. `instance new <id> --bedrock <version>`; the `instance.toml` shape (`kind`, `[bedrock] version`,
   `servers`, `[network] mode`, `width`/`height`); `install`, `profile …` and `pack …` refuse — the
   guide shows it.
5. The sandbox as described (execute: client, libraries, loader, stub; empty read-only mods; display
   and sound as for Java; no `/dev/input`) — matches `profile.rs` and the base view.
6. The data directory deleted at the end; kept: `minecraftWorlds`, `minecraftpe`, the client's
   settings file — **not all of `games/com.mojang`** (the guide said so; corrected).
   `persist_worlds = false` keeps nothing. Scram's closing line ("in each Bedrock instance under
   …/instances") matches.
7. The sign-in: the front end opens the page; the terminal prints the address; **no sandboxed browser
   window** (not built; the guide's mention removed); the final address is not in Firefox's address
   bar — the guide now gives the developer-tools steps, and phase 2 the handler.
8. Answers only from the terminal of a terminal launch or the front end — true (and, with phase 2,
   the desktop's handler, which the session checks the same way).
9. Servers: `[bedrock] servers` plus the instance's, default port 19132. "When the game tries a
   server that is not listed, the launcher says so once" holds for names; **an unlisted IP literal
   never reaches the launcher** — the guide now says so.
10. LAN and NetherNet refused, with a message — true.
11. Realms/event servers: approved in the front end's Realms panel or typed on the terminal; **`totp`
    acts as `code`** — the guide said "your authenticator app" and is corrected.
12. Featured servers: not mentioned in the guide (they need listing like any server; open item).
13. Telemetry off: the base's lists refused, plus the client's own switch — the guide says so.
14. doctor's client line: matches `runtime::Client` (lib dirs, webview places).
15. The after-session lines: status events at the end of a signed-in session — shown end to end.
16. Test names for threat rows: filled in, section 9.
17. The guide's "Verified" table: filled in.

## 11. For the documents pass

(Completed with phase 2.)
