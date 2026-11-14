# The graphical front end — notes of the GUI package

Package: the "workshop HUD" interface of `bastion-mc-gui` (`gui/`), and the front end's
halves of the review findings B-M2, B-M1 and B-L2 (`docs/notes/review-findings.md`).
Nothing outside `gui/` changed except `Cargo.toml` (one block), `Cargo.lock` (six crates)
and `.cargo/config.toml` (a comment). `crates/proto` is untouched: the wire format is what
it was.

Contents: 1 user documentation for the README · 2 design system · 3 what each page shows ·
4 the three review findings · 5 threat model · 6 tests and snapshots · 7 open and
unverified · 8 for the integrator.

How this was made: four engineers in sequence (the first three were interrupted). The
history of who found and verified what is in `docs/notes/gui-progress.md`.

---------------------------------------------------------------------------------------------

## 1. User documentation (replaces the "Graphical front end" section of the README)

### Graphical front end

`bastion-mc-gui` is an optional window around the same launcher. It is its own design — a
dark instrument panel with a ring gauge around the launch control — and it has no
advertising, no news, no shop and no telemetry: every element on it shows something the
launcher said or does something you asked for.

```sh
cargo build --release --workspace        # builds bastion-mc and bastion-mc-gui
target/release/bastion-mc-gui            # finds bastion-mc next to itself, else /usr/bin
bastion-mc-gui --account alt             # start with another local account
bastion-mc-gui --launcher /path/to/bastion-mc
```

A navigation rail on the left leads to five pages; the rail also shows which account a
launch would use, and a small square next to a page when something there needs you (a
sign-in waiting for its code, a protection that is not in force, an incomplete scram).
What the launcher is doing is named at the top right of every page.

* **Launch.** The gauge with the launch control in its middle; the instance that would be
  launched (a list, and *Manage* leads to the instances); the demo-mode switch; the five
  stages of a launch; what the running or the last launch is protected by. *Play* can be
  pressed exactly when a launch can start — otherwise the line under the gauge says why
  not. Stopping is a separate button, so a second press on the launch control can never
  stop a game.

  The gauge, from the outside in: the **sequence** (one arc per stage — files, verify,
  sign-in, sandbox, game; the stage being worked on has a highlight travelling along it),
  a **scale**, the **progress** ring (download progress against the scale; closed and
  slowly breathing while the game runs; amber and still while it is being stopped), and
  the **protection** ring with one segment per aspect of the launch.

* **Protection read-outs.** A launch reports five aspects — *display*, *network*,
  *credential*, *limits*, *launcher* — each at one of three levels. The level is always a
  word and a shape, never a colour alone:

  | Level | Word | Mark | Ring segment |
  |---|---|---|---|
  | on | IN FORCE | filled shield | solid |
  | partial | REDUCED | half-filled shield | broken into three |
  | off | NOT PROTECTED | hollow shield, struck through | two thin lines |
  | — | NO REPORT | outline | one faint line |

  Before any launch the page lists the five aspects with "no report"; nothing is claimed
  about a launch that has not happened. After the game has ended the read-outs stay, under
  the heading "last launch".

* **Accounts.** One card per local account (a *label* with its own stored token, as for
  `bastion-mc --account`): the label, the player name where the launcher knows one, whether
  a token is stored, where it rests, which account is in use and which is the launcher's
  default. *Switch to* uses another account for launching (nothing is signed in or out by
  that); *Make default* changes what the launcher uses when no account is named; *Sign in*
  and *Device code* start a sign-in for that account, *Sign out* removes its token. A new
  label is added with the tile at the end. There is no picture of the player: the
  launcher's events carry none, so the square shows a letter.

  A sign-in in progress has a panel of its own at the top: the device code, large, with
  *Copy code*, *Copy link* and — only for an address on one of Microsoft's four sign-in
  hosts — *Open link*; or, for the browser sign-in, *Open sign-in page* and *Copy link*.
  Both show how long the prompt is still valid and can be cancelled.

* **Instances.** Every instance the launcher found, with its Minecraft version, its kind
  and, for a signed profile, its serial and signer; one that cannot be launched says why.
  Pressing an instance selects it. *New instance* opens a form: a name and a version from
  the launcher's list (with or without snapshots).

* **Console.** The launcher's, the relay's and the game's output, most recent last, each
  line with its source; *Copy all* and *Clear*. The last 800 lines are kept; the complete
  log is the launcher's file.

* **Security.** The emergency panel first: **Scram…** asks, **Yes, scram** does it (two
  different buttons). While it runs the page says so and for how long; it is never
  interrupted, and closing the window does not stop it. Its result is reported in the
  launcher's own lines — "scram complete" only if the launcher exited with status 0 and
  named nothing it could not remove; anything else is **SCRAM INCOMPLETE**, in red, with
  the lines that say what is still there set apart. Below: what the last launch ran with,
  in words, with what each aspect covers; the twelve recovery pages of the runbook (*Open*
  and *Copy* for each — these twelve addresses and no others); and the runbook itself.

While a game runs, controls that cannot apply (another launch, signing in or out, changing
the instance, creating one) are drawn and not offered. One launcher command runs at a time;
the one exception is the scram, which works beside a running game because ending that game
is part of what it does.

The window opens at 1180 × 720 and can be made as small as 640 × 520: below 800 wide the
rail shows its marks only (with the page names as tool tips), and pages fold into fewer
columns or scroll. Nothing moves on screen while nothing happens: the window has no timer
when it is idle, 25 steps a second while a launch is being prepared, five while the game
runs (and only while the launch page is shown), and one a second while a sign-in prompt or
a scram is counting.

Rendering uses the GPU when one is usable and falls back to software otherwise
(`ICED_BACKEND=tiny-skia` forces software). No font or image is bundled: text uses Adwaita
Sans and the system's monospace face where they are installed and whatever sans-serif and
monospace faces there are otherwise; every symbol is drawn as geometry.

(The separate-process paragraph and diagram of the current README stay as they are. The
sentence "The front end has both behind **Emergency…**" in the scram section becomes "The
front end has both on its **Security** page". In the repository-layout table, `gui/` is now
`backend.rs` (worker), `app.rs` (state and logic), `view/` (the pages and the parts they
are built from), `hud.rs` (drawn elements), `theme.rs`, `runbook.rs`, `console.rs`. In the
table of what was tested, the front end's row is: 118 unit tests and 4 tests of the whole
application against a stand-in launcher, without a display; a real demo launch driven
through the application's own controls; the window started and closed on a private X11
display.)

---------------------------------------------------------------------------------------------

## 2. Design system

The rules are in the module headers of `gui/src/theme.rs`, `gui/src/hud.rs` and
`gui/src/view/kit.rs`; pages do not style widgets themselves.

**Palette** (`theme.rs`). One accent, two alarms; no other hue, so a colour always says
one of three things.

| Name | Value | Used for |
|---|---|---|
| `VOID` | `#0a0d10` | the window |
| `DECK` | `#070a0c` | the rail |
| `PLATE` / `PLATE_HI` | `#10161b` / `#16202a` | panels / selected or under the pointer |
| `WELL` | `#05080a` | recessed areas: console, inputs, the device code |
| `LINE` / `LINE_HI` | `#1f2b35` / `#33485a` | hairlines / outlines that have to be seen |
| `ACCENT` | `#3fd9e6` | in force, selected, proceed |
| `AMBER` | `#f5b041` | caution, reduced |
| `RED` | `#ff5f56` | danger, not protected, failed, destructive |
| `TEXT` / `TEXT_DIM` / `TEXT_FAINT` / `TEXT_OFF` | `#dbe6ec` / `#9bb0bd` / `#738a9a` / `#4a5c69` | body / secondary / labels / controls that are shown but not offered |

A unit test recomputes the contrast ratios (body text at least 12:1 on every surface,
labels at least 4.5:1, the three signal colours at least 5:1) and checks that the signal
colours differ in lightness as well as hue.

**Type.** Sizes 11 (labels), 12 (secondary, read-outs), 14 (body), 34 (the device code).
Labels and button captions are tracked capitals (`theme::caps`: upper case with a
six-per-em space between characters — the toolkit has no letter spacing). Read-outs,
identifiers and addresses are monospace. Spacing is a scale of 4, 8, 12, 16, 24.

**Parts** (`view/kit.rs`, `hud.rs`).

* *Plate*: a panel with its top-left and bottom-right corners cut off, a hairline outline
  and a short luminous mark at the cut corner; toned amber or red when the panel reports
  something. Every panel heading is one 26-pixel line.
* *Brackets*: four corner marks around an area without an outline — where the eye should
  go (the block around the gauge, the device code).
* *Buttons*: outlined (`action`), filled for the one decisive action of a view (`solid`),
  lower for a panel heading (`tool`), caption only (`link`). A control that cannot be used
  keeps its place and its caption and loses its colour.
* *Chip* (a small outlined tag that states a fact), *pip* (a square: filled for "this
  one"), *switch* (position says the state), *shield mark*, *monogram*, *well*, *rule*.
* *Gauge*: described in section 1. It holds steps, a fraction and levels, not angles, and
  draws the same picture for the same value and time.
* *Tiles* (`view/tiles.rs`, the one widget of our own): a line of tiles in equal columns,
  all as high as the highest. The toolkit's rows cannot stretch their children to the
  height of a line whose height comes from its content (on a scrolling page, every line);
  this lays the line out twice, once to measure and once at the tallest height.
* Nothing is rounded except the launch control.

**Layout rules** (`view/mod.rs`).

* Rail 188 px (58 px below a window width of 800), a stage with a 20 px margin, and a
  12 px gutter for the scroll bar so that the content edge is the same on every page and
  does not move when a bar appears. A page is handed the room it has, measured.
* Launch: three columns from a page width of 920 (the block is centred, at most 1180 wide,
  the gauge grows from 248 to 336 px); two columns from 540; one below. The block around
  the gauge keeps room for the read-out, three lines of words and the stop control in every
  state, so pressing *Play* moves nothing.
* Accounts: cards at least 300 px wide in one to three columns. Cards that stand in one
  line are equally high and keep their buttons at the bottom, so the buttons of a line are
  on the same lines whatever the texts above them (a vault path takes a line more than the
  keyring). Security: two columns from a page width of 900; the emergency panel is always
  first.
* A notice (an error, a warning) is a banner above the page on every page. It takes at
  most 38 % of the window's height; a longer one scrolls inside the banner.
* **Nothing is drawn outside the place it was given.** Running text breaks inside a word
  that is longer than its line; text that must stay on one line and comes from outside the
  program is cut to a number of characters that fits (and marked with "…"); what the
  launcher says is made printable and bounded where it enters the state. The tests measure
  this (section 6).

---------------------------------------------------------------------------------------------

## 3. What each page shows, and from what

Everything on screen is a function of `app::App`; the view reads no clock and does no work.

| Page | Shows | State it reads |
|---|---|---|
| every page | page title, what the launcher is doing, a notice if there is one, a status line, the launcher's version | `job`, `emergency.state`, `notice`, `status`, `launcher_version` |
| rail | the five pages, attention markers, the account in use and its sign-in state | `page`, `protection`, `login`, `emergency`, `account` |
| Launch | gauge and launch control (word, player name or "demo mode", time or per cent), read-out of what is being downloaded, the launcher's last words about the launch, *Stop*; instance list, demo switch; the five stages; protection read-outs with a summary tag | `can_play()`, `play_blocker()`, `sequence()`, `progress`, `launch_status`, `selected`, `demo`, `protection` |
| Accounts | sign-in panel (device code or browser prompt, countdown, cancel); one card per account; the tile that adds a label; a note when no OAuth client is configured | `accounts` (label + the launcher's summary, kept as last reported), `default_account`, `account_name`, `login`, `can_sign_in()`, `can_sign_out()` |
| Instances | list with selection, tags and reasons; the form with name check, version list and the snapshots switch | `instances`, `selected`, `new_instance`, `can_create_instance()` |
| Console | line count, lines by source and level, *Copy all*, *Clear* | `console` (built only while this page is shown) |
| Security | emergency panel (ask, confirm, running time, verdict, failed lines, full report); protection of the last launch in words with a legend; the twelve recovery pages; the runbook | `emergency`, `protection`, `proto::runbook::{LINKS, SCRAM}` |

The account cards need no launcher call of their own: `Event::Accounts` carries a summary
per account and the state keeps it (`AccountEntry.summary`). A label the launcher has not
described (one just added here; every label after a scram, until the launcher has been asked
again) has no summary, and nothing is offered or claimed for it.

Rules of the state machine that the view relies on and the tests hold: every interaction is
a `Message`; `App::update` is the only place state changes and never blocks; one launcher
command at a time (`App::job`); a link is handed to the desktop only after `is_sign_in_link`
(four Microsoft hosts, exact) or `proto::runbook::is_runbook_link` (twelve addresses, exact)
— checked where the control is built *and* again where the message is handled.

---------------------------------------------------------------------------------------------

## 4. The three review findings

**B-M2 — a scram the launcher reported as incomplete was shown as done.**
`backend::scram` passes the launcher's exit status on unchanged (`ScramEnd::Exited(code)`)
and its report line by line. `App::on_scram_finished` calls a scram complete only if the
status is 0 *and* no line started with "COULD NOT" *and* the launcher said something at
all; a status of 0 is not believed over a "COULD NOT" line. Everything else —
any other status, death by a signal, the wait limit — is `ScramOutcome::Incomplete` with
the reason; a launcher that could not be started is `NotRun` ("nothing was removed").
An incomplete scram is said on every page (a notice of error severity beginning "SCRAM
INCOMPLETE"), on the security page as a red block with the "COULD NOT" lines set apart in
it, in the full report (the same lines in red, in their place), in the console at error
level, and by a red marker on the rail. In that state no text on any page reads "scram
complete" or "scram done" (a test reads every text of every page), and the notice contains
none of "done", "complete" and "removed" outside the word "incomplete".
Afterwards the launcher is asked for the state of the accounts again, so the cards show
what is in fact still stored.
Tests: `backend::tests::a_scram_report_arrives_line_by_line_and_ends_with_the_launchers_exit_status`,
`app::emergency_tests::{a_scram_the_launcher_calls_incomplete_is_never_reported_as_done,
nothing_but_a_clean_exit_with_a_clean_report_counts_as_complete,
a_scram_that_could_not_be_started_says_that_nothing_was_removed,
a_long_report_is_bounded_without_losing_what_could_not_be_removed}`,
`view::tests::{an_incomplete_scram_is_said_plainly_with_what_is_still_there,
a_scram_that_did_not_run_or_did_not_end_is_not_dressed_up}`, and end to end
`emulated::a_scram_the_launcher_reports_as_incomplete_is_shown_as_incomplete_end_to_end`.

**B-M1, front end's half — the scram was killed after 60 seconds.**
Nothing in the front end ends the launcher that is removing credentials any more: no
timeout that kills, `kill_on_drop(false)`, no stop channel, its standard error goes to
`/dev/null` (a pipe whose reader has gone makes the writer fail), and it runs in a process
group of its own (Ctrl-C in the terminal the window was started from is not aimed at it).
While it runs the page says "Removing stored sign-ins… still running, m:ss". The bound that
remains is on the *waiting*: after 30 minutes (`backend::SCRAM_WAIT_LIMIT`) the window stops
waiting, leaves the launcher running, and reports the scram as incomplete with exactly
that reason ("has not finished after 30 minutes. It was left running and may still be
removing things; this window no longer waits for it"). The page says in advance that this
is what will happen. Closing the window during a scram leaves the launcher to finish.
Tests: `backend::tests::{a_scram_that_outlasts_the_wait_limit_is_reported_as_still_running_and_is_not_killed,
a_scram_nobody_listens_to_any_more_runs_to_its_end, a_scram_survives_the_front_end_going_away,
a_scram_runs_in_a_process_group_of_its_own}`,
`view::tests::a_running_scram_says_that_it_is_still_running_and_shows_what_was_reported_so_far`.
Checked that they can fail: with `kill_on_drop(true)` put back, the wait-limit test and the
runtime test fail (second engineer).

**B-L2, front end's half — a pending sign-in stored a token after the scram.**
When a scram is confirmed while a sign-in is the running job, the sign-in is ended first
(`ScramState::StoppingSignIn`): the launcher doing it is told to stop and killed after
three seconds; the scram starts the moment that launcher is reported gone. Should that
report never come, the scram starts after six seconds regardless — credentials are removed
either way — but its result can then not be "complete": a failure line says that the
sign-in may still store a token. While a scram is active no sign-in and no launch can be
started (`can_sign_in`, `play_blocker`, and the same rule in `update` for a message that
slips through).
Tests: `app::emergency_tests::{a_sign_in_in_progress_is_ended_before_the_scram_starts,
a_sign_in_that_was_asked_for_before_its_launcher_started_is_stopped_the_same_way,
the_scram_does_not_wait_forever_for_a_sign_in_to_end,
nothing_that_stores_a_token_can_be_started_while_a_scram_runs}`, and
`emulated::a_scram_ends_a_pending_sign_in_first_and_says_complete_only_for_a_clean_exit`,
which asserts the *order* in which a stand-in launcher was called: `login`, "login ended
by: stop", then `scram --yes`.

**Found while doing B-L2: closing the window could drop a confirmed scram.** Between "Yes,
scram" and the moment the launcher doing it exists (milliseconds normally, up to six
seconds when a sign-in is ended first) no process is removing anything yet; a window closed
in that time took the scram with it. The worker now reports `ScramUpdate::Started` once the
launcher is running, and a window asked to close before that stays until then (it says
"Closing as soon as the scram has started…"). A second request closes at once; a launcher
that cannot be started keeps the window open with the notice, until it is asked again.
Tests: `app::emergency_tests::{a_window_asked_to_close_stays_until_a_confirmed_scram_has_started,
a_closing_window_stays_to_say_that_the_scram_could_not_be_started,
a_window_closing_for_a_game_and_a_scram_waits_for_both}`; the worker test that drops the
whole runtime does so right at `Started`.

Not in this package: the launcher's halves (the order in which `scram` removes things,
B-M1; in-flight commands that are not this window's own job — a `login` in a terminal,
`vault reseal`, `skin` — B-L2). The front end relies on the launcher's contract: exit
status 1 and lines starting "COULD NOT" when something could not be removed.

---------------------------------------------------------------------------------------------

## 5. Threat model: rows and changes this implies (for §4.5 of `docs/THREAT_MODEL.md`)

Unchanged in substance: G1 (no credential in the GUI process), G5 (other X11 clients).
The GUI's dependency tree grew by six crates (lyon, for drawing; see section 8); the
launcher's did not.

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| G2 (extended) | Hostile game output attacks the console: escape sequences, megabyte-long lines, floods | As before; in addition the front end now makes *every* string of an event printable and bounded where it enters its state, not only stray output: log lines (6000 characters), status lines, error messages (2000), names, descriptions. | as before, plus `app::tests::{what_the_launcher_reports_is_bounded_where_it_is_shown_and_longer_in_the_console, what_the_launcher_says_is_made_printable_and_bounded_before_it_is_kept, instances_and_versions_from_the_launcher_are_checked_before_they_are_shown_or_used}`, `app::account_tests::account_labels_from_the_launcher_are_checked_before_they_are_shown` | — |
| G3 (extended) | The sign-in prompt is used to make the desktop open an attacker's URL | As before. The prompt's texts are bounded in the front end too (code 32 characters, address 200); an address that had to be cut is no longer one of the four hosts' addresses and gets no *Open* control. A second check where the message is handled. | as before, plus `view::tests::no_link_outside_the_two_allowlists_ever_gets_an_open_control` (an application in which every text the launcher can supply carries a hostile address: no page has an open control beyond the twelve), `app::tests::a_sign_in_prompt_is_bounded_in_what_it_says_and_in_how_long_it_counts` | — |
| G4 (extended) | The GUI crashes or is killed while a game runs | As before. A malformed number in an event can no longer end the window: a device code with an absurd `expires_in` overflowed the clock it was added to (a panic; found by the hostile states) and is now clamped. | as before, plus the test above and the `unbroken-*` states (every number an event can carry at its maximum) | Other ways to make the toolkit panic are not known and not excluded. A crash stops the game in an orderly way (that is G4), so this is a denial of service. |
| G6 (rewritten) | The emergency panel is used to open an attacker's page; scram is triggered by a stray click; **scram is reported as done when it was not; it is cut short; a sign-in stores a token behind it** | Links: the compiled-in twelve, compared for equality, checked twice. Scram: two different buttons in sequence; leaving the page withdraws the question. Result: "complete" only for exit status 0 with no "COULD NOT" line and a report that says something; anything else is shown as incomplete with the launcher's lines. The launcher doing it is never ended by the front end (no kill, own process group, no pipe it could fail on); the 30-minute limit ends the waiting only. A pending sign-in of this window is ended first; no sign-in or launch starts while a scram is active; a window asked to close before the scram's launcher exists waits for it. | section 4 | Scram can be triggered by anything that can click in the window (G1): a denial of service. **A sign-in or another token-storing command started outside this window is not known to it** (launcher's half of B-L2). The verdict is the launcher's: a launcher that exits 0 after a clean-looking report is believed. "Left running after 30 minutes" is a report, not a remedy. That the real launcher carries on when the window disappears mid-scram is from reading `src/cli.rs` and from an experiment with another command (`instance list`), not from running `scram --yes`, which must not be run on the development machine. |
| G7 (new) | Text from the launcher — which passes on what servers, files and the game say — is made so long or so unbreakable that it is drawn across a control or a warning: a protection read-out, the scram verdict, the *Stop* button | Nothing is drawn outside the place it was given (section 2): text wraps inside long words, one-line text is cut to fit, a notice is bounded in height and scrolls. A test lays out every page in every state — among them states in which every launcher text is 3000 unbreakable letters — at 13 window sizes and compares, for every paragraph, the room it was given with the room its words take. | `view::tests::{no_text_needs_more_room_than_it_was_given_at_any_width, nothing_is_clipped_overlapping_or_out_of_reach_at_any_size, the_installed_monospace_face_is_no_wider_than_the_layout_takes_it_to_be}`; mutation check: with the tag line that once ran out of the rail put back, the first of these fails in every state | Measured with the fonts of the development machine (Adwaita Sans/Mono; the same tests pass with FreeSans/FreeMono substituted, an experiment, not part of the suite). One-line text is cut by a character count that assumes a monospace face no wider than 0.62 em; with a wider face such a line can be too long for its place. The label of a drop-down and the value of an input field are drawn by the toolkit and are not among the measured paragraphs; they fit by construction. Below a window width of about 480 px (the window does not let itself be made smaller than 640, a tiling window manager can) headings of some panels are wider than the panels. Text can still push *other* text down a scrolling page: what is important is placed first (the scram verdict, the launch control). |

For §4.6, row A2 (scram): "…and the front end's emergency panel" should read "…and the
front end's Security page, which shows the launcher's verdict and never a better one (G6)".

---------------------------------------------------------------------------------------------

## 6. Tests, and the snapshots to look at

```sh
# no display needed; the four XDG variables keep the GPU driver's cache out of ~/.cache
# (the emulator tests create a headless GPU context where there is a GPU)
env -u DISPLAY -u WAYLAND_DISPLAY XDG_CONFIG_HOME=… XDG_DATA_HOME=… XDG_CACHE_HOME=… XDG_STATE_HOME=… \
    cargo test -p bastion-mc-gui

# opt-in: the application against the real launcher and a real demo game (see gui/tests/live.rs)
BASTION_GUI_LIVE_LAUNCHER=$PWD/target/debug/bastion-mc BASTION_GUI_LIVE_INSTANCE=<id> \
    cargo test -p bastion-mc-gui --test live -- --nocapture --test-threads=1
```

* `gui/src/app.rs` (53 tests): the state machine — launch control, accounts, instances,
  sign-in, the emergency stop, closing, the clock, what is kept of the launcher's words.
* `gui/src/backend.rs` (15): the worker against shell stand-ins — events, stop, a vanished
  window, over-long lines, and the scram worker.
* `gui/src/view/tests.rs` (32): the widgets, laid out, clicked and rendered in software —
  navigation; the accounts page (cards, switch, default, add, availability of sign in and
  out, cards of one line equally high with their buttons on the same lines); the two-step
  scram and each of its outcomes; the three shield levels on both pages; no open control
  outside the two allowlists; layout at four sizes; measurement of every paragraph at
  thirteen; the launch page fits its window and does not move; snapshots.
* `gui/src/{console,runbook,theme,hud}.rs` (18): folding of game output, the runbook taken
  apart (every word once, in order; the addresses set apart are exactly the allowlist),
  contrast, motion.
* `gui/tests/emulated.rs` (4): the whole application run by iced's emulator against a
  stand-in launcher that records how it was called.
* `gui/tests/live.rs` (2, opt-in): the worker, and the whole application pressing Play and
  Stop, against the real launcher and a real Minecraft demo session.

**Snapshots.** `view::tests::every_screen_renders_dark` writes 115 PNG files to
`target/gui-snapshots/` (deleted and rewritten at every run; `BASTION_GUI_SNAPSHOT_SCALE=2`
for twice the resolution). 44 states, each at 880 × 720 and 1280 × 800, plus:
`opening-*` (1180 × 720, what a user first sees), `between-*` (1000 and 1100 wide),
`smallest-*` (640 × 520), `whole-*` (a scrolling page in a window tall enough for all of
it). For a review, in this order:

1. `opening-launch-idle-1180x720`, `opening-launch-running-1180x720` — the first screen,
   idle and with a game running (one aspect reduced, one not protected).
2. `launch-downloading-880x720`, `launch-verifying-1280x800`, `launch-stopping-880x720`,
   `launch-ended-1280x800`, `launch-blocked-880x720`, `launch-refused-880x720` (the longest
   real error message in the banner).
3. `accounts-1280x800`, `accounts-880x720`, `accounts-device-code-1280x800`,
   `accounts-browser-880x720`, `accounts-no-client-880x720`, `accounts-while-running-880x720`.
4. `instances-880x720`, `instances-new-1280x800`, `instances-none-880x720`.
5. `console-880x720`, `console-empty-880x720`.
6. `security-880x720`, `security-running-1280x800`, `security-scram-asking-880x720`,
   `security-scram-running-880x720`, `security-scram-complete-880x720`,
   `security-scram-incomplete-880x720`, `security-scram-not-run-880x720`,
   `whole-security-scram-incomplete-1280x2500` (the whole page with the runbook).
7. Small and in between: `smallest-launch-idle-640x520`, `smallest-accounts-640x520`,
   `smallest-security-640x520`, `between-launch-running-protected-1000x720`,
   `between-accounts-1100x720`, `between-security-scram-incomplete-1100x720`.
8. Strained input: `launch-long-names-*`, `accounts-long-names-*`, `instances-long-names-*`,
   `launch-many-aspects-*`, and the hostile ones: `unbroken-launch-unusable-880x720`,
   `unbroken-launch-bytes-1280x800`, `unbroken-launch-error-880x720` (a notice that
   scrolls), `unbroken-accounts-code-880x720`, `unbroken-instances-880x720`,
   `unbroken-security-880x720`, `smallest-unbroken-launch-unusable-640x520`.

---------------------------------------------------------------------------------------------

## 7. Open and unverified

Verified by running, on the development machine, after the last change to the code of this
package (the log of each run is in `docs/notes/gui-progress.md`):

* `cargo test -p bastion-mc-gui` without a display: 118 unit tests and 4 emulator tests
  green (the 2 live tests skip without their variables). `cargo fmt --all -- --check`
  clean; `cargo clippy --workspace --all-targets` without a warning; `cargo test
  --workspace`: 26 test binaries, 387 passed, 0 failed.
* The live test (`gui/tests/live.rs`) on the private X11 display, against the real launcher
  and a real Minecraft 26.3 demo session (instance `vanilla-latest`), twice in one run:
  the worker alone (phases Preparing, Verifying, Starting, Running, Stopping; 36 lines of
  game output; exit status 130 after the stop), and the whole application pressing its own
  controls (select the instance, demo switch, Play, "The game is running." with all five
  aspects at a level, the launcher's and the game's output in the console, Stop, "The game
  was stopped."). No launcher or game process was left over.
* The real window on the private display against the real launcher (a configuration with
  `auth.token_store = "none"`, so its `status` call reads no keyring entry), once with the
  GPU renderer and once with `ICED_BACKEND=tiny-skia`: it showed the launcher's instances
  (the signed profile with its serial and signer) and the account state. Its pixels were
  read from the X server (`XGetImage` of the window) and looked at: the same composition as
  the software snapshots. Closed with `WM_DELETE_WINDOW` (what a window manager's close
  button sends): exit status 0 both times, nothing on standard error. Left idle for 15
  seconds, the window used 0 clock ticks of CPU time (less than 10 ms): no timer runs when
  nothing moves.
* `cargo deny check` (0.20.2, offline, advisory database of the same day): advisories,
  bans, licences and sources ok; warnings only (two duplicate crate versions, `syn` and
  `winnow`, and allowed licences that no crate uses). `cargo tree -p bastion-mc` has no
  iced, lyon, tiny-skia, cosmic-text or wgpu.
* Layout: measured as described; every snapshot named in section 6 was looked at after
  the last change that could affect it (of those given with a `*`, at least one size).

Not verified, and why:

* **No click was ever made in a real window.** Clicks cannot be automated in a window on
  this machine and the window must not be opened on the user's desktop. Everything about
  pressing things is from the headless simulator and emulator (same widgets, same `update`,
  no window system).
* **Sign-in was never run against Microsoft** (no account, no client ID here): the device
  code and browser prompts are exercised with fixtures and a stand-in launcher only.
* **`scram --yes` was never run from the window** — it would remove real sign-ins from the
  user's kernel keyring. The scram path is tested against shell stand-ins that follow the
  launcher's contract; whether the real launcher keeps to that contract is the launcher
  package's matter.
* **Wayland**: the window ran on X11 (Xwayland in a headless compositor) only.
* **The GPU renderer** was seen on the opening page of the real window only (above); every
  other picture is from the software renderer. Motion was never watched: the gauge is
  tested as a function of time, not looked at moving.
* **Other fonts**: with FreeSans/FreeMono substituted in `theme.rs` (an experiment, then
  reverted; repeated after the last change) all 32 view tests pass, the measurement of
  every paragraph at thirteen sizes among them, and the two snapshots that were looked at
  (`opening-accounts`, `opening-launch-running`) have the same composition (FreeMono is
  thin at 12 px: legible, fainter). Nothing else was tried.
* **High-density displays**: snapshots at scale 2 render correctly in software; no real
  display of that kind was used.
* Keyboard-only use (tab order, activation by keys) was not examined; iced's defaults apply.

Open:

* The launch page's panels are stretched to one line with the block around the gauge only
  when their words are of usual length; otherwise each is as high as its content.
* The 5-per-second redraw while a game runs (the breathing ring) happens only on the launch
  page; whether it should stop when the window is hidden was not looked into.
* A version name longer than 48 characters, or one that cleaning would change, is not
  offered in the "new instance" form at all (the form says nothing about what it left out).

---------------------------------------------------------------------------------------------

## 8. For the integrator

* **Shared files.** `Cargo.toml`: one block appended at the end, `[profile.dev.package]`,
  which builds the crates that draw in software (tiny-skia, tiny-skia-path, cosmic-text,
  swash, zeno, skrifa, read-fonts, harfrust, iced_tiny_skia, iced_graphics, png, fdeflate) with
  `opt-level = 3` in debug builds — unoptimised, the headless tests take minutes. None of
  them is in the launcher's tree (`cargo tree -p bastion-mc`, all edge kinds, checked).
  Release builds are not affected. `.cargo/config.toml`: comment only. `Cargo.lock`: six
  crates added. No other shared file is touched (`src/` not at all).
* **Dependency.** `gui/Cargo.toml` enables iced's `canvas` feature (the gauge, frames and
  marks are drawn with it). It adds to the GUI binary only: `lyon`, `lyon_algorithms`,
  `lyon_geom`, `lyon_path`, `lyon_tessellation` (MIT OR Apache-2.0) and `float_next_after`
  (MIT), all from crates.io. `deny.toml` is satisfied unchanged. It also enables iced's
  `advanced` feature, which only makes the toolkit's widget interface public (for
  `view/tiles.rs`) and adds no crate: `Cargo.lock` did not change with it.
* **Renamed/moved.** `gui/src/view.rs` became `gui/src/view/` (`mod.rs`, `kit.rs`,
  `tiles.rs`, one file per page, `tests.rs`); new `gui/src/hud.rs`, `gui/src/runbook.rs`,
  `gui/tests/support/mod.rs`, `gui/tests/live.rs` (the old live test, extended).
* **Documentation to carry over** (I did not edit them): README — section 1 above, the
  test-table row, the repository-layout row, "Emergency…" → "Security page". Threat model —
  section 5. `TODO.md`: the front end's review findings B-M2, B-M1 (front end's half) and
  B-L2 (front end's half) are done; the items of section 7 are what is left.
* **Packaging.** `packaging/arch/PKGBUILD` is as synced; the GUI needs no new system
  package (lyon is pure Rust). A desktop entry, if there is one, keeps working: the
  window's application id is still `bastion-mc-gui`.
* **Running the GUI tests** needs the four XDG variables if the user's `~/.cache` is to
  stay untouched (GPU driver shader cache; see section 6).
