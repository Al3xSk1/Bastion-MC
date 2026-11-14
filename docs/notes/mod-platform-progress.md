# mod-platform: hand-over file

Package mod-platform of round 4 (docs/notes/round4-contract.md). Clone
`target/agents/r4-mod`, branch `work`, shared with mod-ui (they own `**/ui/`, `**/phone/`,
`**/home/`, `**/theme/` under `java/companion/src/bastion/companion/`). Private display `:81`,
runtime directory `/run/user/1000/bmc-r1`, data root `target/e2e`. The deliverable notes are
`docs/notes/mod-platform.md`; requests between the two packages `docs/notes/r4-mod-requests.md`.

## State: complete as briefed (2026-11-14)

All seven points of the brief are done and verified; what remains is in "Open" below.

## Done (and how it was checked)

1. **Platform API** (`java/companion/src/bastion/companion/platform/`): `Platform`, `Overlay`,
   `Canvas`, `HudLayer`, `MenuBackground`, `HomeAction`, `HomeLabels`, `Keys`, `Pixels`,
   `Texture`, `Link`, `Json`, `Commands`, `Platforms`; core: `Core`, `Glue`, `Host`, `Hosted`,
   `Events`, `Families`, `MixinPlugin`. Test stand-in: `tests/java/FakePlatform.java`. mod-ui's
   requests R1–R5 answered and done (R4: SDL text input on 26.3; R5: the writer stall, fixed).
2. **Families** (10, `java/stubs/tools/families.txt`): established from Mojang's manifest,
   Mojang's mappings and Fabric's intermediary with `java/stubs/tools/genstubs.py` +
   `regenerate.py` (groups versions by identical stand-in signatures, checks the table).
   `regenerate.py target/r4mod/src --check` passes (inputs in `target/r4mod/src`, hashes in
   `java/stubs/minecraft/SOURCES.txt`).
3. **Hermetic build in passes** (`src/companion/jar.rs`): common → version-independent →
   per-family stand-ins → group sources checked byte-identical across the group's families →
   family sources. Reproducible (unit test).
4. **Adapters**: group `intermediary` (1.20.1–1.21.11) + 7 families; group `mojang` (26.x) +
   3 families; mixins per family chosen by `MixinPlugin` from `minecraft=` in the settings.
5. **Existing features**: zoom/HUD readout/fullbright stay on 26.3 (verified unchanged in the
   real game); elsewhere they say once (information level) that they are not available.
6. **Protocol 2**: Java `Channel` (hello line, framed both ways, strict reader, budgets);
   Rust `channel.rs` (switch after hello 2, `Frames`, global 50 frames/200 KiB per second,
   writer with coalescing and takeover), `phone/dispatch.rs`, `phone/demo.rs`, launch wiring
   (services before step 9, `confinement_needs`, `into_app` after it). `[companion]` keys
   `phone`, `phone_key`, `home`, `dark_menus`; settings line `minecraft=`.
7. **Tests**: unit (`companion::*`, `companion::channel::*`, `phone::*`), integration
   (`tests/companion.rs`, incl. the Java robustness harness of the mod's frame/JSON readers),
   robustness + fuzz target `phone`, the platform core on a stand-in family
   (`PlatformCoreCheck`), and **the real game on every family's representative — title mode,
   world mode, world mode with the phone key really pressed (XTEST), and a last title round
   with the menu-background check — all passed** (table in the notes, part 4).
   `TESTED_MINECRAFT` = those ten versions; the instances no longer carry `force`.
8. fmt clean, clippy without warnings, `cargo test --workspace --no-fail-fast` green except
   mod-ui's `tests/phone_ui.rs` (its check ran out of heap — theirs, told in U1).

## How to run things here

* Private display: `tests/support/private-x11.sh /run/user/1000/bmc-r1/x &` then
  `ln -sfn /run/user/1000/bmc-r1/x/x11/X0 /tmp/.X11-unix/X81`.
* Launcher on the data root: `target/r4mod/bmc.sh <args>` (sets the XDG variables to
  `target/e2e`, unsets the displays).
* A representative instance: `target/r4mod/mkinst.sh <id> <version>` (profile new with
  Fabric Loader 0.20.1, lock, sign with `target/e2e/signing.key`, memory 2560, 480×270,
  install). Existing: `fab1201 fab1204 fab1211 fab1214 fab1215 fab1218 fab12110 fab2612 fab262`
  and round 3's `fab` (26.3).
* Real-game test, one instance: `target/r4mod/game.sh <id> [world]`; several in a row with a
  copied test binary: `[WORLD=1] target/r4mod/games.sh <id>...` (logs `target/r4mod/run-*.log`).
* Quick compile of the mod like the build: `target/r4mod/jc.sh`.

## Learnt about this machine

* JDK 27.0.1 only; Minecraft 1.20.1 (LWJGL 3.3.1) runs on it (sun.misc.Unsafe still allowed,
  with a warning). `[java] match = "newer"` in the data root's config.
* 26.3 reads input through SDL 3 (scan codes, `InputConstants.KEY_A = 4`), 26.1/26.2 still
  GLFW (`KEY_A = 65`).
* A launch confines the test process (Landlock): read everything a test needs (like
  `families.txt`) before calling `launch`.
* The game log is log4j XML: a WARN event and its message are on separate lines.
* Real-game runs take 17–60 s each here (nested gamescope on headless sway).
