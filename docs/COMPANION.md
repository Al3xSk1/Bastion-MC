# The companion mod

`bastion-companion` is a Fabric client mod that belongs to the launcher. It is compiled from
source together with `bastion-mc`, embedded in the launcher binary, and added by the
launcher to Fabric profiles for one session at a time: your signed `mods/` directory is not
touched and stays exactly what you signed. On every Fabric release from 1.20.1 to 26.3 it
gives the game the in-game phone ([`PHONE.md`](PHONE.md)), a redone home screen and darker
menus; on 26.3 also three conveniences — zoom, a HUD read-out and fullbright. Like the
launcher, it is pre-release.

**It is not a security feature, and it enforces nothing.** It runs inside the game's JVM
next to every other mod, and any of them can change it, imitate it or switch it off.
Nothing the launcher promises depends on it, and nothing it shows in the game deserves more
trust than the game itself. What it tells the launcher is of two kinds: its reports, which
the launcher writes into the launch log marked as coming from the game and does nothing else
with; and the phone's commands, which the launcher carries out only within each app's fixed
list of commands and limits — whoever in the game sent them. Everything that protects you
is in the launcher, outside the sandbox. What the mod changes in the threat model is in
[`THREAT_MODEL.md`](THREAT_MODEL.md): rows J19, J2, C2 and C7, §4.7 for the phone (H1 to
H23), and §5.

## What it does

| Feature | Default | Minecraft versions | What it does |
|---|---|---|---|
| Phone | on | 1.20.1 to 26.3 | Press **R** in a world (or on the home screen) and a phone slides up with the apps the launcher set up for this launch: Spotify, Discord and a browser, clips, Realms approvals, settings sync. Everything about it is in [`PHONE.md`](PHONE.md). |
| Home screen | on | 1.20.1 to 26.3 | The title screen is replaced by a dark one: the game's name and version on the left with what the title screen offers — Singleplayer, Multiplayer, Minecraft Realms, Options, Mods (when a mod list such as Mod Menu is installed) and Quit; in demo mode Play Demo World instead of the first three — and below them Language, Accessibility and Classic title screen (the game's own, once). On the right, a sketch of the phone with the key that pulls it out. The captions are the game's own, in your language, and every button does exactly what the title screen's button does. |
| Dark menus | on | 1.20.1 to 26.3 | The game's own menus (options, world list, pause menu …) get a darker background: flat gunmetal outside a world, a dark veil over the world inside one. Only the background changes; the menus are the game's. |
| Zoom | on | 26.3 | Hold **C** to zoom in: the field of view is divided by 4. While you hold it, the mouse wheel changes the magnification (one notch: × 1.15, between 1 and 50) and does not switch the hotbar slot; letting go forgets the change. Zooming in and out is eased over about a fifth of a second, and the mouse turns the view proportionally slower while zoomed (by the ratio of the tangents of the two view angles), so aiming feels the same. With a screen open — chat, inventory, a menu — the key is just a letter: typing "c" does not zoom. |
| HUD read-out | off | 26.3 | In a world, three lines in the top-left corner: frames per second, your position and the direction you face. Hidden with the HUD (F1) and whenever the game's own debug overlay shows something (F3, or debug entries you set to show always). Where reduced debug information is on — your own option, or a server asking for it — position and facing are withheld, as they are on the debug screen, and the read-out is the frame rate alone. |
| Fullbright | off | 26.3 | Dark places are drawn as if the brightness slider went far past its end. It raises the number the game computes for its light map from the Brightness option, so the option itself, `options.txt` and the Darkness effect's own dimming stay as they are. **Some servers do not allow this.** It changes only what your client draws, but a server's rules may still forbid it; that is yours to check before you switch it on. |

The phone, the home screen and the dark menus share one dark theme — near-black gunmetal,
one cyan accent, amber for caution, red for danger — the same family of colours as the
launcher's window.

Zoom, the HUD read-out and fullbright reach the game by 26.3's names. On every other version
they switch themselves off without trying and say so once, on the channel to the launcher
and in the game's log:

```text
[game] companion channel #1: zoom: disabled: not available on Minecraft 1.21.4 (this feature is written for 26.3's classes)
```

A feature that cannot find the part of the game it needs switches itself off for the
session, says so once (in the game's log and on the channel to the launcher), and leaves the
game alone; the other features keep working. The mod's own log output is capped at 120 lines
per session.

### Keys

Key names are Minecraft's own, as in `options.txt`: `key.keyboard.c`, `key.keyboard.r`,
`key.keyboard.left.alt`, `key.keyboard.f6`, `key.keyboard.keypad.0` … The leading
`key.keyboard.` may be left out (`"c"`, `"left.alt"`). Mouse buttons are not supported.

The launcher checks only the shape of the name (lower-case letters and digits in parts
separated by dots, at most 40 characters, not `unknown`); whether the game knows a key of
that name is for the game to say. The mod asks the game the first time the key matters: if
the game knows no keyboard key of that name, zoom switches itself off and says so, and the
phone has no key (the game's log says so). Neither key is one of the game's key bindings:
they do not appear in the game's Controls screen, they are changed only in the launcher's
settings, and another mod that binds the same key gets the press too.

The game has uses of its own for **C**; in 26.3's `options.txt` three bindings name it
(checked in the test instances' `options.txt`): saving a hotbar in creative mode
(`key.saveToolbarActivator`, together with a number key) and, together with the debug key
F3, copying your location and a deliberate crash (`key.debug.copyLocation`,
`key.debug.crash`). Zoom does not take the key away from them; pick another key if they get
in your way. **R** is bound by no Minecraft version from 1.20.1 to 26.3 by default
([`PHONE.md`](PHONE.md) says how that was checked).

## Settings

The settings are the launcher's, under `[companion]` in `~/.config/bastion-mc/config.toml`.
Every key is optional; the values below are the defaults (`bastion-mc config example`
prints them with comments).

| Key | Default | Values | What it decides |
|---|---|---|---|
| `enabled` | `true` | `true`, `false` | Whether the mod is added to Fabric instances at all. `false` keeps it out — and with it the phone, the services only a phone uses (Spotify, the browser, clips) and Realms approvals. |
| `force` | `false` | `true`, `false` | Also add it to Minecraft versions it was not run on (see below). |
| `phone` | `true` | `true`, `false` | The in-game phone. With `false` the launcher also prepares none of the services only a phone uses (Spotify, the browser, clips) and refuses Realms approvals ([`PHONE.md`](PHONE.md)). |
| `phone_key` | `"key.keyboard.r"` | a keyboard key by Minecraft's name for it | The key that pulls the phone out and puts it away. |
| `home` | `true` | `true`, `false` | The redone home (title) screen. |
| `dark_menus` | `true` | `true`, `false` | Darker backgrounds behind the game's own menus. |
| `zoom` | `true` | `true`, `false` | Hold a key to zoom (26.3). |
| `zoom_key` | `"key.keyboard.c"` | a keyboard key by Minecraft's name for it | The zoom key. |
| `zoom_factor` | `4.0` | 1.1 to 50 | The field of view is divided by this. |
| `zoom_wheel` | `true` | `true`, `false` | The mouse wheel changes the factor while the key is held. |
| `hud` | `false` | `true`, `false` | The HUD read-out (26.3). |
| `fullbright` | `false` | `true`, `false` | Fullbright (26.3). Some servers do not allow this. |
| `fullbright_gamma` | `15.0` | 1 to 100 | What the game's brightness (0 to 1; its own slider ends at 1) is raised to. |

```toml
[companion]
enabled = true
force = false
phone = true
phone_key = "key.keyboard.r"  # "r" works too
home = true
dark_menus = true
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
settings as a read-only file, written for that launch — with the defaults, for a 26.3
instance:

```text
# Written by bastion-mc for this launch, from [companion] in its configuration.
# Read-only: nothing in the game can change it.
zoom=true
zoom.key=key.keyboard.c
zoom.factor=4
zoom.wheel=true
hud=false
fullbright=false
fullbright.gamma=15
phone=true
phone.key=key.keyboard.r
home=true
darkmenus=true
minecraft=26.3
channel=/bastion/companion/channel
```

`minecraft` is the version the launcher starts; the mod picks its adapter by it (below).
Nothing in the game can change that file for the next session, and the mod has no way of
writing a file at all (a test scans its class files for one). To change a setting, edit the
launcher's file and start the game again.

## Minecraft versions

Minecraft's own code changed several times between 1.20.1 and 26.3 — how the GUI is drawn,
how pictures become textures, how keys and the mouse arrive, where the open screen lives.
The mod has one adapter per *family* of versions that are alike in what it needs:

| Family | Versions | Run in the real game on |
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

The table is `java/stubs/tools/families.txt`; the build compiles it into the mod. **How the
family is chosen:** at start the mod reads the version from the launcher's settings file
(`minecraft=`) — the launcher started exactly that version — and looks it up in that table.
The family decides which adapter the platform layer uses and which mixins Mixin may apply
(a mixin configuration plugin answers Mixin for each one, so Mixin never looks for a class
that is not there, and the game's log has no Mixin warning on any family). The game's log
says what was picked, for example
`platform: Minecraft 1.21.4, family v1_21_2 (intermediary)`.

The launcher adds the mod by itself to the versions in the last column
(`companion::TESTED_MINECRAFT`); for the other versions of a family, set `force = true` (the
mod picks the family's adapter; that exact version was not started here). A version outside
every family — older than 1.20.1, newer than 26.3 — gets no adapter and no mixin of the mod
at all: with `force` the mod loads, says in the game's log that it has no adapter for that
version, and the phone, the home screen and the dark menus stay off, and so do zoom, the HUD
read-out and fullbright, whose hooks are mixins too (read in the code, not run: no such
version was started).

The phone, the home screen and the dark menus are written once, against the mod's own
platform layer (`java/companion/src/bastion/companion/platform/`: screens that do not pause
the game, the title screen's actions, the HUD layer, the menu background, the phone key,
textures, drawing, input normalised to one numbering on every version, the line to the
launcher). Each family's adapter (`java/companion/platform/<family>/`, with code shared by
the families of a group in `…/intermediary/` and `…/mojang/`) implements that layer with the
game's classes of that family: four to eight small classes and four hooks — the screen being
set, the start of each frame, the HUD's drawing, and the menu background.

## When it is added, and how you can tell

The launcher decides at every launch, in this order:

1. `enabled` is on;
2. the instance is a signed **Fabric** profile (`bastion-mc profile new <id> --minecraft
   <version> --fabric`). A vanilla instance cannot load mods; Quilt profiles are not
   supported;
3. at least one feature is on;
4. the Minecraft version is one the mod was run on — 1.20.1, 1.20.4, 1.21.1, 1.21.4, 1.21.5,
   1.21.8, 1.21.10, 26.1.2, 26.2 and 26.3 — unless `force = true`;
5. the signed profile does not set `-Dfabric.addMods=` itself. Its loader options come after
   the launcher's own on the Java command line and would win, so the launcher leaves the
   profile exactly as signed and adds nothing;
6. the game's Java is 17 or newer.

Every launch says what happened, in the launch log and on the terminal or in the front
end's console. From a demo launch of a signed Fabric 26.3 profile with the default settings
and the phone's services on the merged tree (2026-11-14, the end-to-end run of the threat
model's §7 E32):

```text
[launcher] companion: bastion-companion 0.1.0 added for this session (sha256 d273559f3a1571ea2ca7dd2a643b0c731cfd004db4faf3b63e59d9c512796eaf, 485374 bytes; read-only, outside the signed mods directory); features: zoom, phone, home, darkmenus; channel to the launcher: on (what arrives is logged, never acted on). It is a convenience and enforces nothing.
[game]  - bastion-companion 0.1.0          ← in Fabric Loader's list of the mods it loads
[game] companion channel #1: hello: mod version 0.1.0, protocol 2, features: zoom, phone, home, darkmenus
[launcher] phone: 5 app(s) (spotify, browser, settings, realms, clips), 1 connection(s) greeted, 27 command(s), 27 accepted
[launcher] companion channel: 1 connection(s), 1 hello, 0 diagnostic(s), 27 phone command(s)
```

The first line's "what arrives is logged, never acted on" is true of the mod's reports and
dates from before the phone: the phone's commands are acted on, within each app's list of
commands and its limits (below). Lines that start with `[game] companion channel` are what
*something inside the game* told the launcher; the launcher writes them down and does
nothing else with them. With `force` on a version that was not run, the first line has
`; Minecraft <version> is untested (companion.force)` before its closing sentence. When the
mod is left out, the line is `companion: not added — ` and the reason:

| Reason in the log | Why |
|---|---|
| `switched off (companion.enabled = false)` | `enabled = false` |
| `this instance has no mod loader (…)` | a vanilla instance, or a profile without a loader |
| `the profile's loader is Quilt; only Fabric is supported` | a Quilt profile |
| `every feature is switched off` | all six features are off |
| `Minecraft <version> is not a version it was tested with (1.20.1, 1.20.4, …, 26.3); …` | not run on that version, and `force` is off |
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

One UNIX stream socket between the mod and the launcher (`src/companion/channel.rs`,
`Channel.java`). The mod opens it with one text line, its hello:

```text
hello <protocol> <mod version> <feature>[,<feature>…]      "-" for no feature
```

`<protocol>` is 1 to 4 digits, `<mod version>` 1 to 32 of `A–Z a–z 0–9 . + -`, a feature 1 to
32 of `a–z 0–9 _ -` starting with a letter, at most 16 features.

**Protocol 1** carries information and nothing else: after the hello, one
`diag <feature> <free text>` line for each feature that switched itself off. The launcher
logs it and never acts on it. This mod no longer speaks it, but the launcher still serves
it exactly so.

**Protocol 2** is the phone's, and what this mod speaks: `hello 2 …`, and after that line
frames in both directions — a 4-byte length, a kind byte, the payload. The mod sends
commands (one command line each, at most 1 KiB) and diagnostics (protocol 1's `diag` line,
at most 512 bytes); the launcher sends each app's state (a JSON object, at most 64 KiB),
pictures (at most 4 MiB) and notices (at most 256 bytes). The mod sends at most 50 messages
and 200 KiB a second, from a queue that never holds up the game; it checks everything it
reads (lengths per kind, app names, JSON bounded in size and depth, pictures at most 2048
pixels a side, decoded off the render thread) and closes the connection on anything else.
The launcher hands each command to its dispatcher, which lets it do only what that app's
fixed list of commands allows, at that command's rate. [`PHONE.md`](PHONE.md) has the
commands, the limits and what each app does; the threat model's H1 has the channel as a
lever.

**What the launcher does with what arrives: it logs the reports and never acts on them, and
it carries out phone commands only within each app's list and limits.** Any mod in the same
JVM can connect to the socket and say anything; a hello proves nothing about who sent it,
and the newest connection that says `hello 2` is the phone. So every byte is treated as
hostile input and bounded:

| Bound | Value |
|---|---|
| Length of a line (the hello, and protocol 1) | 512 bytes (a `\r` before the `\n` counts); a longer line is dropped as it arrives, without being buffered |
| Frames (protocol 2) | from the mod only commands (1 KiB) and diagnostics (512 bytes); another kind, a length of 0 or over the limit ends the connection, said once |
| Connections | 4 open at the same time, 64 in a session; further ones are closed unread |
| Reading | 64 KiB per second per connection; in protocol 2 also 50 frames and 200 KiB a second for all connections together |
| Logging | 20 lines at once, then 2 per second, 2000 in a session, for all connections together; the rest is counted |
| Free text of a diagnostic | sanitised (no control or bidi characters) and cut to 200 characters |
| Lines that are not part of the protocol | the first three are quoted (sanitised, 80 characters), the rest only counted |
| Refused phone commands | counted; each kind logged once |

At the end of the session the launcher writes one summary line with the counts, and one for
the phone.

The socket is created before the sandbox starts, in a new private directory
(`$XDG_RUNTIME_DIR/bastion-mc/companion-<12 hexadecimal digits>/channel`, or under the
launcher's cache directory where there is no runtime directory), and its name is removed as
soon as the sandbox holds it, before the launcher confines itself. In the demo launch of the
third round (the threat model's §7 E25) `$XDG_RUNTIME_DIR/bastion-mc/` was empty while the
game ran and afterwards. Before that — from the moment the launcher prepares the sandbox,
through the sign-in, the resource-limit probe and the start of the nested compositor:
seconds, not an instant — the name exists in that private directory, where other programs
running as you could connect and be logged as if they were the game, or send phone commands.
A launch that stops with an error before the launcher has confined itself removes the name
on its way out. The name stays if the launcher cannot establish that the program in the
sandbox exists (it then confines itself and can no longer remove it — and says so, with a
warning in the log and to the front end), or if the launcher is killed or crashes before
that point; a later launch that adds the companion then removes it (a socket at least a
minute old on which nobody listens). The launcher starts reading the socket only after it
has confined itself — like every other input from the game; a test in the default suite
(`tests/companion.rs`) serves and reads the channel from a process confined exactly that
way, after the name was removed.

One approval is now made through the channel: a Realms join, whose code the launcher shows
outside the game and you type into the phone (the threat model's J22). The general design
for approval prompts — with the rule that a decision is made and entered outside the sandbox
— is in `docs/notes/companion-mod.md` (part 2); nothing else of it is built.

## How it is built

`build.rs` compiles the mod with the system `javac` (`JAVAC` overrides it, as for the launch
bootstrap class) by the recipe in `src/companion/jar.rs`, which the tests use to build it
again and compare. Then the jar is embedded in the launcher binary.

* **Inputs:** `java/companion/src/**.java` (everything that names no game class: the
  features, the platform layer, the phone, the home screen, the theme),
  `java/companion/platform/**.java` (the adapters), `java/companion/resources/*`
  (`fabric.mod.json`, the mixin list), `java/stubs/**.java`,
  `java/stubs/tools/families.txt`, `LICENSE`, the package version, and `javac`. Nothing
  else: `-proc:none -implicit:none`, an empty source path, an explicit class path, and
  `JDK_JAVAC_OPTIONS`, `JAVA_TOOL_OPTIONS`, `_JAVA_OPTIONS` and `CLASSPATH` removed from the
  compiler's environment. No network, no Gradle, no annotation processor, no additional
  crate: the archive is written by about 70 lines of the launcher's own code.
* **In passes**, each a separate `javac --release 17` with an explicit class path: (1) the
  common stand-ins; (2) the version-independent sources against (1); (3) each family's
  stand-ins; (4) each group's shared sources against (1), (2) and the stand-ins of **every**
  family of the group — which must all give the same class files, byte for byte, or the
  build stops; one copy is packed; (5) each family's own sources against (1), (2), its
  group's classes and its stand-ins.
* **No Minecraft at build time.** The adapters are compiled against stand-ins of
  Minecraft's classes, one set per family (`java/stubs/minecraft/<family>`, 46 to 56 files
  each): names, descriptors, static-ness and supertypes of the 52 classes and 132 members
  the adapters use (`java/stubs/tools/needed.txt`), no code, no constant values. The
  features written for 26.3 name their targets as strings and look everything up by name
  when the game starts (`Game.java`). Mojang's jar is not needed to build the mod and
  nothing of it is shipped.
* **The stand-ins are generated offline, and can be checked again.** `java/stubs/tools`
  (plain Python 3, standard library only; run with `-I`) downloads, for every release from
  1.20.1 to 26.3 (23 versions), the version file, the client jar, Mojang's client mappings
  (up to 1.21.11) and Fabric's intermediary mappings, each checked against the hash its
  publisher gives (SHA-1 from Mojang's version manifest, SHA-512 from Fabric's maven); it
  writes the stand-ins in the names the game has at run time, groups the versions whose
  stand-ins are identical, and requires each group to be exactly one line of
  `families.txt`. `java/stubs/minecraft/SOURCES.txt` holds the SHA-256 of the 105 files it
  read on 2026-11-14.

  ```sh
  # downloads, generates, checks the family table, compares with java/stubs/minecraft;
  # changes nothing:
  python3 -I java/stubs/tools/regenerate.py target/stubsrc --check
  # without --check it rewrites java/stubs/minecraft/<family>/ and SOURCES.txt
  ```

* **Common stand-ins** (`java/stubs/common`): Fabric Loader's `ClientModInitializer`, eight
  Mixin types (`Mixin`, `Inject`, `At`, `Coerce`, `CallbackInfo`, `CallbackInfoReturnable`,
  `IMixinConfigPlugin`, `IMixinInfo`) and ASM's `ClassNode`, reduced to the names and
  signatures the mod uses. They are on the compiler's class path and never in the jar. An
  opt-in test compiles the mod against the real Fabric Loader 0.20.1, Mixin and ASM jars
  instead and gets the same jar, and compares every member a stand-in declares with the real
  class (30 members of 10 types agree).
* **Reproducible:** sources in sorted order; `-g:none` (no source file names, no line or
  variable tables); `--release 17` (class-file version 61); stored entries, the manifest
  first and the rest by name, the time stamp 1980-01-01 00:00:00, no extra fields, comments
  or directory entries. The same sources and the same `javac` give the same bytes in any
  directory (`companion::tests::building_the_jar_again_gives_the_same_bytes`). A different
  `javac` release may emit different bytecode, so a published hash means something only
  next to the JDK version. On this tree, with `javac` 27.0.1: SHA-256
  `d273559f3a1571ea2ca7dd2a643b0c731cfd004db4faf3b63e59d9c512796eaf`, 485 374 bytes — the
  launch log names the hash and size at every launch.
* **Requirement:** a JDK that can target Java 17, i.e. JDK 17 or newer (the Arch package's
  `makedepends` has `java-environment>=17`). An older one stops the build with "the companion
  mod needs JDK 17 or newer to build".

The mod is 14 477 lines of Java in 146 files, including comments; 800 of them are the
self-test that only the launcher's own real-game test can switch on (`SelfTest.java`,
`PlatformProbe.java`). Anyone who signs a profile can read it in `java/companion`.

## Adding a Minecraft version

The versions the launcher adds the mod to by itself are `companion::TESTED_MINECRAFT` in
`src/companion/mod.rs`: the representative of each family. To add a version:

1. **A version of an existing family** (it is on that family's line of `families.txt`):
   install it as a signed Fabric profile in a test data root, set `force = true` in that
   instance's `instance.toml`, and run the real-game test against it, on a display nobody
   looks at:

   ```sh
   DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=<profile> \
       cargo test --test companion_game -- --nocapture
   ```

   then again with `BASTION_COMPANION_WORLD=1` (it enters the demo world) and with
   `BASTION_COMPANION_KEYPRESS=1` as well (the phone key, really pressed). If it passes, add
   the version to the list and to the comment in `examples/config.toml`.
2. **A new version**: add it to `families.txt` — to the family whose stand-ins it matches,
   which `regenerate.py` checks (it stops and names the versions that differ), or as a new
   family with adapters of its own under `java/companion/platform/<family>/` — run
   `regenerate.py` without `--check`, build, and continue as in step 1.
3. Zoom, the HUD read-out and fullbright stay 26.3's until they are ported: a family mixin
   for each hook and the look-ups in that family's names.

## What was verified, and what was not

On the development machine (Arch Linux, kernel 7.2.14, OpenJDK 27.0.1, bubblewrap 0.13.1,
Landlock ABI 10), in demo mode (no account), with Fabric Loader 0.20.1 (Mixin 0.8.7) and
gamescope 3.17.0. The engineers' records of every run are `docs/notes/companion-mod.md`
(part 4: zoom, the HUD read-out and fullbright), `docs/notes/mod-platform.md` and
`docs/notes/mod-ui.md` (part 4: the families, protocol 2, the phone, the home screen and the
dark menus).

**Without Minecraft**, in every `cargo test`: what the embedded jar contains (only the mod's
own classes and resources, class-file version 61, no debug information, every family's
mixins listed for the plugin); that two more builds give the same bytes; that the mod names
no API for networking beyond the one UNIX socket, for starting processes, loading native code,
defining classes or writing files (the phone and the home screen included); the settings
and their validation; the injection decision and what an injection adds; the channel's
grammar, protocol 2's frames and the dispatcher (generated-input harnesses
`robustness::companion_channel_reader` and `robustness::phone_frames_and_dispatch`); the
mod's reader of the launcher's frames (4 000 mutated frames and texts per run); the platform
core on a stand-in family; the phone's screens against a stand-in game
(`tests/phone_ui.rs`: every command line the phone sends checked against the launcher's own
parser and the apps' lists, 5 504 frames on 13 window sizes checked for drawing outside its
place, hostile states); the mod in a JVM with no game at all (every feature off, said once);
zoom, the HUD read-out and fullbright against a stand-in "game" in `tests/java/fakegame`. In
the real sandbox — bubblewrap, Landlock, seccomp —: the two files are there with the right
bytes and nothing is writable, creatable, removable or executable; no socket can be bound
next to the channel; and the system JVM runs the mod on the launcher's settings, not on a
file the program planted, with its hello reaching the launcher through Landlock and
seccomp.

**In the real game, on every family** (`tests/companion_game.rs`, opt-in; the threat
model's §7 E26): the whole launch path with a signed profile of each family's
representative, every feature on, the phone served by a demo service (a test switch: fake
state for every app, every command it receives written to the launch log), and the mod's
scripted self-test. On each of the ten:

| Family | Version | Title screen | World | World, phone key pressed | Final jar, with the game's options |
|---|---|---|---|---|---|
| `v1_20_1` | 1.20.1 | passed | passed | passed | passed |
| `v1_20_2` | 1.20.4 | passed | passed | passed | passed |
| `v1_21` | 1.21.1 | passed | passed | passed | passed |
| `v1_21_2` | 1.21.4 | passed | passed | passed | passed |
| `v1_21_5` | 1.21.5 | passed | passed | passed | passed |
| `v1_21_6` | 1.21.8 | passed | passed | passed | passed |
| `v1_21_9` | 1.21.10 | passed | passed | passed | passed |
| `v26_1` | 26.1.2 | passed | passed | passed | passed |
| `v26_2` | 26.2 | passed | passed | passed | passed |
| `v26_3` | 26.3 | passed | passed | passed | passed |

Asserted each time: Fabric Loader lists the mod; no line from Mixin or about the mod at
warning level or above; the family the mod picked is the one `families.txt` names; the hello
of protocol 2 with the features that exist on that family (all six on 26.3; phone, home and
darkmenus elsewhere, with one diagnostic for each convenience that is not there); commands
from the mod as the demo received them, and the demo's state, notice and pictures decoded
and drawn in the game; the home screen in place of the title screen; an overlay drawn with
every primitive and closed again; the game's own options with the dark background, painted
through the family's hook; in world mode the demo world entered through the title screen's
own button and the HUD hook drawing every frame; with the key pressed through XTEST on a
private display, the phone opened. The first two runs on 26.3 found two defects, both fixed
before the runs in the table. Separately, the phone's screens were driven through XTEST keys
and clicks on all ten families at two window sizes, in a world on 26.3 and 1.21.4, and at
GUI scale 1 on 26.3, every screenshot looked at (§7 E27).

**Zoom, the HUD read-out and fullbright on 26.3** (the third round, §7 E24; the self-test
measures the same on every run since): zoom from field of view 70 to 17.5 (70 / 4) and back;
for the same mouse movement the game turned the player 30.0° unzoomed and 6.5944° zoomed
(ratio 0.2198, the tangent rule); one wheel notch while zoomed took the factor from 4.000 to
4.600 and left the hotbar slot alone; the HUD read-out's three lines, not drawn with the HUD
hidden or the debug overlay up, the frame rate alone under reduced debug information;
fullbright's light-map input 15.0 instead of 0.5; with nothing scripted, nothing zoomed and
nothing was typed. Those runs covered all three of the launcher's display paths (the nested
compositor, the game as a direct X11 client, a Wayland security context on a headless sway);
the fourth round's ran through the nested compositor only.

**On the merged tree** (commit `c6f25a8`, 2026-11-14): the real-game test again on 1.20.1,
1.21.4 and 26.3 (26.3 in world mode), passed; and one end-to-end run with the real services
behind the phone on 26.3 (§7 E32).

**Not shown by any of this:**

* **Versions of a family other than its representative.** Their stand-ins are identical to
  the representative's by construction; they were not started.
* **A person at the keyboard.** Key presses were synthetic (XTEST, US layout); a physical
  keyboard, another layout, and mouse drags in the game were not involved. How the phone
  feels — the slide, where it stands — is a judgement still to be made.
* **The home screen outside demo mode** (Singleplayer, Multiplayer, Realms) and its Mods
  button: seen only with the stand-in platform (no account, no Mod Menu).
* **What fullbright looks like.** What was observed is the input to the game's light-map
  shader; what the shader makes of it was read from 26.3's shader source, not watched.
* **Wayland** for the phone and the home screen (only the nested compositor on X11), and
  any key press on Wayland (XTEST is an X11 extension).
* **Multiplayer.** There is no account on the development machine; the mod never met a
  server. That a server's demand for reduced debug information goes through the same game
  method as the player's own option was read in the game's code, not exercised.
* **Fabric Loader versions other than 0.20.1**, and Quilt.
* **The cost of the browser's pictures** at 15 a second through the channel and into a
  texture each: the path works; its cost was not measured.

## Open

* **Zoom, the HUD read-out and fullbright on the other families**: not ported (they reach
  the game by 26.3's names). The stand-ins and the mixin plugin are in place for it.
* **Approval prompts** beyond Realms joins: designed (see above), not built.
* **Further conveniences**: none added. Candidates that fit the rule "a harmless
  client-side convenience, nothing servers commonly forbid": a toggle mode for zoom, the
  smooth camera while zoomed, a clock line in the read-out. Deliberately not candidates:
  free look, a minimap, anything that automates input or shows what the unmodified game
  does not show.
* **Fabric Loader versions other than 0.20.1.** A loader too old to know `fabric.addMods`
  would ignore the property silently: the launcher would say "added", and no hello would
  follow.
* **Quilt** is not supported.
* **Reproducibility across compilers**: only one JDK was installed; two builds with the same
  `javac` agree, other vendors and releases were not tried.
* **The front end** shows the companion's and the phone's lines in its console like any
  other log line; it has no indicator of its own. `bastion-mc doctor` does not mention the
  mod, and the `instance.toml` that `instance new` and `profile new` write does not mention
  `[companion]`.

## Licence

The mod is first-party code under the project's licence, MIT ([`LICENSE`](../LICENSE));
`LICENSE` is packed into the jar as `META-INF/LICENSE`, and `fabric.mod.json` says
`"license": "MIT"`. It contains no code of Mojang, Fabric, Mixin or ASM. The common
stand-ins in `java/stubs/common` reproduce the names and signatures of ten public API types
of Fabric Loader (Apache-2.0), Mixin (MIT) and ASM (BSD-3-Clause) — no method bodies — and
are not in the jar. The Minecraft stand-ins in `java/stubs/minecraft` name a few hundred of
Minecraft's classes and members (intermediary names for 1.20.1 to 1.21.11, Mojang's for 26.x,
Mojang's names in comments for readability) — not the mappings, which Mojang's licence
allows using for development and forbids redistributing complete and unmodified; they
contain no code of Mojang's, are not packed into the jar, and the generator's inputs (jars
and mappings) are not committed. The stand-in game under `tests/java/fakegame` uses the
names of some Minecraft classes and members around code written for the tests; it is test
code and is not shipped.
