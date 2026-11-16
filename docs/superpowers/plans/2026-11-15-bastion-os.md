# Bastion OS Implementation Plan — index and shared contract

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> This file holds what every package shares: the constraints, the decisions, the contract, the order and the integration. Each package has its own plan file (table below); an implementer reads this file, then their package's file, then the package plans their package consumes.

**Goal:** Build Bastion OS as designed. That means an ArchTitus-style installer that puts a signed, verified, A/B-updated image on a whole disk; a gamescope session whose only client is bastion-mc's front end; and front-end pages for everything bastion-mc's terminal commands do today. The owner added two things on 2026-11-16: a config manager for every launcher setting, and four new phone apps (a minecraft.wiki app, waypoints, a clock, and a screenshot manager).

**Architecture:** Seven work packages in three waves.
- **Wave 0** lays two foundations in parallel:
  - a QEMU + OVMF + swtpm test harness;
  - the launcher pieces every later page needs: OS detection, config writes, a display of its own, and sign-in windows.
- **Wave 1** runs six packages in parallel against the contract below: the image and installer, the system services, three groups of front-end pages with their launcher commands, and new phone apps.
- **Wave 2** merges everything, runs the whole flow in VMs, and writes the documents.

**Tech Stack:** bash + whiptail (installer); mkosi 27.1 and systemd 262 (systemd-repart, ukify, systemd-cryptenroll, systemd-sysupdate, systemd-boot); nftables, polkit, NetworkManager, gamescope 3.17, PipeWire, Sunshine, age 1.3 + age-plugin-tpm 1.0; Rust for the launcher, the front end (iced 0.14) and a new root helper; QEMU + OVMF + swtpm for verification.

**Spec:** `docs/superpowers/specs/2026-11-14-bastion-os-design.md` (approved; updated 2026-11-15). Where this plan and the spec disagree, the spec wins, except where "Decisions" below rules otherwise and says why.

## Packages

| Package | Plan file | Wave | Owns |
|---|---|---|---|
| F1 VM harness | `2026-11-15-bastion-os-f1-vm-harness.md` | 0 | `crates/vmtest/`, `tests/support/{fetch.sh,arch-iso.*,swtpm.sh}`, `tests/os_harness.rs` |
| F2 Launcher foundation | `2026-11-15-bastion-os-f2-launcher-foundation.md` | 0 | `src/os/`, `src/config_edit.rs`, display and nested-compositor changes, `src/browser/signin.rs`, `crates/proto/src/setup.rs`, the answer kinds |
| P1 Image and installer | `2026-11-15-bastion-os-p1-image-and-installer.md` | 1 | `packaging/os/{mkosi,repart,secureboot,install}/`, `packaging/os/{install.sh,build-image.sh,sources.toml,player-config.toml}`, `tests/os_recipe.rs`, `tests/os_install.rs` |
| P2 System services | `2026-11-15-bastion-os-p2-system-services.md` | 1 | `crates/os/` (the root helper `bastion-os`), the units, polkit rule, firewall, session and Sunshine files under `packaging/os/mkosi/mkosi.extra/`, `tests/os_services.rs` |
| P3 System pages | `2026-11-15-bastion-os-p3-system-pages.md` | 1 | the launcher's `os` and `network` commands, `crates/proto/src/os.rs`, the front end's System, Network and Remote play pages |
| P4 Setup pages | `2026-11-15-bastion-os-p4-setup-pages.md` | 1 | the launcher's `server`, `token-store`, profile/key, phone-login commands with `--events`, the front end's Settings, Servers, Mods and Phone pages; the config manager (`src/config_schema.rs`, `config list|get|set|unset|check` for every key, `instance config`, the Configuration page) |
| P5 Clips and Bedrock | `2026-11-15-bastion-os-p5-clips-and-bedrock.md` | 1 | `src/localsend/`, the launcher's `clips` and Bedrock set-up commands, `crates/proto/src/{clips,bedrock_setup}.rs`, the recipe fragment that builds mcpelauncher, the front end's Sending clips and Install Bedrock flows; the screenshot manager (the phone app and the front-end page) |
| P6 Phone apps | `2026-11-16-bastion-os-p6-phone-apps.md` | 1 | the phone apps Wiki (minecraft.wiki), Waypoints and Clock (`java/companion/src/bastion/companion/phone/`), their launcher handlers in `src/phone/`, `tests/phone_apps.rs` |

Each package works in its own clone under `target/agents/` (the owner's standing preference), on a branch named after the package. Each ends with a report that gives:
- the head commit and the test counts;
- the final signatures of everything in its **Produces** blocks;
- every `Ruling:` line it made.

The spec's `tests/os/` becomes Cargo integration-test files: `tests/os_harness.rs`, `tests/os_recipe.rs`, `tests/os_install.rs` and `tests/os_services.rs`. Their fixtures and support files go under `tests/os/`.

Packages write their user-facing text into `docs/notes/os-<package>.md` (for example `docs/notes/os-p3.md`). Only the Wave 2 documents pass touches README.md, docs/THREAT_MODEL.md, docs/OS.md and the website, so that six packages do not fight over the same paragraphs or the website's REQUIRE ties.

## Global Constraints

Copied from the spec; every task's requirements include these.

- **Kernel:** Arch's `linux`, never `linux-hardened`. Unprivileged user namespaces stay on. Landlock ABI 9 or newer (kernel 7.0+).
- **TPM:** a TPM2 is required. A machine without one is refused at install.
- **Trust anchor:** bastion-mc's source is trusted from `https://github.com/Al3xSk1/Bastion-MC` over HTTPS. Updates build the newest commit on `main` whose CI run passed.
- **Packages:**
  - Only through pacman's signature checks: repositories core, extra and multilib with `SigLevel = Required DatabaseOptional`.
  - Never from the AUR at build time.
  - `mcpelauncher-client`, `mcpelauncher-ui` and Sunshine are built from source in the image recipe, with every source pinned by SHA-256. That includes what their upstream builds download unpinned: curl, nlohmann json, gamecontrollerdb, glfw. npm packages are pinned through the lockfile's integrity hashes.
- **Sandbox:** bastion-mc's sandbox and confinement stay as they are. The OS adds layers and never weakens one: no setuid or file-capability helpers, and user namespaces stay on. The one narrow addition is Decision 6, which is reviewed.
- **The front end:**
  - It never links the launcher.
  - It never handles an account credential, token or key.
  - Anything privileged goes through the launcher or through a polkit-allowed unit.
  - The exception the spec's network page needs is Decision 5.
- **The machine:**
  - No listening services and no sshd.
  - Inbound: only replies and DHCP.
  - Outbound: only the player's account (uid 1000), root's updater, `systemd-resolved` and `systemd-timesyncd`.
  - Two openings (LocalSend, Sunshine), each only while in use.
- **The installer:**
  - It is bash with whiptail menus, run from the official Arch ISO after `pacman -Sy git` and a clone.
  - It uses nothing of archinstall.
  - It builds the image on the target disk's encrypted partition, never in the live environment's RAM.
- **Secure Boot:**
  - The owner's own keys.
  - Microsoft's certificates stay enrolled by default:
    - in db: Microsoft UEFI CA 2011, Microsoft UEFI CA 2023 and Microsoft Option ROM UEFI CA 2023;
    - in KEK: Microsoft KEK CA 2011 and Microsoft KEK 2K CA 2023.
  - A firmware that is not in setup mode is refused, with how to reset its keys, before anything is erased.
- **Verification:**
  - Only in QEMU VMs (OVMF with Secure Boot variables, swtpm), on virtual disks under `target/`. Never against a real disk.
  - mkosi builds and every third-party source build run **inside VMs only**.
  - The development machine runs only signature-verified Arch packages, the pinned and signature-verified Arch ISO (inside QEMU), and this repository's own code.
- **Session isolation (standing rule):** tests never touch the owner's kernel keyring, session D-Bus (unset `DBUS_SESSION_BUS_ADDRESS`), real XDG directories or display `:0`. Visible windows go to the private display `:77` from `tests/support/private-x11.sh`.
- **Tests:**
  - VM tests are opt-in with `BASTION_OS_TESTS=1`. Without it they print `skipped: set BASTION_OS_TESTS=1` and pass; this repository uses no `#[ignore]`.
  - Static tests always run.
  - A test that needs a tool which is not installed (nft, shellcheck, systemd-analyze) prints `skipped: <tool> not installed` and passes.
  - CI keeps its five jobs. New static tests must pass in its Arch container.
- **Non-goals (first version):** NVIDIA's proprietary driver; dual boot; more than one account; a general desktop or browser; remote management. Nothing comes onto the machine except mods from the verified catalog.
- **Code rules:**
  - Rust toolchain 1.100.0; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
  - New dependencies must pass `cargo deny check`.
  - The front end keeps `unsafe_code = "forbid"`.
  - Commits are logical steps with descriptive messages, ending with the session's Co-Authored-By trailer.

## Decisions this plan makes

Each decision rules on something the spec left open, or settles a conflict found while planning, and says what it costs if it is wrong. Implementers do not reopen these. A package that finds one unworkable records a `Ruling:` and says so in its report.

1. **Switching the Java token to the kernel keyring is done on the root side.**
   - The conflict: the launcher refuses `token_store = "keyutils"` while the root-owned `/etc/bastion-mc/age-recipients` exists (src/auth/store.rs:152-160). That refusal is deliberate policy and stays.
   - So the "sign in after each restart" setting:
     - starts `bastion-os-token-store@keyutils.service`, which moves the file to `/var/lib/bastion-os/age-recipients.off`;
     - writes `auth.token_store = "keyutils"`;
     - signs the account out, so the next sign-in stores the token in the new place.
   - Switching back reverses each step.
   - Cost if wrong: one extra unit, and a sign-in after each switch.
2. **Microsoft's certificates are enrolled from `.auth` files the installer makes.** mkosi's `SecureBootAutoEnroll=` enrols only the owner's certificate. Instead:
   - the installer builds PK/KEK/db/dbx with `sbsiglist` (Microsoft's owner GUID `77fa9abd-0359-4d32-bd60-28f4e78f784b`) and `sbvarsign` (PK signs PK and KEK, KEK signs db);
   - the five Microsoft certificates are vendored in `packaging/os/secureboot/microsoft/` with pinned SHA-256s;
   - `loader.conf` uses `secure-boot-enroll force` and `secure-boot-enroll-action reboot`.

   Cost if wrong: on a firmware that rejects authenticated writes in setup mode, enrolment has to be done by hand. The installer's guide says so.
3. **TPM re-binding without the front end seeing the recovery key.**
   - The state volume's crypttab entry links the volume key into root's keyring (`link-volume-key=`, systemd 256+).
   - `bastion-os rebind` then:
     1. adds a temporary keyfile slot with `cryptsetup luksAddKey --volume-key-keyring` (cryptsetup 2.7+);
     2. re-enrols TPM2 with `systemd-cryptenroll --unlock-key-file … --wipe-slot=tpm2 --tpm2-device=auto --tpm2-public-key=… --tpm2-public-key-pcrs=11 --tpm2-pcrs=7`;
     3. wipes the temporary slot;
     4. records the new PCR 7 value;
     5. unlinks the key.
   - `bastion-os-keyring-cleanup` unlinks the key after every boot whose TPM state is "bound", and 30 minutes after any other boot.
   - Root could already read the volume key from the mapped device, so the keyring adds no exposure.
   - Cost if wrong: re-binding is done from the maintenance entry instead.
4. **Sunshine captures through KMS, with the capability given only by its unit.**
   - Under gamescope, Sunshine can capture only through KMS. It runs as its own system user `bastion-sunshine` with `AmbientCapabilities=CAP_SYS_ADMIN CAP_SYS_NICE` in its unit (no file capabilities on the binary).
   - It reaches `/dev/uinput` and `/dev/uhid` through a udev rule for its group, never through `uaccess`.
   - It gets the player's audio through a capture-only PipeWire socket.
   - Its web interface answers on localhost only (`origin_web_ui_allowed=pc`), and the firewall closes port 47990 to everything but loopback.
   - Cost if wrong: upstream plans to phase out KMS capture. The pinned version keeps it, and the version bump that drops it must move to portal capture.
5. **Wi-Fi passwords and the Sunshine pairing PIN pass through the front end once.**
   - The spec's network page has the player type the Wi-Fi password in the front end, and the pairing PIN is typed there too. These are the network's and the pairing's secrets, not the player's account credentials.
   - They travel only over the control pipe (`answer wifi …`, `answer pin …`), never on a command line, and are never stored or logged by the front end.
   - The launcher hands them to NetworkManager or the helper without putting them on a command line.
   - The proto crate's rule text is amended to say exactly this. Account credentials, tokens, the TOTP secret and the recovery key still never reach the front end: the TOTP secret is shown in a launcher-owned window (F2's `show_page`).
   - Cost if wrong: a stricter variant types the password into a launcher-owned window too.
6. **The in-game phone can send clips (LocalSend) while the launcher is confined.**
   - On Bastion OS, and only with `[clips] enabled`, the launcher sets up three things before step 9:
     - a TCP listener on port 53317, which answers only `register` during an opening;
     - TCP connect to port 53317 on private addresses only;
     - one system-bus connection, used only to start `bastion-os-open-localsend.service`.
   - Each allowance has a test proving it is absent on other systems and with clips off.
   - Cost if wrong: the phone could only queue clips for the front end to send after the game.
7. **The game's display is a nested gamescope inside the session's Xwayland.**
   - Under Bastion OS the launcher always takes its nested path for the game and refuses session sockets, whatever the config says. gamescope's clients get only `DISPLAY`, since `--expose-wayland` is never passed.
   - The fallback, if nesting fails in the VM or on hardware, is gamescope's `--xwayland-count`. It must first prove that no selection or clipboard bridge joins the servers.
   - Cost if wrong: double compositing, plus input and overlay issues measured in Wave 2.
8. **Every sign-in on the OS happens in a sign-in window.**
   - The window is Firefox in bastion's browser sandbox, shown on a display of its own. The launcher watches its navigation over WebDriver BiDi and takes the final address: the loopback redirect with the code, Spotify's callback, or the `ms-xal-0000000048183522://` address of Bedrock's Xbox sign-in.
   - Only the hosts a sign-in needs are reachable.
   - Microsoft, Spotify and Xbox sign-ins use a fresh profile deleted afterwards. Discord uses the phone's profile, as `browser login` does today.
   - On desktops nothing changes.
   - Cost if wrong: on the OS, `xdg-open` has nothing to open.
9. **Builds run in VMs.** No mkosi build, and no third-party source build, runs on the development machine. The installer and the update service run mkosi as root inside the machine they build for: in tests, that is a QEMU guest.
10. **systemd-sysupdate is used through its command line only.** systemd 262 marks it experimental again and removes the sysupdated D-Bus API in 263. The transfer definitions follow ParticleOS's.
11. **Three recipe profiles.**
    - `full` (the default) includes Sunshine and mcpelauncher.
    - `minimal` omits them, for faster VM iterations.
    - `test` is `minimal` plus a root shell on `ttyS0` and a way to run a command as the player inside the active session. Those hooks exist only in `test`, and a static test proves it.
    - The owner's machine always gets `full`.
12. **The player's bastion-mc config is written at install** from `packaging/os/player-config.toml` (C1). That makes the age store, the TPM identity, clips and PipeWire audio the defaults on the OS.
13. **The config manager covers bastion-mc's own files: `config.toml` and each instance's `instance.toml`.**
    - Every key is described in one schema in the launcher: its type, default, allowed values, a one-sentence explanation, whether it lowers protection, and whether Bastion OS fixes it. A test fails when the schema and the config structs disagree.
    - Writes go only through the launcher's writer: comments are kept, the real parser validates the result, and the replace is atomic.
    - A setting that lowers protection needs an explicit confirmation that names what it lowers.
    - On Bastion OS, keys the OS fixes (the game's display, for example) are read-only, with the reason shown.
    - Keys that belong to a command (`auth.token_store` belongs to `token-store`, `network.servers` to `server`) point to that command.
    - The game's own files (`options.txt`, mod configs) are not in the first version: the game writes them, so they are untrusted input.
    - Cost if wrong: a later version adds game files, behind the same untrusted-input rules.
14. **Phone apps hold no secrets and reach the network only through the launcher.**
    - The owner chose four apps on 2026-11-16:
      - a minecraft.wiki app, Waypoints and a Clock (P6);
      - a screenshot manager (P5), both in the phone and in the front end, because it sends through P5's LocalSend sender.
    - Anything an app shows from the network comes from a launcher service, as Spotify's and Realms' do.
    - Data the game sends to the launcher is untrusted: lengths are limited and every value is validated.
    - Cost if wrong: an app that needs more is dropped from the first version and recorded.

## The contract

Every package uses these names exactly. A package that must change one records a `Ruling:` and lists it in its report, and Wave 2 reconciles.

### C1 — Accounts, paths, units, files

**Player account**
- uid 1000; primary group `bastion-player`, gid 1000; login name chosen at install (default `player`).
- Home `/home/<name>`; shell `/usr/bin/nologin`; password locked.
- Supplementary group `tss`, for `/dev/tpmrm0` (age-plugin-tpm).
- Root's password is locked. No sudo.

**Sunshine account**
- System user and group `bastion-sunshine` (dynamic uid, no login).
- `/dev/uinput` and `/dev/uhid` are `0660 root:bastion-sunshine`.

**Root helper**
- `/usr/lib/bastion-os/bastion-os`, built from `crates/os` (P2).
- Installed only by the image, never by the Arch PKGBUILD.

**OS identity**
- `/usr/lib/os-release` holds `ID=bastion-os`, `ID_LIKE=arch`, `IMAGE_ID=bastion-os`, `IMAGE_VERSION=<YYYYMMDD>.<n>`, `BASTION_COMMIT=<40 hex>`.
- It is root-owned, in the verity-protected `/usr`.

**Components built from source**
- `/usr/lib/bastion-os/components.json`:

  ```json
  {"mcpelauncher-client": {"version": "…", "sources_sha256": ["…"]}, "mcpelauncher-ui": {…}, "sunshine": {…}}
  ```

**Status files**
- Written by the root helper (P2) in `/run/bastion-os/`, owned by root with mode 0644, replaced atomically.
- Read by the launcher (P3, P5).
- `os.json`:

  ```json
  {"image_version":"20261115.1","commit":"<sha>","slot":"a","other_slot_version":"20261110.3","secure_boot":true,"tpm":"bound"}
  ```

  `other_slot_version` may be `null`. `tpm` is `"bound"` or `"rebind-needed"`.
- `update.json`:

  ```json
  {"state":"idle","commit":null,"step":0,"steps":5,"message":null,"finished_at":null}
  ```

  `state` is one of `idle`, `checking`, `fetching`, `building`, `installing`, `ready`, `failed`.
- `remote-play.json`:

  ```json
  {"on":false,"clients":[]}
  ```

  `clients` holds the device names.
- `openings.json`:

  ```json
  {"localsend_until":null}
  ```

  When an opening is active, `localsend_until` holds its end in unix seconds.

**Units**
- Session and boot:
  - `bastion-session.service`: gamescope on tty1 as the player.
  - `bastion-session-good.service`: the health check `RequiredBy=boot-complete.target`.
  - `bastion-os-firewall.service`.
  - `bastion-os-status.service`.
  - `bastion-os-firstboot-bind.service`: binds PCR 7 once, after the keys are enrolled at first boot. It runs `bastion-os rebind` while `/var/lib/bastion-os/tpm-pcr7` is missing.
  - `bastion-os-keyring-cleanup.service` and `.timer`.
- Started by the player (through polkit):
  - `bastion-os-update.service`
  - `bastion-os-rebind.service`
  - `bastion-os-open-localsend.service`
  - `bastion-os-remote-play-on.service`
  - `bastion-os-remote-play-off.service`
  - `bastion-os-remote-play-pin@<4 digits>.service`
  - `bastion-os-token-store@age.service`
  - `bastion-os-token-store@keyutils.service`
- Sunshine itself: `bastion-sunshine.service`, disabled; started only by `remote-play-on`.

**polkit**
- `/usr/share/polkit-1/rules.d/50-bastion-os.rules`.
- Members of `bastion-player` may use the verb `start` on exactly the units listed above. The PIN template accepts only `^[0-9]{4}$` instances, and the token-store template only `age` and `keyutils`.
- They may also use NetworkManager's `network-control`, `wifi.scan`, `enable-disable-wifi`, `settings.modify.own` and `settings.modify.system`.
- logind's default power-off and reboot for the active local session stay as they are.
- Everything else is denied (there is no administrator).

**Health check**
- The front end writes `/run/user/1000/bastion-mc-gui/ready` once its first frame is shown, when `BASTION_OS_SESSION=1` (P3).
- `bastion-session-good.service` waits up to 180 s for that file (P2).

**Firewall**
- Table `inet bastion` in `/usr/lib/bastion-os/firewall.nft`.
- Set `localsend`: `type inet_service; flags timeout`. An opening adds `53317 timeout 120s`.
- Set `remote_play`: `type inet_service; flags interval`. Filled with 47984, 47989, 48010 and 47998-48000 while remote play is on. 47990 is never in it.

**The player's config, written at install (P1)**

```toml
[auth]
token_store = "age"
[auth.age]
program = "age"
recipients_file = "/etc/bastion-mc/age-recipients"
identity_file = "/home/<name>/.local/share/bastion-mc/vault/tpm-identity.txt"
[sandbox]
audio = "pulse"
[clips]
enabled = true
```

- The identity is made at install with `age-plugin-tpm --generate` (no PIN).
- Its recipient goes into the root-owned `/etc/bastion-mc/age-recipients`.

### C2 — New launcher commands (all available with `--events`)

**Foundation (F2)**
- `config show`: emits `Event::Settings(Settings)`.
- `config set <key> <value>`. The keys:
  - `auth.client_id` (a UUID, or `none`)
  - `auth.token_store` (`age`, `keyutils` or `none`)
  - `spotify.client_id` (an ID, or `none`)
  - `clips.enabled` (`true` or `false`)
  - `realms.approve` (`off`, `code` or `totp`)
  - `bedrock.accept_licence_substitution` (`true` or `false`)

**System pages (P3)**
- `os status`: emits `Event::OsStatus`.
- `os update`: emits `Event::OsUpdate` repeatedly until `ready` or `failed`.
- `os rebind`.
- `os power off|restart`.
- `os remote-play on|off|status`: emits `Event::RemotePlay`.
- `os remote-play pin`: the PIN comes as `answer pin NNNN`.
- `network status|scan`: emits `Event::Network`.
- `network connect <ssid> [--enterprise --identity <id> --domain <suffix>]`: the secret comes as `answer wifi …`.
- `network forget <uuid>`.
- `network real-address <uuid> on|off`.
- `config reset`: Bastion OS only. It keeps the broken file as `config.toml.broken-<time>` and restores the default from `/usr/share/bastion-os/player-config.toml`.

**Setup pages (P4)**
- `server list|add <host[:port]> [--voice P]… [--udp P]… [--allow-private]|remove <host>`: emits `Event::Settings`.
- `token-store age|keyutils`.
- `profile new|add|check|lock|audit|sign|verify|list` with `--events`: emits `Event::Profiles` and `Progress`. `profile sign <id>` without `--key` uses the launcher's own key (P4).
- `spotify login|logout|status`, `browser login <site>` and `realms totp-setup` with `--events`.

**Clips and Bedrock (P5)**
- `clips list`: emits `Event::Clips`.
- `clips send <stem>…`: emits `Event::Receivers`, then waits for `answer receiver <fingerprint>` (and `answer pin NNNN` if the receiver asks). Then emits `Event::Transfer`.
- `bedrock get`: emits `Event::BedrockSetup`.
- `bedrock import --from-ui <version>`.
- `bedrock remove-google-sign-in`.
- `bedrock signin-window <url>`: Bastion OS only; emits `Event::BedrockSignInDone { address }`. The front end hands the address to the running game with `answer signin`.

**Config manager (P4)**
- `config list [--changed]`: emits `Event::Config(ConfigView)`.
- `config get <key>`.
- `config set <key> <value> [--confirm]`: covers every key in the schema. F2's narrower `config set` becomes a special case of it.
- `config unset <key>`: the key returns to its default, and the file's comments are kept.
- `config check`: the parser's message and line, or `ok`.
- `instance config <id> list|get|set|unset …`.

**Phone apps (P5, P6)** add verbs to the phone's channel, not launcher commands. Each plan names its verbs.

**Rules for every new command**
- On a system that is not Bastion OS, a command that needs the OS ends with `Event::Error` and the message `this needs Bastion OS`.
- Secrets never appear in argv. They come over the control pipe.

### C3 — Event protocol additions

- New events need no `PROTOCOL` bump (crates/proto/src/lib.rs:23-24). `PROTOCOL` stays 1, and nothing existing changes meaning.
- Each package puts its types in its own module file and adds its variants to `Event` in `lib.rs`. Wave 2 resolves the adjacent-line conflicts.
- F2 extends the answer kinds (`Answerable` and the control-pipe parser) by `wifi`, `pin` and `receiver` up front, so that P3 and P5 do not both edit them.
- F2 adds `Event::SignInWindow { purpose: String, open: bool }` and `Event::Settings(Settings)` (in `setup.rs`).
- P4 adds `Event::Config(ConfigView)` (in `setup.rs`) for the config manager.

### C4 — Foundation interfaces (F2)

F2 records the final signatures in `docs/notes/os-foundation.md`; Wave 1 reads them there. In outline:

- `bastion_mc::os::detect() -> Option<OsInfo>` and `detect_in(root: &Path)`. `OsInfo { image_version: String, commit: Option<String> }`. `os::RUN_DIR = "/run/bastion-os"`.
- `bastion_mc::config_edit::{Setting, apply(config_file: &Path, settings: &[Setting]) -> Result<()>}`:
  - format-preserving (`toml_edit`);
  - validated by the existing parser before the file is replaced;
  - atomic, mode 0600, with the same ownership checks as reading.
- `OwnDisplay::start(cfg, Purpose::{Game, SignIn, BedrockUi}, size) -> Result<OwnDisplay>`. `OwnDisplay::attach(&mut <bwrap spec>)` gives a sandboxed client only that display. `OwnDisplay::stop()`.
- `browser::signin::open(cfg, reporter, SignInWindow { purpose, start_url, allowed_hosts, finish: Finish::{Redirect{prefix}, Scheme{scheme}, SiteLogin{site}}, profile: Profile::{Fresh, Phone}, timeout }) -> Result<Option<String>>`. It returns the final address, or `None` if the player closed the window.
- `browser::signin::show_page(cfg, reporter, purpose, html, timeout) -> Result<()>`: no network.
- On Bastion OS, `msa::interactive_login`'s browser flow uses `signin::open` with the loopback prefix instead of emitting `BrowserLogin` (F2 does this one, as the mechanism's first user).

### C5 — VM harness (F1)

F1 records the final API in `docs/notes/os-vm-harness.md`. In outline:

- `vmtest::VmConfig` fields:
  - ISO or direct kernel boot
  - disks
  - `tpm: bool`
  - `secure_boot: SetupMode | Vars(path) | Off`
  - memory and CPUs
  - 9p shares (read-only or read-write)
  - user-mode net with host forwards
- `Vm::start(cfg)`.
- `vm.serial().expect(regex, timeout)` and `vm.serial().run(cmd, timeout) -> (status, output)`.
- `vm.screendump(png)`, `vm.send_keys(&[…])`, `vm.reboot()`, `vm.wait_poweroff(timeout)`.
- `vmtest::arch_iso() -> PathBuf`: the pinned, verified ISO.
- `vmtest::os_tests_enabled() -> bool`: prints the skip line.
- Each test keeps its artifacts in `target/vm/<test>/`: `serial.log`, screenshots, disks, OVMF vars, TPM state.

## Order

1. **Wave 0:** F1 and F2 in parallel, from `main`. Then one merge step (`target/agents/os-base`). The two touch disjoint files. The step runs fmt, clippy, the workspace tests and F1's smoke VM test.
2. **Wave 1:** P1–P6 in parallel, each from `os-base`. VM tests that need an installed Bastion OS are written and run in P1. In P2 and P5, those tests are written against C5 and the installed image, and run in Wave 2. Their ISO-only VM tests run in the package.
3. **Wave 2:** one integration agent in `target/agents/os-int`, then the documents pass (tasks below).
4. **Then:** a fresh whole-branch review on the most capable model, one fix pass, and the replay onto `main` (the owner pushes).

## Wave 2: integration and documents

### Task W2.1: Merge in order P2, P1, P3, P4, P5, P6

- [ ] Merge in that order, resolving `Event` variants, workspace members, `sources.toml` and `mkosi.extra/` conflicts by keeping both sides.
- [ ] After each merge, run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Expected: 0 failed.
- [ ] Reconcile every `Ruling:` from the reports against this contract. Each changed name is changed everywhere, or reverted.

### Task W2.2: The whole flow in VMs (full profile)

- [ ] Build the `full` image through the installer in a VM and run every VM test of every package. That includes the P2 and P5 tests that needed an installed OS:
  - `vm_the_session_starts_the_front_end_full_screen`
  - `vm_the_game_gets_a_display_of_its_own`
  - `vm_the_player_can_start_only_the_listed_units`
  - `vm_remote_play_is_off_by_default_and_its_ports_are_closed`
  - `vm_after_a_pcr7_change_the_rebind_makes_the_next_boot_unlock_by_tpm`
  - `vm_the_java_token_survives_a_restart_through_age_and_the_tpm`
  - `vm_the_image_has_mcpelauncher_built_from_pinned_sources`
  - `vm_an_update_started_from_the_front_end_installs_into_slot_b`
  - `vm_nothing_listens_on_the_network_by_default`
- [ ] Expected: all pass. If gamescope cannot drive QEMU's virtual GPU, the session tests record why, and assert the session's units and gamescope's log instead. That limitation goes into the report and docs/OS.md.

### Task W2.3: Documents

- [ ] docs/OS.md, the user guide:
  - install from the ISO, and the confirmation;
  - first boot and the key enrolment warning;
  - keeping the recovery key;
  - networks: college Wi-Fi with the MAC-address method, iPhone hotspots over Wi-Fi and USB ("Trust this computer"), captive portals not supported;
  - sending clips; remote play and what it means; updates and the restart;
  - repair through the maintenance entry; what to do after a firmware update;
  - the accepted risk (trusting GitHub).
- [ ] docs/THREAT_MODEL.md:
  - an OS layer section;
  - rows for each Decision above;
  - the LocalSend LAN-spoofing advisory (GHSA-424h-5f6m-x63f) and how the fingerprint pin answers it;
  - Sunshine's capability;
  - the volume key in root's keyring;
  - sysupdate's experimental status.
- [ ] README: Status rows; the new commands; the test table rows (`BASTION_OS_TESTS`); the counts.
- [ ] TODO: the owner-only checks below, under "Blocked on you".
- [ ] The website: an OS page with REQUIRE ties.
- [ ] The config manager and the phone apps: README sections, mentions in docs/OS.md, and the website's phone page.
- [ ] The site check passes, and so does the GUI suite with only Adwaita fonts.
- [ ] Commit.

## Review Focus

The five failure modes the spec implies but no single package's tests cover, most likely first. The test that pins each is named, with the task that owns it.

1. **A firmware that is not in setup mode, or an enrolment that would leave no picture.**
   - The installer refuses before erasing anything: P1 Task 5 `outside_setup_mode_the_installer_explains_how_to_reset_the_keys`.
   - db holds the three Microsoft certificates after first boot: P1 Task 7 `vm_first_boot_enrols_the_keys_and_secure_boot_is_on`.
2. **An update interrupted by power loss, a full disk or a lost network.**
   - Slot A stays untouched and bootable: P1 Task 8 `vm_an_interrupted_update_leaves_slot_a_bootable`.
   - The status says `failed` with the reason and a retry works: P2 Task 8 `a_failed_build_leaves_the_status_failed_and_installs_nothing`.
3. **A front end that exits on every boot, for example because of a broken config.**
   - It must not crash-loop the machine into an old slot that has the same config. The front end shows an error page instead of exiting: P3 Task 2 `a_broken_config_shows_an_error_page_instead_of_exiting`.
   - The health check means "the front end showed its first frame": P2 Task 5 `session_good_waits_for_the_ready_marker_and_times_out`.
4. **A TPM that was cleared, or a changed PCR 7.**
   - The recovery key unlocks, re-binding works, and a vault the TPM can no longer open says "sign in again" instead of failing: F2 Task 2 `a_vault_the_tpm_can_no_longer_open_asks_to_sign_in_again`, and W2.2 `vm_after_a_pcr7_change_the_rebind_makes_the_next_boot_unlock_by_tpm`.
5. **The game's display.**
   - It is never the front end's: F2 Task 3 `on_bastion_os_wayland_session_sockets_are_never_handed_to_the_game`.
   - In the VM the game's window is shown and gets input: W2.2 `vm_the_game_gets_a_display_of_its_own`.

## Owner-only checks (no package can do these)

- Install on real hardware: the firmware's setup mode, the key enrolment, option ROMs with the Microsoft certificates, a discrete GPU's picture.
- gamescope playability on the owner's GPU; Sunshine's KMS capture to a real Moonlight client.
- A real Wi-Fi network (WPA2/WPA3, eduroam-style 802.1X, a college MAC registration) and a real iPhone hotspot.
- The Microsoft sign-in itself, which needs an Azure client ID: the owner's own app, or bastion-mc's public app once registered and approved by Mojang.
- Google Play's Minecraft download through mcpelauncher-ui, with the owner's Google account.
