# Bastion OS: an image-based Arch install that runs only bastion-mc

Status: design, 2026-11-14. Not built.

## Intent

The user's words: "a custom arch install script that makes arch atomic and only run this
isolated af, using a custom window manager, isolating everything"; "it's a script you run
with `archinstall --script`"; "the image based system"; later (2026-11-15): "I'm fine ditching
archinstall --script in exchange for something like an ArchTitus install script" and "we need
Bedrock support natively, one-click install, for the OS". Decided with the user:

- **Image-based and atomic**: the operating system is a signed, read-only image; an update
  is a whole new image, never a change in place; a failed update falls back to the previous
  image by itself.
- **Images are built and signed on the machine itself**, with keys created on that machine
  (not in CI).
- **The window manager is gamescope, run as the whole session**: it shows bastion's front end
  full-screen as the only application, and the game inside it.
- **A TPM2 is required**: a machine without one is refused at install.
- **bastion-mc's own source is trusted from GitHub** (over HTTPS), for the install and for
  updates.
- **Networking**: Wi-Fi, iPhone hotspots and Ethernet must work. For college Wi-Fi, the guide
  recommends the MAC-address registration method where the college offers it.
- **Clips leave the machine through LocalSend**, with a time-limited opening: the way off the
  machine for the clips the in-game phone saves.
- **Remote play through Sunshine** (to Moonlight clients), installed natively and **disabled
  by default**, so that the owner can remote in when they switch it on.
- **An ArchTitus-style installer** (a bash script in the repository, run from the official
  Arch ISO) instead of an `archinstall --script` module.
- **Bedrock natively, with a one-click install** from the front end.
- **Everything is set up in the front end**: what bastion-mc's terminal commands do today
  (signing in, servers, mod profiles and their signing, the phone services' logins) gets pages
  in the front end, because the machine has no terminal.
- **Repair through a signed maintenance boot entry**, which asks for the recovery key.
- **The Java sign-in stays signed in across restarts by default**, with "sign in after each
  restart" as a setting.

Why, as the project's own documents put it: a launcher that runs as an ordinary user cannot
keep other software running as that user away from the game's memory and the access token
(`docs/THREAT_MODEL.md` P7, §5); the dedicated-account guide (`docs/MC_VAULT_ACCOUNT.md`) is
the partial answer, a machine that runs nothing else is the whole one. An immutable, verified
`/usr` was deferred because it "needs a root component" (THREAT_MODEL §8, TODO "Frozen `/usr`
+ JVM"); an installed operating system is that component. And the weakest display cases in
the threat model — compositors without the security-context protocol, X11 sessions — do not
exist on a machine whose only compositor is ours to configure.

## Goals

1. **G1 Install**: from the official Arch ISO, the repository's installer
   (`packaging/os/install.sh`, ArchTitus-style: clone, run, answer a few menus) installs the
   system on a whole disk, after one unmistakable confirmation that the disk is erased.
2. **G2 Atomic, verified OS**: a signed unified kernel image (UKI) and a dm-verity-protected
   `/usr` in two slots (A/B); updates write the spare slot; boot counting falls back to the
   previous slot when a new one does not reach a working session.
3. **G3 The owner's keys**: Secure Boot keys and the image-signing key are created on the
   machine at install time and enrolled into the firmware from its setup mode, keeping
   Microsoft's third-party UEFI certificate authority enrolled by default (graphics cards'
   option ROMs are signed with it; leaving it out can leave a machine without a picture).
4. **G4 Encrypted state**: everything mutable (configuration state, `/var`, `/home`) lives on
   one LUKS2 partition unlocked by the TPM2 against a *signed* PCR policy (so a new signed
   image unlocks it without re-enrolment), with a recovery key shown once at install.
5. **G5 The session**: automatic login of one dedicated unprivileged account into a gamescope
   session whose only client is `bastion-mc-gui`; no desktop, no terminal, no other
   application, no way to a shell from the session. Shut down, restart and update are the
   front end's controls.
6. **G6 The game stays isolated from the front end**: the game runs through bastion's
   existing isolation (its sandbox, its relay, its own display) and never shares a display
   connection with the front end — in a gamescope session the front end is an X client of
   gamescope's Xwayland, so the game must get a display of its own (bastion's nested
   compositor), or the game could read the front end's window and send it input.
7. **G7 Everything else isolated**: no listening services, no `sshd`; an nftables policy that
   lets only the session's account and the updater reach the network; kernel and sysctl
   hardening that keeps what bastion's sandbox needs (unprivileged user namespaces).
8. **G8 Updates on the machine**: a root-owned update service rebuilds the next image from
   the recipe stored on the machine, from Arch's signed packages and bastion-mc's source,
   signs it with the machine's key, installs it into the spare slot and asks for a restart.
   The front end can start it (and only it) through a narrow polkit rule.
9. **G9 Verified here, without touching a disk**: the whole flow — install, first boot,
   session, update, fallback, tampering — runs in QEMU with UEFI Secure Boot (OVMF) and an
   emulated TPM2 (swtpm), driven by scripts in the repository.

## Non-goals (first version)

- NVIDIA's proprietary driver (its kernel modules would have to be signed for Secure Boot).
- Dual boot, installing onto a partition, or keeping another OS on the disk.
- More than one account; a general desktop; a web browser outside the phone's sandboxed one;
  remote management.

## Anti-goals (a failure even if it "works")

- Any path by which the game, or a page in the phone's browser, reaches the front end's
  window, input or clipboard.
- A key, TPM policy or update step that can lock the owner out with no recovery path.
- An update that can leave the machine unbootable without falling back by itself.
- Code that runs without verification: packages only through pacman's signature checks,
  bastion-mc only from a source the owner's trust anchor covers (see "Left open").
- Enrolling Secure Boot keys in a way that can brick firmware or graphics (only the owner's
  keys, by default).

## Constraints

- **The installer** is a bash script in the repository, ArchTitus-style: from the official
  Arch ISO, `pacman -Sy git`, clone `https://github.com/Al3xSk1/Bastion-MC` (GitHub over
  HTTPS is the trust anchor the owner chose), `packaging/os/install.sh`. Its menus use
  `whiptail` (installed into the live environment with pacman if the ISO lacks it). It
  depends on nothing of archinstall, whose script API changes between releases.
- **Kernel**: Arch's `linux`, not `linux-hardened` — bastion's sandbox (bubblewrap) needs
  unprivileged user namespaces, which `linux-hardened` turns off by default. Landlock ABI 9
  or newer (kernel 7.0+) for the launcher's self-confinement.
- **The live ISO's writable space is small** (a RAM overlay): the image is built on the
  target disk, after partitioning, not in the live environment's RAM.
- **Tools on the ISO**: archinstall is there; `mkosi`, `systemd-ukify` and friends are
  installed into the live environment with pacman (signed packages) at the start.
- **bastion-mc's sandbox and confinement stay exactly as they are**: the OS adds layers
  around them, never weakens one (no setuid helpers, user namespaces stay on).
- **Prior art**: ParticleOS (systemd's image-based OS built with mkosi, Arch supported) — its
  key handling (`mkosi genkey`; `SecureBootKey=`, `SignExpectedPcrKey=`, `VerityKey=`), its
  setup-mode self-enrolment of the owner's keys plus Microsoft's third-party CAs, and its
  update path (`mkosi … sysupdate -- update --reboot` from the recipe) are the model.

## Approaches considered

1. **Where images are built and signed** — (A) on the machine, with keys made there
   (**chosen** by the user: nothing outside to trust, the owner holds the keys; costs a
   rebuild of about ten minutes per update, and the signing key lives on the machine, in the
   encrypted partition); (B) in GitHub Actions (rejected: the key would live in CI secrets and
   every machine would trust it — right for distributing to strangers, wrong for a box that
   is meant to trust nobody else).
2. **What "atomic" means** — (A) image-based, signed UKI + dm-verity `/usr`, A/B,
   `systemd-sysupdate` (**chosen** by the user); (B) btrfs snapshots with rollback (rejected:
   atomic but not verified — a modified system boots as happily as the original).
3. **The window manager** — (A) gamescope as the session (**chosen** by the user: purpose-built
   for games, already part of bastion, quickest to a solid result); (B) a compositor of our
   own in Rust (deferred: full control, but a large security-critical component).
4. **The installer** — (A) an `archinstall --script` module (the first choice; replaced:
   archinstall's script API changes between releases, and the image tools do the real work
   anyway); (B) an ArchTitus-style bash installer in the repository whose menus collect the
   choices and whose steps run mkosi and `systemd-repart` (**chosen** by the owner,
   2026-11-15).

## The design

### Install (`packaging/os/install.sh`)

1. **Checks**, each refusing with the reason: booted by UEFI on x86_64; the archinstall
   version; network; Secure Boot in setup mode (otherwise: how to reset the firmware's keys);
   a TPM2 (required: a machine without one is refused).
2. **Choices** through the installer's menus: the target disk (whole disk; the confirmation
   names its model and size), keyboard layout, time zone, locale, host name, the player
   account's name, Wi-Fi (if any) — nothing else.
3. **Disk**: GPT with the Discoverable Partitions types: an ESP of 1 GiB; `usr-A`,
   `usr-A-verity`, `usr-A-verity-sig`; the same for B (empty); and the rest as one LUKS2
   partition for the mutable state. The LUKS2 partition is created first and used as the build
   workspace and package cache.
4. **Keys**: Secure Boot PK, KEK and db, and the key that signs verity data and the expected
   PCR values, created on the machine (`mkosi genkey` and friends) and stored in the
   encrypted partition, root-only. Firmware enrolment at first boot through systemd-boot's
   setup-mode enrolment, with Microsoft's third-party CA included by default.
5. **Image**: mkosi builds it from the recipe in the repository (`packaging/os/`), copied onto
   the encrypted partition so that updates build from the same recipe: Arch packages
   verified by pacman, bastion-mc built from source in mkosi's build step. Output: the `/usr`
   image (EROFS or squashfs — the builder's choice), its verity data and signature, and the
   signed UKI.
6. **TPM**: the LUKS2 partition bound to TPM2 with the signed PCR 11 policy (plus PCR 7, the
   Secure Boot state); a recovery key generated, shown once on screen, and enrolled as a
   second key slot.
7. **Finish**: a summary (what was installed, the recovery key again, what happens at first
   boot), and restart.

### Boot

UEFI Secure Boot (the owner's keys) → systemd-boot (signed) → the UKI (signed: kernel,
initrd, command line with the `/usr` verity root hash) → the initrd sets up `/usr` through
dm-verity with its signature checked → the TPM2 unlocks the state partition against the
signed policy → boot counting: an entry is marked good only when the session has started;
after three failures systemd-boot boots the other slot.

### The file system

"Hermetic `/usr`": everything that comes from packages lives in the verified `/usr` image.
`/etc` is populated at first boot from the image's factory defaults (systemd-tmpfiles);
`/var` and `/home` are on the encrypted partition; `/tmp` is a tmpfs. The builder documents
every writable path and why it is writable.

### The session

- One account (`player`, the name chosen at install), no password login, no `sudo`, no
  login shell; root's password is locked.
- Automatic login on the first virtual terminal into gamescope on the DRM backend, running
  `bastion-mc-gui` as its only client; when it exits, it is restarted. The other virtual
  terminals have no getty; SysRq and Ctrl+Alt+Del are off.
- The game keeps bastion's isolation (G6): the launcher, seeing an X display, uses its
  nested compositor for the game, so the game never connects to the display the front end is
  on. This must be checked in the VM, not assumed.
- Audio: PipeWire with its PulseAudio server in the player's session; bastion's existing check
  of the sound server applies.
- Graphics: Mesa with the AMD and Intel Vulkan drivers and the firmware package.
- The front end gains controls for shut down, restart, and start an update (and its
  progress), within its architecture rules (it never links the launcher and handles no
  credential; whatever needs privileges goes through the launcher or a polkit-allowed unit).

### Setting up bastion-mc without a terminal (the front end)

Everything bastion-mc's commands do on a desktop must be possible from the front end, through
the launcher (`bastion-mc --events …`), never by the front end itself:

- **Signing in**: the public app by default (`docs/superpowers/specs/2026-11-14-public-signin-design.md`),
  or the owner's own client ID entered in a settings page.
- **Servers**: adding, removing and editing the server allowlist (`[network] servers`, voice
  ports), and joining one.
- **Mod profiles**: creating a Fabric profile, adding mods from the verified catalog, signing
  it — the profile signing key is made and kept by the launcher on the machine (the front end
  never sees it) — and the companion and phone settings.
- **The phone services' logins** (Discord in the phone's browser, Spotify, the Realms TOTP
  set-up): each sign-in window runs in a display of its own (bastion's nested compositor),
  never on the display the front end is on.
- **Settings**: the network page (Network), Sunshine on and off (Remote play), LocalSend
  (Sending clips), the Java sign-in's persistence (below), updates, shut down and restart.
- My call: in the first version nothing comes onto the machine except mods from the verified
  catalog (no worlds, resource packs or skins from outside); clips go out through LocalSend.

### Bedrock, installed with one click

- **In the image**: `mcpelauncher-client` and `mcpelauncher-ui`, built from source by the
  image recipe with every source pinned by checksum — including the curl tarball that the
  upstream build downloads without one — never from the AUR at build time. bastion-mc's
  Bedrock support (the fifth round: the sandboxed client, the network profile, the sign-in
  bridge) is used as it is.
- **One click**: the front end's "Install Bedrock" starts `mcpelauncher-ui` in a sandbox of
  its own (a display of its own through bastion's nested compositor, its own directory on the
  encrypted partition for the Google sign-in, network only to the hosts Google's sign-in and
  Play delivery need). The player signs in to Google there — Minecraft must be bought on
  Google Play; the guide recommends a dedicated Google account — and picks a version. When the
  download ends, bastion imports it (`bedrock import`), shows the licence substitution once
  with a plain explanation and the switch to accept it, and creates the Bedrock instance.
  Updating the game is the same button.
- The Google credential never reaches the game's sandbox or the launcher; it lives only in
  the UI's own sandboxed directory, and "Remove Google sign-in" deletes it.

### The Java sign-in's token

By default the Microsoft refresh token is kept across restarts: bastion-mc's `age` token store
with the machine's TPM as the identity (`age-plugin-tpm`, no PIN, so the front end needs no
prompt), and a root-owned `/etc/bastion-mc/age-recipients` naming that identity, created at
install. The trade-off, stated in the guide: anyone who switches the machine on can play as
the owner. A setting switches to the kernel keyring (`token_store = "keyutils"`): the token
lives in memory only, and the player signs in again after every restart.

### Updates

`bastion-os-update.service` (root): mkosi builds the next image from the recipe on the
encrypted partition (Arch packages through pacman's signature checks; bastion-mc from its
GitHub repository over HTTPS — my call: the newest commit on `main` whose CI run passed, read
through GitHub's API, so that a commit CI rejected never reaches a machine), signs it with the machine's key, and `systemd-sysupdate`
installs it into the spare slot from a local source; then it waits for the player to
restart. A polkit rule lets the player's account start exactly that unit and nothing else.
A failed boot of the new slot falls back (Boot). The recipe itself is updated only from the
same trusted source as bastion-mc.

### Repair: the maintenance entry

A second UKI profile, signed with the machine's key and listed in the boot menu: it does not
unlock the state partition by TPM (its PCR 11 value is not in the signed policy), asks for the
recovery key, and opens a root shell on the console. Secure Boot stays on (the official Arch ISO
does not boot with Secure Boot on). From there the owner can re-bind the TPM after a firmware
or dbx update has changed PCR 7, repair the network, or reset the machine. My call: when the
TPM refuses at a normal boot (PCR 7 changed), the boot asks for the recovery key and the
session then offers to re-bind the TPM (through the launcher and a polkit-allowed unit).

### Network

- **Connections**: NetworkManager (my call: it covers everything asked for in one daemon) —
  Ethernet; Wi-Fi with WPA2/WPA3-Personal and WPA2-Enterprise (802.1X, for networks such as
  eduroam); iPhone hotspots over Wi-Fi, and over USB (the `ipheth` driver with `usbmuxd`, which
  needs the phone's "Trust this computer"). The front end gets a network page (scan, connect
  with a password, forget, show the state) through the launcher; Wi-Fi passwords are kept by
  NetworkManager on the encrypted partition.
- **College Wi-Fi**: the guide recommends the MAC-address method where it exists (register the
  machine's address with the college's device-registration page, as for game consoles). The
  network page shows the Wi-Fi adapter's real MAC address, and a network can be set to use it
  (no randomised address) so that the registration holds. Captive-portal sign-in pages are not
  supported (no general browser on the machine).
- **Firewall** (nftables, loaded from the image): nothing inbound but replies (and DHCP);
  outbound only for the player's account (bastion-mc and its front end; the launcher enforces
  its own allowlist for the game), root's updater (Arch mirrors, GitHub), `systemd-resolved`
  and the time sync; every other account dropped. Two openings exist, each only while its
  feature is in use (below): LocalSend and Sunshine.

### Sending clips off the machine (LocalSend)

- **Send only**: bastion implements the sending side of LocalSend's protocol (v2): announce on
  the multicast group `224.0.0.167:53317`, list the receivers that answer (their `register`
  call, or the multicast reply), and upload the chosen clips (`prepare-upload`, `upload`,
  `cancel`) to the one the player picks, over HTTPS with the receiver's self-signed
  certificate, showing its fingerprint. The machine never accepts files.
- **Time-limited opening**: started from the front end or the in-game phone's clips app; the
  firewall then allows LocalSend's traffic on the local network (multicast discovery, the
  receivers' `register` calls to port 53317, the upload to the chosen receiver) for a fixed
  time (my call: two minutes for discovery, extended only while a transfer runs), and closes
  by itself. During the opening the machine's port 53317 answers only `register`.
- Only the launcher's clip files can be sent, never another path.

### Remote play (Sunshine)

- Sunshine is installed natively in the image and **off by default** (its service disabled,
  its ports closed). The owner switches it on in the front end, with a sentence that says what
  it means: a paired device sees the screen and controls the machine.
- While it is on: Moonlight clients on the local network can pair (the pairing PIN is entered
  in the front end, not through a web page left open), paired clients can stream; the
  firewall opens Sunshine's streaming ports to the local network only; its configuration web
  interface listens on localhost only. The front end shows when a client is connected.
- How Sunshine captures a gamescope session (gamescope's PipeWire stream, or KMS capture with a
  capability confined to Sunshine's unit) and how it injects input (uinput, allowed to
  Sunshine's account only) is for the builder to establish in the VM; capture and input never
  become available to the game or to the front end.

### Hardening

Sysctls: `kernel.yama.ptrace_scope` raised (bastion needs no ptrace), `kptr_restrict`,
`dmesg_restrict`, `unprivileged_bpf_disabled`, `bpf_jit_harden`, `fs.protected_*`,
`kernel.sysrq = 0`, unprivileged user namespaces left on. Kernel command line:
`lockdown=integrity` and memory-hardening options that do not break gaming performance (the
builder measures). No setuid binaries beyond what the base system cannot work without.

### Repository layout

- `packaging/os/` — the mkosi recipe (configuration, the partition definitions for
  systemd-repart, the units, the nftables policy, the polkit rule, the session), the
  archinstall script, and the bootstrap.
- `tests/os/` — the VM tests (below).
- `docs/OS.md` — the user guide; rows for `docs/THREAT_MODEL.md`.

### Verification (in a VM only)

QEMU with KVM, OVMF with Secure Boot variables in setup mode, swtpm (fetched as a verified
Arch package by `tests/support/fetch.sh`, run in bubblewrap like the other test programs),
the official Arch ISO (downloaded and checked against Arch's release signature), a virtual
disk under `target/`. Driven non-interactively over the serial console. Checks: the install
completes; first boot enrols the keys and Secure Boot is on; `/usr` is verity-protected and
a flipped byte in it stops the boot; the state partition unlocks by TPM without a prompt and
by the recovery key when the TPM state is changed; the session shows bastion's front end
(screenshots through QEMU's monitor); an update builds, installs into slot B and boots; a
deliberately broken slot falls back to A; the nftables policy drops traffic from another
account. Never run against a real disk.

## Left to the builder

The exact mkosi options and partition sizes; EROFS or squashfs for `/usr`; ext4 or btrfs for
the state partition; gamescope's flags; greetd or a systemd unit for the automatic login;
the front end's new controls (within its rules); how the VM
test drives the installer (archinstall's `--silent` with a configuration file, or its own
options).

## Decided with the owner after the first draft

- A TPM2 is required (no passphrase-only mode).
- bastion-mc's source is trusted from GitHub over HTTPS.
- Wi-Fi, iPhone hotspots and Ethernet; the MAC-address method recommended for college Wi-Fi.
- LocalSend, with a time-limited opening, to send clips off the machine.
- Sunshine, native, disabled by default.
- Everything is set up in the front end (the section above).
- Repair: a signed maintenance boot entry.
- The Java sign-in is kept across restarts by default; signing in after each restart is a
  setting.

## Accepted risk (the owner's choice)

bastion-mc's source is trusted from GitHub over HTTPS: anyone who can push to `main` — or who
holds a stolen token of someone who can — gets their code built and signed with each
machine's own key at its next update. The CI check (below) does not change that: an
attacker who can push can make CI pass. Signed release tags would close it and can be added
later without changing the rest.

## My calls the owner can still change

- Updates build the newest commit on `main` whose CI run passed.
- Nothing comes onto the machine except mods from the verified catalog, in the first version.
- The phone services' sign-in windows run in a display of their own.
