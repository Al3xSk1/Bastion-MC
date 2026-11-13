# bastion-mc

A Minecraft launcher for Arch Linux that treats the game as hostile code.

* **No central anything.** No built-in OAuth client ID, no telemetry, no update check. The
  launcher talks to Microsoft, Xbox Live and Mojang directly, with *your* Azure application.
* **Verified supply chain.** Every Mojang file is checked against the published hash chain
  on download and again at every launch. Modded instances exist only by virtue of a manifest
  you signed (Ed25519); `mods/` must match it exactly.
* **Stacked sandbox.** bubblewrap namespaces → Landlock → seccomp allowlist, with an empty
  root filesystem, a synthetic `/etc`, no route out of the network namespace except through
  the launcher's allowlisting relay, and only one display socket, one audio socket and the
  GPU as holes.
* **The access token never touches a command line** or the environment; it is handed to the
  JVM over a socket.

Read [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) first. It lists what is defended, how
each claim is tested, and — in §5 — what is **not** defended.
[`docs/TOKEN_INJECTION.md`](docs/TOKEN_INJECTION.md) covers the token path in detail.

## Status

Built and tested on Arch Linux (kernel 7.2, Landlock ABI 10, bubblewrap 0.13.1, OpenJDK 27,
AMD GPU, X11 session).

| | |
|---|---|
| Sandbox, seccomp, Landlock, egress relay, signing, store, TLS policy, event interface, sound-server check | covered by 145 launcher tests; 20 of them start the real sandbox |
| Graphical front end | 41 tests without a display (logic, widgets clicked and rendered in software, the whole application against a stand-in launcher); the window run on the X11 session; a real demo launch driven through its backend worker |
| Arch packages (`makepkg`) | both build, their `check()` passes and both binaries pass the hardening check |
| Real Minecraft 26.3 client in the sandbox (demo mode, no account), vanilla and as a signed Fabric profile | runs: hardware OpenGL, window, audio, HTTPS through the relay, mods loaded from the sealed mount |
| Microsoft → Xbox → Minecraft sign-in | implemented and tested against scripted replies; **not yet run against the live services** (needs your client ID) |
| NVIDIA, Wayland (sandbox and front end), Java 8-era versions end to end, real `age` binary | implemented, **not exercised** on the development machine |

## Requirements

```
pacman -S bubblewrap jre21-openjdk      # plus the JRE each game version needs (8, 17, 21, 25…)
```

Kernel ≥ 6.12 with `landlock` in the LSM list and unprivileged user namespaces (the Arch
defaults). Building needs `cargo` and a JDK (`javac`).

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

On an **X11** session the launcher refuses to start until you set
`allow_x11 = true` under `[sandbox]`: X11 lets any client log keys and inject input into
other windows, which is a way out of the sandbox. Prefer a Wayland session.

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

## Graphical front end

`bastion-mc-gui` is an optional window (iced, always dark) around the same launcher:
account status and sign-in (the device code is shown large, with copy and open buttons),
an instance drop-down with a "new instance" form, a Play button that is disabled while a
launch is in progress and shows download progress, and a collapsible console with the
launcher's, the relay's and the game's output.

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
`logout`, `versions`, `instance list|new`, `install`, `launch`.

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
~/.cache/bastion-mc/         per-launch scratch game root (deleted on exit)
~/.local/state/bastion-mc/   logs/<instance>/launch-*.log, profile-serials/
```

Inside an instance only `saves/`, `logs/`, `config/` and `resourcepacks/` are writable by
the game. `options.txt` and `servers.dat` are copied back by the launcher after each session
(`sandbox.persist_files`).

## Tests

```sh
cargo test                         # the launcher: needs bubblewrap, python3, a JDK, user namespaces
cargo test --workspace             # plus the event protocol and the front end (no display needed)
BASTION_NET_TESTS=1 cargo test     # also the tests that contact Mojang and public DNS
BASTION_X11_TESTS=1 cargo test     # also X11 clients in the sandbox (needs an X session, glxinfo)
```

The front end's tests render in software (`.cargo/config.toml` selects the renderer) and
leave screenshots of every state in `target/gui-snapshots/`.

## Source map

| Path | What |
|---|---|
| `src/sandbox/` | bubblewrap orchestration, in-sandbox init, Landlock, seccomp, netns helper, display/audio/GPU exposure |
| `src/egress/` | DNS responder, SNI parser, allowlisting relay, texture gateway |
| `src/auth/` | OAuth (PKCE, device code), Xbox, Minecraft services, token stores |
| `src/mojang/` | version metadata, argument rendering, launch frame, verified download store |
| `src/profile/` | signed manifest, Ed25519 trust store, strict `mods/` verification, authoring |
| `src/secret.rs`, `src/hardening.rs` | locked self-wiping memory, zero-on-free allocator, process hardening |
| `src/events.rs` | how the launcher reports progress: text for a terminal, JSON events for a front end |
| `crates/proto/` | the event protocol shared by launcher and front ends (types only) |
| `gui/` | `bastion-mc-gui`: `backend.rs` (worker), `app.rs` (state and logic), `view.rs`, `theme.rs`, `console.rs` |
| `java/BastionBootstrap.java` | the 118-line class that receives the launch frame |
| `packaging/arch/PKGBUILD` | hardened Arch packages (`bastion-mc`, `bastion-mc-gui`) |

## Licence

MIT, see [`LICENSE`](LICENSE).
