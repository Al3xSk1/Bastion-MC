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

## Status

**Pre-release.** Version 0.1.0. There is no release, no signed tag and no binary download:
it is built from source. Everything below was built and checked on one development
machine. The Microsoft → Xbox → Minecraft sign-in has never run against Microsoft, so
nothing that needs a signed-in account has run against the live services yet.

Read [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) before relying on any of it. It lists
what is defended, how each claim is tested, and — in §5 — what is **not** defended.

Built and tested on Arch Linux (kernel 7.2, Landlock ABI 10, bubblewrap 0.13.1, OpenJDK 27,
AMD GPU, X11 session).

| | |
|---|---|
| Sandbox, seccomp, Landlock, egress relay, credential broker, the launcher's own confinement, signing, store, TLS policy, event interface, sound-server check, settings sanitisers, account registry, canary, scram, packs, mod catalog, voice-chat relay, companion mod | 576 tests pass in the workspace (`cargo test --workspace`; what that number counts is below this table). Tests in the sandbox, voice, companion, egress, broker and JVM suites start the real sandbox; 4 confine a real multi-threaded process the way the launcher confines itself, and one more opens, connects and uses a UDP socket in a process confined that way, as the voice relay does for each new flow (the relay's own code ran after confinement only inside a real launch, THREAT_MODEL §7 E22); 12 harnesses, one per parser of bytes the launcher does not control, feed generated and mutated input to those parsers |
| Display isolation | Wayland security context: against a real headless sway (the game's socket is listed: 37 interfaces, none of the 16 known privileged ones). Nested compositor: against real gamescope 3.17 on a private X display, including the clipboard cut-off in all four directions and the control run without it. Both with real Minecraft 26.3 started through them. **Not** on a real Wayland desktop session, and playability inside gamescope is not assessed. Both suites were run again on the merged tree of the third round |
| Graphical front end | 118 unit tests (32 of them lay out, click and render the widgets in software) and 4 tests of the whole application against a stand-in launcher, run by iced's emulator without a display; a real demo launch driven through its backend worker and through the application's own controls (an opt-in test, run on the front end's branch); the real window started and closed on a private X11 display, with the GPU and with the software renderer. **No** click in a real window; **not** run on Wayland |
| Arch packages (`makepkg`) | both build, their `check()` passes and both binaries pass the hardening check — in the second pass; **not run again** since the review fixes and the third round (the PKGBUILD now needs `java-environment>=17`) |
| Real Minecraft 26.3 client in the sandbox (demo mode, no account), vanilla and as a signed Fabric profile | runs: hardware OpenGL, window, audio, HTTPS through the relay and the broker, mods loaded from the sealed mount; also as a `--burner` session. Run again after the review fixes (launcher confined before it reads anything from the game; THREAT_MODEL §7 E16), and on the merged tree of the third round, without sound: vanilla and Fabric through the nested compositor, with the companion mod, and with ViaFabric and Simple Voice Chat (whose native libraries load inside the sandbox) |
| Companion mod (zoom, HUD read-out, fullbright) | in the real game (Fabric Loader 0.20.1): it loads, reports to the launcher while the launcher is confined, and the effect of each feature was measured in a running demo world, on the nested-compositor, direct-X11 and Wayland display paths; on the X11 paths also with the zoom key pressed through XTEST on a private display. **Not**: a person at the keyboard, another Minecraft version, a multiplayer server |
| Mod catalog (`profile add`, `profile check`) | against scripted registry answers (28 kinds of refusal, each of which leaves `mods/`, the manifest and the signature as they were) and against a stand-in registry over TLS; the six aliases looked up live at Modrinth; real demo launches with mods it fetched: they load, and their update checks (and Sodium's `lspci`) are refused by the sandbox. **Not**: ViaFabric translating a connection, a Quilt profile, Modrinth through Tor |
| Voice-chat relay (UDP) | 14 tests — 13 in the real sandbox (one of them with a real JVM) and one in a process confined the way the launcher confines itself —, unit tests of its decision table, one generated-input harness; inside a real launch, with the launcher confined, it relayed datagrams for a stand-in client placed in the game's network namespace. **Not**: a real voice session (needs a server and a signed-in account), capture (sound is off on the development machine) |
| `network.launcher_proxy` for the game's connections to Mojang's services (review finding B-M3, fixed in the third round) | against a stand-in SOCKS5 proxy, in the real sandbox; **not** against Tor |
| Microsoft → Xbox → Minecraft sign-in, and everything that needs a signed-in account (the broker against the real Mojang API, `skin`/`cape`, the canary on a real profile, joining a real multiplayer server) | implemented and tested against scripted replies and a stand-in Mojang; **not yet run against the live services** (needs your client ID) |
| NVIDIA, Java 8-era versions end to end, real `age` binary and hardware plugins, coverage-guided fuzzing (needs a nightly toolchain) | implemented, **not exercised** on the development machine |

**What "576 tests" counts.** The integrator's run of `cargo test --workspace --no-fail-fast`
on the merged tree of the third round (no display, separate XDG directories) printed 30
`test result` lines — 27 test binaries and 3 doc-test runs, 5 of them without tests — that
add up to 576 passed, 0 failed, 0 ignored: the launcher 446 (319 unit, 127 integration), the
front end 124 (118 unit, 4 emulator, 2 live) and the event protocol 6. Not every one of them
tests something in such a run: 18 are opt-in and return at once without their environment
variables (see [Tests](#tests)), 5 are halves that other tests start again as child
processes, and a few return early, with a message, on a machine that lacks what they need.
With `BASTION_NET_TESTS=1 BASTION_X11_TESTS=1` the same 576 passed; the display suites, the
real-game tests and two demo launches were run separately on the same tree. The complete
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
  - [Accounts and the token store](#accounts-and-the-token-store)
  - [Emergency: canary and scram](#emergency-canary-and-scram)
- [The graphical front end](#the-graphical-front-end)
- [Mods](#mods)
- [The companion mod](#the-companion-mod)
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
[launcher] launcher: confined itself, on all threads — Landlock: no program execution, files only under the instance's persist/ and the cache, TCP only to port(s) 53, 443; system-call filter: no kernel keyring, no new namespaces
```

Ctrl-C in the terminal stops the game ("game stopped on request", exit status 130). Two
checks can stop a first launch on purpose, and their messages say what to do: on an X11
session without gamescope the launch is refused (see [Display isolation](#display-isolation)),
and with sound on (`audio = "pulse"`, the default) it is refused if your sound server loads
modules for its clients, as PipeWire does as shipped (see [Sound](#sound); `audio = "none"`
under `[sandbox]` tries it silently). `bastion-mc status` lists accounts and instances;
`bastion-mc-gui` (built by `cargo build --release --workspace`) is the
[window](#the-graphical-front-end) for the same things.

(This sequence was run on the merged tree on the development machine, with a debug build —
through gamescope on a private X display, with `audio = "none"`, and with `match = "newer"`
because the machine has no Java 25: it has Java 21 from a jre package, which the launcher
does not find, and Java 27 from `jdk-openjdk` — together with `doctor`, `status`,
`versions` and `config example`.)

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
namespace except through the launcher's allowlisting relay. The holes are one audio socket,
the GPU, and a display of the game's own: on Wayland a socket the compositor knows to be
sandboxed, on X11 a nested compositor. When the launcher adds its companion mod to a Fabric
profile, the sandbox also gets one socket to the launcher, on which the mod reports; the
launcher only logs what arrives there ([The companion mod](#the-companion-mod)).

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
check at its last sign-in here; otherwise demo mode.)

### Network filtering and the credential broker

In the default `filtered` mode the game's network namespace holds nothing but sockets of
the launcher. Names resolve only when they are allowlisted, HTTPS reaches only Mojang's
service domains (the relay reads the server name the TLS handshake asks for), skin
textures come through a gateway, game servers are reachable only when you allowlist them,
and nothing else leaves.

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

**Voice chat** is the only UDP the launcher relays out of the sandbox, and only to a port
that a server's rule names: [Voice chat on a server](#voice-chat-on-a-server).

**Proxy.** `launcher_proxy = "socks5h://127.0.0.1:9050"` under `[network]` sends everything
that goes to Microsoft and Mojang through a SOCKS5 proxy such as Tor, which also resolves
the names: sign-in, downloads, and — while the game runs in the default filtered mode —
what the credential broker forwards, skin textures, and the game's own HTTPS connections to
Mojang's services. **Not** proxied: connections to multiplayer servers, the lookup of their
names (your resolver sees those), and voice. With `mode = "open"` nothing the game sends is
proxied. The launch log states which of these applies. (`profile add` and `profile check`
reach Modrinth through the same proxy.)

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
the game can record your microphone (THREAT_MODEL D3).

### The launcher's own confinement

Before it reads anything the game sends, the launcher gives up — on all its threads —
writing anywhere but one settings directory of the instance (`persist/`) and its cache,
reading anything else but `/etc`, `/usr`, `/proc` and the name resolver's configuration,
program execution, most of the network, and the kernel keyring where the sign-in token
rests (Landlock, and a system-call filter for what Landlock does not cover). The whole of
it needs Landlock ABI 9. On an older kernel `launcher.confine = "auto"` applies what it can
and says what is missing — in the launch log, in `doctor` and in the front end —, and
`"require"` refuses to launch. Not restricted even then: UDP (the DNS responder and the
voice relay answer to the game's ephemeral ports), TCP by destination (the rules are per
port), signals to your other processes (stopping the game needs them), and the access
token the broker holds for the session — a compromised launcher has it.

**What is shown as protected was looked at, not assumed**: the resource limits are read
back from the kernel, the game's Wayland socket is listed, the clipboard cut-off reports
from inside the compositor — and where a check fails the launcher says so, or refuses.

### Accounts and the token store

Each account is a label of your choice with its own stored refresh token
([Accounts](#accounts)). Where that token rests is a choice with consequences.

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
to remove every credential from the machine in one step (and to say what it could not
remove), and a built-in recovery runbook with verified links ([Emergency](#emergency)).

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
prompt or a scram is counting.

Rendering uses the GPU when one is usable and falls back to software otherwise
(`ICED_BACKEND=tiny-skia` forces software). No font or image is bundled: text uses Adwaita
Sans and the system's monospace face where they are installed and whatever sans-serif and
monospace faces there are otherwise; every symbol is drawn as geometry. It knows nothing of
voice chat or of the companion mod beyond the log lines in its console.

It is a separate program on purpose, not a mode of the launcher:

```text
 bastion-mc-gui  (no secrets: window, fonts, GPU)        bastion-mc --events <command>
 ┌───────────────────────────────────────────┐           (hardened, holds the tokens)
 │ view   state ─▶ widgets, pure             │
 │ app    update(): the only place state     │  spawn    one process per command:
 │        changes; never blocks              │ ────────▶ status · account default · login
 │ backend  tokio task per command:          │           logout · versions · instance new
 │        child stdout ─▶ typed Update ─▶    │           launch · scram --yes
 │        tokio mpsc ─▶ iced Task ─▶ update  │ ◀──────── stdout: one JSON event per line
 │                                           │ ────────▶ stdin:  "stop" (or just closing it)
 └───────────────────────────────────────────┘
```

* The UI thread never waits on the network, the disk or the game. Every launcher command
  runs in a child process supervised by a tokio task; progress arrives as messages.
* Tokens stay in the launcher process. The event protocol
  ([`crates/proto`](crates/proto/src/lib.rs)) has no field that could carry one, and the
  launcher binary links none of the GUI's dependencies.
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

Fabric profiles get a small mod of the launcher's own, **bastion-companion**, without you
installing anything: **zoom** (hold C; on by default), a **HUD read-out** of frames per
second, position and facing (off by default), and **fullbright** (off by default; some
servers do not allow it — that is yours to check). It is compiled from source together with
the launcher, embedded in the launcher binary, and added for one session at a time as two
read-only files outside `mods/`, which stays exactly what you signed.

It is a convenience, **not a security feature**: it runs inside the game next to every other
mod and enforces nothing. What it tells the launcher over its socket is written into the
launch log, marked as coming from the game, and never acted on. It is added only to Fabric
profiles of Minecraft versions it was tested with — 26.3 at present (`force = true` adds it
to others, untried in a real game) — and every launch says whether it was added and why
not. Its settings are the launcher's, under `[companion]` in `config.toml`, and any of them
can be set per instance in `instance.toml`; `enabled = false` keeps it out. There is no
settings screen in the game, on purpose. Everything else — keys, settings, how it reaches
the game, the channel, the build, what was verified — is in
[`docs/COMPANION.md`](docs/COMPANION.md).

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
that is waiting for you), then removes the credentials, and only then deletes what the
games left behind. It reports nothing as done that was not confirmed: what it could not do
is a line starting `COULD NOT`, and its exit status is then 1. It needs no network and
revokes nothing at Microsoft — it says so — which is why the runbook is the second step.
The front end has both on its **Security** page, which shows the launcher's verdict and
never a better one.

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
any key of `[companion]`.

### Layout (XDG)

```
~/.config/bastion-mc/        config.toml, trusted-keys.d/
~/.local/share/bastion-mc/   instances/<id>/, libraries/, versions/, assets/, vault/
~/.cache/bastion-mc/         scratch/ (the per-launch game root, deleted on exit), running/
~/.local/state/bastion-mc/   logs/<instance>/launch-*.log (the newest 20), profile-serials/,
                             accounts/, canary/, activity/, default-account
$XDG_RUNTIME_DIR/bastion-mc/ short-lived sockets: the game's Wayland socket (wl-*) and the
                             companion mod's channel (companion-*)
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

The counts of the last full run are in [Status](#status). The opt-in suites return at once
— and count as passed — unless their variables are set:

| Variables | What then runs |
|---|---|
| `BASTION_NET_TESTS=1` | the relay and the texture gateway against Mojang, an SRV lookup over public DNS, real installs of legacy versions, the mod catalog against Modrinth |
| `BASTION_X11_TESTS=1` | X11 clients in the sandbox, in each network mode |
| `BASTION_WAYLAND_TESTS=1` with `WAYLAND_DISPLAY` and `BASTION_WAYLAND_COMPOSITOR` | the security context against a real compositor (`tests/wayland.rs`) |
| `BASTION_NESTED_TESTS=1` (`BASTION_NESTED_CLIPBOARD_TEST=1` for the clipboard test) | the game's display inside gamescope (`tests/nested.rs`) |
| `BASTION_E2E_ROOT` and `BASTION_E2E_INSTANCE` | the real Minecraft client through the whole launch path (`tests/nested_game.rs`), the companion mod in the real game (`tests/companion_game.rs`), and (`BASTION_E2E_ROOT` alone) the companion's compile-only stand-ins against the real Fabric Loader and Mixin jars |
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
there; `BASTION_COMPANION_KEYPRESS=1` also presses the zoom key through XTEST — only on a
private display that nothing else uses, because the key goes to whatever has the focus.

The front end's widget tests render in software (`.cargo/config.toml` selects the
renderer) and leave screenshots of every state in `target/gui-snapshots/`
(`BASTION_GUI_SNAPSHOT_SCALE=2` for twice the resolution). Its emulator tests create a
headless GPU context where there is a GPU, and the driver then writes a shader cache into
`XDG_CACHE_HOME`; point the four XDG variables somewhere else to keep it out of yours.

Coverage-guided fuzzing of the same parsers: [`fuzz/README.md`](fuzz/README.md).
`cargo deny check` (configuration in `deny.toml`) audits the dependency tree.

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
| `src/egress/` | DNS responder, SNI parser, allowlisting relay, texture gateway, `broker.rs` (credential broker), `udp.rs` (datagram relay for voice chat) |
| `src/auth/` | OAuth (PKCE, device code), Xbox, Minecraft services (profile, skins, capes), token stores, account registry, canary, activity log |
| `src/confine.rs`, `src/confine/filter.rs`, `src/persist.rs` | the launcher's self-confinement (Landlock, and its system-call filter); strict re-encoding of settings files |
| `src/scram.rs`, `src/running.rs`, `src/proofs.rs` | emergency removal of credentials, markers of running commands and locks on their directories, the ownership export |
| `src/tree.rs` | deleting and copying directory trees that the game has written (bounded, link-safe) |
| `src/fuzzing.rs` | one harness per parser of untrusted bytes (used by `tests/robustness.rs` and `fuzz/`) |
| `src/companion/` | the companion mod's launcher side: configuration, the injection decision, `jar.rs` (the build recipe, also used by `build.rs`), `channel.rs` (the socket the mod reports on) |
| `native/noclip.c` | the library preloaded into gamescope that cuts its clipboard bridge |
| `docs/` | threat model, token path, the scram runbook, the dedicated-account guide, mods, the companion mod; `notes/` (engineering notes), `images/` (screenshots) |
| `tests/support/` | scripts that fetch, verify and run the compositors the display tests need |
| `tests/java/` | Java programs the tests start: the broker and voice probes, and a stand-in "game" for the companion mod (`fakegame*`) |
| `src/mojang/` | version metadata, argument rendering, launch frame, verified download store |
| `src/profile/` | signed manifest, Ed25519 trust store, strict `mods/` verification, authoring, `capabilities.rs` (the scan of what jars refer to), `modrinth.rs` (registry client), `catalog.rs` (`profile add`, `profile check`), `packs.rs` (resource and shader packs), `zipdir.rs` (the strict zip directory reader they are checked with) |
| `src/secret.rs`, `src/hardening.rs` | locked self-wiping memory, zero-on-free allocator, process hardening |
| `src/events.rs` | how the launcher reports progress: text for a terminal, JSON events for a front end |
| `crates/proto/` | the event protocol shared by launcher and front ends (types only), and the runbook's text with the only links a front end may open |
| `gui/` | `bastion-mc-gui`: `backend.rs` (worker), `app.rs` (state and logic), `view/` (the pages and the parts they are built from), `hud.rs` (drawn elements), `theme.rs`, `runbook.rs`, `console.rs` |
| `java/BastionBootstrap.java` | the 118-line class that receives the launch frame |
| `java/companion/`, `java/stubs/` | the companion mod's sources; compile-only stand-ins for the seven Fabric Loader and Mixin types it is compiled against (never shipped) |
| `packaging/arch/PKGBUILD` | hardened Arch packages (`bastion-mc`, `bastion-mc-gui`) |

## Trademarks and licence

bastion-mc is not an official Minecraft product. It is not approved by or associated with
Mojang or Microsoft. "Minecraft" is a trademark of Mojang and Microsoft; other names belong
to their owners. The launcher downloads Mojang's game files and third-party mods at your
request from where their publishers offer them, and redistributes none of them.

MIT, see [`LICENSE`](LICENSE).
