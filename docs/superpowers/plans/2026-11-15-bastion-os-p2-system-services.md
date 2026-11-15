# Bastion OS — P2: System services Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Everything that runs on Bastion OS as root, or as the session:
- the root helper `bastion-os` and its units;
- the polkit rule that lets the player start exactly those units;
- the firewall with its two timed openings;
- the gamescope session and its health check;
- TPM state and re-binding;
- Sunshine, off by default;
- the update service;
- the token-store switch.

**Architecture:**
- **One small Rust binary,** `crates/os` → `/usr/lib/bastion-os/bastion-os`. It accepts a fixed set of verbs and is run only by systemd units, never by the player directly.
- **The player's side:** the player (group `bastion-player`) may `start` those units through polkit. The launcher does that for the front end (P3, P5) and reads the results from the status files in `/run/bastion-os/` (C1).
- **Image files:** units, rules and configuration live under `packaging/os/mkosi/mkosi.extra/` and in this package's recipe fragments (`mkosi.conf.d/30-sunshine.conf`, `mkosi.conf.d/50-services.conf`).

**Tech Stack:** Rust (std, serde_json, a small blocking HTTPS client such as `ureq` with rustls if `cargo deny` accepts it; no async runtime in the helper), nftables, polkit (JavaScript rules), systemd 262 units, gamescope 3.17, PipeWire, cryptsetup ≥ 2.7, systemd-cryptenroll, Sunshine v2026.914.233613 built from pinned sources.

**Spec:** `docs/superpowers/specs/2026-11-14-bastion-os-design.md`: G5, G7, G8; "The session"; "Updates"; "Repair: the maintenance entry"; "Network: Firewall"; "Sending clips off the machine (LocalSend)": the opening; "Remote play (Sunshine)"; "The Java sign-in's token". Read the index first, in particular Decisions 1, 3 and 4, C1 and C2. This package consumes P1's `build-image.sh` and sysupdate transfers through their contract, and the F1 harness.

## Global Constraints

The index's Global Constraints apply. In addition:
- **The helper takes only fixed input:**
  - verbs: `status`, `session-good`, `update`, `rebind`, `open-localsend`, `remote-play on|off|status|pin <NNNN>`, `token-store age|keyutils`;
  - every other word, and any extra argument, is refused with exit 2;
  - it reads no file a non-root user can write, except the ready marker, which it opens with `O_NOFOLLOW` and checks for owner 1000.
- **Status files** follow C1 exactly: root-owned, 0644, written to a temporary file in `/run/bastion-os/` and renamed.
- **Units:**
  - every helper unit has `NoNewPrivileges=yes`, `PrivateTmp=yes` and `ProtectHome=yes`, and the narrowest `ProtectSystem=`, `CapabilityBoundingSet=` and `ReadWritePaths=` its job allows;
  - `bastion-os-update.service` is the documented exception, because mkosi needs loop devices and mounts.
- **VM tests:**
  - those that need an installed Bastion OS are written here against C5 and P1's installed-disk cache (`target/vm/installed-test.raw`) and run in Wave 2;
  - those that only need the ISO run here.

## Review Focus

1. **gamescope's Xwayland may have no X cookie, while the launcher's nested path expects one or xhost.** Pinned by Task 5 `vm_the_game_gets_a_display_of_its_own` (run in Wave 2).
2. **A front end that exits at once on every start.** The session restarts forever with a growing delay and does not stop at a black screen. Pinned by Task 5 `the_session_restarts_forever_with_a_growing_delay`.
3. **Re-binding after the cleanup has already unlinked the volume key.** Pinned by Task 6 `rebind_without_a_linked_key_says_to_restart_with_the_recovery_key`.
4. **An update on a nearly full state partition.** Pinned by Task 8 `an_update_without_enough_free_space_fails_before_building`.
5. **GitHub's API unreachable, rate-limited, or reporting no successful run.** Pinned by Task 8 `an_api_error_leaves_the_machine_as_it_is`.

---

### Task 1: The helper crate

**Files:**
- Create: `crates/os/{Cargo.toml,src/main.rs,src/lib.rs,src/verbs.rs,src/status.rs}`
- Modify: the root `Cargo.toml` (the workspace member `crates/os`)

**Interfaces:**
- Produces:
  - `bastion_os::verbs::parse(args: &[String]) -> Result<Verb, Refusal>`
  - `pub enum Verb`, whose variants are:
    - `Status`
    - `SessionGood`
    - `Update`
    - `Rebind { first_boot: bool }`
    - `OpenLocalsend`
    - `RemotePlay(On | Off | Status | Pin(u16))`
    - `TokenStore(Age | Keyutils)`
  - `bastion_os::status::write_json(dir: &Path, name: &str, value: &impl Serialize) -> io::Result<()>`
  - the C1 structs `OsJson`, `UpdateJson`, `RemotePlayJson`, `OpeningsJson`, with serde field names exactly as in C1.

- [ ] **Step 1: Write the failing tests:**
  - `only_the_listed_verbs_are_accepted`;
  - `extra_arguments_are_refused`;
  - `a_pin_must_be_exactly_four_digits`;
  - `status_files_are_replaced_atomically_and_are_world_readable`;
  - `the_status_json_field_names_match_the_contract`, comparing serialised output with C1's examples.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-os`. Expected: FAIL.
- [ ] **Step 3: Implement.** On a refusal, print one line to stderr and exit 2.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test -p bastion-os && cargo deny check`. Expected: PASS.
- [ ] **Step 5: Commit.** `os helper: a root helper that accepts only its fixed verbs and writes the status files`

### Task 2: The firewall and its openings

**Files:**
- Create:
  - `packaging/os/mkosi/mkosi.extra/usr/lib/bastion-os/firewall.nft`
  - `…/usr/lib/systemd/system/bastion-os-firewall.service`
  - `…/bastion-os-open-localsend.service`
  - `crates/os/src/firewall.rs`
- Test: unit tests; `tests/os_services.rs`

**Interfaces:**
- Produces the table `inet bastion`:
  - **Input:** policy drop. Accepted: established/related, loopback, DHCP (v4, v6), ICMPv6 neighbour discovery, plus:
    - `udp dport @localsend` from the local networks or the multicast group `224.0.0.167`;
    - `tcp dport @localsend` from the local networks;
    - `@remote_play` from the local networks.
  - **Output:** policy drop. Accepted: `meta skuid { 0, 1000 }` and the `systemd-resolve` and `systemd-timesync` users.
  - "Local networks" means:
    - IPv4: 10/8, 172.16/12, 192.168/16, 169.254/16;
    - IPv6: fe80::/10, fc00::/7.
  - `open-localsend` adds `53317 timeout 120s` to `localsend` and writes `openings.json`.

- [ ] **Step 1: Write the failing tests:**
  - `the_firewall_policy_parses`: `nft -c -f` inside `unshare -rn`. Skipped if nft or user namespaces are missing.
  - `inbound_accepts_only_replies_dhcp_and_the_openings`.
  - `outbound_is_limited_to_the_player_root_resolved_and_timesyncd`.
  - `localsend_opening_adds_port_53317_with_a_two_minute_timeout`.
  - `remote_play_never_opens_47990`.
  - ISO VM:
    - `vm_the_firewall_drops_traffic_from_another_account`: in the ISO VM, load the policy and create uid 1000 and uid 1001. `curl` to the host's forwarded port works for 1000 and fails for 1001.
    - `vm_the_localsend_opening_closes_by_itself`: open, see the element, and after 125 s see it gone.
  - Wave 2, installed OS: `vm_nothing_listens_on_the_network_by_default`. `ss -tulpn` shows no socket bound beyond loopback while remote play is off and no opening is active.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-os firewall && cargo test --test os_services`, then `BASTION_OS_TESTS=1 cargo test --test os_services vm_the_firewall vm_the_localsend -- --test-threads=1`. Expected: FAIL.
- [ ] **Step 3: Implement.** `bastion-os-firewall.service` loads the file early (`Before=network-pre.target`, `Wants=network-pre.target`). The opening runs `nft add element …` with a fixed argument vector.
- [ ] **Step 4: Run them; expect them to pass.** Run the same commands. Expected: PASS.
- [ ] **Step 5: Commit.** `os: the firewall (only the player, root, resolved and timesyncd go out) and the LocalSend opening that closes by itself`

### Task 3: The polkit rule

**Files:**
- Create: `packaging/os/mkosi/mkosi.extra/usr/share/polkit-1/rules.d/50-bastion-os.rules`
- Test: `tests/os_services.rs`

**Interfaces:**
- Produces the C1 rule. Requests are handled only when the subject is in `bastion-player` and is local and active:
  - `org.freedesktop.systemd1.manage-units` with `verb == "start"` and a unit from the C1 list, or a unit matching `^bastion-os-remote-play-pin@[0-9]{4}\.service$`, gets `YES`;
  - the NetworkManager actions listed in C1 get `YES`;
  - everything else is `NOT_HANDLED`, so the defaults deny it.

- [ ] **Step 1: Write the failing tests:**
  - `the_polkit_rule_allows_start_only_on_the_listed_units`: reads the rule's unit list and compares it with C1. The test checks that `verb == "start"` is present, and that no `stop`, `restart` or `enable` appears.
  - `the_pin_template_accepts_only_four_digits`: the regex taken from the file, against `1234` (accepted), and `12345`, `abcd` and `1234;x` (refused).
  - `vm_the_player_can_start_only_the_listed_units` (Wave 2, installed OS): as the player inside the session, `systemctl start bastion-os-open-localsend.service` succeeds, while `systemctl start sshd.service`, `systemctl stop bastion-os-firewall.service` and `systemctl start bastion-os-remote-play-pin@12345.service` are refused.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_services polkit`. Expected: FAIL.
- [ ] **Step 3: Implement the rule.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: the polkit rule — the player may start exactly the Bastion OS units`

### Task 4: The units and their hardening

**Files:**
- Create: every C1 unit not made by another task, under `…/usr/lib/systemd/system/`:
  - `bastion-os-status.service`
  - `bastion-os-update.service`
  - `bastion-os-rebind.service`
  - `bastion-os-firstboot-bind.service`
  - `bastion-os-keyring-cleanup.{service,timer}`
  - `bastion-os-token-store@.service`
  - `bastion-os-remote-play-{on,off}.service`
  - `bastion-os-remote-play-pin@.service`

  Also `mkosi.conf.d/50-services.conf`, with the presets that enable the firewall, status, firstboot-bind, keyring-cleanup and session units.
- Test: `tests/os_services.rs`

**Interfaces:**
- Produces units whose `ExecStart=` is `/usr/lib/bastion-os/bastion-os <verb>`, with the instance (`%i`) as the only variable part. `firstboot-bind` has `ConditionPathExists=!/var/lib/bastion-os/tpm-pcr7` and runs `rebind` with the first-boot flag.

- [ ] **Step 1: Write the failing tests:**
  - `every_unit_passes_systemd_analyze_verify`: a temporary root with stub executables. Skipped without `systemd-analyze`.
  - `every_helper_unit_runs_with_no_new_privileges_and_private_tmp`.
  - `every_unit_runs_the_helper_with_a_fixed_verb`.
  - `the_update_unit_is_the_only_one_without_a_strict_file_system_and_says_why`: it has a comment.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_services unit`. Expected: FAIL.
- [ ] **Step 3: Implement the units.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: the Bastion OS units, each running the helper with one fixed verb`

### Task 5: The session and its health check

**Files:**
- Create:
  - `…/usr/lib/systemd/system/{bastion-session.service,bastion-session-good.service}`
  - the PipeWire user presets
  - `…/usr/share/pipewire/pipewire-pulse.conf.d/50-bastion-os.conf`
  - `crates/os/src/session.rs`
- Test: unit tests; `tests/os_services.rs`

**Interfaces:**
- Consumes: C1's ready marker, which P3's front end writes.
- Produces:
  - **`bastion-session.service`:**
    - `User=1000`, `PAMName=login`, `TTYPath=/dev/tty1`, `UtmpIdentifier=tty1`, `Environment=BASTION_OS_SESSION=1`;
    - `ExecStart=/usr/bin/gamescope --backend drm -f -- /usr/bin/bastion-mc-gui` (never `--expose-wayland`);
    - `Restart=always`, `RestartSec=2s`, `RestartSteps=5`, `RestartMaxDelaySec=60s`, `StartLimitIntervalSec=0`;
    - `Conflicts=getty@tty1.service`, `WantedBy=graphical.target`.
  - **`bastion-session-good.service`:** `Type=oneshot`, runs `bastion-os session-good`, `Before=boot-complete.target`, `[Install] RequiredBy=boot-complete.target`, ordered after the session.
  - **`session-good`:** waits up to 180 s for `/run/user/1000/bastion-mc-gui/ready`, which must be a regular file owned by 1000 and opened `O_NOFOLLOW`. It exits 0 when the file appears and 1 on timeout.
  - **pipewire-pulse:** module loading turned off for clients (the launcher's `audio = "pulse"` check needs it), and the capture-only socket for Sunshine (Task 7).
  - **The X display:** the arrangement the launcher's nested path needs for gamescope's Xwayland (a cookie, or the server's own access rule). The builder establishes it in the VM and records it.

- [ ] **Step 1: Write the failing tests:**
  - `session_good_waits_for_the_ready_marker_and_times_out`: a temporary path, with the timeout shortened in the test.
  - `session_good_ignores_a_marker_that_is_a_symlink_or_not_the_players`.
  - `the_session_runs_only_the_front_end_in_gamescope_without_expose_wayland`.
  - `the_session_restarts_forever_with_a_growing_delay`.
  - `pulse_module_loading_is_off_for_clients`.
  - Wave 2, installed OS:
    - `vm_the_session_starts_the_front_end_full_screen`: a screenshot. If gamescope cannot drive QEMU's GPU, the test asserts the units and gamescope's log instead, and the report says so.
    - `vm_the_session_is_restarted_when_the_front_end_exits`.
    - `vm_the_game_gets_a_display_of_its_own`: a stand-in X client started through F2's `OwnDisplay::start(Purpose::Game)` is shown, and cannot connect to the session's display.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-os session && cargo test --test os_services session`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: the gamescope session with only the front end, restarted forever, and its boot health check`

### Task 6: TPM state, re-binding, first-boot binding, keyring cleanup

**Files:**
- Create: `crates/os/src/tpm.rs`; the cleanup timer (Task 4 made the unit)
- Test: unit tests with a fake command runner that records argument vectors; `tests/os_services.rs`

**Interfaces:**
- Produces:
  - **`status`** writes `os.json`. Its fields come from:
    - `secure_boot`: the `SecureBoot` and `SetupMode` efivars;
    - `image_version` and `commit`: os-release;
    - `slot` and `other_slot_version`: `systemd-sysupdate list`, or the partition labels;
    - `tpm`: `"rebind-needed"` when `/sys/class/tpm/tpm0/pcr-sha256/7` differs from `/var/lib/bastion-os/tpm-pcr7`.
  - **`rebind`** follows Decision 3's sequence exactly. The temporary key file goes on a tmpfs under `/run/bastion-os/rebind/`, mode 0600, and is removed in all cases. `--first-boot` does the same and records PCR 7 for the first time.
  - **`keyring-cleanup`** unlinks the volume key when `tpm` is `"bound"`, and otherwise 30 minutes after boot.

- [ ] **Step 1: Write the failing tests:**
  - `rebind_adds_a_temporary_slot_enrols_tpm2_with_pcr_7_and_11_then_wipes_the_temporary_slot`;
  - `rebind_without_a_linked_key_says_to_restart_with_the_recovery_key`;
  - `the_temporary_key_file_is_removed_even_when_enrolment_fails`;
  - `the_tpm_state_is_rebind_needed_when_pcr7_differs_from_the_recorded_value`;
  - `secure_boot_off_is_reported`;
  - Wave 2, installed OS: `vm_after_a_pcr7_change_the_rebind_makes_the_next_boot_unlock_by_tpm`. Change PCR 7 by altering a Secure Boot variable between boots, through the OVMF variables or db. The boot asks for the recovery key, `os.json` says `rebind-needed`, and the rebind unit runs. The next boot unlocks without a prompt.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-os tpm`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: TPM state, re-binding without retyping the recovery key, and the first-boot PCR 7 binding`

### Task 7: Sunshine, off by default

**Files:**
- Create:
  - `packaging/os/mkosi/mkosi.build.d/30-sunshine.chroot`
  - `packaging/os/mkosi/mkosi.conf.d/30-sunshine.conf` (`[Match] Profiles=full`)
  - `packaging/os/sources.toml` entries
  - `…/usr/lib/systemd/system/bastion-sunshine.service`
  - `…/usr/lib/systemd/system/bastion-os-remote-play-watch.service`
  - `…/usr/lib/udev/rules.d/60-bastion-sunshine.rules`
  - `crates/os/src/remote_play.rs`
- Test: unit tests (a stand-in HTTPS server on 127.0.0.1 with a self-signed certificate); `tests/os_services.rs`

**Interfaces:**
- Produces:
  - **The build:**
    - Sunshine is built from the pinned source (with its submodules); npm packages go through `npm ci` and the lockfile's integrity hashes;
    - `record-component sunshine …` is called;
    - upstream's `60-sunshine.rules` (`uaccess`) is not installed.
  - **`bastion-sunshine.service`:**
    - `User=bastion-sunshine`;
    - `AmbientCapabilities=CAP_SYS_ADMIN CAP_SYS_NICE`, with `CapabilityBoundingSet` the same;
    - `DeviceAllow=` the DRM devices, `/dev/uinput` and `/dev/uhid`;
    - `StateDirectory=bastion-sunshine`;
    - disabled.
  - **The config the helper writes:**
    - `origin_web_ui_allowed = pc`, `address_family = ipv4`, `capture = kms`, `upnp = off`;
    - the credentials file in the state directory;
    - an apps file with one entry, the screen, with no commands, root-owned and read-only to Sunshine.
  - **`remote-play on`:**
    1. makes the credentials with `sunshine --creds` if they are missing (random, root-only);
    2. starts Sunshine;
    3. fills `remote_play`;
    4. writes `remote-play.json`.
  - **`remote-play off`** reverses each step.
  - **`remote-play pin NNNN`** posts `{"pin":"NNNN","name":"Moonlight"}` to `https://127.0.0.1:47990/api/pin` with basic auth, pinned to Sunshine's own certificate.
  - **The watch service** updates `clients` while Sunshine runs, as far as Sunshine's API reports them. If the API reports only paired devices, the notes and the front end say "paired".

- [ ] **Step 1: Write the failing tests:**
  - `the_sunshine_config_keeps_the_web_ui_on_this_machine_and_turns_upnp_off`;
  - `the_apps_file_defines_no_commands`;
  - `the_pin_is_posted_with_basic_auth_to_localhost_only`;
  - `remote_play_off_stops_sunshine_and_closes_its_ports` (fake runner);
  - `the_capabilities_come_only_from_the_unit` (no `setcap` anywhere in the build script);
  - `uinput_and_uhid_belong_to_the_sunshine_group_not_uaccess`;
  - `every_sunshine_source_is_pinned`;
  - Wave 2, full profile:
    - `vm_remote_play_is_off_by_default_and_its_ports_are_closed`;
    - `vm_turning_remote_play_on_opens_only_its_ports` (47990 stays closed to the network).
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-os remote_play && cargo test --test os_services sunshine`. Expected: FAIL.
- [ ] **Step 3: Implement.** If no capture-only PipeWire socket can be configured, ship v1 without remote-play audio and record a `Ruling:`.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: Sunshine from pinned sources, off by default, capability only in its unit, web UI on this machine only`

### Task 8: Updates

**Files:**
- Create: `crates/os/src/update.rs`
- Test: unit tests (a stand-in API on 127.0.0.1, a local git repository over `file://`, a fake `build-image.sh` and a fake sysupdate)

**Interfaces:**
- Consumes:
  - P1's `build-image.sh` options and output names;
  - the sysupdate transfers;
  - `/etc/bastion-os/update.conf`, which must be root-owned and not writable by others.
- Produces **`bastion-os update`.** It takes `/run/bastion-os/update.lock` (flock); a second run exits with `an update is already running`. It writes `update.json` at every step:
  1. **`checking`:** `GET https://api.github.com/repos/Al3xSk1/Bastion-MC/actions/workflows/ci.yml/runs?branch=main&status=success&event=push&per_page=1`, read `head_sha`. If it equals `BASTION_COMMIT`, it ends with `idle` and the message `up to date`.
  2. **`fetching`:** `git fetch` into `/var/lib/bastion-os/src`, then `git merge-base --is-ancestor <sha> origin/main` (refused otherwise), then a checkout.
  3. **`building`:** first check for at least 30 GiB free (fail early otherwise), then run the new checkout's `build-image.sh` with `/var/lib/bastion-os/keys`, the next version, and the profile from `update.conf`.
  4. **`installing`:** `/usr/lib/systemd/systemd-sysupdate --transfer-source=<out> update`.
  5. **`ready`:** a restart is needed.

  On any failure: `failed`, the reason in `message`, and nothing installed unless sysupdate finished.

- [ ] **Step 1: Write the failing tests:**
  - `the_newest_main_commit_whose_ci_passed_is_chosen`;
  - `a_commit_that_is_not_on_main_is_refused`;
  - `an_up_to_date_machine_reports_idle_and_builds_nothing`;
  - `a_failed_build_leaves_the_status_failed_and_installs_nothing`;
  - `an_update_without_enough_free_space_fails_before_building`;
  - `an_api_error_leaves_the_machine_as_it_is` (unreachable, 403 rate limit, an empty `workflow_runs`);
  - `two_updates_cannot_run_at_once`;
  - `the_update_config_is_read_only_from_a_root_owned_file`;
  - Wave 2, installed OS: `vm_an_update_started_from_the_front_end_installs_into_slot_b`. It runs the launcher's `os update` as the player and follows `update.json` to `ready`. After a reboot, the new version runs.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-os update`. Expected: FAIL.
- [ ] **Step 3: Implement.** The API and source URLs come from `update.conf`, so tests can point them elsewhere. Production writes only the defaults.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: updates build the newest main commit whose CI passed, on the machine, into the spare slot`

### Task 9: The token-store switch

**Files:**
- Create: `crates/os/src/token_store.rs`
- Test: unit tests (a temporary root)

**Interfaces:**
- Produces:
  - `token-store keyutils` moves `/etc/bastion-mc/age-recipients` to `/var/lib/bastion-os/age-recipients.off`, keeping its mode and owner.
  - `token-store age` moves it back.
  - Asking for the state the machine is already in does nothing and exits 0. P4's launcher command does the config write and the sign-out (Decision 1).

- [ ] **Step 1: Write the failing tests:**
  - `switching_to_keyutils_moves_the_system_recipients_file_aside_and_back_restores_it`;
  - `switching_twice_is_harmless`;
  - `a_missing_recipients_file_on_both_sides_is_reported`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-os token_store`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `os: the root side of switching the Java sign-in between the age vault and the kernel keyring`

### Task 10: The Java token survives a restart (Wave 2 test), and the notes

**Files:**
- Test: `tests/os_services.rs`
- Create: `docs/notes/os-p2.md`

- [ ] **Step 1: Write the opt-in Wave 2 test** `vm_the_java_token_survives_a_restart_through_age_and_the_tpm`. As the player, it stores a dummy refresh token through the launcher's age store, using a test-only launcher entry point, or `bastion-mc` with a stand-in sign-in. It reboots, then reads the token back. A test-only path must not exist in release builds.
- [ ] **Step 2: Write the notes.**
  - The units and their hardening.
  - The X display arrangement.
  - The capture-only audio socket, or the ruling that there is none.
  - What Sunshine's API reports about clients.
  - Every `Ruling:` this package made.
  - User-facing text for docs/OS.md: remote play, and what "a paired device sees the screen and controls the machine" means; updates; re-binding.
- [ ] **Step 3: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check`. Expected: 0 failed.
- [ ] **Step 4: Commit.** `docs: notes on the Bastion OS system services`
