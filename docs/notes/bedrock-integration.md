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
| `79a10eb` | phase 1 documents: doctor's telemetry and TOTP lines; `docs/BEDROCK.md`; these notes |
| `7d0cfde` | phase 2: the sign-in scheme handler (section 3) |
| the last commit | phase 2 documents: the guide's sign-in section, these notes' sections 3, 6, 9 and 11 |

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
(`target/int-logs/ci-tests-hidden.sh`). Result (phase 1): `--lib` 590 passed, 20 failed, 1 ignored;
proto 13, GUI 137, `--test site` 5 passed; on the final tree: `--lib` 597 passed, the same 20 failed,
1 ignored; proto 13, GUI 138, site 5. **Every one of the 20 failures is an artefact of the method**, not
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

**Why.** The sign-in package found the "paste the final address" step unusable in practice:
Firefox shows `ms-xal-0000000048183522://auth?code=…` neither in its address bar nor in its console
(only the developer tools' Network panel lists it, as the failed 302's `Location`). Its notes
(section 4) recommended a scheme handler; built here (`7d0cfde`).

**What.**

* **The desktop entry** `packaging/arch/bastion-mc-bedrock-signin.desktop`:
  `MimeType=x-scheme-handler/ms-xal-0000000048183522;`, `NoDisplay=true`, `Terminal=false`,
  `Exec=/usr/bin/bastion-mc bedrock hand-back %u` (an absolute path: the package installs the
  launcher there). Installed by `package_bastion-mc` into `usr/share/applications/` (its `install`
  line). **`update-desktop-database` is triggered by pacman's own hook**: checked on this system,
  `/usr/share/libalpm/hooks/update-desktop-database.hook` (owned by desktop-file-utils 0.28-1)
  triggers on Install/Upgrade/Remove of `usr/share/applications/*.desktop` and runs
  `/usr/bin/update-desktop-database --quiet` after the transaction. The PKGBUILD names
  desktop-file-utils as an optional dependency for that reason, and now also installs
  `docs/BEDROCK.md` with the other documents (it did not). `desktop-file-validate`: valid.
* **The command** `bastion-mc bedrock hand-back <address>` (`signin::handback::hand_back`): refuses
  what is not the redirect's own address (printable, ≤ 4096 bytes, `ms-xal-0000000048183522://auth?…`)
  before anything is sent; without `XDG_RUNTIME_DIR`, or with no session holding the handler, says
  so; otherwise sends the address as one line and prints the session's verdict — one sentence, exit
  0 (handed to the game) or 1 (anything else, a missing argument included). It reads no
  configuration, opens nothing, writes no file, and never prints the address (it holds the code).
  Not offered with `--events`.
* **The socket** `$XDG_RUNTIME_DIR/bastion-mc/bedrock-signin.sock` (`signin::handback::take`): the
  directory 0700 (`create_private_dir_all`, which refuses a link or a directory open to others),
  the socket 0600, bound **before step 9** by the launch (`SignIn::prepare`, before the sandbox
  exists), after an exclusive `flock` on `bedrock-signin.lock` beside it. From step 10 the sign-in
  server accepts on it (paced and bounded like the bridge, its own budget; the peer's user id
  checked with `SO_PEERCRED`) — it binds, connects and executes nothing after step 9. The game's
  sandbox has no path to it (none of the runtime directory is mounted; the e2e checks the host path
  is `ENOENT` inside).
* **The session treats an address from the handler as a trusted answer for the waiting prompt**:
  the same `check::check_final` (under the redirect URI, a code, the waiting sign-in's state), the
  same outcome (accepted: the stub gets the address rebuilt from code and state, the prompt ends
  `done`; refused: the prompt says why and stays). Refusals logged once per reason, totals at the
  end. **One deliberate difference: a refused hand-back does not use up one of the prompt's 10
  answers.** Any web page can make a browser call the handler (after the user's consent the first
  time, maybe without asking later); such an address carries no state of this sign-in and cannot
  get through — but with attempts counted, ten of them would end the user's prompt.
* **Two Bedrock sessions at once: the second falls back to paste.** The lock gives the handler to
  one session; a second session started while it runs takes pasted addresses only, for its whole
  session, and its launch log ("another running Bedrock session takes the addresses your browser
  hands back …; in this session paste the address …") and its prompt (`SignInPage.handback =
  false`) say so. Why not address each session uniquely: the redirect URI is fixed by Microsoft's
  client registration (nothing per session can be put in it); the handler cannot tell which session
  a browser tab belongs to; the only key is the address's `state`, so the command would have to
  hand one sign-in's code to every running session and each would have to recognise and ignore the
  others' — more code on the credential path for a rare case whose fallback works. A name left by a
  session that could not remove it (a confined launcher may not remove files in the runtime
  directory; a killed one removes nothing) is removed by the next session that takes the lock: with
  the lock held, the name belongs to nobody. An unconfined session removes its name at its end,
  before it releases the lock.
* **The protocol**: `SignInPage` gains `handback` (`#[serde(default, skip_serializing_if =
  "Not::not")]`): absent when false, so a line from before reads the same and an older front end
  ignores the field (tested both ways in the proto crate).
* **The front end**: the panel's control is "Open the sign-in page" (in the user's browser); where
  this session holds the handler, below it: "When you have signed in, the page hands the result
  back to bastion-mc by itself, where bastion-mc's handler for these links is registered (the Arch
  package registers it; your browser may ask once whether to open such links with bastion-mc: allow
  it)", and the paste instructions under "If nothing happens"; without it, the paste instructions
  alone. The terminal's text likewise. The paste field stays in both.
* **Within the confinement rules**: the handler's socket is a pre-bound listener; after step 9 the
  launcher only accepts on it and writes the verdict to the accepted connection.

**Tested.**

* `signin::handback` unit tests: one session holds the handler, a second at the same time falls
  back, a stale name is removed by the next holder and an unconfined holder removes its own; the
  socket 0600 in a 0700 directory; something other than a socket at the name is refused, not
  removed; the command refuses foreign addresses (other hosts, another path, empty, control
  characters) before sending anything, says "no session" without a runtime directory or socket, and
  reports the session's verdicts (exit 0 / 1, never the code); the packaged desktop entry names the
  scheme, `NoDisplay`, the exact `Exec`, and the PKGBUILD's install line.
* Server tests: with nothing waiting, "no Bedrock sign-in is waiting"; an address of another
  sign-in refused with the reason, the prompt still waiting with all 10 answers and a note; the
  right one handed to the game (the stub's connection gets the rebuilt address); logged as coming
  from the handler, the code nowhere; after the session the handler is gone (`NoSession`).
* `bedrock::launch` unit test: a second session's `SignIn::prepare` does not get the handler and
  says so; without a runtime directory there is none; offline there is no bridge.
* **End to end** (`tests/bedrock_launch.rs`): session 1 shows the prompt with `handback: true` and
  the handler's host path missing inside the sandbox; after session 2 (offline) the real binary's
  `bedrock hand-back` with no session says "nothing was done: no Bedrock session that takes sign-in
  addresses is running" (exit 1); **session 3, with the launcher confined**, is answered by the real
  `bastion-mc bedrock hand-back`, run by a helper started before the session (so before the
  confinement), as the desktop would run it: a foreign address refused by the command (exit 1),
  another sign-in's refused by the session ("its state is not this sign-in's", exit 1; the prompt
  still waiting with 10 answers), the right one handed to the game (exit 0); the stand-in reads the
  rebuilt address from the stub; the after-session cape lines follow.
* The front end: view tests (both texts; "Open the sign-in page" opens the page; the paste field
  stays), a layout state with the hand-back text at every size (snapshot looked at), the emulated
  application with a stand-in launcher whose prompt carries `handback: true`.
* The desktop integration, by hand, in isolated directories under the clone's `target/` (no file of
  the user's touched; the entry's `Exec` pointed at the debug build): `update-desktop-database`
  wrote `x-scheme-handler/ms-xal-0000000048183522=bastion-mc-bedrock-signin.desktop;` into
  `mimeinfo.cache`; `xdg-mime query default` and `gio mime` named the entry as the default; `gio
  open 'ms-xal-0000000048183522://auth?code=M.C5&state=s'` ran the command, which printed
  "bastion-mc: nothing was done: no Bedrock session that takes sign-in addresses is running".

**Not verified**: a real browser following Microsoft's last redirect into the handler (Firefox
asks the system through GIO; that path was exercised with `gio open`, not with a browser); the
real package installed by pacman (makepkg built it, section 6; nothing was installed).

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

**The final tree** (after phase 2; code as of `7d0cfde`, documents as of the last commit):

| Check | Result |
|---|---|
| `cargo fmt --all --check`, and of `fuzz/` | clean |
| `cargo clippy --workspace --all-targets` | no warnings |
| `cargo test --workspace --no-fail-fast` (fresh XDG roots, no display, no session bus) | 44 result lines, **986 passed, 0 failed, 2 ignored** (the two ignored: the terminal reader's pty probe, and the usage snippet in `realms/answers.rs`'s documentation, an `ignore` doc-test) — the library's unit tests 617 (1 ignored), the GUI 131 + 5 (emulated) + 2 (live, without its variables), the protocol 13 |
| the Bedrock suites (in that run) | `bedrock_launch` 1 (three sessions: the sign-in pasted in session 1, handed back by the real `bastion-mc bedrock hand-back` in session 3 with the launcher confined), `bedrock_sandbox` 3, `signin_stub` 4, `bedrock_store` 3, `bedrock_net` 7, `bedrock_cli` 4, `robustness` 40, `scram` 15, `events` 6 — all passed |
| opt-in suites again (`optin-r5.sh` on the final binary, `:77`) | `nested` 4, `wayland` 2, `nested_game` 1, `companion_game` ×3 1 each, `browser_game` 1, `clips_game` 2 — all passed; the three demo launches reached the texture atlases with the launcher confined on all threads (12, 12 and 14 atlas lines), stopped with SIGINT (exit 130) |
| the CI simulation (`target/int-logs/ci-sim-r5.sh`: `ci-sim.sh` with the CI's `--test site` step added and `cargo deny --offline`; a clean target directory, a fresh `HOME`, no XDG variables, no runtime directory, display or session bus) | every step exit 0: fmt, fmt of `fuzz/`, `clippy -D warnings`, the test build, `--lib` **617 passed, 0 failed**, proto 13, GUI 138, `--test site` 5, `cargo check` of `fuzz/`, deny |
| the CI's test steps with bubblewrap, ffmpeg, xdg-dbus-proxy, Firefox and `systemd-run` hidden (`ci-tests-hidden.sh`) | `--lib` 597 passed, 20 failed, 1 ignored — the same 20 root-ownership artefacts as in phase 1 (decision 9), none for a missing program; proto 13, GUI 138, site 5 |
| `cargo deny --offline --locked check` | advisories ok, bans ok, licenses ok, sources ok; no dependency was added in round 5 (`Cargo.toml` and `Cargo.lock` unchanged since `c25c3c4`) |
| `python3 -I site/build.py --check` | passed: 12 pages, 916 internal links, 81 outbound links, two runs byte-identical |
| `target/int-logs/makepkg.sh` (the real PKGBUILD on a `git archive` of HEAD, `makepkg -f -d`) | exit 0; three packages built; `check()`: `--lib` 617 passed (1 ignored), proto 13, GUI 131, the hardening check of both binaries all "ok" (PIE, RELRO, immediate binding, NX stack, no W+X segment, no RPATH, no text relocations); the `bastion-mc` package holds `usr/share/applications/bastion-mc-bedrock-signin.desktop` and `usr/share/doc/bastion-mc/docs/BEDROCK.md`, and names desktop-file-utils among its optional dependencies (the only warning: makepkg's "Skipping dependency checks", from `-d`) |
| the handler's desktop entry (E37) | `desktop-file-validate`: valid; `update-desktop-database`, `xdg-mime`, `gio mime` and `gio open` in isolated directories, section 3 |

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
* **The CI on GitHub** with this tree (simulated locally, section 6), and its manual integration
  workflow.
* **The packages installed** (`makepkg` built them; nothing was installed with pacman, so the
  pacman hook was read, not run on this package).
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

| # | Threat | Defence | Verified by | Residual risk | From |
|---|---|---|---|---|---|
| B1 | **Licence check and DRM**: bastion-mc used to defeat or answer a licence check, or to decrypt the game | No circumvention in bastion-mc's code: it decrypts nothing, patches no game file, answers no licence query, contains no Google Play client. `mcpelauncher-client` answers the game's Google Play query itself, after `mcpelauncher-ui` asked Google at download. Every launch refused until `[bedrock] accept_licence_substitution`; the refusal says what is accepted and what bastion-mc does not do. The Windows (GDK) build is not supported in any form | `bedrock::tests::bedrock_is_refused_until_the_licence_substitution_is_accepted`, `bedrock::policy::tests::the_gate_names_what_is_accepted_and_what_bastion_does_not_do`, `tests/bedrock_cli.rs::a_bedrock_launch_is_gated_and_refuses_the_java_launch_options` (the gate before everything) | bastion-mc cannot check that an imported version was bought. `mcpelauncher-client` changes the game's code in memory to run it | risk B1 |
| B2 | **The client program** (C): a user-writable, swapped or foreign client | Root-owned, every directory and link hop root-owned and writable by nobody else, not under the home, not on FUSE, an x86-64 ELF, not set-user-ID; Flatpak (by location, configuration and layout) and AppImage (magic, mount) refused, each with the reason; never downloaded or built | `bedrock::runtime::tests::*` (14) | `[bedrock] client` is the user's file: a same-user program can point it at another root-owned program. Unaudited native code that patches the game, trusted as much as any package | store B-S1, B-S2; risk B2 |
| B3 | **The client's build** (C): its HTTP stack | — (a finding) | read in the manifest | mcpelauncher's manifest builds curl 8.21.0 statically, downloaded **without a checksum** (`ext/curl.cmake`, `USE_OWN_CURL=ON` by default), and the AUR PKGBUILD keeps that default. An AUR PKGBUILD is user-submitted: read it before building | net BN10; risk B2 |
| B4 | **Import pulls a private file in** (a link or hard link in the download directory) | Links, hard links, FIFOs, sockets, devices refused in what is copied; every name opened relative to a directory descriptor with `O_NOFOLLOW`; device/inode/size/times compared between scan and copy | `bedrock::store::tests::links_hard_links_fifos_and_sockets_in_the_copied_part_are_refused_by_name`, `…::a_file_that_changes_between_the_scan_and_the_copy_ends_the_import` | — | store B-S4 |
| B5 | **Tampering with an imported version** between sessions, or between check and use | Per-file SHA-256 manifest written last; every launch re-hashes every file, refuses anything unlisted, linked or hard-linked; the checked descriptor is what is mounted (a duplicate, held with the version for the session) | `…::open_finds_a_changed_added_removed_or_replaced_file`, `…::open_refuses_a_damaged_or_missing_manifest_or_game_directory`; `tests/bedrock_launch.rs` (session 1 runs on a version opened by `store::open`) | Whatever can write the store can write a matching manifest: the check makes files checkable, not authentic. **No signer is checked** (mcpelauncher-ui keeps no APK). Checked at launch, not after | store B-S5; risk B3 |
| B6 | **A hostile download directory** exhausting disk, inodes or time | Bounded entries, bytes (checked before copying: a sparse file refused at once), depth, libraries, manifest size; free space first; staging removed on failure | `…::the_import_stops_at_its_limits_before_copying` | The bounds are guesses: no real version measured | store B-S6 |
| B7 | **Removing a version a running session uses** | `store::open`'s shared `flock`, held by the launch for the whole session (the sandbox gets a duplicate); `remove` takes it exclusively; also refused while a `bedrock:` (or `launch:`) marker's instance names the version | `…::remove_refuses_while_a_launch_holds_the_version_and_removes_it_after`, `…::remove_refuses_while_a_marked_launch_of_a_bedrock_instance_names_the_version`; `tests/bedrock_launch.rs` (refused while the stand-in waits, possible after) | A launch that lost both its lock and its marker can have its version removed under it (the game then fails; nothing else) | store B-S7 |
| B8 | **Parsers of imported bytes** (compiled XML, the store's manifest, ELF headers, pacman records) | Strict, bounded, fuzzed (`bedrock_store`: a stable robustness test, and a libFuzzer target) | `tests/robustness.rs`, unit tests of `store::axml`, `store::manifest`, `store::elf` | libFuzzer targets compiled, not run (no nightly) | store B-S8 |
| B9 | **Hostile game, server or content (S), native mods (M), against your files and session** | The Java sandbox with a profile of its own: namespaces, empty root, `/usr` read-only, synthetic `/etc`, no home; Landlock: only the client, its libraries, the loader and the stub execute, nothing writable is executable; the client's own system-call allowlist with the tripwire; the display only through the security context or the nested compositor; no `/dev/input`, hidraw; cgroup limits; the mod directory empty and read-only | `tests/bedrock_launch.rs` (the stand-in: no home, no host paths, no keyring, written code not executable, `mods/` sealed), `tests/bedrock_sandbox.rs` (3), `bedrock::launch::profile::tests::*` | The allowlist was derived from the client's source, never profiled on the real client (`[bedrock] seccomp = "log"` exists for that). Within a session, code already running in the client can `mmap(PROT_EXEC)` a file it wrote (Landlock governs `execve`). Kernel and GPU driver surface; same-user memory reading (P7) | risk B4; launch BL2 (in part) |
| B10 | **The client creates namespaces** (an escape surface, or a browser sandbox) | `unshare`, `clone3`, `clone` with namespace flags refused in both filter modes; `setns` on the tripwire; bubblewrap's `--disable-userns` | `tests/bedrock_launch.rs` (`unshare`, `clone(CLONE_NEWUSER)` → ENOSYS), `tests/bedrock_sandbox.rs` | — | launch BL5 |
| B11 | **Log mode, meant for profiling, opens the filter** | Log mode keeps the tripwire and an always-refused filter (namespaces, keyrings, io_uring, userfaultfd, perf events, `pidfd_getfd`) in front of the logging allowlist; loud in the launch; `extra_syscalls` cannot name those | `sandbox::seccomp::tests::bedrock_filters_compile_fit_and_keep_the_tripwire_in_log_mode`, `…::bedrock_extra_syscalls_cannot_reopen_what_is_always_denied`, `tests/bedrock_sandbox.rs::log_mode_lets_the_rest_through_but_keeps_refusing_namespaces_keyrings_and_io_uring` | Everything else outside the allowlist is allowed during a log-mode session | launch BL6 |
| B12 | **A session plants native code for the next one** (the client loads helper libraries and mods from its data directory first) | The whole data root is per-session (fresh, deleted at the end); `mods/` empty and read-only every session; only worlds, `minecraftpe/` and the client's settings file persist (none of its keys names a path or program); the working directory is `/`; `XDG_DATA_DIRS` unset | `tests/bedrock_launch.rs` (a library planted in session 1 is gone in session 2; `mods/` not writable), `bedrock::launch::profile::tests::*` | Worlds and `options.txt` are game-written content parsed by the next session (as with Java worlds); the settings file can switch the client's own patches for the next session | launch BL2; store's finding |
| B13 | **The client's legacy Microsoft sign-in path** (`msa-daemon`, for old versions and telemetry upload) | Landlock lets the sandbox execute only the client, its libraries, the loader and the stub | the Landlock rules (`nothing_writable_is_executable_and_the_stub_and_client_are`); not exercised with an `msa-daemon` | Not tested with a real `msa-daemon` | risk B13 |
| B14 | **Two sessions of one instance** corrupt its worlds or remove each other's data | An exclusive lock on `bedrock-data/` for the whole session; session directories locked while in use and never removed then | `…::one_instance_runs_once_at_a_time_and_its_kept_directories_must_be_plain`, `bedrock::policy::tests::leftovers_of_killed_sessions_are_removed_and_running_ones_are_not` | — | launch BL7 |
| B15 | **The game's Xbox identity carried away** (S, M) | Cannot be prevented: the game's sign-in library needs it. Narrowed by the filtered network (B28–B36) and by no Java credential in the session (B17) | the network rows' tests | **The game holds the account's Xbox identity for the whole session.** A game taken over can use it and copy it out through any allowed connection; the copy works until "sign out everywhere" (Microsoft: up to 24 h). At stake: joining as you, your Realms (owner's rights), Minecoins, your Bedrock character and cape, your Xbox friends; possibly your Java profile (not verified) | risk B5 |
| B16 | **The game's sign-in at rest** (U, a later session) | The client's whole `-dd` root is the token store (`XboxLocalStorage.getPath` → the data directory's root): per-session (`policy::SessionDir`, 0700, locked), deleted at the end (bounded, said only if true); leftovers deleted at the next Bedrock start (`remove_leftovers`, which refuses a sessions directory that is a link or open to others) and by scram; doctor warns | `bedrock::policy::tests::*`, `tests/scram.rs` (Bedrock tests), `tests/bedrock_launch.rs` (tokens gone, a crashed session's directory deleted at the next start, the directory deleted with the launcher confined) | Where the real game writes its tokens was read in the client's source, not observed: if the game wrote a token into `minecraftWorlds/` or `minecraftpe/`, it would persist (symptom: no sign-in asked next time). During a session, and after a crash until the next start or scram, it is a file of the user's. Deleting is not a secure wipe | risk B6; launch BL1 |
| B17 | **A Java credential in a Bedrock session** | None exists: no Java sign-in, no token store or vault opened; no path to them in the sandbox; `keyctl`/`add_key` refused by the client's filter; the relay denies the Java services | `tests/bedrock_launch.rs` (host paths ENOENT, `keyctl`/`add_key` ENOSYS), `bedrock::net::tests::the_java_services_are_denied_even_where_a_suffix_would_reach_them` | If the game can turn its Xbox identity into a Java token (not verified), a copy taken out (B15) works elsewhere | risk B7; launch BL4 |
| B18 | **A real webview (a browser engine) inside the sandbox** | The stub (the launcher's own executable) at `/bastion/bin/mcpelauncher-webview`, first in PATH (`/bastion/bin:/usr/bin`, no empty entry), and over every installed webview (`Client::installed_webviews`: beside the client, `/usr/bin`, pacman's records, a manual install's tree, links resolved); no browser could run anyway (namespaces refused) | `tests/bedrock_launch.rs` (the client's first candidate is the stub; the planted "installed webview" never ran), `bedrock::runtime::tests::a_packaged_client_is_found_with_its_data_directory_version_and_every_webview_place` | A client compiled with an unusual `XAL_WEBVIEW_QT_PATH` and a webview installed by hand outside the client's tree under `/usr` would be missed (and then could not start a browser in the sandbox anyway) | store B-S3; launch BL3; risk B8 (part) |
| B19 | **The game makes the launcher show a phishing page** or another application's consent | Only `https://login.live.com/oauth20_authorize.srf`, client `0000000048183522`, redirect `ms-xal-0000000048183522://auth`, `response_type=code`, scope exactly Xbox sign-in, PKCE S256, a state; no `request`/`request_uri`/`resource`/return-address parameters; names plain and unique; no `;`. The front end opens only Microsoft sign-in hosts | `bedrock::signin::check::tests::*` (7), `tests/signin_stub.rs::a_page_bastion_does_not_show_leaves_the_stub_silent_on_standard_output` | Cosmetic parameters (`login_hint`, `display`, `prompt`, `mkt`, …) are the game's choice; the scope check must be reviewed after the first real sign-in | sign-in B-S1; risk B8 |
| B20 | **The game answers its own prompt** | Answers come only from outside the game: the front end's control pipe, the terminal (foreground only), and the desktop's handler (a 0600 socket in the user's 0700 runtime directory, which the sandbox cannot reach: not mounted, no route; the peer's uid checked); the sandbox starts a new session (no `TIOCSTI`), both filters deny the terminal-injection ioctls | `events::input::tests::*`, `tests/bedrock_launch.rs` (the handler's host path ENOENT inside) | The user can be talked into pasting something; the state check limits that to an address of this very sign-in | sign-in B-S2 |
| B21 | **Prompt spam** | One prompt at a time, 6 per session, 10 s after one ends without an address, 15 min each; connections paced and read by at most 4 tasks | `bedrock::signin::server::tests::one_prompt_at_a_time_and_a_bounded_number_per_session`, `…::a_cancelled_prompt_cannot_be_reopened_at_once`, `…::a_flood_of_connections_is_paced_and_the_prompt_still_works` | A game can show 6 prompts per session | sign-in B-S3 |
| B22 | **Hostile bytes on the bridge socket** (the game can connect without the stub) | Frames ≤ 16 KiB, length checked before allocation, exact JSON shapes, URLs parsed strictly; fuzzed | `bedrock::signin::wire::tests::*`, harnesses `signin_frame`, `signin_url` | — | sign-in B-S4 |
| B23 | **The sign-in code exposed on the way back** | Single-use, useless without the game's PKCE verifier; never logged, never in an event; only the rebuilt `code`+`state` goes to the game; the hand-back command never prints it | `…::a_sign_in_is_shown_outside_the_game_and_only_its_rebuilt_address_goes_back`, `tests/bedrock_launch.rs` (no launcher line holds the code) | The terminal echoes a pasted address; the front end shows it in its field; after use the code is worthless | sign-in B-S5 |
| B24 | **The stub as a way out** | It reads no configuration or environment, writes no file, needs 30 system calls (`stub::SYSCALLS`), all in the client's allowlist; it is not a boundary — every check is the launcher's | `tests/signin_stub.rs::the_stub_needs_no_system_call_beyond_its_documented_list`, `sandbox::seccomp::tests::the_bedrock_client_allows_every_system_call_of_the_sign_in_stub` | Inside the sandbox the stub can be run with any arguments — with exactly the game's rights | sign-in B-S6; launch BL3 |
| B25 | **Another program of the user uses the bridge** to phish a sign-in through bastion-mc's prompt | The bridge socket is bound into the sandbox as a file and its host name removed as soon as the sandbox holds it; names left by killed launches swept | `bedrock::launch::profile::tests::a_killed_launchs_bridge_socket_is_swept_and_a_live_one_is_not`, `tests/bedrock_launch.rs` (the name gone after the session) | If the program in the sandbox is not identified in time the name stays for the session (a warning; the read-out says Partial). Same-user code is otherwise out of reach (P7): the prompt cannot tell a request from the game from one by another program of the user | sign-in B-S7; launch BL8 |
| B26 | **A backgrounded terminal launch stopped by its own reader** (`SIGTTIN`) | The reader reads only while the launcher is the terminal's foreground job | `events::input::tests::only_a_controlling_terminal_owned_by_another_job_is_left_unread`, a pty probe | A job backgrounded between the check and the read is stopped until brought back | sign-in B-S8 |
| B27 | **The desktop's handler**: a web page, another program of the user or another session hands the waiting sign-in an address | The handler's address is checked exactly as a pasted one (under the redirect, a code, this sign-in's state); a refused one does not use up the prompt's answers and is logged once per reason; the socket is 0600 in the user's 0700 runtime directory, bound before step 9, accepted on after it (paced, bounded, peer uid checked); one session holds it (a lock), a second falls back to pasting and says so; the command never prints the address | `bedrock::signin::handback::tests::*` (4), `bedrock::signin::server::tests::an_address_the_browser_hands_back_is_checked_like_a_pasted_one_and_handed_to_the_game`, `bedrock::launch::tests::a_second_bedrock_session_falls_back_to_pasting_and_says_so`, `tests/bedrock_launch.rs` (session 3, confined) | A web page can make the browser call the handler (after the user's first consent): it cannot get an address through without this sign-in's state, but its refusal shows in the prompt. Any program of the user can connect to the socket (P7), as it could type into the terminal. With two sessions, the second session's browser hands its address to the first, which refuses it (wrong state); that user pastes | integration (phase 2) |
| B28 | **The game reaches other HTTPS hosts** | No route out of the namespace; the resolver answers only listed names (exact, sub-domain suffixes, minus the deny list) with synthetic addresses; the ClientHello must name the looked-up host; Landlock: TCP to 443 only | `egress::bedrock::tests::*`, `tests/bedrock_net.rs`, `tests/bedrock_launch.rs` (an unlisted name does not resolve; `api.minecraftservices.com` does not) | Everything below `.xboxlive.com`, `.minecraft-services.net`, `.playfabapi.com` is reachable: services that hold the Xbox identity, through which a hostile game can exfiltrate (B15) | net BN1 |
| B29 | **The game sends UDP elsewhere** | No route; datagrams reach only the relay's sockets (Landlock ABI 10: DNS, the listed ports, the pass's); a flow goes only to the address resolved for a listed server or to an approved join, opens only with a RakNet ping or connection request; 32 flows, 1472-byte datagrams, 500/s to and 4000/s from a server per flow, 30 s idle | `egress::raknet::tests::*`, `tests/bedrock_net.rs::a_raknet_client_reaches_the_listed_servers_and_their_answers_come_from_where_it_sent` | A listed server receives whatever the game sends within those limits. Without Landlock ABI 10 the game may send to any port inside its namespace (where only the relay listens) | net BN2 |
| B30 | **A poisoned name lookup turns the relay against the LAN** | Non-public addresses refused for servers and approved joins; one lookup per name per session | `tests/bedrock_net.rs::nethernet_private_addresses_host_names_and_unknown_join_answers_are_refused_with_the_reason` | — | net BN3 |
| B31 | **A Realms or event-server join without the user** (an attacker's Realm), and without the phone | Join answers held (TLS terminated for four hosts only); approval outside the game by the code shown, typed at the trusted input (the terminal or the front end's Realms panel; `realms::answers`, never the game); the pass leads to that one server (one join, up to four flows within 30 s); `totp` acts as `code` (the TOTP secret stays in the token store) | `tests/bedrock_net.rs::a_realms_join_waits_for_the_approval_and_then_the_pass_reaches_exactly_that_server` (approved through the trusted input), `…::an_event_server_join_from_the_gatherings_service_is_approved_the_same_way`, `realms::answers::tests::*` | The user must read the prompt; a game can raise up to 20 prompts per session (a nuisance). In `code` mode the code binds the approval to the prompt it was shown with | net BN4; risk B9; sign-in B-R1 |
| B32 | **A bug in the TLS termination** | Four hosts only; rustls (TLS 1.3) and hyper; bodies bounded; no credential held; join answers parsed strictly, fuzzed (`bedrock_realms`, `bedrock_gatherings`) | `egress::services::tests::*`, `tests/robustness.rs` | The broker holds the game's Realms and Minecraft tokens in memory while forwarding them (never logged); a compromised launcher could use them — the game holds them anyway | net BN5 |
| B33 | **The per-launch authority used against other hosts** | Name constraints: the four hosts; made in memory per launch, never written, only in this sandbox's trust store (first in its bundle) | `egress::services::tests::the_authority_signs_for_the_four_hosts_and_its_constraints_void_anything_else`, `bedrock::launch::tests::the_brokers_authority_comes_first_in_the_trust_store_and_garbage_is_refused` | Name constraints also admit names *below* a permitted host (none is issued) | net BN6 |
| B34 | **Tokens in the log** | Header values, bodies and query strings never logged; upstream error lines stripped of the query | `egress::services::tests::only_end_to_end_headers_are_forwarded_and_credentials_are_marked_sensitive`, `tests/bedrock_net.rs` (the game's token never in the log) | — | net BN7 |
| B35 | **NetherNet or LAN around the relay** | NetherNet signalling denied by name; NETHERNET join answers refused with the reason; broadcasts have no route | `tests/bedrock_net.rs` | Friends' worlds, NetherNet Realms and LAN games do not work in this version | net BN8 |
| B36 | **The confined launcher** | The relay starts after step 9; the confinement keeps TCP 443 (or the proxy's port), the cache writable (the session's data root is deleted from there), nothing executable; the sign-in bridge and the handler are pre-bound listeners | `tests/bedrock_net.rs::the_bedrock_relay_works_after_the_launcher_has_confined_itself`, `bedrock::launch::tests::the_confined_launcher_can_still_delete_the_sessions_data_root`, `tests/bedrock_launch.rs` (session 3) | UDP is not the confinement's to restrict: a compromised launcher can send UDP anywhere (as with voice chat) | net BN9 |
| B37 | **A Bedrock session running when you scram** | Marked as its own kind (`bedrock:<instance>`), stopped through a pidfd like a Java launch; then, after bastion-mc's own credentials and the last stop pass, what Bedrock sessions left is deleted for a bounded time and confirmed; an unstoppable or unmarked session is a `COULD NOT` line saying to end it and to sign out everywhere; worlds and versions kept, and said | `running::tests::a_bedrock_session_is_marked_as_a_launch_of_its_own_kind_and_stopped_like_one`, `tests/scram.rs` (4 Bedrock tests) | Revokes nothing at Microsoft. A launcher older than this takes a `bedrock:` marker for a stale one. `mcpelauncher-ui`'s data is not touched | risk B10 |
| B38 | **Noticing it afterwards** | After a session in which the game was signed in through the bridge, the launch says where a new cape shows and what the canary does; the canary never alarms on a gained cape, names it and pins it; every other change is a difference | `bedrock::policy::tests::after_a_signed_in_session_the_user_learns_where_the_cape_shows_and_what_the_canary_does`, `launch::canary_tests::a_gained_cape_is_named_pinned_and_never_stops_a_launch`, `tests/bedrock_launch.rs` (the lines only after the signed-in sessions) | The canary sees the Java profile only, and only at the next Java sign-in: nothing watches Minecoins, Realms, the Bedrock character or Xbox friends | risk B11 |
| B39 | **Account enforcement, and the Google credential** | Not the launcher's to defend; the guide advises a Google account of its own and signing out of `mcpelauncher-ui` after downloading | — | Whether Google or Microsoft act against accounts used with unofficial clients is unknown. `mcpelauncher-ui` keeps a Google sign-in on disk, outside bastion-mc and scram | risk B12 |

**Experiments for §7** (besides the risk package's E33 and E34):

| # | What | Result |
|---|---|---|
| E35 | A Bedrock session end to end with a stand-in client in the real sandbox (`tests/bedrock_launch.rs`, three sessions, 2026-11-14) | The client's arguments, the version opened from the store and held for the session (`bedrock remove` refused meanwhile), the data split (world kept; tokens, a planted library and a crashed session's directory gone), the stub over the installed webview, no home, keyring or namespaces, HTTPS through the relay with the system's libcurl, the sign-in from the stub to the bridge and back (pasted; and handed back by `bastion-mc bedrock hand-back` with the launcher confined), the telemetry switch, the launcher confined |
| E36 | The CI's test steps with bubblewrap, ffmpeg, xdg-dbus-proxy, Firefox and `systemd-run` hidden and no `XDG_RUNTIME_DIR` (`target/int-logs/ci-tests-hidden.sh`) | Only tests that check root ownership failed — an artefact of the unprivileged user namespace used to hide the programs; none for a missing program (section 2, decision 9) |
| E37 | The sign-in handler's desktop entry in isolated XDG directories (`update-desktop-database`, `xdg-mime`, `gio mime`, `gio open`) | Registered as the scheme's default; `gio open ms-xal-…://auth?…` ran `bastion-mc bedrock hand-back`, which said that no session was running |

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

* **Fold in**: the threat-model table above (B1–B39, a new section of `docs/THREAT_MODEL.md`) and
  experiments E35–E37; the risk package's asset row, adversary paragraph, trust-boundary sentence,
  "not defended" items 17–19, the canary deviation and E33/E34 (`bedrock-risk.md` §2); its README
  text (§1.1: intro paragraph, status-table row, contents line, the Bedrock section, the emergency
  paragraph, the tests counts) and its `docs/README.md` index line (§1.2); its runbook text (§2.8).
* **Correct while folding** (the risk package wrote before the merge): the README section's sign-in
  sentence must say that the browser hands the address back where bastion-mc's handler is
  registered, and that pasting is the fallback; `totp` acts as `code` for Bedrock; Realms joins are
  approved in the front end's Realms panel or on the terminal, never in a phone; what is kept is
  `minecraftWorlds/`, `minecraftpe/` and the client's settings file — not all of
  `games/com.mojang`; `--demo` changes nothing for Bedrock.
* **README, "Tests"**: the counts of this tree (section 6); the new suites (`bedrock_launch`,
  `bedrock_sandbox`, `bedrock_cli`, `bedrock_store`, `bedrock_net`, `signin_stub`); the risk
  package's observation that the suite assumes a short `TMPDIR` on a file system without fs-verity
  (with a long btrfs `TMPDIR`, 23 tests outside Bedrock failed for it).
* **README, packaging**: the PKGBUILD now installs `docs/BEDROCK.md` and the sign-in handler's desktop
  entry (and names desktop-file-utils as optional for its hook).
* **TODO.md**: round 5 is integrated and committed in this clone; what only the user can do is the
  list in section 8 (the real client once in log mode, a real version's size, a real sign-in, a
  real server/Realm/event, a cape reaching Java).
* **The website** (`site/`): unchanged; `python3 -I site/build.py --check` passes on this tree. The
  pages mention no Bedrock yet; when they do, they must say what the guide says (above all the
  residual of B15: the game holds the Xbox identity for the session).
* **Pushing**: this clone's `integration` branch has every commit; the user pushes (the session's
  token cannot).
