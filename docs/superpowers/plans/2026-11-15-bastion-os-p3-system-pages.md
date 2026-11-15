# Bastion OS — P3: System pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The front end can run the machine without a terminal:
- updates and their progress;
- shut down and restart;
- re-binding the TPM, and the warnings that need it;
- the network page (Wi-Fi including college networks, iPhone hotspots, Ethernet);
- remote play on and off, with pairing.

The front end is also a good session client: it says when its first frame is up, and it never exits on a broken config.

**Architecture:**
- **New launcher commands** (`os …`, `network …`, `config reset`):
  - `os …` starts the polkit-allowed units (C1), reads the status files in `/run/bastion-os/`, and emits new events.
  - `network …` drives NetworkManager through `nmcli` with fixed argument vectors. Secrets go over the control pipe and reach nmcli through stdin, never through argv.
- **The front end** gains a System page, a Network page and a Remote play page, shown only on Bastion OS. It follows its existing rules: one job at a time, `App::update` as the only place state changes, no D-Bus client, and no credential beyond Decision 5's Wi-Fi secret and pairing PIN.

**Tech Stack:** Rust; iced 0.14 (the existing front end); the existing event protocol; `/usr/bin/systemctl` and `/usr/bin/nmcli`.

**Spec:** `docs/superpowers/specs/2026-11-14-bastion-os-design.md`: G5, G8; "The session" (the front end's controls); "Network"; "Remote play (Sunshine)"; "Updates"; "Repair". Read the index first, in particular Decision 5, C1–C3 and the F2 notes (`docs/notes/os-foundation.md`: `os::detect`, `RUN_DIR`, the answer kinds). Consumes P2's units and status files through C1, not P2's code.

## Global Constraints

The index's Global Constraints apply. In addition:
- **Bastion OS only:**
  - every `os` and `network` command and `config reset` ends with `Event::Error` and `this needs Bastion OS` elsewhere;
  - the three new pages appear only after an `OsStatus` event has arrived.
- **No secret in argv or logs:**
  - the Wi-Fi password and the 802.1X identity's password never appear in an argument vector, an environment variable or a log line;
  - the front end clears the field after sending it once, and never shows it on the Console page.
- **Layout:** every new view is laid out at the front end's four test window sizes and checked with only Adwaita fonts, as the existing view tests are.
- **Text:** user-facing sentences are plain and complete, in the project's voice. They go into `docs/notes/os-p3.md` for the documents pass.

## Review Focus

1. **SSIDs with colons, backslashes, emoji or bytes that are not UTF-8, and hidden networks.** Pinned by Task 3 `ssids_with_colons_backslashes_and_non_utf8_bytes_round_trip`.
2. **NetworkManager not running.** Pinned by Task 3 `network_commands_report_networkmanager_missing`.
3. **An update still running while the front end restarts.** Pinned by Task 1 `follow_resumes_from_the_current_state`.
4. **Shut down while a game runs.** Pinned by Task 2 `power_off_during_a_game_asks_first`.
5. **The Wi-Fi secret leaking onto the Console page.** Pinned by Task 4 `the_password_never_reaches_the_console_page`.

---

### Task 1: The launcher's `os` commands

**Files:**
- Create: `src/os/commands.rs`, `crates/proto/src/os.rs`
- Modify: `src/os/mod.rs` (`pub mod commands;`); `src/cli.rs` (dispatch, HELP, the `--events` allow-list); `crates/proto/src/lib.rs` (`pub mod os;` and the variants)
- Test: unit tests with an injectable runner and status directory; proto round-trip tests

**Interfaces:**
- Consumes: C1's units and status files; F2's `os::detect`, `RUN_DIR`, and `Answerable { kind: pin }`.
- Produces:
  - `proto::os::OsStatus` with fields:
    - `image_version: String`
    - `commit: Option<String>`
    - `slot: String`
    - `other_slot_version: Option<String>`
    - `secure_boot: bool`
    - `tpm: TpmState` (`Bound` or `RebindNeeded`)
    - `update: OsUpdate`
  - `proto::os::OsUpdate` with fields:
    - `state: UpdateState` (`Idle`, `Checking`, `Fetching`, `Building`, `Installing`, `Ready` or `Failed`)
    - `step: u32`, `steps: u32`
    - `message: Option<String>`
    - `commit: Option<String>`
  - `proto::os::RemotePlay { on: bool, clients: Vec<String> }`
  - `Event::OsStatus(OsStatus)`, `Event::OsUpdate(OsUpdate)` and `Event::RemotePlay(RemotePlay)`.
  - The commands:
    - `os status`
    - `os update [--follow]` (start, or only follow; it ends at `ready`, `failed`, or `idle` after a check)
    - `os rebind`
    - `os power off|restart`
    - `os remote-play on|off|status`
    - `os remote-play pin`: emits `Answerable { kind: pin }`, then starts `bastion-os-remote-play-pin@NNNN.service`.

- [ ] **Step 1: Write the failing tests:**
  - `os_status_reports_the_slots_versions_secure_boot_and_tpm`
  - `an_update_is_followed_until_ready`
  - `a_failed_update_reports_its_message`
  - `follow_resumes_from_the_current_state`
  - `os_commands_are_refused_outside_bastion_os`
  - `power_off_runs_systemctl_poweroff`
  - `rebind_starts_the_unit_and_reports_the_new_state`
  - `the_pin_comes_from_the_control_pipe_and_starts_the_pin_unit`
  - `os_events_round_trip` (proto)
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib os::commands && cargo test -p bastion-mc-proto`. Expected: FAIL.
- [ ] **Step 3: Implement.** `systemctl start --no-block <unit>` uses the absolute path. Follow the status files by polling once a second, or by inotify. Read the files with `O_NOFOLLOW` and check that they are root-owned.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `launcher: os status, update, rebind, power and remote play, through the Bastion OS units`

### Task 2: The front end as the session's client, and the System page

**Files:**
- Create: `gui/src/session.rs` (the ready marker); `gui/src/view/system.rs`
- Modify:
  - `gui/src/app.rs`: the pages, the nav rail, the startup `os status`, and the error page for a failed start;
  - `gui/src/backend.rs`: requests for the `os` commands and `config reset`;
  - `gui/src/view/mod.rs`;
  - `src/cli.rs` and a new `src/os/reset.rs`: `config reset`, Bastion OS only. It moves `config.toml` to `config.toml.broken-<unix time>`, then copies `/usr/share/bastion-os/player-config.toml` with `<name>` replaced by the player's home.
- Test: `gui/src/view/tests.rs` (view tests at four sizes); unit tests for `session.rs` and `reset.rs`

**Interfaces:**
- Consumes: Task 1's events; C1's ready-marker path.
- Produces:
  - **The ready marker:** with `BASTION_OS_SESSION=1`, the front end creates `$XDG_RUNTIME_DIR/bastion-mc-gui/ready` (directory 0700) once its first frame has been drawn.
  - **The error page:** when the startup `status` fails, it shows the launcher's message, "Try again", and on Bastion OS "Restore the default settings". The front end never exits on its own.
  - **The System page:**
    - version, short commit and slot;
    - "Check for updates", with the progress in plain words (one sentence per state) and "Restart now" once ready;
    - "Shut down" and "Restart", which ask first while a game runs;
    - a banner for `RebindNeeded`, with a re-bind button;
    - a red banner when `secure_boot` is false.

- [ ] **Step 1: Write the failing tests:**
  - `the_ready_marker_is_written_once_in_a_bastion_os_session`
  - `no_marker_outside_a_bastion_os_session`
  - `a_broken_config_shows_an_error_page_instead_of_exiting`
  - `config_reset_keeps_the_broken_file_and_restores_the_default`
  - `config_reset_is_refused_outside_bastion_os`
  - `the_system_page_shows_the_update_progress`
  - `restart_now_appears_only_when_an_update_is_ready`
  - `the_rebind_banner_appears_when_the_tpm_needs_it`
  - `a_secure_boot_off_banner_is_shown`
  - `power_off_during_a_game_asks_first`
  - `the_system_page_is_absent_outside_bastion_os`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-mc-gui --lib && cargo test --lib os::reset`. Expected: FAIL.
- [ ] **Step 3: Implement.** Detect the first frame with iced's frame subscription, its first tick after the window opens. Record the mechanism.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, then the GUI suite with only Adwaita fonts as the README describes. Expected: PASS.
- [ ] **Step 5: Commit.** `front end: the session's ready marker, an error page instead of exiting, and the System page`

### Task 3: The launcher's `network` commands

**Files:**
- Create: `src/os/network.rs`; fixtures under `tests/fixtures/nmcli/`, synthetic only, never copied from a real machine
- Modify: `crates/proto/src/os.rs` (the network types); `src/cli.rs`
- Test: unit tests with a fake `nmcli` that records argv and stdin and replays a fixture

**Interfaces:**
- Produces:
  - `proto::os::Network` with fields:
    - `networking: bool`
    - `connectivity: Connectivity` (`Full`, `Limited`, `Portal`, `None` or `Unknown`)
    - `devices: Vec<NetDevice>`
    - `wifi: Vec<WifiNetwork>`
    - `connections: Vec<NetConnection>`
  - `NetDevice` fields: `name`, `kind`, `state`, `address: Option<String>`, `permanent_address: Option<String>`, `iphone: bool`.
  - `WifiNetwork` fields: `ssid: String`, `ssid_hex: String`, `signal: u8`, `security` (`open`, `wpa2`, `wpa3` or `enterprise`), `in_use: bool`, `known: bool`.
  - `NetConnection` fields: `uuid`, `name`, `kind`, `real_address: bool`, `autoconnect: bool`.
  - `Event::Network(Network)`.
  - The commands:
    - `network status`
    - `network scan`
    - `network connect <ssid> [--hidden] [--enterprise --identity <id> --domain <suffix>]`: emits `Answerable { kind: wifi }` for a secured network.
    - `network forget <uuid>`
    - `network real-address <uuid> on|off`: `802-11-wireless.cloned-mac-address permanent`, or back to `stable`.
  - Rules:
    - every nmcli call is `/usr/bin/nmcli -t -e yes …` with `LC_ALL=C`;
    - a profile is created without its secret, and the secret is given with `connection up … passwd-file /dev/stdin`;
    - enterprise means PEAP or TTLS with MSCHAPv2, `802-1x.domain-suffix-match` (required) and `802-1x.system-ca-certs yes`;
    - the connectivity comes from `nmcli networking connectivity check`;
    - an iPhone is a device whose driver is `ipheth`.

- [ ] **Step 1: Write the failing tests:**
  - `terse_output_with_escaped_colons_and_backslashes_is_parsed`
  - `ssids_with_colons_backslashes_and_non_utf8_bytes_round_trip`
  - `a_wifi_password_never_appears_in_an_argument`
  - `connect_with_802_1x_requires_a_domain_suffix`
  - `a_hidden_network_is_connected_with_hidden_yes`
  - `use_the_real_address_sets_cloned_mac_permanent`
  - `a_captive_portal_is_reported_and_not_retried`
  - `network_commands_report_networkmanager_missing`
  - `an_iphone_over_usb_is_recognised`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib os::network`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `launcher: the network commands, through nmcli, with secrets only on stdin`

### Task 4: The Network page

**Files:**
- Create: `gui/src/view/network.rs`
- Modify: `gui/src/app.rs`, `gui/src/backend.rs`, `gui/src/view/mod.rs`
- Test: `gui/src/view/tests.rs`

**Interfaces:**
- Consumes: Task 3.
- Produces the Network page:
  - Wi-Fi networks with signal, security and known state;
  - Ethernet and iPhone states;
  - "Connect", with a masked password field. Enterprise networks also get the identity and the domain, with a sentence on where the domain comes from.
  - "Forget";
  - "This machine's address", which shows the permanent address. It explains the college method: register this address on the college's device-registration page, then turn on "Use this machine's real address on this network".
  - A note that sign-in pages (captive portals) are not supported, with what to do instead.
  - The iPhone hints: "Unlock your iPhone and tap Trust, then turn on Personal Hotspot", and the Wi-Fi hotspot shown as an ordinary network.

- [ ] **Step 1: Write the failing view tests:**
  - `the_network_page_lists_wifi_networks_with_their_security`
  - `the_password_is_sent_once_and_the_field_is_cleared`
  - `the_password_never_reaches_the_console_page`
  - `an_enterprise_network_asks_for_identity_password_and_domain`
  - `the_real_address_section_explains_the_mac_registration`
  - `a_captive_portal_says_it_is_not_supported`
  - `an_iphone_waiting_for_trust_shows_the_hint`
  - each laid out at the four sizes
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-mc-gui --lib network`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, and the Adwaita-only run. Expected: PASS.
- [ ] **Step 5: Commit.** `front end: the Network page, with college Wi-Fi's MAC-address method and iPhone hotspots`

### Task 5: The Remote play page

**Files:**
- Create: `gui/src/view/remote_play.rs`
- Modify: `gui/src/app.rs`, `gui/src/backend.rs`, `gui/src/view/mod.rs`
- Test: `gui/src/view/tests.rs`

**Interfaces:**
- Consumes: Task 1's `os remote-play` and `RemotePlay`.
- Produces the Remote play page:
  - the switch, with the sentence "When remote play is on, a paired device sees this screen and controls this machine.";
  - "Pair a device", with a four-digit field for the PIN Moonlight shows, sent with `answer pin`;
  - the paired or connected devices, in the words Sunshine's reporting allows (P2's notes).

- [ ] **Step 1: Write the failing view tests:**
  - `the_remote_play_switch_says_what_it_means`
  - `the_pin_field_accepts_only_four_digits`
  - `paired_devices_are_listed`
  - each at the four sizes
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-mc-gui --lib remote_play`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `front end: the Remote play page, off by default, with pairing by PIN`

### Task 6: The notes

**Files:**
- Create: `docs/notes/os-p3.md`

- [ ] **Step 1: Write the notes.**
  - The new commands and events, with their final signatures.
  - The first-frame mechanism.
  - Every `Ruling:` this package made.
  - The user-facing text for docs/OS.md: the System page, updates and the restart, the network page, the college Wi-Fi MAC method, iPhone hotspots over Wi-Fi and USB, captive portals, remote play.
- [ ] **Step 2: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Expected: 0 failed.
- [ ] **Step 3: Commit.** `docs: notes on the Bastion OS system pages`
