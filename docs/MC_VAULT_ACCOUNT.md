# Keeping other programs of yours away from the game: `ptrace_scope` and a dedicated account

The sandbox protects *you from the game*. It does not protect *the game and the launcher
from the rest of what runs as you* — the editor plugin, the `npm install` script, the
browser extension host (adversary **U** in the threat model, row P7 and §5.1). Software
with your UID can:

* read the **game's memory**, token included when the broker is off, because the game
  lives in a user namespace that your UID owns (`process_vm_readv` succeeded in experiment
  E1 even with `kernel.yama.ptrace_scope=1`);
* read the **refresh token** from the kernel keyring of your login session;
* edit `config.toml`, the instance directories, and whatever `bastion-mc` your `$PATH`
  finds first.

Nothing a program running as you can do removes that: the kernel decides it by UID. There
are two real fixes, and they can be combined. Both need root once. **Neither was exercised
on the development machine** (they change the machine, not the launcher); the commands are
the standard ones, and `bastion-mc doctor` reports the state of the first.

What already narrows the gap without them: the credential broker (the game holds only a
placeholder, so its memory is not worth reading for the token), `memfd_secret` memory for
the tokens in the launcher (not readable through `/proc/<pid>/mem` or `process_vm_readv`
even by a process that could otherwise attach), and an age vault bound to a hardware key
instead of the keyring (a stolen vault file cannot be decrypted elsewhere, and each
decryption needs the device).

## Option 1 — `kernel.yama.ptrace_scope = 3`

No process can attach to another, by any means, until the next boot.

```sh
echo 'kernel.yama.ptrace_scope = 3' | sudo tee /etc/sysctl.d/90-ptrace.conf
sudo sysctl --system
```

* Closes the memory-reading path (P7) for the game *and* the launcher, against every
  same-user program.
* Costs: `gdb -p`, `strace -p`, `perf` on running processes, and crash handlers that attach
  stop working for everyone, root included. Starting a program *under* a debugger still
  works. The value cannot be lowered again without a reboot — that is what makes it worth
  something.
* Does not close: the keyring (use the age vault), and tampering with files.

## Option 2 — a dedicated UNIX account (`mc-vault`)

Run the launcher as a different user. Programs running as your everyday user are then
"another local user" (adversary **O**) to it, and everything in the threat model that is
"none for O" applies to them: no memory access, no keyring access, no access to its files.

```sh
sudo useradd --create-home --shell /usr/bin/nologin --comment 'Minecraft launcher' mc-vault
sudo chmod 700 /home/mc-vault
sudo loginctl enable-linger mc-vault        # a user manager for it: needed for the resource limits
```

Starting a session as that user — a real one, with its own `XDG_RUNTIME_DIR`, user manager
and session keyring (plain `sudo -u` gives none of these):

```sh
run0 --user=mc-vault --setenv=WAYLAND_DISPLAY=/run/user/$(id -u)/wayland-0 bastion-mc launch main
#   or: sudo machinectl shell mc-vault@ /usr/bin/bastion-mc launch main
```

The account needs three things from your session, and each is a decision:

| Needs | How | What that grants |
|---|---|---|
| The display | **Wayland:** `setfacl -m u:mc-vault:x /run/user/$(id -u)` and `setfacl -m u:mc-vault:rw /run/user/$(id -u)/wayland-0` (after each login; the socket is recreated). The launcher still wraps it in a security context for the game. **X11:** `xhost +si:localuser:mc-vault`. | Wayland: what any Wayland client of yours gets. X11: everything — any X client can watch and drive every other one, so with X11 the separate account protects the tokens and files, **not** your desktop. Use gamescope: then only the compositor, not the game, is that X client. |
| The GPU | Render nodes (`/dev/dri/renderD*`) are world-accessible on Arch; nothing to do. Check with `ls -l /dev/dri`. | — |
| Sound | `setfacl -m u:mc-vault:rw /run/user/$(id -u)/pulse/native`, and `--setenv=PULSE_SERVER=unix:/run/user/<your uid>/pulse/native` — or play with `audio = "none"`. | Playback and recording through your sound server, as for the game today. |

Its configuration, instances, vault and keyring are its own (`/home/mc-vault/.config/bastion-mc/…`):
sign in once as that user, and keep your signing key and Azure client ID there.

* Closes: P7 (memory), the keyring, tampering with the launcher's files and configuration —
  for every program that runs as your everyday user.
* Costs: a privileged step at each start (`run0` asks through polkit), files such as
  screenshots and worlds live in the other home directory, and the display has to be
  shared deliberately.
* Does not close: the desktop itself on X11 (above); anything running as root.

## Which one

| | `ptrace_scope=3` | dedicated account |
|---|---|---|
| Game/launcher memory read by your other programs | closed | closed |
| Refresh token in the keyring read by your other programs | open (use the age vault) | closed |
| Config, instances, launcher binary tampered with by your other programs | open | closed |
| Debuggers on the rest of the system | disabled | unaffected |
| Convenience | set once | a prompt per start, a second home |

If the machine also runs code you do not fully trust as yourself — a development machine
does — the dedicated account is the one that changes the picture. If it is a gaming
machine, `ptrace_scope=3` plus the age vault on a hardware key is the smaller step.
