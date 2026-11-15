# Bastion OS — P5: Clips and Bedrock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- Clips leave the machine through LocalSend, send-only, during a time-limited opening. Sending works from the front end, and from the in-game phone while a game runs.
- Bedrock installs with one click: mcpelauncher is built into the image from pinned sources; mcpelauncher-ui runs sandboxed for the Google sign-in and download; bastion imports the version, asks once about the licence substitution, and creates the instance.

**Architecture:**
- **`src/localsend/`** implements LocalSend protocol v2's sending side:
  - announcement and discovery;
  - a `register` endpoint that answers only during an opening;
  - `prepare-upload`, `upload` and `cancel` over HTTPS, pinned to the receiver's announced fingerprint.
- **The launcher** starts C1's opening unit and sends only files of its clips library. Decision 6 lets the confined launcher do the same for the phone.
- **Bedrock:**
  - a recipe fragment builds mcpelauncher-client and mcpelauncher-ui;
  - `bedrock get` runs the UI in a sandbox on F2's `OwnDisplay`, with a directory and a network allowlist of its own;
  - the rest reuses round 5's store, import and launch;
  - on Bastion OS, the Xbox sign-in of a running Bedrock game goes through a sign-in window started by the front end. Its final address goes back through the existing `answer signin` path.

**Tech Stack:** Rust (the existing rustls/tokio stack; `rcgen` for the opening's self-signed certificate, if `cargo deny` accepts it), UDP multicast, HTTPS, the existing sandboxed ffmpeg, mkosi build scripts, CMake, Qt6 and QtWebEngine (Arch packages), the F1 harness.

**Spec:** `docs/superpowers/specs/2026-11-14-bastion-os-design.md`: "Sending clips off the machine (LocalSend)"; "Bedrock, installed with one click". Read the index first, in particular Decisions 6, 8 and 9, C1–C3, the F2 notes, and the round-5 Bedrock documents (`docs/BEDROCK.md`, `docs/notes/bedrock-*.md`, `target/research/REPORT.md`).

## Global Constraints

The index's Global Constraints apply. In addition:
- **What may be sent:** only clips of the launcher's clips library, chosen by their stem, never by a path. The machine never accepts a file.
- **Receivers:**
  - HTTPS only; a receiver announcing `protocol: "http"` is shown and refused;
  - the TLS certificate must hash (SHA-256 of its DER) to the announced fingerprint;
  - only private addresses: IPv4 10/8, 172.16/12, 192.168/16, 169.254/16; IPv6 fe80::/10, fc00::/7.
- **The mcpelauncher build:**
  - every source is pinned in `packaging/os/sources.toml`: the client and UI manifests at pinned commits, every submodule, curl 8.21.0, nlohmann json 3.7.3, gamecontrollerdb.txt at a pinned commit, and glfw at a pinned commit;
  - nothing is downloaded by CMake, so the build uses `FETCHCONTENT_FULLY_DISCONNECTED=ON` or patches that point the `ext/*.cmake` files at the pinned local copies;
  - the build runs only in VMs (Decision 9);
  - upstream's `mcpelauncher-webview` is not installed, because round 5's sign-in stub replaces it.
- **Google:** the Google sign-in lives only in the UI's own directory, `<data>/bedrock/ui/`. It never reaches the game's sandbox or the launcher's other files.
- **Text:** user-facing sentences go into `docs/notes/os-p5.md`.

## Review Focus

1. **A receiver that vanishes mid-upload, or a Wi-Fi drop.** Pinned by Task 1 `a_receiver_that_vanishes_mid_upload_ends_the_transfer_cleanly`.
2. **Two receivers with the same alias, one spoofed (GHSA-424h-5f6m-x63f).** Pinned by Task 4 `two_receivers_with_the_same_alias_are_told_apart_by_fingerprint`.
3. **Google's sign-in or Play delivery needing a host that is not in the allowlist.** Pinned by Task 6 `a_blocked_host_is_named_in_the_setup_status`.
4. **A receiver that declines or has no room.** Pinned by Task 1 `a_receiver_refusing_the_files_reports_it`.
5. **The licence switch.** It is never on by default, and launches wait for it. Pinned by Task 8 `the_licence_switch_starts_off_and_launches_wait_for_it`.

---

### Task 1: LocalSend's sending side

**Files:**
- Create: `src/localsend/{mod.rs,announce.rs,register.rs,upload.rs,tls.rs}`; `tests/support/localsend_standin.rs`
- Modify: `src/lib.rs`; `Cargo.toml` (`rcgen`)
- Test: unit tests; `tests/localsend.rs` (stand-in receiver over loopback)

**Interfaces:**
- Produces:
  - **`Sender::open(identity: Identity, transport: Box<dyn Transport>) -> Result<Sender>`:**
    - `Identity` has `alias` ("Bastion OS (<hostname>)"), `device_model` "Bastion OS", `device_type` "desktop", and the opening's ephemeral certificate and its fingerprint.
    - `Transport` is the multicast group `224.0.0.167:53317` in production, and unicast loopback in tests.
  - **`Sender::receivers(&self) -> Vec<Receiver>`.** `Receiver` has `alias`, `model`, `kind`, `fingerprint`, `address` and `port`.
  - **`Sender::send(&self, to: &Receiver, files: &[ClipFile], pin: Option<&str>, progress: impl FnMut(Transfer)) -> Result<()>`:**
    1. `prepare-upload` (`{info, files:{id:{id,fileName,size,fileType,sha256}}}`);
    2. one `upload?sessionId&fileId&token` per file;
    3. `cancel` when stopped.

    Responses 204, 401 (PIN), 403, 409, 429 and 507 are turned into plain sentences.
  - **The register endpoint** answers only `POST /api/localsend/v2/register`, with a body of at most 4 KiB, and only while the sender is open. Any other request gets 404.

- [ ] **Step 1: Write the failing tests:**
  - `the_announcement_has_every_v2_field`
  - `a_receiver_that_registers_is_listed`
  - `a_receiver_that_answers_by_multicast_is_listed`
  - `the_upload_is_refused_when_the_certificate_does_not_match_the_fingerprint`
  - `an_http_receiver_is_refused`
  - `a_public_address_is_refused`
  - `a_pin_protected_receiver_asks_for_its_pin`
  - `cancel_stops_the_upload_and_tells_the_receiver`
  - `a_receiver_that_vanishes_mid_upload_ends_the_transfer_cleanly`
  - `a_receiver_refusing_the_files_reports_it`
  - `the_register_endpoint_answers_nothing_else`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test localsend && cargo test --lib localsend`. Expected: FAIL.
- [ ] **Step 2b: Write the opt-in VM test** `vm_localsend_discovers_a_receiver_on_the_lan`: two network namespaces joined by a veth pair in an ISO VM, one running the stand-in receiver. It is run in Wave 2 if it needs the installed OS.
- [ ] **Step 3: Implement.** Verify certificates with a custom rustls verifier that compares the leaf's SHA-256 with the fingerprint. The receiver's name is not checked, since LocalSend certificates are self-signed.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `localsend: the sending side of LocalSend v2, pinned to each receiver's fingerprint`

### Task 2: `clips list` and `clips send`, with the opening

**Files:**
- Create: `crates/proto/src/clips.rs`
- Modify: `src/clips/mod.rs` and `src/clips/library.rs` (clips by stem; an `.mp4` copy remuxed without re-encoding through the existing sandboxed ffmpeg, in a temporary directory, deleted afterwards); `src/cli.rs`; `crates/proto/src/lib.rs`
- Test: unit tests; tests/events.rs (with the stand-in receiver and a fake systemctl)

**Interfaces:**
- Consumes: Task 1; C1's `bastion-os-open-localsend.service` and `openings.json`; F2's `Answerable { receiver | pin }`.
- Produces:
  - **Types:**
    - `proto::clips::Clip { stem, title, seconds, bytes, thumbnail: Option<String> }`, where `thumbnail` is the PNG's path;
    - `Receiver { alias, model, kind, fingerprint }`;
    - `Transfer { file, done, total }`.
  - **Events:** `Event::Clips(Vec<Clip>)`, `Event::Receivers(Vec<Receiver>)` and `Event::Transfer(Transfer)`.
  - **`clips send <stem>…`** works in this order:
    1. on Bastion OS, start the opening;
    2. announce every 2 s;
    3. emit `Receivers` as they change;
    4. wait for `answer receiver <fingerprint>`, then `answer pin` if the receiver asks for one;
    5. upload, emitting `Transfer`, and start the opening unit again every 60 s while the upload runs;
    6. end.

    Off Bastion OS there is no unit, and the desktop's firewall is the owner's business.

- [ ] **Step 1: Write the failing tests:**
  - `clips_list_reports_the_library`
  - `only_clips_of_the_library_can_be_sent`
  - `send_starts_the_opening_on_bastion_os_and_keeps_it_open_while_uploading`
  - `the_receiver_is_chosen_over_the_control_pipe`
  - `clips_are_sent_as_mp4_copies_and_the_copies_are_removed`
  - `clips_events_round_trip` (proto)
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib clips && cargo test --test events clips_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `clips: list and send clips to a LocalSend receiver, with the firewall opened only while sending`

### Task 3: Sending from the in-game phone (Decision 6)

**Files:**
- Modify:
  - `src/launch.rs`, before step 9;
  - `src/confine.rs` and `src/confine/` (one TCP connect port, 53317; no other change);
  - `src/spotify/dbus.rs`, generalised so the hand-written client can also reach the system bus (or a new `src/dbus.rs` that both use);
  - `src/clips/mod.rs` (the phone verb `send`; the verb list at :439-446, :723-795);
  - the companion mod's `ClipsApp.java`, plus its phone UI for choosing a receiver and seeing progress.
- Test: unit tests (the confinement plan, the D-Bus call builder); the phone-verb tests in the existing style; the companion's own tests if it has them

**Interfaces:**
- Produces, only when `os::detect()` is `Some` and `[clips] enabled`:
  - a system-bus connection that can call only `StartUnit("bastion-os-open-localsend.service", "replace")`;
  - a TCP listener on port 53317, bound before step 9 and answering only during an opening;
  - Landlock's TCP connect allowance for port 53317, with the destination limited to private addresses in the code (Landlock filters ports, not addresses).

  The phone's `send` verb runs Task 2's flow and shows receivers and progress in the phone.

- [ ] **Step 1: Write the failing tests:**
  - `off_bastion_os_or_with_clips_off_none_of_the_three_allowances_exist`
  - `the_system_bus_connection_can_start_only_the_opening_unit`
  - `the_phone_sender_refuses_a_public_address`
  - `the_register_listener_answers_only_during_an_opening`
  - `the_phone_send_verb_lists_receivers_and_reports_progress`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib confine && cargo test --lib clips`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add a comment at each allowance that points to Decision 6, so reviewers find the reasoning.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, and the confinement suites (`cargo test --test confine*` as they exist). Expected: PASS, with the existing confinement tests unchanged.
- [ ] **Step 5: Commit.** `phone: send clips over LocalSend during a game, with three narrow allowances made before confinement on Bastion OS`

### Task 4: The Sending clips page

**Files:**
- Create: `gui/src/view/clips.rs`
- Modify: `gui/src/app.rs`, `gui/src/backend.rs`, `gui/src/view/mod.rs`
- Test: `gui/src/view/tests.rs`

**Interfaces:**
- Consumes: Task 2.
- Produces the Sending clips page:
  - the clips with their thumbnails, where the front end reads the PNG files the events name;
  - choose clips, then "Send to a device", which shows the opening's countdown;
  - receivers with alias, model and the first 16 hex characters of the fingerprint, grouped as "xxxx xxxx xxxx xxxx";
  - a PIN prompt when a receiver asks for one;
  - progress and Cancel.

- [ ] **Step 1: Write the failing view tests:**
  - `the_sending_page_lists_clips_with_thumbnails`
  - `receivers_show_alias_model_and_a_short_fingerprint`
  - `two_receivers_with_the_same_alias_are_told_apart_by_fingerprint`
  - `a_receiver_pin_is_asked_for`
  - `progress_and_cancel`
  - `the_opening_countdown_is_shown`
  - each at the four sizes
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-mc-gui --lib clips`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, and the Adwaita-only run. Expected: PASS.
- [ ] **Step 5: Commit.** `front end: the Sending clips page`

### Task 5: mcpelauncher built into the image

**Files:**
- Create:
  - `packaging/os/mkosi/mkosi.build.d/40-mcpelauncher.chroot`
  - `packaging/os/mkosi/mkosi.conf.d/40-mcpelauncher.conf` (`[Match] Profiles=full`; build and runtime packages, including qt6-base, qt6-declarative, qt6-svg and qt6-webengine)
  - `packaging/os/mkosi/patches/mcpelauncher/` (only what is needed to keep CMake offline)
  - `packaging/os/sources.toml` entries
- Test: `tests/os_recipe.rs`; the Wave 2 VM test in `tests/os_install.rs`

**Interfaces:**
- Consumes: P1's `fetch-pinned` and `record-component`.
- Produces:
  - `/usr/bin/mcpelauncher-client`, `/usr/bin/mcpelauncher-ui-qt` (or the UI's binary name; record it), and their libraries;
  - the `components.json` entries.

- [ ] **Step 1: Write the failing tests:**
  - `every_mcpelauncher_source_is_pinned`: every URL the build uses is in `sources.toml`, and no `ext/*.cmake` download is left without a hash.
  - `the_mcpelauncher_build_never_installs_the_upstream_webview`.
  - `the_build_script_stops_on_a_source_mismatch`.
  - Wave 2, full profile: `vm_the_image_has_mcpelauncher_built_from_pinned_sources`. `components.json` lists both programs, and `mcpelauncher-client --help` (or the version flag) runs.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --test os_recipe mcpelauncher`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Pin the sources by reading the downloaded upstream trees in `target/research/mcpe-*`: list the submodules and their commits there.
  - Fetch the pinned tarballs only inside the build VM.
  - Run the build as an unprivileged build user in the mkosi build overlay.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, and a `full` build in the VM: `BASTION_OS_TESTS=1 cargo test --test os_install vm_build_image -- --test-threads=1`, with `--profile full` if the test takes a profile. Expected: PASS.
- [ ] **Step 5: Commit.** `os: mcpelauncher built into the image from pinned sources, with CMake kept offline`

### Task 6: `bedrock get`, import from the UI, and removing the Google sign-in

**Files:**
- Create: `src/bedrock/ui.rs`; `crates/proto/src/bedrock_setup.rs`; `tests/support/bedrock_ui_standin.rs` (a stand-in UI that writes a fake version directory)
- Modify:
  - `src/bedrock/runtime.rs` (:583): on Bastion OS the version comes from `components.json`;
  - `src/bedrock/store.rs` (:718, :1265-1318): an import source under the UI's directory;
  - `src/cli.rs`: `bedrock get`, `bedrock import --from-ui <version>`, `bedrock remove-google-sign-in`, and `--events` for `bedrock import|versions|remove`;
  - `crates/proto/src/lib.rs`.
- Test: unit tests; tests/events.rs

**Interfaces:**
- Consumes: F2's `OwnDisplay` (`Purpose::BedrockUi`) and the existing sandbox, relay and store.
- Produces:
  - `proto::bedrock_setup::BedrockSetup` with fields:
    - `step: BedrockStep` (`UiOpen`, `Downloaded`, `Importing`, `Licence`, `Ready` or `Failed`)
    - `versions: Vec<String>`
    - `message: Option<String>`

    and `Event::BedrockSetup(BedrockSetup)`.
  - **`bedrock get`** runs the UI with:
    - a display of its own;
    - `HOME` and the XDG directories under `<data>/bedrock/ui/`, and nothing else of `<data>`;
    - a read-only `/usr`;
    - the network only through the relay, to the Google sign-in and Play delivery hosts. The list is derived from the upstream sources and recorded, with sources, in the notes.

    QtWebEngine's own sandbox is used if it works inside bastion's. If it does not, the reason and the replacement go into a `Ruling:`. When the UI exits, the command emits `Downloaded` with the versions found under the UI's `mcpelauncher/versions/`.
  - **`bedrock import --from-ui <version>`** applies the existing import checks: x86_64 only, no links or special files, everything hashed, read-only.
  - **`bedrock remove-google-sign-in`** deletes `<data>/bedrock/ui/`.

- [ ] **Step 1: Write the failing tests:**
  - `on_bastion_os_the_client_version_comes_from_components_json`
  - `the_ui_sandbox_sees_only_its_own_directory_and_its_display`
  - `the_ui_network_reaches_only_the_google_hosts`
  - `bedrock_get_offers_the_versions_the_ui_downloaded`
  - `import_from_the_ui_directory_uses_the_existing_checks`
  - `remove_google_sign_in_deletes_the_ui_directory`
  - `a_blocked_host_is_named_in_the_setup_status`
  - `bedrock_setup_events_round_trip`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib bedrock && cargo test --test events bedrock_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, and the existing Bedrock suites (`cargo test --test bedrock_store --test bedrock_launch`). Expected: PASS.
- [ ] **Step 5: Commit.** `bedrock: get a version through a sandboxed mcpelauncher-ui, import it, and remove the Google sign-in`

### Task 7: The Xbox sign-in of a running Bedrock game, on Bastion OS

**Files:**
- Modify: `src/cli.rs` (`bedrock signin-window <url>`, Bastion OS only); `src/bedrock/signin.rs` (reuse its URL check); `crates/proto/src/bedrock_setup.rs` (`Event::BedrockSignInDone { address: String }`); the front end's Launch page and app logic
- Test: unit tests with an injected window; an app test in `gui/src/view/tests.rs` or the app tests

**Interfaces:**
- Consumes: F2's `signin::open` with `Finish::Scheme { scheme: "ms-xal-0000000048183522" }` and `Profile::Fresh`; the existing `SignInPage` event; `answer signin`.
- Produces:
  - **`bedrock signin-window <url>`** checks the URL exactly as the sign-in bridge does (login.live.com, client 0000000048183522). It opens the window and emits `BedrockSignInDone { address }`.
  - **The front end, on Bastion OS:** a `SignInPage` from the running launch starts that command. Its address goes to the launch with `answer signin <address>`, the path a pasted address takes on desktops today. The launch itself stays confined and starts nothing.

- [ ] **Step 1: Write the failing tests:**
  - `bedrock_signin_window_takes_only_the_ms_xal_address`
  - `bedrock_signin_window_refuses_a_url_the_bridge_would_refuse`
  - `on_bastion_os_the_front_end_hands_the_xbox_address_to_the_running_game`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib bedrock::signin && cargo test -p bastion-mc-gui --lib bedrock`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `bedrock: on Bastion OS the Xbox sign-in runs in a sign-in window, and the game stays confined`

### Task 8: The Install Bedrock flow

**Files:**
- Create: `gui/src/view/bedrock_setup.rs`
- Modify: `gui/src/app.rs`, `gui/src/backend.rs` (`CreateInstance` for Bedrock, and the new requests), `gui/src/view/instances.rs`, `gui/src/view/mod.rs`
- Test: `gui/src/view/tests.rs`

**Interfaces:**
- Consumes: Tasks 6–7; F2's `config set bedrock.accept_licence_substitution`.
- Produces "Install Bedrock" (and "Update Bedrock"):
  1. the UI window opens;
  2. the downloaded version is chosen (the newest is preselected);
  3. it is imported;
  4. the licence step: the explanation from `docs/BEDROCK.md` in plain words, and a switch that starts off;
  5. the instance is created.

  "Remove Google sign-in" asks first.

- [ ] **Step 1: Write the failing view tests:**
  - `the_install_bedrock_flow_walks_through_ui_import_licence_and_instance`
  - `the_licence_switch_starts_off_and_launches_wait_for_it`
  - `remove_google_sign_in_asks_first`
  - each at the four sizes
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-mc-gui --lib bedrock_setup`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, and the Adwaita-only run. Expected: PASS.
- [ ] **Step 5: Commit.** `front end: Install Bedrock in one flow, with the licence question asked once`

### Task 9: The notes

**Files:**
- Create: `docs/notes/os-p5.md`

- [ ] **Step 1: Write the notes.**
  - The final signatures and events.
  - The Google host list and its sources.
  - The QtWebEngine sandbox decision.
  - Every pinned mcpelauncher source.
  - Every `Ruling:` this package made.
  - User-facing text for docs/OS.md and docs/BEDROCK.md: sending clips; the opening; checking a receiver's fingerprint; Install Bedrock; the dedicated Google account the guide recommends; removing the Google sign-in.
- [ ] **Step 2: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check`. Expected: 0 failed.
- [ ] **Step 3: Commit.** `docs: notes on clips and Bedrock for Bastion OS`
