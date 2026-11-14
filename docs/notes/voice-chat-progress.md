# Voice chat (work package a3-voice): hand-over

Kept current and committed with every step. If the job is interrupted again, continue from
here; `docs/notes/voice-chat.md` is the deliverable (user documentation, threat-model text,
what is open), this file is the state of the work.

## 0. History (who worked when; newest last)

* **First engineer**, 2026-11-13 about 19:45–20:40: built the package. Commits `f7c87c7` …
  `9826343`, then the integrator's `05c197e` "WIP checkpoint" (holds only
  `docs/notes/voice-chat.md`), then `cd9da3a` "sync:" which must stay as it is.
* **Second engineer**, 20:45–21:31: wrote this file, measured the stability of the test
  suites (section 4), began one more test. Commits `41273db`, `962c7c0`, then the
  integrator's `ae996c2` "WIP checkpoint 2".
* **Third engineer** (this hand), from 21:33: took over at `ae996c2`. Sections 1–4 below are
  the second engineer's and were left as written, except the plan (section 3), which is kept
  current. What I found and did is section 5.

* **Fourth engineer**, from about 22:15 (after the team was moved to another model at 22:04):
  took over at `8f00f12` "WIP checkpoint 3", which holds one new file only,
  `docs/notes/broker-test-race.patch` (no code changed since the third engineer's final
  verification). What I found and did is section 6.

**Read this before trusting `ae996c2`:** that checkpoint caught the second engineer in the
middle of a *mutation check*. Its `src/egress/udp.rs` has the `IP_PKTINFO` control message
removed from `GameSocket::send` on purpose (to see whether the new test notices an answer
that leaves from the wrong address); the unmutated file had been saved as
`target/scratch/t2/udp.rs.keep`, identical to `962c7c0:src/egress/udp.rs`. Left like that the
relay would answer from 127.0.0.1 and four tests fail. Restored in the commit after it.

## 1. The brief against what exists (first reading, 2026-11-13 20:50)

Checked by reading the code and by running it, not taken from the previous notes.

| Brief | State when taken over | Where |
|---|---|---|
| Rule syntax `voice = N` / `udp = [..]`, global and per instance; no rule, no UDP | done, tests pass | `src/config.rs` (`ServerEntry`, `ServerTable`, `udp_ports`), 3 unit tests |
| Helper: wildcard UDP socket per port, explicit and tested socket order | done, tests pass | `src/sandbox/netns.rs` (`NetnsSpec::sockets`, `SocketRole`, `check_socket`), `sandbox::spawn` checks each descriptor |
| `IP_PKTINFO` on receive, same source address on the way back | done, tests pass | `src/egress/udp.rs` (`GameSocket`) |
| One bounded flow table; rule must name the port; tied to an open TCP game connection and to its address; `is_public`/`allow_private` | done, tests pass | `src/egress/udp.rs` (`Table`), `relay_game` registers `udp::anchor` |
| Bounds: flows, idle, size, rate | done: 8 flows, 30 s, 1400 bytes, 100/s up and 500/s down per flow (deliberately not 50/50, reason in the notes) | `Limits::default()` |
| Log first packet of a flow and each kind of refusal, never per packet; statistics | done | `udp.rs` (`note`, `refuse`, `done`), `Stats.udp_*`, summary line in `src/launch.rs` |
| Landlock UDP list gains exactly those ports | done, test passes on ABI 10 | `src/launch.rs` (`landlock.udp_send`), `Policy::sandbox_udp_ports` |
| seccomp: say what a Java `DatagramSocket` needs | done: nothing added; a real JVM does the round trip in the sandbox | `tests/voice.rs` (JVM test), notes 2.7 |
| Launcher's own confinement and UDP | done: notes 2.6 and a test | `tests/voice.rs::the_relay_can_still_open_flows_after_the_launcher_has_confined_itself` |
| B-M3: pass-through connections through SOCKS5, name resolved by the proxy; exact messages; test with a stand-in proxy | done, test passes | `src/net/http.rs` (`socks5_dial`), `src/egress/mod.rs` (`relay_service`, `describe_proxy`, `outbound_ports`), `tests/egress.rs` |
| Stand-in tests: round trip, every refusal, expiry, Landlock | done, 12 `#[test]` in `tests/voice.rs` (one of them is the re-executed half of another) | |
| Harness in `src/fuzzing.rs`, seeds, robustness test | done (`udp`) | `egress::udp::fuzz_table`, `tests/robustness.rs`, `fuzz/fuzz_targets/udp.rs` |
| `examples/config.toml` | done (comment only: the default `servers = []` is unchanged) | |
| Real mod in the sandbox | reported as done by the first engineer (E17); **to be re-checked by me** | `target/scratch/launch-fabvoice.out`, instance `fabvoice` in `target/e2e` |
| Notes file | present (WIP checkpoint); **to be checked line by line against the code** | `docs/notes/voice-chat.md` |

State of the tree when taken over: `cargo fmt --check` clean, `cargo clippy --workspace
--all-targets` no warnings, `cargo test --workspace --no-fail-fast` green in one run
(launcher 291 tests = 204 unit + 87 integration; front end 53; protocol 6) and with one
failure in another run — see section 4.

## 2. Found so far while checking (to fix or to report)

* The notes' test counts are wrong: the branch adds **34** tests to the launcher crate
  (257 → 291), not 50; "13 sandbox tests, 22 unit tests" does not match either (11 in
  `tests/voice.rs` plus the helper half, 1 in `tests/egress.rs`, 20 unit tests, 1 robustness).
* The notes say `net = []` for the nix feature; it is `net = ["socket"]` (`socket` was
  already enabled; `Cargo.lock` is unchanged — checked with `git diff`).
* The round trip with a literal address is tested with loopback literals only (127.0.0.1,
  127.0.0.2). A non-loopback literal takes another path in the helper (the address is added
  to `lo`) and the way back names that address in `IP_PKTINFO`; candidate for one more test.

## 3. Plan (in this order)

1. [x] Read history, WIP diff, notes, tests; build; run everything once. (2nd)
2. [x] Stability of `tests/voice.rs`: 25 of 25 runs of the file green, one after the other. Four copies of
       the test binary at once: 5 of 56 runs failed, none of them through an assertion about the
       relay — see "sandboxes killed at start" in section 4. (2nd)
2a. [x] Undo the mutation left in `ae996c2`; finish the test for a literal address that is no
       loopback address, and mutation-check it. (3rd — section 5.1)
3. [x] Opt-in suites (3rd, at `a0f02c0`): `BASTION_NET_TESTS=1 cargo test --test egress` — 6 of 6,
       including the two live tests and the no-proxy control inside the proxy test;
       `BASTION_FUZZ_ITERATIONS=300000 cargo test --release --test robustness
       datagram_relay_decisions` — ok (build 51 s, run 1.1 s); `cargo check --manifest-path
       fuzz/Cargo.toml --target-dir target/fuzz-check` — ok on the stable toolchain (the
       targets were still never *run*: no nightly here).
4. [x] Critical review of `udp.rs`, `egress/mod.rs`, `netns.rs`, `config.rs` changes.
       (3rd — sections 5.2 and 5.5: three things fixed, the rest found to hold)
5. [x] Real mod: demo launch of `fabvoice` on display :77 re-run at `7173b87` and its log read;
       the relay exercised inside that launch (3rd — section 5.4).
6. [x] Notes against code (3rd): the three errors of section 2 corrected, plus a claim about
       the mod's packet size that its class files contradict (section 5.7); everything found
       and verified since is in `voice-chat.md` (sections 2.5 E16–E19, 3, 4, 6, 7, 8).
7. [x] Final: fmt, clippy, full suite, opt-in suites — section 5.8. What is left is the
       structured result; if this file is the last thing a successor sees, the package is
       finished and only the hand-in has to be written from sections 5 and 5.8.

## 4. Things learned about this machine and this code base

* **A test that existed before fails now and then, unrelated to this package**:
  `sandbox::resource_limits_kill_a_memory_hog_inside_the_scope` expects exit code 137 and
  sometimes sees a death by signal (`None`). Measured here: 0 of 15 runs of the test alone,
  5 of 40 runs of the three `resource_limits_*` tests together, 4 of 20 runs of the whole
  file. Cause, shown by an experiment that uses none of this project's code
  (`systemd-run --user --scope -p MemoryMax=128M -p MemorySwapMax=0 -- bwrap … python3`
  allocating 600 MiB, started from Python so that the wait status is visible): the user
  manager's `DefaultOOMPolicy=stop` applies to the scope, so after the kernel has killed
  the payload systemd stops the unit and sends SIGTERM to what is left of it — which races
  with bubblewrap's own exit(137). Result of the experiment: 137 in 11 of 12 runs one at a
  time, and SIGTERM (−15) in 9 of 24 runs six at a time. Not fixed here (`tests/sandbox.rs`
  and `src/sandbox/cgroup.rs` belong to others); two ways to fix it are in the notes.
* Seen once in 20 runs of the whole `tests/sandbox.rs`, not reproduced in 40 runs of the
  three tests: `resource_limits_put_the_sandbox_in_a_scope_and_cap_its_tasks` read an empty
  first line instead of `ready=1`; the user journal shows that the scope
  `bastion-mc-test-tasks-<pid>.scope` was never started in that run. Cause not found.
* Test binaries live under `target/debug/build/bastion-mc/<hash>/out/` in this clone.
* The private display `:77` is up (`xprop -display :77 -root`).
* The first engineer's scratch files are in `target/scratch/` (not committed): the mod jar
  (`svc-jar/`), Modrinth's version list (`svc-meta/`), the launch log
  (`launch-fabvoice.out`), three full test logs. Mine go to `target/scratch/t2/`.
* **Sandboxes killed at start when many start at once — not this package, not this project's
  code.** Found by running four copies of the voice test binary at the same time: now and
  then a sandbox's payload ends with exit code 137 and no output, once the namespace helper
  was gone before handing over its sockets, and once two whole test binaries were killed
  (SIGKILL) right after starting. The same happens to the pre-existing sandbox tests, and
  **with plain bubblewrap and no code of ours**:
  `seq 1920 | xargs -P 48 -n 1 ./one-bwrap.sh` where the script runs `bwrap --unshare-user
  --unshare-ipc --unshare-pid --unshare-uts --unshare-cgroup --unshare-net --new-session
  --clearenv --cap-drop ALL --die-with-parent --ro-bind /usr /usr … -- /usr/bin/echo ok`:
  2 of 1920 ended with 137 and no output; in a repeat the wrapper script itself was killed by
  signal 9. From Python threads: 0 of 480 one at a time, 0 of 480 six at a time, 2 of 480
  with 24, 4 of 480 with 48. Without `--die-with-parent` a run hung: the outer bwrap was a
  zombie with wait status 9 (killed by SIGKILL) and its inner process sat in
  `read(eventfd)` waiting for it. The kernel log is silent at those moments (no OOM kill,
  no seccomp record), bwrap imports no `kill`, the shell has no seccomp filter and is in no
  namespace of its own. **Who sends the signal was not found** (no strace/perf/bpftrace
  here, and none can be installed). Consequences: (a) a real launch starts one or two
  sandboxes and is far below the level where this shows; (b) a sandbox-based test of any
  suite (sandbox, egress, broker, jvm, voice) can fail with "137 and empty output" when
  several suites run at the same moment — five engineers on one machine do that. Re-run the
  test; it is not a finding about the code under test.
* `tests/sandbox.rs` cannot run next to a second copy of itself (two engineers at the same
  moment): `kernel_keyring_is_unreachable_from_the_sandbox` adds its key under the fixed
  description `bastion-test-key` to the session keyring, so two runs replace and unlink
  each other's key. (Seen when I ran four copies on purpose; no test key was left behind —
  checked with `keyctl show @s`.) Not mine; reported.
* Do not leave experiments with bubblewrap running without `--die-with-parent` and a
  timeout: an inner process whose outer one was killed blocks forever and keeps the pipes.

## 5. Third engineer: what I found and did

### 5.1 The mutation in the checkpoint, and the test it was for (done)

* `ae996c2` = `962c7c0` + (a) the mutation described in section 0 and (b) a new test in
  `tests/voice.rs`, `a_literal_address_that_is_no_loopback_address_is_answered_from_that_address`
  (the candidate named in section 2, third bullet).
* Ran `tests/voice.rs` on the tree exactly as found (mutated): 4 tests failed — the round
  trip, the refusals test (its `with_connection` line), the IPv6 test and the JVM test, each
  because the answer came from 127.0.0.1 — and **the new test passed**. So the new test did
  not see what it was written to see. Reason: its two client sockets were unbound, the
  kernel gave them the server's own address as source (the destination is local), and an
  answer *to* that address leaves *from* it with or without `IP_PKTINFO`.
* Fixed the test: a third client socket bound to 127.0.0.1 sends to the literal address; its
  answer must still come from the literal address. Mutation check repeated: with the
  mutation that assertion fails (`…@127.0.0.1:port` instead of `…@<address>:port`), without
  it the test passes. `src/egress/udp.rs` is byte for byte `962c7c0`'s again.
* The test binds its stand-in servers to this machine's address on the local network (no
  other way to have a server at a literal non-loopback address without privileges). Added
  `from_this_machine` to both stand-ins: they answer only peers on this machine. Nothing
  leaves the machine in that test (own addresses are reached over `lo`). The test is skipped
  where the machine has no such address.

### 5.2 Review findings (step 4 of the plan)

* **Fixed — a refused send was logged as a local fault.** A connected UDP socket reports
  the ICMP "port unreachable" for one datagram on the flow's *next* socket call. If that
  call is the receive in `pump`, the log says "the server's address reports that datagrams
  to this port do not arrive" (right). If it is the next send in `from_game` — which
  happens whenever the game sends two datagrams back to back — the log said "the operating
  system could not send it on (no socket, or a full buffer)" (wrong: nothing is the matter
  with this machine). Shown first: the extended test
  `voice::a_voice_port_on_which_nothing_listens_is_reported_once` (20 datagrams without a
  pause) got that line in 3 of 3 runs before the change. Now `unsent_because` maps
  `ECONNREFUSED` on that send to `Refusal::Unreachable`; the datagram is still counted as
  refused (it did not leave). 5 of 5 runs green after; unit test
  `egress::udp::tests::a_refused_send_is_the_servers_answer_and_any_other_failure_is_this_machines`.
* **Checked and found to hold** (read against the code, and where it says so, run):
  the decision table and its lock discipline (`dest_of` and `open_upstream` run under the
  table lock; nothing takes the locks in the other order); flows cannot outlive their game
  connection (`Anchor::drop` removes them before the `watch` sender goes); the socket-order
  contract (one function, checked per descriptor in `sandbox::spawn`); configuration bounds
  (ports ≥ 1024, ≤ 4 per rule, ≤ 16 in all, unknown keys refused); B-M3 (`relay_service`
  through `socks5_dial`, 443 dropped from the confined launcher's ports when proxied, the
  proxy's own port kept — `launch.rs` adds it); the mod's network code as the notes describe
  it (`javap` on `ClientVoicechatSocketImpl`, `InitializationData`, `ClientManager`,
  `ClientVoicechatConnection`: `new DatagramSocket()`, host = `voice_host` if set, else the
  game connection's remote address, port as announced); the jar's SHA-512 against a fresh
  query of Modrinth's API (same value as the first engineer's saved answer).
* **Not changed, to be said in the notes**: `src/confine.rs` is another engineer's this
  round (findings A-F1/A-F2). Its header lists what the confined launcher still handles
  ("DNS questions, TLS hellos, HTTP requests, log lines"); with this package that list has
  one more entry — datagrams, relayed unparsed. For the integrator to add.

### 5.3 State of the full suite at `dd0191f` (one run)

`cargo test --workspace --no-fail-fast`: 350 passed, 1 failed —
`sandbox::resource_limits_kill_a_memory_hog_inside_the_scope` (`left: None, right:
Some(137)`), the pre-existing flake of section 4, first bullet. `tests/voice.rs`: 13 of 13.

### 5.4 The real launch, and what it showed (step 5 of the plan)

Run at `7173b87` (debug build), `launch fabvoice --demo`, display :77, sound off. Scratch
(not committed): `target/scratch/t3/real/` — `launch-fabvoice.out`, `inside.out`,
`standin.py`, `inside.py`.

* The mod: "Loading 51 mods" with `voicechat 2.6.27+26.3`; Opus, RNNoise, Speex and LAME
  each "Successfully initialized"; 13 log events from the mod, all INFO. Warnings and errors
  in the whole log: Realms (2, no account), the narrator (`libflite` is not in the sandbox;
  vanilla), `/player/attributes` 401 (no account), the sound system (audio is off), oshi
  (2, vanilla hardware probing). Nothing refused on the mod's behalf, no tripwire.
* **The relay inside a real launch** (new; the notes used to list this as not verified).
  `instance.toml` of `fabvoice` now points its rule at two stand-ins on the host's loopback
  that `standin.py` starts (TCP echo "game server", UDP echo "voice server"; ports chosen by
  the kernel). While the game ran, a Python process was put into the *game's network
  namespace* — `nsenter -t <outer bwrap> -U -n --preserve-credentials` (same user, so
  allowed: THREAT_MODEL P7) — where it did what the mod would do. It is not inside the
  game's sandbox, so this says nothing about Landlock or seccomp (the tests do); it shows
  the launcher's side of a real launch, after `confine::apply`:
  * sockets in the namespace: UDP `0.0.0.0:<voice>` and `127.0.0.1:53`, TCP `443`, `80` and
    the game port; interfaces: `lo`;
  * a datagram before any game connection: no answer, `DENY udp … (no game connection …)`;
  * TCP to the game port: relayed (`ALLOW … game server →`), then datagrams answered from
    `127.0.0.1:<voice>`: `ALLOW udp … (flow 1 …; follows the game connection …)`;
  * at the end `done udp … ↑5 ↓5 datagrams, 0 refused (the game sent nothing for 30 s)` and
    `egress summary, UDP: 10 datagram(s) relayed in 1 flow(s), 1 refused`;
  * SIGINT to the launcher: "game stopped on request", exit 130, nothing left running.
* **It also showed a flaw, now fixed (section 5.5):** the last datagram, sent *after* the
  stand-in client had closed its game connection, was still relayed. My stand-in server
  never closed its end, and the relay counted the connection as open until both ends had.

### 5.5 Fixed: "open" now means open in both directions

* Before: the anchor of a game connection lived until `copy_bidirectional` returned, i.e.
  until **both** sides had closed. So (a) a server that does not hang up (frozen, or its
  close lost on the way) kept the datagram path open after the game had closed the
  connection, and (b) code in the game could keep the path open after the *server* had hung
  up, for as long as it liked, by not closing its socket. No new destination either way, but
  not what "while a game connection is open" promises.
* Test first: `voice::datagrams_stop_as_soon_as_either_side_hangs_up_the_game_connection`
  (a stand-in server that keeps its end; and one that hangs up first while the client keeps
  its end). On the old code both datagrams were relayed.
* Fix (`src/egress/mod.rs`): `splice_anchored` — the same two copies and shutdowns that
  `tokio::io::copy_bidirectional` does (same `tokio::io::copy` machinery, first error ends
  both), plus: the anchor is dropped when the *first* direction ends. Used only for game
  connections that datagrams can follow (`Policy::followed_by_datagrams`: a rule for that
  host and TCP port names UDP ports); every other game connection goes through `splice`
  exactly as before, and no longer registers an anchor at all (it could never be matched).
* `voice::a_game_connection_that_datagrams_follow_carries_its_bytes_to_the_end`: 512 KiB each
  way through such a connection, the game half-closing first; echoed byte for byte,
  `done … ↑524288 ↓524288 bytes`.
* `tests/voice.rs`: 15 of 15. `cargo test --lib egress`: 41 of 41. `tests/egress.rs` 6 of 6.

### 5.6 Things learned (additions to section 4)

* **An orphan of this clone is still running and I did not kill it**: PID 742946,
  `/usr/bin/bwrap --args 103 -- /bastion/init __sandbox-init 135`, started 20:53:59 (the
  second engineer's parallel runs), parent `init`, no children, sleeping. Its working
  directory and two of its descriptors point into this clone
  (`readlink /proc/742946/cwd`, `ls -l /proc/742946/fd`), so it is ours — the "inner process
  whose outer one was killed" of section 4. Its command line does not contain the clone
  path, and the machine rule is literal about that, so it is reported instead of killed.
* `pkill -f <pattern>` also matches the shell that runs it when the pattern appears in that
  shell's own command line (it killed my command once, exit 144). Anchor the pattern with
  `^`, or write one character as a class (`[s]tandin.py`).
* `broker::a_sandboxed_jvm_holding_only_the_placeholder` (the first engineer's flake, notes
  section 7) failed once for me in a run of `--test egress --test broker --test voice`
  (`tests/broker.rs:649`, `join_warm`: 403 instead of 204), then passed 17 of 17 alone and
  14 of 14 as a whole file. That test has no UDP rule, so it runs the unchanged `splice`.
* The shell's own environment has `DISPLAY=:0` and the user's `XAUTHORITY`. Every command
  that could open a window must set `DISPLAY=:77 XAUTHORITY=/dev/null` itself.

### 5.7 Notes against the code (step 6)

* Corrected in `voice-chat.md`: the test counts (38 added, not 50: 21 unit, 17
  integration); the nix feature (`net = ["socket"]`, not `[]`); who mutation-checked what;
  "the mod caps its packets at 1024" — Simple Voice Chat 2.6.27's `ServerConfig` has
  `mtu_size` default 1275 (512 to 2048), which is the Opus payload limit its encoder gets
  (`OpusManager.createEncoder` → `setMaxPayloadSize(getMtuSize())`); with the framing read
  from `NetworkMessage`, `MicPacket` and `PlayerSoundPacket` the largest packet at the
  default is about 1330 bytes up and 1365 down, under the relay's 1400. The comment at
  `Limits::default()` said 1024 too and is corrected. Also checked there: `keep_alive`
  default 1000 ms, default port 24454 (−1 = the game port), 48 kHz and 960-sample frames
  (50 a second), `new DatagramSocket()`, the host from the game connection's remote address.
* `grep` in this shell is a function that skips binary files: use `command grep -a` on
  class files (it cost me two wrong "no match" answers).
* A user-documentation claim that no test backed was reworded: "when the rule names the
  wrong port" does not produce a `DENY` line (the datagram never leaves the sandbox).
* An untested claim about Tor's defaults was taken out of the proposed threat-model row.

### 5.8 Final verification (step 7) — at the last commit of the branch

* `cargo fmt --all -- --check`: clean. `cargo clippy --workspace --all-targets`: no warnings
  (run after touching every crate root so that nothing came from a cache).
* `cargo test --workspace --no-fail-fast`: **354 passed, 0 failed** (launcher 295 = 205 unit
  + 90 integration; front end 53; protocol 6). Four full runs in my session: the first had
  the pre-existing memory-hog flake (section 4), the second caught a race in a test I had
  just written (fixed: `a_game_connection_that_datagrams_follow_…` now waits for the echo
  like every other test), the third and the fourth were green.
* `tests/voice.rs` alone: 20 of 20 runs green after the last change to it.
* `BASTION_NET_TESTS=1 cargo test --test egress`: 6 of 6 (run again after the last change
  to `src/egress/mod.rs`).
* Release run of the harness with 300 000 scripts: at `a0f02c0`; since then `udp.rs` changed
  in comments only (`git diff a0f02c0 HEAD -- src/egress/udp.rs`, no non-comment line).
* Not run, because nothing in them touches this package: `BASTION_X11_TESTS` (its one test
  uses the open network and no network, never the filtered one), the nested-compositor and
  Wayland suites, `tests/install.rs` with the network switch.

### 5.9 A mistake of mine, for the record

At about 21:58 I ran `./target/debug/bastion-mc doctor` to read the Landlock line and
**forgot the four XDG variables and the display variables**. What that run did, established
from the code (`cli.rs:314`, `doctor.rs`, `sandbox/display.rs`, `sandbox/pulse.rs`) and
from `stat` on the directories:

* `paths.ensure()` ran on the user's real `~/.config`, `~/.local/share`, `~/.cache`,
  `~/.local/state` `…/bastion-mc`. All four existed already (created 04:51 that day) and
  none was created, written to or touched: their modification times are 04:51, 13:30, 19:50
  and 04:51, all hours before this session. No file was written anywhere.
* The display plan saw `DISPLAY=:0` in the shell's environment. It did **not** connect to
  the X server and started nothing (planning only describes); it did `stat` the socket and
  read the user's `~/.Xauthority` into the memory of that short-lived process.
* The audio plan, with the built-in default `audio = "pulse"` (the user's real
  configuration directory is empty), ran the module-loading probe against the user's real
  sound server: connect, authenticate, ask for a module that does not exist, read the
  answer. No stream was opened: nothing was played or recorded. It is what the user's own
  `bastion-mc doctor` does.
* It also started `java -version` in a sandbox without network or display (the self-test).

**Rule for whoever continues:** every start of the launcher binary, also a read-only
command, gets the four `XDG_*_HOME` variables *and* `DISPLAY=:77 XAUTHORITY=/dev/null`. The
shell's environment has the user's real display, and the launcher's default configuration
has sound on.


## 6. Fourth engineer: what I found and did

### 6.1 Taking over (22:15)

* `8f00f12` "WIP checkpoint 3" adds only `docs/notes/broker-test-race.patch`: a draft fix,
  not applied, for the pre-existing flake `broker::a_sandboxed_jvm_holding_only_the_placeholder`
  (section 5.6, third bullet). It touches `tests/broker.rs` and `tests/java/BrokerProbe.java`,
  not this package's code. Decision on it: section 6.2 (in progress).
* Environment at take-over: display `:77` up; no `/run/user/1000/bmc-a3*` left; the orphan
  of section 5.6 (PID 742946) still there, left alone as instructed.

### 6.2 WIP checkpoint 3: the broker-test patch — finished (kept as a patch, not applied)

* Decision: **finish** it, and keep it as a patch file for the integrator, as the notes
  (section 7, item 4) already said: `tests/broker.rs` and `tests/java/BrokerProbe.java`
  belong to the broker work of this round ("fixes" package, review A), and this package's
  code does not cause the race — `gate.opened` is called at the same point as at the
  baseline (after connect, before splicing; the test's rule is a literal address without
  UDP ports, so it takes the unchanged `HostSpec::Ip` + `splice` path; compared with
  `git show 51bc003:src/egress/mod.rs`).
* Finished: the probe's wait for the echo is bounded (`setSoTimeout(15000)`), so a relay
  that never splices ends the probe with an exception and the test with its output,
  instead of a hang until the test's 120 s timeout; the patch file got a header (ignored
  by `git apply`) saying what it fixes, why, how it was verified and how to apply it.
* Verified (scratch script `target/scratch/t4/broker-loop.sh`, runs one after the other):
  a 1 s delay injected in `relay_game` just before `gate.opened` (to widen the race; removed
  afterwards, `git diff src/` empty) → without the patch 4 of 4 runs failed with
  `join_warm` = 403 (`tests/broker.rs:649`), with the patch 4 of 4 passed. Without the
  delay: with the patch 40 of 40 passed; without it 39 of 40 (one `join_warm` = 403 — the
  flake, seen today). `git apply --check` of the final file against the branch: ok.
* Notes updated: `voice-chat.md` section 7, item (4).

### 6.3 Full suite at take-over (code unchanged since the third engineer's final run)

`cargo fmt --all -- --check` clean. `cargo test --workspace --no-fail-fast`: everything green
except `sandbox::resource_limits_kill_a_memory_hog_inside_the_scope` (`left: None, right:
Some(137)`) — the pre-existing flake of section 4, first bullet. `tests/voice.rs` 15 of 15,
`tests/egress.rs` 6 of 6, `tests/broker.rs` 10 of 10. Log: `target/scratch/t4/full-1.log`.

### 6.4 My review pass (done; nothing to change)

Read against the brief: `src/egress/udp.rs` whole (table, buckets, truncation handling —
the receive buffer is one byte larger than the limit, so a truncated datagram always counts
as oversize —, lock scope in `from_game`/`pump`, pump lifetime when an anchor closes between
the decision and the spawn, error branches that cannot spin), the `src/egress/mod.rs` diff
(policy functions, B-M3 path, `outbound_ports` with a proxy, `splice_anchored` against
`copy_bidirectional`), `src/sandbox/netns.rs` (order, `check_socket`), `src/config.rs`
(hand-written `Deserialize`, bounds), `src/launch.rs` (the proxy note's mode is computed
exactly as the launch's own mode, lines 408 and 480). Found to hold; no change made.
Also checked: the notes' test counts against `git grep` of the test attributes at
`51bc003` and `HEAD` (21 unit + 17 integration added, 257 → 295; matches); the claim
"kernel 7.2 adds Landlock UDP" matches README line 56. The 300 000-script release run of
the harness (third engineer, at `a0f02c0`) still covers the current code: since then
`udp.rs` changed in comments only, and `mod.rs` by extracting `followed_by_datagrams`
(same expression) and by `splice_anchored`, which the harness does not drive.

Notes updated: header (four engineers), section 7 (the patch file as an integrator item).

### 6.5 Final verification (done) — on the final code

* `cargo fmt --all`: no change. `cargo clippy --workspace --all-targets`, run after
  touching the crate roots of all three crates: exit 0, no warning, no error
  (`target/scratch/t4/clippy.log`).
* `cargo test --workspace --no-fail-fast`: **354 passed, 0 failed**, exit 0
  (`target/scratch/t4/full-2.log`). The earlier run of the day (section 6.3) had only the
  memory-hog flake.
* `BASTION_NET_TESTS=1 cargo test --test egress`: 6 of 6, the two live tests and the B-M3
  proxy test included (`target/scratch/t4/egress-net.log`).
* Not repeated, because nothing they cover changed since they were recorded as done: the
  release harness run (section 6.4), the stability runs of `tests/voice.rs` (5.8), the real
  launches (5.4; E17/E19 in the notes).

**State: the package is finished.** What is left is the structured result; if this file is
the last thing a successor sees, only the hand-in has to be written (sections 5, 5.8 and 6).
