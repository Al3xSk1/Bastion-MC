# Bastion OS — P4: Setup pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every set-up that today needs bastion-mc's terminal commands has a page in the front end, through the launcher:
- which Microsoft app sign-ins go to, and whether the Java sign-in survives a restart;
- the server allowlist, and joining a server;
- mod profiles from the verified catalog, signed with a key the launcher keeps;
- the phone services' sign-ins (Spotify, Discord, the Realms TOTP set-up).

**Architecture:**
- **The launcher** gains `--events` support for the set-up commands that are terminal-only today (cli.rs:343-350 lists what is allowed). It also gains `server` and `token-store` commands, built on F2's config writer and P2's token-store unit.
- **Sign-ins** run in F2's sign-in windows on Bastion OS. The Realms TOTP secret is shown by the launcher in F2's `show_page`, never by the front end.
- **The front end** gains Settings, Servers, Mods and Phone pages. They follow its rules: one job at a time, `App::update` as the only place state changes, no credential.

**Tech Stack:** Rust; iced 0.14; the event protocol; F2's `config_edit`, `signin::open` and `show_page`; `qrcode` for the TOTP page (with its SVG renderer, if `cargo deny` accepts it).

**Spec:** `docs/superpowers/specs/2026-11-14-bastion-os-design.md`: "Setting up bastion-mc without a terminal" (signing in, servers, mod profiles, the phone services' logins, settings); "The Java sign-in's token"; the "my call" that only catalog mods come onto the machine. Read the index first, in particular Decisions 1, 5 and 8, C1–C3, and the F2 notes (`docs/notes/os-foundation.md`).

## Global Constraints

The index's Global Constraints apply. In addition:
- **Desktops:** the new `--events` support and `server` work everywhere. The launcher's own signing key, the catalog-only rule and `token-store` are Bastion OS only. Everything else on desktops behaves as today.
- **No secret reaches the front end:** no token, key, TOTP secret or Spotify/Discord credential is ever in an event. A test captures every event of a TOTP set-up and asserts the secret is in none of them.
- **Layout:** the new views are laid out at the four test window sizes and checked with only Adwaita fonts.
- **Text:** user-facing sentences go into `docs/notes/os-p4.md` for the documents pass.

## Review Focus

1. **A config the owner edited by hand into a layout the writer has to handle** (servers as dotted keys, arrays of tables, comments between entries). The result is a valid file or a clean refusal, never a corrupt one. Pinned by Task 2 `servers_written_in_an_unusual_layout_are_rewritten_or_refused_cleanly`.
2. **Switching where the Java sign-in is kept while a game runs.** Pinned by Task 1 `the_token_store_cannot_switch_while_a_game_runs`.
3. **The launcher's own signing key lost** (a data directory restored without it). Pinned by Task 3 `a_profile_signed_by_a_key_that_is_gone_says_how_to_re_sign`.
4. **The Spotify developer app registered with a different redirect address.** Pinned by Task 4 `a_spotify_redirect_error_names_the_address_to_register`.
5. **A second sign-in window while one is open.** Pinned by Task 4 `a_second_sign_in_window_is_refused_while_one_is_open`.

---

### Task 1: The Settings page — the sign-in app and where the Java sign-in is kept

**Files:**
- Create: `gui/src/view/settings.rs`
- Modify:
  - `src/cli.rs`: the `token-store age|keyutils` command, Bastion OS only; HELP; the `--events` allow-list.
  - `gui/src/app.rs`, `gui/src/backend.rs`, `gui/src/view/mod.rs`.
- Test: unit tests with an injectable runner; `gui/src/view/tests.rs`

**Interfaces:**
- Consumes:
  - F2: `config set`, `config show` and `Event::Settings`;
  - C1's `bastion-os-token-store@<x>.service`;
  - the existing logout.
- Produces:
  - **`token-store <x>`:**
    1. refuses while a game runs (from the existing running-games record);
    2. runs `systemctl start bastion-os-token-store@<x>.service` and waits for it;
    3. applies `Setting::AuthTokenStore(x)`;
    4. signs every account out;
    5. emits `Status` ("Signed out: sign in again, and the sign-in is kept in the new place.") and then `Settings`.

    Off Bastion OS it ends with `Event::Error` and points to `config set auth.token_store`.
  - **The Settings page:**
    - "Sign-ins go to bastion-mc's app" or "… your own app (client ID …)";
    - "Use your own Azure application", a field with a client-ID shape check, Save and Remove;
    - "Stay signed in across restarts" (the default on Bastion OS) and "Sign in after each restart", each with its trade-off in one sentence from the spec. Staying signed in means anyone who switches the machine on can play as the owner.

- [ ] **Step 1: Write the failing tests:**
  - `switching_to_keyutils_on_bastion_os_starts_the_unit_writes_the_config_and_signs_out`
  - `the_token_store_cannot_switch_while_a_game_runs`
  - `token_store_is_refused_outside_bastion_os_and_names_config_set`
  - view tests:
    - `the_settings_page_shows_which_app_sign_ins_go_to`
    - `an_own_client_id_must_look_like_a_client_id`
    - `the_token_store_choice_states_its_trade_off`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib token_store && cargo test -p bastion-mc-gui --lib settings`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `settings: which Microsoft app sign-ins go to, and where the Java sign-in is kept, from the front end`

### Task 2: Servers

**Files:**
- Create: `src/servers.rs`; `gui/src/view/servers.rs`
- Modify:
  - `src/cli.rs`;
  - `gui/src/backend.rs`, where the `Launch` request gains `join: Option<String>` and passes `--join`;
  - `gui/src/app.rs`, `gui/src/view/mod.rs`.
- Test: unit tests (temporary config files); `gui/src/view/tests.rs`

**Interfaces:**
- Consumes: F2's `Setting::NetworkServers` and `Settings.servers`.
- Produces:
  - **The commands:**
    - `server list` emits `Settings`.
    - `server add <host[:port]> [--voice P]… [--udp P]… [--allow-private]`: an existing entry for the host is replaced, and every check of the config parser applies (port range, private addresses only with `--allow-private`).
    - `server remove <host>`.
  - **The Servers page:**
    - a list with ports, and voice ports when there are any;
    - Add, with host, port, voice ports and a private-address switch with its warning;
    - Remove;
    - Join, which starts the launch with `--join`.

- [ ] **Step 1: Write the failing tests:**
  - `adding_a_server_keeps_the_rest_of_the_config`
  - `a_server_with_voice_ports_is_written_as_a_table`
  - `removing_an_unknown_server_says_so`
  - `a_private_address_needs_allow_private`
  - `servers_written_in_an_unusual_layout_are_rewritten_or_refused_cleanly`
  - view tests:
    - `the_servers_page_lists_adds_and_removes_servers`
    - `join_starts_the_launch_with_the_server`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib servers && cargo test -p bastion-mc-gui --lib servers`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `servers: add, remove and join allowlisted servers from the front end`

### Task 3: Mod profiles, signed with the launcher's own key on Bastion OS

**Files:**
- Create: `gui/src/view/mods.rs`
- Modify:
  - `src/cli.rs`: `--events` for `profile new|add|check|lock|audit|sign|verify|list`;
  - `src/profile/` and `src/profile/signing.rs` (`SecretKey::load` at :119-132; the trust store at :193-254);
  - `crates/proto/src/setup.rs` (`Event::Profiles`);
  - the front end's app, backend and view mod.
- Test: unit tests; tests/events.rs; `gui/src/view/tests.rs`

**Interfaces:**
- Produces:
  - **`Event::Profiles(Vec<ProfileSummary>)`.** `ProfileSummary` fields:
    - `id`, `minecraft`, `loader`
    - `mods: Vec<ModEntry { name, version, source }>`
    - `signed: bool`, `verified: Option<bool>`
    - `findings: Vec<String>`, from `profile audit`
  - **The launcher's own key (Bastion OS only):** `profile sign <id>` without `--key` uses `<data>/keys/profile-signing.key`.
    - It is created on first use with `O_EXCL | O_NOFOLLOW`, mode 0600.
    - Its public key is trusted once, as `bastion-os`, in the user trust store. With a root store present, the error says root must install `bastion-os.pub`.
    - The key is never printed or put in an event.
  - **The catalog rule (Bastion OS only):** `profile add` accepts only mods of the verified catalog (read `docs/notes/mod-catalog.md` and its code). Any other mod is refused with a sentence that says why.
  - **The Mods page:**
    - create a profile (Minecraft version and loader);
    - add mods from the catalog, with search;
    - each mod's audit findings;
    - Lock, Sign and Verify, with the results.

- [ ] **Step 1: Write the failing tests:**
  - `profile_commands_work_over_events_and_report_progress`
  - `signing_on_bastion_os_without_a_key_file_uses_the_launchers_own_key_and_trusts_it_once`
  - `the_launchers_key_is_created_with_mode_0600_and_never_printed`
  - `off_bastion_os_profile_sign_still_needs_a_key_file`
  - `on_bastion_os_profile_add_refuses_a_mod_outside_the_verified_catalog`
  - `a_profile_signed_by_a_key_that_is_gone_says_how_to_re_sign`
  - view tests:
    - `the_mods_page_creates_a_profile_adds_catalog_mods_and_signs_it`
    - `audit_findings_are_shown_per_mod`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib profile && cargo test --test events profile_ && cargo test -p bastion-mc-gui --lib mods`. Expected: FAIL.
- [ ] **Step 3: Implement.** Use the existing stand-in Modrinth from the mod-catalog tests, so that no test reaches the network.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `mods: profiles from the verified catalog, signed with the launcher's own key on Bastion OS, from the front end`

### Task 4: The phone services' sign-ins

**Files:**
- Create: `gui/src/view/phone.rs`
- Modify:
  - `src/cli.rs`: `--events` for `spotify login|logout|status`, `browser login <site>` and `realms totp-setup`;
  - `src/spotify/login.rs` (:114-141);
  - `src/browser/mod.rs`, the `login` entry;
  - the Realms TOTP set-up in `src/realms/`;
  - the front end's app, backend and view mod;
  - `Cargo.toml` (`qrcode`).
- Test: unit tests with an injected sign-in window; tests/events.rs; `gui/src/view/tests.rs`

**Interfaces:**
- Consumes: F2's `signin::open` and `show_page`, and `config set spotify.client_id`, `config set realms.approve`.
- Produces:
  - **Spotify:** on Bastion OS, `spotify login` calls `open` with:
    - `Finish::Redirect` at the callback address;
    - `Profile::Fresh`;
    - Spotify's account hosts.

    A redirect carrying `error=` says which redirect address to register. Bastion OS has no Spotify desktop app (MPRIS), so the page says the phone's Spotify app works through the Web API only.
  - **Discord:** `browser login discord` calls `open` with `Finish::SiteLogin { site }` and `Profile::Phone`.
  - **Realms TOTP:** `realms totp-setup [--replace]` makes the secret as today and shows it through `show_page`: an SVG QR code, the base32 text, and the issuer and account name. The front end receives only `Status` ("The set-up code is shown in its own window.").
  - **The Phone page:**
    - Spotify: the client-ID field with how to create the developer app, the redirect address to register, sign in and out, and the state.
    - Discord: sign in, and the state.
    - Realms: approval off, code or TOTP, and the TOTP set-up button.

- [ ] **Step 1: Write the failing tests:**
  - `spotify_login_on_bastion_os_uses_a_sign_in_window`
  - `a_spotify_redirect_error_names_the_address_to_register`
  - `discord_login_on_bastion_os_uses_the_phones_profile_in_a_sign_in_window`
  - `totp_setup_over_events_never_sends_the_secret_to_the_front_end`
  - `the_totp_page_has_a_qr_code_and_the_base32_text`
  - `a_second_sign_in_window_is_refused_while_one_is_open`
  - view tests:
    - `the_phone_page_shows_each_service_and_its_state`
    - `on_bastion_os_the_spotify_card_says_there_is_no_desktop_app`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib spotify::login && cargo test --test events totp_ && cargo test -p bastion-mc-gui --lib phone`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, then `cargo deny check`. Expected: PASS.
- [ ] **Step 5: Commit.** `phone services: Spotify, Discord and the Realms TOTP set-up from the front end, in sign-in windows`

### Task 5: The notes

**Files:**
- Create: `docs/notes/os-p4.md`

- [ ] **Step 1: Write the notes.**
  - The new commands and events, with their final signatures.
  - How the catalog rule finds the catalog.
  - The Spotify and Discord host lists.
  - Every `Ruling:` this package made.
  - The user-facing text for docs/OS.md and the README: signing in, staying signed in, servers, mods and signing, the phone services.
- [ ] **Step 2: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check`. Expected: 0 failed.
- [ ] **Step 3: Commit.** `docs: notes on the Bastion OS setup pages`
