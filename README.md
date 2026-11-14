# bastion-mc

bastion-mc is a Minecraft launcher for Arch Linux that treats the game as hostile code. A
modded client runs whatever its mods, their updates and the servers it joins make it run,
with your files, your network and your account within reach; this launcher is built for
the day one of them is hostile. It starts the game in a stacked sandbox, lets it reach the
network only through an allowlisting relay of its own, keeps the account's access token
out of the game, checks every game file against Mojang's published hashes and every mod in
`mods/` against a manifest you signed, and confines itself before it reads anything the
game sends. No built-in OAuth client ID, no telemetry, no update check. It talks to
Microsoft, Xbox Live and Mojang directly, with *your* Azure application. Its documents say
what is defended, how each claim was tested, and what is not defended.

Fabric instances also get the launcher's own companion mod (Minecraft 1.20.1 to 26.3; ten
versions of that range were run in the real game): an in-game phone (Spotify, Discord and
other sites you list in a browser that runs in a sandbox of its own, clips, approvals of
Realms joins, settings sync), a redone title screen and darker menus. The mod is interface
only: the launcher decides what each command from the phone may do, and no token, cookie or
other secret of these services enters the game.

It can also run Minecraft: Bedrock Edition — the Android build, started by your installed
`mcpelauncher-client` from your own Google Play purchase — in the same sandbox, with a
profile of its own, mainly for Mojang's Bedrock-only cape events. There, unlike on Java,
the game itself holds your Xbox sign-in for the session;
[Minecraft: Bedrock Edition](#minecraft-bedrock-edition) says what that means.

## Status

**Pre-release.** Version 0.1.0. There is no release, no signed tag and no binary download:
it is built from source. Everything below was built and checked on one development
machine. The Microsoft → Xbox → Minecraft sign-in has never run against Microsoft, so
nothing that needs a signed-in account has run against the live services yet. Neither has
anything of the fourth round that needs an account of yours: the Spotify Web API, a
signed-in Discord in the phone's browser, Realms. Nor has Minecraft: Bedrock Edition (the
fifth round) run with the real game at all: there was no `mcpelauncher-client`, no game
file, no Google Play purchase and no Microsoft account on the development machine, so all
of it ran against stand-ins.

Read [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) before relying on any of it. It lists
what is defended, how each claim is tested, and — in §5 — what is **not** defended.

Built and tested on Arch Linux (kernel 7.2, Landlock ABI 10, bubblewrap 0.13.1, OpenJDK 27,
AMD GPU, X11 session; for the phone's services Firefox 159.0.1, ffmpeg 9.0.2 and
xdg-dbus-proxy).

| | |
|---|---|
| Sandbox, seccomp, Landlock, egress relay, credential broker, the launcher's own confinement, signing, store, TLS policy, event interface, sound-server check, settings sanitisers, account registry, canary, scram, packs, mod catalog, voice-chat relay, companion mod, the phone's channel and dispatcher, the phone's services (browser, Spotify, clips, Realms, settings sync), Bedrock's version store, sandbox profile, relay, sign-in bridge and scram | 986 tests pass in the workspace (`cargo test --workspace`; what that number counts is below this table). Tests in the sandbox, voice, companion, egress, broker, JVM and Bedrock suites start the real sandbox, those of Spotify and Realms the D-Bus proxy and the Realms dialer in theirs; 9 confine a real multi-threaded process the way the launcher confines itself (4 in the confinement suite; the others read the companion channel, drive the Spotify proxy, save a clip, relay Bedrock's traffic and run a whole Bedrock session — its relay, sign-in bridge and hand-back — from inside that confinement), and one more opens, connects and uses a UDP socket in a process confined that way, as the voice relay does for each new flow (the voice relay's own code ran after confinement only inside a real launch, THREAT_MODEL §7 E22); 31 harnesses, one per parser of bytes the launcher does not control, feed generated and mutated input to those parsers |
| Display isolation | Wayland security context: against a real headless sway (the game's socket is listed: 37 interfaces, none of the 16 known privileged ones). Nested compositor: against real gamescope 3.17 on a private X display, including the clipboard cut-off in all four directions and the control run without it. Both with real Minecraft 26.3 started through them. **Not** on a real Wayland desktop session, and playability inside gamescope is not assessed. Both suites were run again on the merged trees of the third and fourth rounds |
| Graphical front end | 131 unit tests (37 of them lay out, click and render the widgets in software; since the fourth round also the Realms approval prompt, and since the fifth the Bedrock sign-in prompt and a Bedrock session's six protection aspects, laid out at four window sizes) and 5 tests of the whole application against a stand-in launcher (one of them plays a Bedrock instance), run by iced's emulator without a display; a real demo launch driven through its backend worker and through the application's own controls (an opt-in test, run on the front end's branch and again on the merged tree of the third round); the real window started and closed on a private X11 display, with the GPU and with the software renderer. **No** click in a real window; **not** run on Wayland; the Realms prompt never shown for a real join, nor the Bedrock sign-in prompt for a real sign-in |
| Arch packages (`makepkg`) | both build, their `check()` passes and both binaries pass the hardening check — run again on the merged tree of the fifth round, from a `git archive` of that tree (the PKGBUILD's own `source` fetches the repository); the `bastion-mc` package also holds `docs/BEDROCK.md` and the desktop entry of the Bedrock sign-in handler. On GitHub the CI workflow builds them the same way; its second run there (2026-11-14, on the tree the fifth round started from) passed all five jobs. **Not**: the packages installed with pacman (the hook that registers the desktop entry was read, not run on them), the fifth round's tree on GitHub (simulated on the development machine) |
| Real Minecraft 26.3 client in the sandbox (demo mode, no account), vanilla and as a signed Fabric profile | runs: hardware OpenGL, window, audio, HTTPS through the relay and the broker, mods loaded from the sealed mount; also as a `--burner` session. Run again after the review fixes (launcher confined before it reads anything from the game; THREAT_MODEL §7 E16), and on the merged trees of the third and fourth rounds, without sound: vanilla and Fabric through the nested compositor, with the companion mod, and with ViaFabric and Simple Voice Chat (whose native libraries load inside the sandbox) |
| Companion mod: zoom, HUD read-out, fullbright (third round) | in the real game (Minecraft 26.3, Fabric Loader 0.20.1): it loads, reports to the launcher while the launcher is confined, and the effect of each feature was measured in a running demo world, on the nested-compositor, direct-X11 and Wayland display paths; on the X11 paths also with the zoom key pressed through XTEST on a private display. **Not**: a person at the keyboard, a multiplayer server; on the nine other families of versions these three switch themselves off (they are written for 26.3's classes) |
| Companion mod on every Fabric release from 1.20.1 to 26.3 (ten families), protocol 2 of its channel, the phone's dispatcher | in the real game (demo mode, Fabric Loader 0.20.1, the nested compositor on a private X display), on the representative of each family — 1.20.1, 1.20.4, 1.21.1, 1.21.4, 1.21.5, 1.21.8, 1.21.10, 26.1.2, 26.2, 26.3 — in four rounds each: at the title screen; in the demo world; in the demo world with the phone key really pressed (XTEST); at the title screen with the final jar, including a menu of the game's own with the dark background. Each time: no warning from Mixin, the family the table names, a hello of protocol 2, commands in and state, notices and pictures out through the dispatcher — all of it against the demo service (fake apps, a test switch). The channel and the dispatcher also under generated and mutated frames. **Not**: the other versions of each family (built against the same stand-ins, never started), Fabric Loader other than 0.20.1, Quilt, the direct-X11 and Wayland display paths |
| The in-game phone, the home screen, the dark menus | `tests/phone_ui.rs` in the default suite, against a stand-in game (no Minecraft): every command the phone sent parses with the launcher's own parser and fits its app's vocabulary; every draw call on 13 window sizes stays inside its place; hostile and mutated launcher messages crash nothing. In the real game on all ten families at two window sizes, in the demo world on 26.3 and 1.21.4, at GUI scale 1 on 26.3 — against the demo service, with keys, typing and a click sent through XTEST, every screenshot looked at. **Not**: the real services behind the phone (each was run without it, see the next rows), mouse drags in the game, the title screen with an account (only demo mode), Wayland, a person at the keyboard |
| The phone's browser: Firefox, headless, in a sandbox of its own; `browser login` | Firefox 159.0.1 in its sandbox against stand-in HTTPS sites served by the test (opt-in, 6 of 6 passed): pictures, typing, clicks, a form sent, links and images from unlisted hosts refused, the request and API-call budgets, the frame rate, and the sign-in window on a private display, whose session the headless browser then had; the `browser login` command itself on that display, as an X11 client and on the same headless compositor's Wayland socket; with `BASTION_NET_TESTS=1` example.com and Discord's sign-in page, not signed in; a real demo launch (vanilla 26.3): Firefox started before the launcher confined itself, the launch's self-test opened a site and got a picture, and no file of the browser's profile could be found from the game's sandbox. **Not**: Discord signed in (needs your Discord account), the phone in the game driving it, captchas, two-factor prompts, passkeys, other Firefox versions, the relay through `launcher_proxy` |
| Spotify on the phone: the desktop app over MPRIS, the Web API | against stand-ins only: a private D-Bus daemon with a stand-in player (an independent implementation, GDBus) behind the real `xdg-dbus-proxy` in its sandbox — properties, every transport command, the proxy refusing other objects, other names and the player's root interface, a process confined as the launcher is driving it; the Web API against a stand-in HTTPS server (refresh, Premium refused, no active device, 429 with `Retry-After`, the sign-in with PKCE); in a real demo launch, phone commands from a test script reached the stand-in player through the proxy after the launcher had confined itself. **Not**: the real Spotify app, Spotify's real Web API and image server (needs your Spotify client ID and account; playback control there needs Premium), the token in the real keyring |
| Clips: ffmpeg in a sandbox of its own, saved from the phone | saving checked with ffprobe; saving from a process confined as the launcher is; the recorder in its sandbox recording a window on a private display; real demo launches of 26.3, through the nested compositor and as a client of the session's X display: phone commands from a test script saved an 8-second clip (240 frames) with its thumbnail, and a frame from each clip shows the game. **Not**: saving from the phone in the game, finding the window by its process on a desktop X server (the private servers here cannot tell), Wayland (not supported), sound (never recorded) |
| Realms: brokered, every join approved outside the game, a one-time pass | against a stand-in Realms service over TLS, with the real broker: the cookie rebuilt around the real token, other headers and cookies dropped, owner operations refused before they leave, a join held until the code is typed, the pass's address in the answer; the real dialer in its sandbox; the real relay with a client in a private network namespace (no connection before the approval, one after it, none a second time); the front end's prompt in its tests. **Not**: the live Realms service, a launch with `approve` set (it needs a signed-in account and a Realm), `realms totp-setup` against the real keyring |
| Settings sync across instances | unit tests on real `options.txt` files written by 1.20.1, 1.21.4 and 26.3, translations in both directions, concurrent updates under the lock; real demo launches: a field of view changed with the keyboard (XTEST) in a 1.21.4 instance was written to the synced copy by the confined launcher at exit, then taken into a 26.3 and a 1.20.1 instance. **Not**: a real launch with any other setting, a burner launch (unit-tested) |
| Vanilla instances of 1.20.1 to 1.21.11 | did not start before this round (the JVM refused the signed client's classes next to the launch bootstrap in the default package); the bootstrap is in a package of its own now (`bastion.boot`): vanilla 1.21.4 started and reached its texture atlases (stopped, exit status 130), and the four suites that start the bootstrap in a real JVM pass. **Not**: the other versions of that range as vanilla instances; the round's other real-game runs were made before the move |
| `scram`: the browser's profile, the Spotify token, the Realms TOTP secret | deleting the profile, and refusing to while a `browser login` window holds it, in `tests/scram.rs`; reading the Spotify token's entry in the kernel's key list, in unit tests. **Not**: removing the Spotify token or the TOTP secret from a real keyring (the tests limit scram to accounts they created, so they cannot reach these two keys) |
| Mod catalog (`profile add`, `profile check`) | against scripted registry answers (28 kinds of refusal, each of which leaves `mods/`, the manifest and the signature as they were) and against a stand-in registry over TLS; the six aliases looked up live at Modrinth; real demo launches with mods it fetched: they load, and their update checks (and Sodium's `lspci`) are refused by the sandbox. **Not**: ViaFabric translating a connection, a Quilt profile, Modrinth through Tor |
| Voice-chat relay (UDP) | 14 tests — 13 in the real sandbox (one of them with a real JVM) and one in a process confined the way the launcher confines itself —, unit tests of its decision table, one generated-input harness; inside a real launch, with the launcher confined, it relayed datagrams for a stand-in client placed in the game's network namespace. **Not**: a real voice session (needs a server and a signed-in account), capture (sound is off on the development machine) |
| `network.launcher_proxy` for the game's connections to Mojang's services (review finding B-M3, fixed in the third round) | against a stand-in SOCKS5 proxy, in the real sandbox; **not** against Tor |
| Minecraft: Bedrock Edition (fifth round): finding the installed `mcpelauncher-client`, importing versions and checking them at every launch (`bedrock client`, `import`, `versions`, `remove`) | against stand-ins only: unit tests on fake install trees and synthetic versions (links, devices, limits, a 3 GiB sparse file, tampering); the `bedrock` commands end to end with the real binary; `/usr/bin/true` standing in for a root-owned client from a package; the parsers of imported files under generated input. **Not**: a real `mcpelauncher-linux` package or a version that `mcpelauncher-ui` downloaded — so the import's limits and the time a launch spends checking a real version are guesses |
| Bedrock: a session in the sandbox — the client's own system-call allowlist, Landlock, a data directory of the session's own, the kept worlds, the stub in place of the client's sign-in window; scram and `doctor` for Bedrock | against a stand-in client (a program the test compiles and starts as the client, in the real sandbox), three sessions of one instance: its arguments, the version held for the session, the world kept and the tokens, a planted library and a crashed session's directory gone, no home, no keyring, no namespaces, the launcher confined; the allowlist's two modes in the kernel with a Python payload; scram with stand-in sessions and token files; `doctor` in isolated directories. **Not**: the real client and game (the allowlist was derived from the client's source and never profiled), a display, sound, a GPU, resource limits, a terminal launch (it takes only a root-owned client under `/usr`) |
| Bedrock: the network — HTTPS by name, RakNet to the servers you list, Realms and event-server joins brokered and approved outside the game | a sandboxed client (Python for RakNet, the system's curl for HTTPS) with the real relay and broker, against a stand-in RakNet server and stand-in Realms and gatherings services over TLS: listed servers reached and everything else refused with the reason, a Realms and an event-server join approved through the trusted input, the pass reaching exactly the approved server, the per-launch authority accepted by curl, the relay working after the launcher confined itself. **Not**: a real Bedrock server, Realm or event server, the game's own TLS stack (the services' paths and answers come from reading a third-party client, gophertunnel) |
| Bedrock: the sign-in outside the game — the bridge, the desktop's handler (`bastion-mc bedrock hand-back`), the front end's panel | the stand-in ran `mcpelauncher-webview` inside the sandbox under the client's own filters: the stub reached the launcher, the page was offered, another sign-in's address was refused, the right one came back rebuilt — pasted at the trusted input, and handed back by the real `bastion-mc bedrock hand-back` with the launcher confined; the desktop entry registered and resolved by `xdg-mime` and `gio` in isolated directories, and `gio open` running the command; the front end's panel in its emulator; Firefox 159 against a stand-in redirect page (it shows the final address only in its developer tools). **Not**: Microsoft's real sign-in, two-factor steps, passkeys, the game's sign-in library accepting the address, a real browser calling the handler, Chromium-based browsers |
| Bedrock: the licence gate, capes on the Java profile, the canary after a session | unit tests: the gate refuses without its switch and says what is accepted; the lines after a signed-in session; the canary names a gained cape, pins it and never stops a launch for it (stand-in profiles). **Not**: a cape claimed in Bedrock appearing in `skin show` and worn with `cape set` — that needs your accounts and bastion-mc's own Java sign-in, which has never run live |
| Microsoft → Xbox → Minecraft sign-in, and everything that needs a signed-in account (the broker against the real Mojang API, `skin`/`cape`, the canary on a real profile, joining a real multiplayer server, Realms) | implemented and tested against scripted replies and a stand-in Mojang; **not yet run against the live services** (needs your client ID). Realms was tested against a stand-in Realms service, and needs a Realm as well |
| Accounts of other services: the Spotify Web API, Discord signed in in the phone's browser | implemented and tested against stand-ins (rows above); **not run against the live services** (needs your Spotify client ID and account, Premium for playback control, and your Discord account) |
| NVIDIA, Java 8-era versions end to end, real `age` binary and hardware plugins, coverage-guided fuzzing (needs a nightly toolchain) | implemented, **not exercised** on the development machine |

**What "986 tests" counts.** A run of `cargo test --workspace --no-fail-fast -- --show-output`
on the merged tree of the fifth round, made for these documents on 2026-11-14 (no display, no
session bus, separate XDG directories and a private runtime directory), printed 44 `test
result` lines — 41 test binaries and 3 doc-test runs; 4 of them hold no test, and the
launcher's doc-test run only an ignored one — that add up to 986 passed, 0 failed, 2 ignored:
the launcher 835 (617 unit, 218 integration), the front end 138 (131 unit, 5 emulator, 2
live) and the event protocol 13. The two ignored ones are a probe of the terminal reader that
needs a pseudo-terminal, and a usage example in the documentation of `src/realms/answers.rs`.
Not every one of them tests something in such a run: 27 are opt-in and return at once
without their environment variables (see [Tests](#tests)), 11 are halves that other tests
start again as child processes, and a few return early, with a message, on a machine that
lacks what they need — in that run the 3 tests of the resource limits, because the private
runtime directory has no systemd user manager in it. (An earlier run of this pass had one
failure, a flaky unit test of the Bedrock store: [`TODO.md`](TODO.md).) The integrator's
run of the same code counted the same; the display suites, the real-game tests and three demo launches were run
separately on it, the tests that reach Mojang's and Microsoft's live services
(`BASTION_NET_TESTS=1`) not. The complete
lists of what is not defended and what was not exercised are THREAT_MODEL §5 and §7; what
is open, and what needs something only you have, is in [`TODO.md`](TODO.md).

## Contents

- [Quick start (demo mode, no account)](#quick-start-demo-mode-no-account)
- [How it protects you, layer by layer](#how-it-protects-you-layer-by-layer)
  - [Verified downloads and signed profiles](#verified-downloads-and-signed-profiles)
  - [The sandbox](#the-sandbox)
  - [Network filtering and the credential broker](#network-filtering-and-the-credential-broker)
  - [Display isolation](#display-isolation)
  - [The launcher's own confinement](#the-launchers-own-confinement)
  - [The phone's helpers](#the-phones-helpers)
  - [Accounts and the token store](#accounts-and-the-token-store)
  - [Emergency: canary and scram](#emergency-canary-and-scram)
- [The graphical front end](#the-graphical-front-end)
- [Mods](#mods)
- [The companion mod](#the-companion-mod)
  - [Minecraft versions](#minecraft-versions)
- [The in-game phone](#the-in-game-phone)
  - [Spotify](#spotify)
  - [Discord and the browser](#discord-and-the-browser)
  - [Clips](#clips)
  - [Realms and Settings on the phone](#realms-and-settings-on-the-phone)
- [The home screen and dark menus](#the-home-screen-and-dark-menus)
- [Realms](#realms)
- [Settings sync](#settings-sync)
- [Minecraft: Bedrock Edition](#minecraft-bedrock-edition)
- [Accounts](#accounts)
- [Emergency](#emergency)
- [Configuration](#configuration)
- [Tests](#tests)
- [Documentation](#documentation)
- [Source map](#source-map)
- [Trademarks and licence](#trademarks-and-licence)

## Quick start (demo mode, no account)

Mojang's demo mode needs no account and no token, so the whole sandbox can be tried before
you have an OAuth application of your own ([Accounts](#accounts) explains signing in).

### Requirements

```
pacman -S bubblewrap jdk25-openjdk      # Java 25 for Minecraft 26.3: the jdk package, see below
pacman -S gamescope                     # X11 sessions: gives the game a display of its own
pacman -S xdg-dbus-proxy firefox ffmpeg # optional, for the phone: Spotify, its browser, clips
```

Kernel ≥ 6.12 with `landlock` in the LSM list and unprivileged user namespaces (the Arch
defaults). Newer kernels add more: the launcher's own confinement needs Landlock ABI 8 for
its file and network part and ABI 9 for all of it (on older kernels it is partial, and
said to be), ABI 10 (Linux 7.2) adds UDP rules; `bastion-mc doctor` says what this machine
can and cannot enforce. A systemd user session provides the resource limits. Building
needs `cargo`, a C compiler (the one Rust links with) and a JDK (`javac`). That JDK must be
17 or newer: the companion mod is compiled with `--release 17` (`jdk25-openjdk` is one).

The launcher uses exactly the Java release a game version declares, from root-owned
runtimes under `/usr/lib/jvm`, and never downloads one; with `match = "newer"` under
`[java]` in `config.toml` (see below) it takes the lowest installed release at or above
the declared one instead. It recognises a runtime by the `release` file in its directory,
and for Java 17 and later Arch puts that file only into the jdk packages (`jdk17-openjdk`,
`jdk21-openjdk`, `jdk25-openjdk`, `jdk-openjdk`), not into the jre ones (package file
lists, read on 2026-11-13): a runtime installed from a jre package is not found. So
install the jdk package of each Java release your game versions need; for Java 8,
`jre8-openjdk` brings the file (through `jre8-openjdk-headless`). The launcher's own
refusal still suggests the jre package, which does not help for Java 17 and later
([`TODO.md`](TODO.md)).

`cargo build --release` leaves the program at `target/release/bastion-mc`. The commands in
this file call it by name, so put that directory on your `PATH` (or call it by its path):

```
cargo build --release
export PATH="$PWD/target/release:$PATH"
bastion-mc doctor              # checks the machine and starts a real test sandbox
bastion-mc --help              # every command and option
```

`packaging/arch/PKGBUILD` does not build this checkout: until there is a release it is a
template, and `makepkg` builds whatever its `source=` line points to.

Settings go in `~/.config/bastion-mc/config.toml`, which does not exist until you create
it: `bastion-mc config example` prints a commented template of every key, and
`bastion-mc config path` prints where the file goes.

### First run

```sh
bastion-mc instance new main --minecraft 26.3     # a vanilla instance
bastion-mc launch main --demo                     # downloads and verifies the game's files, then starts it
```

The first launch fetches the client, its libraries and its assets from Mojang and checks
each against the published hashes (`bastion-mc install main` does that without starting
the game). Before anything the game prints is shown, the launch says what it set up and
what it saw in force; from a run on the development machine, shortened:

```text
network: filtered — HTTPS to *.{minecraft.net, mojang.com, minecraftservices.com} only (+ skin textures via HTTPS gateway)
auth: demo mode — no account, no token
[launcher] display: the nested compositor's clipboard bridge is cut — the cut-off reported from inside the compositor that the bridge's calls land in it
[launcher] limits: in force — memory.max, memory.swap.max, pids.max (and cpu.max, where a quota is set) of the sandbox's control group were read back from the kernel
[launcher] launcher: confined itself, on all threads — Landlock: no program execution, files read-only under /etc, /usr and /proc and writable only under the instance's persist/ and the cache, TCP only to port(s) 53, 443; system-call filter: no kernel keyring, no new namespaces
```

Ctrl-C in the terminal stops the game ("game stopped on request", exit status 130). Two
checks can stop a first launch on purpose, and their messages say what to do: on an X11
session without gamescope the launch is refused (see [Display isolation](#display-isolation)),
and with sound on (`audio = "pulse"`, the default) it is refused if your sound server loads
modules for its clients, as PipeWire does as shipped (see [Sound](#sound); `audio = "none"`
under `[sandbox]` tries it silently). `bastion-mc status` lists accounts and instances;
`bastion-mc-gui` (built by `cargo build --release --workspace`) is the
[window](#the-graphical-front-end) for the same things.

(This sequence was run on the merged tree of the third round on the development machine,
with a debug build — through gamescope on a private X display, with `audio = "none"`, and
with `match = "newer"` because the machine has no Java 25: it has Java 21 from a jre
package, which the launcher does not find, and Java 27 from `jdk-openjdk` — together with
`doctor`, `status`, `versions` and `config example`.)

## How it protects you, layer by layer

Each layer is described here with its limits. The attack-by-attack rows, with the test
behind each claim, are in [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md).

### Verified downloads and signed profiles

* Every Mojang file is checked against the published hash chain on download and again at
  every launch. A file that does not match stops the launch; `--repair` fetches it again.
* Modded instances exist only by virtue of a manifest you signed (Ed25519); `mods/` must
  match it exactly, and its serial cannot go back ([Mods](#mods)).
* Mods fetched from Modrinth with `profile add` are checked for size and SHA-512 (and SHA-1,
  where the registry lists one) before they reach `mods/`, and they are nothing until you
  sign the manifest.
* The game's JVM is one of your system's root-owned runtimes; the launcher never downloads
  one.

### The sandbox

bubblewrap namespaces → Landlock → seccomp allowlist, inside a systemd scope with memory and
task limits; an empty root filesystem, a synthetic `/etc`, no route out of the network
namespace except through the launcher's allowlisting relay. The holes are one audio socket
(raw `/dev/snd` instead with `audio = "alsa"`), the GPU, and a display of the game's own: on Wayland a socket the compositor knows to be
sandboxed, on X11 a nested compositor. When the launcher adds its companion mod to a Fabric
profile, the sandbox also gets one socket to the launcher. The mod's reports arrive there and
are only logged; the phone's commands arrive there too, and the launcher does for them only
what the named app's fixed vocabulary and rate limits allow
([The in-game phone](#the-in-game-phone)). None of the phone's helpers — the browser, the
D-Bus proxy, the clip recorder, the Realms dialer — runs inside the game's sandbox
([The phone's helpers](#the-phones-helpers)).

Inside an instance only `saves/`, `logs/` and `config/` are writable by the game; `mods/`,
`resourcepacks/` and `shaderpacks/` reach it as sealed read-only mounts of verified files.

**Limits.** The sandbox runs in a systemd scope: memory ceiling (default: twice the heap plus
2 GiB), no swap, 4096 tasks; `cpu_quota_percent` adds a CPU ceiling. Once the game runs the
limits are read back from the kernel: if they are not in force, the launch says so and does
not show them as on (`cgroup = "require"` stops it).

**Content you do not trust.**

```sh
bastion-mc launch main --burner
```

No network, no sign-in, no token; the game works on copies of the instance's worlds and
settings, and everything it writes is deleted when it exits — the launcher says so when
the deletion worked, and says what is left when it did not. The copy is bounded by the
free space of the cache. For a downloaded map or pack you want to look at before it gets
near your real saves. (The full game starts only for an account that passed the ownership
check at its last sign-in here; otherwise demo mode.) Such a session starts no browser for
the phone and takes no part in settings sync or Realms
([The in-game phone](#the-in-game-phone)).

### Network filtering and the credential broker

In the default `filtered` mode the game's network namespace holds nothing but sockets of
the launcher. Names resolve only when they are allowlisted, HTTPS reaches only Mojang's
service domains (the relay reads the server name the TLS handshake asks for), skin
textures come through a gateway, game servers are reachable only when you allowlist them,
and nothing else leaves.

The other two modes (`mode` under `[network]`, or per instance): `none` gives the game a
network namespace with nothing in it, no network at all (a `--burner` session always runs
so); `open` gives it the host's network: no relay, no allowlist, no credential broker (the
session token goes into the game), and services listening on the host's loopback are
reachable. `open` exists for what the filtered mode cannot carry, and gives up most of what
this section describes.

Multiplayer servers must be allowlisted (`servers = [...]` under `[network]` in
`config.toml` or in the instance's `instance.toml`); everything else is unreachable.
`bastion-mc launch main --join play.example.org` allowlists one server for one session.

**The game does not get your access token.** It is given a placeholder; the launcher keeps
the real token and attaches it to the handful of Mojang API requests that playing needs.
Nothing in the game can change your skin, cape or name, or carry the token away. (Where the
token does have to be in the game — open network mode — it still never touches a command
line or the environment.) What code in the game can still do is ask for what is switched
on: account reads, and one join per game connection it opens — a join that is good for a
login on any server, not only the one the connection goes to (THREAT_MODEL J13 and §5).
[`docs/TOKEN_INJECTION.md`](docs/TOKEN_INJECTION.md) follows the token's path in detail.

**Realms** is refused unless you choose how its joins are approved (`[realms] approve`). Then
the broker also answers for the Realms hosts, and every join waits for an approval you give
outside the game before the game gets one connection to that Realm's server
([Realms](#realms)).

**Voice chat** is the only UDP the launcher relays out of a Java game's sandbox, and only to
a port that a server's rule names: [Voice chat on a server](#voice-chat-on-a-server). (A
Bedrock game's servers speak RakNet, over UDP, and are relayed only when you list them:
[Minecraft: Bedrock Edition](#minecraft-bedrock-edition).)

**Proxy.** `launcher_proxy = "socks5h://127.0.0.1:9050"` under `[network]` sends everything
that goes to Microsoft and Mojang through a SOCKS5 proxy such as Tor, which also resolves
the names: sign-in, downloads, and — while the game runs in the default filtered mode —
what the credential broker forwards, skin textures, and the game's own HTTPS connections to
Mojang's services. **Not** proxied: connections to multiplayer servers, the lookup of their
names (your resolver sees those), and voice. With `mode = "open"` nothing the game sends is
proxied. The launch log states which of these applies. (`profile add` and `profile check`
reach Modrinth through the same proxy, and so do the phone's Spotify Web API and covers, the
broker's requests to Realms and the phone browser's connections to its sites; none of these
three was tried through a proxy here. A Realm's server is connected directly, like a
multiplayer server.)

### Display isolation

X11 lets any client log keys, inject input into other windows and read the clipboard, so
handing the game the session's X display would be a way out of the sandbox. The launcher
does not do that:

* On an **X11** session it runs the game inside **gamescope** (`pacman -S gamescope`), a
  nested compositor that itself runs in a sandbox. The game gets a display server of its
  own and nothing of your session's — no other windows, no keystrokes, and no clipboard in
  either direction: gamescope would bridge the clipboard, so the launcher loads a cut-off
  into it, and that cut-off reports from inside the compositor. Without its report the
  launcher says that the clipboard is *not known* to be cut (`nested_compositor =
  "require"` refuses instead). Options such as size and full screen go in
  `nested_compositor_args = ["-W", "1920", "-H", "1080", "-f"]`.
  Without gamescope the launch is refused, unless you set `allow_x11 = true` and accept
  the risk.
* On a **Wayland** session it asks the compositor for a socket that is marked as sandboxed
  (`wp_security_context_v1`), on which a compositor is expected to withhold screen capture,
  input injection and clipboard-manager access. The protocol only says "should", so the
  launcher connects to that socket itself and lists what is offered there; if a privileged
  interface it knows is still on offer, it says so (`wayland_security_context = "require"`
  refuses). According to the protocol registry's support table (wayland.app, read
  2026-11-13) KWin (KDE Plasma), Sway, Hyprland, niri, COSMIC, labwc and river implement
  it; Mutter (GNOME), Weston and Wayfire do not. Only sway was tested here. Where a
  compositor lacks it, the launcher says so and uses the session socket
  (`wayland_security_context = "require"` refuses instead) — on GNOME that means the game
  is an ordinary Wayland client of your session.

`bastion-mc doctor` shows which of the two this machine offers; the front end shows what a
running game got in the protection read-out of its Launch page.

#### Sound

PulseAudio and pipewire-pulse both let any client ask the server to load modules, by
default, and that leads out of the sandbox. The launcher asks your sound server before each
launch and refuses to hand the game the audio socket unless the server says no. To keep
sound, switch that off once (PipeWire):

```sh
mkdir -p ~/.config/pipewire/pipewire-pulse.conf.d
echo 'pulse.properties = { pulse.allow-module-loading = false }' \
    > ~/.config/pipewire/pipewire-pulse.conf.d/90-no-module-loading.conf
systemctl --user restart pipewire-pulse
```

(`pactl load-module …` stops working for everything, which is the point.) Otherwise set
`audio = "none"`, or `allow_pulse_module_loading = true` to accept the risk. With audio on,
the game can record your microphone (THREAT_MODEL D3). `audio = "alsa"` hands the game the
raw sound devices (`/dev/snd`) instead of a server's socket: playback and capture of every
sound card, with no server to ask and no module-loading check, the most access of the three.

### The launcher's own confinement

Before it reads anything the game sends, the launcher gives up — on all its threads —
writing anywhere but one settings directory of the instance (`persist/`), its cache and,
for the phone's services that are on, the settings-sync directory, the clips folder and the
clip buffer; reading anything else but `/etc`, `/usr`, `/proc` and the name resolver's
configuration; program execution; most of the network; and the kernel keyring where the
sign-in token rests (Landlock, and a system-call filter for what Landlock does not cover).
Every helper of the phone's services is started, and every socket to one connected, before
that point; the launch log's confinement line names the directories added for them. The
whole of it needs Landlock ABI 9. On an older kernel `launcher.confine = "auto"` applies what
it can and says what is missing — in the launch log, in `doctor` and in the front end —, and
`"require"` refuses to launch. Not restricted even then: UDP (the DNS responder, the voice
relay and, in a Bedrock session, the RakNet relay answer to the game's ephemeral ports), TCP
by destination (the rules are per
port), signals to your other processes (stopping the game needs them), and the access
token the broker holds for the session — a compromised launcher has it, and can ask the
Spotify token keeper for fresh Spotify access tokens and the Realms dialer for its few
connections.

**What is shown as protected was looked at, not assumed**: the resource limits are read
back from the kernel, the game's Wayland socket is listed, the clipboard cut-off reports
from inside the compositor — and where a check fails the launcher says so, or refuses.

### The phone's helpers

The phone's services need programs of their own. Each is started before the launcher
confines itself, and all but one run in a sandbox of their own (bubblewrap, Landlock and a
system-call filter) that shares nothing with the game's sandbox except what the feature
needs:

* **The browser**: the installed Firefox, headless — no home directory, no display, no D-Bus,
  no sound. Its only way out is the launcher's relay, which answers name lookups for the
  hosts of the site that is open and for nothing else, takes a connection only when its TLS
  handshake names one of those hosts, and holds each site to its budgets. Its system-call
  filter is a denylist, so that Firefox's own sandbox for its content processes keeps
  working. The profile (your site sessions) is in `~/.local/share/bastion-mc/browser/`,
  outside the game's sandbox, and the confined launcher cannot read it either.
* **The D-Bus proxy** (Spotify over MPRIS): `xdg-dbus-proxy` passes the Spotify app's player
  and properties interfaces on its one object path, and nothing else of your session bus.
* **The clip recorder**: ffmpeg, with the game's display to read — with the nested
  compositor that is the game's own display; as a client of your X display (`allow_x11`) it
  is your whole X session, as the game has it then — and a buffer directory in memory to
  write. No network.
* **The Realms dialer**: makes the one connection to a Realm's server after you approved the
  join — public addresses only, at most 8 a session — because the confined launcher cannot
  connect to a port it did not know in advance.
* **The Spotify token keeper** is the one helper without a sandbox: it needs the kernel
  keyring or the age vault, where the refresh token rests. It refreshes the access token
  when the launcher asks — at most every 20 seconds and 60 times a session — and hands over
  only that, never the refresh token.

The game reaches none of these programs. It reaches the launcher's apps through the phone,
with the words of each app's vocabulary and nothing else
([The in-game phone](#the-in-game-phone)).

### Accounts and the token store

Each account is a label of your choice with its own stored refresh token
([Accounts](#accounts)). Where that token rests is a choice with consequences. The same
store keeps, under names of their own, the Spotify refresh token (`bastion-mc spotify
login`) and the secret of the Realms TOTP approval (`bastion-mc realms totp-setup`); with
`token_store = "none"` neither can be kept.

#### Where the refresh token rests

`token_store = "keyutils"` (default) keeps it in the kernel keyring: RAM only, gone at
reboot, unreachable from the sandbox and (once the game runs) from the launcher itself —
and readable by other programs of yours in the same login session.

`token_store = "age"` keeps it in a file encrypted by `age`. What that is worth has two
halves, and `bastion-mc doctor` reports on both:

* **Who can open a vault file that exists** depends on the identity. With an identity kept
  in a device (`age-plugin-yubikey`, `age-plugin-tpm`, `age-plugin-fido2-hmac`) decrypting
  needs the device, so a *copy* of the vault file — a backup, a stolen disk — is useless.
  It does not need more than the device: a program running as you can ask a connected
  device to decrypt, and only the device's touch or PIN policy stands in the way.
* **To whom the next vault file is encrypted** depends on the recipients file, and
  encrypting needs no device. The launcher re-encrypts the token at every launch; if the
  recipients file is one of your own files, a program running as you can add its own
  recipient and read your token from then on. To close that, have root keep the list:
  when `/etc/bastion-mc/age-recipients` exists (root-owned), the vault is encrypted to
  exactly those recipients, the `recipients_file` settings are ignored, and the launcher
  refuses to store tokens in the keyring instead.

Neither half keeps away a program that stands between you and the launcher itself — another
`bastion-mc` earlier in `$PATH`, a wrapper, a preloaded library: it has the token when you
launch. That takes a separate UNIX account
([`docs/MC_VAULT_ACCOUNT.md`](docs/MC_VAULT_ACCOUNT.md), which also has the commands for
the root-owned list).

List a second recipient (a backup key kept offline): a TPM identity sealed to firmware
state stops working after a firmware update, and `bastion-mc vault reseal` re-encrypts the
vault to the current recipients. Every recipient can decrypt the vault, so the backup
key's secret half must not be on this machine. None of this was run against a real `age`
binary or device here (see [Status](#status)).

### Emergency: canary and scram

For the day something goes wrong: a canary that notices when the account's name, skin or
cape changed behind your back ([The profile canary](#the-profile-canary)), `bastion-mc scram`
to remove every credential the launcher keeps from the machine in one step — the sign-ins,
the Spotify token, the Realms TOTP secret, the phone browser's profile — and to say what it
could not remove, and a built-in recovery runbook with verified links ([Emergency](#emergency)).
Scram also stops a running Bedrock session and deletes what killed or crashed Bedrock
sessions left on disk, which can hold the game's Xbox sign-in.

## The graphical front end

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

Screenshots, from the front end's own tests (rendered in software from test states, not
taken from a real session): [the Launch page while a game runs, with one aspect of the
launch reduced and one not protected](docs/images/gui-launch-running.png), [the Accounts
page with three local accounts](docs/images/gui-accounts.png), and [the Security page after
a scram the launcher reported as incomplete](docs/images/gui-security-scram-incomplete.png).
[`docs/README.md`](docs/README.md) shows them inline.

A navigation rail on the left leads to five pages; the rail also shows which account a
launch would use, and a small square next to a page when something is going on there or
needs you (a launch under way, a sign-in waiting for its code, a protection that is reduced
or not in force, a scram that is running, incomplete or could not be started).
What the launcher is doing is named at the top right of every page.

* **Launch.** The gauge with the launch control in its middle; the instance that would be
  launched (a list, and *Manage* leads to the instances); the demo-mode switch; the five
  stages of a launch; what the running or the last launch is protected by. *Play* can be
  pressed exactly when a launch can start — otherwise the line under the gauge says why
  not. Stopping is a separate button, so a second press on the launch control can never
  stop a game. While a Realms join waits for your approval, the page shows the prompt: the
  Realm, the server address the game would connect to, in `code` mode the six-digit code
  (in two groups of three), and how long it is still valid; when the prompt ends it says
  how ([Realms](#realms)). A Bedrock instance needs neither a Java sign-in nor demo mode. While
  its game waits for a Microsoft sign-in, the page shows the *Microsoft sign-in for Bedrock*
  panel (*Open the sign-in page*, *Copy link*, a field to paste the address into with
  *Send*, and *Cancel*), and a Bedrock session's Realms prompt is answered on the page, with
  *Approve join* or *Refuse* ([Minecraft: Bedrock Edition](#minecraft-bedrock-edition)).

  The gauge, from the outside in: the **sequence** (one arc per stage — files, verify,
  sign-in, sandbox, game; the stage being worked on has a highlight travelling along it),
  a **scale**, the **progress** ring (download progress against the scale; closed and
  slowly breathing while the game runs; amber and still while it is being stopped), and
  the **protection** ring with one segment per aspect of the launch.

* **Protection read-outs.** A launch reports five aspects — *display*, *network*,
  *credential*, *limits*, *launcher* — each at one of three levels; a Bedrock launch reports
  a sixth, *sign-in*: where the game's sign-in pages are shown, and who may answer them. The
  level is always a word and a shape, never a colour alone:

  | Level | Word | Mark | Ring segment |
  |---|---|---|---|
  | on | IN FORCE | filled shield | solid |
  | partial | REDUCED | half-filled shield | broken into three |
  | off | NOT PROTECTED | hollow shield, struck through | two thin lines |
  | — | NO REPORT | outline | one faint line |

  Before any launch the page lists the five aspects with "no report"; nothing is claimed
  about a launch that has not happened. After the game has ended the read-outs stay, under
  the heading "last launch".

* **Accounts.** One card per local account (a *label* with its own stored token, as for the
  launcher's `--account` option): the label, the player name where the launcher knows one,
  whether a token is stored, where it rests, which account is in use and which is the
  launcher's default. *Switch to* uses another account for launching (nothing is signed in
  or out by that); *Make default* changes what the launcher uses when no account is named;
  *Sign in* and *Device code* start a sign-in for that account, *Sign out* removes its
  token. A new label is added with the tile at the end. There is no picture of the player:
  the launcher's events carry none, so the square shows a letter.

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
  the lines that say what is still there set apart. If the launcher has not finished after
  30 minutes the window stops waiting, leaves it running and says so. A sign-in this
  window started is ended before the scram begins, and no sign-in or launch can start while
  it runs. Below: what the last launch ran with, in words, with what each aspect covers;
  the twelve recovery pages of the runbook (*Open* and *Copy* for each — these twelve
  addresses and no others); and the runbook itself.

While a game runs, controls that cannot apply (another launch, signing in or out, changing
the instance, creating one) are drawn and not offered. One launcher command runs at a time;
the one exception is the scram, which works beside a running game because ending that game
is part of what it does.

The window opens at 1180 × 720 and can be made as small as 640 × 520: below 800 wide the
rail shows its marks only (with the page names as tool tips), and pages fold into fewer
columns or scroll. Nothing moves on screen while nothing happens: the window has no timer
when it is idle, 25 steps a second while a launch is being prepared and five while the
game runs (both only while the Launch page is shown), and one a second while a sign-in
prompt, a waiting Realms prompt or a scram is counting.

Rendering uses the GPU when one is usable and falls back to software otherwise
(`ICED_BACKEND=tiny-skia` forces software). No font or image is bundled: text uses Adwaita
Sans and the system's monospace face where they are installed and whatever sans-serif and
monospace faces there are otherwise; every symbol is drawn as geometry. It knows nothing of
voice chat, the companion mod or the phone's services beyond the log lines in its console,
except the Realms prompt above. It offers none of the fourth round's commands
(`browser login`, `spotify …`, `realms totp-setup`): they are run in a terminal. Nor the
`bedrock` commands that find the client and import versions.

It is a separate program on purpose, not a mode of the launcher:

```text
 bastion-mc-gui  (no tokens: window, fonts, GPU)         bastion-mc --events <command>
 ┌───────────────────────────────────────────┐           (hardened, holds the tokens)
 │ view   state ─▶ widgets, pure             │
 │ app    update(): the only place state     │  spawn    one process per command:
 │        changes; never blocks              │ ────────▶ status · account default · login
 │ backend  tokio task per command:          │           logout · versions · instance new
 │        child stdout ─▶ typed Update ─▶    │           launch · scram --yes
 │        tokio mpsc ─▶ iced Task ─▶ update  │ ◀──────── stdout: one JSON event per line
 │                                           │ ────────▶ stdin:  "stop", or an answer to a
 │                                           │           prompt (or just closing it)
 └───────────────────────────────────────────┘
```

* The UI thread never waits on the network, the disk or the game. Every launcher command
  runs in a child process supervised by a tokio task; progress arrives as messages.
* Tokens stay in the launcher process. The event protocol
  ([`crates/proto`](crates/proto/src/lib.rs)) has no field that could carry one, and the
  launcher binary links none of the GUI's dependencies. What the front end writes back, in a
  Bedrock session, is what you answer a prompt with: a Realms code, or the address a
  Microsoft sign-in ended on (`answer realms …`, `answer signin …`), whose code is
  single-use and worthless without the verifier the game holds.
* If the window is closed, or the front end dies, the launcher notices its control pipe
  closing and shuts the game down properly. A scram is the exception: the front end never
  ends the launcher that is removing credentials.

`bastion-mc --events <command>` is usable on its own (`… | jq`): `status`, `login`,
`logout`, `account list|default`, `versions`, `instance list|new`, `install`, `launch`,
`scram --yes`.

Building the front end needs nothing beyond the Rust toolchain; its system libraries are
loaded at run time:

```
pacman -S --needed rust                              # build (base-devel for the linker)
pacman -S --needed libxkbcommon ttf-dejavu           # run: keyboard handling, any TTF font
pacman -S --needed wayland                           # run on Wayland
pacman -S --needed libx11 libxcursor libxi libxkbcommon-x11   # run on X11
pacman -S --needed vulkan-icd-loader vulkan-radeon   # GPU rendering (or vulkan-intel / nvidia-utils)
pacman -S --needed xdg-utils                         # the "Open link" button
```

Without a usable Vulkan or OpenGL driver it renders in software; `ICED_BACKEND=tiny-skia`
forces that. An `age` vault whose identity asks for a PIN on the terminal cannot be unlocked
from the window; sign in and launch from the command line in that setup. What the front end
does and does not defend is THREAT_MODEL §4.5.

## Mods

Mods run only in a curated profile: a Fabric (or Quilt) instance whose `mods/` directory
is described by a manifest you sign. The full reference — every rule of `profile add`, its
exit statuses, what Modrinth learns, the aliases, the voice-chat limits — is
[`docs/MODS.md`](docs/MODS.md).

### Signed profiles

```sh
mkdir -m 700 -p ~/secure                           # key generate does not create directories
bastion-mc key generate ~/secure/bastion.key       # ideally kept off this machine
bastion-mc key trust me ed25519:…                  # the public key it printed

bastion-mc profile new perf --minecraft 26.3 --fabric
bastion-mc profile add perf sodium lithium modmenu     # from Modrinth, dependencies included
bastion-mc profile check perf                          # what Modrinth knows about each jar
$EDITOR ~/.local/share/bastion-mc/instances/perf/profile.json    # review it
bastion-mc profile sign perf --key ~/secure/bastion.key
bastion-mc launch perf                 # add --demo until your client ID is approved
```

A mod that is not on Modrinth is copied into `mods/` by hand and recorded with
`bastion-mc profile lock perf`, which writes name, size and SHA-256 of each jar into the
manifest and says which sensitive Java APIs an updated mod newly refers to.
`bastion-mc profile audit perf` lists, per jar, the sensitive APIs its classes refer to (a
signal for your review: reflection, classes defined at run time and native code can hide
anything); `bastion-mc profile verify perf` checks signature and `mods/` exactly as a launch
would.

At launch the signature, the manifest's serial (no rollback), every loader library and every
jar in `mods/` are verified; one extra, missing or modified file aborts the launch. If
root-owned keys exist in `/etc/bastion-mc/trusted-keys.d/`, only those are trusted. The one
mod that runs without being in your manifest is the launcher's own companion mod
([below](#the-companion-mod)).

### profile add and profile check

`profile add <id> <mod>…` fetches mods from [Modrinth](https://modrinth.com) into the
profile's draft. `<mod>` is a Modrinth slug, a project id, or a built-in alias:
`viafabric`, `simple-voice-chat` (also `voicechat`), `fabric-api`, `sodium`, `lithium`,
`modmenu` — an alias names its project by id, because a slug can be given up and taken by
somebody else. For each mod it takes the newest **release** that Modrinth lists for the
profile's Minecraft version and loader, resolves what that build marks as **required**,
and prints, before anything is downloaded: project, slug, project id and licence; version,
channel and publication date; file name, size, SHA-512 and (where the registry lists one)
SHA-1; the address it comes from. Then it downloads into a staging directory, checks size,
SHA-512, SHA-1 where listed and the file's own date, and only when every file is in hand
touches `mods/`. It ends exactly where `profile lock` ends: the manifest lists the new
jars, the serial is bumped, the old signature is gone. **Nothing is signed.** The static
scan of what the new jars refer to (network, native code, starting programs, reflection …)
is printed, so you see what a new mod can reach before you sign for it.

* **Releases only.** A beta or alpha is taken only with `--prerelease`, and only for a
  project that has no release for that Minecraft version.
* **48-hour quarantine.** A build published less than 48 hours ago is passed over (the next
  older release is taken, and the output names what was skipped). Bytes that Modrinth's
  file server has held for less than 48 hours are refused, however old their build is, and
  the command fails. `--allow-fresh` lifts both. It is a delay to let a bad upload be
  noticed, not a verdict; when the file server rewrites its storage, old files meet the
  same refusal for two days, and the message says so.
* **Dependencies.** Required ones are added too, unless a suitable build is already in
  `mods/`; optional ones are only named.
* **Replacing.** A jar is removed only if Modrinth identifies it by its hash as a build of
  the same project, both jars declare the same mod id, and it is older than the build that
  takes its place (or, for a required dependency, is not listed for this Minecraft version
  at all). Nothing leads to a downgrade, and a jar Modrinth does not know is never removed.
* **All or nothing.** A download that does not match, bytes that are too new, an unsafe
  file name, an address outside `cdn.modrinth.com`, a malformed answer: the command fails
  and `mods/`, the manifest and the signature are exactly as they were. `--dry-run` shows
  the plan and downloads nothing.

`profile check <id>` hashes every jar in `mods/` and asks Modrinth which project and version
each one is: **known**, **unknown**, or **fresh** (its build is under 48 hours old). "Known"
means Modrinth lists exactly these bytes under that project — not that they are harmless:
whoever controls a project's account publishes a bad build together with its hash. Only
Modrinth is asked, so "unknown" also covers every mod published only elsewhere.

What Modrinth learns from these two commands: your address (the proxy's, with
`launcher_proxy`), the user agent `bastion-mc/<version>`, the names you asked for, the
profile's Minecraft version and loader, and the SHA-512 of every jar in `mods/`. No
account, cookie or token is involved. A launch contacts neither of Modrinth's hosts, and
the relay does not let the game reach them.

### ViaFabric and Simple Voice Chat

```sh
bastion-mc profile add perf viafabric simple-voice-chat --prerelease
```

`--prerelease` because neither had a release build for Minecraft 26.3 when this was
checked (2026-11-14 UTC; ViaFabric: alphas, Simple Voice Chat: betas). Both load in the
sandbox, Simple Voice Chat with its four native libraries. ViaFabric lets the client join
servers of other Minecraft versions; its client-side translation is off until you switch
it on (`enable-client-side` in the instance's `config/ViaFabric/viafabric.yml`, that is
`~/.local/share/bastion-mc/instances/<id>/config/…`, written at the mod's first start; or
in the game). Simple Voice Chat needs a microphone (`audio = "pulse"`, see
[Sound](#sound)) and UDP to the server's voice port, which a server rule has to name:
`profile add` reminds you of that when it adds the mod, because installing it opens
nothing. Neither mod was tried against a server here: only loading was.

### Voice chat on a server

Voice-chat mods send sound as UDP datagrams to a port on the game server's machine. In the
default network mode the game has no UDP at all, so such a mod loads but stays silent. To
let it talk, name the server's voice port **in that server's rule**:

```toml
[network]
servers = [
    "survival.example.org",                              # no voice: nothing changes
    { host = "play.example.org", voice = 24454 },        # Simple Voice Chat's default port
    { host = "lan-box", port = 25570, allow_private = true, udp = [24454, 24455] },
]
```

The same works per instance, in `instances/<id>/instance.toml` under `[network]`.

* `voice = N` and `udp = [N, …]` mean the same thing. At most four ports per server,
  sixteen different ones in total, each 1024 or higher. The port is the one the *server's*
  voice chat listens on — ask its operator if it is not 24454; some hosts run voice on the
  game port (`voice = 25565`).
* There is no switch that allows UDP for all servers, and `--join` alone allows none. A
  mistyped key in a server's table is an error.
* Datagrams are relayed to that port **only while a game connection to that server is
  open**, and only **to the address that game connection is using**: voice cannot go
  anywhere the game is not already connected to, and it stops as soon as either the game or
  the server hangs up.
* At most 1400 bytes per datagram, 100 a second from you and 500 a second to you per flow,
  8 flows at once. Nothing is inspected or decoded: the launcher does not know that it is
  voice. The launch log says which ports are open, logs each flow when it starts and ends
  and each kind of refusal once, and gives the totals at the end.
* Not supported: a server that tells clients to use a different host for voice than for
  the game (`voice_host` in its voice-chat settings). Voice is not sent through
  `launcher_proxy` (neither is the game connection): the server sees your address.
* With audio on, the game can record your microphone whether or not you use voice chat;
  with a voice rule, that recording could leave as datagrams to that server, which is what
  voice chat is and cannot be told apart from it (THREAT_MODEL D3).

### Resource packs and shaders

The game can read packs but cannot put one on disk: `resourcepacks/` and `shaderpacks/`
are read-only inside the sandbox, and a pack a server sends is gone after the session.

```sh
bastion-mc pack import main ~/Downloads/Faithful.zip        # checked, hashed, recorded
bastion-mc pack import main ~/Downloads/shader.zip --shader
bastion-mc pack list main
bastion-mc pack adopt main        # record packs you copied into the instance by hand
```

A pack must be a zip archive that every reader takes for the same thing: no archive comment,
no zip64, a directory that holds exactly the declared members, each name once, no links.
Archives that are ambiguous in one of these ways are refused at import and again at launch
— re-create the zip with an ordinary tool. (Why: `java.util.zip.ZipFile`, which the game
reads packs with, lists members that a check based on the declared count never sees.)

## The companion mod

Fabric profiles get a mod of the launcher's own, **bastion-companion**, without you
installing anything: the [in-game phone](#the-in-game-phone) (press R), the
[redone home screen and darker menus](#the-home-screen-and-dark-menus), and three
conveniences — **zoom** (hold C; on by default), a **HUD read-out** of frames per second,
position and facing (off by default), and **fullbright** (off by default; some servers do
not allow it — that is yours to check) — these three on Minecraft 26.3 only. It is compiled
from source together with the launcher, embedded in the launcher binary, and added for one
session at a time as two read-only files outside `mods/`, which stays exactly what you
signed.

It is a convenience, **not a security feature**: it runs inside the game next to every other
mod and enforces nothing. What it reports over its socket is written into the launch log,
marked as coming from the game, and never acted on. What the phone asks for over the same
socket is done only as far as the named app's fixed vocabulary and limits allow — exactly as
for any other code in the game that sends the same words. The launcher adds the mod by itself
to Fabric profiles of the ten Minecraft versions it was run with
([Minecraft versions](#minecraft-versions); `force = true` adds it to the other versions
from 1.20.1 to 26.3), and every launch says whether it was added and why not. Its settings
are the launcher's, under `[companion]` in `config.toml`, and any of them can be set per
instance in `instance.toml`; `enabled = false` keeps it out. No screen in the game changes
them, on purpose. Everything else — keys, settings, how it reaches the game, the channel,
the build, what was verified — is in [`docs/COMPANION.md`](docs/COMPANION.md); the phone and
its services in detail are in [`docs/PHONE.md`](docs/PHONE.md).

### Minecraft versions

The companion mod — and with it the phone, the home screen and the dark menus — runs on
every Fabric release from 1.20.1 to 26.3. Minecraft's own code changed several times in that
span (how the interface is drawn, how pictures become textures, how keys and the mouse
arrive, where the open screen lives), so the mod has one adapter per *family* of versions
that are alike in what it needs, and picks the adapter when the game starts:

| Family | Versions | Run in the real game |
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

The launcher adds the mod by itself to the versions in the last column. For the other
versions of a family, set `force = true` under `[companion]`: the mod then uses the family's
adapter, but that exact version was never started here. Versions outside every family
(older than 1.20.1, newer than 26.3) get no adapter: with `force` the mod loads, says in the
game log that it has none, and the phone, the home screen and the dark menus stay off. Zoom,
the HUD read-out and fullbright are written for 26.3's classes; on every other version they
switch themselves off and say so once in the launch log. The adapters are compiled against
stand-ins of Minecraft's classes — names and signatures only, no code — that were generated
from Mojang's and Fabric's published files, each checked against its publisher's hash
(`java/stubs/tools/regenerate.py --check` does it again); the build reads only those
stand-ins, with no Minecraft jar and no network.

**Vanilla instances** need no mod. Until this round, vanilla 1.20.1 to 1.21.11 did not start
at all: their client jars are signed and keep their classes in the default package, where
the launcher's unsigned launch bootstrap was too, and the JVM refused the game's classes
(`SecurityException: … signer information does not match …`). The bootstrap now lives in a
package of its own (`bastion.boot`). Vanilla 1.21.4 was started after that change; the other
versions of the range were not. Each version needs the Java release it declares — 17 up to
1.20.4, 21 from 1.20.5 to 1.21.11, 25 for 26.x (the packages `jdk17-openjdk`,
`jdk21-openjdk`, `jdk25-openjdk`) — or `match = "newer"` under `[java]`, which is how the
development machine ran every one of the versions above, on Java 27.

## The in-game phone

In a Fabric instance with the companion mod, press **R** — in a world or on the home screen
— and a phone slides up at the right of the screen; press R again, or Escape on its home
screen, to put it away. The game keeps running behind it (it is not a pause menu), and the
mouse is free while it is out. Everything works with the mouse or the keyboard alone: arrow
keys or Tab move the highlight, Enter or Space use it, Escape or Backspace go back. While a
text field has the cursor, keys are letters: R types an "r" there. The status bar shows the
time, how many notices the launcher sent (a clip saved, a join approved …) and whether the
phone is connected to the launcher; a notice that arrives while the phone is away peeks in
at the edge of the screen. The clock shows the time zone of the game's sandbox, which has none
unless `expose_timezone = true` under `[sandbox]`, so it may show UTC.

The phone shows only the apps the launcher set up for this launch:

| App | On the phone when | Needs |
|---|---|---|
| Spotify | the D-Bus proxy to the desktop app started (`[spotify] mpris = true`, the default), or the Web API is signed in | `xdg-dbus-proxy`, the Spotify desktop app; for the Web API your own Spotify client ID |
| Discord | the browser is on and lists a site named `discord` | `[browser]`, Firefox |
| Browser | `[browser] enabled = true` | Firefox, and sites listed |
| Clips | `[clips] enabled = true` | ffmpeg, and the game on an X11 display (the nested compositor or yours) |
| Realms | every launch but a burner session; it can approve a join only with `[realms] approve` set | a signed-in account, the filtered network with the credential broker |
| Settings | every launch but a burner session | — |

Spotify, clips and the browser are started only for a launch whose companion has its phone
on: without a phone there is nothing to use them with. Settings sync works without it — in a
vanilla instance too. Realms is brokered only in a launch with the phone, because a join is
approved there; in any other Java launch Realms joins are refused at once, and the launch log
says why (a Bedrock session has no phone, and is approved outside the game:
[Minecraft: Bedrock Edition](#minecraft-bedrock-edition)). A `--burner` session starts no
browser, records no clips, takes no part in settings sync or Realms, and its Spotify has
neither the Web API nor album art.

```toml
[companion]
phone = true                  # the in-game phone
phone_key = "key.keyboard.r"  # Minecraft's name for the key, as in options.txt; "r" works too
```

R because no Minecraft version from 1.20.1 to 26.3 binds it by default, Simple Voice Chat
does not use it, and it sits in the same place on QWERTY, QWERTZ and AZERTY keyboards.
`phone = false` takes the phone out of the game and nothing more: while the companion is
added to the instance, Spotify, the browser and clips are still prepared (each as its own
section says), and any code in the game can use them over the channel. To switch a service
off, use its own settings (`mpris = false` and no `client_id` under `[spotify]`,
`enabled = false` under `[browser]` or `[clips]`), or keep the companion out
(`enabled = false` under `[companion]`).

**What the game can see and do through the phone.** The phone is code inside the game, next
to every other mod, and carries no authority: any mod can read what it shows, read what you
type into it, draw a fake one, or send whatever the phone could send.

* **Whatever is shown on the phone or typed into it is visible to every mod in the game**:
  what you listen to, the open web page as pictures (a Discord conversation on screen
  included), clip names, Realms world names and server addresses; a search, text typed into
  a web page, a Realms code. Type no password into the phone: sign in to sites with
  `bastion-mc browser login <site>`, outside the game.
* Whatever an app allows, any code in the game can do, at the rate the launcher allows:
  control your Spotify playback, open a listed site and act there as you if you are signed
  in, save, rename or delete clips the launcher saved, type a Realms code, sync settings.
* What it cannot get through the phone: a token, a cookie, the browser's profile, the TOTP
  secret, a path, a web address or a program to run. Every command is parsed strictly,
  routed to an app of this launch by its exact name, refused unless its verb is in that app's
  fixed vocabulary, and limited per app and verb (and in all, per second); no argument is
  ever used as a path, an address, a command line or code. What the launcher sends the phone
  is what the phone shows — state, pictures, short notices — and nothing more.

### Spotify

The phone's Spotify app controls the **Spotify desktop app on this machine** — play, pause,
next, previous, seek, volume, shuffle, repeat, open a track, album, playlist, artist, episode
or show — and shows what is playing, with the cover. This is on by default and needs no
account: the launcher talks to the desktop app over MPRIS (D-Bus), through `xdg-dbus-proxy`
in a sandbox of its own that passes the Spotify app's player interface and nothing else of
your session bus. Nothing leaves the machine except the cover: when the phone asks for it,
the launcher fetches it from Spotify's image server (`i.scdn.co`), decodes it and gives the
phone a picture it encoded itself (`album_art = false` turns that off).

With the **Spotify Web API** the phone can also search, add to the queue, list your Spotify
devices and move playback to one of them, and control playback when the desktop app is not
running (Spotify allows playback control through the Web API only with Premium; the phone
says so when it is missing). It needs your own Spotify application — the launcher ships no
client ID and no secret:

1. Create an app at <https://developer.spotify.com/dashboard> and register the redirect URI
   `http://127.0.0.1/callback` (Spotify then accepts the port the sign-in uses; with
   `redirect_port` set, register `http://127.0.0.1:<port>/callback`).
2. Put its client ID under `[spotify]` in `config.toml`: `client_id = "…"` (32 lower-case
   hexadecimal characters).
3. `bastion-mc spotify login` (`--open` starts your browser) and sign in. The refresh token
   goes to the launcher's token store, under a name of its own.

`bastion-mc spotify status` says what is set up and whether it answers (`--offline`: from the
configuration and the store only); `bastion-mc spotify logout` forgets the stored token (the
grant itself is revoked under "Manage apps" in your Spotify account); `bastion-mc scram`
removes it with everything else. The scopes asked for are
`user-read-playback-state user-modify-playback-state` — nothing about your account, library
or playlists. During a launch with the Web API, a token keeper process holds the refresh
token ([The phone's helpers](#the-phones-helpers)).

```toml
[spotify]
mpris = true          # the desktop app over MPRIS
album_art = true      # covers from i.scdn.co when the phone asks
# client_id = "…"     # your Spotify application: the Web API
# redirect_port = 43117   # a fixed port for the sign-in (1024 or higher); unset: any free port
```

What the game can do through it: control your playback (the desktop app, or any of your
Spotify devices through the Web API), search, queue, move playback to one of your listed
devices, and read what is playing, your device names and search results — within limits per
verb and per kind of Web API call, 60 calls a minute in all. It can open only the six kinds
of `spotify:` address, and name only a device the launcher listed. Whatever program owns the
desktop app's name on your session bus can show the phone made-up titles.

### Discord and the browser

The phone has a web browser: the installed Firefox (`pacman -S firefox`), headless, in a
sandbox of its own ([The phone's helpers](#the-phones-helpers)), able to show only the
sites you list — Discord first:

```toml
[browser]
enabled = true
sites = [
  { preset = "discord" },
  { name = "wiki", title = "Minecraft Wiki", start = "https://minecraft.wiki/",
    hosts = ["minecraft.wiki", "*.minecraft.wiki"] },
]
```

* At most 8 sites, each with the host names its pages may load from (`example.com`, or
  `*.example.com` for every name below it), up to 32. The phone opens a site by its name, at
  its `start` page; a link to a host the site does not list is refused, and the phone says
  so. Only `https://` pages.
* The built-in `discord` preset is `discord.com` and the hosts Discord's web client needs. A
  site named `discord` gets an app of its own on the phone.
* Budgets per site and minute (defaults): `requests_per_minute = 6000` (everything its pages
  load; Discord's client loads about 700 files when it starts), `api_calls_per_minute = 300`
  (fetch, XMLHttpRequest, WebSocket and beacon calls — sending a message is one),
  `connections_per_minute = 120`, `megabytes_per_minute = 60`. Requests, calls and
  connections beyond a budget are refused until it refills (the phone says so); bytes beyond
  it are slowed down.
* `frame_rate` (1 to 15, default 10) is how many pictures a second the phone gets — only
  while it shows the page, and only while the page changes; `quality` (30 to 90, default 60)
  their JPEG quality; `width` and `height` (160 to 1280; 800 × 600) the page size it starts
  with; `memory_max_mb` (default 2048) the browser's own memory ceiling where the game's
  resource limits are in use.

**Sign in outside the game**, never on the phone:

```sh
bastion-mc browser login discord
```

opens a Firefox window on your desktop, in the same sandbox and with the same profile. Sign
in there and close the window: the in-game browser starts with that session. The window,
too, reaches only the site's hosts. Firefox's password manager is off, so the profile keeps
the site's session, not your password; Ctrl-C instead of closing the window ends it without
recording the sign-in. On X11 the window is a client of your X server like any other window.

**What does not work**: voice and video calls, Discord's included (no microphone and no
camera in the sandbox, WebRTC off, no UDP); sites that are not listed and hosts a listed site
does not name — including what a site embeds from elsewhere (on Discord: video players, GIFs
from Tenor, payments); uploading files; keeping downloads; extensions; developer tools;
passkeys and security keys in the sign-in window; more than one browser at a time (one
profile: while one runs, another launch's browser and `browser login` are refused with a
message). Sites can tell that the in-game browser is remote-controlled (`navigator.webdriver`
is true there) and may ask for more captchas; the sign-in window is not remote-controlled.
Typing more than a few words in a row into a page loses text: the phone sends what you type
in up to ten batches a second, and the launcher takes ten batches at once and then one a
second (found by reading the code, not seen in the game; an open item in
[`TODO.md`](TODO.md)).

What the game can do with it: open the listed sites, see what they show, click, scroll and
type in them — in a site where you are signed in, that is acting as you there, within the
site's budgets. It cannot open anything else, cannot read the profile, its cookies or the
site's token (the phone gets pictures, the host name, the page title and short notices), and
cannot reach your files or your desktop through it. A site that can be made to show its own
secrets on screen is beyond what the launcher can prevent. The profile lives in
`~/.local/share/bastion-mc/browser/`; `bastion-mc scram` deletes it, but sessions a site
keeps on its own side end only when you sign out there (Discord: Settings, Devices).

### Clips

With `[clips] enabled = true` (off by default: recording costs a CPU core) the launcher
records the game while it runs and keeps the last `buffer_seconds` in memory. The phone's
Clips app saves them as a clip — an MPEG-TS video (H.264; no sound) with a thumbnail next to
it — into `~/.local/share/bastion-mc/clips` (or `folder`), lists the clips with their
thumbnails, renames and deletes them. When a save goes over `max_clips` or `max_total_mb`,
the oldest clips this feature saved are removed; nothing else in the folder is ever touched.

It needs ffmpeg (`pacman -S ffmpeg`) and the game on an X11 display: the nested compositor
(the default on X11 when gamescope is installed) or your X display itself. It does not work
with a Wayland display: nothing outside the compositor can read the game's window there.

```toml
[clips]
enabled = false
buffer_seconds = 30   # 5 to 120
fps = 30              # 5 to 30
max_width = 1280      # 320 to 1280; narrower windows keep their size
saves_per_minute = 4  # 1 to 6
max_clips = 50        # 1 to 500
max_total_mb = 2048   # 16 to 65536
# folder = "/home/you/Videos/minecraft"   # an absolute path; default ~/.local/share/bastion-mc/clips
```

What the game can do with it: save clips (within those limits), see their names and
thumbnails, rename and delete the clips this feature saved, and fill the folder up to
`max_total_mb`. It never names a path: a clip is a number the launcher made up. The recorder
processes what the game draws; one taken over through ffmpeg could write a clip meant to
attack the video player you open it with — the launcher checks the file's structure, not
the video.

### Realms and Settings on the phone

The Realms app is where you type the code that approves a Realms join (the launcher's code,
or your authenticator's). It shows the Realm, the server address, how long the prompt is
still valid and how many tries are left, and a waiting join peeks in at the edge of the
screen while the phone is away ([Realms](#realms)). The Settings app shows whether your
settings are kept in step across instances and when that last happened, and syncs on request
([Settings sync](#settings-sync)).

## The home screen and dark menus

The title screen is replaced by a dark one of the launcher's own design: the game's name and
version on the left with what the title screen offers — Singleplayer, Multiplayer,
Minecraft Realms, Options, Mods (when a mod list such as Mod Menu is installed) and Quit; in
demo mode Play Demo World instead of the first three — and below them Language,
Accessibility and Classic title screen (the game's own, once). On the right, a sketch of the
phone with the key that pulls it out. The captions are the game's own, in your language, and
every button does exactly what the title screen's button does.

The phone and the home screen share one dark theme — near-black, one cyan accent, amber for
caution, red for danger: the same family of colours as the launcher's window. The game's
own menus (options, world list, pause menu …) get a darker background: flat outside a
world, a dark veil over the world inside one. Only the background changes; the menus are
the game's.

```toml
[companion]
home = true          # the redone title screen
dark_menus = true    # darker backgrounds behind the game's own menus
```

Seen in the real game in demo mode on all ten families; the title screen with an account
(Singleplayer, Multiplayer, Realms) and the Mods button only against the stand-in game of the
tests (no account, no Mod Menu here).

## Realms

Realms works in the filtered network once you decide how a join is approved (this section is
about Java; Bedrock's Realms and event servers are approved outside the game, without a phone:
[Minecraft: Bedrock Edition](#minecraft-bedrock-edition)):

```toml
[realms]
approve = "code"   # or "totp"; "off" (the default) refuses Realms joins, as before
```

* **What the game may do with your account.** The credential broker also answers for the
  Realms hosts. The game sees your Realms list, its notifications, news and invitations, can
  accept or decline an invitation (or redeem an invite code, 26.3), agree to the Realms terms
  and report region pings — with your credential, which it still never holds. Everything an
  owner does to a Realm (configure it, reset it, upload, download or restore a world, invite,
  op or remove players, open, close, delete it) is refused before it leaves your computer:
  manage your Realm from the official client.
* **Every join needs your approval, outside the game.** When the game asks to join a Realm,
  the launcher asks Realms where the server is, but does not hand that over. It shows a prompt
  in its own output — and in bastion-mc-gui, on the Launch page — with the Realm, the server
  address and, with `approve = "code"`, a six-digit code. Type the code into the phone's
  Realms app (with `"totp"`: the code your authenticator app shows). You have 120 seconds and
  3 attempts; one prompt at a time, at most 20 in a session. If you did not ask to join that
  Realm, do nothing: the join is refused.
* **One connection.** An approved join gives the game an address of the launcher's own, good
  for one connection within two minutes, which the launcher connects to exactly the server
  Realms named — public addresses only — through the dialer
  ([The phone's helpers](#the-phones-helpers)).
* **TOTP.** `bastion-mc realms totp-setup` makes a secret, keeps it in your token store and
  shows it once, on the terminal, as an `otpauth` URI and as base32 for your authenticator
  app (SHA-1, 6 digits, 30 seconds); `--replace` makes a new one. A code that was used once
  is not accepted again, in a later launch either; after 10 wrong codes in a session, Realms
  joins are refused until the next launch.
* **What it needs.** A signed-in account (not demo mode), the filtered network with the
  credential broker, and the phone to type the code into: a Fabric instance with the
  companion mod and its phone. A launch without one — a vanilla instance, the companion not
  added, `phone = false` — does not broker Realms: it says why, and its Realms joins are
  refused at once. So does a launch that cannot prepare the approvals (no TOTP secret stored,
  the dialer does not start).
* **What stays possible for code in the game.** It can accept an invitation someone sent you,
  redeem an invite code it knows and agree to the Realms terms in your name, and it sees your
  Realms list. In `totp` mode a code is not bound to the prompt it is typed for: code that
  reads what you type into the phone can cancel your prompt, start a join to another of your
  Realms and spend your code on that (`code` mode binds the code to the prompt it was shown
  with). The Realms app is shown the Realm's name and the server address. An approval is only
  as good as your reading of the prompt, and whoever can read your terminal or the front end
  can read the code.
* **Known gaps.** A Realm with a resource pack probably fails to join: the pack's address is
  not reachable from the sandbox. None of this has run against the real Realms service (it
  needs a signed-in account and a Realm).

## Settings sync

On by default (`[sync] options = true`). The settings that mean the same in every Minecraft
version from 1.20.1 to 26.3 follow you from instance to instance: video, sound volumes, mouse
and controls, key bindings, language, chat and accessibility. Change your field of view or a
key binding in one instance, and the next instance you start has it — also when it runs
another Minecraft version: where a setting changed its form between versions (the graphics
mode, the menu blur, the "now playing" toast), it is translated exactly as Minecraft itself
translates it, or left alone.

Never synced: anything about one world or one server (last server, tutorial and first-run
flags), the resource-pack list (packs are checked per instance), privacy choices
(telemetry, server listing, presence) and this machine's devices (sound device, graphics
backend). A burner session takes no part. The synced copy is
`~/.local/share/bastion-mc/sync/options.json`: before the game starts the launcher merges it
into the instance's `options.txt`, and at exit it writes back each listed setting the game
changed — per setting, the last writer wins. The phone's Settings app can do the same during a
session. The companion mod's own settings (zoom key, phone key …) come from `config.toml`
anyway. `[sync] options = false` turns it off.

What the game can do with it: code in the game of one instance can change, through what it
leaves in its `options.txt`, the listed settings of your other instances — bind a key
elsewhere, change a volume, the field of view, the language. Nothing else crosses: a setting
must be on the list, and its value must have the shape the game writes for that setting (a
boolean, a number, a short identifier, a language code, a key name) — never free text, a
resource pack or a server.

## Minecraft: Bedrock Edition

bastion-mc can also run Minecraft: Bedrock Edition — the Android build of the game, started by
the `mcpelauncher-client` you installed, in the sandbox Java games run in, with a profile of
its own. It exists mainly for the Bedrock-only steps of Mojang's cape promotions (an event
server, a challenge in Bedrock survival, signing in to Bedrock once), but it is the whole
game. The real game has never run under bastion-mc: there was no Google Play purchase, no
game file and no Microsoft account on the development machine, and everything below was
verified against stand-ins ([Status](#status)). The guide, with every detail, is
[`docs/BEDROCK.md`](docs/BEDROCK.md).

**What it is not.** It is not the Windows edition, in any form. Projects such as
BedrockOnLinux run the Windows (GDK) build, and that build runs on Linux only after keys are
extracted from Microsoft's licence data and the game is decrypted outside Microsoft's
protected path. That is circumventing a technical protection measure, so bastion-mc does not
do it, does not wrap a tool that does, and has no switch for it. Nor is it a downloader:
bastion-mc contains no Google Play client and no Minecraft files, and it never downloads or
builds `mcpelauncher-client`. Nothing in bastion-mc decrypts, patches or answers a licence
check.

**The licence substitution, and its switch.** When the game starts, it asks Google Play
whether this copy is licensed. There is no Google Play on Linux, so `mcpelauncher-client`
answers that question itself, with "licensed"; the real check happened earlier, when
`mcpelauncher-ui` downloaded the game only after Google said that your Google account bought
it. bastion-mc cannot check that a version you import was bought: import only versions from
your own purchase. Every Bedrock launch is refused until you accept this:

```toml
[bedrock]
accept_licence_substitution = true   # every Bedrock launch is refused without it
```

### What you need

* An x86_64 computer, and `mcpelauncher-linux` (the client) and `mcpelauncher-ui` (the
  download tool) from the AUR: source builds of about thirty git repositories, so read each
  PKGBUILD before you build it. Installed that way the client is a root-owned file in `/usr`,
  which bastion-mc requires; a Flatpak or AppImage client is refused.
* Minecraft bought on Google Play: a purchase of its own, which owning Java or the Windows
  edition does not replace. **Use a Google account of its own for it**, not the one with your
  mail and photos: `mcpelauncher-ui` keeps a Google sign-in on your disk, and whether Google
  ever acts against accounts used with unofficial clients is not known. Sign out in
  `mcpelauncher-ui` once your versions are downloaded.
* Your Microsoft account, to sign in to in each Bedrock session — the one that owns your Java
  profile, if you want the capes on Java.

### Importing a version and playing

```sh
bastion-mc bedrock client         # the installed client bastion-mc would run, and why (or why not)
bastion-mc bedrock import ~/.local/share/mcpelauncher/versions/<version>
bastion-mc bedrock versions       # what is imported, and what mcpelauncher-ui has that is not
bastion-mc instance new bedrock --bedrock 1.26.62.1
bastion-mc launch bedrock         # or Play in the front end
```

The import copies only what the client reads (x86_64 builds only, no links or special files,
within limits), records the SHA-256 of every file and leaves the copy read-only; every launch
checks the version against that record again, and `bastion-mc bedrock remove <id>` is refused
while a session uses it. A Bedrock instance's `instance.toml` has `kind = "bedrock"`, the
window's `width` and `height`, a `[bedrock]` table with the `version` and the instance's own
`servers`, and `[network] mode`: `filtered` (the default) or `none` for offline single-player;
`open` is refused. Java keys in it are refused with the reason, and the Java-only commands
(`install`, `profile …`, `pack …`) refuse a Bedrock instance.

In a session the game reaches Microsoft's and Mojang's Bedrock services by name, the servers
you listed and the Realms and event servers you approved, and nothing else. It may execute
only the client, its libraries, the dynamic loader and bastion-mc's sign-in stub, under a
system-call allowlist of the client's own; native mods are not supported (the mods directory
is empty and read-only). No Java credential exists in a Bedrock session: the Java sign-in is
skipped, and the token store and the vault are not opened — so `--demo` changes nothing for
Bedrock, and `--burner`, `--join` and `--repair` are refused with what to do instead. The
game's data directory belongs to the session alone and is deleted at its end, with the game's
sign-in in it. Kept between sessions, in the instance's `bedrock-data/`, are only the worlds
(`games/com.mojang/minecraftWorlds`), the game's own settings (`games/com.mojang/minecraftpe`)
and the client's settings file (`mcpelauncher-client-settings.txt`) — and with
`persist_worlds = false` nothing. `bastion-mc doctor` has a Bedrock section.

### Signing in, every session

Each session that needs your Microsoft account (servers, Realms, events, capes) starts with a
sign-in; single-player works without one. Microsoft's sign-in page opens **outside the
game**: the front end's *Microsoft sign-in for Bedrock* panel opens it in your browser, and a
terminal launch prints its address. Only Microsoft's page for Minecraft for Android's own
client (`https://login.live.com/…`) is ever offered, and your password and two-factor codes
never reach the game.

At the end Microsoft sends your browser to an address meant for the game,
`ms-xal-0000000048183522://auth?code=…`. **Where bastion-mc's handler for these links is
registered, the browser hands it back by itself**: the Arch package installs a desktop entry
for that scheme, desktop-file-utils' pacman hook registers it, your browser may ask once
whether to open such links with bastion-mc (allow it), and it then runs
`bastion-mc bedrock hand-back <address>`, which hands the address to the running session. The
handler serves one session at a time and needs `XDG_RUNTIME_DIR`. **Otherwise, paste the
address** into the front end's panel or into the terminal. Your browser may not show it at
all — Firefox shows it only in its developer tools, as the `Location` header of the last
request — and [`docs/BEDROCK.md`](docs/BEDROCK.md) has the steps. Either way bastion-mc
accepts only this sign-in's own address, and hands the game nothing but its code and state.

### Servers, Realms and event servers

```toml
[bedrock]
servers = [{ host = "play.example.org", port = 19132 }]   # every Bedrock instance; an instance adds its own
```

Only listed servers can be reached (RakNet over UDP; the port defaults to 19132). When the
game looks up a name that is not listed, the launcher says once what to add. Servers on a
private or loopback address, LAN games and friends' worlds (NetherNet) are refused in this
version. Realms and Mojang's event servers are reached only through a join you approve,
with `[realms] approve = "code"`; `"totp"` acts as `"code"` in a Bedrock session (the TOTP
secret is in the token store, which a Bedrock session never opens), and `"off"`, the
default, refuses them. The launcher holds the join and shows the Realm or event, the server's
address and a six-digit code. There is no phone in a Bedrock session, so you approve
**outside the game**: in the front end's Realms panel (*Approve join*, *Refuse*), or by
typing the code on the terminal of a terminal launch — never in the game. Approved, the game
gets a one-time pass to that one server.

Microsoft's telemetry hosts are reachable by default, because whether a challenge cape is
credited without them is not known; `[bedrock] telemetry = false` refuses them and tells the
client to keep the game's telemetry from starting.

### Capes from Bedrock to Java

A cape claimed in Bedrock belongs to the Microsoft account, and shows on that account's Java
profile later (Mojang has announced anything from at once to within 30 days). Sign in to
Bedrock with the account that owns your Java profile; when a signed-in session ends, the
launcher says what comes next. On Java, `bastion-mc skin show` lists the capes your profile
owns and `bastion-mc cape set "<name>"` wears one, and the profile canary names a newly owned
cape at the next Java sign-in and pins it without an alarm. Never verified with an account:
it needs yours, and bastion-mc's own Java sign-in has never run live.

### What it protects, and what it cannot

**The game holds your Xbox identity.** Once you sign in, the game holds a refresh token of
your Microsoft account for Minecraft for Android, the key it made to sign its Xbox requests,
and the tokens made with them: the game's own sign-in code needs them, so nothing can take
them away from it — unlike Java, where the launcher keeps the token. A game taken over (by a
flaw that a hostile server, Realm or add-on exploits, or through a compromised client) can
use that sign-in, and copy it out through any connection it is allowed. Then your Realms
(with the owner's rights), your **Minecoins**, your Bedrock character and cape and your Xbox
friends are at stake, and possibly your Java profile (not verified), until you **sign out
everywhere** at Microsoft (`bastion-mc wiki scram` has the page; Microsoft says it takes
effect within 24 hours).

What bastion-mc does about it: the sandbox keeps the game from your files and your desktop,
the network filter narrows where it can send anything, sign-in pages run outside the game, no
Java credential exists in the session, and the game's sign-in lives one session (deleted at
its end; after a crash, at the next Bedrock start or by `scram`). What it cannot do: stop the
game from using or copying the sign-in during the session, or see the Bedrock side of the
account — the canary watches the Java profile only, so look at your Minecoins, Realms and
character yourself. THREAT_MODEL §4.8 has every row, and §5 says it once more.

## Accounts

### Your own OAuth application

There is no shared client ID, by design. Once:

1. Azure portal → *Microsoft Entra ID* → *App registrations* → *New registration*.
   Supported account types: **Personal Microsoft accounts only**. Redirect URI: platform
   **Public client/native (mobile & desktop)**, value `http://localhost`.
2. *Authentication* → enable **Allow public client flows** (needed for `--device`).
3. Copy the **Application (client) ID** into `~/.config/bastion-mc/config.toml`:

   ```toml
   [auth]
   client_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
   ```
4. **Ask Mojang to approve the ID.** New application IDs are refused by the Minecraft API
   (`Invalid app registration`) until approved: <https://aka.ms/AppRegInfo>. This can take
   days. `bastion-mc launch <id> --demo` works in the meantime (Mojang's demo mode, no
   account, no token).

`bastion-mc config example` prints a fully commented configuration file.

### Signing in and playing

```sh
bastion-mc status                                  # who is signed in, which instances exist
bastion-mc login                                   # browser + loopback redirect (or --device)
bastion-mc instance new main --minecraft 26.3
bastion-mc launch main
bastion-mc launch main --join play.example.org     # allowlist one server for this session
```

None of this has run against Microsoft yet ([Status](#status)).

### Several accounts, skins and capes

```sh
bastion-mc login --account alt                     # a second account, under a label of your choice
bastion-mc account list
bastion-mc account default alt                     # used when --account is not given
bastion-mc skin show                               # skins and capes as Mojang reports them
bastion-mc skin set ~/skins/new.png --slim         # decoded and encoded again: only the pixels are uploaded
bastion-mc cape set Migrator                       # or: none
```

Each label has its own stored token (its own keyring entry, or its own vault file — with
its own keys, if you configure `[auth.age.accounts.<label>]`).

### The profile canary

The launcher pins what an account looks like — name, skin, cape — and compares at every
launch. If that changed and not through the commands above, the launch stops and tells you
(`canary show`, `canary accept`): either you changed it on minecraft.net, or someone else
is in the account. A `skin set` or `cape set` pins the part it changed and nothing else.
With `auth.canary = "warn"` the difference is shown by every command that signs in (also
under `--quiet`) and the pin stays as it was. It is a tripwire for visible changes only:
someone who just plays on the account changes nothing it compares.

## Emergency

### If you think the account is compromised

```sh
bastion-mc scram            # stop running games, remove every stored sign-in from this machine
bastion-mc wiki scram       # what to do next, with the exact Microsoft pages
bastion-mc export-proofs proofs.txt     # what the launcher recorded that shows the account is yours
```

`scram` first stops whatever could still store a sign-in (running launches, a sign-in
that is waiting for you, a Spotify token keeper), then removes the credentials, and only
then deletes what the games left behind. It reports nothing as done that was not confirmed:
what it could not do is a line starting `COULD NOT`, and its exit status is then 1. It needs
no network and revokes nothing at Microsoft — it says so — which is why the runbook is the
second step. The front end has both on its **Security** page, which shows the launcher's
verdict and never a better one.

The credentials of the phone's services go too: the Spotify refresh token and the secret of
the Realms TOTP approval are removed from the kernel keyring (in the age vault they are
files of the vault, which is emptied), and the phone browser's profile — its site sessions,
such as a signed-in Discord — is deleted with the record of sign-ins. While a
`bastion-mc browser login` window still uses that profile, scram leaves it and says
`COULD NOT`: close the window and run scram again. Nothing is revoked at those services
either: sessions a site keeps on its own side end only when you sign out there (Discord:
Settings, Devices), and the Spotify grant ends when you remove the app under "Manage apps"
in your Spotify account. To use them again afterwards: `bastion-mc spotify login`,
`bastion-mc realms totp-setup` (a new secret for your authenticator app) and
`bastion-mc browser login <site>`.

Bedrock sessions are stopped like Java launches; what a killed or crashed Bedrock session left
on disk — its data directory, with the game's Xbox sign-in — is deleted with the credentials
and the deletion confirmed. A Bedrock session that cannot be stopped, or that runs without a
marker, is a `COULD NOT` line that says to end it and to sign out everywhere: a running
Bedrock game holds your sign-in. Bedrock worlds and imported versions are kept, and the output
says so.

The runbook is [`docs/SCRAM_RUNBOOK.md`](docs/SCRAM_RUNBOOK.md), compiled into the launcher
(`bastion-mc wiki scram --links` lists its addresses). If this computer itself may be
compromised, follow its account steps from another device: a compromised machine can show
you a different text.

## Configuration

[`examples/config.toml`](examples/config.toml) lists every key with its default and what it
does; `bastion-mc config example` prints the same file and `bastion-mc config path` says
where yours goes (`~/.config/bastion-mc/config.toml`; it must be your own file, not a
link, and not writable by group or others, e.g. mode 0600). Unknown keys are errors, also
in a server's table. An instance's `instance.toml` can set its own Minecraft version
(vanilla), `memory_mb`, `width`, `height`, `[network]` `mode` and additional `servers`, and
any key of `[companion]` (the phone, its key, the home screen and the dark menus included).
A Bedrock instance's `instance.toml` has keys of its own
([Minecraft: Bedrock Edition](#minecraft-bedrock-edition)), and `[bedrock]` in `config.toml`
(`accept_licence_substitution`, `client`, `telemetry`, `persist_worlds`, `seccomp`, `servers`,
`extra_syscalls`) applies to every Bedrock instance.

The sections of the phone's services are in `config.toml` only, the same for every instance:
`[spotify]` ([Spotify](#spotify)), `[clips]` ([Clips](#clips)), `[browser]`
([Discord and the browser](#discord-and-the-browser)), `[realms]` ([Realms](#realms)) and
`[sync]` ([Settings sync](#settings-sync)). Their defaults: Spotify over MPRIS, album art
and settings sync on; the browser, the Spotify Web API (no client ID), clips and Realms
approval off.

### Layout (XDG)

```
~/.config/bastion-mc/        config.toml, trusted-keys.d/
~/.local/share/bastion-mc/   instances/<id>/, libraries/, versions/, assets/, vault/,
                             browser/ (the phone browser's profile and record of sign-ins),
                             clips/ (the default clips folder), sync/ (the synced settings),
                             bedrock/versions/ (imported Bedrock versions; a Bedrock
                             instance keeps its worlds in instances/<id>/bedrock-data/)
~/.cache/bastion-mc/         scratch/ (the per-launch game root, deleted on exit), running/,
                             realms-totp-last-step (no secret: the last TOTP step used),
                             bedrock-sessions/ (each Bedrock session's data directory, with
                             the game's sign-in; deleted at the session's end)
~/.local/state/bastion-mc/   logs/<instance>/launch-*.log (the newest 20), profile-serials/,
                             accounts/, canary/, activity/, default-account
$XDG_RUNTIME_DIR/bastion-mc/ short-lived: the game's Wayland socket (wl-*), the companion
                             mod's channel (companion-*), the D-Bus proxy's socket
                             directory (mpris-*), the clip buffer in memory (clips-*), the
                             Bedrock sign-in bridge (signin-*) and the socket of the
                             handler that hands a sign-in back (bedrock-signin.sock)
```

`options.txt` and `servers.dat` are carried into each session and back by the launcher,
parsed and re-encoded both ways, never copied (`sandbox.persist_files`). Between sessions
they rest in the instance's `persist/` directory — the one part of the instance the
launcher can still write while the game runs. (An instance from before this layout has them
moved there at its next launch; to change a setting by hand, edit the file in `persist/`.)

## Tests

```sh
cargo test                         # the launcher: needs bubblewrap, python3, a JDK, user namespaces
cargo test --workspace             # plus the event protocol and the front end (no display needed)
BASTION_NET_TESTS=1 cargo test     # also the tests that contact Mojang, Modrinth and public DNS
BASTION_X11_TESTS=1 cargo test     # also X11 clients in the sandbox (needs an X session, glxinfo)
BASTION_FUZZ_ITERATIONS=300000 cargo test --release --test robustness   # a longer generated-input run
```

The phone's suites in the default run use more tools where they are installed, and say
`skipped` and pass where they are not: `tests/spotify.rs` needs `dbus-daemon`,
`xdg-dbus-proxy` and python3 with PyGObject (its stand-in player is written with GDBus),
`tests/clips.rs` ffmpeg and ffprobe. `tests/phone_ui.rs` drives the phone, the home screen
and the dark menus in a stand-in game with the JDK, and `tests/realms.rs` runs a stand-in
Realms service, the real dialer and the real relay.

The fifth round's suites run in the default run as well, all against stand-ins:
`tests/bedrock_launch.rs` (three Bedrock sessions of a stand-in client that it compiles with
`rustc`, in the real sandbox, with the relay, the sign-in bridge and the real
`bastion-mc bedrock hand-back`; its last session confines the test process itself),
`tests/bedrock_sandbox.rs` (the client's system-call filter in the kernel, with a Python
payload; its log mode makes the kernel log a few `type=1326` lines), `tests/bedrock_net.rs`
(the relay and the broker with a sandboxed Python and curl client, against a stand-in RakNet
server and stand-in Realms and gatherings services), `tests/signin_stub.rs` (the stub in
bubblewrap), and `tests/bedrock_store.rs` and `tests/bedrock_cli.rs` (the real binary's
`bedrock` and `instance` commands). They need what `tests/sandbox.rs` needs — bubblewrap,
python3, unprivileged user namespaces, Landlock — and `tests/bedrock_launch.rs` also `rustc`
and the system's libcurl, `tests/bedrock_net.rs` curl. `BASTION_TEST_RUNTIME` gives
`tests/bedrock_launch.rs` a directory with a short path for its sockets (default:
`XDG_RUNTIME_DIR`).

The suite assumes a short temporary directory on a file system without fs-verity, as `/tmp`
is on Arch (tmpfs). With `TMPDIR` pointed at a directory inside the checkout on btrfs (a path
of 76 bytes), 50 tests failed on the development machine, 25 of them Bedrock's: most bind
UNIX sockets under it (the companion's channel, the sound-server and Wayland checks, the
Bedrock sign-in bridge and its stub), whose paths then exceed the 108 bytes a socket path
may have; two tests of mods and packs rewrite a file that the launcher sealed with
fs-verity, which btrfs supports; two expect the short paths of `/tmp` in what they check.

The counts of the last full run are in [Status](#status). The opt-in suites return at once
— and count as passed — unless their variables are set:

| Variables | What then runs |
|---|---|
| `BASTION_NET_TESTS=1` | the relay and the texture gateway against Mojang, an SRV lookup over public DNS, real installs of legacy versions, the mod catalog against Modrinth |
| `BASTION_X11_TESTS=1` | X11 clients in the sandbox, in each network mode |
| `BASTION_WAYLAND_TESTS=1` with `WAYLAND_DISPLAY` and `BASTION_WAYLAND_COMPOSITOR` | the security context against a real compositor (`tests/wayland.rs`) |
| `BASTION_NESTED_TESTS=1` (`BASTION_NESTED_CLIPBOARD_TEST=1` for the clipboard test) | the game's display inside gamescope (`tests/nested.rs`) |
| `BASTION_E2E_ROOT` and `BASTION_E2E_INSTANCE` | the real Minecraft client through the whole launch path (`tests/nested_game.rs`), the companion mod in the real game with the demo service behind its phone (`tests/companion_game.rs`, a signed Fabric profile of one of the ten versions), and (`BASTION_E2E_ROOT` alone) the companion's compile-only stand-ins against the real Fabric Loader and Mixin jars |
| `BASTION_BROWSER_TESTS=1` (with `BASTION_NET_TESTS=1` also example.com and Discord's sign-in page; with `BASTION_BROWSER_LOGIN_DISPLAY=:N`, a private display, the sign-in window) | the phone's browser — the installed Firefox in its sandbox, the relay, the remote control — against stand-in HTTPS sites (`tests/browser.rs`) |
| `BASTION_E2E_ROOT`, `BASTION_BROWSER_TESTS=1` and a private `DISPLAY` | the browser in a real demo launch, and a search of the game's sandbox for its profile (`tests/browser_game.rs`) |
| `BASTION_CLIPS_DISPLAY=:N` (a private display) | the clip recorder in its sandbox, recording a window there (`tests/clips.rs`) |
| `BASTION_E2E_ROOT`, `BASTION_E2E_INSTANCE` and a private `DISPLAY` (`BASTION_E2E_CONFIG` for another configuration root) | clips and Spotify in a real demo launch, the phone's apps driven by a script of commands, a private D-Bus daemon with the stand-in player (`tests/clips_game.rs`) |
| `BASTION_PULSE_SOCKET` and `BASTION_PULSE_EXPECT` | the module-loading probe against your running sound server |
| `BASTION_GUI_LIVE_LAUNCHER` and `BASTION_GUI_LIVE_INSTANCE` | the front end against the real launcher and a real demo game (`gui/tests/live.rs`) |

The display tests need a Wayland compositor and gamescope, which need not be installed:
`tests/support/fetch.sh` downloads the Arch packages, checks each against the pacman sync
database *and* its signature, and unpacks them under `target/third-party/`; they only ever
run inside bubblewrap. Socket paths are limited to 108 bytes, so keep the runtime directory
short:

```sh
tests/support/fetch.sh
RT=/run/user/$(id -u)/bmc-test; mkdir -p "$RT"
tests/support/sway-headless.sh "$RT/wl" gles2 &      # a headless Wayland compositor
BASTION_WAYLAND_TESTS=1 WAYLAND_DISPLAY=$RT/wl/wayland-1 BASTION_WAYLAND_COMPOSITOR=$RT/wl/wayland-1 \
    cargo test --test wayland
tests/support/private-x11.sh "$RT/x" & sleep 2; ln -s "$RT/x/x11/X0" /tmp/.X11-unix/X77   # an invisible X display
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_NESTED_TESTS=1 BASTION_NESTED_CLIPBOARD_TEST=1 \
    BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt) cargo test --test nested -- --test-threads=1
```

The real game, on that invisible display, from a data root that holds the four XDG
directories with the instance installed (a vanilla instance for the first test, a signed
Fabric profile of a tested version for the second):

```sh
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=vanilla-latest \
    BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt) cargo test --test nested_game -- --nocapture
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=fab \
    BASTION_COMPANION_WORLD=1 cargo test --test companion_game -- --nocapture
```

`BASTION_COMPANION_WORLD=1` lets the companion's self-test enter the demo world and measure
there; `BASTION_COMPANION_KEYPRESS=1` also presses the phone key (and on 26.3 the zoom key)
through XTEST — only on a private display that nothing else uses, because the key goes to
whatever has the focus. The phone in that test is served by the demo service: fake Spotify,
browser, clips, Realms and settings that write every command they receive into the launch
log. `BASTION_PHONE_DEMO=1` in the launcher's environment switches it on for any launch; it
is a test switch, not a configuration key, and the launch log says `DEMO` for every app it
serves.

`tests/support/phone-shots.sh <data-root> <instance> <display> <out-dir> <width> <height>
[world]` is how the phone, the home screen and the dark menus were looked at in the real
game: it launches the instance in demo mode on a private display with the demo service,
presses keys and clicks there through XTEST, and saves a screenshot of the game's window for
each step. It changes the instance: the window size, `memory_mb = 2048` and, where there
is no `[companion]` table, `force = true` go into its `instance.toml`, and quiet settings
(no sound, no onboarding screen) into its `persist/options.txt`.

The front end's widget tests render in software (`.cargo/config.toml` selects the
renderer) and leave screenshots of every state in `target/gui-snapshots/`
(`BASTION_GUI_SNAPSHOT_SCALE=2` for twice the resolution). Its emulator tests create a
headless GPU context where there is a GPU, and the driver then writes a shader cache into
`XDG_CACHE_HOME`; point the four XDG variables somewhere else to keep it out of yours.

Coverage-guided fuzzing of the same parsers: [`fuzz/README.md`](fuzz/README.md).
`cargo deny check` (configuration in `deny.toml`) audits the dependency tree.

On GitHub, [`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on every push to
`main` and every pull request: `cargo fmt` and `cargo clippy` with warnings as errors (on a
pinned toolchain); the launcher's unit tests, the event protocol, the front end and the
website's tests, in an Arch Linux container as an ordinary user, with the fonts the front
end's layout tests measure; a check that every fuzz harness still compiles; `cargo deny`
(also once a week, for new advisories); and both packages built from the commit with
`makepkg`, `check()` and the hardening check included. The suites that start the real
sandbox run on a development machine; on a GitHub runner they can be started by hand
([`integration.yml`](.github/workflows/integration.yml)), which has not been tried yet. The
workflow's second run on GitHub (2026-11-14, on the tree the fifth round started from) passed
all five jobs; the fifth round's tree has been through a local simulation of it only.
[`pages.yml`](.github/workflows/pages.yml) builds the website and publishes it.

## Documentation

- [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) — what is defended, how each claim is
  tested, what is not defended (§5), and the experiments behind the claims (§7). Read it
  first.
- [`docs/TOKEN_INJECTION.md`](docs/TOKEN_INJECTION.md) covers the token path in detail.
- [`docs/MC_VAULT_ACCOUNT.md`](docs/MC_VAULT_ACCOUNT.md) — what to do about other software
  running as you: `ptrace_scope=3`, a dedicated UNIX account, the root-owned recipients
  list.
- [`docs/SCRAM_RUNBOOK.md`](docs/SCRAM_RUNBOOK.md) is the emergency text the launcher
  carries (`bastion-mc wiki scram`).
- [`docs/MODS.md`](docs/MODS.md) — mods in detail: signed profiles, `profile add` and
  `profile check`, the aliases, ViaFabric, Simple Voice Chat and voice chat on a server.
- [`docs/COMPANION.md`](docs/COMPANION.md) — the companion mod.
- [`docs/PHONE.md`](docs/PHONE.md) — the in-game phone and its services: what each app does,
  its settings and limits, and what the game can see.
- [`docs/BEDROCK.md`](docs/BEDROCK.md) — Minecraft: Bedrock Edition: what it is and is not,
  what you need, the licence substitution, importing, signing in every session, servers,
  Realms and event servers, capes, what bastion-mc cannot protect, troubleshooting, and what
  was verified.
- [`docs/README.md`](docs/README.md) — an index of everything under `docs/`, including the
  engineering notes in `docs/notes/` (how each part was built and verified) and the
  screenshots.
- [`TODO.md`](TODO.md) — what is open and why, what is blocked on something only you have,
  what was done, and what was decided against.
- [`examples/config.toml`](examples/config.toml) — every configuration key.
- [`fuzz/README.md`](fuzz/README.md) — coverage-guided fuzzing.

## Source map

| Path | What |
|---|---|
| `src/sandbox/` | bubblewrap orchestration, in-sandbox init, Landlock, seccomp, netns helper, display/audio/GPU exposure; `wayland.rs` (security context), `nested.rs` (gamescope), `cgroup.rs` (limits) |
| `src/egress/` | DNS responder, SNI parser, allowlisting relay, texture gateway, `broker.rs` (credential broker), `udp.rs` (datagram relay for voice chat and for Bedrock's servers); for Bedrock `bedrock.rs` (its rules), `raknet.rs` (RakNet's opening messages) and `services.rs` (the broker for Realms and event-server joins) |
| `src/auth/` | OAuth (PKCE, device code), Xbox, Minecraft services (profile, skins, capes), token stores, account registry, canary, activity log |
| `src/confine.rs`, `src/confine/filter.rs`, `src/persist.rs` | the launcher's self-confinement (Landlock, and its system-call filter); strict re-encoding of settings files |
| `src/scram.rs`, `src/running.rs`, `src/proofs.rs` | emergency removal of credentials, markers of running commands and locks on their directories, the ownership export |
| `src/tree.rs` | deleting and copying directory trees that the game has written (bounded, link-safe) |
| `src/fuzzing.rs` | one harness per parser of untrusted bytes (used by `tests/robustness.rs` and `fuzz/`) |
| `src/companion/` | the companion mod's launcher side: configuration, the injection decision, `jar.rs` (the build recipe in passes, one per family of versions, also used by `build.rs`), `channel.rs` (the socket: the mod's reports, and protocol 2 for the phone) |
| `src/phone/` | the phone's contract (`mod.rs`: commands, services, apps), `dispatch.rs` (routing, vocabularies, limits), `demo.rs` (the demo service, a test switch) |
| `src/browser/` | the phone's browser: configuration and the `discord` preset, Firefox's sandbox and the stage inside it, the relay and its budgets, WebSocket and WebDriver BiDi, the phone app, `login.rs` (`browser login`) |
| `src/spotify/` | Spotify: the D-Bus client and MPRIS through `xdg-dbus-proxy`, the Web API, the sign-in, the token keeper, album art, `spotify:` addresses |
| `src/clips/` | clips: the recorder's sandbox and its X11 helper, the buffer, the library of saved clips |
| `src/realms/`, `src/sync/` | Realms: the endpoint table, approvals, TOTP, the dialer, `answers.rs` (approvals typed at the trusted input); settings sync: the list of synced settings, translations between versions, the synced copy |
| `src/bedrock/` | Minecraft: Bedrock Edition: `runtime.rs` (finding the installed `mcpelauncher-client`), `store.rs` and `store/` (importing versions, the manifest, checking them at each launch), `launch.rs` and `launch/` (the instance, the client's sandbox profile, the session), `net.rs` (what a session may reach; the relay's Bedrock parts are `src/egress/bedrock.rs`, `raknet.rs` and `services.rs`), `signin/` (the bridge, its checks, the stub, the hand-back), `policy.rs` (the gate, `doctor`'s section, the sessions' data directories) |
| `native/noclip.c` | the library preloaded into gamescope that cuts its clipboard bridge |
| `docs/` | threat model, token path, the scram runbook, the dedicated-account guide, mods, the companion mod, the phone, Bedrock; `notes/` (engineering notes), `images/` (screenshots) |
| `tests/support/` | scripts that fetch, verify and run the compositors the display tests need; the stand-in Spotify player, stand-in HTTPS sites, XTEST keys, clicks and screenshots on a private display, `phone-shots.sh`; `bedrock_standin.rs`, the stand-in for `mcpelauncher-client` |
| `tests/java/` | Java programs the tests start: the broker and voice probes, a stand-in "game" for the companion mod (`fakegame*`), a stand-in platform and the phone's check (`FakePlatform.java`, `PhoneUiCheck.java`) |
| `src/mojang/` | version metadata, argument rendering, launch frame, verified download store |
| `src/profile/` | signed manifest, Ed25519 trust store, strict `mods/` verification, authoring, `capabilities.rs` (the scan of what jars refer to), `modrinth.rs` (registry client), `catalog.rs` (`profile add`, `profile check`), `packs.rs` (resource and shader packs), `zipdir.rs` (the strict zip directory reader they are checked with) |
| `src/secret.rs`, `src/hardening.rs` | locked self-wiping memory, zero-on-free allocator, process hardening |
| `src/events.rs`, `src/events/input.rs` | how the launcher reports progress: text for a terminal, JSON events for a front end; the trusted input that takes answers to prompts from the terminal and the front end |
| `crates/proto/` | the event protocol shared by launcher and front ends (types only), and the runbook's text with the only links a front end may open |
| `gui/` | `bastion-mc-gui`: `backend.rs` (worker), `app.rs` (state and logic), `view/` (the pages and the parts they are built from, the Realms prompt among them), `hud.rs` (drawn elements), `theme.rs`, `runbook.rs`, `console.rs` |
| `java/BastionBootstrap.java` | the 122-line class (package `bastion.boot`) that receives the launch frame |
| `java/companion/` | the companion mod: `src/` (version-independent: the platform API, the phone, the home screen, the theme, zoom and the other features), `platform/` (the adapters of each family and group of families) |
| `java/stubs/` | compile-only stand-ins, never shipped: `common/` (Fabric Loader, Mixin, ASM), `minecraft/<family>/` (Minecraft's classes per family: names and signatures only), `tools/` (how they are made again from the published files, and the table of families) |
| `packaging/arch/PKGBUILD` | hardened Arch packages (`bastion-mc`, `bastion-mc-gui`); `bastion-mc-bedrock-signin.desktop`, the handler that hands a Bedrock sign-in back to the running session |

## Trademarks and licence

bastion-mc is not an official Minecraft product. It is not approved by or associated with
Mojang or Microsoft. "Minecraft" is a trademark of Mojang and Microsoft; other names belong
to their owners. The launcher downloads Mojang's game files and third-party mods at your
request from where their publishers offer them, and redistributes none of them.

MIT, see [`LICENSE`](LICENSE).
