# realms-sync: Realms through an approval you give outside the game, and settings sync

Round 4, package realms-sync (clone `r4-realms`). This file is the deliverable: text for the
README, text for the threat model, what was verified by running (and against what), what was
not, what is open. The hand-over log is `realms-sync-progress.md`.

## 1. For the README

### Realms

Realms works in the filtered network once you decide how a join is approved:

```toml
[realms]
approve = "code"   # or "totp"; "off" (the default) refuses Realms joins, as before
```

* **What the game may do with your account.** The credential broker now also answers for the
  Realms hosts. It lets the game see your Realms list, its notifications, news and
  invitations, accept or decline an invitation (or redeem an invite code, 26.3), agree to the
  Realms terms and report region pings — with your credential, which the game still never
  holds. Everything an owner does to a Realm (configure it, switch, reset, upload, download or
  restore a world, invite, op or remove players, open, close, delete) is refused before it
  leaves your computer: manage your Realm from the official client.
* **Joining needs your approval, outside the game.** When the game asks to join a Realm, the
  launcher asks Realms where its server is, but does not hand that over. It shows a prompt in
  its own output — and in bastion-mc-gui, on the Launch page — with the Realm, the server
  address and, with `approve = "code"`, a six-digit code. Type the code into the phone's Realms
  app in the game (with `approve = "totp"`: the code your authenticator app shows). You have
  120 seconds and 3 attempts; meanwhile the game shows "Connecting to the realm…". If you did
  not ask to join that Realm, do nothing: the join is refused.
* **One connection.** An approved join gives the game an address of the launcher's own, good
  for one connection within two minutes, which the launcher connects to exactly the server
  Realms named.
* **TOTP.** `bastion-mc realms totp-setup` makes a secret, keeps it in your token store (the
  kernel keyring by default) and shows it once, on the terminal, as an `otpauth://` URI and
  as base32 for your authenticator app. `--replace` makes a new one.
* **What it needs.** A signed-in account (not demo mode), the filtered network with the
  credential broker, and the phone: a Fabric instance with the companion mod. A vanilla
  instance has nowhere to type the code, so Realms joins stay refused there. If the launch
  cannot prepare the approvals (no TOTP secret stored, the dialer does not start), it says so
  and Realms joins are refused for that session; the Realms hosts are then passed through as
  before (the 1.21.8+ ones are resolvable in that session, and refuse the game's placeholder).
* **Known gaps.** A Realm with a resource pack probably fails to join: the pack's download
  address is not reachable from the sandbox. Not tried against the real Realms service (it
  needs a signed-in account with a Realm).

### Settings sync

On by default (`[sync] options = true`). The settings that mean the same in every Minecraft
version from 1.20.1 to 26.3 follow you from instance to instance: video, sound volumes, mouse
and controls, key bindings, language, chat and accessibility. Change your field of view or a
key binding in one instance, and the next instance you start has it — also when it runs
another Minecraft version: where a setting changed its form between versions (the graphics
mode, the menu blur, the "now playing" toast), it is translated exactly as Minecraft itself
translates it, or left alone.

Never synced: anything about one world or one server (last server, tutorial and first-run
flags), the resource-pack list (packs are checked per instance), privacy choices (telemetry,
server listing, presence) and this machine's devices (sound device, graphics backend). A
burner session takes no part. The phone's Settings app shows the state and can sync during a
session (`sync-now`). The companion mod's own settings (zoom key, HUD, …) are in `config.toml`
anyway — global, with per-instance overrides under `[companion]` in `instance.toml`.

The synced copy is `~/.local/share/bastion-mc/sync/options.json`. `[sync] options = false`
turns it off.

## 2. For the threat model

**Section 5, item 7** — replace "Realms servers are handed out as bare IPs and are not
reachable in filtered mode" with: "Realms: refused unless `[realms] approve` is set; then
brokered (J20) and every join needs an approval given outside the game (J21)."

### J20 — Realms through the credential broker (`[realms] approve` = code | totp)

* **Who can do what.** Code in the game can make the requests on the Realms table with your
  account: read your Realms list, notifications, news, invitations, pending invite count,
  feature flags, preferred regions, the details and backups list of a Realm you own; mark
  notifications seen or dismissed; accept or decline an invitation; redeem an invite code
  (26.3 frontend); agree to the Realms terms of service in your name; send region pings; ask
  to join (J21). It cannot manage a Realm (every owner operation is answered `403` locally,
  flagged `[SECURITY]`), cannot learn the token (the cookie is rebuilt around it on the way
  out; replies are checked for it and withheld if they contain it; an encoded reply to a
  credentialed request is withheld), and cannot use the credential on any path that is not
  exactly on the table (canonical paths only, no query strings, case-exact).
* **How.** The per-launch authority's name constraints include `realms.minecraft.net` and
  `realms.minecraft-services.net` only when Realms is brokered. The game's session cookie
  must carry the placeholder and the session's own profile (`sid=token:<placeholder>:<profile>`;
  another profile is refused and flagged); the broker writes `sid=token:<real token>:<profile>`
  and copies `user` and `version` only if they have the client's shapes; every other cookie and
  header is dropped (`Client-Ref`, `X-NetworkProtocolVersion`, `Is-Prerelease` and the broker's
  usual allowlist are kept). The 26.3 frontend's `x-mojangauth` header is replaced the same
  way, for the one request on its table. Upstream TLS goes through the launcher's own client:
  TLS 1.3, pinned to Microsoft's roots for `.minecraft.net` and (new)
  `.realms.minecraft-services.net`.
* **Residual.** With Realms brokered, code in the game can accept a pending invitation (from
  someone who invited you), redeem an invite code it knows, and agree to the Realms terms in
  your name. It sees your Realms list. Off by default.

### J21 — Approving a Realms join, and the one-time pass

* **Who can do what.** Code in the game can ask to join any Realm you have access to. Before
  your approval it gets "wait" (`503`, `Retry-After: 3`) and nothing else: the server's address
  stays in the launcher; the pass address leads nowhere (verified: connection reset). It can
  type into the phone like you can — but not produce the code: in `code` mode the code exists
  only in the launcher's output and the front end; in `totp` mode only in your authenticator.
  After an approval it gets one connection, within 120 s, to exactly the address Realms named
  (public addresses only), and like any game connection one server join (J13).
* **Bounds.** A prompt lives 120 s and takes 3 attempts; one prompt at a time; at most 20
  prompts per session, 10 s between a refusal and the next. TOTP: one step of drift, a used
  step is never accepted again (also not in the next launch: the last step is kept in the
  cache), at most 10 wrong codes per session, then Realms joins are refused until the next
  launch.
* **Residual.** (1) In `totp` mode a code is not bound to a prompt: code in the game that
  reads what you type into the phone can cancel your prompt, start a join to another of your
  Realms and spend your code on that (`code` mode binds the code to the prompt it was shown
  with). (2) The approval is only as good as your reading of the prompt: the launcher shows
  where the game would connect; it cannot know whether you meant to. (3) The phone's Realms app
  is shown the Realm's name and address (the contract's state), so the game learns the server's
  address — it still cannot connect there except through the pass. (4) The code appears on
  your terminal and in the front end; whoever can read those can approve one join.

### J22 — The Realms dialer

* **Why it exists.** The confined launcher may connect only to TCP ports fixed before step 9;
  a Realm's port is known only when Realms answers the join. Rather than widen its own
  confinement, the launcher starts (before step 9, only when Realms is brokered) a helper —
  itself again, `__realms-dial`, in a sandbox of its own (bubblewrap with the host network, no
  files of yours, Landlock: connect anywhere, bind nowhere, no UDP, no file writes; the
  system-call filter) — and receives the one connected socket per approved join over a UNIX
  socket (`SCM_RIGHTS`).
* **Who can do what.** Only the launcher talks to it. It refuses addresses that are not
  public, makes at most 8 connections per session and nothing else.
* **Residual.** A launcher taken over by an exploit could use it for up to 8 connections to
  public addresses on ports its own confinement does not allow. With `network.launcher_proxy`
  set, the broker's requests to Realms go through the proxy like its other requests, but the
  connection to a Realm's server is made directly by the dialer — like connections to game
  servers, which are never proxied either.

### J23 — Settings sync

* **Who can do what.** Code in the game of one instance can change, through what it leaves in
  its `options.txt`, the listed settings of your other instances: bind a key elsewhere, change
  a volume, the field of view, the language. Nothing else crosses: a setting must be on the
  list and its value must have the shape the game writes for that setting (a boolean, a
  number, a short identifier in quotes, a language code, a key name) — never free text, never
  a resource pack, never a server. What the game wrote has already been through the options
  sanitiser (`persist`) before sync reads it.
* **How.** The synced copy is the launcher's file (mode 0600), read with bounds (1 MiB, 4096
  settings, every entry re-checked) and written under a lock by write-then-rename. The confined
  launcher gets write access to its directory (and nothing more) for the sync at exit. A burner
  session neither reads nor writes it.
* **Residual.** Last writer wins per setting: two instances changing the same setting in
  overlapping sessions keep the later one. A Minecraft version newer than 26.3 that changes a
  setting's format would get the old form, and fall back to its default for that setting.

## 3. Decisions

* **Default `approve = "off"`.** The contract lists Realms approval among what is off until
  configured, and rightly: approving needs an account (demo mode has none), the phone (Fabric
  with the companion), and — the real reason — a decision to let the game use your account for
  Realms at all (J20 is new surface: invitations, the terms, your Realms list). `"off"` leaves
  everything as it was before this round. `"code"` is the recommended setting once you want
  Realms: nothing to set up, and the code is bound to the prompt.
* **The address of the pass** is `127.127.255.254:25565`: the top of the range of synthetic
  addresses (the resolver hands them out from the bottom and stops at 4096) and the default game
  port, for which the relay gets a listener when Realms is brokered.
* **The approval is asked after Realms has answered**, so that the prompt can name the server
  address (the brief asks for it). Asking Realms for a join has no effect beyond the answer
  (and possibly waking a sleeping Realm).
* **What is synced** is an explicit allowlist (`src/sync/keys.rs`) with a test that every
  setting in real files of 1.20.1, 1.21.4 and 26.3 is either on it or on the "never" list.
* **Translations** are exactly Mojang's own data fixers (26.3's `DataFixers`), in both
  directions where an exact inverse exists: `menuBackgroundBlurriness` (×10 at DataVersion
  3945), `graphicsMode` ↔ `cutoutLeaves`/`weatherRadius`/`vignette`/`improvedTransparency` +
  `graphicsPreset "custom"` (4651/4656), `showNowPlayingToast` ↔ `musicToast` (4763). Key
  bindings keep their format across 1.20.1–26.3 (real files). A setting a family does not have
  is not added to an existing file; an instance without an `options.txt` gets one with the
  synced settings and its game's DataVersion (from the verified client jar's `version.json`).

## 4. Verified, and how

By running, in this clone (`cargo test`, DISPLAY unset, XDG into `target/e2e`):

* `src/sync` — 17 unit tests, among them merges into the **real** `options.txt` files of 1.20.1,
  1.21.4 and 26.3 (`tests/data/options/`, written by the game itself on the private display),
  translations both ways, last-writer-wins, burner/off/not-carried, a hostile `options.txt`,
  concurrent updates under the lock (8 threads), the DataVersion from a jar (and the real 26.3
  jar: 5023).
* `src/realms` — 21 unit tests (and one in `auth::store` for the slot): the endpoint table for every family's paths, owner operations
  refused, spellings and queries, the cookie rebuild and its refusals, the join answer and its
  rewrite, the approvals (code, expiry at 120 s, 3 attempts, cooldown, one prompt at a time,
  prompt budget, the pass's window and single use, TOTP replay in-session and across launches,
  the TOTP lock), **RFC 6238 appendix B vectors (SHA-1)** and RFC 4226 appendix D, HMAC with a
  long key (RFC 2202), base32 (RFC 4648), the phone's `realms` app (the code never reaches its
  state), the token-store slot (against the existing age stand-in; `none` refuses).
* `tests/realms.rs` — 5 end-to-end tests against a **stand-in Realms service over TLS** (own CA,
  scripted answers) with the real broker: the rebuilt cookie and dropped headers/cookies, the
  frontend's `x-mojangauth`, another profile flagged, owner operations never leaving, demo mode;
  a join held (503 + `Retry-After: 3`, Realms asked once), approved with the code, answered with
  the pass address; **the real dialer in its sandbox** connecting to a stand-in Realm server
  (an echo server) and refusing `10.1.2.3`; wrong codes, expiry, Realms' own 503 passed
  through. The last test runs **the real relay** with a Python client in a private network
  namespace (the game's side): before the answer `ECONNRESET`, the approved connection `ping`,
  a second connection `ECONNRESET`, the server itself unreachable from inside.
* `tests/robustness.rs` — harnesses `realms` and `sync` (4000 mutations each per run; also
  60 000 with two seeds), plus their starting points. cargo-fuzz targets
  `fuzz/fuzz_targets/{realms,sync}.rs` (not run: no nightly toolchain on this machine).
* `-p bastion-mc-gui` — 120 tests, among them the new prompt (code shown in two groups, the
  countdown, the end) and the layout check of the new state at four window sizes; snapshot
  `target/gui-snapshots/launch-realms-approval-*.png` looked at.
* `-p bastion-mc-proto` — the `realms_approval` event round-trips; its wire form is fixed.
* Unchanged suites still green: `tests/broker.rs`, `tests/egress.rs`. Full workspace run on
  the final code: `cargo test --workspace --no-fail-fast` — 633 passed, 0 failed (an earlier run
  had 1 failure: a TOTP test whose test clock truncated to whole seconds; fixed, see the
  progress file). `cargo fmt --all -- --check` clean, `cargo clippy --workspace --all-targets`
  without warnings.

By running the real game (demo mode, private display :85, Fabric 0.20.1 instances `f1201` and
`f1214` signed with `target/e2e/signing.key`, vanilla `vanilla-latest` 26.3; evidence in
`docs/notes/realms-sync-evidence/`):

1. `f1214` (1.21.4): "settings sync: on — nothing synced yet"; in the game Options → FOV raised
   to 73 with the keyboard (XTEST on the private display), Options closed → the game wrote
   `fov:0.075`; at exit, by the **confined** launcher: "settings sync: 1 setting(s) you changed
   now apply to your other instances: fov"; synced copy `fov 0.075, data_version 4189, f1214`.
2. `vanilla-latest` (26.3): "settings sync: 1 setting(s) taken from the synced copy (last set
   in instance "f1214"): fov"; the game, rewriting its options when the onboarding screen was
   left, kept `fov:0.075` (it had loaded it); its own change (`onboardAccessibility`, not on the
   list) was not synced back. The confinement line names the synced copy's directory.
3. `f1201` (1.20.1): "1 setting(s) taken … fov"; nothing changed, nothing synced back.

The real `options.txt` of 1.20.1 and 1.21.4 were obtained the same way (Escape on the
accessibility onboarding screen makes the game save); each passed the launcher's sanitiser
byte-identical.

## 5. Not verified

* **The live Realms service** — blocked on the user: it needs a signed-in account (the user's
  Azure client ID and Mojang's approval, see the TODO) and a Realm. Everything about Realms
  was run against the stand-in only: the endpoint table comes from the clients' own
  `RealmsClient` classes (javap, 1.20.1 … 26.3), not from watching real traffic. In particular
  not known: which ports Realms servers listen on (hence the dialer), whether Realms answers
  the rebuilt cookie exactly as the client's, and whether a real Realm server accepts the
  session join made through the broker for a pass connection (it should: it is the J13 join).
* **The phone side**: the Java phone (mod-ui) and the dispatcher (mod-platform) are other
  packages; here the apps were driven through `PhoneApp` directly. The launch holds the apps
  (`_phone_apps`) for the dispatcher to take over.
* **A real launch with `[realms] approve` on**: impossible without an account (demo mode
  turns Realms off, by design). The launch wiring (`prepare_realms`, the dialer started before
  step 9, the broker with Realms) compiles and its parts are tested; the whole was not run.
* `realms totp-setup` against the real kernel keyring: deliberately not run (the keyring is the
  user's); the slot logic was tested against the age stand-in and `token_store = "none"`.
* A burner launch on the real game (unit-tested only).
* cargo-fuzz (no nightly toolchain); `cargo deny` (no new crate was added).

## 6. Where this package needed the contract to differ

* **Token store**: `auth::store::Slot` (+ `TokenStore::{load,save,delete}_slot`) for a secret
  that is not an account's refresh token (the TOTP secret); slot names start with `_`, which no
  account label can. Other packages that store secrets (Spotify) can add a variant.
* **`src/main.rs`**: one dispatch line for `__realms-dial` (like the other re-exec modes).
* **`src/events.rs`**: the new event's terminal text in human mode (shown even with `--quiet`).
* **`src/net/tls.rs`**: `.realms.minecraft-services.net` added to the pinned Microsoft hosts.
* **The broker's authority**: `Authority::generate_for(realms)`; `Broker::with_realms`,
  `Broker::terminates` (the relay now asks the broker instance which names it terminates).
* **`egress::Policy::with_realms`**: Realms hosts resolvable, a listener for the pass port.
* **Phone state, `settings`**: `keys` and `instances` are counts; `last_sync` is seconds since
  the epoch; added `last_instance`, `changed_this_session`. **`realms`**: added `locked` (TOTP
  lockout) and, when approvals are unavailable, `error`.
* **Approval event**: `Event::RealmsApproval(RealmsApproval { world, address, mode, code?,
  expires_in, attempts_left, state })`, `RealmsMode`, `ApprovalState` in `crates/proto`.

## 7. Open

* **Vanilla 1.20.1 – 1.21.11 do not start at all** (found while getting the test files, not
  this package's to fix): the client jars of the obfuscated versions are signed, their classes
  live in the default package, and so does `BastionBootstrap` (unsigned) — the JVM refuses
  ("class "ezz"'s signer information does not match signer information of other classes in the
  same package"). Fabric instances of those versions start (Knot loads the game classes). Fix:
  move the bootstrap class into a package (java/BastionBootstrap.java, build.rs, `lib.rs`
  `BOOTSTRAP_CLASS_NAME`, the mount path in launch.rs) — owned by mod-platform/build.rs.
* Realms resource packs (the pack URL host is not reachable from the sandbox).
* An approval from the front end (for vanilla instances) would need the front end to send
  something other than `stop`; not done (the protocol is deliberately one-way).
* `sync-now` and the phone apps wait for the dispatcher (mod-platform).
