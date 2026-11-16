# Bastion OS — P4: Setup pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every set-up that today needs bastion-mc's terminal commands has a page in the front end, through the launcher:
- which Microsoft app sign-ins go to, and whether the Java sign-in survives a restart;
- the server allowlist, and joining a server;
- mod profiles from the verified catalog, signed with a key the launcher keeps;
- the phone services' sign-ins (Spotify, Discord, the Realms TOTP set-up);
- a config manager: every setting of `config.toml` and of each instance's `instance.toml`, from a Configuration page and the `config` commands (index Decision 13).

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
5. **A config key the schema does not describe,** for example a field added to the config structs later. The schema test fails until the key is described. Pinned by Task 5 `the_schema_covers_every_key_of_the_config`.

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

### Task 5: The config schema and a `config` command for every key

**Files:**
- Create: `src/config_schema.rs`
- Modify:
  - `src/config_edit.rs` (F2's writer, generalised from its fixed `Setting` list to any schema key with a typed value; F2's variants become shorthands);
  - `src/cli.rs`: `config list [--changed]`, `config get`, `config set <key> <value> [--confirm]`, `config unset`, `config check`, all with `--events`; HELP;
  - `crates/proto/src/setup.rs` (`ConfigView`, `KeyView`, `Event::Config`).
- Test: unit tests in `src/config_schema.rs` and `src/config_edit.rs`; tests/events.rs

**Interfaces:**
- Consumes: F2's `config_edit::apply`, the existing parser, and `EXAMPLE_CONFIG` (`examples/config.toml`).
- Produces:
  - **`pub struct KeyInfo`** with fields:
    - `path: &'static str` (dotted, for example `sandbox.allow_x11`)
    - `kind: Kind` (`Bool`, `Int { min, max }`, `Str`, `Choice(&'static [&'static str])`, `List(Box<Kind>)`, `Table`)
    - `default: &'static str` (as TOML text)
    - `doc: &'static str` (one sentence)
    - `lowers_protection: Option<&'static str>` (what it lowers, in one sentence)
    - `os: OsRule` (`Editable`, `Fixed(&'static str)` with the reason, or `Command(&'static str)` naming the command that owns the key)
  - **`pub fn schema() -> &'static [KeyInfo]`.**
  - **`proto::setup::ConfigView`** with fields:
    - `file: String`
    - `keys: Vec<KeyView>`, each with `path`, `kind`, `value` (TOML text), `default`, `changed: bool`, `doc`, `lowers_protection: Option<String>`, `fixed_reason: Option<String>`, `command: Option<String>`

    and `Event::Config(ConfigView)`.
  - **Rules:**
    - `set` of a `lowers_protection` key without `--confirm` ends with an `Error` that quotes what it lowers.
    - On Bastion OS, a `Fixed` key is refused with its reason.
    - A `Command` key names its command.
    - Every value goes through the real parser before the file is replaced.

- [ ] **Step 1: Write the failing tests:**
  - `the_schema_covers_every_key_of_the_config`: every key of `EXAMPLE_CONFIG` is in the schema. So is every key of `Config::default()` serialised to TOML, and nothing else.
  - `every_schema_default_is_the_structs_default`.
  - `setting_any_key_round_trips_through_the_parser`: for each key, a valid non-default value is written, read back and parsed.
  - `a_protection_lowering_key_needs_confirmation`.
  - `every_protection_lowering_key_says_what_it_lowers`.
  - `on_bastion_os_keys_the_os_fixes_are_read_only_with_the_reason`.
  - `unset_returns_a_key_to_its_default_and_keeps_comments`.
  - `config_check_reports_the_parsers_message_and_line`.
  - `config_list_changed_shows_only_keys_that_differ_from_their_default`.
  - In tests/events.rs: `config_list_over_events_emits_the_whole_view`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib config_schema && cargo test --lib config_edit && cargo test --test events config_list`. Expected: FAIL.
- [ ] **Step 3: Implement the schema by hand from `src/config.rs` and `examples/config.toml`.**
  - Classify protection-lowering keys from the code and docs/THREAT_MODEL.md. At least these:
    - `sandbox.allow_x11`, `sandbox.allow_pulse_module_loading`, `sandbox.extra_syscalls`, `sandbox.exec_allow`, `sandbox.extra_rw_dirs`, `sandbox.seccomp`
    - `launcher.confine`
    - `java.allow_serialization`
    - `tls.builtin_pins`, `tls.pins`
    - `network.mode`
    - `downloads.extra_hosts`
    - `bedrock.seccomp`, `bedrock.extra_syscalls`

    List the final set in the notes.
  - The OS-fixed keys follow F2's display rule, plus `launcher.confine`.
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `config: a schema of every setting, and config list, get, set, unset and check for all of them`

### Task 6: The Configuration page

**Files:**
- Create: `gui/src/view/config.rs`
- Modify: `gui/src/app.rs`, `gui/src/backend.rs`, `gui/src/view/mod.rs`
- Test: `gui/src/view/tests.rs`

**Interfaces:**
- Consumes: Task 5.
- Produces the Configuration page:
  - sections in the config file's order, and a search box;
  - each key with a control for its kind (switch, number, choice, text, list), its current value, its default and its sentence;
  - a "changed" mark and "Reset to default";
  - protection-lowering settings: a confirmation that says what they lower before the change is sent with `--confirm`;
  - read-only keys with their reason; keys owned by a command, linked to that command's page;
  - "Check the file" (`config check`), and the file's path.

- [ ] **Step 1: Write the failing view tests:**
  - `the_configuration_page_groups_keys_by_section_and_finds_them_by_search`
  - `a_changed_key_is_marked_and_can_be_reset`
  - `turning_on_a_protection_lowering_setting_asks_first_and_says_what_it_lowers`
  - `keys_fixed_by_bastion_os_are_read_only_with_the_reason`
  - `an_invalid_value_shows_the_parsers_message`
  - each at the four sizes
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-mc-gui --lib config`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command, and the Adwaita-only run. Expected: PASS.
- [ ] **Step 5: Commit.** `front end: the Configuration page — every setting, with its explanation, its default and a warning where it lowers protection`

### Task 7: Instance settings

**Files:**
- Modify:
  - `src/config_schema.rs` (`instance_schema()` for `instance.toml`, from `InstanceConfig` at src/config.rs:855);
  - `src/config_edit.rs` (the same writer for `instance.toml`);
  - `src/cli.rs`: `instance config <id> list|get|set|unset`;
  - `gui/src/view/instances.rs` (a Settings panel per instance).
- Test: unit tests; `gui/src/view/tests.rs`

**Interfaces:**
- Produces:
  - `instance config <id> …`, with the same rules as Task 5, validated by the instance parser.
  - `Event::Config` with `file` set to the instance file.
  - The Instances page's Settings panel: edit and reset, with the same confirmation rule.

- [ ] **Step 1: Write the failing tests:**
  - `instance_settings_round_trip_through_the_instance_parser`
  - `an_instance_setting_that_lowers_protection_needs_confirmation`
  - `the_instance_schema_covers_every_key_of_instance_toml`
  - view test: `the_instance_settings_panel_edits_and_resets_a_key`
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib config_schema::instance && cargo test -p bastion-mc-gui --lib instance_settings`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them; expect them to pass.** Run the same command. Expected: PASS.
- [ ] **Step 5: Commit.** `config: instance settings from the command line and the Instances page`

### Task 8: The notes

**Files:**
- Create: `docs/notes/os-p4.md`

- [ ] **Step 1: Write the notes.**
  - The new commands and events, with their final signatures.
  - The schema's protection-lowering and OS-fixed lists.
  - How the catalog rule finds the catalog.
  - The Spotify and Discord host lists.
  - Every `Ruling:` this package made.
  - The user-facing text for docs/OS.md and the README: signing in, staying signed in, servers, mods and signing, the phone services.
- [ ] **Step 2: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check`. Expected: 0 failed.
- [ ] **Step 3: Commit.** `docs: notes on the Bastion OS setup pages`
