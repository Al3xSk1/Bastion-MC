# Public sign-in Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users sign in through bastion-mc's own Microsoft application when they have not
registered their own, while their own `[auth] client_id` keeps working and wins.

**Architecture:** One new module, `src/auth/client.rs`, decides which application a sign-in
goes to (own over public over none) and owns every user-facing sentence about it; every place
that read `cfg.auth.client_id` asks it instead. The event protocol's `Account` gains an
optional `app` field so the front end can say which application is in use.

**Tech Stack:** Rust (launcher crate, `crates/proto`, the iced front end in `gui/`), serde,
the site generator in Python (`site/build.py`) for the documents' quotes.

**Spec:** `docs/superpowers/specs/2026-11-14-public-signin-design.md`

## Global Constraints

- `PUBLIC_CLIENT_ID` is `None` in this change; the owner supplies the approved ID later as a
  one-line edit. Everything must behave correctly with `None` today and with `Some(id)` later.
- `[auth] client_id`, when set, always wins ("your own app"); bring-your-own is unchanged.
- The browser flow with PKCE stays the default; the device-code flow stays opt-in (`--device`).
- No change to the token chain's requests, the token store or the broker.
- The event protocol stays backward compatible both ways: the new field is
  `#[serde(default, skip_serializing_if = "Option::is_none")]` (the protocol has no
  `deny_unknown_fields`); `client_id_configured` keeps its meaning (true for either app).
- The front end handles no credential (a client ID is public; it is shown, never stored there).
- No new dependency. `examples/config.toml` unchanged (`client_id` stays commented out).
- User-facing names: "bastion-mc's app" and "your own app".

## Review Focus

- Minecraft's "Invalid app registration" with the **public** app: the message must not tell
  the user to get "your Azure application" approved (Task 1 and Task 3 pin both texts).
- A front end older than this change receiving `app`, and this front end receiving an
  `Account` without it (old launcher): both parse (Task 2's tests).
- `PUBLIC_CLIENT_ID` set later to something that is not a UUID-shaped client ID: a test fails
  (Task 1).
- `status`, `doctor`, the login line, the launch refusal and the front end agree on which app
  is used, because they all call `client::effective` (Task 3, Task 4).
- An own `client_id` set while a public one exists: the own one is used everywhere, including
  token refresh at launch (Task 1's selection test; Task 3 routes refresh through it).

---

### Task 1: The selection, its sentences, and the error that depends on it

**Files:**
- Create: `src/auth/client.rs`
- Modify: `src/auth/mod.rs` (add `pub mod client;`; `authenticate` and `session_from_msa`
  take the client kind), `src/auth/minecraft.rs:162-184` (`login_with_xbox` takes the kind)
- Test: unit tests in `src/auth/client.rs`; the existing "Invalid app registration" test in
  `src/auth/mod.rs` (around line 260) extended to both kinds; `src/config.rs:928` keeps its
  assertion (the default configuration names no client ID) with the message changed to
  `"the default configuration names no client ID; the public one is auth::client::PUBLIC_CLIENT_ID"`

**Interfaces:**
- Produces:
  - `pub const PUBLIC_CLIENT_ID: Option<&str> = None;`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum ClientKind { Own, Public }`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Client<'a> { pub id: &'a str, pub kind: ClientKind }`
  - `pub fn effective(auth: &AuthConfig) -> Option<Client<'_>>` (uses `PUBLIC_CLIENT_ID`)
  - `pub fn effective_with<'a>(auth: &'a AuthConfig, public: Option<&'a str>) -> Option<Client<'a>>`
  - `pub fn describe(client: Option<Client<'_>>) -> String` — one sentence for status/doctor
  - `pub fn none_message(config_file: &Path) -> String` — the refusal text when there is no client
  - `pub fn not_approved_message(kind: ClientKind) -> String`
  - `pub async fn authenticate(api, client: Client<'_>, refresh_token, allow_game_pass)` and
    `pub async fn session_from_msa(api, msa_access, allow_game_pass, kind: ClientKind)` in
    `src/auth/mod.rs`; `login_with_xbox(api, xsts, kind: ClientKind)` in `minecraft.rs`.

- [ ] **Step 1: Write the failing tests** in `src/auth/client.rs`

```rust
#[test]
fn your_own_app_wins_over_the_public_one() {
    let own = AuthConfig { client_id: Some("11111111-2222-3333-4444-555555555555".into()), ..AuthConfig::default() };
    let c = effective_with(&own, Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")).unwrap();
    assert_eq!((c.id, c.kind), ("11111111-2222-3333-4444-555555555555", ClientKind::Own));
}
#[test]
fn the_public_app_is_used_when_no_own_app_is_set() {
    let c = effective_with(&AuthConfig::default(), Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")).unwrap();
    assert_eq!(c.kind, ClientKind::Public);
}
#[test]
fn without_either_there_is_no_client_and_the_message_names_both_ways() {
    assert!(effective_with(&AuthConfig::default(), None).is_none());
    let m = none_message(Path::new("/x/config.toml"));
    assert!(m.contains("no public bastion-mc app") && m.contains("auth.client_id") && m.contains("--demo"));
}
#[test]
fn the_built_in_public_id_if_any_is_shaped_like_a_client_id() {
    if let Some(id) = PUBLIC_CLIENT_ID { crate::config::validate_client_id(id).unwrap(); }
}
#[test]
fn not_approved_speaks_of_whose_app_it_is() {
    assert!(not_approved_message(ClientKind::Own).contains("your Azure application"));
    let public = not_approved_message(ClientKind::Public);
    assert!(public.contains("bastion-mc's application") && !public.contains("your Azure application"));
    assert!(public.contains("AppRegInfo") || public.contains("Mojang"));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --lib auth::client`
Expected: FAIL (module does not exist)

- [ ] **Step 3: Implement `src/auth/client.rs`** with the interfaces above; make
  `crate::config::validate_client_id` `pub(crate)` if it is not. Exact copy:
  - `none_message`: `"No Microsoft client ID: this build has no public bastion-mc app yet and auth.client_id is not set in {path}. Register your own Azure application (README, \"Signing in\"); until then only `launch --demo` works."`
  - `describe`: `"bastion-mc's app (client ID {id})"`, `"your own app (client ID {id}, from auth.client_id)"`, `"none (only demo mode)"`.
  - `not_approved_message(Own)`: the current text of `minecraft.rs` unchanged.
  - `not_approved_message(Public)`: `"Minecraft services refused bastion-mc's application (\"Invalid app registration\"): Mojang has not approved it yet, or has withdrawn the approval (https://aka.ms/AppRegInfo). Nothing on this machine can change that; until it is approved, sign in through your own Azure application (README, \"Signing in\") or play in demo mode."`

- [ ] **Step 4: Thread the kind through the chain**: `authenticate(api, client, …)` calls
  `msa::refresh(api, client.id, …)` and `session_from_msa(…, client.kind)`;
  `login_with_xbox` returns `Error::Auth(client::not_approved_message(kind))` on
  "Invalid app registration". Extend the existing test near `src/auth/mod.rs:260` to assert
  the public text for `ClientKind::Public` and the own text for `ClientKind::Own`.

- [ ] **Step 5: Run the tests**

Run: `cargo test --lib auth`
Expected: PASS (callers in `cli.rs`/`launch.rs` fail to compile until Task 3; run Task 3
before the workspace build)

- [ ] **Step 6: Commit** — `auth: which Microsoft app a sign-in goes to (own over public), and an approval error that says whose app it is`

### Task 2: The protocol says which app

**Files:**
- Modify: `crates/proto/src/lib.rs:165-183` (`Account`), its tests near line 369
- Modify (constructors): `tests/events.rs:111`, `gui/src/app.rs:1644,2746`, `gui/src/view/tests.rs:644,1376,1562`

**Interfaces:**
- Produces: `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum SignInApp { Own, Public }` and `Account::app: Option<SignInApp>` (doc: "Which Microsoft application a sign-in goes to; absent from launchers older than this field and when there is none").

- [ ] **Step 1: Write the failing tests** in `crates/proto/src/lib.rs`

```rust
#[test]
fn an_account_without_app_still_parses() {
    let a: Account = serde_json::from_str(r#"{"account":"default","signed_in":false,"store":"kernel keyring","client_id_configured":true,"device_flow":false}"#).unwrap();
    assert_eq!(a.app, None);
}
#[test]
fn the_app_travels_as_a_lowercase_word() {
    let a = Account { app: Some(SignInApp::Public), ..Account::default() };
    assert!(serde_json::to_string(&a).unwrap().contains("\"app\":\"public\""));
}
```

- [ ] **Step 2: Run them to see them fail** — `cargo test -p bastion-mc-proto`, FAIL (no field)
- [ ] **Step 3: Add the enum and the field**; the existing JSON test (line ~375) stays
  byte-identical because `None` is skipped; add `app: None` where struct literals need it.
- [ ] **Step 4: Run** — `cargo test -p bastion-mc-proto && cargo test -p bastion-mc-gui`, PASS
- [ ] **Step 5: Commit** — `proto: an account says which Microsoft app its sign-in goes to`

### Task 3: Every launcher call site asks the selection

**Files:**
- Modify: `src/cli.rs:265` (summary: `client_id_configured: effective(..).is_some()`,
  `app: effective(..).map(|c| match c.kind { Own => SignInApp::Own, Public => SignInApp::Public })`),
  `src/cli.rs:368-369` (status prints `describe`), `src/cli.rs:905-923` (login: refuse with
  `none_message`; before the flow print `"Signing in through {describe}."` and, for Public,
  `"To use your own Azure application instead, set auth.client_id."`),
  `src/cli.rs:955-960` (same refusal; `authenticate(http, client, …)`),
  `src/launch.rs:479-483` (refusal via `none_message`), `src/launch.rs:861-866` (`authenticate` with the effective client),
  `src/doctor.rs:562-564` (`ok` for Own and Public with `describe`; `info` for none)
- Test: `tests/events.rs` (the account summary event carries `app` — `None` today); a unit
  test in `src/doctor.rs`'s test module if one exists for this section, else in `client.rs`
  for `describe`

- [ ] **Step 1: Write the failing test** — in `tests/events.rs`, assert the emitted account
  summary has `client_id_configured == false` and `app == None` with no client configured, and
  `client_id_configured == true` with `app == Some(SignInApp::Own)` when the test config sets
  `[auth] client_id`.
- [ ] **Step 2: Run** — `cargo test --test events`, FAIL until the summary sets `app`
- [ ] **Step 3: Change the call sites** listed above; no other file reads `cfg.auth.client_id`
  for signing in afterwards (check with `grep -rn "auth.client_id" src` — only config.rs and
  client.rs remain).
- [ ] **Step 4: Run** — `cargo test --workspace`, PASS; `cargo clippy --workspace --all-targets`, no warnings
- [ ] **Step 5: Commit** — `launcher: status, doctor, login, refresh and launch all use the effective Microsoft app`

### Task 4: The front end says which app

**Files:**
- Modify: `gui/src/view/accounts.rs:43-52` (no client: the new copy; with a client: one small
  line), `gui/src/view/accounts.rs:206` (unchanged logic)
- Test: `gui/src/view/tests.rs` (a view test next to the existing one at line 644)

- [ ] **Step 1: Write the failing test** — feed `Account { app: Some(SignInApp::Public), client_id_configured: true, ..account(false) }` and assert the page shows `"Sign-ins go to bastion-mc's app."`; with `Some(SignInApp::Own)` assert `"Sign-ins go to your own app."`; with `client_id_configured: false` assert the text contains `"no public bastion-mc app"`.
- [ ] **Step 2: Run** — `cargo test -p bastion-mc-gui accounts`, FAIL
- [ ] **Step 3: Implement** — copy for the unconfigured case: `"This build has no public bastion-mc app yet. Set auth.client_id in config.toml to your own Azure application to sign in; demo mode works without."`
- [ ] **Step 4: Run** — `cargo test -p bastion-mc-gui`, PASS (the layout tests included)
- [ ] **Step 5: Commit** — `front end: the accounts page says which app sign-ins go to`

### Task 5: The documents and the website

**Files:**
- Modify: `README.md:10` ("No built-in OAuth client ID…" becomes: bastion-mc's own public app
  once registered and approved, or your own), `README.md:1354-1372` (the section becomes
  "Signing in: bastion-mc's app or your own": what each is, which wins, why someone would pick
  their own, the existing four steps for one's own; and a short "For the project's owner"
  paragraph: register the app with personal Microsoft accounts only, public client flows on,
  redirect `http://localhost`, request Minecraft API access, then set `PUBLIC_CLIENT_ID`);
  `docs/THREAT_MODEL.md` (a new N row after N5: a shared public client — one registration for
  everyone that Microsoft or Mojang can block, bring-your-own unaffected; its owner can change
  the app's name and settings but never receives anyone's tokens; device-code phishing with a
  widely known client ID, why the device flow stays opt-in); `TODO.md` (Blocked on you:
  register bastion-mc's app, get it approved, send the ID; the record of this change);
  `site/src/pages/getting-started.html:130-146` and `site/src/pages/index.html:338` (the
  statements and their `{{REQUIRE}}` quotes follow the new README text)

- [ ] **Step 1: Edit the documents**, then run `python3 -I site/build.py --check`; every
  broken quote it names gets its statement re-read and re-quoted.
- [ ] **Step 2: Run** — `python3 -I site/build.py --check` (no problems) and `cargo test --test site` (PASS)
- [ ] **Step 3: Commit** — `docs: signing in through bastion-mc's app or your own`

### Task 6: Whole-tree verification

- [ ] **Step 1:** `cargo fmt --all --check`; `cargo clippy --workspace --all-targets` (no warnings); `cargo test --workspace` (fresh XDG directories, no display, no session bus): all pass; report the counts.
- [ ] **Step 2:** `grep -rn "auth.client_id" src` shows only `config.rs` and `auth/client.rs`.
- [ ] **Step 3: Commit** any count changes in README/TODO, then hand over.
