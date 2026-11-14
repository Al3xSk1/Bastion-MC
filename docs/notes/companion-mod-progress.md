# Companion mod — hand-over file

Work package: the first-party companion Fabric mod (zoom, HUD readout, fullbright), built
from source by `build.rs` and injected by the launcher. This file is what a successor needs
to continue: what is done and how it was checked, what is in progress, what comes next,
and what is not obvious about the code or this machine. It is updated and committed with
every step. The package's documentation proper is `docs/notes/companion-mod.md`.

Second engineer on this package. The first one was cut off after about an hour (a usage
limit); their commits are `ad87362` … `4dfabb3`, their uncommitted state is the
integrator's `1647b53` ("WIP checkpoint"), and `ba4a2be` ("sync:") brings three files from
the real tree — leave that one alone.

**Third engineer, from 2026-11-13 21:33** (the second one's session ended at 21:31, in the
middle of a second round of final runs). I took over at commit `b444119`; the tree was clean
(no "WIP checkpoint 2"). What I found and did is in "Third engineer" at the end of this
file; the sections in between are the second engineer's, left as they were written.

**Fourth engineer, from 2026-11-13 22:06** (the third one was stopped on purpose at 22:04 to
move the team to another model). I took over at commit `6d35868`; the tree was clean and
there is no "WIP checkpoint 2" or "3" commit — everything of the third engineer's is in
their own commits. What I found and did is in "Fourth engineer" at the very end.

## State when I took over (verified, not taken on trust)

Checked by reading every file and by running things; "✓ ran" means I ran it myself.

| Brief | What exists | Checked how |
|---|---|---|
| Hermetic build, no Minecraft, no Gradle | `src/companion/jar.rs` (compiled into `build.rs` and into the tests), `java/companion`, `java/stubs` | ✓ ran `cargo build`; read `jar.rs`: javac with `-proc:none -implicit:none -sourcepath <empty dir>`, tool-option environment variables removed |
| Stubs equal the real types | 7 stubs under `java/stubs` | ✓ ran `BASTION_E2E_ROOT=$PWD/target/e2e cargo test --test companion` ("19 members and annotation properties of 7 types agree"; the mod compiled against the real jars is byte-identical); ✓ ran `javap` on the real Fabric Loader 0.20.1 and Mixin 0.8.7 classes by hand |
| Reproducible jar | own stored-zip writer in `jar.rs` (no new build dependency), `-g:none`, `--release 17`, fixed order and time stamp | ✓ ran `companion::tests::building_the_jar_again_gives_the_same_bytes` |
| Injection points from the real 26.3 jar | `Camera.calculateFov(F)F` (TAIL), `MouseHandler.turnPlayer(D)V` and `onScroll(JDD)V` (HEAD), `Hud.extractRenderState(GuiGraphicsExtractor,DeltaTracker)V` (TAIL), `LightmapRenderStateExtractor.extract(LightmapRenderState,F)V` (TAIL) | ✓ ran `javap -p -c` on all four classes in `target/e2e/data/bastion-mc/versions/26.3/26.3.jar`; the comments in the mixins and features say what the bytecode does |
| `fabric.addMods` | `-Dfabric.addMods=/bastion/companion/bastion-companion.jar` | ✓ ran `javap -c` on `ArgumentModCandidateFinder` in Fabric Loader 0.20.1: system property, split on `:`, `@file` lists, missing path = warning; the same name is also read as a game argument |
| Read-only mount outside `mods/`, Fabric only, version list, force switch, log line with SHA-256 and features | `src/companion/mod.rs` (`decide`, `plan`, `Plan::attach/describe/serve`), 3 small blocks in `src/launch.rs` | ✓ ran the unit tests (25 in `companion::`) |
| `[companion]` in config.toml, override in instance.toml, read-only into the game | `CompanionConfig`, `CompanionOverride`, `examples/config.toml`, settings file as `Mount::RoData` | ✓ ran unit tests incl. `config::tests` (example = defaults) |
| Channel: one UNIX socket, launcher listens, line protocol, untrusted | `src/companion/channel.rs`, `Channel.java` | ✓ ran unit tests; fuzz harness `fuzzing::companion` + seeds + `robustness::companion_channel_reader` |
| Zoom, HUD, fullbright; registry | `feature/Zoom.java`, `HudReadout.java`, `Fullbright.java`, `Feature.java`, `Hooks.ALL` | read; real-game evidence below |
| Real game, opt-in test | `tests/companion_game.rs` (title mode; `BASTION_COMPANION_WORLD=1` enters the demo world) | previous engineer's last log `target/e2e/state/bastion-mc/logs/fab/launch-1794620101.log` shows all of it passing; **I have not yet run it myself** |
| Sandbox footprint | `tests/companion.rs::in_the_sandbox_the_mod_is_two_read_only_files_and_a_socket_to_the_launcher` | ✓ ran |
| Offline JVM tests (no Minecraft: every lookup fails) | `tests/companion.rs`, `tests/java/CompanionOffline.java` | ✓ ran |
| **WIP**: fake-game harness | `tests/java/CompanionFakeGame.java`, `tests/java/fakegame/`, `tests/java/fakegame-changed/` — compiles and runs by hand, **not wired into any Rust test** | ✓ compiled and ran by hand; output looks right |
| **WIP**: `Zoom.bindKey` reports an unknown key name as such | in the checkpoint commit | builds; covered once the fake-game test exists |
| Notes file | **missing** | — |

`cargo test --workspace` at the take-over state: see "Log" below.

## Plan (in order)

1. [x] Wire the fake-game harness into `tests/companion.rs` (default test run, no Minecraft):
       zoom arithmetic, screen-open rule, wheel, HUD text, fullbright, a hook that starts
       failing, and the "changed game" variant (one feature off, the others on). Three new
       tests; the whole file passed three times in a row. Mutation check: with the
       "no screen open" condition removed from `Client.playing()` the first of them fails
       (`fov_screen_open=17.5`), then restored.
2. [x] Run `tests/companion_game.rs` myself, title and world mode, on `:77` (see Log).
3. [x] Review pass over what exists (findings below); what did not hold is fixed.
4. [x] `docs/notes/companion-mod.md`.
5. [x] `cargo fmt --all`, `cargo clippy --workspace --all-targets` (no warnings),
       `cargo test --workspace` (351 passed, 0 failed), the opt-in suites (Log, last entry).
6. [x] A real key press (XTEST, opt-in `BASTION_COMPANION_KEYPRESS`): works, see finding 7.

**Nothing is in progress. The package is complete as far as this machine allows; what is
open is listed in part 5 of `companion-mod.md`.**

If you continue anyway, the natural next things are, in this order: (a) a person at the
keyboard (hold C, wheel, fullbright in a cave); (b) `force` on another Minecraft version in
a real game; (c) approval prompts — designed in the notes, part 2, deliberately not built.

## Findings of the review pass

1. **Fixed — a race in `tests/companion.rs::in_the_sandbox_…`.** The Python payload ran to
   its end at once, and `Running::payload_pid()` then sometimes (after my change to the
   file: always) looked for a process that had already exited, so the test failed on
   `assert!(has_socket)`. It had passed at the take-over state by timing alone. Now the
   payload waits for its standard input to end, the test serves the channel and removes the
   socket's name first, and only then lets it go on — which is also the order of a real
   launch, so the test now shows that the socket still works after its name is gone.
2. **Fixed — the stand-in `InputConstants.getKey`** did not do what the real one does for a
   name that ends in a number (`javap` on 26.3: `Type.getOrCreate(n)` names the key
   `key.keyboard.<n>`, or returns the key that number already has). Corrected, and a test
   for a key named by number added.
3. **Fixed — the self-test entered the world too early.** My first world-mode run failed:
   `world entered … screen=…PopupScreen`, `fov_zoomed=70`. The demo's greeting screen
   appears a moment *after* the world does; the old loop had stopped waiting by then, and
   with a screen open the zoom (rightly) did nothing. `SelfTest.settledInWorld` now requires
   four uninterrupted seconds of "playing" and closes screens that turn up meanwhile; the
   Rust test asserts `screen=none`. Passed three times afterwards.
4. **Added — the wheel through the game's own method.** The only hook whose effect had not
   been measured in the real game was `MouseHandler.onScroll`. World mode now calls the
   game's `onScroll` once with the key scripted down (hotbar slot unchanged, factor 4 → 4.6)
   and once with it up (slot 0 → 8).
5. **Fixed — a stale socket name after a failed sandbox start.** `Plan::serve(false, …)`
   keeps the socket's name (bubblewrap may still need it); the launcher then confines
   itself and can no longer remove anything under `$XDG_RUNTIME_DIR`, so `Endpoint::drop`
   fails silently and `companion-<random>/channel` would stay until logout. Happens only
   when the payload's pid cannot be established (the sandbox failed to start, or a very
   slow machine). Now `Endpoint::create` first sweeps such leftovers (`channel::sweep`):
   only a directory named as this module names them, holding a socket called `channel`
   that refuses connections and is at least a minute old (a socket another launch has just
   bound is not listening yet either). No change outside `channel.rs`. (The Wayland
   security-context socket has the same trait; not mine to change.)
6. Checked and found in order: `fabric.addMods` parsing (javap on Fabric Loader 0.20.1);
   all five injection points and the reasoning in the mixins' comments (javap -c on 26.3);
   `InputConstants.isKeyDown` is the SDL keyboard state indexed by scancode, hence the
   bound `0 < value < 512` in `Zoom.bindKey`; manifest validation only admits
   `-Dname=value`, so `starts_with("-Dfabric.addMods=")` is a complete check;
   the vanilla warning `Requested post effect does not exist: minecraft:end_of_frame` on
   world entry is the game's own (the string is in `GameRenderer.class`), not the mod's.

7. **Added — the key really pressed.** XTEST on a private X display delivers a key event
   that the game's SDL input takes like a keyboard's. `tests/support/x11-key.py` presses
   `c` when told; the self-test's last phase watches the camera with nothing scripted,
   while playing and with the chat open. Result: zoom to 17.5 while playing, no zoom and
   `cccc…` typed with the chat open — directly and through gamescope. This needs a display
   that **nothing else** uses (the key goes to whatever has the focus), so I started one of
   my own (`tests/support/private-x11.sh /run/user/1000/bmc-a2`, linked as `:78`) instead
   of pressing keys on the shared `:77`, and removed it afterwards.
8. Read, not changed: what 26.3's light-map shader does with the brightness value
   (`assets/minecraft/shaders/core/lightmap.fsh`: unclamped mix factor) — so 15 does mean
   "every light level at full brightness"; nobody watched it in a cave.

## Things that are not obvious

* Machine: display `:77` only, `XAUTHORITY=/dev/null`; never `:0`. One game at a time;
  stop with `pkill -INT -f "<clone>/target/debug/bastion-mc launch"`.
  `target/a2/run-game.sh <instance> <pattern> <max-s> <linger-s> <logfile>` (not in git,
  `target/` is ignored) does a bounded launch and never leaves a game running.
* `tests/companion_game.rs` must stay alone in its file: a launch ends with the launcher
  confining the *test process* (Landlock).
* The self-test's world mode sets `pauseOnLostFocus=false` on the game's options object (a
  test window is not necessarily focused). In the runs here the game did not write that to
  `options.txt` (the test instance's file still says `true`); if it did, the launcher would
  carry it back into the instance — only ever in a test data root.
* World mode creates `saves/Demo_World` in the test instance on first use and reuses it.
* A launch creates `$XDG_RUNTIME_DIR/bastion-mc/companion-<random>/channel` (the real runtime
  directory: the systemd scope needs the real one too) and removes it within a second.
* `target/a2/` (ignored by git) holds my and the previous engineer's scratch files: run
  logs (`final-*.log` are the last runs), `run-game.sh`, `shot.py` + `shots-loop.py`
  (screenshots of a private display; `shots78/` are from my own display during a key-press
  run: readout visible, zoomed picture visible), screenshots.
* A private display of your own, for key presses: run
  `tests/support/private-x11.sh /run/user/1000/bmc-a2` in the background, then
  `ln -s /run/user/1000/bmc-a2/x11/X0 /tmp/.X11-unix/X78` and use `DISPLAY=:78
  XAUTHORITY=/dev/null`. To remove it: kill the `bwrap` whose command line names
  `/run/user/1000/bmc-a2`, `rm /tmp/.X11-unix/X78`, and remove `/run/user/1000/bmc-a2`
  (file by file — a recursive delete with a variable in it was refused by the permission
  system here). It is not running now.
* A foreground `sleep` is not available in this environment's shell tool; I used
  `python3 -I -c "import time; time.sleep(n)"` where a short wait was needed, and scripts
  for anything longer.
* The cargo here (1.100) puts build-script output under
  `target/debug/build/bastion-mc/<hash>/out/`.
* `javap` and the real jars: `target/e2e/data/bastion-mc/versions/26.3/26.3.jar`,
  `target/e2e/data/bastion-mc/libraries/net/fabricmc/{fabric-loader,sponge-mixin}/…`.

## Log

* take-over: read everything, built, ran `cargo test --lib companion` (25 ok),
  `BASTION_E2E_ROOT=… cargo test --test companion` (4 ok).
* take-over: `cargo test -j 6 --workspace` at the take-over state: 347 passed, 0 failed
  (output kept in `target/a2/test-all-takeover.log`, not in git).
* step 1 done: fake-game tests wired in (`with_a_stand_in_game_every_feature_does_what_it_says`,
  `a_game_in_which_one_method_has_changed_loses_that_feature_and_keeps_the_others`,
  `the_zoom_settings_are_honoured_and_a_key_the_game_does_not_know_switches_zoom_off`);
  `BASTION_E2E_ROOT=$PWD/target/e2e cargo test --test companion`: 7 passed, three runs.
  fmt and clippy clean.
* step 2: real game, by me, all on `:77` (2026-11-13 ~21:00): title mode ok (15 s); world
  mode FAILED once (finding 3), then ok ×2 directly and ×1 with `BASTION_NESTED_LAYERS`
  (gamescope); after adding the wheel measurement ok ×1. CLI launches: `vanilla-latest`
  ("companion: not added — this instance has no mod loader"), `fab` with the default
  configuration ("features: zoom", hello received). No process left, runtime dir empty.
* step 4: notes file written and committed.
* finding 5 fixed (sweep), finding 7 added (real key press), notes updated.
* final verification on the committed sources (2026-11-13 ~21:25–21:35):
  `cargo fmt --all` (no change), `cargo clippy --workspace --all-targets` (0 warnings, after
  touching every crate root), `cargo test --workspace` (351 passed, 0 failed);
  `BASTION_E2E_ROOT=… cargo test --test companion` (7 passed, stand-ins agree with the real
  jars); `BASTION_FUZZ_ITERATIONS=300000 cargo test --test robustness companion_channel_reader`
  (passed, with the default seed and with seed 20261113); `tests/companion_game.rs`: title on
  `:77`, world on `:77`, world + gamescope on `:77`, world + key press on `:78`, world + key
  press + gamescope on `:78` — all passed; `tests/nested_game.rs` for `vanilla-latest` and
  `fab` — passed. No game, no helper and no display of mine left running; `:78` and
  `/run/user/1000/bmc-a2` removed.

## Third engineer (from 2026-11-13 21:33)

### State at take-over (verified, not taken on trust)

* HEAD was `b444119`, tree clean. Three commits (`2e91a5d`, `d47761f`, `b444119`) are
  *later* than the last entry of the Log above: the self-test's `hud_conditions` phase with
  its assertions in `tests/companion_game.rs`, the notes for it, and a comment. The second
  engineer had started a second round of final runs for them (`target/a2/*final2*`):
  clippy clean, `cargo test --workspace` green, title mode passed at 21:30; the world-mode
  run was killed with the session (its log ends in "the nested compositor ended
  unexpectedly (signal: 15 (SIGTERM))" — the interruption, not a defect).
  **So at take-over the last three commits had one passing real-game run on record
  (`target/a2/world-hudcond.log`, 21:28) and no completed final round.**
* Leftovers removed: `/run/user/1000/bmc-a2` (an unfinished private display) and the
  dangling `/tmp/.X11-unix/X78`. No process of this clone was running.
* ✓ ran at take-over: `cargo build --workspace --all-targets` (up to date),
  `cargo test --lib companion` (26 passed), `BASTION_E2E_ROOT=… cargo test --test companion`
  (7 passed, "19 members and annotation properties of 7 types agree").

### Plan

1. [x] World mode on `:77` at `b444119` (a verified baseline before anything changes): passed.
2. [ ] Review pass of my own over everything (findings below).
3. [ ] Final round on the final sources: fmt, clippy, `cargo test --workspace`, the opt-in
       suites (stand-ins, fuzz iterations, title, world, world + gamescope, key press on a
       display of my own, `nested_game` for both instances).
4. [ ] Notes and this file brought up to date; structured result.

(Items 2–4 were left open when the team was stopped at 22:04; the fourth engineer finished
them — see "Fourth engineer" at the end of this file.)

### Findings (third engineer)

T1. **Fixed — a JVM option could point the mod at settings the game can write.** `Config.java`
    took the path of its settings file from the system property `bastion.companion.config`
    (there for the tests, which run the mod without a launcher) *in preference to* the
    launcher's file. A signed profile's loader options are arbitrary `-Dname=value`
    properties and come after the launcher's on the command line, so a profile carrying
    `-Dbastion.companion.config=/game/…` would have made the mod read a file inside the game
    directory — settings that anything in the game could rewrite for later sessions
    (fullbright on, say). That contradicted "the game cannot change it persistently".
    Shown first: the new test `tests/companion.rs::a_jvm_in_the_sandbox_runs_the_mod_on_the_
    launchers_settings_not_on_a_file_the_game_wrote` (system JVM in the real sandbox, started
    through the launch bootstrap, the injection's files and socket, a planted file in `/game`
    named by the property) **failed** against the old mod: `hud disabled`, `fullbright
    disabled`, `no launcher channel configured` — the planted settings were in force. Fix:
    where the launcher's file exists it is the only source; the property is consulted only
    when there is no such file (`NoSuchFileException`), i.e. outside a launcher's sandbox.
    The test passes with the fix, and so do the offline tests that rely on the property.
    The same test is also the first on the default `cargo test` to run the mod's own channel
    code (Java NIO, UNIX socket) through Landlock and seccomp to the launcher's reader.
T3. **Observed — what Mixin does when the game does not fit** (the `force` case; until now
    "not observed"). An experiment by hand, with a deliberately altered build that was not
    committed: `CameraMixin` aimed at a class that does not exist, `LightmapMixin` at a
    method that does not exist; title mode on `:77` (`target/a3/experiment-mismatch-title-77.log`,
    `target/e2e/state/bastion-mc/logs/fab/launch-1794624017.log`). Result with Mixin 0.8.7
    (sponge-mixin 0.17.5), Fabric Loader 0.20.1, Minecraft 26.3: for the missing *class*
    Mixin logs two warnings (`Error loading class: net/minecraft/client/CameraGone …`,
    `@Mixin target net.minecraft.client.CameraGone was not found bastion-companion.mixins.json:CameraMixin`),
    the game starts and reaches the title screen, zoom says once that it is off (`hook
    bastion$zoomFov was not applied to net.minecraft.client.Camera`, log and channel), the
    hello lists the other two. For the missing *method* (`require = 0`) Mixin logs nothing
    and merges the handler without calling it from anywhere — which is why a feature checks
    the target method itself (`Game.requireHook`) and why the real-game test measures every
    hook's effect. Not observed: a real other Minecraft version.
T4. **Corrected — every real-game run so far was inside gamescope.** The notes (and the Log
    above) distinguish runs "directly on the X display" from runs "through gamescope". On
    this machine there was no such difference: `gamescope 3.17.0-1` has been installed
    system-wide since 19:50 today (`/var/log/pacman.log`), `sandbox.nested_compositor` is
    `auto`, and all launch logs of this clone (`target/e2e/state/bastion-mc/logs/fab/`) say
    "the game's display is :0 inside the nested compositor". `BASTION_NESTED_LAYERS` only
    swaps the installed gamescope for the verified package copy of the same version. So the
    game as a direct client of the X display had never been run. Now it can be asked for:
    `BASTION_COMPANION_DIRECT_X11=1` (the test sets `nested_compositor = "off"`), and the test
    prints which display path a run had (`display  | …`). World mode passes on both paths.
T2. Fixed — stale references in comments: `TESTED_MINECRAFT` and `self_test` named
    `tests/companion.rs` where `tests/companion_game.rs` is meant; `Feature.java` named a
    `FEATURES` list that does not exist.

### Log (third engineer)

* 21:33 take-over; read the hand-over file, the notes, `git log --stat`, every source file
  of the package; cleaned the leftovers; built; ran the two quick suites (above).
* 21:38 `tests/companion_game.rs`, world mode, on `:77`, at `b444119` (jar sha256 `312642fd…`,
  67600 bytes): **passed** in 45 s, including the `hud_conditions` assertions
  (`frames_while_hud_hidden=0 frames_while_debug_overlay=0 frames_afterwards=43`,
  `lines_with_reduced_debug_info=[62 fps]`). Log: `target/a3/baseline-world-77.log` (not in
  git). `target/a3/game-test.sh <log> <display> [VAR=value …] [-- <test target>]` is my
  bounded runner for these tests; it refuses to start while anything of this clone runs.
* 21:39 T1 found, shown (the new test failing against the old mod), fixed; T2. The jar's
  bytes change with it, so every real-game run so far is of the *previous* jar: the final
  round (plan item 3) is still to come. `cargo test --lib companion` 26 passed,
  `BASTION_E2E_ROOT=… cargo test --test companion` 8 passed; fmt and clippy clean.
* 21:40 T3: the mismatch experiment (above); sources restored (`git status` clean), rebuilt.
* 21:41–21:44 final round on the sources of `778a8ec` (jar sha256 `a5e1857f…`, 67867 bytes),
  part 1: `cargo fmt --all` (no change); `cargo clippy --workspace --all-targets` after
  touching every crate root (0 warnings); `cargo test --workspace` with `DISPLAY=:77
  XAUTHORITY=/dev/null` (352 passed, 0 failed); `BASTION_E2E_ROOT=… cargo test --test
  companion` (8 passed, stand-ins agree with the real jars); `BASTION_FUZZ_ITERATIONS=300000
  BASTION_FUZZ_SEED=1794624100 cargo test --test robustness companion_channel_reader`
  (passed, 22 s); `tests/companion_game.rs` title on `:77` (passed, 19 s) and world on `:77`
  (passed, 53 s). Still to run: world + gamescope, the key press on a display of my own
  (directly and through gamescope), `nested_game`, a CLI launch of `fab` with the default
  configuration.
* 21:44 world + `BASTION_NESTED_LAYERS` on `:77`: passed (55 s). Then T4; test switch added.
* 21:47 world + `BASTION_COMPANION_DIRECT_X11=1` on `:77`: passed (45 s), "display | the
  session's display itself, no nested compositor". (`tests/companion_game.rs` changed for
  this — the test file only, the jar is the same `a5e1857f…`.)
* 21:48–21:50 the key really pressed, on a display of my own (`:78`, started with
  `tests/support/private-x11.sh /run/user/1000/bmc-a2`, removed afterwards): world +
  `BASTION_COMPANION_KEYPRESS=1` passed three times — in the nested compositor (the default
  here), with `BASTION_COMPANION_DIRECT_X11=1`, and with `BASTION_NESTED_LAYERS`. Each time:
  `fov_min=17.500` while playing, `fov_min=70.000` and `cccc…` typed with the chat open.
* 21:51–21:52 `tests/nested_game.rs` for `fab` and for `vanilla-latest` on `:77`: passed
  (launch logs: `features: zoom` + hello for `fab`; "not added — this instance has no mod
  loader" and no `fabric.addMods` for `vanilla-latest`). `bastion-mc launch fab --demo`
  through the command line (`target/a2/run-game.sh`): added, `features: zoom`, hello,
  summary, exit 130 on SIGINT.
* 21:53–21:55 **the Wayland path**, which had never been run with the companion: a headless
  Wayland compositor of my own (`tests/support/sway-headless.sh /run/user/1000/bmc-a2w
  gles2`, removed afterwards), `WAYLAND_DISPLAY=/run/user/1000/bmc-a2w/wayland-1`, no
  `DISPLAY`: title and world mode passed; the launcher's display line was "Wayland security
  context: capture and input injection withheld", the game's "Created window using SDL
  video driver: wayland". The test now prints the display path in the launcher's own words
  (the `display` item of its protection event) instead of guessing it from the log.
* After all of it: no process of this clone, `/run/user/1000/bmc-a2*` gone,
  `/run/user/1000/bastion-mc/` empty.

## Fourth engineer (from 2026-11-13 22:06)

### State at take-over

* HEAD `6d35868`, tree clean, no WIP checkpoint commit to finish or revert.
* The third engineer's plan items 2 (review pass), 3 (final round) and 4 (notes, this file)
  were not ticked. Their final-round runs (Log above, 21:41–21:55) were on the sources of
  `778a8ec`…`0f601be`; after that, `9fc7318` changed only the notes, but `6d35868` (22:02)
  changed launcher code — `src/companion/channel.rs`: the session lock is taken with
  `unwrap_or_else(PoisonError::into_inner)` instead of `expect`, plus a new unit test — and
  is not in their Log. The jar is unaffected by it (the mod's sources did not change after
  `778a8ec`); the launcher binary is. So nothing at all has been run on the final launcher.
* No process of this clone running; no `/run/user/1000/bmc-a2*` leftovers; `:77` alive.

### Plan

1. [x] Build; run the quick suites and `cargo test --workspace` on `6d35868`.
2. [x] Review `6d35868` (and finish the review pass the third engineer had open): F1, F2.
3. [x] Final round on the final sources: fmt, clippy, `cargo test --workspace`, stand-ins
       against the real jars, fuzz iterations (the reader's file changed), real game title +
       world on `:77` (the launcher changed: hello, summary), `nested_game` for `fab`.
       Key press / Wayland / direct X11 are not repeated unless something they exercise
       changes (they exercise the jar and the display path, neither touched since).
4. [ ] Notes and this file brought up to date; structured result.

### Findings (fourth engineer)

F1. **Corrected — `6d35868` claimed more than it does.** The change itself holds: every
    counter and the token bucket are updated *before* the log sink is called, so a guard
    recovered from a poisoned lock sees valid state, and the real sink can panic (on a
    terminal `Reporter::emit` prints with `println!`, which panics on a closed pipe). But
    the release profile has `panic = "abort"` (Cargo.toml; the PKGBUILD and the README
    build with `--release`), so in the packaged launcher a panicking sink ends the process
    and the recovery never runs; it helps debug builds and the tests only. Also, a reader
    that panics never reaches `close`, so its connection slot stays taken (after four such
    panics further connections are refused — counted). Comment on `channel::locked` and
    the notes row now say both. No code change.
F2. Checked and found in order: the shared-file insertions (`launch.rs`: socket created
    before the sandbox, served before the launcher confines itself, summary read before
    settings are carried back; `config.rs`, `lib.rs`, `fuzzing.rs`, `build.rs` small and
    contiguous); `settings()` renders only validated values (no newline can reach the
    properties file; `zoom_factor = nan` fails the range check); `self_test` is
    `#[serde(skip)]` and reaches the mod only as a `selftest=` line in the launcher's own
    file, which (T1) is the mod's only source where it exists; every hook is guarded and
    switches its feature off once.

### Things that are not obvious (fourth engineer)

* **My shell inherits `DISPLAY=:0` and `XAUTHORITY=/home/ski/.Xauthority`**, and
  `tests/nested.rs` reads `DISPLAY`. Every `cargo test` here must override them:
  `target/a4/safe.sh <command>` (not in git) runs a command with `DISPLAY=:77
  XAUTHORITY=/dev/null` and no `WAYLAND_DISPLAY`.
* `cargo clippy` replays cached diagnostics for fresh units, but to be sure every target is
  really checked, touch the crate roots first
  (`find src tests gui/src crates examples benches -maxdepth 3 -name '*.rs' | xargs touch`);
  with `-v` every target shows as `Running`.

### Log (fourth engineer)

* 22:06 take-over; read this file, the notes, `git log --stat`, the diff of `6d35868`.
* 22:08 `cargo build --workspace --all-targets` (ok); `cargo test --lib companion`
  (27 passed); `cargo test --workspace` through `safe.sh` (353 passed, 0 failed, 25 s).
* 22:15 F1, F2 (review). `cargo fmt --all` (no change besides mine);
  `cargo clippy --workspace --all-targets` after touching every crate root: 0 warnings,
  every target re-checked (`-v`: all `Running`).
* Since `778a8ec` (the sources of the third engineer's final runs) only
  `src/companion/channel.rs` and `tests/companion_game.rs` changed: the jar's inputs
  (`java/`, `tests/java/`, `jar.rs`, `build.rs`) did not, so the jar should still be
  `a5e1857f…` — confirmed by the launch logs of the runs below (sha256 `a5e1857f63c7…`,
  67867 bytes, rebuilt by `build.rs` after `jar.rs` was touched: reproducible again).
* 22:19 final round on the final launcher (logs in `target/a4/`, not in git), each with
  `target/a3/game-test.sh` (bounded; refuses to start while anything of this clone runs):
  `tests/companion_game.rs` title on `:77` — passed (19 s); world on `:77` — passed (42 s),
  same deterministic measurements as every earlier run; `tests/nested_game.rs` for `fab`
  on `:77` — passed (18 s; launch log: `features: zoom`, hello, summary). CLI
  `bastion-mc launch fab --demo` (`target/a2/run-game.sh`, human mode, i.e. the `println!`
  sink): added, `features: zoom`, launcher confined, hello after that, summary, exit 130.
  `BASTION_E2E_ROOT=… cargo test --test companion`: 8 passed ("19 members and annotation
  properties of 7 types agree"). `BASTION_FUZZ_ITERATIONS=300000 cargo test --test
  robustness companion_channel_reader` with the default seed (24 s) and with
  `BASTION_FUZZ_SEED=1794626100` (22 s): passed. Not repeated (they exercise the jar and the
  display path, unchanged since the third engineer ran them on the same jar): key press,
  direct X11, Wayland, `BASTION_NESTED_LAYERS`, `nested_game` for `vanilla-latest` (the
  vanilla path never reaches the channel code). After all runs: no process of this clone,
  `/run/user/1000/bastion-mc/` empty.
* Notes: part 4 has a "Fourth engineer" paragraph with these runs; the row about a failing
  log sink says it helps unwinding builds only (F1).
* 22:24 the mandated last chain on the final tree: `cargo fmt --all` (no change);
  `cargo clippy --workspace --all-targets` (0 warnings); `cargo test --workspace` through
  `safe.sh` (353 passed, 0 failed, 32 s; the opt-in real-game files skip themselves there).
  4. [x] — **Nothing is in progress. The package is complete as far as this machine allows;
  what is open is part 5 of `companion-mod.md`** (a person at the keyboard, `force` on
  another Minecraft version, other Fabric Loader versions, approval prompts — designed,
  deliberately not built).
