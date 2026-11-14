# The companion mod

`bastion-companion` is a small Fabric client mod that belongs to the launcher. It is
compiled from source together with `bastion-mc`, embedded in the launcher binary, and
added by the launcher to Fabric profiles for one session at a time: your signed `mods/`
directory is not touched and stays exactly what you signed. It gives the game three
conveniences — zoom, a HUD read-out and fullbright. Like the launcher, it is pre-release.

**It is not a security feature, and it enforces nothing.** It runs inside the game's JVM
next to every other mod, and any of them can change it, imitate it or switch it off.
Nothing the launcher promises depends on it, nothing it shows in the game deserves more
trust than the game itself, and nothing it tells the launcher is acted on: the launcher
writes it into the launch log, marked as coming from the game. Everything that protects
you is in the launcher, outside the sandbox. What the mod changes in the threat model is in
[`THREAT_MODEL.md`](THREAT_MODEL.md), rows J19, J2, C2 and C7 and §5.

## What it does

| Feature | Default | What it does |
|---|---|---|
| Zoom | on | Hold **C** to zoom in: the field of view is divided by 4. While you hold it, the mouse wheel changes the magnification (one notch: × 1.15, between 1 and 50) and does not switch the hotbar slot; letting go forgets the change. Zooming in and out is eased over about a fifth of a second, and the mouse turns the view proportionally slower while zoomed (by the ratio of the tangents of the two view angles), so aiming feels the same. With a screen open — chat, inventory, a menu — the key is just a letter: typing "c" does not zoom. |
| HUD read-out | off | In a world, three lines in the top-left corner: frames per second, your position and the direction you face. Hidden with the HUD (F1) and whenever the game's own debug overlay shows something (F3, or debug entries you set to show always). Where reduced debug information is on — your own option, or a server asking for it — position and facing are withheld, as they are on the debug screen, and the read-out is the frame rate alone. |
| Fullbright | off | Dark places are drawn as if the brightness slider went far past its end. It raises the number the game computes for its light map from the Brightness option, so the option itself, `options.txt` and the Darkness effect's own dimming stay as they are. **Some servers do not allow this.** It changes only what your client draws, but a server's rules may still forbid it; that is yours to check before you switch it on. |

A feature that cannot find the part of the game it needs switches itself off for the
session, says so once (in the game's log and on the channel to the launcher, below), and
leaves the game alone; the other features keep working. The mod's own log output is capped
at 120 lines per session.

### Keys

Key names are Minecraft's own, as in `options.txt`: `key.keyboard.c`, `key.keyboard.z`,
`key.keyboard.left.alt`, `key.keyboard.f6`, `key.keyboard.keypad.0` … The leading
`key.keyboard.` may be left out (`"c"`, `"left.alt"`). Mouse buttons are not supported.

The launcher checks only the shape of the name (lower-case letters and digits in parts
separated by dots, at most 40 characters, not `unknown`); whether the game knows a key of
that name is for the game to say. The mod asks the game the first time the key matters, and
if the game knows no keyboard key of that name, zoom switches itself off and says so.

The game has uses of its own for **C**; in 26.3's `options.txt` three bindings name it
(checked in the test instances' `options.txt`):
saving a hotbar in creative mode (`key.saveToolbarActivator`, together with a number key)
and, together with the debug key F3, copying your location and a deliberate crash
(`key.debug.copyLocation`, `key.debug.crash`). Zoom does not take the key away from them;
pick another key if they get in your way.

## Settings

The settings are the launcher's, under `[companion]` in `~/.config/bastion-mc/config.toml`.
Every key is optional; the values below are the defaults (`bastion-mc config example`
prints them with comments).

| Key | Default | Values | What it decides |
|---|---|---|---|
| `enabled` | `true` | `true`, `false` | Whether the mod is added to Fabric instances at all. `false` keeps it out. |
| `force` | `false` | `true`, `false` | Also add it to Minecraft versions it was not tested with (see below). |
| `zoom` | `true` | `true`, `false` | Hold a key to zoom. |
| `zoom_key` | `"key.keyboard.c"` | a keyboard key by Minecraft's name for it | The zoom key. |
| `zoom_factor` | `4.0` | 1.1 to 50 | The field of view is divided by this. |
| `zoom_wheel` | `true` | `true`, `false` | The mouse wheel changes the factor while the key is held. |
| `hud` | `false` | `true`, `false` | The HUD read-out. |
| `fullbright` | `false` | `true`, `false` | Fullbright. Some servers do not allow this. |
| `fullbright_gamma` | `15.0` | 1 to 100 | What the game's brightness (0 to 1; its own slider ends at 1) is raised to. |

```toml
[companion]
enabled = true
force = false
zoom = true
zoom_key = "key.keyboard.c"   # "c" works too
zoom_factor = 4.0
zoom_wheel = true
hud = false
fullbright = false            # some servers do not allow this
fullbright_gamma = 15.0
```

Any of these keys can be set for one instance only, under `[companion]` in that instance's
`instance.toml` (`~/.local/share/bastion-mc/instances/<id>/instance.toml`); what is not
set there comes from `config.toml`:

```toml
[companion]
zoom_key = "left.alt"
hud = true
```

In both files an unknown key, a key name of the wrong shape or a value outside its range is
a configuration error, and the launcher stops (exit status 3) instead of guessing.

There is no settings screen in the game, on purpose. The launcher hands the game the merged
settings as a read-only file, written for that launch:

```text
zoom=true
zoom.key=key.keyboard.c
zoom.factor=4
zoom.wheel=true
hud=false
fullbright=false
fullbright.gamma=15
channel=/bastion/companion/channel
```

Nothing in the game can change that file for the next session, and the mod has no way of
writing a file at all (a test scans its class files for one). To change a setting, edit the
launcher's file and start the game again.

## When it is added, and how you can tell

The launcher decides at every launch, in this order:

1. `enabled` is on;
2. the instance is a signed **Fabric** profile (`bastion-mc profile new <id> --minecraft
   <version> --fabric`). A vanilla instance cannot load mods; Quilt profiles are not
   supported;
3. at least one feature is on;
4. the Minecraft version is one the mod was tested with — **26.3** at present — unless
   `force = true`;
5. the signed profile does not set `-Dfabric.addMods=` itself. Its loader options come after
   the launcher's own on the Java command line and would win, so the launcher leaves the
   profile exactly as signed and adds nothing;
6. the game's Java is 17 or newer.

Every launch says what happened, in the launch log and on the terminal or in the front
end's console. From a demo launch of the signed Fabric profile on this tree (on the
development machine, 2026-11-13):

```text
[launcher] companion: bastion-companion 0.1.0 added for this session (sha256 a5e1857f63c79f37a7cc22fd9265457124b4f9c4286e71aa5883d9c3c98cc782, 67867 bytes; read-only, outside the signed mods directory); features: zoom; channel to the launcher: on (what arrives is logged, never acted on). It is a convenience and enforces nothing.
[game]  - bastion-companion 0.1.0          ← in Fabric Loader's list of the mods it loads
[game] companion channel #1: hello: mod version 0.1.0, protocol 1, features: zoom
[launcher] companion channel: 1 connection(s), 1 hello, 0 diagnostic(s)
```

Lines that start with `[game] companion channel` are what *something inside the game*
told the launcher; the launcher writes them down and does nothing else with them. With
`force` on an untested version the first line has `; Minecraft <version> is untested
(companion.force)` before its closing sentence. When the mod is left out, the line is
`companion: not added — ` and the reason:

| Reason in the log | Why |
|---|---|
| `switched off (companion.enabled = false)` | `enabled = false` |
| `this instance has no mod loader (…)` | a vanilla instance, or a profile without a loader |
| `the profile's loader is Quilt; only Fabric is supported` | a Quilt profile |
| `every feature is switched off` | `zoom`, `hud` and `fullbright` are all off |
| `Minecraft <version> is not a version it was tested with (26.3); …` | not tested, and `force` is off |
| `the signed profile sets fabric.addMods itself, and the launcher does not alter it` | step 5 |
| `it needs Java 17 or newer; this launch uses Java <n>` | step 6 |

## How it reaches the game

For a launch that gets the mod, the launcher adds four things to the sandbox it assembles
(`src/companion/mod.rs`) and nothing else:

* the jar, as a read-only file `/bastion/companion/bastion-companion.jar` (mode 0444) that
  exists only inside the sandbox — outside the game directory and outside `mods/`, which
  stays a sealed directory of exactly the jars the signature covers;
* the settings above, as a read-only file `/bastion/companion/config.properties`;
* the channel's socket, bound read-only at `/bastion/companion/channel`, and that one path
  in the sandbox's Landlock list of sockets it may connect to;
* the Java option `-Dfabric.addMods=/bastion/companion/bastion-companion.jar`, which makes
  Fabric Loader take the jar as one more mod. (What the loader does with that property was
  read from Fabric Loader 0.20.1's bytecode with `javap`: the value is split at `:`, an entry
  starting with `@` names a file listing paths, a path that does not exist is skipped with
  a warning. The launcher passes exactly one path of its own.)

Nothing is added to what the sandbox may write or execute, and no network destination. The
jar and the settings are materialised for each launch, from the launcher binary and its
configuration, and not kept: nothing the game writes reaches the next session's copy.

**Where the launcher's settings file exists, the mod reads no other.** The mod also knows a
Java property, `bastion.companion.config`, that names a settings file — that is how the
tests run it in a plain JVM without a launcher. It is consulted only when
`/bastion/companion/config.properties` does not exist, which never happens in a session the
launcher set up. The order matters: a signed profile's loader options are `-D` properties of
its author's choosing, and one that pointed the mod at a file inside the game directory
would let anything in the game rewrite the settings for later sessions (fullbright switched
on, say, on a server that forbids it). A test runs the mod in the real sandbox with exactly
such a planted file and property, and the launcher's settings are the ones in force
(`a_jvm_in_the_sandbox_runs_the_mod_on_the_launchers_settings_not_on_a_file_the_game_wrote`
in `tests/companion.rs`). If the launcher's file is there but cannot be read, the mod has no
settings and every feature stays off.

## The channel to the launcher

One UNIX stream socket, from the mod to the launcher, carrying information and nothing
else (`src/companion/channel.rs`, `Channel.java`). Protocol version 1, one message per line,
fields separated by single spaces:

```text
hello <protocol> <mod version> <feature>[,<feature>…]      "-" for no feature
diag <feature> <free text>
```

`<protocol>` is 1 to 4 digits, `<mod version>` 1 to 32 of `A–Z a–z 0–9 . + -`, a feature 1 to
32 of `a–z 0–9 _ -` starting with a letter, at most 16 features. The mod sends its hello once
its features are set up, then one `diag` line for each feature that switched itself off
(`diag zoom disabled: lookup failed (…)`). Sending never holds up the game: a queue of 64
lines and a thread of its own; with no channel, or none reachable, the mod says so once in
the game's log and works the same.

**What the launcher does with it: it logs it, and never acts on it.** Any mod in the same
JVM can connect to the socket and say anything; a hello proves nothing about who sent it.
So every line is treated as hostile input and bounded:

| Bound | Value |
|---|---|
| Length of a line | 512 bytes (a `\r` before the `\n` counts); a longer line is dropped as it arrives, without being buffered |
| Connections | 4 open at the same time, 64 in a session; further ones are closed unread |
| Reading | 64 KiB per second per connection |
| Logging | 20 lines at once, then 2 per second, 2000 in a session, for all connections together; the rest is counted |
| Free text of a `diag` line | sanitised (no control or bidi characters) and cut to 200 characters |
| Lines that are not part of the protocol | the first three are quoted (sanitised, 80 characters), the rest only counted |

At the end of the session the launcher writes one summary line with the counts.

The socket is created before the sandbox starts, in a new private directory
(`$XDG_RUNTIME_DIR/bastion-mc/companion-<12 hexadecimal digits>/channel`, or under the
launcher's cache directory where there is no runtime directory), and its name is removed as
soon as the sandbox holds it, before the launcher confines itself. In the demo launch above
`$XDG_RUNTIME_DIR/bastion-mc/` was empty while the game ran and afterwards. Before that —
from the moment the launcher prepares the sandbox, through the sign-in, the resource-limit
probe and the start of the nested compositor: seconds, not an instant — the name exists in
that private directory, where other programs running as you could connect and be logged as
if they were the game. A launch that stops with an error before the launcher has confined
itself removes the name on its way out. The name stays if the launcher cannot establish
that the program in the sandbox exists (it then confines itself and can no longer remove
it — and says so, with a warning in the log and to the front end), or if the launcher is
killed or crashes before that point; a later launch that adds the companion then removes
it (a socket at least a minute old on which nobody listens). The launcher starts reading
the socket only after it has confined itself — like every other input from the game; a
test in the default suite (`tests/companion.rs`) serves and reads the channel from a
process confined exactly that way, after the name was removed.

Version 1 has no message from the launcher to the mod, and no message that could ask the
launcher for anything. A design for approval prompts that would use the channel — with the
rule that a decision is made and entered outside the sandbox, never in the game — is in
`docs/notes/companion-mod.md` (part 2). It is not built.

## How it is built

`build.rs` compiles the mod with the system `javac` (`JAVAC` overrides it, as for the launch
bootstrap class) by the recipe in `src/companion/jar.rs`, which the tests use to build it
again and compare. Then the jar is embedded in the launcher binary.

* **Inputs:** `java/companion/src/**.java`, `java/companion/resources/*`
  (`fabric.mod.json`, the mixin list), `java/stubs/**.java`, `LICENSE`, the package version,
  and `javac`. Nothing else: `-proc:none -implicit:none`, an empty source path, an explicit
  class path, and `JDK_JAVAC_OPTIONS`, `JAVA_TOOL_OPTIONS`, `_JAVA_OPTIONS` and `CLASSPATH`
  removed from the compiler's environment. No network, no Gradle, no annotation processor,
  no additional crate: the archive is written by about 70 lines of the launcher's own code.
* **No Minecraft at build time.** The mod's sources name no game class. Its mixins name their
  targets as strings, and everything else is looked up by name when the game starts
  (`Game.java`). Mojang's jar is not needed to build the mod and nothing of it is shipped.
* **Stand-ins** (`java/stubs`): `net.fabricmc.api.ClientModInitializer` and six Mixin types
  (`Mixin`, `Inject`, `At`, `Coerce`, `CallbackInfo`, `CallbackInfoReturnable`), reduced to
  the names and signatures the mod uses. They are on the compiler's class path and never in
  the jar. An opt-in test compiles the mod against the real Fabric Loader 0.20.1 and Mixin
  jars instead and gets the same bytes, and compares every member a stand-in declares with
  the real class.
* **Reproducible:** sources in sorted order; `-g:none` (no source file names, no line or
  variable tables); `--release 17` (class-file version 61); stored entries, the manifest
  first and the rest by name, the time stamp 1980-01-01 00:00:00, no extra fields, comments
  or directory entries. The same sources and the same `javac` give the same bytes in any
  directory (`companion::tests::building_the_jar_again_gives_the_same_bytes`). A different
  `javac` release may emit different bytecode, so a published hash means something only
  next to the JDK version. On this tree, with `javac` 27.0.1: SHA-256
  `a5e1857f63c79f37a7cc22fd9265457124b4f9c4286e71aa5883d9c3c98cc782`, 67 867 bytes — the
  launch log names the hash and size at every launch.
* **Requirement:** a JDK that can target Java 17, i.e. JDK 17 or newer (the Arch package's
  `makedepends` has `java-environment>=17`). An older one stops the build with "the companion
  mod needs JDK 17 or newer to build".

The mod is 1 780 lines of Java including comments, 411 of them the self-test that only the
launcher's own real-game test can switch on. Anyone who signs a profile can read it in
`java/companion`.

## Adding a tested Minecraft version

The list is `companion::TESTED_MINECRAFT` in `src/companion/mod.rs`, today `["26.3"]`. To
add a version:

1. install it as a signed Fabric profile in a test data root;
2. run the real-game test against it, with `force = true` in that instance's `instance.toml`
   while the version is not yet listed:

   ```sh
   DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=<profile> \
       cargo test --test companion_game -- --nocapture
   ```

   and again with `BASTION_COMPANION_WORLD=1` (on a display nobody is looking at);
3. if every feature reports in (`features: zoom, hud, fullbright` in the hello, and no
   `disabled:` line) and the test passes, add the version to the list and to the comment in
   `examples/config.toml`;
4. if a name has changed, the feature says which lookup failed; fix it in that feature's
   `resolve`, or make the lookup try both names.

The mod finds the game by the names Minecraft has shipped unobfuscated since 26.1. On
older, obfuscated versions every lookup fails and every feature switches itself off;
`force` does not make it useful there.

## What was verified, and what was not

On the development machine (Arch Linux, kernel 7.2.14, OpenJDK 27.0.1, bubblewrap 0.13.1,
Landlock ABI 10), with Minecraft 26.3 in demo mode (no account), Fabric Loader 0.20.1 with
Mixin 0.8.7, Fabric API 0.166.2 and gamescope 3.17.0. The engineers' record of every run is
`docs/notes/companion-mod.md`, part 4.

**Without Minecraft**, in every `cargo test`: what the embedded jar contains (only the mod's
own classes and resources, class-file version 61, no debug information); that two more
builds give the same bytes; that the mod names no API for networking beyond the one UNIX
socket, for starting processes, loading native code, defining classes or writing files; the
settings and their validation; the injection decision and what an injection adds; the
channel's grammar and limits (14 unit tests and a generated-input harness,
`robustness::companion_channel_reader`); the mod in a JVM with no game at all (every feature
off, said once); its logic against a stand-in "game" in `tests/java/fakegame` (zoom
arithmetic and easing, no zoom with a screen open, the wheel, the HUD text and when it is
hidden, fullbright under the Darkness effect, a feature whose method has changed). In the
real sandbox — bubblewrap, Landlock, seccomp —: the two files are there with the right bytes
and nothing is writable, creatable, removable or executable; no socket can be bound next to
the channel; and the system JVM runs the mod on the launcher's settings, not on a file the
program planted, with its hello reaching the launcher through Landlock and seccomp.

**In the real game** (`tests/companion_game.rs`, opt-in): the whole launch path with the
signed profile, every feature on, and the mod's scripted self-test. Asserted from the
launch log: the launcher's `companion:` line, Fabric Loader's listing of the mod, no
warning from Mixin, the hello (received while the launcher was confined), the title screen
reached. Measured at the title screen and, with `BASTION_COMPANION_WORLD=1`, in Mojang's
demo world:

* zoom: field of view 70 → 17.5 (70 / 4), back to 70 on release; for the same mouse movement
  the game turned the player 30.0° unzoomed and 6.5944° zoomed (ratio 0.2198, the tangent
  rule); one wheel notch while zoomed took the factor from 4.000 to 4.600 and left the
  hotbar slot alone, a notch after the key was released moved the hotbar slot (0 → 8);
* HUD read-out: the three lines `62 fps`, `XYZ -43.5 / 70.0 / 63.5` and `facing north (-Z),
  yaw -140.4, pitch 0.0` (the game's own `getDirection()` agreed); nothing drawn with the HUD
  hidden or the debug overlay up; the frame rate alone under reduced debug information (the
  player's own option);
* fullbright: the light map's brightness input 15.0 instead of 0.5 in the running world;
* five seconds with nothing scripted, playing and with the chat open: nothing zoomed, and
  nothing was typed.

The display was the launcher's choice, as in any launch, and all three of its paths were run:
the nested compositor (gamescope is installed on that machine), the game as a direct X11
client (`BASTION_COMPANION_DIRECT_X11=1`), and the Wayland security context on a headless
sway. With `BASTION_COMPANION_KEYPRESS=1` the key was really pressed through the X server's
XTEST extension, on a private display nothing else used: pressed while playing, the view
zoomed and came back on release; pressed with the chat open, nothing zoomed and the letter
was typed. On the final jar (SHA-256 `a5e1857f…`) the third engineer ran the test twelve
times — title and world modes, the nested compositor with the installed gamescope and with
another copy of it, the direct X11 client, the Wayland path, and three key-press runs — and
all passed; after the last change to the launcher's channel code the fourth engineer ran
title and world mode once more, both passed with the same measurements.

One deliberately broken build (not committed) checked what Mixin does with a mixin whose
target is missing: the game started and reached the title screen, zoom said once that it
was off, the other two features worked. For a missing *method* Mixin logged nothing at all,
which is why each feature checks its target itself and why the test measures each hook's
effect instead of trusting that it was applied.

**On the merged tree, for this document** (2026-11-13): one demo launch of the signed Fabric
profile through the nested compositor on a private display, with the default settings. The
lines shown above; Fabric Loader listed 50 mods, one more than the 49 the same profile
loaded before the companion existed (§7 E16 of the threat model); the launcher was
confined (`Seccomp: 2`, `Seccomp_filters: 3`, `NoNewPrivs: 1` in its `/proc/<pid>/status`);
`$XDG_RUNTIME_DIR/bastion-mc/` was empty during and after the run; exit status 130 on
SIGINT. The threat model records this run as §7 E25.

**Not shown by any of this:**

* **A person at the keyboard.** The key press was synthetic (XTEST, US layout); a physical
  keyboard, another layout, and the wheel and the mouse as devices were not involved (their
  input was fed to the game's own handlers and measured from there).
* **What fullbright looks like.** What was observed is the input to the game's light-map
  shader; what the shader makes of it was read from 26.3's shader source, not watched.
  Nobody looked at a dark cave with fullbright on.
* **Another Minecraft version.** `force` was tested as a decision, with the stand-in game and
  with the one broken build above; a real second version of Minecraft was never started
  with the mod.
* **Wayland beyond a headless sway**, and any key press on Wayland (XTEST is an X11 extension).
* **Multiplayer.** There is no account on the development machine; the mod never met a
  server. That a server's demand for reduced debug information goes through the same game
  method as the player's own option was read in the game's code, not exercised.
* **Sound and playability** — how the zoom feels, pointer capture inside gamescope.

## Open

* **Approval prompts**: designed (see above), not built.
* **Further features**: none added. Candidates that fit the rule "a harmless client-side
  convenience, nothing servers commonly forbid": a toggle mode for zoom, the smooth camera
  while zoomed, a clock line in the read-out. Deliberately not candidates: free look, a
  minimap, anything that automates input or shows what the unmodified game does not show.
* **Fabric Loader versions other than 0.20.1.** A loader too old to know `fabric.addMods`
  would ignore the property silently: the launcher would say "added", and no hello would
  follow.
* **Quilt** is not supported.
* **Reproducibility across compilers**: only one JDK was installed; two builds with the same
  `javac` agree, other vendors and releases were not tried.
* **The front end** shows the companion's lines in its console like any other log line; it
  has no indicator of its own. `bastion-mc doctor` does not mention the mod, and the
  `instance.toml` that `instance new` and `profile new` write does not mention `[companion]`.

## Licence

The mod is first-party code under the project's licence, MIT ([`LICENSE`](../LICENSE));
`LICENSE` is packed into the jar as `META-INF/LICENSE`, and `fabric.mod.json` says
`"license": "MIT"`. It contains no code of Mojang, Fabric or Mixin. The stand-ins in
`java/stubs` reproduce the names and signatures of seven public API types of Fabric Loader
(Apache-2.0) and Mixin (MIT) — no method bodies — and are not in the jar. The stand-in game
under `tests/java/fakegame` uses the names of some Minecraft classes and members around code
written for the tests; it is test code and is not shipped.
