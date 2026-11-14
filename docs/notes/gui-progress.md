# GUI package — hand-over file

Kept current with every commit. A successor continues from this file alone.
Package brief: the "workshop HUD" front end (`gui/`), plus the GUI halves of review
findings B-M2, B-M1 and B-L2 (`docs/notes/review-findings.md`).

## 1. Where the work stood at the takeover (commit 372e10c), checked by building and running

The first engineer had got much further than the checkpoint message suggested.

| Brief item | State found | How checked |
|---|---|---|
| Rail + stage, five pages (Launch, Accounts, Instances, Console, Security) | all five exist and render | `cargo test -p bastion-mc-gui`: 94 unit tests green; looked at the snapshots |
| Account summaries kept (`AccountEntry.summary`), cards without extra launcher calls | done, tested in `app::account_tests` and `view::tests` | tests green |
| State machine rules (one job, Message-only, pure view, `unsafe` forbidden) | kept | read `app.rs`, `view/`, `gui/Cargo.toml` |
| Scram: incomplete result (B-M2), no kill (B-M1), sign-in stopped first (B-L2) | implemented in `app.rs` / `backend.rs`; tested at the `App` level only | tests green; **no test runs `backend::scram` against a stand-in launcher** |
| Theme, drawn elements (`hud.rs`: frames, gauge, shield marks, rail glyphs) | done, with contrast and motion tests | tests green |
| Tick only while something animates | done (`App::tick_interval`), tested | tests green |
| View tests ported and extended (navigation, accounts, scram, shields, link allowlists, layout at three sizes, 30 states x 2 sizes of snapshots) | done in the WIP checkpoint | tests green |
| `gui/tests/emulated.rs` (whole application against a stand-in launcher) | **fails**: still clicks the old interface's captions | ran it |
| `cargo fmt`, `cargo clippy` | **not clean**: 36 rustfmt diffs in `gui/`, 3 clippy warnings | ran both |
| `gui/src/view/old_view.rs.txt` | leftover copy of the old view, kept as reference while porting | every old test has a successor (compared name by name and assertion by assertion) |
| Visual pass | **not done**. Defects seen on first look are listed in section 3 | opened the PNG files |
| `docs/notes/gui.md` | **missing** | — |
| deny.toml policy with the lyon crates, start on `:77`, live test, workspace tests | **not checked** | — |

## 2. Plan (in this order; each step is one or more commits)

1. Mechanical: fmt, clippy, remove `old_view.rs.txt`, port `tests/emulated.rs`.
2. `backend::scram` tests with stand-in launchers (exit 1 + "COULD NOT", slow scram is not
   killed at the wait limit or when the receiver is dropped, missing launcher).
3. Visual pass, several rounds (section 3).
4. `docs/notes/gui.md`.
5. Final verification: fmt, clippy (workspace), `cargo test --workspace`, the live test
   (`gui/tests/live.rs`) on `:77`, start/exit of the real window on `:77`, `cargo deny`
   if the tool is available.

## 3. Visual defects found (first look at the snapshots at takeover)

- Rail: the tag line "SANDBOXED LAUNCHER" is wider than the rail and runs over its edge
  into the stage (all pages, both sizes).
- Launch, 880x720: while a launch is under way the page needs scrolling (status lines and
  Stop make the control block taller); the block also changes height between states.
- Launch, 1280x800: three columns of different heights at the top, the lower half empty.
- Security: the emergency panel is below the fold at 880x720, so the "COULD NOT" lines of
  an incomplete scram are not visible without scrolling; at 1280 the page is capped at
  980 wide while the banner above it is not (ragged right edge).
- Security: the running-scram text says "has no time limit here" although the window
  stops waiting after 30 minutes; the text must state what happens then.
- Scroll bar: 10 px wide and bright; meant to be a thin line.
- Console: its well ends 10 px left of where the panels of the other pages end.
- Accounts: a card whose first button row is empty shows a hole above "Sign out".
- Snapshot fixture `accounts-device-code` shows a sign-in for an account that is listed as
  signed in (the fixture, not the code).

## 4. Things learned about the code and this machine

- The shell of this session has `DISPLAY=:0` set (the user's desktop). Every command that
  could open a window must unset it or set `DISPLAY=:77 XAUTHORITY=/dev/null`.
  `cargo test -p bastion-mc-gui` is run as `env -u DISPLAY -u WAYLAND_DISPLAY cargo test …`.
- Never run `bastion-mc scram --yes` (the kernel keyring is the user's real one). The
  scram path of the GUI is therefore tested against shell stand-ins only.
- `iced_test` renders with tiny-skia (`.cargo/config.toml` sets `ICED_TEST_BACKEND`);
  snapshots land in `target/gui-snapshots/` (deleted and rewritten by
  `view::tests::every_screen_renders_dark`).
- Test binaries live under `target/debug/build/bastion-mc-gui/<hash>/out/` on this
  toolchain (cargo 1.100), not under `target/debug/deps/`.
- In event mode the launcher's `scram --yes` prints its report as `status` events, then an
  `accounts` event, and exits 1 when its report is incomplete (`src/cli.rs`).

## 5. Log

- (takeover) Read history, checkpoint diff, notes, all GUI sources; built; ran the tests;
  looked at the snapshots; wrote this file.
- (step 1, commit 016734c) `cargo fmt`, clippy clean; `old_view.rs.txt` removed;
  `gui/tests/emulated.rs` ported and extended to four end-to-end tests (play + console,
  accounts + device-code sign-in + cancel, scram after a pending sign-in with the order of
  launcher calls asserted, incomplete scram). Verified: `cargo test -p bastion-mc-gui`
  green without a display. Learned: four emulators starting in parallel crash in the
  Vulkan loader (`coredumpctl info` showed `vkEnumerateInstanceExtensionProperties`), so
  they take a mutex; the stand-in launcher script is written while that mutex is held.
- (step 2) `backend::scram` tests against shell stand-ins: report line by line with the
  exit status (1 stays 1), error event, killed by a signal, missing launcher, the wait
  limit (`StillRunning`, stand-in not killed), receiver dropped, runtime dropped. Mutation
  check: with `kill_on_drop(true)` put back, the wait-limit and the runtime tests fail.
  Experiment: the real launcher run as `--events instance list` with its output pipe
  already closed exits 0, not 141 (it does not die of SIGPIPE; `src/events.rs` marks the
  queue broken and carries on). The same for `scram` is from reading `src/cli.rs` only —
  the real `scram --yes` must not be run here.
- Learned: `bastion-mc status` reads the kernel keyring entry of the account it is asked
  about (`TokenStore::has` loads the secret). The window runs `status` when it starts, so
  to start the real window use a config with `[auth] token_store = "none"` in an XDG root
  of its own (not the e2e config, which leaves the default "keyutils").

- (steps 3 onwards, commits a0cee12 .. 0919c90 — written down by the third engineer from the
  commit messages, the second engineer did not get to it) Visual pass rounds 1 and 2
  (measured page layout, fixed-height launch block, runbook laid out from its parts,
  strained inputs, whole-page snapshots); review of the launch page code; opt-in live test
  of the whole application (`gui/tests/live.rs`, run then against a real Minecraft 26.3
  demo launch on the private display: both tests passed); the scram launcher gets a process
  group of its own; the window opens at 1180x720.

### Third engineer — takeover at commit 0919c90 (2026-11-13, about 21:35)

The work tree was clean (no "WIP checkpoint 2" commit). Checked by running, not by reading:

| Item | State found | How checked |
|---|---|---|
| `cargo test -p bastion-mc-gui` without a display | green: 108 unit, 4 emulator, 2 live (skipped without their variables) | ran it |
| `cargo fmt --check`, `cargo clippy -p bastion-mc-gui --all-targets` | clean | ran both |
| Five pages, 30 states x 2 sizes of snapshots + 12 extra | all render; looked at about 20 of them: well composed | opened the PNG files |
| `docs/notes/gui.md` | **missing** | — |
| Integrator's note about overextension (below) | rail tag line fixed ("MC LAUNCHER"), but **the layout test cannot see the defect it was written for**: iced clamps a text widget's bounds to the room it is given (`Limits::resolve`), so "text that starts in the rail ends in the rail" holds even when the glyphs run over the edge | read `iced_core/src/widget/text.rs` and `layout/limits.rs`; mutation check recorded in the log below |
| Launcher text that is neither cleaned nor bounded before it is shown | `Event::Error.message`, `Event::Status.text`, the device code and its address, the browser sign-in address; a version id that was cut is still offered | read `app.rs` |
| Drop-down labels | drawn by iced on one line without a bound: a long instance name with the "cannot be launched" suffix, or a long version id, runs out of its box | read `iced_widget/src/pick_list.rs` |
| `cargo deny`, start on `:77`, live test after the last two commits, workspace tests | **not checked yet** | — |

Plan of the third engineer, in this order:
1. A test that measures: every paragraph of text needs no more room than it was given
   (walk the widget tree after layout, compare each paragraph's `min_bounds` with its
   `bounds`), at 640, 720, 800, 880, 1000, 1100, 1180, 1280 and 1600 wide; a state in which
   every text the launcher supplies is one long unbroken word. Fix what it finds.
2. Clean and bound the remaining launcher text; drop version ids that cleaning changes.
3. Another look at every snapshot (also at 1000 and 1100 wide).
4. `docs/notes/gui.md`.
5. Final verification (plan step 5 above).


Log of the third engineer:

- (overextension, this commit) **Integrator's note of 21:05: done.** New test
  `view::tests::no_text_needs_more_room_than_it_was_given_at_any_width`: after a layout
  pass done by hand it walks the widget tree, takes every text widget's paragraph out of its
  state and compares the room the paragraph was given (`bounds`) with what its words take
  (`min_bounds`, and the height of all its lines), in every state at 13 window sizes from
  480x400 to 1600x900 (1000 and 1100 wide among them). Mutation check, run: with the rail
  tag line set back to "Sandboxed launcher" the old layout test still passes and the new one
  fails with `"SANDBOXED LAUNCHER" is 162.9 wide in a place 130 wide` in every state.
  It found, in the states that existed: the captions of the buttons in panel headings were
  2.3 px higher than their place (now `kit::tool`, a lower button for headings).
- (same commit) Eleven new states `unbroken-*`: every text the launcher supplies is 3000
  letters "W" without a space, with the largest numbers an event can carry. They found:
  a **panic** (`Instant + Duration` overflow for a device code with a huge `expires_in`:
  the window would have died; now clamped to an hour), and overextension in the notice
  banner, the status line, the device code and its address, account and instance
  descriptions, the progress read-out and both drop-downs. Fixed by rule, not case by case:
  running text breaks inside a word that is longer than its line (`kit::wrapped`, used by
  `body`/`small`/`mono`); one-line text from outside is cut to a number of characters that
  fits (`backend::fit`, `kit::mono_chars`, drop-down labels via `kit::with_picker_room`);
  the block under the gauge grows instead of clipping; a notice takes at most 38 % of the
  window's height and scrolls inside its banner; `Progress::amount` is at most 21
  characters whatever the numbers.
- (same commit) State: every string of an event is now made printable and bounded where it
  enters the state (status, error, log lines, device code and address, browser address); a
  version whose name cleaning would change, or that starts with "-", is not offered at all
  (before, a cut name was offered and would have been passed to `instance new`).
- Learned: iced never gives a text widget bounds larger than the room there is
  (`Limits::resolve` clamps), so nothing that looks at bounds can see letters running out of
  their place; the paragraph in the widget's state knows both sizes. A drop-down
  (`pick_list`) draws its label on one line without any bound and keeps its paragraphs in a
  private state: its labels must fit by construction. `Instant + Duration` panics on
  overflow.
- Verified after this step: `cargo test -p bastion-mc-gui` green without a display (114
  unit, 4 emulator, 2 live skipped); fmt and clippy clean; looked at the `unbroken-*`
  snapshots and at the changed normal ones (launch-downloading, instances-new).

- (closing during a scram, this commit) Found while re-reading the scram for B-L2: between
  "Yes, scram" and the moment the launcher doing it exists (up to six seconds when a sign-in
  has to be ended first), closing the window dropped the scram that had just been
  confirmed. Now `backend::scram` reports `ScramUpdate::Started` once the launcher is
  running, and a window asked to close while `Emergency::awaiting_start()` stays until then
  (a second request closes at once; a launcher that cannot be started keeps the window open
  with the notice). Tests in `app::emergency_tests` (they look into the returned task for
  `Action::Exit`); the worker test that drops the whole runtime now does so at `Started`,
  before the stand-in has said a word, and the stand-in still runs to its end.
- (same commit) Snapshots `between-*` (1000 and 1100 wide) and more `smallest-*`; the
  status line is part of the state `unbroken-accounts-browser`.

- **Machine rule, learned the hard way:** `cargo test -p bastion-mc-gui` must be run with
  the four XDG variables set (as well as without `DISPLAY`). The emulator tests
  (`gui/tests/emulated.rs`) create a headless GPU renderer, and the Vulkan driver writes its
  shader cache under `$XDG_CACHE_HOME` — without the variable that is the user's real
  `~/.cache`. My first three runs of this session lacked the variables;
  `~/.cache/radv_builtin_shaders/part0/mesa_cache.idx` has a modification time inside this
  session (the driver's own cache index; any Vulkan program of the user updates it too, so
  it cannot be said for certain that it was these runs). Nothing else under the real home
  was written. From then on every run used `XDG_*_HOME=target/a1-xdg/...`. The emulator
  cannot be told to use the software renderer (it passes no backend to
  `Headless::new`), and the crate forbids the `unsafe` that setting a variable in-process
  needs, so this stays a rule for whoever runs the tests.
- (verification so far, at commit ceaf24d) `cargo clippy --workspace --all-targets`: no
  warnings. `cargo test --workspace`: 386 passed, 0 failed, 26 test binaries.
  `cargo deny check` (0.20.2 from the main tree's `target/tools/bin`, `--offline --locked`,
  advisory database fetched today 18:55): advisories ok, bans ok, licenses ok, sources ok;
  duplicate versions of `syn` and `winnow` only (as before the lyon crates). `cargo tree -p
  bastion-mc` has no `iced`, `lyon` or any crate of the `[profile.dev.package]` block.

Still to do (in this order): look at every snapshot again, also at 1000 and 1100 wide;
`docs/notes/gui.md`; final verification (workspace fmt/clippy/tests, `cargo deny` with the
binary in the main tree's `target/tools/bin`, live test and start/exit on `:77` with a
configuration that has `token_store = "none"`).

- (commit 55bdf76, third engineer) The launch page names which launch its protection
  read-outs belong to ("report awaited" / "last launch"); the held step of the gauge is lit
  as a whole.

### Fourth engineer — takeover at commit ccd4077 ("WIP checkpoint 3"), 2026-11-13 22:06

Found by running: `cargo test -p bastion-mc-gui` (no display, XDG variables set) green —
117 unit, 4 emulator, 2 live (skip without their variables). What the checkpoint held:

| Part of WIP checkpoint 3 | Decision |
|---|---|
| `app.rs`: a scram that could not be started resets `closing` even while a game is being stopped for the close (before, the window exited when the game was down and the "Nothing was removed" notice was never read), with the second half of `a_closing_window_stays_to_say_that_the_scram_could_not_be_started` | **kept** (compiles, test green, logic right); repaired: the launch line no longer says "Stopping the game before closing…" once the window stays |
| `docs/notes/gui.md`, first complete draft (459 lines) | **kept and finished**: its section 7 claims verifications ("live test", "real window started", "screenshot") that were planned, not yet run in this round — they stay only once I have run them; counts are re-checked against the suite |

Plan of the fourth engineer: (1) the WIP repair; (2) the remaining snapshot pass (every
snapshot, also `between-*` at 1000/1100); (3) finish `gui.md` with numbers checked;
(4) final verification: workspace fmt/clippy/tests, `cargo deny`, live test on `:77`,
start/exit of the real window on `:77` with `token_store = "none"`.

Log of the fourth engineer:


## Note from the integrator, 21:05 — feedback from the user (please act on it in the visual pass)

The user opened `launch-running-protected-1280x800.png` and `accounts-1280x800.png` and said:
"the layout isn't perfect — the styling causes it to overextend". What is visible in every
snapshot: the tag line "SANDBOXED LAUNCHER" is wider than the rail and runs across its
right edge into the stage. Treat overextension as the first item of the visual pass:
nothing — a text, a panel, the gauge block, a row of buttons — may reach beyond its own
container or beyond the window, at 880x720, at 1280x800 and at widths in between (try
1000 and 1100 as well). Where the test harness can measure layout bounds, add a test that
fails when an element is wider than its container, so that this cannot come back; then
look at every snapshot again. (This note was appended while you were working; keep it in
the file and tick it off in your log.)
- (WIP repair, this commit) WIP checkpoint 3 kept. Repaired: when a scram that the window
  waited for cannot be started while a game is being stopped for the close, the launch line
  says "Stopping the game…" instead of "…before closing…". Assertion added to
  `a_closing_window_stays_to_say_that_the_scram_could_not_be_started`; mutation check run
  (without the repair the test fails). `emergency_tests`: 17 green.
- (visual pass, this commit) Looked at every snapshot kind again (opening, launch states at
  both sizes, accounts incl. device code/browser/no client/while running/long names,
  instances, console, security incl. every scram outcome and the whole page, smallest,
  between 1000/1100, unbroken). One real defect: **account cards in one line had different
  heights** (opening size, 1180x720: 232.5/232.5/248.1 px, because the vault path takes a
  line more than the keyring), and the third card's buttons stood a line lower. iced's rows
  cannot stretch Fill-height children in a Shrink-height row (they collapse to the tallest
  *non-fill* sibling: `iced_core/src/layout/flex.rs`, third pass), and its `grid`/`table`
  do not equalise either. New widget `gui/src/view/tiles.rs` (`Tiles`): lays a line of tiles
  out twice (measure with height compression, then all at the tallest height); everything
  but layout delegates like `Row`. Needs iced's `advanced` feature (exposes the widget
  interface; adds no crate — `Cargo.lock` unchanged). Cards now fill their line and keep
  their buttons at the bottom (a bottom-aligned container, not a spacer: a spacer adds the
  column spacing twice). Test
  `view::tests::cards_side_by_side_are_equally_high_and_their_buttons_stand_on_the_same_lines`;
  mutation check run: with the old row and the old card the test fails on "equally high"
  (232.5 vs 248.1). All 32 view tests green, incl. the paragraph-measuring test at 13 sizes.
- (this commit) Under the gauge, a cut instance name no longer gets a full stop after its
  "…" ("Ready: an-instance-yyyy…"). Looked again at launch-long-names, launch-many-aspects,
  instances-long-names, security-running, smallest-{security,instances-new,accounts},
  unbroken-launch-error: nothing else to fix. **Visual pass finished.** Next: `gui.md`
  (counts, `Tiles`, `advanced` feature, open items), then the final verification.
- (`gui.md` updated: four engineers, 118 unit tests, `Tiles` in the design system and the
  layout rules, `advanced` feature in section 8, `tiny-skia-path` in the crate list, the
  card-height item removed from "open". Section 7 is rewritten at the very end, from what
  the final verification shows.)
- (final verification, part 1, at commit 03209f5 + notes) `cargo fmt --all -- --check`:
  clean. `cargo clippy --workspace --all-targets`: exit 0, no warning. `cargo test
  --workspace` (no DISPLAY, XDG in `target/a1-xdg`): 26 test binaries, 387 passed, 0 failed
  (log `target/a1-logs/workspace-test.log`). `cargo-deny 0.20.2 deny --offline --locked
  check` (binary of the main tree's `target/tools/bin`, advisory db of 18:54): advisories ok,
  bans ok, licenses ok, sources ok; warnings only (duplicate `syn`, `winnow`; seven allowed
  licences not encountered). `cargo tree -p bastion-mc -e all`: 630 lines, no iced, lyon,
  tiny-skia, cosmic-text or wgpu.
- Machine note: only `xprop` of the X tools is installed (no xdotool, wmctrl, xwd,
  ImageMagick). Next: live test on `:77` with `XDG_CONFIG_HOME=target/a1-live/config`
  (= the e2e config plus `token_store = "none"`, so the window's `status` call reads no
  keyring entry) and the e2e data/cache/state; then start/close of the real window.
- (final verification, part 2) **Live test** on `:77` (`XDG_CONFIG_HOME=target/a1-live/config`,
  data/cache/state of `target/e2e`, instance `vanilla-latest`): 2 passed in 9.7 s — worker:
  phases Preparing..Stopping, 36 game lines, exit 130; application: played and stopped
  through its own controls. No process of the clone left (pgrep). Log
  `target/a1-logs/live-test.log`.
- **Real window** on `:77`, scripts in the session scratchpad (not in the repo):
  `xwin.py` (Python ctypes on the system libX11: find the window by `_NET_WM_PID`, read its
  pixels with `XGetImage`, send `WM_DELETE_WINDOW`) and `real-window.sh` (bounded polling).
  Config `target/a1-live/config` (token_store none), data `target/e2e/data`, cache/state
  `target/gui-run`. GPU renderer and `ICED_BACKEND=tiny-skia`: both mapped (the WM sized the
  window 1276x696), both showed the instances and the account state, both exited 0 on
  `WM_DELETE_WINDOW`, empty stderr. Pictures `target/gui-run/final-{gpu,software}.png`,
  looked at: same composition as the snapshots. Idle 15 s: 0 clock ticks of CPU.
- **Fonts experiment repeated** (FreeSans/FreeMono in `theme.rs`, reverted after): it
  showed that the new card test's precondition depended on the font (in FreeSans the
  vault text and the keyring text take the same number of lines, so the case did not
  arise). The test now brings its own, clearly longer store text; with the free fonts all
  32 view tests pass, and the fair mutation (old row, old card) still fails on "equally
  high" (232.5 vs 279.3). Note: the snapshot test always writes `target/gui-snapshots`
  (there is no variable for another directory) — after a font experiment, rerun the tests
  with the real fonts (done: 118 green, snapshots rewritten).
- (this commit) `view/mod.rs` header: the paragraphs are measured at thirteen window sizes
  (it said eleven). Final run on the final state: `cargo fmt --all -- --check` clean;
  `cargo clippy --workspace --all-targets` exit 0, 0 warnings; `cargo test --workspace`
  26 binaries, 387 passed, 0 failed (logs `target/a1-logs/{clippy,workspace-test}-final.log`).
  The live test and the real-window runs were made before the last two commits, which
  changed only a test fixture and a comment.

### State at hand-in (fourth engineer, 2026-11-13 ~22:35)

**The package is complete.** Everything in the brief exists, is tested, and was verified as
recorded above; `docs/notes/gui.md` is the deliverable (user documentation, design system,
pages, the three review findings, threat-model rows, tests and snapshots, open items, notes
for the integrator). What remains open is listed in `gui.md` section 7 ("Open" and "Not
verified"): no real click, no sign-in against Microsoft, no real `scram --yes`, X11 only, GPU
renderer seen on the opening page only, keyboard-only use not examined, the launch page's
panel heights with very long texts, the pulse on a hidden window, versions the form leaves
out silently.
