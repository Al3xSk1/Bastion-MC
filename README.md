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
* **The launcher locks itself down too**: once the game runs it gives up file access
  outside the instance, program execution and most of the network, on all its threads.
* **For the day something goes wrong**: a canary that notices when the account's name,
  skin or cape changed behind your back, `bastion-mc scram` to remove every credential from
  the machine in one step, and a built-in recovery runbook with verified links.

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
| Sandbox, seccomp, Landlock, egress relay, credential broker, signing, store, TLS policy, event interface, sound-server check, settings sanitisers, account registry, canary, scram, packs | 312 tests in the workspace (`cargo test --workspace`); about 25 of them start the real sandbox; 8 feed generated and mutated input to every parser of untrusted bytes |
| Display isolation | Wayland security context: against a real headless sway (privileged globals withheld). Nested compositor: against real gamescope 3.17 on a private X display, including the clipboard cut-off in all four directions. Both with real Minecraft 26.3 rendering through them. **Not** on a real Wayland desktop session, and playability inside gamescope is not assessed |
| Graphical front end | 51 tests without a display (logic, widgets clicked and rendered in software, the whole application against a stand-in launcher); the window run on the X11 session; a real demo launch driven through its backend worker |
| Arch packages (`makepkg`) | both build, their `check()` passes and both binaries pass the hardening check |
| Real Minecraft 26.3 client in the sandbox (demo mode, no account), vanilla and as a signed Fabric profile | runs: hardware OpenGL, window, audio, HTTPS through the relay and the broker, mods loaded from the sealed mount; also as a `--burner` session |
| Microsoft → Xbox → Minecraft sign-in, and everything that needs a signed-in account (the broker against the real Mojang API, `skin`/`cape`, the canary on a real profile) | implemented and tested against scripted replies and a stand-in Mojang; **not yet run against the live services** (needs your client ID) |
| NVIDIA, Java 8-era versions end to end, real `age` binary and hardware plugins, coverage-guided fuzzing (needs a nightly toolchain) | implemented, **not exercised** on the development machine |

## Requirements

```
pacman -S bubblewrap jre21-openjdk      # plus the JRE each game version needs (8, 17, 21, 25…)
pacman -S gamescope                     # X11 sessions: gives the game a display of its own
```

Kernel ≥ 6.12 with `landlock` in the LSM list and unprivileged user namespaces (the Arch
defaults). Newer kernels add more: 7.0 lets the launcher confine itself, 7.2 adds UDP
rules; `bastion-mc doctor` says what this machine can and cannot enforce. A systemd user
session provides the resource limits. Building needs `cargo`, a C compiler (the one Rust
links with) and a JDK (`javac`).

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
  either direction. Options such as size and full screen go in
  `nested_compositor_args = ["-W", "1920", "-H", "1080", "-f"]`.
  Without gamescope the launch is refused, unless you set `allow_x11 = true` and accept
  the risk.
* On a **Wayland** session it asks the compositor for a socket that is marked as sandboxed
  (`wp_security_context_v1`), on which the compositor withholds screen capture, input
  injection and clipboard-manager access. According to the protocol registry's support
  table (wayland.app, read 2026-11-13) KWin (KDE Plasma), Sway, Hyprland, niri, COSMIC,
  labwc and river implement it; Mutter (GNOME), Weston and Wayfire do not. Only sway was
  tested here. Where a compositor lacks it, the launcher says so and uses the session
  socket (`wayland_security_context = "require"` refuses instead) — on GNOME that means
  the game is an ordinary Wayland client of your session.

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
bastion-mc skin set ~/skins/new.png --slim         # text chunks, time stamps, EXIF are stripped first
bastion-mc cape set Migrator                       # or: none
```

Each label has its own stored token (its own keyring entry, or its own vault file — with
its own keys, if you configure `[auth.age.accounts.<label>]`).

The launcher pins what an account looks like — name, skin, cape — and compares at every
launch. If that changed and not through the commands above, the launch stops and tells you
(`canary show`, `canary accept`): either you changed it on minecraft.net, or someone else
is in the account.

### Resource packs and shaders

The game can read packs but cannot put one on disk: `resourcepacks/` and `shaderpacks/`
are read-only inside the sandbox, and a pack a server sends is gone after the session.

```sh
bastion-mc pack import main ~/Downloads/Faithful.zip        # checked, hashed, recorded
bastion-mc pack import main ~/Downloads/shader.zip --shader
bastion-mc pack list main
bastion-mc pack adopt main        # record packs you copied into the instance by hand
```

### Content you do not trust

```sh
bastion-mc launch main --burner
```

No network, no sign-in, no token; the game works on copies of the instance's worlds and
settings, and everything it writes is deleted when it exits. For a downloaded map or pack
you want to look at before it gets near your real saves. (The full game starts only for an
account that passed the ownership check at its last sign-in here; otherwise demo mode.)

### Where the refresh token rests

`token_store = "keyutils"` (default) keeps it in the kernel keyring: RAM only, gone at
reboot, unreachable from the sandbox — and readable by other programs of yours in the same
login session. `token_store = "age"` keeps it in a file encrypted by `age`; with a
**hardware-backed identity** (`age-plugin-yubikey`, `age-plugin-tpm`,
`age-plugin-fido2-hmac`) every decryption needs the device, and a copied vault file is
useless. `bastion-mc doctor` says whether your identity file is one of those or a software
key. List a second recipient (a backup key kept offline) in the recipients file: a TPM
identity sealed to firmware state stops working after a firmware update, and
`bastion-mc vault reseal` re-encrypts the vault to the current recipients.

### Limits, proxy

The sandbox runs in a systemd scope: memory ceiling (default: twice the heap plus 2 GiB),
no swap, 4096 tasks; `cpu_quota_percent` adds a CPU ceiling. `launcher_proxy =
"socks5h://127.0.0.1:9050"` under `[network]` sends the launcher's own traffic (sign-in,
downloads, what the broker forwards) through a SOCKS5 proxy such as Tor; the game's
connections to multiplayer servers still come from your address.

### If you think the account is compromised

```sh
bastion-mc scram            # stop running games, remove every stored sign-in from this machine
bastion-mc wiki scram       # what to do next, with the exact Microsoft pages
bastion-mc export-proofs proofs.txt     # what the launcher recorded that shows the account is yours
```

`scram` needs no network and revokes nothing at Microsoft — it says so — which is why the
runbook is the second step. The front end has both behind **Emergency…**.

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
copied (`sandbox.persist_files`).

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
| `src/confine.rs`, `src/persist.rs` | the launcher's self-confinement; strict re-encoding of settings files |
| `src/scram.rs`, `src/running.rs`, `src/proofs.rs` | emergency removal of credentials, markers of running launches, the ownership export |
| `src/fuzzing.rs` | one harness per parser of untrusted bytes (used by `tests/robustness.rs` and `fuzz/`) |
| `native/noclip.c` | the library preloaded into gamescope that cuts its clipboard bridge |
| `docs/` | threat model, token path, the scram runbook, the dedicated-account guide |
| `tests/support/` | scripts that fetch, verify and run the compositors the display tests need |
| `src/mojang/` | version metadata, argument rendering, launch frame, verified download store |
| `src/profile/` | signed manifest, Ed25519 trust store, strict `mods/` verification, authoring, `packs.rs` (resource and shader packs) |
| `src/secret.rs`, `src/hardening.rs` | locked self-wiping memory, zero-on-free allocator, process hardening |
| `src/events.rs` | how the launcher reports progress: text for a terminal, JSON events for a front end |
| `crates/proto/` | the event protocol shared by launcher and front ends (types only) |
| `gui/` | `bastion-mc-gui`: `backend.rs` (worker), `app.rs` (state and logic), `view.rs`, `theme.rs`, `console.rs` |
| `java/BastionBootstrap.java` | the 118-line class that receives the launch frame |
| `packaging/arch/PKGBUILD` | hardened Arch packages (`bastion-mc`, `bastion-mc-gui`) |

## Licence

MIT, see [`LICENSE`](LICENSE).
