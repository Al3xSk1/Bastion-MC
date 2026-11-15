# Bastion OS — F2: Launcher foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The launcher pieces every Bastion OS page builds on:
- it knows when it runs on Bastion OS;
- it can change its own config without losing the owner's comments;
- it never gives the game the session's display;
- it can show a program in a display of its own;
- it runs every sign-in in a sign-in window.

**Architecture:**
- `src/os/` reads the root-owned `/usr/lib/os-release`.
- `src/config_edit.rs` edits `config.toml` with `toml_edit` and validates the result with the existing parser before replacing the file.
- The display plan gains a Bastion OS rule.
- The nested gamescope (`src/sandbox/nested.rs`) is generalised from "the game's compositor" to "a display of its own" for any sandboxed client.
- `src/browser/signin.rs` runs Firefox from the existing browser sandbox, visible on such a display, and watches it over the existing WebDriver BiDi bridge until the sign-in reaches its final address.

**Tech Stack:** Rust; `toml_edit` (new; check `cargo deny`); the existing bwrap/Landlock/seccomp code, nested gamescope, Firefox + WebDriver BiDi bridge, and event protocol.

**Spec:** `docs/superpowers/specs/2026-11-14-bastion-os-design.md` ("The session" G6; "Setting up bastion-mc without a terminal"; "The Java sign-in's token"). Read the index `docs/superpowers/plans/2026-11-15-bastion-os.md` first: Decisions 1, 5, 7 and 8, and C1–C4.

## Global Constraints

The index's Global Constraints apply. In addition:
- **On desktops nothing changes.** Every new behaviour is keyed on `os::detect()` returning `Some`, except the config writer and the new answer kinds, which work everywhere.
- **Config writes** go only to `$XDG_CONFIG_HOME/bastion-mc/config.toml`, under the same checks as reading (src/util.rs:270-287: user-owned, not a symlink, not group- or other-writable). The file is written with mode 0600 and replaced atomically. A result the existing parser rejects is never written.
- **Shared code:**
  - The existing modules keep their tests green.
  - `src/sandbox/display.rs` and `src/sandbox/nested.rs` are changed only by this package in this round. Wave 1 packages consume `OwnDisplay`; they do not edit those files.
- **Signatures:** the final ones go into `docs/notes/os-foundation.md` (Task 9). Wave 1 reads them there.

## Review Focus

1. **A sign-in window nobody finishes.** The timeout closes it, the command ends with a plain error, and the fresh profile is deleted. Pinned by Task 7 `the_window_closes_after_the_timeout_and_its_profile_is_deleted`.
2. **A redirect that carries `error=` or the wrong `state`.** It is reported as refused or cancelled and never redeemed. Pinned by Task 8 `an_error_redirect_is_reported_and_not_redeemed`.
3. **Two writers at once** (two front-end actions, or a hand edit in between). Writes take `config.toml.lock` (flock), re-read the file under the lock, and lose nothing. Pinned by Task 4 `two_writers_do_not_lose_each_others_change`.
4. **Wi-Fi and 802.1X secrets with UTF-8 or control characters.** UTF-8 is accepted for `wifi`, control characters are refused, and the length is capped at 512 bytes. Pinned by Task 5 `a_utf8_wifi_secret_is_accepted_and_a_control_character_is_refused`.
5. **A desktop.** The display plan, the Microsoft browser flow (`BrowserLogin`) and `browser login` are unchanged. Pinned by Task 3 `off_bastion_os_the_display_plan_is_unchanged` and Task 8 `off_bastion_os_the_microsoft_sign_in_still_emits_a_browser_link`.

---

### Task 1: Knowing it is Bastion OS

**Files:**
- Create: `src/os/mod.rs`
- Modify: `src/lib.rs` (`pub mod os;`)

**Interfaces:**
- Produces:
  - `pub struct OsInfo { pub image_version: String, pub commit: Option<String> }`
  - `pub fn detect() -> Option<OsInfo>`: reads `/usr/lib/os-release`, expecting root as owner.
  - `pub fn detect_in(root: &Path, owner: u32) -> Option<OsInfo>`: for tests.
  - `pub const RUN_DIR: &str = "/run/bastion-os";`

- [ ] **Step 1: Write the failing tests:**
  - `a_bastion_os_release_is_detected_with_its_version_and_commit`: `ID=bastion-os`, `IMAGE_VERSION=20261115.1`, `BASTION_COMMIT=<40 hex>`.
  - `an_arch_os_release_is_not_bastion_os`.
  - `an_os_release_that_is_a_symlink_or_writable_by_others_is_ignored`.
  - `a_missing_os_release_is_not_bastion_os`.
  - `a_malformed_commit_is_dropped_but_the_os_is_detected`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib os::`. Expected: FAIL (the module is missing).
- [ ] **Step 3: Implement** with `O_NOFOLLOW`, an owner check, and a mode check (no group or other write). Parse only `KEY=value` lines, with optional double quotes.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test --lib os::`. Expected: PASS.
- [ ] **Step 5: Commit.** `os: know when the launcher runs on Bastion OS (from the root-owned os-release)`

### Task 2: A vault the TPM can no longer open asks to sign in again

**Files:**
- Modify: `src/auth/store.rs` (the age decrypt path, around :472-548)
- Test: unit tests in `src/auth/store.rs`; a fixture program in `tests/support/` that imitates age failing with an identity plugin error

**Interfaces:**
- Produces: when `age -d` fails because the identity cannot be used, a `TokenStore` read returns `Error::Auth`. The message is: `The stored sign-in for <account> can no longer be opened on this machine: its TPM key is gone (a cleared TPM or a firmware reset). Sign in again; the old vault was kept as <path>.unopenable.` The vault file is renamed, never deleted. A plugin error is any stderr line containing `age-plugin-` and `identity`, or the TPM plugin's documented failure text; record the exact matcher in the notes.

- [ ] **Step 1: Write the failing tests:**
  - `a_vault_the_tpm_can_no_longer_open_asks_to_sign_in_again`: the fixture prints the plugin error and exits 1. The test asserts the message and that the vault was renamed with the `.unopenable` suffix.
  - `any_other_age_failure_keeps_todays_message`: other stderr gives the message unchanged, and the vault stays in place.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib auth::store`. Expected: FAIL.
- [ ] **Step 3: Implement** the classification on age's stderr. Today stderr is inherited; capture it into a buffer capped at 8 KiB, and still print it.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test --lib auth::store`. Expected: PASS.
- [ ] **Step 5: Commit.** `token store: a vault this machine's TPM can no longer open says to sign in again and is kept aside`

### Task 3: The game's display on Bastion OS (G6)

**Files:**
- Modify: `src/sandbox/display.rs` (`plan`/`plan_display`, :355-378); `src/launch.rs:486` and `src/bedrock/launch.rs:220`, which pass `os::detect()`
- Test: unit tests in `src/sandbox/display.rs`

**Interfaces:**
- Produces: `display::plan(…, bastion_os: bool)` (or an `Option<&OsInfo>`; record which). Under Bastion OS:
  - Wayland is never chosen, even when `WAYLAND_DISPLAY` is set or the config says `display = "wayland"`;
  - `allow_x11` is ignored;
  - the nested compositor is required, and its absence refuses the launch with `on Bastion OS the game always gets a display of its own, and the nested compositor (gamescope) is missing`;
  - the game's sandbox never binds `$XDG_RUNTIME_DIR/gamescope-*`, `pipewire-0` or the session's X socket.

- [ ] **Step 1: Write the failing tests:**
  - `on_bastion_os_wayland_session_sockets_are_never_handed_to_the_game`: `WAYLAND_DISPLAY=gamescope-0` with the socket present gives the nested plan.
  - `on_bastion_os_allow_x11_is_ignored_and_the_nested_compositor_is_required`.
  - `on_bastion_os_without_a_nested_compositor_the_launch_is_refused`.
  - `the_game_sandbox_never_binds_gamescope_or_pipewire_sockets`: inspects the bwrap spec.
  - `off_bastion_os_the_display_plan_is_unchanged`: the existing cases, with `bastion_os = false`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib sandbox::display`. Expected: FAIL.
- [ ] **Step 3: Implement** the rule at the top of the plan, before the auto/wayland/x11 choice.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test --lib sandbox`. Expected: PASS, and the existing display tests still pass.
- [ ] **Step 5: Commit.** `display: on Bastion OS the game always gets a nested display, never the session's`

### Task 4: Writing the config (`config show`, `config set`)

**Files:**
- Create: `src/config_edit.rs`, `crates/proto/src/setup.rs`
- Modify: `Cargo.toml` (`toml_edit`, the newest release that `cargo deny check` accepts); `src/lib.rs`; `src/cli.rs` (add `config show` and `config set` to the commands allowed with `--events`, cli.rs:343-350, and to HELP); `crates/proto/src/lib.rs` (`pub mod setup;`, `Event::Settings(setup::Settings)`)
- Test: unit tests in `src/config_edit.rs`; `tests/events.rs`

**Interfaces:**
- Produces:
  - `pub enum Setting` with variants:
    - `AuthClientId(Option<String>)`
    - `AuthTokenStore(TokenStoreKind)`
    - `NetworkServers(Vec<ServerEntry>)`
    - `SpotifyClientId(Option<String>)`
    - `ClipsEnabled(bool)`
    - `RealmsApprove(Approve)`
    - `BedrockAcceptLicenceSubstitution(bool)`

    The types are the existing config types; record their real names.
  - `pub fn apply(config_file: &Path, settings: &[Setting]) -> Result<()>`.
  - `pub fn settings(cfg: &Config) -> proto::setup::Settings`.
  - `setup::Settings` has the fields:
    - `client_id: Option<String>`
    - `token_store: String`
    - `servers: Vec<Server { host, port: Option<u16>, voice: Vec<u16>, udp: Vec<u16>, allow_private: bool }>`
    - `spotify_client_id: Option<String>`
    - `clips_enabled: bool`
    - `realms_approve: String`
    - `bedrock_licence_accepted: bool`
    - `bastion_os: bool`
  - `bastion-mc config set <key> <value>` for the C2 keys, and `bastion-mc config show`. Both work with `--events` and print a sentence otherwise.

- [ ] **Step 1: Write the failing tests:**
  - `setting_a_key_keeps_comments_and_other_keys`.
  - `a_missing_config_file_is_created_with_only_the_set_key`.
  - `a_value_the_config_parser_rejects_is_not_written`: for example `auth.client_id = "not-a-uuid"`. The file is unchanged, and the error names the key.
  - `an_unknown_key_is_refused`.
  - `a_symlinked_or_group_writable_config_is_refused`.
  - `the_file_is_replaced_atomically_with_mode_0600`.
  - `servers_are_written_as_strings_or_inline_tables`: a plain host gives `"host:port"`, and one with voice ports gives `{ host = …, port = …, voice = […] }`.
  - `two_writers_do_not_lose_each_others_change`.
  - In tests/events.rs: `config_show_reports_every_editable_setting` and `config_set_over_events_emits_the_new_settings`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib config_edit && cargo test --test events config_`. Expected: FAIL.
- [ ] **Step 3: Implement.** Under the lock:
  1. read the file;
  2. edit it with `toml_edit::DocumentMut`;
  3. serialise it and parse the result with the existing config parser (deny_unknown_fields, the same validation);
  4. write it to `config.toml.tmp-<pid>` (0600), fsync, and rename it;
  5. fsync the directory.
- [ ] **Step 4: Run them; expect them to pass.** Run the Step 2 command, then `cargo deny check`. Expected: PASS.
- [ ] **Step 5: Commit.** `config: show and set the settings the front end edits, keeping the file's comments`

### Task 5: Answer kinds for secrets typed in the front end

**Files:**
- Modify: `crates/proto/src/answer.rs` (the kinds); `src/events/input.rs` (the parser, :219); the rule text in `crates/proto/src/lib.rs:3-8` (Decision 5)
- Test: proto round-trip tests; input parser tests

**Interfaces:**
- Produces: the answer kinds `wifi`, `pin` and `receiver`, beside `signin` and `realms`.
  - `answer wifi <text>` accepts UTF-8 up to 512 bytes without control characters.
  - `answer pin <text>` accepts digits only, 4 to 8 of them.
  - `answer receiver <hex>` accepts 64 hex characters.
  - A command announces which kinds it takes with `Answerable { kind }`. An answer of another kind is ignored with a `Status` line, as today.

- [ ] **Step 1: Write the failing tests:**
  - `answer_kinds_travel_as_lowercase_words`: covers wifi, pin and receiver.
  - `the_control_pipe_accepts_the_new_answer_kinds`.
  - `a_utf8_wifi_secret_is_accepted_and_a_control_character_is_refused`.
  - `a_pin_must_be_digits`.
  - `an_answer_of_a_kind_the_command_did_not_ask_for_is_ignored`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test -p bastion-mc-proto && cargo test --lib events::input`. Expected: FAIL.
- [ ] **Step 3: Implement.** Rewrite the rule text so it says exactly what Decision 5 says:
  - account credentials, tokens and keys never cross the protocol;
  - a Wi-Fi or 802.1X secret, or a pairing PIN, typed by the player passes once to the launcher and is never stored or logged.
- [ ] **Step 4: Run them; expect them to pass.** Run the Step 2 command. Expected: PASS.
- [ ] **Step 5: Commit.** `protocol: answer kinds for a Wi-Fi secret, a pairing PIN and a LocalSend receiver`

### Task 6: A display of its own (`OwnDisplay`)

**Files:**
- Modify: `src/sandbox/nested.rs` (`start` :416, `Plan::spec` :241-309, `attach` :501-515)
- Test: unit tests in `src/sandbox/nested.rs`; an opt-in test in `tests/nested.rs` (`BASTION_NESTED_TESTS`, on the private display `:77`)

**Interfaces:**
- Produces:
  - `pub enum Purpose { Game, SignIn, BedrockUi }`
  - `pub struct OwnDisplay`
  - `OwnDisplay::start(cfg: &Config, purpose: Purpose, size: (u32, u32)) -> Result<OwnDisplay>`
  - `OwnDisplay::attach(&self, spec: &mut <the bwrap spec type>) -> Result<()>`: binds only this display's X socket (and cookie, if any) and sets `DISPLAY`.
  - `OwnDisplay::stop(self) -> Result<()>`

  The game path becomes `Purpose::Game` with unchanged behaviour. `noclip.so` stays on for every purpose.

- [ ] **Step 1: Write the failing tests:**
  - `an_own_display_for_a_sign_in_window_runs_gamescope_with_no_network_and_a_private_x_directory`.
  - `attach_binds_only_that_displays_socket`.
  - `the_game_purpose_keeps_todays_spec`: compares the spec with the current one.
  - Opt-in: `an_own_display_starts_and_an_x_client_can_connect_only_to_it`. An X client given the display reaches it, and the parent display `:77` is not in the client's sandbox.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib sandbox::nested`. Expected: FAIL.
- [ ] **Step 3: Implement** by generalising `Plan` over `Purpose`. Size and title depend on the purpose; the sandbox stays the same.
- [ ] **Step 4: Run them; expect them to pass.** Run `cargo test --lib sandbox::nested`, and `BASTION_NESTED_TESTS=1 DISPLAY=:77 XAUTHORITY=/dev/null cargo test --test nested own_display` with `tests/support/private-x11.sh` running. Expected: PASS.
- [ ] **Step 5: Commit.** `nested display: a display of its own for any sandboxed client, not only the game`

### Task 7: Sign-in windows

**Files:**
- Create: `src/browser/signin.rs`
- Modify: `src/browser/mod.rs` and `src/browser/sandbox.rs` (a visible mode on an `OwnDisplay` with the BiDi bridge kept; a per-window host allowlist through the relay; profiles); `crates/proto/src/lib.rs` (`Event::SignInWindow { purpose: String, open: bool }`)
- Test: unit tests; opt-in tests with the stand-in sites of `tests/support/browser_sites.rs` (`BASTION_BROWSER_TESTS`)

**Interfaces:**
- Produces:
  - `pub struct SignInWindow<'a>` with:
    - `purpose: &'static str`
    - `start_url: &'a str`
    - `allowed_hosts: &'a [&'a str]`
    - `finish: Finish<'a>`
    - `profile: Profile`
    - `timeout: Duration`
  - `pub enum Finish<'a> { Redirect { prefix: &'a str }, Scheme { scheme: &'a str }, SiteLogin { site: &'a str } }`
  - `pub enum Profile { Fresh, Phone }`
  - `pub fn open(cfg: &Config, reporter: &Reporter, w: SignInWindow<'_>) -> Result<Option<String>>`: the final address, or `None` if the player closed the window.
  - `pub fn show_page(cfg: &Config, reporter: &Reporter, purpose: &'static str, html: &str, timeout: Duration) -> Result<()>`: no network at all; the page comes from a private file.
  - Only one sign-in window at a time. A second `open` returns `Error::Busy("a sign-in window is already open")`.

- [ ] **Step 1: Write the failing tests:**
  - Unit:
    - `the_redirect_prefix_must_match_scheme_host_port_and_path`;
    - `a_scheme_finish_takes_the_ms_xal_address`;
    - `a_fresh_profile_is_deleted_after_the_window_closes`;
    - `a_second_window_is_refused_while_one_is_open`.
  - Opt-in:
    - `a_sign_in_window_ends_when_the_page_navigates_to_the_redirect`: the stand-in site redirects to `http://localhost:<port>/?code=x&state=y`, and `open` returns that address;
    - `a_host_outside_the_allowlist_is_not_reachable_from_a_sign_in_window`;
    - `the_window_closes_after_the_timeout_and_its_profile_is_deleted`;
    - `show_page_has_no_network`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib browser::signin`, then `BASTION_BROWSER_TESTS=1 cargo test --test browser signin_ -- --test-threads=1`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Use BiDi's `network.beforeRequestSent` with an intercept on the finish prefix or scheme, so the final request is read and blocked rather than sent.
  - The relay allows only `allowed_hosts` on 443.
  - `Profile::Fresh` lives in `<cache>/browser/signin-<n>`.
  - Emit `SignInWindow { open: true }` when the window shows and `open: false` when it closes.
- [ ] **Step 4: Run them; expect them to pass.** Run the Step 2 commands. Expected: PASS.
- [ ] **Step 5: Commit.** `browser: sign-in windows on a display of their own that end at the sign-in's final address`

### Task 8: The Microsoft sign-in on Bastion OS goes through a sign-in window

**Files:**
- Modify: `src/auth/msa.rs` (`interactive_login`, :416-437); the front end's Accounts page (`gui/src/view/accounts.rs`) and `gui/src/app.rs` (show a panel while `SignInWindow { open: true }`, with Cancel sending `stop`)
- Test: unit tests in `src/auth/msa.rs` (with an injectable window opener); a view test in `gui/src/view/tests.rs`

**Interfaces:**
- Consumes: Task 7 `open`; Task 1 `detect`.
- Produces:
  - On Bastion OS the browser flow calls `open` with:
    - `start_url` = the authorize URL;
    - `Finish::Redirect { prefix: "http://localhost:<port>/" }`;
    - `Profile::Fresh`;
    - `allowed_hosts` = the Microsoft sign-in hosts, listed in `src/auth/msa.rs` with a comment saying where each comes from;
    - a timeout of 300 s.
  - It checks `state`, takes `code`, and redeems it as today. No loopback listener is opened on the OS.
  - The desktop path is unchanged.

- [ ] **Step 1: Write the failing tests:**
  - `on_bastion_os_the_microsoft_sign_in_opens_a_sign_in_window_instead_of_a_browser_link`.
  - `an_error_redirect_is_reported_and_not_redeemed`.
  - `a_redirect_with_the_wrong_state_is_refused`.
  - `off_bastion_os_the_microsoft_sign_in_still_emits_a_browser_link`.
  - View test: `a_sign_in_window_panel_offers_cancel`.
- [ ] **Step 2: Run them; expect failure.** Run `cargo test --lib auth::msa && cargo test -p bastion-mc-gui --lib`. Expected: FAIL.
- [ ] **Step 3: Implement.** Inject the opener as a function parameter (or a trait object) so the tests need no browser.
- [ ] **Step 4: Run them; expect them to pass.** Run the Step 2 command, then `cargo test --workspace`. Expected: PASS, 0 failed.
- [ ] **Step 5: Commit.** `sign-in: on Bastion OS the Microsoft sign-in runs in a sign-in window`

### Task 9: The foundation notes

**Files:**
- Create: `docs/notes/os-foundation.md`

- [ ] **Step 1: Write the notes.**
  - Every produced signature, exactly as built.
  - The Microsoft host list and its sources.
  - The plugin-error matcher from Task 2.
  - Every `Ruling:` this package made.
  - For Wave 1: how to call `OwnDisplay` and `signin::open`.
- [ ] **Step 2: Run the whole suite.** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check`. Expected: 0 failed; deny ok.
- [ ] **Step 3: Commit.** `docs: the launcher foundation for Bastion OS, as built`
