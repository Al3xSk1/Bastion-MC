# mod-platform: the companion mod on every Minecraft family, and the phone's protocol

Round 4, package **mod-platform** (docs/notes/round4-contract.md). Notes for the integrator
and for whoever owns README.md and THREAT_MODEL.md. Part 1 is user text, part 2 threat-model
text, part 3 how it works (and how to repeat the offline steps), part 4 what was verified —
by running, against stand-ins, or not at all — and part 5 what is open. The phone's screens,
the home screen and the dark menus themselves are mod-ui's (docs/notes/mod-ui.md); this
package is what they stand on.

Everything describes the tree at this branch's last commit.

---------------------------------------------------------------------------------------

## 1. For the README

### Minecraft versions

The companion mod — and with it the phone, the redone home screen and the dark menus — now
runs on every Fabric release from **1.20.1 to 26.3**. Minecraft's own code changed several
times in that span (how the GUI is drawn, how pictures become textures, how keys and the
mouse arrive, where the open screen lives); the mod has one adapter per *family* of
versions that are alike in what it needs, and picks the adapter when the game starts:

| Family | Versions | Checked in the real game on |
|---|---|---|
| `v1_20_1` | 1.20.1 | 1.20.1 |
| `v1_20_2` | 1.20.2 – 1.20.6 | 1.20.4 |
| `v1_21` | 1.21, 1.21.1 | 1.21.1 |
| `v1_21_2` | 1.21.2 – 1.21.4 | 1.21.4 |
| `v1_21_5` | 1.21.5 | 1.21.5 |
| `v1_21_6` | 1.21.6 – 1.21.8 | 1.21.8 |
| `v1_21_9` | 1.21.9 – 1.21.11 | 1.21.10 |
| `v26_1` | 26.1 – 26.1.2 | 26.1.2 |
| `v26_2` | 26.2 | 26.2 |
| `v26_3` | 26.3 | 26.3 |

The launcher adds the mod by itself to the versions in the last column
(`companion::TESTED_MINECRAFT`); for the other versions of a family, set `force = true`
under `[companion]` (the mod picks the right adapter; that exact version was not started
here). Versions outside every family (older than 1.20.1, newer than 26.3) get no adapter:
with `force` the mod loads, says in the game log that it has no adapter for that version,
and the phone, home screen and dark menus stay off.

**Zoom, the HUD readout and fullbright** are written for 26.3's classes and work there only;
on every other version they switch themselves off and say so once (launch log:
`… zoom: disabled: not available on Minecraft 1.21.4 (this feature is written for 26.3's
classes)`).

New settings under `[companion]` (also per instance in instance.toml):

```toml
phone = true                    # the in-game phone
phone_key = "key.keyboard.r"    # pulls it out and puts it away (Minecraft's key names, as zoom_key)
home = true                     # the redone home (title) screen
dark_menus = true               # darker backgrounds behind the game's own menus
```

How a launch says what happened (the lines that matter):

```text
[launcher] companion: bastion-companion 0.1.0 added for this session (…); features: zoom, hud, fullbright, phone, home, darkmenus; …
[game]     platform: Minecraft 1.21.4, family v1_21_2 (intermediary)
[game]     companion channel #1: hello: mod version 0.1.0, protocol 2, features: phone, home, darkmenus
[launcher] phone: 2 app(s) (spotify, browser), 1 connection(s) greeted, 14 command(s), 14 accepted
[launcher] companion channel: 1 connection(s), 1 hello, 3 diagnostic(s), 14 phone command(s)
```

**For testing only**: `BASTION_PHONE_DEMO=1` in the launcher's environment (or
`CompanionConfig::demo_phone` in a test) serves the phone with a demo service — fake
Spotify, browser, clips, Realms and settings, every command written to the launch log. It is
not a configuration key. The launch log says `phone: spotify — DEMO (a test switch): fake
state, nothing real behind it` for each app.

**Source map additions**

| Path | What |
|---|---|
| `java/companion/src/bastion/companion/platform/` | the Platform API (what the phone and the home screen are built on) and its version-independent core |
| `java/companion/platform/<group>/`, `…/<family>/` | the adapters: shared code of the intermediary families (1.20.1–1.21.11) and of the unobfuscated ones (26.x), and each family's own classes and mixins |
| `java/stubs/common/` | Fabric Loader, Mixin and ASM stand-ins (names and signatures only) |
| `java/stubs/minecraft/<family>/` | Minecraft stand-ins per family, generated offline (names and signatures only, never shipped) |
| `java/stubs/tools/` | how those are made again: `fetch.py`, `genstubs.py`, `regenerate.py`, `needed.txt`, `families.txt` |
| `src/phone/dispatch.rs` | the phone's dispatcher: routing, vocabularies, limits |
| `src/phone/demo.rs` | the demo service (a test switch) |

**Tests**

```sh
cargo test                                          # includes the platform's fakes, protocol 2, the dispatcher, the demo
# the mod in the real game, one family's representative at a time (demo mode, no account,
# on a display nobody looks at); BASTION_COMPANION_WORLD=1 also enters the demo world:
DISPLAY=:81 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=fab1214 \
    cargo test --test companion_game -- --nocapture
python3 -I java/stubs/tools/regenerate.py <download dir> --check   # the stubs are what Mojang's and Fabric's files give
```

---------------------------------------------------------------------------------------

## 2. For the threat model

### What changes

The companion channel is now **two-way** for the phone (protocol 2): the launcher answers,
and code in the game can ask the launcher's services to do things. Before, nothing received
changed what the launcher did. Now a command can — within the fixed vocabulary of the app it
names, and nothing more. This section is about the channel and the dispatcher; each service
(Spotify, the browser, clips, Realms, settings) has its own rows in its own package's notes.

**§3, the diagram.** The companion channel's hole gains a direction: *"1 socket to the
launcher (companion channel; Fabric profiles only): reports in; the phone's commands in, the
phone's state and pictures out"*. The launcher box: *"phone dispatcher: per-app vocabulary
and rate limits; holds no secret"*.

**§4.2, new row** (suggested P-D1; mod-ui's rows are P1–P6):

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| P-D1 | **The phone's channel as a lever on the launcher.** Code in the game (a hostile mod, an exploit in the JVM) (a) sends commands as if it were the phone; (b) floods the channel or sends malformed frames; (c) takes the phone's connection over; (d) reads what the launcher sends the phone; (e) makes the launcher work hard (pictures, state) or stall. | (a) Every command is parsed strictly (`phone::Command::parse`), routed only to an app of this launch by its exact name, refused unless its verb is in that app's fixed list (`PhoneApp::limits`), and refused when that verb's token bucket is empty; no argument is ever a path, a URL, a command line or code (each app's own rule, contract invariant 2). Refusals are counted, and each kind is logged **once**: a stream of different nonsense costs counters, not log lines (the name the game chose for an unknown app is never logged). (b) Frames from the mod are of two kinds only (COMMAND ≤ 1 KiB, DIAG ≤ 512 B); anything else — another kind, a length of 0 or over the limit — ends that connection at once (said once per session); at most one bounded frame is held per connection; all connections together: 50 frames and 200 KiB a second (frames over the rate are dropped and counted, reading pauses for bytes), plus the existing 4 connections at once, 64 per session, 64 KiB/s each. (c) The newest connection that says `hello 2` is the phone and gets what the services send; this is deliberate (the mod may reconnect) — see residual. (d) What is sent to the phone is state for display only: the contract's invariant 1 keeps every secret (tokens, the browser profile, the TOTP secret) out of the game, and the phone state is no more than the game could be shown. (e) Sending never blocks a service: the outbox is a bounded queue that drops (and counts) when full; the writer keeps only the newest state per app and picture per slot while the phone reads slowly, notices up to 32; a stalled game cannot stall a service or grow the launcher's memory beyond that. All of it runs in the launcher after it has confined itself (step 10). | `phone::dispatch::tests::*` (3), `companion::channel::tests::*` (protocol 2: 8 — the switch within one read, DIAG as a line, every malformed frame ending the connection once, the global rate, the byte budget, encoding and coalescing, a real socket with a takeover and a 1 MiB picture), `robustness::phone_frames_and_dispatch` and libFuzzer target `phone` (generated and mutated frames after a hello 2: same outcome however the bytes are cut, one bounded frame held, nothing reaches the app outside grammar, vocabulary and limits, nothing logged with a control character), `tests/companion_game.rs` in the real game on all ten families (commands in, state, notices and pictures out, every command as the demo received it in the launch log). | Any code in the game can send any command an app accepts, at the rate it accepts — the vocabulary and the limits are the defence, not the sender's identity (there is none). Any code in the game can take the phone over (the real phone then sees nothing until it reconnects) and can read everything the launcher sends the phone. A bug in the frame reader or the dispatcher is a bug in the trusted base, like the DNS or SNI parser: small (`Frames::feed`, `Session::received`/`frame`, `Dispatcher::route`), fuzzed, run confined. |

**The mod's side** (part of mod-ui's rows; recorded here because the reader is mine): the
mod reads what the launcher sends with the same strictness — frame lengths per kind (state
64 KiB, picture 4 MiB, notice 256 B), app names `[a-z0-9-]{1,16}`, a state must be a JSON
object (a reader bounded to 64 KiB and 32 levels), pictures decoded with the JDK's readers on
bytes in memory (no file cache), their announced size checked against the file's own before a
pixel is decoded, at most 2048 px a side and 4 Mpx; anything else closes the connection. The
mod trusts the launcher no less than before (it already gets its settings from it); the
point is that a launcher bug cannot crash the game. Verified by
`tests/companion.rs::the_mods_reader_of_the_launchers_frames_…` (4 000 mutated frames and
texts per run).

**§4.4, C7** (build inputs) — add: "The companion's adapters are compiled against stand-ins
of Minecraft's classes, one set per family of versions (`java/stubs/minecraft/<family>`):
names, descriptors, static-ness and supertypes of the ~50 classes and members the adapters
use, no code, no constant values. They were generated offline by `java/stubs/tools` from
Mojang's client jars and mappings and Fabric's intermediary mappings — each file checked
against the hash its publisher gives (SHA-1 from Mojang's version manifest, SHA-512 from
Fabric's maven) and its SHA-256 recorded in `java/stubs/minecraft/SOURCES.txt`;
`regenerate.py --check` repeats it and fails if anything differs. The build reads only the
committed stand-ins: no Minecraft jar, no mappings, no network. Code shared by several
families is compiled against each family's stand-ins and must produce the same class files,
or the build stops." Residual: "The stand-ins are as faithful as the generator: a member it
got wrong would compile and fail in the game — which is what the real-game test on each
family's representative is for."

**§5, not defended** — add: *"Versions within a family that were not started."* The mod
assumes every version of a family behaves like the representative that was run (their
stand-ins are identical by construction; their behaviour was not observed).

### Licence note

The stand-ins name Minecraft's classes and members (intermediary names for 1.20.1–1.21.11,
Mojang's for 26.x, Mojang's names in comments for readability) — a few hundred names, not the
mappings: Mojang's mapping licence allows using them for development and forbids
redistributing them complete and unmodified. They contain no code of Mojang's, are not
packed into the jar, and the generator's inputs (jars and mappings) are not committed.

---------------------------------------------------------------------------------------

## 3. How it works

### Families, and how they were established

From Mojang's version manifest: the releases between 1.20.1 and 26.3 are 1.20.1–1.20.6,
1.21–1.21.11, 26.1, 26.1.1, 26.1.2, 26.2, 26.3 (23). For each, `genstubs.py` writes the
stand-ins of what the adapters use (`needed.txt`, by Mojang's names) in the names the game has
at run time, and a `SIGNATURES.txt` of what they declare plus the methods the mixins inject
into. Versions with identical signatures form a family; `regenerate.py` checks that each
line of `families.txt` is exactly one such group (and that no two lines are). Where the
families split, and why:

| Change | Between | What the adapters do about it |
|---|---|---|
| `mouseScrolled(DDD)` → `(DDDD)`; `renderBackground(GuiGraphics)` → `(GuiGraphics, int, int, float)` | 1.20.1 / 1.20.2 | each family's host screen overrides its own signature |
| `Gui.render(GuiGraphics, float)` → `(GuiGraphics, DeltaTracker)` | 1.20.6 / 1.21 | the HUD mixin targets the family's descriptor |
| `blit` takes a `Function<ResourceLocation, RenderType>`; `NativeImage.setPixelRGBA` (ABGR) → `setPixel` (ARGB) | 1.21.1 / 1.21.2 | the family's canvas and texture |
| `DynamicTexture(NativeImage)` → `(Supplier<String>, NativeImage)`; `RenderPipelines` | 1.21.4 / 1.21.5 | the family's texture |
| GUI as render state: `pose()` a JOML `Matrix3x2fStack`, `drawString` returns nothing, `blit(RenderPipeline, …)` and `blit(id, x0, y0, x1, y1, u0, u1, v0, v1)`; `getGuiScale()` an int; `SoundManager.play` returns a result | 1.21.5 / 1.21.6 | the family's canvas and glue |
| input as records (`KeyEvent`, `MouseButtonEvent`, `CharacterEvent`); `isKeyDown(Window, int)`; `Button.onPress(InputWithModifiers)` | 1.21.8 / 1.21.9 | the family's host screen and glue |
| unobfuscated: Mojang's names at run time; `GuiGraphicsExtractor`, `extractRenderState` | 1.21.11 / 26.1 | the `mojang` group |
| the open screen lives on `Gui` (`gui.screen()`, `gui.setScreen`); the HUD is a `Hud` of its own | 26.1.2 / 26.2 | the family's glue and mixins |
| SDL 3 input (scan codes, SDL modifiers, 1-based buttons, `isKeyDown(int)`, text input started per window); `renderpearl` render API; `OptionsScreen(Screen, Options)` | 26.2 / 26.3 | the family's glue (`Sdl.java` translates to GLFW numbering) |

Within the intermediary group (1.20.1–1.21.11) names are stable — Fabric's intermediary keeps
a member's name across versions — so most adapter code is shared: `platform/intermediary`
(Base glue, host screen base, canvas base) compiles to the same class files against all seven
families' stand-ins, and the build checks exactly that. Each family adds four to eight small
classes. The 26.x group shares host screen, canvas and texture entirely.

The **representative** of each family (the version started in the real game) is the newest
or the most played: 1.20.1, 1.20.4, 1.21.1, 1.21.4, 1.21.5, 1.21.8, 1.21.10, 26.1.2, 26.2,
26.3.

### Repeating the offline steps

```sh
# downloads (each checked against its publisher's hash), generates the stand-ins of all 23
# versions, checks the family table, compares with java/stubs/minecraft (no changes made):
python3 -I java/stubs/tools/regenerate.py target/stubsrc --check
# without --check it rewrites java/stubs/minecraft/<family>/ and SOURCES.txt
```

`fetch.py` downloads into `<dir>/<version>/`: the version JSON (SHA-1 from Mojang's
manifest), the client jar and, up to 1.21.11, Mojang's client mappings (SHA-1 from the
version JSON), the JOML jar (SHA-1 from the version JSON), and Fabric's
`intermediary-<version>-v2.jar` (SHA-512 published next to it on maven.fabricmc.net). It
prints the SHA-256 of every file; `SOURCES.txt` holds them as they were on 2026-11-14 (105
files). The tools are plain Python 3 (standard library only; run with `-I`): a class-file
reader (`classfile.py`), the ProGuard and Tiny v2 readers (`maps.py`), and the generator.
Intermediary lists a method under the class that declares it first; the generator walks the
real class hierarchy of the jar to name overrides.

What a stand-in holds: kind, the nearest listed superclass and every listed interface
(flattened, so that `Screen`'s stand-in implements `GuiEventListener` directly), the listed
members with their descriptors, static-ness and visibility; fields without `final` (so that
javac inlines no constant that differs between versions), records and enums as plain types
where the adapters need no more, and a constructor taking `stubs.Marker` (a type only the
stand-ins have) so that stand-in subclasses compile.

### The build (`src/companion/jar.rs`)

In passes, each a separate `javac --release 17` with an explicit class path: (1) the common
stand-ins; (2) the version-independent sources (`java/companion/src`, which includes
mod-ui's code) against (1); (3) each family's stand-ins; (4) each group's sources against (1),
(2) and every family's stand-ins of the group — the outputs compared byte for byte, one copy
packed; (5) each family's sources against (1), (2), its group's classes and its stand-ins.
`families.txt` is an input (also compiled into `FamilyTable.java`, from which the mod picks
its family at run time). The jar stays reproducible (same bytes from two directories:
`companion::tests::building_the_jar_again_gives_the_same_bytes`) and holds only the mod's own
classes (about 480 KB now).

### The Platform API (`platform/`)

`Platform` (screens that do not pause the game and give back the screen underneath, the
home screen in place of the title screen, title-screen actions, the HUD layer, the menu
background, the phone key, textures, GUI scale, click sound, the render thread), `Overlay`
(input normalised to GLFW numbering on every version), `Canvas` (fill, rounded rectangles,
literal text and its width, images, nested clipping, a transform stack), `Pixels` (PNG/JPEG/
RGBA8 decoded off the render thread, with limits), `Texture`, `Link` (the phone's line:
`send`, newest state/picture per app, notices, a listener delivered on the render thread),
`Json`, `Commands` (the grammar, as the launcher's), `Platforms`. mod-ui builds only on these
(requests and answers: `docs/notes/r4-mod-requests.md`).

Underneath: `Core` implements `Platform` once, on a family's `Glue` (the operations that
name game classes); `Host` wraps one overlay for a family's host screen and contains its
failures (an exception closes that overlay, logged once); `Events` is where the mixins call
in. Per family, four hooks: `setScreen` (the title screen → the home screen; also the screen
underneath an overlay), `runTick` (start of each frame: the phone key, the link's
deliveries), the HUD's render method (TAIL), and the screen background (HEAD, cancellable:
the dark menus). `MixinPlugin` reads `minecraft=` from the launcher's settings and lets Mixin
apply only that family's mixins — so Mixin never looks for a class that is not there, and
the game log has no warning on any family.

The title screen's demo action is done by the game's own code: the adapter opens a real
title screen (without the home screen taking its place, once) and presses its "Play Demo
World" button — whatever each version's world loading looks like.

### Protocol 2

As the contract says, with two interpretations recorded under *contract changes* below
(DIAG's payload is protocol 1's whole line, `diag <feature> <text>`; the newest hello 2 is
the phone). The mod: one writer thread (bounded queue; ≤ 50 messages and 200 KiB a second
from its side), one reader thread, strict checks on everything read (part 2). The launcher:
`channel.rs` (`Frames`, the switch after the hello line within the same read, the global
budgets, the writer with coalescing, takeover) and `phone/dispatch.rs`.

### Launch wiring (`src/launch.rs`)

After the companion's plan and the log exist (before step 8): `services` (empty except the
demo, when its test switch is set; other packages add theirs here), each described in the
log; in step 9 each `confinement_needs` is applied to the launcher's `Confinement`; in step
10, after the confinement, `into_app` for each, the dispatcher with an outbox of 256, served
on the companion channel; at the end the phone's summary line before the channel's.

---------------------------------------------------------------------------------------

## 4. What was verified, and how

Machine: Arch Linux, kernel 7.2.14, OpenJDK 27.0.1 (javac and the game's JVM), bubblewrap,
Landlock ABI 10; Fabric Loader 0.20.1 (Mixin 0.17.5 = 0.8.7) on every version; the game in
demo mode (no account) inside the launcher's nested compositor (gamescope) on a private,
invisible X display (`:81`, headless sway with Xwayland, `tests/support/private-x11.sh`).

### In the real game — every family's representative

`tests/companion_game.rs` through the whole launch path (a signed Fabric profile per
version in `target/e2e`, created and installed by the launcher itself: `profile new … --fabric
--loader-version 0.20.1`, `profile sign`, `install`; `force = true` in each instance.toml
while the version was not yet listed), every feature on, the demo service serving the
phone, and the mod's self-test. Asserted on every family:

* the launcher's `companion:` line (version, SHA-256, features); Fabric Loader lists the mod;
* **no line from Mixin or about the mod at warning level or above**, no mention of the mixin
  configuration or classes, no `platform (…) … failed` line;
* the family the mod picked is the one `families.txt` names for the version;
* **hello of protocol 2** with the features that exist on that family (all six on 26.3;
  phone, home, darkmenus elsewhere, and one diagnostic per convenience that is not there);
* **the phone state at the hello**: `{"apps":["spotify","browser","clips","realms","settings"],"theme":"dark"}`;
* commands from the mod (`settings sync-now`, `browser open discord`, `browser size 200
  120`, `browser frames on`, `browser text -- hi phone`, `browser mouse down 50 60 left`),
  each in the launch log as the demo received it; three lines outside the grammar refused by
  the mod itself and never sent; answers back: the settings notice, the browser's state
  (`open: discord`) and a 200×120 RGBA8 page, the album art (128×128 PNG) — all decoded in the
  game, the page uploaded as a texture and drawn;
* **the home screen** in place of the title screen (the open screen is the family's host
  screen, `home=true`), demo mode seen, "Play Demo World" available, the phone key's name;
* **an overlay** opened, drawn every frame with every primitive (fill, rounded rectangle,
  outline, clip, transform, literal text with `§`, a texture made from pixels and one from
  the album art), text measured (36 units for "Bastion", line height 9 everywhere), closed
  again with the screen underneath back;
* the phone summary `phone: 5 app(s) (…), 1 connection(s) greeted, 6 command(s), 6
  accepted` and the channel summary `…, 6 phone command(s)`;
* **a screen of the game's own** (its options, opened through the home screen's `OPTIONS`
  action): its background painted by mod-ui's dark menus through the family's background
  hook (about 60 times a second on every family), then back to the home screen;
* with `BASTION_COMPANION_WORLD=1`: the demo world entered **through the platform's own
  action** (the title screen's button, pressed by the adapter) and playing for four seconds,
  the HUD hook drawing in every frame (`hud_frames_in_1s` ≥ 1); on 26.3 additionally all of
  round 3's world measurements;
* with `BASTION_COMPANION_KEYPRESS=1` as well: **the phone key really pressed** (`r`, through
  the X server's XTEST extension on the private display, into the nested compositor) while
  playing — the family's own keyboard state (`isKeyDown(window, code)`, `isKeyDown(Window,
  code)`, SDL's `isKeyDown(scancode)` on 26.3) saw it and mod-ui's phone opened (a host screen
  of the family, not the home screen); on 26.3 also round 3's zoom presses.

Runs (2026-11-14, display `:81`, nested compositor; from the TESTED_MINECRAFT commit on the
launcher added the mod by itself, earlier runs had `force = true` in the instance):

| Family | Version | Title mode | World mode | World + key presses | Title mode, final jar (with the options/background check) |
|---|---|---|---|---|---|
| v1_20_1 | 1.20.1 | passed (2 runs) | passed | passed | passed |
| v1_20_2 | 1.20.4 | passed | passed | passed | passed |
| v1_21 | 1.21.1 | passed | passed | passed | passed |
| v1_21_2 | 1.21.4 | passed | passed | passed | passed |
| v1_21_5 | 1.21.5 | passed | passed | passed | passed |
| v1_21_6 | 1.21.8 | passed | passed | passed | passed |
| v1_21_9 | 1.21.10 | passed | passed | passed | passed |
| v26_1 | 26.1.2 | passed | passed | passed | passed |
| v26_2 | 26.2 | passed | passed | passed | passed |
| v26_3 | 26.3 | passed (3 runs) | passed | passed | passed |

No run failed except two on 26.3 at the very start, both defects found and fixed: the
launcher's phone writer stalled after its first burst (nothing sent after the hello
arrived — the fix is in commit 3726815), and the test read `families.txt` after the launch
had confined it. The jar differed between the rounds (mod-ui kept working on its part in the
same clone; the last round's jar: SHA-256 `e4591dbd…`, 485 362 bytes on 26.3's run). The
mod-ui screens drawn in these runs were whatever was in the tree at the time; what this
package asserts is the platform underneath them.

### Without Minecraft — part of `cargo test`

| What | Test |
|---|---|
| The jar: built in passes, group classes identical across their families, reproducible, only the mod's classes, every family's mixins listed for the plugin | `companion::tests::*` (building again, the embedded jar, describing itself, the forbidden-API scan) |
| Settings: the new keys, the version line (written only when it cannot break out of its line), the phone key validated like the zoom key, the decision with the phone alone | `companion::tests::the_settings_file_…`, `…settings_are_validated…`, `…only_fabric_profiles…` |
| Protocol 2 in the launcher | `companion::channel::tests::*` (22 in all) |
| The dispatcher | `phone::dispatch::tests::*` |
| The demo: five apps, the phone state, every verb answered and logged, the PNG encoder read back | `phone::demo::tests::*` |
| The phone fuzz harness | `robustness::phone_frames_and_dispatch` (libFuzzer target `phone` compiles with the others; never run — no nightly toolchain here) |
| The mod's reader of launcher frames and its JSON reader | `tests/companion.rs::the_mods_reader_of_the_launchers_frames_…` |
| The mod with protocol 2 in a plain JVM, with the stand-in game, and in the real sandbox | `tests/companion.rs` (the round-3 tests, now reading the mod's frames) |
| The stand-ins are what the published files give | `java/stubs/tools/regenerate.py --check` (run by hand; needs the network for the downloads) |

### Only against stand-ins

* **The phone's services**: only the demo service. What the real Spotify, browser, clips,
  Realms and settings services send is their packages' to verify; the channel and the
  dispatcher carried the demo's state, notices and PNG and RGBA8 pictures in the real game.
* **JPEG pictures** were decoded in the JVM by the robustness harness (made by the JDK's own
  writer), not in the game (the demo sends none).

### Not verified

* **Versions of a family other than its representative** (their stand-ins are identical to
  the representative's, by construction; they were not started).
* **Typing and clicking as devices** in my runs: the phone key was really pressed (XTEST) on
  every family; characters, mouse clicks and the wheel were not (the self-test drives the
  overlay through the platform). mod-ui's `tests/support/phone-shots.sh` types and clicks
  through XTEST on its own display; typing on 26.3 depends on R4 (SDL text input), which I
  made but did not see typing through.
* **The Wayland path, the game as a direct X11 client**: only the nested compositor.
* **Fabric Loader versions other than 0.20.1**, Quilt.
* **The browser's frame rate** (15 pictures a second through the channel and into a texture
  each): the path works (one 200×120 page); its cost at 15/s was not measured. RGBA8 is copied
  pixel by pixel into the game's image on every family.

---------------------------------------------------------------------------------------

## 5. Open, and for the integrator

* **Contract changes** (also in the hand-in): `src/phone/mod.rs` gained `pub mod demo;` and
  `pub mod dispatch;`; DIAG's payload is protocol 1's whole `diag <feature> <text>` line;
  the newest connection that says hello 2 is the phone (takeover); the demo is five services
  (one app per service, as `Service::into_app` returns one app); the demo's switch is
  `CompanionConfig::demo_phone` (serde-skipped) or `BASTION_PHONE_DEMO=1`;
  `companion::Plan::serve` takes the phone (`Option<channel::Phone>`), and
  `channel::Endpoint::serve_phone` exists; the mod's settings file gains `phone`,
  `phone.key`, `home`, `darkmenus` and `minecraft=<version>`; the Java `Link` has a
  render-thread listener (mod-ui's request) next to polling.
* **Other packages' services** go into the `services` block of `launch.rs` (one block each);
  nothing else is needed for their apps to be served.
* **Zoom, HUD readout and fullbright on the other families**: not ported (they reach the game
  by 26.3's names). Porting means a family mixin for each hook and the lookups in names of
  that family; the stand-ins and the plugin are in place for it.
* **The front end** shows the phone's lines like any log line; no event of its own.
* `examples/config.toml` gained the four keys; the comment there still says which versions
  get the mod — it names the tested list now.
