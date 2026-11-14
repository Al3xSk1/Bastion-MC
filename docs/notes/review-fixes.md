# What was done about the two independent reviews (2026-11-13)

The findings are in [`review-findings.md`](review-findings.md): 27 entries, none critical or
high. This file is the record, finding by finding: **how it was verified**, **what changed**,
**which test covers it** — or why nothing changed. Where the work stands as a whole (and how
to continue it) is in [`review-fixes-progress.md`](review-fixes-progress.md).

Four engineers worked on this package one after the other (the first two were cut off, the
third was stopped on purpose to move the team to another model; the fourth read the whole
package again critically before handing it in); commit ids are given so that each change
can be read on its own. "Baseline" is the tree the
reviewers read (`c1e3ea0`). Every defect below was confirmed in the baseline source before
anything was changed; "reproduced" means that it was also made to happen.

**Result:** of the 27 findings, 25 belong to this package (all but B-M2 and B-M3), plus the
adjacent point about the log budget. **All of them hold, and all are fixed** — for A-F10 and
for part of B-M4 the fix is a narrower claim, not more code. No finding was rejected. B-M2
(GUI) and B-M3 (egress relay) belong to other engineers and are untouched; of B-M1 and B-L2
the launcher halves are done here and the GUI halves are theirs.

Verification at the end, all on this machine (kernel 7.2.14, Landlock ABI 10, systemd 262,
OpenJDK 27, gamescope 3.17; no account, no `age`, no root): see the last section.

---------------------------------------------------------------------------------------------

## Review A

### A-F1 — the confined launcher could still read the refresh token from the kernel keyring

* **Verified, and reproduced.** Baseline `confine.rs` installs a Landlock ruleset and nothing
  else. Experiment (a throw-away test, in a child process with a session keyring of its own):
  under a fully enforced Landlock ruleset that denies every file (`/etc/passwd`:
  `PermissionDenied`), `keyctl` search and read of a key still return its 22 bytes. Landlock
  has no say over keys.
* **Changed** (`b4de0b1`): a seccomp filter on **all threads** (`SECCOMP_FILTER_FLAG_TSYNC`),
  installed by `confine::apply` right after the Landlock ruleset — new module
  `src/confine/filter.rs`. It is a **denylist**, and the module header says why (an allowlist
  for glibc + resolver + async runtime + TLS stack would break in somebody's session the day
  a library takes a new path) and what makes a denylist sound here:
  * `keyctl`, `add_key`, `request_key`, `execve`, `execveat`, `unshare`, `io_uring_*`,
    `userfaultfd`, `perf_event_open`, `modify_ldt`, `pidfd_getfd` → `EPERM`;
  * `clone` with any namespace flag → `EPERM`; `clone3` → `ENOSYS` (glibc then uses `clone`,
    whose flags a filter can read);
  * `ioctl` with `TIOCSTI`, `TIOCLINUX`, `TIOCCONS`, `TIOCSETD` → `EPERM` (the reviewer's
    related point about the terminal the launcher was started from);
  * the game sandbox's tripwire list (`ptrace`, `process_vm_*`, `bpf`, the mount API,
    `setns`, `kexec_*`, modules, …) → the process is killed;
  * **every x32 system-call number** (`nr >= 0x40000000`) → `ENOSYS`, checked before
    anything else, and any other architecture's table → kill. Without these two a denylist
    is walked around under the other number of the same call (this kernel has the x32 ABI).
* **Does anything after confinement need the keyring, or a new program?** No, by reading
  `launch.rs`: the token store is opened, read, written and (canary wipe) deleted in step 7,
  before the sandbox exists; `bwrap`, `systemd-run`, `age` and the compositor are all started
  before step 9. This could only be read, not run: there is no account on this machine, so
  no launch here ever touched a token store.
* **Tests:** `tests/confine.rs::a_confined_process_keeps_only_what_it_was_left` — a real
  multi-threaded child (a thread and a runtime worker that exist before confinement, the main
  thread, a thread started after it): before confinement the four keyring calls work; after
  it they fail with `EPERM` on every thread and with `ENOSYS` under their x32 numbers, as do
  `execve` (also of a memfd, which no Landlock rule can name), namespaces, `io_uring`,
  `userfaultfd`, `perf_event_open`, `pidfd_getfd`, `TIOCSTI` — while relaying on a socket
  bound earlier, writing the settings back through the sanitiser, removing the scratch
  directory, starting threads, resolving names, allocating token memory, signalling and
  reaping the sandbox all still work. `…::an_exploit_only_system_call_kills_the_confined_
  launcher` (a `ptrace` from a thread that existed before the filter: SIGSYS).
  `confine::filter::tests::*`. Real launches: `/proc/<launcher>/status` showed `Seccomp: 2`,
  `Seccomp_filters: 3`, `NoNewPrivs: 1`.
* **Not guaranteed, and said so in the module:** a system call that a future kernel adds is
  allowed until it is put on the list; UDP, TCP by destination, signals to other processes
  of the user and text written to the terminal are not restricted (THREAT_MODEL J18).

### A-F2 — on Landlock ABI 8 the pathname-socket rule was dropped and "confined" was still said

* **Verified, and reproduced.** Baseline: `MIN_ABI = 8`, the ruleset requested under
  `BestEffort`, a partially enforced ruleset accepted. Reproduced on this ABI-10 kernel by
  applying exactly what an ABI-8 kernel can (`confine::apply_as(.., 8)`): a pathname socket
  outside the kept trees is then connectable from every thread.
* **Changed** (`b4de0b1`, launch side `4a62271`): **the code now meets the claim** instead of
  the claim being narrowed. `confine::support_for(abi)` says what a kernel can enforce
  (`Whole` from ABI 9; below that the list of what is missing). Nothing is requested
  best-effort any more (`CompatLevel::HardRequirement`). `apply` returns the parts that are
  *not* in force. `launcher.confine = "require"` means the whole: refused before anything is
  started (`launch.rs`, right after the instance is loaded) and again in `apply`. `"auto"`
  applies what it can and says what is missing in three places: the launch log
  (`launcher: WARNING: only PARTLY confined — not in force: …`), `doctor`
  (`self-confinement: only partly possible on this kernel: …`, a failure under `require`),
  and the `Protection` event (`Shield::Partial` with the gaps as detail).
* **Tests:** `tests/confine.rs::a_kernel_without_pathname_socket_control_is_confined_partly_
  and_says_so` (gap real and reported; with `Demand::Whole` nothing at all is applied) and
  `…::a_kernel_without_thread_synchronised_landlock_gets_the_filter_alone` (emulated ABI 7);
  `confine::tests::only_a_kernel_that_mediates_pathname_sockets_supports_the_whole_
  confinement`; `launch::shields::tests::a_shield_is_on_only_for_what_was_seen_to_be_whole`
  (the `Protection` event: a confinement with a gap is `Partial` and names the gap).
* **Not verified:** a real kernel with ABI 8 or older. The partial states are produced here
  by asking an ABI-10 kernel for less.

### A-F3 — the write grant covered `instance.toml`

* **Verified** by reading: baseline `keep.write.push(inst.dir.clone())`, also in burner
  sessions; `inst.config.network.mode` is read from that directory at the next launch.
* **Changed** (`0807147`/`4a62271`, config `9e8e143`): the settings files the launcher
  carries now rest in **`instances/<id>/persist/`**, and that directory — plus the cache — is
  all the confined launcher can write. `instance.toml`, the manifest and its signature and
  `packs.lock.json` are outside it. **Existing instances keep working:** at the next launch
  `instance::adopt_persisted` moves `options.txt`/`servers.dat` from the instance directory
  into `persist/` (a regular file only, never over an existing one, never through a link; if
  both places hold one, the one in `persist/` is used and the other is left and reported).
  **Burner sessions get no grant at all** and create or move nothing: they read the files
  from wherever they are (`instance::persisted_in`). `sandbox.extra_rw_dirs` refuses
  `persist` (the game writing there would bypass the sanitiser).
* **Tests:** `instance::tests::settings_files_of_an_existing_instance_move_into_the_persist_
  directory`, `config::tests::the_launchers_own_instance_directories_cannot_be_handed_to_
  the_game`; real launches: the log says `files only under the instance's persist/ and the
  cache` (burner: `(nothing of the instance)`), and `persist/options.txt` was written back
  by the confined launcher.
* **Found while checking the migration** (third engineer): `sandbox.persist_files` accepted
  `profile.json` and `packs.lock.json` (both are `.json`, a known format) — and the move
  into `persist/` would then have taken the instance's manifest or pack list with it.
  The launcher's own files (`instance::OWN_FILES`) are now refused as settings files by the
  configuration and by `adopt_persisted` itself. Both tests failed before the change:
  `config::tests::the_launchers_own_instance_files_cannot_be_carried_as_settings`, and the
  new part of the instance test above. (The configuration's refusal had a run of spaces in
  the middle of its text — a broken line continuation; repaired by the fourth engineer,
  `a62a00c`, and that test now checks the wording.)
* **What a compromised launcher can still do there:** rewrite the settings files in
  `persist/`. They are parsed and re-encoded again before the next game gets them.

### A-F4 — the Wayland event reader had no overall bound

* **Verified** by reading: baseline `pump` loops for as long as a byte arrives every 5 s; the
  256 KiB guard cannot fire because complete messages are drained before every read;
  `connect()` precedes every timeout.
* **Changed** (`de848e8`): one conversation is bounded in time (`DEADLINE`, 10 s), in events
  (`MAX_EVENTS`, 4096) and in bytes (`MAX_BYTES`, 1 MiB); the connect is non-blocking with a
  bounded retry (a full backlog no longer blocks).
* **Tests:** `sandbox::wayland::tests::a_peer_that_never_stops_sending_is_given_up_on` (as
  fast as the socket takes it: the event budget; slowly but steadily: the deadline),
  `…::a_socket_with_a_full_backlog_does_not_block_the_connect`,
  `…::protocol_errors_and_silence_are_failures`; `robustness::wayland_event_reader`.

### A-F5 — the broker could be made to hold about 2 GiB of replies

* **Verified** by reading: baseline buffers every reply whole (8 MiB cap) in a
  `Full<Bytes>`, 256 connection slots, no budget across them.
* **Changed** (`aa2ae5c`, `src/egress/broker.rs` only): **one budget for all buffered
  bodies, 64 MiB over all connections** (`BUFFER_BUDGET`). A request body is reserved by its
  declared size; before a request is forwarded, room for the largest possible reply is
  reserved and the unused part given back when the reply is here; a reply keeps its share
  until the game has read it or its connection is gone, and is handed to the HTTP layer in
  16 KiB frames so that the unread part stays where it is counted. A request that finds no
  room waits 10 s and is answered `503`. The only one this can starve is the game.
  Streaming anonymous replies instead (the reviewer's other suggestion) was not chosen: a
  reply is checked for the token as a whole, and one code path for all replies is easier to
  keep right than two.
* Also the reviewer's caveat under "found clean" (an encoded reply would pass the literal
  token check): **a reply to a request that carried the token and comes back with a
  `Content-Encoding` is withheld** (`502`, logged `[SECURITY]`); the launcher asks for
  `identity`.
* **Tests:** `tests/broker.rs::replies_the_game_does_not_read_come_out_of_one_bounded_
  budget`, `…::an_encoded_reply_to_a_credentialed_request_is_withheld`,
  `egress::broker::tests::buffered_bodies_come_out_of_one_budget`.
* **What the figure does not include** (stated at the constant): hyper's own buffers
  (64 KiB each way per connection) and allocator slack. **Not under the budget:** the
  texture gateway in `src/egress/mod.rs` (another engineer's file), which holds up to 4 MiB
  per connection while a texture is fetched; said in THREAT_MODEL §5.6.

### A-F6 — "launcher" protection announced before `apply()`, relay started before it

* **Verified** by reading: baseline builds the `Protection` event from
  `confine::supported().is_ok()` before `apply` runs, and spawns `egress::start` and the log
  relays before it.
* **Changed** (`0807147`/`4a62271`): the start-up order allows it, so the order changed.
  Now: start the sandbox and deliver the frame → read the limits back → **confine** → emit
  the `Protection` event, built from what each step returned → only then `egress::start`
  and the output relays. SRV records are looked up before the game exists, so the set of
  ports is closed when the launcher confines itself. The one stream that was already being
  read — the nested compositor's output, which the game can influence once it runs — is
  held back for that stretch (`nested::Session::hold_output`). The "launcher" item is `On`
  only for a whole confinement, `Partial` with the gaps, `Off` when confining failed or is
  switched off.
* **Tests:** `sandbox::nested::tests::held_output_is_not_read_until_it_is_released_and_
  nothing_is_lost`; `launch::shields::tests::a_shield_is_on_only_for_what_was_seen_to_be_
  whole` (what the event says for each outcome — the decisions were moved into a small
  module `launch::shields` so that they can be tested without a launch); the real launches,
  where the log shows `launcher: confined itself…` before the first line from the game, the
  relay or the compositor after the game's start, and the `Protection` event after it.
  The order itself is a property of `launch()` that no unit test asserts.
* **What is read before confinement:** bubblewrap's own status line and `/proc` (to find the
  game's process id and its control group) — written by bubblewrap and the kernel, not by
  the game. Seen in a real launch by the fourth engineer: no process in the game's sandbox
  holds a descriptor of bubblewrap's status pipe (last section).

### A-F7 — the cgroup preflight proved a scope, not limits

* **Verified** by reading: baseline `preflight` runs `systemd-run … /usr/bin/true` and takes
  exit status 0 for "limits enforced"; `CPUQuota` was never part of the probe. **The
  scenario itself was not reproduced** (a user manager without a delegated controller): on
  this machine every controller is delegated.
* **Changed** (`de848e8`, launch side `0807147`/`4a62271`/`8b7e743`): **read back from the
  kernel.** `cgroup::verify`: the process must be in the scope that was asked for, and that
  control group's `memory.max`, `memory.swap.max`, `pids.max` (and `cpu.max`) must hold the
  values. The preflight does this with a probe scope around a sleeping process (and includes
  the CPU quota when one is configured); the launch does it again for the game — and for the
  nested compositor's scope — once they run. Not in force: `cgroup = "require"` stops the
  game and fails the launch; `"auto"` warns and the limits item is `Off`. A sandbox that has
  already ended (a game that failed to start) is reported as that, not as failed limits.
* **Tests:** `sandbox::cgroup::tests::limits_count_only_when_the_kernel_holds_them` (a fake
  hierarchy: wrong value, missing controller file, wrong group, hostile path);
  `tests/sandbox.rs::resource_limits_put_the_sandbox_in_a_scope_and_cap_its_tasks` (a real
  scope: the payload verifies, other limits do not, a process outside the scope does not);
  real launches (`limits: in force — … read back from the kernel`).
* **Not verified by a test:** the two branches in `launch()` for limits that are not in
  force (there is no way to produce that state here).

### A-F8 — a "secured" Wayland socket was never looked at

* **Verified** by reading: baseline `secure` returns after one round trip without a protocol
  error, and the launch then asserts "capture and input injection withheld". Reproduced with
  a stand-in compositor that takes the context and filters nothing.
* **Changed** (`de848e8`): after the setup the launcher **connects to the game's socket
  itself and lists it** (`wayland::Offered`): every interface, those of 16 known privileged
  ones among them (`wayland::PRIVILEGED`), how many a trusted client is offered and this
  socket is not. A privileged interface on offer, or a socket that cannot be listed:
  `wayland_security_context = "require"` refuses; `"auto"` says so in the display note and
  the display item is `Partial`, not `On`. `doctor`'s line no longer says "the compositor
  withholds…": it says that the protocol is implemented and points to the listing.
* **Tests:** `sandbox::wayland::tests::a_compositor_that_accepts_the_context_but_filters_
  nothing_is_found_out`, `sandbox::display::tests::a_security_context_that_withholds_nothing_
  is_reported_by_auto_and_refused_by_require` (`5a45193`); against real headless sway
  `tests/wayland.rs::*` (37 interfaces on the game's socket, none of the 16; 20 withheld).
* **What this cannot see** (module header): a privileged interface that is not on the list,
  and what a compositor does with requests on the interfaces it does offer.

### A-F9 — JSON re-encoding left DEL, C1 controls and bidi overrides raw

* **Verified, and reproduced** in the test: `serde_json::to_vec` writes U+009B as the bytes
  `C2 9B`.
* **Changed** (`8823b55`): `persist::write_json` — every character outside `' '..='~'` in a
  string or key is written as `\uXXXX` (a surrogate pair beyond U+FFFF). A JSON reader gets
  the identical value; the file is printable ASCII. A result past the size limit is refused.
* **Tests:** `persist::tests::json_strings_and_keys_come_out_as_printable_ascii_with_the_
  same_value`; the `persist` harness asserts it for every accepted input
  (`robustness::settings_sanitisers`).

### A-F10 — the join residual was understated in the threat model

* **Verified** by reading `broker.rs` (`JoinGate`, `join_body`): a join is single-use per
  ticket, a ticket is any game connection the relay has just opened to any allowlisted
  server, and `serverId` is only checked for its shape.
* **Changed:** documentation only — the claim was narrowed to what the code does.
  THREAT_MODEL J13 (residual), §5.3 and §8, TODO "Decided against": the join is **not tied
  to the server the connection goes to**; a mod that learns another server's hash can
  authorise a login as the user there, once per game connection it opens, with no limit on
  how many it opens over time; it needs at least one allowlisted game server. No code
  changed: removing this needs the login-protocol proxy (§8).

### A-adjacent — one log budget for all sources

* **Verified** by reading: baseline `MAX_LOG_BYTES` is one counter for every line.
* **Changed** (`0807147`, test `ccf9213`): a share per kind of line — launcher 8 MiB, relay
  and broker 24 MiB, the nested compositor's output 24 MiB, game 200 MiB. The relay's and
  the compositor's shares are still driven by what the game does; the launcher's own is out
  of its reach, and that is where the summary of refusals and the tripwire report go.
* **Test:** `launch::log_tests::a_source_that_fills_its_share_of_the_log_does_not_silence_
  the_others`.

### Review A, "found clean" — the two minor points

* The grant for the resolver configuration (`9e8e143`): only a daemon's own directory under
  `/run` is granted as a directory; a target directly in `/run`, or anywhere else, is granted
  as that file alone. `confine::tests::the_baseline_never_grants_a_shared_directory_for_the_
  resolver_configuration`.
* The literal token check on encoded replies: see A-F5.

---------------------------------------------------------------------------------------------

## Review B

### B-M1 (launcher half) — `scram` deleted game directories before it removed a credential

* **Verified** by reading: baseline `run_for` calls `kill_all`, then `sweep_leftovers`
  (unbounded `remove_dir_all`), and only then touches tokens.
* **Changed** (`123d860`): the order is the point of `scram.rs` now — (1) stop everything
  that could write a credential, (2) remove the credentials, (3) once more if something
  appeared meanwhile, (4) **last, and for at most 15 s**, the game directories. The deletion
  itself is `tree::remove` (bounded, cannot be stopped by permission bits or depth).
* **Tests:** `tests/scram.rs::writers_are_stopped_first_credentials_go_next_and_game_
  directories_last`, `…::what_cannot_be_deleted_is_reported_after_the_credentials_are_gone`.
* The GUI's 60-second timeout is the GUI engineer's half.

### B-L2 (launcher half) — `scram` did not stop commands that were about to store a token

* **Verified** by reading: only launches had markers; `login`, `skin`, `cape`, `canary` and
  `vault reseal` all end in `store.save`.
* **Changed** (`123d860`): every command that can store a token writes a marker
  (`running::Marker::for_command`): `login`, `skin`, `cape`, `canary`, `vault`. All four
  call sites of `store.save` are behind one. `scram` stops them like launches, and after
  removing the credentials looks again: a command that started meanwhile is stopped and the
  removal repeated; if one is still there, that is a `COULD NOT` line.
* **Tests:** the test above (a pending sign-in is killed and never stores its token);
  `running::tests::a_command_that_is_not_a_launch_is_marked_and_stoppable`.
* Stopping the front end's own sign-in job when scram is confirmed is the GUI engineer's.

### B-L3 — silent gaps in what `scram` removes

* **Verified, and the keyring scenario reproduced** (the reviewer had it as "plausible, not
  run"): two login sessions of the same user, the second signs in again and runs scram — on
  this kernel the first session's key **does** stay readable in that session.
* **Changed** (`123d860`, `ccf9213`): the kernel's key list is read before and after;
  removal no longer depends on a successful read of the key; a listed key that this session
  cannot reach, an unreadable `/proc/keys`, an unreadable vault directory, a directory inside
  the vault, a vault file that could not be deleted — each is a `COULD NOT` line and makes
  the result incomplete. Vault file names are shown sanitised.
* **Tests:** `tests/scram.rs::a_token_that_only_another_login_session_holds_is_reported_
  not_passed_over`, `scram::tests::a_listed_key_that_cannot_be_reached_makes_the_result_
  incomplete`, `…::an_unreadable_or_unexpected_vault_is_reported`.
* **Two more gaps of the same kind, found by the fourth engineer's review of the package**
  (`a62a00c`):
  * The markers of running commands were listed with `running::list`, which returns
    nothing for a directory it cannot read — and scram then said "no launch was in
    progress" and exited 0. Now `running::unreadable` is asked as well; an unreadable
    markers directory (or entry) is a `COULD NOT find out which launcher commands are
    running` line. Test (failed before): `tests/scram.rs::markers_that_cannot_be_read_are_
    not_taken_for_nothing_running`.
  * The module header said "a vault is empty when the directory was read again", but the
    directory was read once, and entries that could not be read were dropped by
    `.flatten()`. Now the directory is read a second time; anything there that was not
    already reported (a directory, a file that could not be deleted) is a `COULD NOT …
    is still there` line, and a second listing that fails is a `COULD NOT` too. The case it
    exists for — an entry the first listing did not return, or one that appeared meanwhile
    — could not be produced deterministically; `scram::tests::an_unreadable_or_unexpected_
    vault_is_reported` now checks that what the first look reported is not reported again.

### B-L4 — scratch cleanup could be defeated and was reported as done

* **Verified, and reproduced** in the test: `std::fs::remove_dir_all` stops with
  `PermissionDenied` at a directory without permission bits, which the game can make.
* **Changed** (`123d860`, `0807147`/`4a62271`): new module `src/tree.rs`. `tree::remove`
  makes each directory enterable before it enters it, never follows a link, holds one
  directory open however deep the tree is, takes a deadline, and returns an error for what
  it could not do. The launch says "deleted" only when that is true and otherwise names what
  is left (also in a burner session); a sweep reports what it could not remove.
* **Tests:** `tree::tests::*` (6), `running::tests::a_leftover_the_game_made_undeletable_is_
  reported_not_counted`, `launch::scratch_tests::a_game_root_that_could_not_be_deleted_is_
  named_and_one_without_permission_bits_is_deleted` (`670869c`); a real `--burner` launch.

### B-L5 — a launcher was recognised only by the looking process's own name

* **Verified** by reading (`comm == own`), **and reproduced**: the test starts the same
  binary through a symbolic link and finds it under another command name.
* **Changed** (`123d860`): the marker records the command name of the process that wrote it,
  and that is what is compared.
* **Test:** `running::tests::a_launcher_running_under_another_name_is_recognised_and_
  stopped`. A launch whose marker is lost altogether still holds its game directory locked:
  `running::tests::a_directory_locked_by_a_live_launch_survives_a_sweep_even_without_a_
  marker`, and scram reports it (`tests/scram.rs::a_launch_without_a_marker_is_reported_by_
  the_directory_it_holds`).

### B-L6 — kill race, and "killed" reported unconditionally

* **Verified** by reading: baseline `kill_all` checks, then `kill(pid)`, then returns every
  listed launch whatever happened.
* **Changed** (`123d860`): a **pidfd** is opened first, the identity (start time) is checked
  after that, the signal goes to the descriptor, and `Stopped::Killed` is only returned when
  the kernel reported the end of that process within the wait. Otherwise "ended by itself"
  or a `COULD NOT` line.
* **Tests:** `running::tests::only_the_process_that_wrote_a_marker_is_ever_found_or_killed`,
  `…::a_command_that_is_not_a_launch_is_marked_and_stoppable`.
* **Repaired by the fourth engineer** (`a62a00c`): a command that could not be stopped got,
  besides its `COULD NOT stop …` line, a second one on scram's second look claiming that it
  "started while this was running" — untrue, it had been there all along. Now only a command
  that was in neither pass gets that line. Test (failed before):
  `tests/scram.rs::a_command_that_cannot_be_stopped_is_reported_as_that_and_as_nothing_else`.
  A failed stop is produced deterministically with a marker that names a *thread* of a
  child process: its identity checks out (a thread has a start time and a name of its own in
  `/proc`), and `pidfd_open` refuses it (`ENOENT` on this kernel), so stopping fails before
  any signal is sent. (How a real command fails to stop — stuck in the kernel for longer than
  scram waits — cannot be arranged in a test.)

### The scram contract (what the GUI engineer builds against) — unchanged in form

Incomplete: **exit status 1**, and every thing left undone is a line that **starts with**
`COULD NOT ` (`tests/scram.rs::what_cannot_be_deleted…` asserts the prefix for every such
line). Complete: exit status 0. With `--events` the lines arrive as `status` events, as
before. Never run here: the real `scram --yes` (the kernel keyring is the user's); the tests
go through `scram::run_for` with uniquely named accounts.

### B-M4 — the "hardware-backed" vault trusted a recipients file anyone running as the user can edit

* **Verified** by reading: `AgeVault::save` passes `--recipients-file <path from
  config.toml>`; the launcher re-encrypts the rotated token at every launch; encrypting
  needs no device. Not run against a real `age` (not installed here).
* **Changed** (`0a987c4`): two things.
  1. **A root-owned anchor**, in the style of the root-owned trusted-keys directory:
     when `/etc/bastion-mc/age-recipients` exists, the vault is encrypted to exactly its
     recipients; `recipients_file` settings are ignored; only a program called `age` or
     `rage` (root-owned, as before) is run; and `token_store = "keyutils"` is refused —
     otherwise one line of `config.toml` would be the way around it. Something at that path
     that is not a plain root-owned file in a root-owned directory is an error, never a
     reason to fall back.
  2. **The claims were narrowed to what holds** (`store.rs` header, `doctor`, README,
     THREAT_MODEL §5.1, MC_VAULT_ACCOUNT): a device identity makes a *copy* of the vault file
     useless; it does not stop a program running as the user from asking the connected
     device to decrypt, and it has no say over the recipients. Without the anchor the vault
     is no stronger against same-user software than the files it is configured with.
* **What the anchor does not do** (also said in those places): protect a launcher that is
  itself started through something another program put in its way (a replaced binary in
  `$PATH`, a wrapper, a preloaded library) — such a program has the token when the user
  launches, whatever the vault is encrypted to. The anchor ends the quiet, lasting variant.
* **Tests:** `auth::store::tests::a_root_owned_recipients_file_overrides_the_configuration`
  (a stand-in path and a stand-in `age`). **Not verified:** real root ownership (no root
  here), a real `age`, a real device.

### B-L12 — `identity_kind` over-reported hardware backing

* **Verified** by reading: baseline returns `Plugin` at the first `AGE-PLUGIN-` line and
  treats every plugin name as hardware.
* **Changed** (`0a987c4`): every identity in the file counts. A software key next to a
  plugin identity is reported as a software key. Only `yubikey`, `tpm` and `fido2-hmac` are
  called device identities, and the text says that this is what their documentation says,
  not something the launcher can test; any other plugin is "not known". The recipients file
  is classified the same way (`store::recipient_kinds`), and `doctor` reports both halves.
* **Tests:** `auth::store::tests::identity_files_are_classified_by_what_they_hold`,
  `…::recipients_are_counted_by_kind`.

### B-L1 — canary `warn` mode was silent in several paths and re-pinned a foreign change

* **Verified** by reading: baseline `profile_admin` discards the note of `canary_verdict`
  and later pins the whole profile (`canary::accept(paths, &account, &after)`).
* **Changed** (`0807147`/`4a62271`): `launch::CanaryNote` carries `notable`; a notable note
  (a difference under `warn`, a damaged record, a first pinning) is shown by every caller
  through `Reporter::warn`, which also prints under `--quiet`, and is recorded in the launch
  log. Under `warn` the pin and a damaged record stay exactly as they were. A change made
  through the launcher pins **the aspect that command changed** (`canary::accept_change`),
  not the whole profile — so a third-party change that lands between the two profile
  fetches stays a difference, and is reported.
* **Tests:** `auth::canary::tests::an_own_change_pins_its_own_aspect_and_leaves_foreign_
  changes_unpinned`, `launch::canary_tests::the_canary_pins_first_then_enforces_warns_or_
  stays_out_of_it`. Not run against a real profile (no account).

### B-L7, B-L8, B-L9 — packs: the check saw one archive, the game another

* **Verified, and reproduced** (THREAT_MODEL §7 E17, re-run by the third engineer): an
  archive whose end record declares 1 member in front of a directory with 3 headers — the
  zip crate (9.5.1) lists 1, `java.util.zip.ZipFile` (OpenJDK 27) lists 3, including a
  `../../` name; the same name in two headers — the crate keeps 1, Java lists 2. Minecraft
  26.3 opens resource packs with `java.util.zip.ZipFile` (`FilePackResources`, read from the
  bytecode of the installed client jar). B-L8 and B-L9 by reading: `ZipArchive::new` before
  the entry limit; check and hash in separate passes over a source the game may write.
* **Changed** (`73dbe11`): new module `src/profile/zipdir.rs`, a **strict reader of the zip
  directory** that replaces the lenient one for this purpose. It accepts one shape and
  refuses the rest instead of guessing: the archive starts with a member and ends with the
  end record (no comment, nothing after it); no zip64; the directory lies exactly between
  the members and the end record and holds exactly the declared number of headers; every
  name is UTF-8, plain, relative, and occurs once; no "Unicode path" or zip64 extra field;
  no links, special files or encrypted members. It is bounded before it reads (16-bit count,
  directory size checked against the limit and the file's length before allocation). **One
  set of bytes:** `pack import` reads its source once — copy and hash in the same pass — and
  everything after that (the check, the record) is about its own copy through the descriptor
  it was written with; `adopt` and the launch check the descriptor that is hashed.
* **Tests:** `profile::zipdir::tests::*` (7), `profile::packs::tests::archives_with_an_
  ambiguous_directory_are_refused_everywhere`, `…::an_import_reads_its_source_once_and_
  records_what_it_checked`; harness `fuzzing::zipdir` (an accepted archive is listed
  identically by the zip crate), `robustness::zip_directory_reader`, fuzz target
  `fuzz/fuzz_targets/zipdir.rs`.
* **Consequence for users:** an archive with a comment, or in zip64 format, is refused — also
  one that was recorded before this change, at the next launch, with a message that says how
  to get out (re-create the zip, import again). Said in README.
* **Not checked, deliberately** (module header): local headers beyond their signature; a
  streaming reader may see other names. How a shader mod opens a shader pack is that mod's
  business.

### B-L10 — the PNG sanitiser passed arbitrary non-image bytes

* **Verified, and reproduced** in the test: an RGBA file with ninety `tRNS` chunks, a
  `PLTE` in an RGBA image, bytes behind the compressed pixels and a zip where the pixels
  should be all went through the baseline sanitiser unchanged.
* **Changed** (`5dcfdd8`): **the claim is now true.** `sanitize_skin_png` decodes the image
  to its pixels (inflate with an exact size, un-filter, palette and transparency applied)
  and encodes a new file in one fixed form. What is uploaded is the image's height and, per
  pixel, four colour values. `flate2` became a direct dependency — the inflate/deflate crate
  `zip` already brings, with the same features; `Cargo.lock` gained one edge and no crate.
* **Tests:** `auth::minecraft::tests::nothing_but_the_pixels_can_ride_along`,
  `…::a_skin_is_decoded_and_encoded_again_and_only_its_pixels_remain`, `…::rgb_and_indexed_
  skins_become_the_same_rgba_image`, `…::files_that_are_not_skins_are_refused`;
  `robustness::profile_reply_and_skin_files`.
* **What still leaves with the pixels**, because it is part of them: the colour values of
  fully transparent pixels (the game shows some of them). Not run against Mojang's upload
  endpoint (no account).

### B-L11 — `copy_tree` was unbounded and followed a swapped directory

* **Verified** by reading.
* **Changed** (`123d860`, call site `0807147`/`4a62271`): `tree::copy` — every name resolved
  relative to an open directory descriptor, `O_NOFOLLOW`, bounded in entries (1,000,000),
  depth (64) and bytes. The byte budget of a burner session is what is free in the cache
  less a tenth (at least 1 GiB), and a file's size is compared with what is left *before* a
  byte is written.
* **Tests:** `tree::tests::a_copy_stops_at_its_limits_before_filling_the_disk` (a sparse
  file of 1 TiB is refused at once), `…::a_copy_takes_files_and_directories_and_nothing_
  else`; a real `--burner` launch.

### B-L13 — the clipboard shim failed open

* **Verified, and reproduced**: started without the library, gamescope 3.17 bridges
  CLIPBOARD both ways (the control run of the nested suite), and the baseline would have
  said "clipboard cut" all the same.
* **Changed** (`de848e8`): the preloaded library **reports from inside the compositor** — a
  status word that the launcher's ready child passes on. Only `active` lets the launch say
  that the clipboard is cut: the library is loaded, the six SDL text-clipboard functions
  resolve to its definitions, an SDL library that provides them stands behind it, and SDL's
  interface for clipboard data of any type does not exist in the compositor's symbol scope.
  No word, or any other word: `nested_compositor = "auto"` says that the clipboard is *not
  known* to be cut (display item `Partial`); `"require"` stops the launch. The status can
  only come from the library: the variable is removed from the compositor's environment.
* **Tests:** `tests/nested.rs::a_compositor_without_the_cut_off_is_noticed_and_not_called_
  cut`, `…::no_selection_crosses_the_nested_compositor`, `sandbox::nested::tests::only_the_
  report_active_counts_as_a_cut_clipboard`, `…::without_the_library_nothing_is_preloaded_
  and_no_report_can_be_inherited`.
* **What `active` does not establish** (module header): that a given build of the compositor
  has no other way of moving a selection. Tested for gamescope 3.17 as packaged by Arch.

---------------------------------------------------------------------------------------------

## Not findings: tests that failed here for reasons of their own

* `tests/sandbox.rs::resource_limits_kill_a_memory_hog_inside_the_scope` — a race between
  bubblewrap reporting its child's death (status 137) and systemd stopping the scope under
  its default `OOMPolicy=stop`. The test accepts both ends (THREAT_MODEL §7 E18).
* `tests/broker.rs::a_sandboxed_jvm_holding_only_the_placeholder` — the probe asked for a
  join before the relay had accepted its game connection. It now waits for the server's
  answer, as the real protocol does.
* `tests/confine.rs` — its pathname socket lay under the build directory; with a seven-digit
  process id the path in this (deep) clone was one byte longer than a socket address holds.
  The socket now lives in a short directory of its own.
* `tests/sandbox.rs::x11_client_connects_in_each_network_mode` (opt-in,
  `BASTION_X11_TESTS=1`) — "unable to open display". The test hands the client the
  session's display directly, but took the defaults for everything else; gamescope was
  installed system-wide on this machine at 19:50 on the day of this work, and from then on
  the display plan for an X11 session is a nested compositor, which the test does not
  start. Nothing in this package changed that logic. The test now asks for the direct
  display explicitly (`nested_compositor = "off"`).

## Left as it is, and why

* **The login-protocol proxy** (A-F10): not built; days of work, per protocol version.
* **Signals** from the confined launcher are not scoped: it must be able to stop the game,
  which is outside its Landlock domain. Listed as a residual (J18) and as an open design
  item in TODO.md (a stop channel instead of a signal would allow scoping them).
* **The texture gateway's buffering** and **B-M3** are in `src/egress/mod.rs`, which belongs
  to the egress engineer. README has one sentence about B-M3 that describes *this* tree and
  must go once that fix is merged (integration note).
* **Reviewer B's note under "found clean"** — the directories `cache/running`,
  `cache/scratch` and `vault` themselves are followed if they are symbolic links, which only
  an unconfined program running as the user can plant — is unchanged: that program can do
  what such a link would achieve directly.
* **`doctor` reports a display plan that `require` would refuse as a warning, not as a
  failure** (as it does for every refusal of the display plan). Visible, but it does not
  change `doctor`'s exit status. Unchanged.

## Final verification (third engineer, on the final tree)

Everything below was run on this machine after the last change to the code it concerns.
Every command had `DISPLAY=:77 XAUTHORITY=/dev/null` (the private display); launches used
the isolated data root `target/e2e` and demo mode.

| What | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets` | no warnings |
| `cargo test --workspace --no-fail-fast` | 373 passed, 0 failed (launcher 314: 229 unit, 85 integration; front end 53; protocol 6) |
| the launcher's tests with `BASTION_NET_TESTS=1 BASTION_X11_TESTS=1` | 314 passed, 0 failed |
| `tests/confine.rs` | 5/5, several runs |
| `tests/scram.rs` | 8/8 (the two-sessions test took the branch in which the other session's key survives) |
| Wayland suite against headless sway 1.12 (`BASTION_WAYLAND_TESTS=1`) | 2/2 — 57 interfaces for a trusted client, 37 on the game's socket, none of the 16 privileged ones, 20 withheld |
| nested suite against gamescope 3.17 (`BASTION_NESTED_TESTS=1 BASTION_NESTED_CLIPBOARD_TEST=1`) | 4/4 — with the cut-off nothing crosses in 4 of 4 directions; in the control run without it CLIPBOARD crosses both ways and that is noticed |
| `bastion-mc doctor`, X11 session and headless sway | lines as documented (self-confinement, limits read back, the listing of the game's socket) |
| demo launch, vanilla 26.3, through gamescope, `--events` | started, ran, exit status 130 on SIGINT; `Protection`: display, network, credential, limits, launcher all `on`, emitted after the confinement; `Seccomp: 2`, `Seccomp_filters: 3`, `NoNewPrivs: 1`; 5 connections relayed, 0 denied; `persist/options.txt` written back; nothing left in the cache |
| demo launch through a security context on headless sway | the same, window from SDL's Wayland driver |
| demo launch `--burner` | the same without network; "everything else the game wrote have been deleted"; `persist/` untouched |
| demo launch of the signed Fabric profile (`fab`) | the same; 49 mods loaded; its `options.txt` was moved into `persist/` (the migration of an existing instance, seen for real) |
| `cargo check` of the fuzz crate (`fuzz/`, offline) | compiles, all targets including `zipdir`; `fuzz/Cargo.lock` needed the `flate2` edge |

**Not verified here, and why:** `makepkg` and the package's `check()` (not run; `check()`
runs the unit tests, which pass); `cargo deny check` (no `cargo-deny` in this working copy;
`Cargo.lock` gained an edge and no crate); the GUI live test and the front end against the
changed `Protection` details and scram lines (the front end is another engineer's this
round); the libFuzzer targets (no nightly toolchain); anything with an account (sign-in,
the broker with a real token, `skin`/`cape`, the canary on a real profile, a signed-in
launch after confinement); a real `age` and device; the root-owned recipients file with
real root ownership; a kernel with Landlock ABI 8 or older; a user manager without a
delegated controller; a compositor that takes a security context and filters nothing
(stand-in only); `bastion-mc scram --yes` itself (the kernel keyring is the user's real
one: the tests go through `scram::run_for` with uniquely named accounts).

## Final verification (fourth engineer, on the final tree)

After the repairs above (`a62a00c`) and the document updates, on this machine, every
command with `DISPLAY=:77 XAUTHORITY=/dev/null`; logs in `target/a5-logs/*-e4*` (not
committed).

| What | Result |
|---|---|
| `cargo fmt --all` | no change |
| `cargo clippy --workspace --all-targets` | no warnings |
| `cargo test --workspace --no-fail-fast` | 375 passed, 0 failed (launcher 316: 229 unit, 87 integration; front end 53; protocol 6) |
| the launcher's tests with `BASTION_NET_TESTS=1 BASTION_X11_TESTS=1` | 316 passed, 0 failed |
| `tests/scram.rs` | 10/10 (with the two new tests; the two-sessions test again took the branch in which the other session's key survives) |
| `tests/confine.rs`, `tests/sandbox.rs` | 5/5, 18/18 (in the workspace run) |
| Wayland suite against headless sway (`BASTION_WAYLAND_TESTS=1`) | 2/2 — 37 interfaces on the game's socket, none of the 16 known privileged ones, 20 withheld |
| nested suite against gamescope 3.17 (`BASTION_NESTED_TESTS=1 BASTION_NESTED_CLIPBOARD_TEST=1`) | 4/4 — with the cut-off nothing crosses in 4 of 4 directions; without it CLIPBOARD crosses both ways and that is noticed |
| demo launch, vanilla 26.3, through gamescope | started, ran, exit status 130 on SIGINT; clipboard cut reported, limits read back, `launcher: confined itself` logged before the first line of the game; `Seccomp: 2`, `Seccomp_filters: 3`, `NoNewPrivs: 1`, 16 threads; 5 connections relayed, 0 denied, 5 requests forwarded by the broker; nothing left in the cache, no process left |
| the same launch: the descriptors of every process under the launcher | the JVM holds no descriptor of bubblewrap's status pipe (bubblewrap's two pipes, fd 6 and fd 85, appear nowhere else; the JVM's pipes are its standard output and error and one pipe of its own). So the one stream the launcher reads before it confines itself — `payload_pid` reads bubblewrap's `child-pid` line — is not one the game can write to (A-F6) |
