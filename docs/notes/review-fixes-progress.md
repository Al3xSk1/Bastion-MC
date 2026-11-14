# Review fixes — hand-over file (work package "a5-fixes")

Kept up to date and committed with every step, so that the job can be continued from this
file alone. The record of each finding (verification, change, test) is
`docs/notes/review-fixes.md`; this file is about *where the work stands*.

## History of the job

1. First engineer: three commits (`b4de0b1`, `123d860`, `de848e8`), then cut off.
2. Integrator: `0807147` "WIP checkpoint" (uncommitted `src/launch.rs` of the first
   engineer; **did not compile**: 8 errors), `853bee4` "sync:" (leave as it is).
3. Second engineer (this file's first author): everything after `853bee4` up to `9e8e143`,
   then cut off at 21:31 (the orchestrating session ended).
4. Integrator: `8b7e743` "WIP checkpoint 2" — the second engineer's uncommitted state:
   README.md and docs/THREAT_MODEL.md largely rewritten for the findings, one line each in
   docs/TOKEN_INJECTION.md and examples/config.toml, and in `src/launch.rs` the case "the
   sandbox ended before its limits could be read back".
5. Third engineer: took over at `8b7e743` (2026-11-13, 21:34). Found at take-over: the
   tree **builds**, and `cargo test --workspace --no-fail-fast` is green (369 tests; log in
   `target/a5-logs/test-4.log`, not committed). The second engineer's last runs, found as
   logs under `target/a5-logs/` and not recorded in this file until now: the nested suite
   with the clipboard test (`nested-1.log`, 4 passed), a demo launch through a security
   context on headless sway (`launch-wl.out`) and a `--burner` demo launch
   (`launch-burner.out`), both ending with exit status 130 after SIGINT. Missing at
   take-over: `docs/notes/review-fixes.md` (did not exist), TODO.md "Independent review",
   docs/MC_VAULT_ACCOUNT.md, the review of the WIP documents against the code, and the
   final verification. See "State at the third take-over" below.
6. Fourth engineer: took over at `c7325c0` (2026-11-13, 22:06; the third engineer was
   stopped on purpose at 22:04 to move the team to another model). Found at take-over: the
   working tree clean, no "WIP checkpoint 3" commit, this file saying the package is
   complete. Plan: a critical review of the whole package against the findings (the third
   engineer's 30 minutes covered much ground), repairs where something does not hold, then
   the final verification once more on the final tree. See "Fourth engineer" below.

## State found at take-over (2026-11-13, read and checked, not assumed)

`cargo build` at `853bee4` failed with 8 errors, all in `src/launch.rs`, all references to
things the first engineer had not written yet:

* `instance::PERSIST_DIR`, `instance::adopt_persisted`, `instance::persisted_in` (A-F3)
* `canary_verdict` returning a note with `.warning` / `.text` (B-L1)
* `nested::Session::hold_output` (A-F6)
* the old unit test `burner_tests` still calling the removed `copy_tree` (B-L11)
* the tail of `launch()` (settings carried back, scratch removal) still in its old form:
  `persist::carry(&scratch.root, &inst.dir, …)` and `drop(scratch)` (A-F3, B-L4)

What was committed and what I found on reading it (critically; tests re-run later, see
"Verified" below):

| Finding | State at take-over |
|---|---|
| A-F1 seccomp filter on all threads | done: `src/confine/filter.rs`, `tests/confine.rs` |
| A-F2 partial enforcement visible | done in `confine.rs` (`Support`, `Demand`, `Applied`), doctor; launch side in the WIP |
| A-F3 write grant | WIP only (launch.rs half; `instance` half missing) |
| A-F4 Wayland reader bound | done: `sandbox/wayland.rs` (`DEADLINE`, `MAX_EVENTS`, `MAX_BYTES`, non-blocking connect) |
| A-F5 broker buffering | not started |
| A-F6 protection event / order | WIP only (launch.rs reordered; `hold_output` missing) |
| A-F7 cgroup read-back | done: `sandbox/cgroup.rs` (`verify`, `verify_started`, probe scope); launch side in the WIP |
| A-F8 secured socket listed | done: `wayland::Offered`, `display.rs` (`wayland_isolation`) |
| A-F9 JSON re-encoding | not started |
| A-F10 wording | not started |
| A-adjacent log budget | WIP only (`LOG_SHARES`, `Share`; no test) |
| B-M1, B-L2, B-L3, B-L5, B-L6 scram | done: `scram.rs`, `running.rs`, `tests/scram.rs`, markers in `cli.rs` |
| B-M4 age recipients | not started |
| B-L1 canary warn | WIP only (call site; `canary_verdict` itself and `cli.rs` untouched) |
| B-L4 scratch cleanup | `tree::remove` done; `Scratch::remove` in the WIP; reporting at exit missing |
| B-L7, B-L8, B-L9 packs | not started |
| B-L10 skins | not started |
| B-L11 copy_tree | `tree::copy` done; call site in the WIP |
| B-L12 identity_kind | not started |
| B-L13 clipboard shim | done: `native/noclip.c` status word, `nested::ClipboardCut`; launch side in the WIP |
| README / THREAT_MODEL / TODO | not started |
| `docs/notes/review-fixes.md` | did not exist |

## Done and verified since take-over

Each line: what, where, and how it was checked (commands always with
`DISPLAY=:77 XAUTHORITY=/dev/null`).

* **Build repaired** (`4a62271`): `instance::{PERSIST_DIR, adopt_persisted, persisted_in}`
  (A-F3), `nested::Session::hold_output` + `OutputHold` (A-F6), `launch::CanaryNote` and
  `canary::accept_change` (B-L1), tail of `launch()` (settings into `persist/`, scratch
  removal reported: B-L4). Unit tests: `instance::tests::settings_files_of_an_existing…`,
  `sandbox::nested::tests::held_output_is_not_read…`, `auth::canary::tests::an_own_change…`,
  `launch::canary_tests::*`.
* **First real demo launch** with the reordered start-up (vanilla-latest, `:77`, through
  gamescope, which is installed on this machine now): limits read back, launcher confined
  (`Seccomp: 2`, `Seccomp_filters: 3` in `/proc/<launcher>/status`), game ran, broker and
  relay worked, SIGINT → exit 130, `persist/options.txt` written back *after* confinement,
  scratch and marker gone. Output: `target/a5-logs/launch-1.out` (not committed).
* **Log shares** (A-adjacent, `ccf9213`): `launch::log_tests::a_source_that_fills_its_share…`.
* **scram** (`ccf9213`): a launch whose marker is gone is reported by the directory it holds
  locked (`Sweep::in_use`); `tests/scram.rs::a_launch_without_a_marker…`. All 8 scram tests
  and 7 `running::tests` green.
* **A-F5** (`aa2ae5c`): one budget (`broker::BUFFER_BUDGET`, 64 MiB) for all buffered
  bodies; `ReplyBody` hands replies over in 16 KiB frames and holds its share until
  dropped; encoded replies to credentialed requests withheld. Tests:
  `tests/broker.rs::replies_the_game_does_not_read_come_out_of_one_bounded_budget`,
  `…::an_encoded_reply_to_a_credentialed_request_is_withheld`,
  `egress::broker::tests::buffered_bodies_come_out_of_one_budget`.
* **A-F9** (`8823b55`): `persist::write_json` (printable ASCII, `\u` escapes); test
  `persist::tests::json_strings_and_keys_come_out_as_printable_ascii…`; harness asserts it.
* **B-M4, B-L12** (`0a987c4`): `paths::SYSTEM_AGE_RECIPIENTS`, `store::{system_recipients,
  anchor_at, TokenStore::open_anchored, AgeVault::anchored, IdentityKind, DEVICE_PLUGINS,
  Recipients, recipient_kinds}`; doctor's `age_store`. Tests:
  `auth::store::tests::{a_root_owned_recipients_file_overrides…, recipients_are_counted_by_kind,
  identity_files_are_classified…}`. `bastion-mc doctor` run by hand against a scratch
  configuration with an age vault (no `age` binary here: its line fails, as it must).
* **Two tests that existed before and failed here for reasons of their own** (both
  reported in `review-fixes.md`, "Not findings"):
  `tests/sandbox.rs::resource_limits_kill_a_memory_hog…` (a race between bubblewrap
  reporting 137 and systemd stopping the scope under its default `OOMPolicy=stop`; the test
  now accepts both ends) and `tests/broker.rs::a_sandboxed_jvm…` (the probe asked for a join
  before the relay had accepted its game connection; the probe now waits for the server's
  answer, as the real protocol does).

## Third engineer: done and verified (2026-11-13, from 21:34)

* **Read all of the two previous engineers' code against the findings** (every file named in
  `git log --stat` since `c1e3ea0`). Nothing had to be redone. Repaired on the way:
  three user-visible messages with a run of spaces in the middle (`instance.rs` ×2,
  `launch.rs`), `cargo fmt` over everything, three clippy warnings.
* **A-F8, launcher side** (`5a45193`): `sandbox::display::tests::a_security_context_that_
  withholds_nothing_is_reported_by_auto_and_refused_by_require` — the decision in
  `add_wayland` had no test (the stand-in compositor of the wayland tests is now shared).
* **`tests/confine.rs` failed here once the machine's process ids reached seven digits**:
  the pathname socket of the test lay under `CARGO_TARGET_TMPDIR/confine-<tag>-<pid>/`,
  which in this clone is 108 bytes with a 7-digit pid — one more than a socket address
  holds (`path must be shorter than SUN_LEN`, in `an_exploit_only_system_call…`). Not a
  defect of the launcher; the socket now lives in a short directory under the temporary
  directory. Run three times, 5/5 each.
* **What `/proc` is worth to a confined launcher** (new probes in `tests/confine.rs`, all
  four threads): another process's command line is readable, its environment is not
  (`EACCES`: Landlock refuses access to processes outside the domain); with the filter alone
  (emulated ABI 7) it is readable, as reported. Goes into THREAT_MODEL J18.
* **Experiments E17 re-run**: `target/a5-probe/zipexp` (the second engineer's files) with
  OpenJDK 27 — `hidden.zip`: Java lists 3 members where the end record declares 1;
  `dup.zip`: Java lists the same name twice. `FilePackResources$SharedZipFileAccess` of the
  26.3 client jar (byte-identical to the class in `target/e2e/.../26.3.jar`) constructs a
  `java.util.zip.ZipFile`.

* **The record** `docs/notes/review-fixes.md` (`5a8b022` and later): one entry per finding
  — verification, change, tests — and the final verification table.
* **A-F1 reproduced by experiment** (throw-away test, not committed): under a fully enforced
  Landlock ruleset that denies every file, `keyctl` search and read of a key in the
  process's own session keyring still succeed.
* **B-L3's keyring scenario exists on this kernel**: in `tests/scram.rs::a_token_that_only_
  another_login_session_holds…` the first session's key stays readable (the test takes the
  branch that demands the `COULD NOT` line).
* **B-L4, launcher side** (`670869c`): `launch::scratch_tests::a_game_root_that_could_not_be_
  deleted_is_named…`.
* **Documents** (`f0ccd7a`, `8835344`, `00ecdee`, `46fd3fa` and the TODO commit): the
  second engineer's README and THREAT_MODEL text was checked against the code and the runs;
  added: what the root-owned recipients file does *not* do (`store.rs` header, README,
  THREAT_MODEL §5.1, MC_VAULT_ACCOUNT with the commands), J18's residual (signals, what
  `/proc` yields), E16 as run, "not exercised", README status numbers, TODO "Independent
  review" and the open items that carry the word "review".
* **`fuzz/Cargo.lock`** (`bb06478`): needed the `flate2` edge; the fuzz crate compiles
  (`cargo check --offline`, all targets).
* **A-F3, one more case** (`fe9f96c`): `persist_files` accepted `profile.json` and
  `packs.lock.json`, and the migration into `persist/` would have moved them. Refused now
  (`instance::OWN_FILES`); two tests that failed before.
* **`launch::shields`** (`6c8fefd`): what the `Protection` event says for each outcome is
  decided in one small module with a test (`a_shield_is_on_only_for_what_was_seen_to_be_
  whole`). No change in behaviour.
* **`tests/confine.rs`** (`bc7ef0b`): the confined child removes its scratch directory with
  `tree::remove`, as the launcher does, including a mode-000 directory.
* **Opt-in X11 test** (`51b0373`): `tests/sandbox.rs::x11_client_connects_in_each_network_
  mode` failed with `BASTION_X11_TESTS=1` — gamescope was installed system-wide today at
  19:50 (`pacman -Qi gamescope`), so the plan for an X11 session became a nested
  compositor, which that test does not start. Unrelated to this package; the test now
  asks for the direct display explicitly.

## Final verification (third engineer) — done

On the final tree, every command with `DISPLAY=:77 XAUTHORITY=/dev/null`:

* `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets` no warnings;
  `cargo test --workspace --no-fail-fast`: **373 passed, 0 failed**.
* The launcher's tests with `BASTION_NET_TESTS=1 BASTION_X11_TESTS=1`: 314 passed, 0 failed.
* Wayland suite against headless sway 1.12: 2/2. Nested suite on `:77` against gamescope
  3.17 with the clipboard test: 4/4.
* Four real demo launches (logs in `target/a5-logs/launch-{last-x11,final-wl,final-burner,
  last-fab}.{out,info}`, not committed): through gamescope with `--events`, through a
  security context on headless sway, `--burner`, and the signed Fabric profile. Each: limits
  read back, launcher confined (`Seccomp: 2`, `Seccomp_filters: 3`), exit status 130 on
  SIGINT, nothing left in the cache, no process left.
* The table with every result is at the end of `docs/notes/review-fixes.md`.

## Fourth engineer (2026-11-13, from 22:06)

* **Found at take-over:** `cargo test --workspace --no-fail-fast` 373 passed, 0 failed
  (`target/a5-logs/test-e4-1.log`) — the third engineer's figure holds.
* **Critical review of the whole package** (every file in `git diff 51bc003..HEAD`, read
  against the findings and the brief). Holds as written: `confine/filter.rs` (x32 guard ahead
  of the compiled program, i386 killed by the arch check, relative jumps), `confine.rs`,
  the start-up order and `launch::shields`, `nested.rs` (output hold: tokio's `read_until`
  keeps partial bytes over a cancelled select, the fair RwLock), `noclip.c`, `cgroup.rs`,
  `wayland.rs`, `display.rs`, `instance.rs`, `persist.rs`, `broker.rs`, `store.rs` and
  `doctor`, `zipdir.rs`/`packs.rs`, the skin decoder, `canary.rs`/`cli.rs`, `tree.rs`,
  `running.rs`, `tests/confine.rs`, `tests/scram.rs`. The cache grant of the confined
  launcher holds only per-launch directories (verified downloads are in the data directory).
* **Repaired** (each with a test that failed before):
  * scram: a command that could not be stopped got a second `COULD NOT` line saying it
    "started while this was running" (false). Test:
    `tests/scram.rs::a_command_that_cannot_be_stopped_is_reported_as_that_and_as_nothing_else`
    (a marker naming a *thread* of a child process: identity checks out, `pidfd_open`
    refuses it — `ENOENT` on this kernel — so stopping fails before any signal).
  * scram: an unreadable markers directory gave "no launch was in progress" and exit 0.
    Now a `COULD NOT find out which launcher commands are running` line
    (`running::unreadable`). Test: `tests/scram.rs::markers_that_cannot_be_read_are_not_taken_for_nothing_running`.
  * scram: the vault loop dropped directory-entry errors (`.flatten()`) and never read the
    directory again, though the module header said "a vault is empty when the directory was
    read again". Now the second look exists; what it finds that was not already reported is
    a `COULD NOT … is still there` line. The case it exists for (an entry the first listing
    did not return) could not be produced deterministically; the unit test
    `scram::tests::an_unreadable_or_unexpected_vault_is_reported` checks that nothing is
    reported twice.
  * `config.rs`: the `persist_files` refusal had a run of spaces in the middle of the
    message (a broken line continuation, the same kind the third engineer fixed elsewhere);
    the config test now checks the wording.

* **Documents** (`a1a11e1` and the last commit): README, THREAT_MODEL, TODO, MC_VAULT_ACCOUNT,
  TOKEN_INJECTION read against the code — they hold; updated for the repairs (THREAT_MODEL
  A2: 9 scram tests, the vault's second look, unreadable markers; TODO "Independent
  review"; README test count 375 and one over-long line reflowed); `review-fixes.md` has
  the repairs under B-L3, B-L6 and A-F3 and a final-verification section of its own.
* **Final verification on the final tree — done** (logs `target/a5-logs/*-e4*`, not
  committed): `cargo fmt --all` no change; clippy no warnings; `cargo test --workspace
  --no-fail-fast` **375 passed, 0 failed**; launcher tests with `BASTION_NET_TESTS=1
  BASTION_X11_TESTS=1` 316/316; Wayland suite on headless sway 2/2; nested suite with the
  clipboard test 4/4; one real demo launch through gamescope on `:77`
  (`target/a5-logs/run-launch-e4.sh e4-final plain vanilla-latest --demo`): started, ran,
  confined (`Seccomp: 2`, `Seccomp_filters: 3`), exit 130 on SIGINT, nothing left.
  New in that launch: the descriptors of every process under the launcher were recorded —
  the JVM holds no descriptor of bubblewrap's status pipe, so the one stream read before
  confinement is not writable by the game (A-F6's claim, now observed).

## In progress

Nothing. **The package is complete** (fourth engineer, 2026-11-13).

## Next

Nothing in this package. For whoever integrates: see "Integration" below.

## Integration (what the other four branches and the integrator need to know)

* **Start-up order changed.** In `launch()` the launcher now confines itself *before*
  `egress::start` and before the output relays. Whatever `egress::start` (or anything added
  after step 9) does runs confined: no `execve`, no new TCP listener (`bind` is denied; the
  relay's sockets are created before), TCP `connect` only to `keep.tcp_connect` (ports the
  policy resolved, plus the SOCKS5 proxy's port), files only under `instances/<id>/persist/`
  and the cache, no kernel keyring. UDP is not restricted. A relay that needs something
  else must get it before step 9 or add it to the `Confinement` there.
* **`Protection` event:** same shape; `shield` can now be `partial` for `launcher`, `limits`
  is `off` when limits were asked for but are not in force, and the `detail` texts changed
  (`launch::shields`). A front end that matches on detail strings must be updated.
* **scram:** the contract is unchanged (exit status 1 and lines starting `COULD NOT ` when
  incomplete). New line shapes: ``killed `bastion-mc login` (pid N) before it could store a
  sign-in``, `… ended by itself`; from the fourth engineer: `COULD NOT find out which
  launcher commands are running (…)`, `COULD NOT clear the vault directory: "<name>" in
  <dir> is still there`, `COULD NOT read the vault directory <dir> again (…)`. "no launch
  was in progress" is now only said when the markers directory could be read. Its own waits are bounded: 5 s to stop writers and 2 s
  for the key list per pass, at most two passes, 15 s for the sweep — about half a minute
  in the worst case.
* **Shared files touched:** `src/launch.rs` (large: reordered start-up, `shields`, scratch
  handling, log shares), `src/cli.rs` (markers for `login`/`skin`/`cape`/`canary`/`vault`,
  canary notes, skin message), `src/config.rs` (two refusals in `validate`, doc comments),
  `examples/config.toml` (comments only: it still parses to the defaults), `src/fuzzing.rs`
  and `tests/robustness.rs` (the `zipdir` harness: one line in `ALL`, one test, seeds),
  `Cargo.toml` (`flate2`, one block), `Cargo.lock` and `fuzz/Cargo.lock` (one edge each),
  `src/events.rs` (`Reporter::warn`), `src/lib.rs` (`pub mod tree`), `src/util.rs`
  (`hash_file`), `src/paths.rs` (`SYSTEM_AGE_RECIPIENTS`). `build.rs` is untouched. The
  fourth engineer touched `src/config.rs` once more (the text of one refusal, and its test),
  `src/running.rs` (one new function, `unreadable`), `src/scram.rs` and `tests/scram.rs`.
* **README "Limits, proxy"** has one sentence that says pass-through connections do not go
  through `launcher_proxy` (B-M3). It describes this branch. Remove it when the egress
  branch with that fix is merged.
* **Behaviour that users will notice:** settings files move to `instances/<id>/persist/` at
  the next launch; packs with an archive comment or in zip64 format are refused (also ones
  recorded earlier); the `require` modes are stricter (`launcher.confine`: Landlock ABI 9;
  `sandbox.cgroup`: limits read back; `wayland_security_context`: nothing privileged on the
  game's socket; `nested_compositor`: the clipboard cut-off must report).

## Things learned about the code and this machine

* Shell state does not persist: every command starts with `cd <clone> && …`.
* **`DISPLAY=:0` is set in the agent's environment.** Every cargo test and every launch
  must be given `DISPLAY=:77 XAUTHORITY=/dev/null` explicitly.
* `:77` is the private X display; check with `timeout 5 xprop -display :77 -root | head -1`.
* gamescope 3.17 is installed in `/usr/bin` on this machine (installed 2026-11-13 19:50, per
  `pacman -Qi`; the binary is dated 2026-10-24), so a launch on `:77` goes through the
  nested compositor by default — and so does every display plan that takes the defaults.
* Runtime directories must be short: `/run/user/1000/bmc-a5…` (stale sockets of the first
  engineer's sway are in `/run/user/1000/bmc-a5`; no process of this clone was left).
* The kernel keyring is the user's real one: tests only through `scram::run_for` with
  uniquely named accounts.
* systemd 262, `DefaultOOMPolicy=stop`: a scope struck by the OOM killer is stopped by the
  user manager, which usually beats bubblewrap to it (exit by SIGTERM, not status 137).
* The machine is shared: timing-sensitive tests do flake under load; re-run before
  believing a failure, and look for the race.
* Demo launch helper used here (`target/a5-logs/run-launch.sh`, not in the repository):
  start the launcher in the background with the four XDG variables pointing into
  `target/e2e`, poll its output for "Reloading ResourceManager", record
  `/proc/<pid>/status`, `pkill -INT -f "<clone>/target/debug/bastion-mc launch"`, wait for
  exit status 130. With `--events` the launcher's standard input is its control channel
  (end-of-file means stop): give it a FIFO that the script holds open.
* Process ids on this machine passed 1,000,000 during this work: anything that builds a
  path or a name from a pid got one character longer.
* The headless sway for the Wayland suite: `tests/support/sway-headless.sh
  /run/user/1000/bmc-a5 gles2` under `nohup … &`; it survives between commands. Stopped
  and its directory removed at the end of this session.
* `cargo test --workspace` takes about 1.5 minutes here; the GUI tests alone 16 s.
