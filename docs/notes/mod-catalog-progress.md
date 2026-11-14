# Mod catalog (package A4) — hand-over / progress file

Kept current with every commit. A successor continues from this file alone; the notes for
the integrator and the README/threat-model text are in `mod-catalog.md` next to it.

Clone: `target/agents/a4-catalog`, branch `work`. Never touch the real repository or the
sibling clones. Machine rules: display `:77` with `XAUTHORITY=/dev/null` only, demo launches
only, the four XDG variables always set to `target/e2e/...`, one launch at a time.

## History of this package

1. First engineer (commits `9daa633` … `237d618`, 2026-11-14 01:06–01:39 UTC): built the
   whole package and wrote `mod-catalog.md`. Cut off by a usage limit, no hand-over file.
2. Integrator: `917d295` "sync:" (three files from the real tree; leave it alone). There is
   no "WIP checkpoint" commit in this clone: everything had been committed.
3. Second engineer (this file, from 2026-11-14 01:40 UTC): review, verification, fixes.

## Brief against what exists (state found at take-over, verified by reading and running)

| Brief item | State | Where / how verified |
|---|---|---|
| Modrinth client in a module of its own on `net::http`, two extra hosts for the authoring commands only | done | `src/profile/modrinth.rs` (`Live::new`); `…::the_registry_hosts_are_reachable_only_through_the_client_built_for_it` |
| Strict bounded JSON, `validate_file_name`, https on `cdn.modrinth.com`, size limits, SHA-512 verified while downloading, a mismatch never reaches `mods/` | done | `modrinth::tests::*` (15), `tests/catalog.rs` over real TLS |
| `profile add <id> <mod>…`: newest release for version+loader, shows project/version/date/file/size/hashes, required dependencies bounded, keeps suitable present builds, replaces older build, pre-releases and <48 h builds only with a flag, ends like `profile lock` with a `source`, signs nothing | done | `src/profile/catalog.rs`; `catalog::tests::*` (19) |
| Aliases viafabric, simple-voice-chat/voicechat, fabric-api, sodium, lithium, modmenu, by project id | done | `catalog::ALIASES`; live test prints the table |
| `profile check <id>` with the statement of what it does not prove | done | `catalog::check`, `CHECK_CAVEAT` |
| Capability report for added jars before signing | done | end of `catalog::add` (uses `capabilities::scan_mods`, `changes`) |
| `src/cli.rs` minimal | done | 7 help lines + 1 dispatch arm |
| Harness in `src/fuzzing.rs`, seeds + test in `tests/robustness.rs`, cargo-fuzz target | done | `("registry", modrinth::fuzz)`; `robustness::mod_registry_answers` |
| Opt-in live tests (`BASTION_NET_TESTS=1`) | exist | `tests/catalog.rs::live_*`; first engineer ran them once; **to re-run by me** |
| Real game: copy of `fab`, add viafabric + simple-voice-chat, lock, sign, launch on :77, read the log | done by the first engineer | instances `fabcat`, `fabrep` under `target/e2e/data/bastion-mc/instances/`; logs `target/e2e/state/bastion-mc/logs/fabcat/launch-1794619627.log` and `…/fabrep/launch-1794619873.log`. I read both logs: every statement of `mod-catalog.md` §5.2 that I checked is in them (54 mods, ViaFabric + ViaVersion, voicechat, Opus/RNNoise/Speex/LAME initialised, DENY `update.viaversion.com`, 5 relayed / 0 denied / 2 lookups refused; second launch: 53 mods, `lspci` EACCES, DENY `api.modrinth.com` and `meta.fabricmc.net`) |
| Notes file `docs/notes/mod-catalog.md` | done | 388 lines, all three parts |
| Hand-over file | **was missing** | this file |

State of the tree at take-over (run by me, 2026-11-14 01:45 UTC):
`cargo build --workspace --all-targets` ok; `cargo fmt --all -- --check` clean;
`cargo clippy --workspace --all-targets` no warnings; `cargo test --workspace` green
(launcher lib 220, `tests/catalog.rs` 8, `tests/robustness.rs` 11, GUI 51+1+1, proto 6, …).

## Plan for the rest (in this order)

1. [x] Review the code as a reviewer would (client, catalog, lock_with, tests); findings below.
2. [x] Live tests run once by me (2026-11-14 01:50 UTC): both pass; the alias table of
       `mod-catalog.md` §5.1 is what the registry says now (log: `target/a4-logs/live-1.log`).
3. [x] The real binary against Modrinth in a fresh copy of `fab` (`fabv2`): `profile add … --dry-run`,
       refusal without `--prerelease` (exit 5), `profile add viafabric voicechat --prerelease`,
       `profile check`, `profile lock`, `profile sign`, `profile verify` — all as the notes describe
       (output: `target/a4-logs/add-fabv2.out`).
4. [x] Findings F1–F4 fixed, one commit each, each with tests (see "Fixed" below).
5. [x] `mod-catalog.md` updated for everything that changed (README text, command reference,
       verification model, threat rows C9/C10/C12, open items, integrator notes, §9 on the review).
6. [x] Real-game launch with ViaFabric + Simple Voice Chat repeated (`fabv3`, 02:06 UTC): the log shows
       what the first engineer's launch 1 showed; written up as launch 3 in `mod-catalog.md` §5.2.
       (Binary of commit `7a1f600`; what came after touches `profile add` only — the file-date
       wording, name pre-validation, help text — and neither what gets installed nor the launch.)
7. [x] Final checks, see "State at hand-in" at the end of this file.

**Nothing is in progress. The package is complete; what is open is listed in `mod-catalog.md` §7.**

## Findings of the review (2026-11-14)

What holds (checked by reading the code and Modrinth's `openapi.yaml`, fetched 2026-11-14 into
`target/a4-logs/docs/`): the five routes and their parameters are the documented ones
(`GET /v2/project/{id|slug}`, `GET /v2/projects?ids=`, `GET /v2/project/{id}/version?loaders=&game_versions=&include_changelog=false`,
`GET /v2/version/{id}`, `POST /v2/version_files` with `{hashes, algorithm}`); user-agent rule, 300/min,
410 and "slugs can change" are quoted correctly; every string that reaches the terminal from the
registry is cleaned or validated; nothing touches `mods/`, the manifest or the signature before every
file is verified; `--events` refuses `profile`, so `println!` is in line with `profile lock`/`audit`.

To fix:

* **F1 — an added jar can be missing from the capability report.** `capabilities::scan_mods` files
  reports under an *identity* (mod id, else the file name up to its version); two jars with one
  identity leave one entry. `catalog::add` prints from that map, so a jar it just added is not shown
  when another jar sorts after it under the same identity (e.g. the new `alpha-1.0.jar` with mod id
  `alpha` next to a hand-copied `alpha-9.jar` that declares none). Fixed at the root, in `scan_mods`
  (so `profile lock` and `profile audit` no longer lose a jar either).
* **F2 — a present build that is not older can be replaced.** README text and module header say a jar
  is replaced only if it is an *older* build. For a dependency the code replaced whatever build of the
  project was present once it was "not suitable" — also a newer one (a dependent that names an exact,
  fresh or pre-release build led to a downgrade; a newer build listed for another Minecraft version was
  deleted under the words "an older build"). Fix: a present build that is not older than the chosen
  one always stays, with a note saying why; only older builds are replaced.
* **F3 — two places where an answer is not held to what was asked / to a bound while reading.**
  `Registry::projects` accepted projects it had not asked for (only their names are shown, but the
  module promises "a reply has to be about what was asked"); the hash→version map of a hash lookup was
  bounded only by the 32 MiB body limit while being read. Fix both.
* **F4 — the 48-hour rule goes by the date of the *build*, and a build's files can change later.**
  Modrinth's API lets a project member add a file to a published version (`POST /version/{id}/file`)
  and make it the primary one (`primary_file` in `PATCH /version/{id}`); the version keeps its
  `date_published` and the API states no date per file. So "published 30 days ago" does not say the
  *file* is 30 days old, and a taken-over account gets past the quarantine at once. The file server
  does state a date per file: `Last-Modified` on `cdn.modrinth.com`. Fix: `profile add` reads it
  with the download and refuses bytes the file server has held for less than 48 hours; where the
  server states none, it says so.
  Residual to document: a file put into a build as a secondary file and made primary two days later,
  and (not verifiable without a project of one's own) a build prepared as a draft or scheduled.
  **Learned the hard way, by comparing more real files after the first version was committed:**
  `Last-Modified` is when the file server last *wrote* its copy. 10 of 15 files: 0.3–3 s after
  `date_published`. The other 5 (Mod Menu 21.0.0 and its beta.1, Text Placeholder API 3.2.0, Lithium
  0.26.1/0.26.2 — builds of September) are all dated 2026-10-19 10:15–10:17 UTC: a storage rewrite.
  The first version printed "the file was put into the build after its publication" for every late
  date; that was false for these and was taken out (`2174e11`). A late date that is not recent has no
  consequence and no remark; a recent one is refused with both readings named.
* **F5 — notes.** `mod-catalog.md` must say the above (C9 residual, §3), and that `profile check`
  goes by the build's date only.

Fixed (all committed; `cargo clippy --workspace --all-targets` silent, `cargo test -p bastion-mc` green after each):

| Finding | Commit | Tests that hold it |
|---|---|---|
| F1 | `6c0447a` (`capabilities::scan_mods`: the later jar of two with one identity is filed under identity + file name) | `capabilities::tests::two_jars_that_go_by_one_identity_are_both_reported`, `catalog::tests::what_was_added_is_scanned_and_shown_even_next_to_a_jar_that_goes_by_the_same_name` (failed before the fix) |
| F2 | `15f3084` (`catalog::resolve`; `Replaced { file, older }`; `Choice { skipped, remarks }`) | `…::a_build_that_is_not_older_is_never_replaced_by_an_older_one` (fails on the old condition: mutation-checked), `…::a_dependency_built_for_another_minecraft_version_is_replaced_and_the_output_says_why` |
| F3 | `1f6d046` (`modrinth::Lookup` visitor; `Registry::projects`) | `modrinth::tests::a_hash_lookup_is_accepted_only_for_what_was_asked` (flood given up at column 131), `…::the_routes_ask_what_the_documentation_says…`; harness 300 000 × 2 seeds |
| F4 | `f2495d4`, `20ae968`, reworked in `2174e11` (`Answer::last_modified`, `Downloaded::stored`, `catalog::file_date`, `LATE_FILE_SECS`) | `catalog::tests::bytes_the_file_server_has_held_for_less_than_48_hours_are_refused_whatever_the_age_of_the_build`, the refusal matrix (28 cases now), `tests/catalog.rs::a_file_the_file_server_has_held_for_less_than_48_hours_is_refused_over_tls`, live test asserts Modrinth dates its files; a real `profile add` of Mod Menu (files re-dated on 2026-10-19) goes through without a remark (`target/a4-logs/add-fabv4.out`) |
| — | `1681572` (names checked before the first request; help text names the flags), `9fc5a23` (docs: a build's dependency list can be edited after publication) | `catalog::tests::an_alias_names_its_project_by_id` (last block) |
| — | `96efad6` (a robustness harness `jar` for the capability scan, which `profile add` now runs on registry bytes; nine hostile archives probed by hand — nothing had to change in the scanner) | `robustness::mod_jars_and_class_files` (300 000 × 2 seeds), `robustness::the_jar_starting_points_are_scanned_in_full` |

Design decisions taken with F4, for whoever reads the code later: the file's date matters only when
it is more than an hour after the build's (`LATE_FILE_SECS`) — otherwise a build that has just left
quarantine would be refused for the seconds its upload took; a file server that states no date does
not stop the command (the documented rule, the build's date, still holds) but is said in the output;
`profile check` and `--dry-run` do not download and therefore go by the build's date only, and say so;
after a storage rewrite at Modrinth the rule refuses old files for two days (fail closed, the message
says both readings, `--allow-fresh` overrides — and then also takes fresh builds, which the message
says as well). If that turns out to happen often, the alternative is a warning instead of a refusal.

Looked at and left alone (reasons): `MAX_REQUESTS` (96) can be reached by a command that legitimately
involves the full 24 projects with exact-build pins — the error says so and the bound is the point;
a replaced jar is deleted, not kept aside (already an open item in the notes); no lock against two
`profile add` on one instance at a time (no authoring command has one).

## Things worth knowing about this code and this machine

* `profile` commands are refused in `--events` mode by `cli::dispatch`, so they print with
  `println!` like `profile lock`/`audit` do; `catalog::Output` keeps the lines for tests.
* The e2e configuration (`target/e2e/config/bastion-mc/config.toml`) has `audio = "none"`,
  `allow_x11 = true`, `java.match = "newer"`, filtered network. Launches go through the
  system's gamescope (the log shows `[gamescope] … version 3.17.0`).
* Test logs of mine are under `target/a4-logs/` (not tracked): `final-1.log` (fmt, clippy, workspace
  tests), `final-net.log` (with `BASTION_NET_TESTS=1`), `live-*.log`, `add-fab*.out`, `check-fabv3.out`,
  `launch-fabv3.out`, `fuzz-check.log`, `docs/openapi.yaml` (Modrinth's API description as fetched),
  `scripts/launch-once.sh` (one bounded demo launch on :77, SIGINT 25 s after the title screen).
* **The shell this job runs in has `DISPLAY=:0` and `XAUTHORITY=~/.Xauthority` in its environment.**
  Anything that could open a display must set `DISPLAY=:77 XAUTHORITY=/dev/null` itself (the launch
  script does; I also set both for every `cargo test`). The default test suites open no display
  (the X11, nested and GUI-live tests are opt-in).
* In a shell command, never put Markdown backticks inside double quotes (bash runs them).
* Instances in the e2e data root: `fab`, `vanilla-latest` (given); `fabcat`, `fabrep` (first engineer's
  launches 1 and 2); `fabv3` (my launch 3, signed, serial 3). Their launch logs are under
  `target/e2e/state/bastion-mc/logs/<instance>/`.
* Modrinth's file server (Fastly in front of object storage) sends `Last-Modified`, and `Date` is the
  current time even for cached answers (`Age` says how long cached). The API (Cloudflare) answers are
  not cached (`cf-cache-status: DYNAMIC`), so its `Date` is a usable clock.

* A test of the baseline is racy on a loaded machine:
  `sandbox::resource_limits_kill_a_memory_hog_inside_the_scope` (systemd stops a scope after a kernel
  OOM kill — `DefaultOOMPolicy=stop` — and its SIGTERM can reach bubblewrap before bubblewrap exits
  with 137). Root cause, journal evidence and a launcher-free reproduction are in `mod-catalog.md` §8.
  Not mine to fix (`tests/sandbox.rs`, `src/sandbox/cgroup.rs`); if it fails, run it again.
* `/usr/bin/time` does not exist here; `target/a4-logs/scripts/measure.py` measures wall time and peak
  RSS of a command under an address-space limit.

## State at hand-in (2026-11-14, about 02:50 UTC)

Everything is committed on `work`; the working tree is clean. Run on the last code commit:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets` | no warnings |
| `cargo test --workspace` (with `DISPLAY=:77 XAUTHORITY=/dev/null`) | 370 passed, 0 failed (launcher lib 225, `tests/catalog.rs` 9, `tests/robustness.rs` 13, …; GUI 53, proto 6) — `target/a4-logs/final-2.log` |
| `BASTION_NET_TESTS=1 cargo test --workspace` | 370 passed, 0 failed, nothing skipped for the flag — `final-net-3.log`. The run before it (`final-net-2.log`, launcher only) had the one racy sandbox failure described above |
| `BASTION_NET_TESTS=1 cargo test --test catalog live_ -- --nocapture` | both live tests pass; alias table as in `mod-catalog.md` §5.1 — `live-final.log` |
| `BASTION_FUZZ_ITERATIONS=300000 cargo test --test robustness mod_registry_answers` / `mod_jars_and_class_files`, default seed and `BASTION_FUZZ_SEED=42` | pass |
| `cd fuzz && CARGO_TARGET_DIR=../target/fuzz-check cargo check --offline` | compiles (nine targets); never run (no nightly toolchain) |
| Real registry, real binary: `profile add` (dry run, refusal without `--prerelease`, ViaFabric + Simple Voice Chat, Mod Menu with its dependency), `profile check`, `lock`, `sign`, `verify` | as described in `mod-catalog.md` |
| Real game: `launch fabv3 --demo` on `:77` | both mods and their native libraries load; ViaVersion's update check refused by the relay; exit 130 on SIGINT (`mod-catalog.md` §5.2, launch 3) |

Not verified, and open: `mod-catalog.md` §7. Nothing is half done.
