# Round 5, package "risk": hand-over log

Clone: `target/agents/r5-risk`, branch `work`, base `3f78f31` (the round-5 contract).
Deliverable notes: `docs/notes/bedrock-risk.md`. User guide: `docs/BEDROCK.md`.

## State (2026-11-14): done

- [x] Read the contract, the research (§0, 2.1, 3, 4, 5), `src/bedrock/`, scram, running, canary,
      doctor, the runbook and its link list, the threat model's table style; the client's and
      the UI's sources for the facts the guide states (listed in the notes, §3).
- [x] `running.rs`: a marker of its own for a Bedrock session (`bedrock:<instance>`,
      `Marker::bedrock`, `Activity::Bedrock`, `Activity::is_launch`), counted as a launch by the
      leftover sweep; `in_use` is `pub(crate)` for the session leftovers.
- [x] `bedrock::policy`: gate text; doctor lines; `SessionDir` (the per-session data
      directory, locked, removed bounded and on drop); `remove_leftovers` (locked ones left
      alone; a base that is a link or open to others refused, nothing deleted); `set_up`;
      `after_session` / `report_after_session` (capes and the canary).
- [x] `scram.rs`: Bedrock sessions stopped with their own wording; their leftovers deleted
      after the last stop pass, bounded, confirmed by reading again; running sessions holding
      one reported (unstoppable or unmarked) with "sign out everywhere"; worlds kept and said;
      "no launch was in progress" no longer printed next to an unmarked launch (Java too).
- [x] canary: `Snapshot::gained`, `canary::gained`; `launch::canary_verdict` names gained capes
      and the command that wears one (one block in the shared `launch.rs`).
- [x] `doctor.rs`: a `bedrock` section (one block). `cli.rs`: the scram confirmation names the
      Bedrock leftovers and that worlds are kept (one string).
- [x] Tests: `bedrock::policy` (9 + a child half), `running` (+1), `auth::canary` (extended),
      `launch::canary_tests` (+1), `tests/scram.rs` (+4, 2 extended).
- [x] `docs/BEDROCK.md`, `docs/notes/bedrock-risk.md` (README text, threat-model rows B1–B13,
      runbook text, verification, re-check list, contract changes, open items).
- [x] fmt, clippy (0 warnings), the whole workspace's tests, cargo deny; committed.

## Final verification (2026-11-14, on the last commit)

Environment: `DBUS_SESSION_BUS_ADDRESS` and `WAYLAND_DISPLAY` unset, `DISPLAY=:77`,
`XAUTHORITY=/dev/null`, the four XDG variables under `target/xdg/`, `TMPDIR` at its default.

- `cargo fmt --all` (changed formatting only, committed), `cargo clippy --workspace
  --all-targets`: no warning.
- `cargo test --workspace --no-fail-fast`: exit 0. The launcher 651 tests in 28 binaries (470
  unit, `tests/scram.rs` 15), the front end 126, the protocol 7; 0 failed.
- `/home/ski/RustroverProjects/bastion-mc/target/tools/bin/cargo-deny deny --offline --locked
  check`: "advisories ok, bans ok, licenses ok, sources ok" (one warning about an unused
  licence allowance in `deny.toml`, there before this round). No dependency was added.
- `bastion-mc doctor` and `echo no | bastion-mc scram` in isolated directories (notes §3, E33).

**Observed on the way, not caused by this package, reported:** with `TMPDIR` pointed at a
directory under this clone (`target/tmp`, on btrfs), 18 unit tests and 5 of `tests/companion.rs`
failed: the companion, pulse and wayland tests bind UNIX sockets under `temp_dir()`, and the
clone's path makes those longer than the 108 bytes a socket path may have ("path must be shorter
than SUN_LEN"); `profile::mods::tests::every_kind_of_deviation_is_refused` and
`profile::packs::tests::import_verify_remove` rewrite a file that `verify_mods_dir` /
`packs::import` sealed with fs-verity, which btrfs supports (EPERM). With `TMPDIR` on tmpfs
(`/tmp`, the default) all pass. The suite assumes a short `temp_dir()` on a file system without
fs-verity; worth a sentence in the README's Tests section.

## Decisions

- The token store of the game's sign-in library is the root of the client's data directory
  (`XboxLocalStorage.getPath` → `PathHelper::getPrimaryDataDirectory()`, client
  `src/jni/xbox_live.cpp`). So a session's data directory itself is per-session scratch, and
  scram treats the whole per-session directory as a credential store.
- Per-session directories live under `$XDG_CACHE_HOME/bastion-mc/bedrock-sessions/` (not under
  `scratch/`, whose sweep is the last, bounded step of scram and says "not the launcher's
  credentials"). The launch package should create them with `bedrock::policy::SessionDir`; if
  it chose another place, the integrator points `sessions_dir` there.
- Worlds and the game's settings are kept by scram (not credentials by design) and scram says so;
  deleting them is the user's choice (the instance directory). No scram option for it.
- No new configuration key, event, dependency or fuzz harness (no new parser of untrusted
  bytes: the marker's `bedrock:` form goes through the existing marker parser).

## Next (for the integrator)

The wiring in the notes, §4 (the launch package's Bedrock launch: gate, `Marker::bedrock`,
`remove_leftovers`, `SessionDir`, the cache in its `Confinement`, `report_after_session`), the
re-check list, §5, and folding the README, THREAT_MODEL and runbook texts in once the website
refresh is done.
