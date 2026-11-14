# mod-ui: hand-over file

Package mod-ui of round 4 (docs/notes/round4-contract.md): the phone and its apps' screens,
the home (title) screen, the dark theme — `java/companion/**/ui/`, `**/phone/`, `**/home/`,
`**/theme/`, built only on the platform API (`java/companion/**/platform/`, mod-platform's).
Clone `target/agents/r4-mod`, branch `work`, **shared with mod-platform** (same working tree,
same index). Private display `:82`, runtime directory `/run/user/1000/bmc-r2`, data root
`target/e2e-ui`, `CARGO_TARGET_DIR=target-ui` for every cargo command.

## Rules learnt for the shared tree

- Commit **only with pathspecs**: `git add <my paths> && git commit -m … -- <my paths>`.
  The index is shared; mod-platform stages renames (`java/stubs` → `java/stubs/common`) that a
  plain `git commit` would sweep into my commit.
- Their uncommitted work compiles into my builds too (build.rs compiles everything under
  `java/companion/src`); keep my files compiling at every save.
- Quick compile check without cargo: `target-ui/scratch/jc.sh [test sources…]` compiles
  HEAD's `java/companion/src` with **my working-tree packages on top** (so mod-platform's
  uncommitted work cannot break it), against `java/stubs/common`, with stand-ins for the
  generated `BuildInfo` and `FamilyTable`; classes in `target-ui/scratch/classes`, test classes
  in `target-ui/scratch/test-classes`. Then `java -cp classes:test-classes PhoneUiCheck` in
  `target-ui/scratch` runs the harness in about a second. Not in git (`target-ui/` is ignored).
- `companion::tests::building_the_jar_again_gives_the_same_bytes` fails while mod-platform is
  regenerating stubs in the shared tree (the sources change between build.rs and the test);
  re-run when the tree is quiet before believing it.
- `target-ui/debug` was seeded by a reflink copy of the clone's pre-built `target/debug`
  (btrfs; `cp -a --reflink=always`), so the first cargo build there is incremental.

## Done

1. Request file `docs/notes/r4-mod-requests.md`: R1 (what the UI needs beyond the platform's
   draft) and R2 (`[companion]` keys `phone`, `phone_key` = `key.keyboard.r`, `home`,
   `dark_menus`; feature ids `phone`, `home`, `darkmenus`). Both answered/accepted by
   mod-platform in `021230c` (Platform API v1).
2. UI sources, compiling against Platform API v1 with `-Xlint:all` and no warnings:
   - `theme/Theme.java` (palette of gui/src/theme.rs), `theme/DarkMenus.java` (feature
     `darkmenus`, menu background painter);
   - `ui/`: `Rect`, `Column`, `Gfx` (fitted labels, tracked caps, panels), `Icons` (pixel
     glyphs as code), `Hits` (hit regions, spatial keyboard focus, clip stack), `Kit`
     (buttons, rows, tabs, sliders, fields, paragraphs), `Scroller`, `TextField`, `Text`
     (cleaning launcher text), `Ui` (entry point: `Ui.features()`);
   - `phone/`: `Phone` (the overlay: geometry, slide, status bar, home grid, banner, HUD
     toast, input rules), `Store` (link listener: states, textures, notices), `Send` (every
     command line), `AppStates` (bounded readers of each app's JSON), apps `SpotifyApp`,
     `BrowserApp` (also Discord), `ClipsApp`, `RealmsApp`, `SettingsApp`, `PhoneFeature`;
   - `home/`: `HomeScreen` (title screen replacement), `HomeFeature`.
3. Fake-game harness `tests/java/PhoneUiCheck.java`, run by `tests/phone_ui.rs` (in the
   default `cargo test`): plays the platform and the launcher; drives every app; checks every
   draw call (inside its clip — text never cut, only shortened —, inside the phone body and
   its screen, on the screen, no text over text, nothing inside out) on 13 sizes from 200×150
   to 2560×1080 with rich, overlong, wide-script (CJK) and offline states; feeds 27 hostile and
   300 mutated launcher messages, pictures from 1×1 to 2048×1 in all 34 slots, 100 notices;
   every command line is parsed by the launcher's own `phone::Command::parse` and matched to
   the contract's vocabulary (verb, argument count and shape, text where text belongs), and
   34 (app, verb) pairs must each appear. **Verified (ran):** `cargo test --test phone_ui`
   passes — 82 scripted checks, 681 command lines, 5 452 frames, 817 396 draw operations, 0
   violations. Mutation check: with `Gfx.label` drawing text unshortened the harness reports
   13 131 violations. Defects it found and that are fixed: two Realms facts drawn over each
   other in narrow rows (now two lines when they do not fit), inside-out fills from
   `Canvas.outline` on 1-unit rectangles (my `Gfx.frame` guards; the platform's `outline`
   itself still does it — told mod-platform in R3), Escape stuck in the browser's landscape
   mode after the page closed, Spotify's two time labels able to meet (now `Gfx.pair`).
4. `ui/Clock.java`: the time source, set by the harness (animations, batching, countdowns are
   tested without waiting).
5. **Looked at, without the game:** a preview renderer (`target-ui/scratch/preview/Preview.java`,
   not in git) draws every screen with a Java2D `Canvas` and the game's own bitmap font
   (`ascii.png` + `nonlatin_european.png` read from the local 26.3 jar into
   `target-ui/scratch/preview/assets`), over a painted stand-in for the world, at 480×270 ×2,
   320×240 ×3, 640×360 ×2 and 854×480 ×2. Run: `cd target-ui/scratch && ./jc.sh
   ../../tests/java/PhoneUiCheck.java && javac -cp classes:test-classes -d test-classes
   preview/Preview.java && java -Djava.awt.headless=true -cp classes:test-classes Preview
   preview/assets/assets/minecraft/textures/font/ascii.png preview/out 480 270 2`. Changes
   from looking: phone wider (0.66 of its height, ≤ 196), Spotify tabs as pictograms with the
   selected one widened to carry its label, shorter clips captions, the value of a settings
   fact keeps its room first, home-screen links in plain case on rows of their own, shuffle
   and repeat glyphs redrawn. The preview is not the game (no world, no GUI scaling of the real
   window, no textures through the game's pipeline).

6. **Real game, 26.3 (fab), demo service** (`BASTION_PHONE_DEMO=1`), `tests/support/phone-shots.sh`
   on `:82`, window 854×480 (GUI 427×240, scale 2, shown letterboxed in gamescope's 1276×696
   window): hello `features: zoom, phone, home, darkmenus`; the home screen replaces the title
   screen (demo mode: Play Demo World / Options / Quit; Language, Accessibility, Classic title
   screen; copyright line); Tab focus visible; R pulls the phone out over the home screen
   (all six apps announced: the demo's five plus Discord); Spotify with the demo's album art,
   Discord's page (after `open discord`; `frames on`, `size 360 451`), **a real XTEST click on
   the page landed where the demo drew its cross — under the pointer (page pixel 180,236)**,
   the browser's site list, clips with the demo's thumbnails, the Realms prompt (countdown,
   tries), settings; vanilla Options with the dark background; in the demo world the phone
   over the running world (no blur, no dimming), the pause menu with the darker veil (the
   game's own blur stays: the platform hooks the background only). Runs r1–r4 in
   `target-ui/shots/` (not in git).
   Found and fixed: black behind the phone over the home screen (now the dark menu
   background); the edition line cut ("26.3 ·..."); in a world the phone covered the hotbar's
   right end at small GUI sizes (now it stands on the hotbar row when that leaves it at least
   180 high — 427×240: 198 high); the driver's navigation (Escape count; Tab cycling).
   Found, not mine: **no typed characters on 26.x** (SDL text input never started for the
   host screen) — request R4 to mod-platform; keys work, `charTyped` never comes.

7. **All ten families in the real game** (details and the table in `docs/notes/mod-ui.md`,
   part 4): sweep 1 at 854×480 (hello with phone, home, darkmenus on all ten), sweep 2 at
   1600×900 (all ten complete, the same 21–22 demo commands each, typing everywhere), world
   mode on 26.3 and 1.21.4, GUI scale 1 on 26.3, final runs (26.3 with landscape, 1.20.1).
   Fixed from them: the banner's missing second line, the Realms empty text, the hint wording,
   the hotbar placement; the driver's readiness marker and its lagging captures.
8. U1 from mod-platform (the harness's heap) answered and fixed; R3/R4 done by them; R5
   withdrawn.
9. Final checks on the shared tree at `0fa6da1`+: `cargo fmt --all -- --check` clean,
   `cargo clippy --workspace --all-targets` no warnings, `cargo test --workspace
   --no-fail-fast` (DISPLAY/WAYLAND_DISPLAY unset, XDG under `target-ui/xdg`): 601 passed, 0
   failed, 0 ignored, `tests/phone_ui.rs` among them.

## In progress

Nothing. The package is complete as far as this machine allows; open points are part 5 of
`docs/notes/mod-ui.md`.

## Next (if anyone continues)

1. Run the phone against the real services (media, browser, realms-sync) once they are merged:
   `tests/support/phone-shots.sh` without `BASTION_PHONE_DEMO` on an instance whose launcher
   config sets them up.
2. Mouse drags in the game (seek, volume, page drags) — the driver's `x11-input.py` would
   need a `drag` request.
3. A person at the keyboard: how the slide and the phone's place feel.

## Family instances in my data root (`target/e2e-ui`)

`f1201` 1.20.1, `f1204` 1.20.4, `f1211` 1.21.1, `f1214` 1.21.4, `f1215` 1.21.5, `f1218`
1.21.8, `f12110` 1.21.10, `f2612` 26.1.2, `f262` 26.2, `fab` 26.3 — signed Fabric 0.20.1
profiles without mods (`target-ui/scratch/install-families.sh`); `phone-shots.sh` writes
`width`/`height`/`memory_mb = 2048` and `[companion] force = true` into each instance.toml
(TESTED_MINECRAFT is still only 26.3) and quiet options into `persist/options.txt`.

## Learnt about this machine

- The private display is a headless sway with a **rootless** Xwayland: `ffmpeg -f x11grab -i :82`
  of the root window fails (BadMatch on GetImage; MIT-SHM fails across the sandbox). Capture the
  game's window by id: `xprop -root _NET_CLIENT_LIST` → `ffmpeg -f x11grab -window_id <id>`.
  The window is gamescope's (`WM_CLASS gamescope`), filling sway's 1280×720 output (1276×696);
  the game renders at the instance's width × height inside it, letterboxed.
- Stop a launcher by its PID with SIGINT (`pkill -f` patterns miss the relative path).

- Minecraft 26.3 key bindings (from options.txt) and Simple Voice Chat 2.6.27's defaults
  (M N H V, SDL scancodes 16 17 11 25 in its `KeyEvents` bytecode) are why the phone key is R.
