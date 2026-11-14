# Findings of the two independent security reviews (2026-11-13)

Two reviewers read the code fresh, read-only, without building or running anything. Line
numbers are from before a `cargo fmt` run and may be off by a few lines; find the code by
the quoted fragments. Neither reviewer found a way for the game to obtain the real token or
to get the credential attached to a request outside the broker's table. No critical or high
findings. Each finding below still has to be **verified** before it is fixed.

Severity as given by the reviewer. "Owner" says who fixes it in this round:
**fixes** = the review-fix engineer (this file's reader), **gui** = the engineer redesigning
`gui/`, **egress** = the engineer working in `src/egress/` on the UDP relay.

---------------------------------------------------------------------------------------------

## Review A — broker.rs, wayland.rs, nested.rs, confine.rs, persist.rs, cgroup.rs

### A-F1 (medium) — the confined launcher can still read the refresh token from the kernel keyring. Owner: fixes
- Where: `src/confine.rs` (`apply`, the ruleset), call site in `src/launch.rs` (the `confine::apply(&keep)` block), token location `src/auth/store.rs` (`keyutils_load`).
- Defect: self-confinement is Landlock only (files, TCP ports, abstract-socket scope). Landlock does not mediate `keyctl`. The default token store keeps the Microsoft refresh token in the kernel keyring.
- Scenario: a bug in the relay, broker or log handling is exploited from the game after confinement. The attacker code does what `keyutils_load()` does (keyctl search + read of `bastion-mc:msa-refresh:<account>`) and sends the token out: the TCP rules are per port, not per destination, and UDP is unhandled. The long-lived credential leaves, not only the 24-hour token. The comment "from here on the launcher holds no refresh token" is true of memory, not of reach. With `token_store = "age"` this does not apply.
- Related (plausible, low): inherited stdio keeps working; if the launcher was started from a terminal, compromised code can write raw escape sequences to it, or use TIOCSTI where `dev.tty.legacy_tiocsti=1`.
- Fix direction: a seccomp filter on all threads (TSYNC) at confinement time denying `keyctl` / `add_key` / `request_key`.

### A-F2 (medium on Landlock-ABI-8 kernels, none on 9+) — pathname-socket restriction silently dropped, confinement still reported complete. Owner: fixes
- Where: `src/confine.rs` (`MIN_ABI = 8`, `TARGET_ABI = V9`, `BestEffort`, the final status check), messages in `src/launch.rs` and `src/doctor.rs`.
- Defect: `supported()` accepts ABI 8, but the only thing stopping a connection to D-Bus or systemd's private socket is `ResolveUnix` (ABI 9). It is requested under `BestEffort`, and a `PartiallyEnforced` ruleset is accepted.
- Scenario: kernel with thread-sync but without `RESOLVE_UNIX`. The log says "confined itself … no program execution", the front end shows the launcher shield as on, `launcher.confine = "require"` passes. A compromised launcher connects to `$XDG_RUNTIME_DIR/bus` or `systemd/private` (pathname sockets; abstract scoping is irrelevant), starts a transient unit: an arbitrary program runs outside the domain.
- Fix direction: require ABI 9 for `require`; report the partial state in `auto`.

### A-F3 (low) — the write grant covers `instance.toml`, so a compromised launcher can weaken the next launch. Owner: fixes
- Where: `src/launch.rs` (`keep.write.push(inst.dir.clone()); // settings carried back at exit`), `src/config.rs` (`InstanceConfig`), where `inst.config.network.mode` is read.
- Defect: write access is granted to the whole instance directory, which also holds the instance's security configuration; also in burner sessions, where nothing is carried back.
- Scenario: post-confinement attacker code rewrites `instances/<id>/instance.toml` with `[network] mode = "open"` or adds `servers = ["attacker.example"]`. On the next launch the instance setting wins: the real token is handed to the JVM, or the game gains an exfiltration endpoint. Only a log line shows it.
- Fix direction: carry persisted files through a dedicated sub-directory and grant only that; skip the grant in burner mode.

### A-F4 (low) — the Wayland event reader has no overall bound, and its volume guard is dead code. Owner: fixes
- Where: `src/sandbox/wayland.rs` (`Connection::pump`, `Connection::open`).
- Defect: `pump` loops for as long as the peer sends at least one byte every 5 seconds. The "more than a setup connection should receive" check can never fire: complete messages are drained before every read, so the buffer never exceeds about 8 KiB against a threshold of 256 KiB. `connect()` happens before any timeout is set.
- Scenario: the peer at `$WAYLAND_DISPLAY` answers `get_registry` with an endless stream of 8-byte events for an unknown object. `globals()` and `secure()` never return; `launch` and `doctor` hang before the game exists.
- Fix direction: an overall deadline plus an event budget.

### A-F5 (low) — the broker buffers up to about 2 GiB of replies in the launcher, outside the game's memory scope. Owner: fixes
- Where: `src/egress/broker.rs` (reply handling: `let mut payload = Vec::new(); … connection.call(…, MAX_RESPONSE_BODY, &mut payload)` … `Full::new(Bytes::from(payload))`), `MAX_CONNECTIONS` in `src/egress/mod.rs`.
- Defect: every reply is fully buffered (8 MiB cap) and held until the game reads it; 256 connection slots, each living up to 30 minutes, no aggregate budget.
- Scenario: a hostile mod opens 256 connections to a brokered name (any sub-domain of mojang.com resolves), sends anonymous GETs for objects just under 8 MiB and never reads the replies: roughly 2 GiB resident in the launcher, not charged to the game's `MemoryMax`; with `mlockall(MCL_FUTURE)` that memory is locked or allocation fails and the launcher aborts.
- Fix direction: stream anonymous replies (no token was sent, so no leak check is needed), or add a global byte budget.

### A-F6 (low) — the "launcher" protection is announced before `apply()` runs, and the relay starts before it. Owner: fixes
- Where: `src/launch.rs` (`let confined = cfg.launcher.confine != ConfineMode::Off && confine::supported().is_ok();` in the `Protection` event; `Err(e) => log.line(…"launcher: not confined — {e}")`; the order of `egress::start`, the log relays and `confine::apply`).
- Scenario: in `auto` mode, if `apply()` fails for a reason other than the ABI (rule add, `restrict_self`), the session continues unconfined while the front end keeps showing "confines itself while the game runs". Separately, `egress::start` and the log relays are spawned before `apply()`, so game input is handled unconfined for a short window.

### A-F7 (low, plausible) — the cgroup preflight proves a scope can be created, not that the limits are enforced. Owner: fixes
- Where: `src/sandbox/cgroup.rs` (`preflight`: `if output.status.success() { Ok(program) }`), consumers in `src/launch.rs`.
- Defect: success means only that `systemd-run … /usr/bin/true` exited 0. The comment claims it catches "controllers not delegated". The probe never includes `CPUQuota`.
- Scenario: a user manager lacks a delegated controller; systemd accepts the properties and enforces nothing. The launcher prints "limits: … (systemd scope)", shows the limits shield as on, `cgroup = "require"` passes, while a mod can exhaust host memory. (Depends on systemd ignoring unavailable controllers without an error; not run.)
- Fix direction: read the scope's `cgroup.controllers` and `memory.max` after start.

### A-F8 (low, plausible) — a "secured" Wayland socket is never checked for what it actually offers. Owner: fixes
- Where: `src/sandbox/wayland.rs` (`secure`), `src/launch.rs` (display protection text).
- Defect: success is one round trip without a protocol error. The launcher then asserts "capture and input injection withheld". `wayland::globals()` exists but is never pointed at the new socket.
- Scenario: a compositor implements the protocol object but applies no global filter (filtering is "should" in the protocol). The game binds screencopy or virtual-keyboard globals through its context socket while the display shield reads as on.
- Fix direction: list globals on the new socket; refuse or warn if the manager or known privileged globals are still there.

### A-F9 (low) — JSON re-encoding leaves DEL, C1 controls and bidi overrides raw in strings and keys. Owner: fixes
- Where: `src/persist.rs` (`json`: `serde_json::to_vec(&value)`), contrast `printable()` in the same file.
- Scenario: the user persists a `.json` file; the game writes a string containing U+009B. serde_json escapes only bytes below 0x20, `"` and `\`. The sequence lands in the instance directory — the "terminal escape sequences waiting for the next `cat`" case from the module header.

### A-F10 (informational) — join residual is server-agnostic and unlimited in rate; threat-model wording understates it. Owner: fixes (documentation)
- Where: `src/egress/broker.rs` (`JoinGate`, `join_body`), `docs/THREAT_MODEL.md` row J13.
- Finding: a join is single-use per ticket, but a ticket is any TCP connection to any allowlisted server and `serverId` is only shape-checked. A mod that learns a server hash (e.g. by chat on an allowlisted public server) can authorise a login as the user on an arbitrary third-party server, once per connection it opens. The module header says this accurately; the threat-model row ("appear on that server as you while you are connecting to it") understates it.

### A-adjacent (low) — the launch log's size budget is shared by all sources. Owner: fixes
- Where: `src/launch.rs` (`Log::line`, `MAX_LOG_BYTES`).
- A game that prints the whole budget first keeps later broker `[SECURITY]`, egress `DENY` and tripwire lines out of the log file (they are still shown live).

### Review A, checked and found clean
broker credential attachment (exact host/method/path, no query, placeholder required; host from the connection's DNS name, SNI must match; requests re-serialised with six allowlisted headers; no redirects; join body rebuilt); one caveat: the token-leak check on replies is a literal match while `Content-Encoding` is relayed — an encoded echo would pass (hypothetical; no endpoint echoes). wayland.rs wire format, bounds, descriptors. nested.rs (game gets only the inner socket; mount order; env; ready line consumed once). persist.rs names, `O_NOFOLLOW`, size caps, NBT bounds. cgroup.rs arguments and environment. Minor: confine.rs grants the parent directory of the resolv.conf target (broad if that target sits directly in `/run`).

---------------------------------------------------------------------------------------------

## Review B — scram, running, canary, minecraft (profile/PNG/multipart), registry, activity, store, proofs, packs, mods, http (SOCKS5), secret, landlock (UDP), noclip, CLI, GUI

### B-M1 (medium) — `scram` deletes leftover game directories before it removes any credential, and the GUI kills it after 60 s. Owner: fixes (launcher order), gui (timeout)
- Where: `src/scram.rs` (`run_for`: `running::sweep_leftovers(paths, Duration::ZERO)` runs before "// 2. Refresh tokens"), `src/running.rs` (`sweep_leftovers`), `gui/src/backend.rs` (`scram`: `tokio::time::timeout(Duration::from_secs(60), … .kill_on_drop(true).output())`).
- Scenario: a hostile game fills its scratch root with millions of small files. The user presses Scram. The launcher is killed, then the sweep grinds through `remove_dir_all`. At 60 s the GUI drops the future, `kill_on_drop` kills scram, and no keyring key or vault file has been touched.
- Fix: remove tokens first; sweep last and time-boxed.

### B-M2 (medium) — the GUI reports "Scram done … removed" when the launcher reported an incomplete scram. Owner: gui (the launcher's contract stays: exit status 1 and "COULD NOT …" lines when incomplete)
- Where: `gui/src/backend.rs` (`scram` ignores `output.status` unless there are no lines), `src/cli.rs` (`return Ok(if report.incomplete { 1 } else { 0 })`), `gui/src/app.rs` (`ScramFinished`).

### B-M3 (medium) — with `network.launcher_proxy` set, pass-through Mojang connections are resolved locally and dialled directly. Owner: egress
- Where: `src/egress/mod.rs` (`relay_service` → `connect_out(name, 443, false)` → `tokio::net::lookup_host` + direct `TcpStream::connect`), message in `src/launch.rs`, doc in `src/config.rs`.
- Only brokered hosts and the texture gateway use the proxied `HttpClient`. Any `*.minecraft.net:443` connection the game makes leaks the name to the local resolver and the real IP to Mojang; with `broker.enabled = false` that covers every Mojang API call carrying the real token. The launch message names only "multiplayer servers" as the exception.

### B-M4 (medium) — the "hardware-backed" age vault trusts an unchecked, user-writable recipients file. Owner: fixes
- Where: `src/auth/store.rs` (header claim; `AgeVault::new` checks only `program`; `save` uses `--recipients-file <cfg path>`), `src/launch.rs` (re-save of the rotated refresh token at every launch), `src/doctor.rs` ("the stored token cannot be copied and used elsewhere").
- Scenario: a same-user program appends its own `age1…` recipient. At the next launch the user touches the YubiKey as usual, the new token is sealed to the attacker as well, and the attacker decrypts `vault/<account>.age` offline.
- Caveat: THREAT_MODEL §5.1 concedes that same-user software can edit `config.toml`, but `store.rs`, the README and `doctor` claim this store resists it.

### B-L1 (low) — canary in `warn` mode is silent in several paths and re-pins the foreign change. Owner: fixes
- Where: `src/cli.rs` (`profile_admin`: the `Ok(note)` of `canary_verdict` is discarded; later `canary::accept(paths, &account, &after)`), `src/launch.rs` (`canary_verdict`; the note with `launch --quiet`; the note is said before the launch log file is opened).
- Scenario: `auth.canary = "warn"`, a thief renames the player. The user runs `bastion-mc skin set x.png`: no warning, the thief's state becomes the pin, later launches say "unchanged". A damaged pin file is replaced the same way. `config.rs` promises "Say so, launch anyway, keep the old pin". In enforce mode the wholesale `accept(after)` pins any third-party change landing between the two profile fetches (a seconds-wide race). The "pinned for the first time" note is also dropped in these commands.

### B-L2 (low) — `scram` does not stop in-flight commands that will store a token. Owner: fixes (launcher), gui (stop the running sign-in job when scram is confirmed)
- Where: `src/running.rs` (only launches have markers), `src/cli.rs` (`login`; `open_session` → `store.save`), `gui/src/app.rs` (`ScramFinished` hides the prompt, the job keeps running).
- Scenario: a device-code or browser sign-in is pending (up to 15 minutes). The user scrams and is told everything is removed. The sign-in completes and `store.save` writes a fresh refresh token. A concurrent `vault reseal`, `skin` or `cape` command has the same effect in a shorter window.

### B-L3 (low) — `scram` has silent gaps in what it removes. Owner: fixes
- Where: `src/scram.rs`.
- A label discovered in `/proc/keys` whose key is not reachable from the caller's session keyring yields `Ok(false) => {}`: no line, no `incomplete`. An unreadable `/proc/keys` or vault directory is also silent. Deletion is gated on a successful read (`has()`), and nothing re-reads `/proc/keys` afterwards.
- Scenario: an older key object survives only in another login session's keyring after a login in a second session displaced the old link in the persistent keyring. Scram from the second session invalidates the new key, exits 0, and the older token stays readable in the first session. (Keyring topology: plausible, not run.)
- Minor: vault file names are printed unsanitised.

### B-L4 (low) — scratch cleanup can be defeated by the game and is reported as done. Owner: fixes
- Where: `src/launch.rs` (`impl Drop for Scratch`: `let _ = fs::remove_dir_all(&self.root);`; the burner message "everything else the game wrote are deleted now"), `src/running.rs` (`sweep_leftovers`).
- Seccomp allows `chmod`, so the game can leave a mode-000 or 0500 directory and removal aborts with EACCES. A burner session then claims deletion while the tree (including world copies) stays in `~/.cache/bastion-mc/scratch/`; later sweeps fail silently every time.

### B-L5 (low) — a launcher is recognised only if its `comm` equals that of the process looking. Owner: fixes
- Where: `src/running.rs` (`live`: `comm == own`).
- Scenario: the launcher was started through a symlink or a renamed binary and scram runs as `bastion-mc`. The marker is pruned as stale, scram prints "no launch was in progress", the game and the in-memory session token survive. The sweep then deletes the running game's root; a second launch does the same after 60 s.

### B-L6 (low) — kill race, and "killed" is reported unconditionally. Owner: fixes
- Where: `src/running.rs` (`kill_all`), `src/scram.rs`.
- Check-then-`kill(pid)` window (needs pid wrap-around; `pidfd` would close it). `kill_all` returns the listed launches whatever the re-check or `kill()` result, so scram prints "killed … its session token is gone" unverified.

### B-L7 (low) — the pack check sees only the declared entry count; the game sees the whole central directory. Owner: fixes
- Where: `src/profile/packs.rs` (`check_archive`); zip crate 9.5.1 reads `number_of_files` headers without checking that they span `directory_size`.
- Scenario: an archive whose end record says 1 entry (`pack.mcmeta`) but whose directory holds more (`../` names, symlink entries, more than 200,000 entries) passes import; OpenJDK's `ZipFile` recounts headers when the total is wrong and exposes all of them (Java side from memory: plausible). Duplicate names also collapse in the crate's map; `has_duplicate_names()` is unused. Impact: bypass of the name, link and count screening only; bytes stay hash-locked and vanilla never extracts packs.

### B-L8 (low) — import parses the whole central directory before the entry limit applies. Owner: fixes
- Where: `src/profile/packs.rs` (`zip::ZipArchive::new` before `archive.len() > MAX_ENTRIES`).
- A zip64 file under the 2 GiB cap with tens of millions of 46-byte headers makes `ZipArchive::new` allocate many gigabytes; `pack import` is OOM-killed.

### B-L9 (low) — pack import/adopt validate one read and record another. Owner: fixes
- Where: `src/profile/packs.rs` (`import`: `check_archive(&file, kind)` and `hash(&file)` are separate passes over the source, the staged copy is compared only to the hash; `adopt` hashes through the directory fd and then re-opens by path for the check).
- Scenario: the user imports `saves/<world>/resources.zip` while the game, which can write there, swaps the content between the two passes.

### B-L10 (low) — the PNG sanitiser passes arbitrary non-image bytes. Owner: fixes
- Where: `src/auth/minecraft.rs` (`sanitize_skin_png`).
- `tRNS` has no once-only, colour-type or length rule; `PLTE` is accepted for RGB/RGBA; `IDAT` is not decoded (documented). An RGBA file with about 90 `tRNS` chunks of 256 arbitrary bytes, or a zip embedded in `IDAT`, is uploaded unchanged, despite "nothing else that was in the original leaves the machine". No panics: slicing, CRC, ordering and unknown-critical/APNG handling are correct.

### B-L11 (low) — `copy_tree` is unbounded and follows a swapped directory. Owner: fixes
- Where: `src/launch.rs` (`copy_tree`).
- No byte or file cap: a sparse multi-terabyte file left in `saves/` by an earlier session fills the disk on the next `--burner` launch (on file systems without reflink). `read_dir(src)` follows a symlink swapped in after the `file_type()` check (the game cannot create symlinks; needs a same-user program). Leaf files are opened `O_NOFOLLOW` + regular-file check.

### B-L12 (low) — `identity_kind` over-reports hardware backing. Owner: fixes
- Where: `src/auth/store.rs` (`identity_kind`), `src/doctor.rs`.
- Returns `Plugin` at the first `AGE-PLUGIN-` line even if an `AGE-SECRET-KEY-` is in the same file, and treats every plugin name as hardware.

### B-L13 (low, unverified) — the clipboard shim fails open with no self-check. Owner: fixes
- Where: `native/noclip.c`, `src/sandbox/nested.rs` (`NOCLIP`, `LD_PRELOAD`), display protection text in `src/launch.rs`.
- Only the six SDL text functions are interposed. A gamescope build using SDL3's data API (`SDL_SetClipboardData` and friends), static SDL, or a preload that ld.so silently skips leaves the bridge open while the launcher still reports "clipboard cut". (gamescope 3.17 from the Arch package was verified with `tests/nested.rs`; the concern is other builds and silent failure.)

### Review B, checked and found clean
`running.rs` parsing of `comm` with spaces/parentheses and field 22; pid edge cases cannot reach `kill`; per-entry deletes do not follow links (the roots `cache/running`, `cache/scratch`, `vault` are followed if they are symlinks, plantable only by an unconfined same-user program). `canary.rs` (damaged, foreign-mode or symlinked pin is an error; a deleted file gives first-sight auto-accept, as documented). `minecraft.rs` requests (profile parsing strict and bounded; cape JSON hex-only; multipart boundary 128-bit random with a constant filename). `registry.rs`, `activity.rs`, `proofs.rs`. `store.rs` plumbing. `mods.rs` verification (dir fd, `openat` + `O_NOFOLLOW`, hashes the descriptor that is mounted). SOCKS5 client (exact reads, no local-resolution fallback, allowlist before dialling, TLS and pins unchanged). `secret.rs` (fd always closed; falls back if `MADV_DONTFORK` fails; wiped on drop for both backings). `restrict_udp` (struct sizes and bit values match the kernel header; fails closed). CLI argument handling. GUI (runbook links exact-match; `ConfirmScram` only after `AskScram`; `xdg-open` by absolute path with one argument).
