# bastion-mc

A Minecraft launcher for Arch Linux that treats the game as hostile code.

* **No central anything.** No built-in OAuth client ID, no telemetry, no update check. The
  launcher talks to Microsoft, Xbox Live and Mojang directly, with *your* Azure application.
* **Verified supply chain.** Every Mojang file is checked against the published hash chain
  on download and again at every launch. Modded instances exist only by virtue of a manifest
  you signed (Ed25519); `mods/` must match it exactly.
* **Stacked sandbox.** bubblewrap namespaces → Landlock → seccomp allowlist, inside a
  systemd scope with memory and task limits; an empty root filesystem, a synthetic `/etc`,
  no route out of the network namespace except through the launcher's allowlisting relay.
  The holes are one audio socket, the GPU, and a display of the game's own: on Wayland a
  socket the compositor knows to be sandboxed, on X11 a nested compositor.
* **The game does not get your access token.** It is given a placeholder; the launcher
  keeps the real token and attaches it to the handful of Mojang API requests that playing
  needs. Nothing in the game can change your skin, cape or name, or carry the token away.
  (Where the token does have to be in the game — open network mode — it still never
  touches a command line or the environment.)
* **The launcher locks itself down too**: before it reads anything the game sends, it
  gives up — on all its threads — file access outside one settings directory of the
  instance and its cache, program execution, most of the network, and the kernel keyring
  where the sign-in token rests.
* **What is shown as protected was looked at, not assumed**: the resource limits are read
  back from the kernel, the game's Wayland socket is listed, the clipboard cut-off reports
  from inside the compositor — and where a check fails the launcher says so, or refuses.
* **For the day something goes wrong**: a canary that notices when the account's name,
  skin or cape changed behind your back, `bastion-mc scram` to remove every credential from
  the machine in one step (and to say what it could not remove), and a built-in recovery
  runbook with verified links.

Read [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) first. It lists what is defended, how
each claim is tested, and — in §5 — what is **not** defended.
[`docs/TOKEN_INJECTION.md`](docs/TOKEN_INJECTION.md) covers the token path in detail,
[`docs/MC_VAULT_ACCOUNT.md`](docs/MC_VAULT_ACCOUNT.md) what to do about other software
running as you, and [`docs/SCRAM_RUNBOOK.md`](docs/SCRAM_RUNBOOK.md) is the emergency text
the launcher carries (`bastion-mc wiki scram`).

## Status

Built and tested on Arch Linux (kernel 7.2, Landlock ABI 10, bubblewrap 0.13.1, OpenJDK 27,
AMD GPU, X11 session).

| | |
|---|---|
| Sandbox, seccomp, Landlock, egress relay, credential broker, the launcher's own confinement, signing, store, TLS policy, event interface, sound-server check, settings sanitisers, account registry, canary, scram, packs | 375 tests in the workspace (`cargo test --workspace`); about 25 of them start the real sandbox, 4 confine a real multi-threaded process the way the launcher confines itself; 8 harnesses feed generated and mutated input to every parser of untrusted bytes |
| Display isolation | Wayland security context: against a real headless sway (the game's socket is listed: 37 interfaces, none of the 16 known privileged ones). Nested compositor: against real gamescope 3.17 on a private X display, including the clipboard cut-off in all four directions and the control run without it. Both with real Minecraft 26.3 started through them. **Not** on a real Wayland desktop session, and playability inside gamescope is not assessed |
| Graphical front end | 51 tests without a display (logic, widgets clicked and rendered in software, the whole application against a stand-in launcher); the window run on the X11 session; a real demo launch driven through its backend worker |
| Arch packages (`makepkg`) | both build, their `check()` passes and both binaries pass the hardening check |
| Real Minecraft 26.3 client in the sandbox (demo mode, no account), vanilla and as a signed Fabric profile | runs: hardware OpenGL, window, audio, HTTPS through the relay and the broker, mods loaded from the sealed mount; also as a `--burner` session. Run again after the review fixes (launcher confined before it reads anything from the game; THREAT_MODEL §7 E16), without sound |
| Microsoft → Xbox → Minecraft sign-in, and everything that needs a signed-in account (the broker against the real Mojang API, `skin`/`cape`, the canary on a real profile) | implemented and tested against scripted replies and a stand-in Mojang; **not yet run against the live services** (needs your client ID) |
| NVIDIA, Java 8-era versions end to end, real `age` binary and hardware plugins, coverage-guided fuzzing (needs a nightly toolchain) | implemented, **not exercised** on the development machine |

## Requirements

```
pacman -S bubblewrap jre21-openjdk      # plus the JRE each game version needs (8, 17, 21, 25…)
pacman -S gamescope                     # X11 sessions: gives the game a display of its own
```

Kernel ≥ 6.12 with `landlock` in the LSM list and unprivileged user namespaces (the Arch
defaults). Newer kernels add more: the launcher's own confinement needs Landlock ABI 8 for
its file and network part and ABI 9 for all of it (on older kernels it is partial, and
said to be), ABI 10 (Linux 7.2) adds UDP rules; `bastion-mc doctor` says what this machine
can and cannot enforce. A systemd user session provides the resource limits. Building
needs `cargo`, a C compiler (the one Rust links with) and a JDK (`javac`).

```
cargo build --release          # or: cd packaging/arch && makepkg -si
bastion-mc doctor              # checks the machine and starts a real test sandbox
```

## Your own OAuth application

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

## Playing

```sh
bastion-mc status                                  # who is signed in, which instances exist
bastion-mc login                                   # browser + loopback redirect (or --device)
bastion-mc instance new main --minecraft 1.21.4
bastion-mc launch main
bastion-mc launch main --join play.example.org     # allowlist one server for this session
```

Multiplayer servers must be allowlisted (`servers = [...]` under `[network]` in
`config.toml` or in the instance's `instance.toml`); everything else is unreachable.

### The display

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
running game got, next to the Play button.

**Sound** needs one decision too. PulseAudio and pipewire-pulse both let any client ask the
server to load modules, by default, and that leads out of the sandbox. The launcher asks
your sound server before each launch and refuses to hand the game the audio socket unless
the server says no. To keep sound, switch that off once (PipeWire):

```sh
mkdir -p ~/.config/pipewire/pipewire-pulse.conf.d
echo 'pulse.properties = { pulse.allow-module-loading = false }' \
    > ~/.config/pipewire/pipewire-pulse.conf.d/90-no-module-loading.conf
systemctl --user restart pipewire-pulse
```

(`pactl load-module …` stops working for everything, which is the point.) Otherwise set
`audio = "none"`, or `allow_pulse_module_loading = true` to accept the risk.

### Accounts, skins and capes

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

The launcher pins what an account looks like — name, skin, cape — and compares at every
launch. If that changed and not through the commands above, the launch stops and tells you
(`canary show`, `canary accept`): either you changed it on minecraft.net, or someone else
is in the account. A `skin set` or `cape set` pins the part it changed and nothing else.
With `auth.canary = "warn"` the difference is shown by every command that signs in (also
under `--quiet`) and the pin stays as it was.

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

### Content you do not trust

```sh
bastion-mc launch main --burner
```

No network, no sign-in, no token; the game works on copies of the instance's worlds and
settings, and everything it writes is deleted when it exits — the launcher says so when
the deletion worked, and says what is left when it did not. The copy is bounded by the
free space of the cache. For a downloaded map or pack you want to look at before it gets
near your real saves. (The full game starts only for an account that passed the ownership
check at its last sign-in here; otherwise demo mode.)

### Where the refresh token rests

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
binary or device here (see Status).

### Limits, proxy

The sandbox runs in a systemd scope: memory ceiling (default: twice the heap plus 2 GiB),
no swap, 4096 tasks; `cpu_quota_percent` adds a CPU ceiling. Once the game runs the limits
are read back from the kernel: if they are not in force, the launch says so and does not
show them as on (`cgroup = "require"` stops it). `launcher_proxy =
"socks5h://127.0.0.1:9050"` under `[network]` sends the launcher's own traffic (sign-in,
downloads, what the broker forwards) through a SOCKS5 proxy such as Tor; the game's
connections to multiplayer servers still come from your address. In this tree the same is
true of the HTTPS connections that the relay only passes through (`*.minecraft.net`): they
are resolved and dialled locally, not through the proxy (review finding B-M3).

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
The front end has both behind **Emergency…**.

## Graphical front end

`bastion-mc-gui` is an optional window (iced, always dark) around the same launcher:
account status, an account picker and sign-in (the device code is shown large, with copy
and open buttons), an instance drop-down with a "new instance" form, a Play button that is
disabled while a launch is in progress and shows download progress and — once the game
starts — which protections that launch runs with, a collapsible console with the
launcher's, the relay's and the game's output, and an emergency panel (scram, the runbook
and its links).

```sh
cargo build --release --workspace        # builds bastion-mc and bastion-mc-gui
target/release/bastion-mc-gui            # finds bastion-mc next to itself, else /usr/bin
```

It is a separate program on purpose, not a mode of the launcher:

```text
 bastion-mc-gui  (no secrets: window, fonts, GPU)        bastion-mc --events <command>
 ┌───────────────────────────────────────────┐           (hardened, holds the tokens)
 │ view   state ─▶ widgets, pure             │
 │ app    update(): the only place state     │  spawn    one process per command:
 │        changes; never blocks              │ ────────▶ status · login · logout · versions
 │ backend  tokio task per command:          │           instance new · install · launch
 │        child stdout ─▶ typed Update ─▶    │ ◀──────── stdout: one JSON event per line
 │        tokio mpsc ─▶ iced Task ─▶ update  │ ────────▶ stdin:  "stop" (or just closing it)
 └───────────────────────────────────────────┘
```

* The UI thread never waits on the network, the disk or the game. Every launcher command
  runs in a child process supervised by a tokio task; progress arrives as messages.
* Tokens stay in the launcher process. The event protocol
  ([`crates/proto`](crates/proto/src/lib.rs)) has no field that could carry one, and the
  launcher binary links none of the GUI's dependencies.
* If the window is closed, or the front end dies, the launcher notices its control pipe
  closing and shuts the game down properly.

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
from the window; sign in and launch from the command line in that setup.

## Curated (modded) profiles

```sh
bastion-mc key generate ~/secure/bastion.key       # ideally kept off this machine
bastion-mc key trust me ed25519:…                  # the public key it printed

bastion-mc profile new perf --minecraft 1.21.4 --fabric
cp ~/Downloads/sodium-*.jar ~/.local/share/bastion-mc/instances/perf/mods/
bastion-mc profile lock perf                       # records name, size, SHA-256 of each jar
$EDITOR ~/.local/share/bastion-mc/instances/perf/profile.json    # review it
bastion-mc profile sign perf --key ~/secure/bastion.key
bastion-mc launch perf
```

At launch the signature, the manifest's serial (no rollback), every loader library and every
jar in `mods/` are verified; one extra, missing or modified file aborts the launch. If
root-owned keys exist in `/etc/bastion-mc/trusted-keys.d/`, only those are trusted.

## Layout (XDG)

```
~/.config/bastion-mc/        config.toml, trusted-keys.d/
~/.local/share/bastion-mc/   instances/<id>/, libraries/, versions/, assets/, vault/
~/.cache/bastion-mc/         per-launch scratch game root (deleted on exit), running/
~/.local/state/bastion-mc/   logs/<instance>/launch-*.log (the newest 20), profile-serials/,
                             accounts/, canary/, activity/, default-account
```

Inside an instance only `saves/`, `logs/` and `config/` are writable by the game;
`mods/`, `resourcepacks/` and `shaderpacks/` reach it as sealed read-only mounts of
verified files (`packs.lock.json` records the packs). `options.txt` and `servers.dat` are
carried into each session and back by the launcher, parsed and re-encoded both ways, never
copied (`sandbox.persist_files`). Between sessions they rest in the instance's `persist/`
directory — the one part of the instance the launcher can still write while the game
runs. (An instance from before this layout has them moved there at its next launch; to
change a setting by hand, edit the file in `persist/`.)

## Tests

```sh
cargo test                         # the launcher: needs bubblewrap, python3, a JDK, user namespaces
cargo test --workspace             # plus the event protocol and the front end (no display needed)
BASTION_NET_TESTS=1 cargo test     # also the tests that contact Mojang and public DNS
BASTION_X11_TESTS=1 cargo test     # also X11 clients in the sandbox (needs an X session, glxinfo)
BASTION_FUZZ_ITERATIONS=300000 cargo test --release --test robustness   # a longer generated-input run
```

The display tests need a Wayland compositor and gamescope, which need not be installed:
`tests/support/fetch.sh` downloads the Arch packages, checks each against the pacman sync
database *and* its signature, and unpacks them under `target/third-party/`; they only ever
run inside bubblewrap.

```sh
tests/support/fetch.sh
RT=$(mktemp -d)
tests/support/sway-headless.sh "$RT" &            # a headless Wayland compositor
BASTION_WAYLAND_TESTS=1 WAYLAND_DISPLAY=$RT/wayland-1 BASTION_WAYLAND_COMPOSITOR=$RT/wayland-1 \
    cargo test --test wayland
tests/support/private-x11.sh "$RT/x" & sleep 2; ln -s "$RT/x/x11/X0" /tmp/.X11-unix/X77   # an invisible X display
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_NESTED_TESTS=1 BASTION_NESTED_CLIPBOARD_TEST=1 \
    BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt) cargo test --test nested -- --test-threads=1
```

Coverage-guided fuzzing of the same parsers: [`fuzz/README.md`](fuzz/README.md).
`cargo deny check` (configuration in `deny.toml`) audits the dependency tree.

The front end's tests render in software (`.cargo/config.toml` selects the renderer) and
leave screenshots of every state in `target/gui-snapshots/`.

## Source map

| Path | What |
|---|---|
| `src/sandbox/` | bubblewrap orchestration, in-sandbox init, Landlock, seccomp, netns helper, display/audio/GPU exposure; `wayland.rs` (security context), `nested.rs` (gamescope), `cgroup.rs` (limits) |
| `src/egress/` | DNS responder, SNI parser, allowlisting relay, texture gateway, `broker.rs` (credential broker) |
| `src/auth/` | OAuth (PKCE, device code), Xbox, Minecraft services (profile, skins, capes), token stores, account registry, canary, activity log |
| `src/confine.rs`, `src/confine/filter.rs`, `src/persist.rs` | the launcher's self-confinement (Landlock, and its system-call filter); strict re-encoding of settings files |
| `src/scram.rs`, `src/running.rs`, `src/proofs.rs` | emergency removal of credentials, markers of running commands and locks on their directories, the ownership export |
| `src/tree.rs` | deleting and copying directory trees that the game has written (bounded, link-safe) |
| `src/fuzzing.rs` | one harness per parser of untrusted bytes (used by `tests/robustness.rs` and `fuzz/`) |
| `native/noclip.c` | the library preloaded into gamescope that cuts its clipboard bridge |
| `docs/` | threat model, token path, the scram runbook, the dedicated-account guide |
| `tests/support/` | scripts that fetch, verify and run the compositors the display tests need |
| `src/mojang/` | version metadata, argument rendering, launch frame, verified download store |
| `src/profile/` | signed manifest, Ed25519 trust store, strict `mods/` verification, authoring, `packs.rs` (resource and shader packs), `zipdir.rs` (the strict zip directory reader they are checked with) |
| `src/secret.rs`, `src/hardening.rs` | locked self-wiping memory, zero-on-free allocator, process hardening |
| `src/events.rs` | how the launcher reports progress: text for a terminal, JSON events for a front end |
| `crates/proto/` | the event protocol shared by launcher and front ends (types only) |
| `gui/` | `bastion-mc-gui`: `backend.rs` (worker), `app.rs` (state and logic), `view.rs`, `theme.rs`, `console.rs` |
| `java/BastionBootstrap.java` | the 118-line class that receives the launch frame |
| `packaging/arch/PKGBUILD` | hardened Arch packages (`bastion-mc`, `bastion-mc-gui`) |

## Licence

MIT, see [`LICENSE`](LICENSE).
