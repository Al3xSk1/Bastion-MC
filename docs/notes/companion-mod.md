# The companion mod

Notes for the integrator and for whoever owns README.md and THREAT_MODEL.md in this round.
Part 1 is user documentation, written to be pasted into the README. Part 2 is what the
threat model has to say about it. Part 3 is how it is built and how it works. Part 4 is
what was verified, how, and what was not. Part 5 is what remains open.

Everything here describes the tree at this branch's last commit. "Verified" always names
the test or the run; where nothing was run, it says so.

---------------------------------------------------------------------------------------

## 1. For the README

### Zoom and other conveniences (the companion mod)

Fabric profiles get a small mod of the launcher's own, without you installing anything:
**bastion-companion**. It is compiled from source together with the launcher, lives inside
the launcher binary, and is added to the game for one session at a time — your signed
`mods/` directory is not touched and stays exactly what you signed.

What it gives you:

| Feature | Default | What it does |
|---|---|---|
| **Zoom** | on | Hold **C** to zoom in (the field of view is divided by 4). While you hold it, the mouse wheel changes the magnification (and does not switch the hotbar slot); letting go forgets the change. Zooming in and out is eased over about a fifth of a second, and the mouse turns the view proportionally slower while zoomed, so aiming feels the same. With a screen open — chat, inventory, a menu — the key is just a letter: typing "c" does not zoom. |
| **HUD readout** | off | Frames per second in the top-left corner, and in a world your position and the direction you face. Hidden with the HUD (F1) and whenever the game's own debug overlay shows something (F3, or debug entries you have set to show always). Where reduced debug information is on — your own option, or a server asking for it — position and facing are withheld, as they are on the debug screen. |
| **Fullbright** | off | Dark places are drawn as if the brightness slider went far past its end. **Some servers do not allow this.** It changes only what your client draws, but a server's rules may still forbid it; that is yours to check before you switch it on. |

It is a convenience, **not a security feature**: it runs inside the game next to every other
mod and enforces nothing. Everything that protects you is in the launcher, outside the
sandbox.

Settings are the launcher's, in `~/.config/bastion-mc/config.toml`:

```toml
[companion]
enabled = true
zoom = true
zoom_key = "key.keyboard.c"   # Minecraft's own name for the key, as in options.txt; "c" works too
zoom_factor = 4.0             # 1.1 to 50
zoom_wheel = true
hud = false
fullbright = false            # some servers do not allow this
fullbright_gamma = 15.0       # 1 to 100; the game's own brightness slider ends at 1
force = false                 # also add it to Minecraft versions it was not tested with
```

and any of these keys can be set for one instance only, under `[companion]` in that
instance's `instance.toml` (`~/.local/share/bastion-mc/instances/<id>/instance.toml`):

```toml
[companion]
zoom_key = "left.alt"
hud = true
```

There is no settings screen in the game, on purpose: the game gets the settings as a
read-only file and cannot change them, and the mod reads no other file. Edit the
launcher's file and start the game again.
`enabled = false` (globally or for one instance) keeps the mod out altogether.

Key names are Minecraft's: `key.keyboard.c`, `key.keyboard.z`, `key.keyboard.left.alt`,
`key.keyboard.f6`, `key.keyboard.keypad.0` … (the leading `key.keyboard.` may be left out).
Mouse buttons are not supported. The game has uses of its own for **C** (three bindings in
26.3's options.txt): saving a hotbar in creative mode (`key.saveToolbarActivator`, together
with a number key) and, together with the debug key F3, copying your location and a
deliberate crash (`key.debug.copyLocation`, `key.debug.crash`). Zoom does not take the key
away from those; pick another key if they get in your way.

When it is added, and how you can tell:

* Only to **Fabric** profiles (`bastion-mc profile new <id> --minecraft <version> --fabric`).
  A vanilla instance cannot load mods; Quilt profiles are not supported.
* Only to Minecraft versions it was tested with — **26.3** at present. On any other version
  the launcher leaves it out and says so; `force = true` adds it anyway. The mod looks the
  game up by name when it starts, and a feature whose part of the game is not as expected
  switches itself off and says so, rather than breaking the game. (`force` on another
  version has not been tried in a real game.)
* Every launch says what happened, in the launch log and on the terminal or in the front
  end's console:

  ```text
  [launcher] companion: bastion-companion 0.1.0 added for this session (sha256 …, 67867 bytes;
             read-only, outside the signed mods directory); features: zoom; channel to the
             launcher: on (what arrives is logged, never acted on). It is a convenience and
             enforces nothing.
  [game]     - bastion-companion 0.1.0                      ← Fabric Loader's own mod list
  [game]     companion channel #1: hello: mod version 0.1.0, protocol 1, features: zoom
  [launcher] companion channel: 1 connection(s), 1 hello, 0 diagnostic(s)
  ```

  or `companion: not added — …` with the reason. Lines that start with
  `[game] companion channel` are what *something inside the game* told the launcher; the
  launcher writes them down and does nothing else with them.

Building needs a JDK that can target Java 17 (any JDK 17 or newer); nothing is downloaded
and Minecraft itself is not needed to build.

**Source map additions**

| Path | What |
|---|---|
| `src/companion/` | the companion mod's launcher side: configuration, the injection decision, `jar.rs` (the build recipe, also used by `build.rs`), `channel.rs` (the socket the mod reports on) |
| `java/companion/` | the mod: sources and resources (`fabric.mod.json`, the mixin list) |
| `java/stubs/` | compile-only stand-ins for the seven Fabric Loader and Mixin types the mod is compiled against; never shipped |
| `tests/java/fakegame*/` | a stand-in "game" for testing the mod's logic without Minecraft |

**Tests** (additions to the README's list)

```sh
cargo test                                             # includes the mod without Minecraft and in the sandbox
BASTION_E2E_ROOT=$PWD/target/e2e cargo test --test companion      # also: the stand-ins against the real jars
# the mod in the real game (demo mode, no account), on a display nobody is looking at:
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=fab \
    cargo test --test companion_game -- --nocapture
#   … BASTION_COMPANION_WORLD=1 also enters the demo world and measures there
#   … which display the game gets is the launcher's decision, as in any launch (on X11 with
#     gamescope installed: a nested compositor); the test prints it (`display  | …`). To choose:
#     BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt)   a gamescope that is not installed
#     BASTION_COMPANION_DIRECT_X11=1                               the game as a client of $DISPLAY itself
#     WAYLAND_DISPLAY=<a compositor's socket>, no DISPLAY          the Wayland path (security context)
#   … BASTION_COMPANION_KEYPRESS=1 (with …_WORLD) really presses the zoom key, with XTEST:
#     only on a private display that nothing else is using — the key goes to whatever has the focus
```

---------------------------------------------------------------------------------------

## 2. For the threat model

### What changes

The launcher now adds code of its own to the game, opens one more door between the sandbox
and itself, and gains one more parser of bytes from the game. None of it carries a
guarantee; the rows below say what keeps it from weakening the existing ones.

**§3, the diagram.** The list of holes gains one: *"1 socket to the launcher (companion
channel; Fabric profiles only)"*. The launcher box gains *"companion channel: logs what the
mod reports, bounded, acts on nothing"*.

**§4.2, new row** (suggested number J18):

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| J18 | **The launcher's own mod as a way in or a lever.** A hostile mod (a) impersonates the companion on its channel, floods it or feeds it hostile bytes; (b) rewrites the companion's jar or settings so that the next session runs something else; (c) uses the injection to load further code. | The companion **has no privilege and enforces nothing**: it is a client of the same sandbox as every other mod. (a) The channel is one-way and informational. The launcher treats every byte as untrusted: lines of at most 512 bytes (longer ones are dropped without being buffered), 4 connections at a time and 64 per session, each read at 64 KiB/s at most, at most 20 log lines at once, 2 per second and 2000 per session; a strict grammar, free text sanitised; logged as *game* output. Nothing received changes what the launcher does. The reader runs in the launcher after it has confined itself. (b) Jar and settings are generated by the launcher for each launch, exist only inside the sandbox as read-only files under `/bastion/companion` (outside the game directory, outside `mods/`), and are not kept: nothing the game writes reaches the next session's copy. Where the launcher's settings file exists the mod reads no other, so a JVM option cannot point it at a file in the game directory (a signed profile's loader options are `-D` properties of its author's choosing, and they follow the launcher's on the command line). (c) The launcher passes exactly one path of its own in `fabric.addMods`; a signed profile that sets that property itself is left exactly as signed and gets no companion. The injection adds no readable, writable or executable path to the Landlock policy beyond the two files and the socket, no network destination, and nothing executable. | `companion::channel::tests::*` (14), `robustness::companion_channel_reader` (generated and mutated input), `companion::tests::*` (13: the injection decision, what is mounted, the embedded jar's contents, a tripwire on the Java APIs the mod names), `tests/companion.rs::in_the_sandbox_the_mod_is_two_read_only_files_and_a_socket_to_the_launcher` (real bubblewrap + Landlock + seccomp: write, unlink, rename, chmod, create, mkdir, exec and bind are refused; connecting works; the launcher logs and counts), `tests/companion.rs::a_jvm_in_the_sandbox_runs_the_mod_on_the_launchers_settings_not_on_a_file_the_game_wrote` (the system JVM in that sandbox, started like the game; the mod's settings property names a file the program has just written into the game directory: the launcher's settings are in force all the same, and the mod's report reaches the launcher through Landlock and seccomp) | A bug in the channel's line reader would be a bug in the trusted base, like one in the DNS or SNI parser — it is small (about 120 lines: `parse_line`, `Lines`, `Session::received`), fuzzed, and runs in the confined launcher. Any mod can say anything on the channel: the log lines are claims, marked as coming from the game. A hostile mod can use up the connection allowance so that the real mod's hello never arrives (an informational loss). The channel lets code in the game put up to 2000 short lines per session into the launch log — it could already write far more through its standard output. For about a second per launch (until the sandbox holds the socket) its name exists in a private directory of the user's runtime directory, where other programs of the same user (U) could connect and be logged as if they were the game; if the sandbox never comes up the name stays until the next launch sweeps it. |

**§4.4, C2** (unsigned, modified or extra mod jar) — add to *Defence*: "The one mod that is
not in the signed list is the launcher's own companion: part of the launcher binary, built
from source with it, never in `mods/`, named with its SHA-256 in every launch log, and
removable with `companion.enabled = false`." Add to *Residual*: "What runs in a curated
instance is the signed manifest **plus the launcher's companion**. Whoever trusts the
launcher binary already trusts it; it is about 1 800 lines of Java, comments included
(410 of them the self-test, which runs only when the launcher's test asks for it), that
anyone signing a profile can read in `java/companion`."

**§4.4, C7** (malicious dependency / build inputs) — add: "The companion mod is compiled by
`build.rs` with the system `javac` from `java/companion`, like the bootstrap class: no
Gradle, no annotation processor, no network, no build-time dependency (the archive is
written by about 70 lines of the launcher's own code), and neither Mojang's nor Fabric's code as
an input — the seven Fabric Loader and Mixin types it is compiled against are stand-ins in
`java/stubs` holding names and signatures only, compared with the real classes by a test.
Two builds give the same bytes (`companion::tests::building_the_jar_again_gives_the_same_bytes`)."
Residual: "The bytes depend on the `javac` release; a published hash is only meaningful
next to the JDK version (as for the bootstrap class)."

**§4.2, J2** (persistence) — the companion's files can be listed with the other read-only
mounts ("ro: … and the companion's jar and settings").

**§5, not defended** — add:

* **The companion mod is not a defence.** It can be changed, imitated or switched off by
  any other mod in the same JVM. Nothing in this document relies on it, and nothing it
  shows in the game (the HUD readout today, an approval notice later) can be trusted more
  than the game itself.
* **Server rules.** Fullbright (off by default) is forbidden on some servers, and the
  launcher knows nothing about any server's rules. Zoom magnifies what the game already
  draws, and the HUD readout repeats what the game's debug screen shows (and is withheld
  with it under "reduced debug info") — a server may have rules about those too. The mod
  names no networking API beyond the socket to the launcher (a test scans its class files),
  so it sends nothing to a server and there is no protocol-level trace of it — which also
  means a server cannot be told that it is harmless.
* **Same-user software (U)** can edit `config.toml` and so switch companion features on or
  off (it can do far worse, §5.1).

**§8, "In-game companion (Fabric) for approvals"** moves from *not built* to: "*Built:* the
mod as a convenience (zoom, HUD readout, fullbright), injected by the launcher, and a
one-way channel to the launcher (version 1: hello and diagnostics). *Not built:* approval
prompts. Design below."

### How approval prompts would use the channel (not built)

The rule that shapes everything: **the decision is made and entered outside the sandbox.**
Whatever is shown in the game can be drawn over, and whatever is typed in the game can be
typed by a mod. So the mod can only ever be the place where the player *learns* that the
launcher is asking — never the place where the answer is given.

* *Who asks.* The launcher, when one of its own gates fires: the relay refused a server
  that is not allowlisted, the broker refused a request class that is switched off, a pack
  was offered. The request is the launcher's own observation (the SNI it saw, the request
  line it parsed), not a message from the game. A line arriving on the channel must never
  create a prompt: version 1 has no such message, and a later version must not add one.
* *Where it is shown.* In the launcher's own interface — the front end's window or the
  terminal — with the subject rendered by the launcher from its own data. In addition the
  launcher sends a notice down the channel (protocol 2 would add launcher-to-mod lines:
  `ask <id> <kind> <subject>`, later `done <id> approved|denied|expired`), and the mod shows
  "the launcher is asking: switch to its window" in the game. Other mods can read these
  lines too (they can connect to the same socket, and they share the mod's memory anyway),
  so a notice must contain nothing that is not already theirs to know.
* *Where it is answered.* Only in the launcher's interface. On X11 the game sits in a
  nested compositor and cannot type into the front end's window; on Wayland the security
  context withholds input injection. A `seen <id>` line from the mod may tell the launcher
  that the notice was displayed, for the log — it is unauthenticated like everything else
  on the channel and must not count as consent.
* *Fatigue.* A hostile game can cause prompts at will (connection attempts cost nothing).
  The launcher must coalesce and cap them per session, never offer "approve" as the
  default or on timeout, and never let an approval outlive the session unless it goes
  through the signed-allowlist model of §8 (a second factor stored on this machine is
  readable by the user it is meant to stop).
* *What the channel needs for that.* Writing to accepted connections with a per-connection
  budget; the protocol number in the hello to choose the dialect; the same bounds as today
  for what is read. On the mod's side: a reader thread with the same line limit, and a
  drawing hook (the HUD hook that exists can carry it).

---------------------------------------------------------------------------------------

## 3. How it works

### What it is and is not

A Fabric *client* mod of about 1 800 lines of Java including comments (`java/companion/src`;
some 1 300 lines of code, 410 lines of the total being the self-test), id
`bastion-companion`, version = the launcher's. It provides conveniences. It contains no
check, filter or prompt that anything depends on. The launcher's module header
(`src/companion/mod.rs`), the mod's description in `fabric.mod.json`, its start-up log line
and the launch log line all say so.

### The build (`src/companion/jar.rs`, called from `build.rs`)

* **Inputs:** `java/companion/src/**.java`, `java/companion/resources/*`, `java/stubs/**.java`,
  `LICENSE`, the package version, and the system `javac` (`JAVAC` overrides, as for the
  bootstrap class). Nothing else: `-proc:none -implicit:none -sourcepath <an empty
  directory>`, an explicit class path, and `JDK_JAVAC_OPTIONS`, `JAVA_TOOL_OPTIONS`,
  `_JAVA_OPTIONS` and `CLASSPATH` removed from the compiler's environment.
* **No Minecraft at compile time.** The mod's sources name no game class. Mixins use string
  targets (`@Mixin(targets = "net.minecraft.client.Camera")`, methods by name and
  descriptor, game types in handler signatures taken as `Object` through `@Coerce`), and
  everything else goes through `Game.java`: classes by name, members as
  `java.lang.invoke.MethodHandle`s with their types erased, resolved once when the mod
  starts. A lookup that fails is an exception to the *feature* that asked.
* **Stand-ins** (`java/stubs`): `net.fabricmc.api.ClientModInitializer` and six Mixin types
  (`Mixin`, `Inject`, `At`, `Coerce`, `CallbackInfo`, `CallbackInfoReturnable`), each reduced
  to the members the mod uses. They are compiled into a directory of their own that is on
  the class path and never in the archive. No LWJGL stand-in was needed: the key state is
  read through the game's own `InputConstants.isKeyDown`.
* **Reproducible:** sources are passed sorted; `-g:none` (no source file names, no line or
  variable tables); `--release 17` (class-file version 61); the archive is written by
  `jar::archive`: stored entries, the manifest first and the rest by name, DOS time
  1980-01-01 00:00:00, no extra fields, comments or directory entries. Same sources + same
  `javac` ⇒ same bytes, in any directory. A different `javac` release may emit different
  bytecode; nothing here can change that.
* **No new dependency.** The brief allowed making `zip` a build-dependency; the previous
  engineer wrote the (stored-only) archive by hand instead, about 70 lines, and `zip` is used only
  by the tests that read the result back. `Cargo.toml` and `Cargo.lock` are unchanged.
* The jar ends up in `OUT_DIR/bastion-companion.jar` and is embedded with `include_bytes!`
  (`companion::JAR`, about 66 KiB).

### Injection (`src/companion/mod.rs`; an import and four small insertions, 19 lines, in `src/launch.rs`)

`companion::plan` decides (`decide`) and prepares; `Plan::attach` adds to the sandbox being
assembled:

* `Mount::RoData` `/bastion/companion/bastion-companion.jar` (the embedded bytes, mode 0444),
* `Mount::RoData` `/bastion/companion/config.properties` (the settings, rendered for this
  launch),
* `Mount::RoBind` of the channel's socket at `/bastion/companion/channel`, plus that path in
  the Landlock policy's `unix_sockets`,
* the JVM option `-Dfabric.addMods=/bastion/companion/bastion-companion.jar`.

`fabric.addMods` was checked in Fabric Loader 0.20.1 with `javap -c`
(`net.fabricmc.loader.impl.discovery.ArgumentModCandidateFinder`): the system property is
split at `:`; an entry starting with `@` names a file listing paths; a path that does not
exist is skipped with a warning; a jar is accepted as a mod candidate. The loader also
reads a *game argument* of the same name; the launcher does not use that.

The decision, in order: `enabled`; a Fabric loader in the signed manifest (vanilla, a
profile without loader, Quilt: no); at least one feature on; the Minecraft version in
`companion::TESTED_MINECRAFT` unless `force`; the profile does not set `-Dfabric.addMods=`
itself (its option would come later on the command line and win, so the launcher steps
back and says so); Java 17 or newer. The outcome is one line in the launch log
(`Plan::describe`): added, with version, SHA-256, size and the features that are on — or
not added, with the reason.

### The tested-version list, and how to extend it

`companion::TESTED_MINECRAFT` in `src/companion/mod.rs`, today `["26.3"]`. To add a version:

1. install it as a signed Fabric profile in a test data root;
2. `BASTION_E2E_INSTANCE=<that profile> … cargo test --test companion_game`, also with
   `BASTION_COMPANION_WORLD=1` — with `force = true` for that instance while it is not yet
   listed (in its `instance.toml`);
3. if every feature reports in (`features: zoom, hud, fullbright` in the hello and no
   `disabled:` line) and the test passes, add the version to the list and to the comment
   in `examples/config.toml`;
4. if a name has changed, the feature says which lookup failed; fix it in the feature's
   `resolve`, or — if the two versions differ — make the lookup try both.

The mod finds everything by the names Minecraft has shipped unobfuscated since 26.1. On
versions before that (obfuscated names) every lookup fails and every feature switches
itself off; `force` does not make it useful there.

### Configuration

`CompanionConfig` (`[companion]` in config.toml) and `CompanionOverride` (`[companion]` in
instance.toml, every key optional), merged per launch and validated (the key name's shape,
`zoom_factor` 1.1–50, `fullbright_gamma` 1–100; unknown keys are errors). The result is
rendered to `key=value` lines:

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

The mod reads that file once (`Config.java`: at most 16 KiB, anything unreadable means
"every feature off", numbers outside their range fall back to the default). It has no API
for writing a file at all (a test scans its class files for one). `self_test` is not a
configuration key: it exists only in the Rust struct, for `tests/companion_game.rs`.

**Where the launcher's file exists, it is the only source.** The mod also knows a system
property, `bastion.companion.config`, that names a settings file — it is how the tests run
the mod in a plain JVM, where there is no launcher. It is consulted only when
`/bastion/companion/config.properties` does not exist, which in a session the launcher set
up never happens. The order is deliberate, and was the other way round until the third
engineer's review: a signed profile's loader options are `-Dname=value` properties of the
profile author's choosing and come after the launcher's own, so a profile carrying
`-Dbastion.companion.config=/game/…` would have made the mod read a file inside the game
directory — settings that anything in the game could then rewrite for later sessions
(fullbright switched on, say, on a server that forbids it). A test showed it (it failed
against the old mod) and now guards it: see part 4. If the launcher's file is there but
unreadable, the mod has no settings; it does not go looking elsewhere.

### The feature registry

`Feature.java` is the base class: `configure` (read settings), `resolve` (look up what it
needs — throwing means "not available here"), and the hook methods `Hooks.java` calls. Any
failure — at start-up or later inside a hook — switches that one feature off for the
session, with one log line and one diagnostic to the launcher; no exception ever reaches
the game, and the mod's log output is capped at 120 lines per session. `Hooks.ALL` is the
registry.

A new feature is: a subclass of `Feature`; an entry in `Hooks.ALL`; if it needs a new
injection point, a hook method in `Hooks` and a mixin class (listed in
`bastion-companion.mixins.json`) that calls it; its switch in `CompanionConfig`,
`CompanionOverride`, `merged`, `features` and `settings` in `src/companion/mod.rs`; a line
in `examples/config.toml`; a stand-in class in `tests/java/fakegame` if it touches a game
class the others do not.

Where each feature hooks in (all found by inspecting the 26.3 classes with `javap`):

| Feature | Injection point | Why there |
|---|---|---|
| Zoom | `Camera.calculateFov(F)F`, at its last return | called once per frame by `Camera.update`; the early return (panorama screenshots, a fixed 90°) is left alone; the first-person hand has its own method (`calculateHudFov`) |
| | `MouseHandler.turnPlayer(D)V`, at its head | the accumulated mouse movement is scaled there by tan(zoomed/2)/tan(normal/2); the game resets the sums itself afterwards |
| | `MouseHandler.onScroll(JDD)V`, at its head, cancellable | the wheel changes the factor and is consumed only while the key is held *and* nothing is in front of the world |
| HUD readout | `Hud.extractRenderState(GuiGraphicsExtractor, DeltaTracker)V`, at its end | after the game's own HUD; text through the game's own `GuiGraphicsExtractor.text` |
| Fullbright | `LightmapRenderStateExtractor.extract(LightmapRenderState, F)V`, at its end | reached only when the game has recomputed the light map's inputs; the feature adds (configured value − Brightness option) to `brightness`, so the option, `options.txt` and the Darkness effect's own dimming stay as they are |

The zoom key is read with `InputConstants.isKeyDown(scancode)` (in 26.3 that is the SDL
keyboard state; `key.keyboard.c` is scancode 6), after resolving the configured name with
the game's own `InputConstants.getKey` on first use. "Playing" means: a player exists, no
screen is open, no loading overlay is up (`Client.playing`).

No further feature was added in this round. Candidates that fit the rule "harmless
client-side convenience, nothing servers commonly forbid" and would be small: a toggle mode
for zoom (press instead of hold), the smooth ("cinematic") camera while zoomed, a clock
line in the HUD readout. Deliberately not candidates: free look, a minimap, anything that
automates input or shows what the unmodified game does not show.

### The channel (`src/companion/channel.rs`, `Channel.java`)

One `AF_UNIX` stream socket. The launcher creates it in a new private directory
(`$XDG_RUNTIME_DIR/bastion-mc/companion-<random>/channel`, or under its cache directory
when there is no runtime directory) before the sandbox starts, bubblewrap binds it
read-only to `/bastion/companion/channel`, and the launcher removes its name from the host
as soon as the sandbox holds it. It is being listened on before the launcher confines
itself; afterwards only existing descriptors are used. In the sandbox, Landlock grants
connecting to that one socket and nothing next to it.

One case leaves the name behind: when the launcher cannot establish that the sandbox holds
the socket (the sandbox failed to start), it keeps the name — bubblewrap might still need
it — and, once it has confined itself, can no longer remove anything in the runtime
directory. Every new endpoint therefore first removes such leftovers (`channel::sweep`): a
directory of this module's own naming that holds a socket nobody listens on, at least a
minute old (so that a socket another launch has just bound is never mistaken for one).

Protocol version 1, mod to launcher only, one message per line (`\n`; an optional `\r`
before it is ignored, but counts towards the line's 512 bytes), fields separated by single
spaces:

```text
hello <protocol> <mod version> <feature>[,<feature>…]      "-" for no feature
diag <feature> <free text>
```

`<protocol>`: 1–4 digits. `<mod version>`: 1–32 of `[A-Za-z0-9.+-]`. `<feature>`: 1–32 of
`[a-z0-9_-]` starting with a letter, at most 16 of them. Free text: sanitised
(`util::sanitize_line`) and cut to 200 characters. Everything else is "not part of the
protocol": the first three such lines of a session are quoted (sanitised, 80 characters),
the rest only counted. Limits: see row J18. At the end of the session the launcher logs one
summary line with the counts.

The mod sends its hello once its features have resolved, then one diagnostic per feature
that switched itself off (`diag zoom disabled: lookup failed (…)`). Sending never blocks
the game: a bounded queue (64 lines) and a daemon thread; with no socket configured, or
none reachable, the mod says so once in the game log and works the same.

### Licence

The mod is first-party code under the project's licence (MIT; `LICENSE` is packed into the
jar as `META-INF/LICENSE`, and `fabric.mod.json` says `"license": "MIT"`). It contains no
code of Mojang, Fabric or Mixin. The stand-ins in `java/stubs` reproduce *names and
signatures only* of seven public API types of Fabric Loader (Apache-2.0) and Mixin (MIT) —
no method bodies, no documentation — and are not distributed in the jar. The stand-in game
in `tests/java/fakegame*` likewise uses the *names* of some Minecraft classes and members
(which the mod must look up by exactly those names) around bodies written for the test; it
is test code and is not shipped. Mojang's jar is not a build input and nothing of it is
redistributed.

---------------------------------------------------------------------------------------

## 4. What was verified, and how

Machine: Arch Linux, kernel 7.2.14, OpenJDK 27.0.1 (`javac` and the game's JVM), bubblewrap
0.13.1, Landlock ABI 10; Minecraft 26.3 in demo mode (no account), Fabric Loader 0.20.1 with
Mixin 0.8.7 (sponge-mixin 0.17.5), Fabric API 0.166.2; gamescope 3.17.0 (installed, so the
launcher's own choice on an X11 session here is the nested compositor); invisible private
displays: headless sway with Xwayland for X11, headless sway for Wayland, each in
bubblewrap.

### Without Minecraft — part of `cargo test`

| What | Test |
|---|---|
| The embedded jar: manifest first, entries sorted, only the mod's own classes and resources, class-file version 61, no debug attributes | `companion::tests::the_embedded_jar_holds_the_mod_and_nothing_else` |
| Reproducibility: two more builds in two other directories give the embedded bytes | `companion::tests::building_the_jar_again_gives_the_same_bytes` |
| The archive writer (order-independent, readable by the `zip` crate, refuses odd names) | `companion::tests::the_archive_writer_is_a_function_of_its_entries` |
| `fabric.mod.json` and the mixin list (version, client-only, no `depends`, nothing `required`) | `companion::tests::the_mod_describes_itself_to_fabric_loader_with_the_launchers_version` |
| The mod names no API for networking (beyond the one UNIX socket), processes, native code, class definition or writing files | `companion::tests::the_mod_names_no_api_for_networking_processes_native_code_or_writing_files` |
| Rendering and validation of the configuration; instance overrides | `companion::tests::the_settings_file_says_what_the_configuration_says`, `…::settings_are_validated_and_keys_are_minecrafts_names`, `…::an_instance_overrides_single_keys`; `config::tests::*` (the example file equals the defaults) |
| The injection decision: Fabric only, the version list, `force`, a profile with its own `fabric.addMods`, Java version | `companion::tests::only_fabric_profiles_of_tested_versions_get_the_mod`, `…::a_profile_that_adds_mods_itself_is_left_exactly_as_signed` |
| What an injection adds (two read-only data mounts, one option, one socket) and that a skipped one adds nothing; no socket place → no channel, mod still added | `companion::tests::an_injection_adds_…`, `…::a_launch_without_the_mod_…`, `…::without_a_place_for_the_socket_…` |
| The channel: grammar, hostile lines, sanitising, line/rate/session/connection limits, a real socket end to end | `companion::channel::tests::*` (14, with the next two rows) |
| A log sink that fails while the channel is logging (it panics: its output is gone) costs the launch nothing but that line: the counts and the summary the launch asks for at its end are still there. Only where panics unwind (debug builds, the tests): the release profile has `panic = "abort"`, so in the packaged launcher a panicking sink ends the process whatever the channel does | `companion::channel::tests::a_logger_that_fails_does_not_take_the_end_of_the_launch_with_it` |
| Socket names left behind by launchers that could not remove them are swept; live, young or foreign ones are not | `companion::channel::tests::names_left_behind_by_dead_launchers_are_removed_and_nothing_else_is` |
| The channel reader on generated and mutated input: same result however the bytes are cut, bounded memory, no control character in anything logged | `robustness::companion_channel_reader`, harness `fuzzing::companion`, libFuzzer target `fuzz/fuzz_targets/companion.rs` (compiles with the others; never run here — no nightly toolchain) |
| The mod in a JVM with no game at all: every feature switches itself off, says so once (log and channel), the hooks return the game's own values 2 000 times over, the launcher's parser understands every line it sent | `tests/companion.rs::where_the_game_is_not_what_it_expects_…`, `…::without_settings_or_without_a_channel_…` |
| The mod's logic against a stand-in game: zoom arithmetic and easing, no zoom without a world / with a screen open / under a loading overlay / while typing, mouse scaling by the tangent rule, the wheel (limits, consumed only while zoomed, forgotten on release), key, factor and wheel settings, unknown and non-keyboard key names, HUD text (and reduced-info, hidden HUD, debug screen), fullbright (Darkness effect, option above the value), a hook that fails 500 times is reported once and leaves the others alone | `tests/companion.rs::with_a_stand_in_game_every_feature_does_what_it_says`, `…::the_zoom_settings_are_honoured_…` |
| A game in which one method has changed: that feature off (one diagnostic), the others on | `tests/companion.rs::a_game_in_which_one_method_has_changed_…` |
| In the real sandbox (bubblewrap, Landlock, seccomp; a Python payload): the two files with the right bytes, nothing writable, creatable, removable or executable, no socket can be bound next to the channel, the Landlock path lists are unchanged apart from the socket, the socket works after its name is gone from the host | `tests/companion.rs::in_the_sandbox_the_mod_is_two_read_only_files_and_a_socket_to_the_launcher` |
| In the real sandbox with the system JVM, started through the launch bootstrap like the game: the JVM loads the read-only jar, the mod reads the launcher's settings and **not** the file the program planted in the game directory and named with the mod's settings property; its hello and its one diagnostic arrive at the launcher (the mod's own channel code — Java NIO, a UNIX socket — through Landlock and seccomp) | `tests/companion.rs::a_jvm_in_the_sandbox_runs_the_mod_on_the_launchers_settings_not_on_a_file_the_game_wrote` |

The stand-in game says nothing about Minecraft; it exists so that the mod's own rules are
checked on every test run. One mutation was tried to see that it bites: with the "no screen
open" condition removed from `Client.playing()`, the first of those tests fails
(`fov_screen_open=17.5`).

### Against the real jars — opt-in (`BASTION_E2E_ROOT`)

`tests/companion.rs::the_stand_ins_match_the_real_fabric_and_mixin_types`: compiled against
the real Fabric Loader 0.20.1 and Mixin jars instead of the stand-ins, the mod is the same
bytes; and every member a stand-in declares exists in the real class with the same
descriptor, the same annotation default, `@Target` and `@Retention` ("19 members and
annotation properties of 7 types agree"). Also read by hand with `javap`.

### In the real game — opt-in (`tests/companion_game.rs`)

The test goes through the whole launch path (`launch::launch`) with the signed `fab`
profile, every feature on, and the mod's scripted **self-test** (`SelfTest.java`, started
only by a switch that no configuration file can set). By default nothing presses a key or
moves a mouse: the state of the zoom key is scripted inside the mod. With
`BASTION_COMPANION_KEYPRESS` the key is really pressed (below).

**Which display the game had** is the launcher's decision, as in any launch, and the test
prints it in the launcher's own words (the `display` item of its protection report). It was
run on all three paths the launcher has:

| Display path | The test printed | How it was asked for |
|---|---|---|
| X11, nested compositor | `nested compositor: a display server of the game's own, clipboard cut` | nothing: gamescope is installed on this machine and `sandbox.nested_compositor` is `auto`. Also with `BASTION_NESTED_LAYERS` (the verified package copy of the same gamescope 3.17.0 instead of the installed one) |
| X11, the game a client of the display itself | `the session's X11 display: the game can watch and type into other windows` | `BASTION_COMPANION_DIRECT_X11=1` (the test sets `nested_compositor = "off"`; the test data root has `allow_x11 = true`) |
| Wayland | `Wayland security context: capture and input injection withheld` | `WAYLAND_DISPLAY` naming a headless sway's socket, no `DISPLAY` (the game then says `Created window using SDL video driver: wayland`) |

(Earlier versions of these notes spoke of runs "directly on the X display" as opposed to
runs "through gamescope". That was a mistake about this machine: gamescope had been
installed system-wide before the first run (19:50 that day, by `/var/log/pacman.log`), so
every launch without further switches was inside the nested compositor — every launch log
and output that was kept says so, from 19:58 on — and `BASTION_NESTED_LAYERS` only
exchanged one copy of gamescope for another. The game as a direct X11 client, and the
Wayland path, were first run by the third engineer.)

Asserted, from the launch log:

* the launcher's `companion:` line with version, SHA-256 of the embedded jar and
  `features: zoom, hud, fullbright`;
* Fabric Loader lists `- bastion-companion 0.1.0` among the mods it loads;
* **Mixin:** no line at warning level or above from Mixin or about the mod, and no mention
  of the mod's mixin classes or configuration (Mixin names them only to report a problem);
  each feature found its callback merged into its target class (`Game.requireHook`) or it
  would have reported itself off — and each callback's effect was measured, below;
* the hello arrived: `[game] companion channel #1: hello: mod version 0.1.0, protocol 1,
  features: zoom, hud, fullbright`, and the launcher's summary `1 connection(s), 1 hello,
  0 diagnostic(s)` — while the launcher was confined;
* the game reached the title screen: once it reported its loading finished, the open screen
  was `net.minecraft.client.gui.screens.TitleScreen`.

Measured at the title screen (the camera's own `calculateFov`, with the injected callback
in it, called with a scripted key and clock; the other hooks with objects of the game's own
types):

```text
zoom key=key.keyboard.c scancode=6 factor=4.000 fov_idle=70.000 screen_open=true
     fov_key_held_real_state=70.000          ← title screen open: the key does nothing
     fov_first_frame=46.233 fov_zoomed=17.500 ← eased in; 70 / 4
     turn_scale=0.21981 moved=21.981/-8.793   ← tan(8.75°)/tan(35°); a movement of 100/-40
     wheel_used=true wheel_factor=4.600 fov_wheel=15.217
     fov_first_frame_out=24.024 fov_after=70.000 wheel_used_when_released=false
fullbright gamma=15.000 option=0.500 brightness=15.000 brightness_under_darkness_0.3=14.700
hud lines=[5 fps]
```

Measured **in a world** (`BASTION_COMPANION_WORLD=1`: the self-test opens Mojang's demo
world the way the title screen's button does, closes the demo's greeting screen, waits
until nothing has been in front of the world for four seconds, then reads the running
game):

```text
world entered window_active=true mouse_grabbed=true screen=none
world zoom  fov_idle=70.000 fov_zoomed=17.500 fov_after=70.000   ← Camera.getFov(), the game's own per-frame value
            turn_plain=30.0000 turn_zoomed=6.5944                ← degrees the *game* turned the player for the same
                                                                   mouse movement: ratio 0.2198, the tangent rule
world wheel slot_before=0 slot_after_notch_while_zoomed=0 factor_before=4.000 factor_after=4.600
            slot_after_notch_released=8                          ← through the game's own MouseHandler.onScroll
world hud   frames_drawn_in_1s=63 lines=[62 fps | XYZ -43.5 / 70.0 / 63.5 | facing north (-Z), yaw -140.4, pitch 0.0]
            direction=north                                      ← the game's own getDirection() agrees
world hud_conditions frames_while_hud_hidden=0 frames_while_debug_overlay=0 frames_afterwards=43
            lines_with_reduced_debug_info=[62 fps]               ← HUD hidden / debug overlay up: not drawn;
                                                                   reduced debug info: the frame rate alone
world fullbright gamma=15.000 option=0.500 brightness=15.000 applied=199   ← the light map's state in the running world
world watched state=playing fov_min=70.000 fov_end=70.000                ← five seconds with nothing scripted:
world watched state=chat    fov_min=70.000 fov_end=70.000                  nothing zooms by itself,
world chat typed=[]                                                        and nothing is typed
```

**The key really pressed** (`BASTION_COMPANION_KEYPRESS=1`, on a private display of my own,
so that the key could not reach anybody else's window): `tests/support/x11-key.py` presses
`c` through the X server's XTEST extension — to the game an ordinary key event from its
display server — once during each of the two watching phases. Nothing is scripted in the
mod then; it reads the game's own keyboard state.

```text
key presses: pressed keysym=0x63 keycode=54 held=1500ms  (twice)
world watching state=playing screen=none
world watched  state=playing fov_min=17.500 fov_end=70.000    ← pressed while playing: zoomed, and back when released
world watching state=chat screen=net.minecraft.client.gui.screens.ChatScreen
world watched  state=chat    fov_min=70.000 fov_end=70.000    ← pressed with the chat open: no zoom …
world chat typed=[cccccccccccccccccccccccc]                   ← … the key is a letter there (it repeats while held)
```

By the second engineer: six runs, every one inside the nested compositor (see the table
above — the key pressed on the outer display reaches the game through gamescope). Five
passed; the very first failed on an expectation of the test itself — one letter in the
chat, where a held key repeats — with the same measurements as the others. By the third
engineer, on the final jar, again on a private display used by nothing else (`:78`): three
runs, all passed —
in the nested compositor (installed gamescope), in the nested compositor
(`BASTION_NESTED_LAYERS`), and with the game as a direct client of the X display
(`BASTION_COMPANION_DIRECT_X11=1`). On the Wayland path no key was pressed: XTEST is an X11
extension, and nothing here can type into a Wayland client from outside (which is the
point of that path).

Screenshots of that private display, taken by hand during one run (not part of a test):
the readout's three lines in the top-left corner, and the zoomed picture with the
first-person hand unchanged.

Runs of `tests/companion_game.rs` (2026-11-13; the first engineer's are not counted).

*Second engineer:* eighteen. Fifteen up to the first round of final runs, thirteen of
them passed; the two failures were defects of the test, not of the mod, and are fixed: the
self-test once began measuring before the demo's greeting screen had appeared (with a
screen open the zoom, correctly, did nothing; it now waits until nothing has been in front
of the world for four seconds, and the test reports an open screen as such), and the first
key-press run expected a single letter. Then, for the readout's conditions (HUD hidden,
debug overlay, reduced debug information), one world-mode run and one title-mode run, both
passed; a last world-mode run was killed with the session (its log ends in the nested
compositor receiving SIGTERM) and says nothing either way.

*Third engineer:* one world-mode run of the jar as taken over (passed), then — after the
change to `Config.java` described in part 3, **on the final jar** (SHA-256 `a5e1857f…`,
67867 bytes) — twelve runs, all passed:

| Mode | Display path | Runs |
|---|---|---|
| title | nested compositor | 2 |
| world | nested compositor | 2 |
| world | nested compositor, `BASTION_NESTED_LAYERS` | 1 |
| world | X11 directly | 2 |
| world + key press (on `:78`) | nested compositor; the same with `BASTION_NESTED_LAYERS`; X11 directly | 1 each |
| title, world | Wayland security context | 1 each |

The measurements were the same in every run as far as they are deterministic (the field
of view, the turn ratio, the wheel's factor, the brightness); frame counts, the frame rate
and where the player looks differ from run to run. The existing `tests/nested_game.rs`
passed for the vanilla and for the Fabric instance (final jar).

*Fourth engineer:* after those runs one more launcher change went in (`channel.rs`: the
channel's lock is recovered when a log sink panicked while holding it; the mod's sources,
the stubs and the build recipe did not change, and the launch log shows the same jar,
SHA-256 `a5e1857f…`, 67867 bytes). On these final sources, on `:77`, in the nested
compositor: title mode and world mode passed, with the same deterministic measurements as
above (`fov_zoomed=17.500`, `turn_zoomed=6.5944` for `turn_plain=30.0000`, wheel factor
4.000 → 4.600 with the hotbar slot unchanged while zoomed, `brightness=15.000`, readout not
drawn with the HUD hidden or the debug overlay up); `tests/nested_game.rs` for the Fabric
instance passed (`features: zoom`, hello, summary); a command-line launch of the Fabric
instance with the default configuration showed the same as before (hello received after the
launcher confined itself, summary, exit status 130 on SIGINT). Not repeated, because what
they exercise — the jar and the display path — had not changed since: the key presses,
the direct X11 and Wayland paths, and `BASTION_NESTED_LAYERS`.

Also run by hand through the command line (`bastion-mc launch … --demo`): the vanilla
instance (`companion: not added — this instance has no mod loader …`, no `fabric.addMods`
on the command line; by the second engineer, and seen again in the launch log of
`nested_game` on the final sources) and the Fabric instance with the default configuration
(`features: zoom`, Fabric Loader lists the mod, hello received after the launcher had
confined itself, summary at the end, exit status 130 on SIGINT; again on the final jar).
After all runs `$XDG_RUNTIME_DIR/bastion-mc/` was empty.

**A game that does not fit — one experiment, by hand.** What Mixin itself does with a
mixin whose target is not there had only been reasoned about. The third engineer built the
mod once with `CameraMixin` aimed at a class that does not exist and `LightmapMixin` at a
method that does not exist (not committed), and ran title mode. With Mixin 0.8.7 in
Fabric Loader 0.20.1 on Minecraft 26.3:

```text
[FabricLoader/Mixin WARN] Error loading class: net/minecraft/client/CameraGone (java.lang.ClassNotFoundException: …)
[FabricLoader/Mixin WARN] @Mixin target net.minecraft.client.CameraGone was not found bastion-companion.mixins.json:CameraMixin from mod bastion-companion
[bastion-companion WARN]  zoom disabled: lookup failed (NoSuchMethodException: hook bastion$zoomFov was not applied to net.minecraft.client.Camera)
[game] companion channel #1: hello: mod version 0.1.0, protocol 1, features: hud, fullbright
[game] companion channel #1: zoom: disabled: lookup failed (NoSuchMethodException: hook bastion$zoomFov was not applied to …)
selftest: loaded screen=net.minecraft.client.gui.screens.TitleScreen player=none
```

The game started and reached the title screen; zoom said once that it was off; the other
two features worked. For the *method* that does not exist (`require = 0`) Mixin logged
nothing at all and merged the handler without calling it from anywhere — the reason why a
feature checks the target method itself (`Game.requireHook`) and why the test measures
each hook's effect instead of trusting that it was applied. The integration test failed on
that build, as it should: its Mixin assertion reported both warnings. This is one
deliberately broken build on the tested game version, not another Minecraft version.

### What the real-game runs do **not** show

* **No person at the keyboard.** The key press above is synthetic (XTEST), on a US
  layout; a physical keyboard, another layout, and the wheel and the mouse as *devices*
  were not involved (wheel and mouse movement were fed to the game's own
  `MouseHandler.onScroll` and to its accumulated movement, and measured from there on).
* **Nobody looked at a dark place with fullbright on.** What was observed is the input to
  the game's light-map shader in a running world (`brightness` 15.0 instead of 0.5). What
  the shader does with it was read, not watched: 26.3's
  `assets/minecraft/shaders/core/lightmap.fsh` ends in `color = clamp(color, 0.0, 1.0);
  … color = mix(color, notGamma(color), BrightnessFactor)` with `notGamma` scaling a colour
  whose largest component is m to 1 − (1 − m)⁴. By that arithmetic a factor of 15 takes
  every colour with m above about 0.02 to full brightness or beyond, and makes anything
  darker about 46 times brighter. `Lightmap` hands the value to the shader as it is (in
  its bytecode the state's `brightness` field goes straight into the uniform block). Both
  read by two engineers, watched by none: the picture actually getting brighter in a cave
  was not looked at.
* **No other Minecraft version.** `force` was tested as a decision (unit tests), with the
  stand-in game, and by the one experiment above (a mod deliberately built not to fit the
  tested game). A real second version of Minecraft was never started with the mod.
* **The Wayland path** was run with a headless sway only: launch, hello, title screen and
  the scripted measurements in the world. No key press there, and no other compositor.
* **No multiplayer.** No account on this machine; the mod was never run against a server.
  In particular, reduced debug information was switched on through the player's own
  option; a server demanding it sets a flag on the player that the same game method
  (`Minecraft.showOnlyReducedInfo`) also consults, which was read, not exercised.
* **No sound, no person playing** — playability (how the zoom *feels*, pointer capture
  inside gamescope) is not assessed.

---------------------------------------------------------------------------------------

## 5. Open, unverified, and for the integrator

See also the hand-over file `docs/notes/companion-mod-progress.md`.

* **Approval prompts**: not built (design in part 2).
* **Further features**: none added; candidates in part 3.
* **A person at the keyboard** — the first thing to do with the real game: hold C in a
  world, turn the wheel, switch fullbright on in a cave; how it *feels* is not something a
  test here could say.
* **`force` on another version** in a real game; **Fabric Loader versions other than
  0.20.1** (a loader too old to know `fabric.addMods` would ignore the option silently: the
  launcher would say "added" and no hello would follow); **Quilt** (not supported).
* **Reproducibility across compilers**: only one JDK is installed here. Two builds with the
  same `javac` agree; whether another vendor's or release's `javac` produces the same bytes
  was not tested (it need not).
* **The front end** shows the companion's lines in its console like any other log line;
  there is no indicator of its own, and `crates/proto` was not changed.
* **`bastion-mc doctor`** does not mention the companion.
* **The instance.toml template** written by `instance new` / `profile new` does not mention
  `[companion]` (not touched: `src/instance.rs` is outside this package).
* **PKGBUILD** (not touched): `makedepends` says `java-environment`; the companion needs
  `javac --release 17`, i.e. `java-environment>=17`. With an older JDK the build stops with
  "the companion mod needs JDK 17 or newer to build".
* **README "Status" numbers** change with the tests added here.
