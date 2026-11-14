# realms-sync — hand-over file

Package: Realms in filtered mode through a settable approval ("OTP override"), and settings
sync across instances. Brief: docs/notes/round4-contract.md (apps `realms`, `settings`) and the
round-4 brief for realms-sync. Deliverable notes: **docs/notes/realms-sync.md** (README text,
threat-model rows J20–J23, decisions, verification record, contract differences, open items).

## State (newest first)

- 2026-11-14 01:35 — **package complete**; final full-suite run recorded below. Nothing in
  progress. If continuing: only the open items of realms-sync.md §7 remain, and they belong
  to other packages (bootstrap class package: mod-platform; dispatcher: mod-platform).
- 2026-11-14 01:25 — real-launch sync check done (1.21.4 → 26.3 → 1.20.1, evidence in
  docs/notes/realms-sync-evidence/). First full workspace run: 632 passed, 1 failed — the
  TOTP approval test, whose moved clock truncated to whole seconds and could land in the step
  just used; the test clock is now an exact offset (`Approvals::advance_clock`), re-run 3×
  green. Notes written.
- 2026-11-14 01:10 — code complete: src/sync, src/realms, broker Realms parts, relay pass,
  launch wiring, CLI `realms totp-setup`, proto event, events.rs terminal text, GUI view,
  token-store `Slot`, pins, example config, fuzz harnesses.
- 2026-11-14 00:20 — research done (below); private display :85; instances created.

## Final verification (what to re-run)

All with `E=<copy>/target/e2e; XDG_CONFIG_HOME=$E/config XDG_DATA_HOME=$E/data
XDG_CACHE_HOME=$E/cache XDG_STATE_HOME=$E/state`, DISPLAY and WAYLAND_DISPLAY unset:

- `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets` no warnings.
- `cargo test --workspace --no-fail-fast` — see the last entry of "Runs" below.
- `cargo test --lib sync:: realms::` (17 + 21), `--test realms` (5), `--test robustness`
  (19; `BASTION_FUZZ_ITERATIONS=60000` for the `realms`/`sync` harnesses passed with the default
  seed and with seed 42), `-p bastion-mc-gui` (120).

## Runs

- full-test-1 (before the clock fix): 632 passed, 1 failed (see above).
- full-test-2 (final code, commit after the clock fix): `cargo test --workspace
  --no-fail-fast` — **633 passed, 0 failed, 0 ignored**, exit 0 (log not committed:
  `target/realms-research/runs/full-test-2.out`). fmt clean, clippy no warnings on the same tree.

## How the real-launch check was done (repeatable)

1. Private display: `tests/support/private-x11.sh /run/user/1000/bmc-r5/x` in the background,
   `ln -sfn /run/user/1000/bmc-r5/x/x11/X0 /tmp/.X11-unix/X85`, `DISPLAY=:85 XAUTHORITY=/dev/null`.
2. Fabric instances f1201 (1.20.1) and f1214 (1.21.4): `profile new <id> --minecraft <v>
   --fabric`, `profile lock`, `profile sign --key target/e2e/signing.key`; instance.toml
   memory_mb 2048, 480×270. (Vanilla 1.20.1 does not start: signer clash of the default-package
   bootstrap with the signed client jar — open item.)
3. Keys into the game: `target/realms-research/scripts/xkey.py <window> <keys…>` (XTEST via
   ctypes, refuses DISPLAY :0; window from `xprop -root _NET_CLIENT_LIST`). F2 = screenshot into
   the scratch root's screenshots/. In 1.21.x/26.x: Tab/arrows move the focus, Enter puts a
   slider into edit mode, arrows change it, Escape closes Options (the game saves options.txt).
   Escape on the accessibility onboarding screen also saves.
4. Synced copy: `target/e2e/data/bastion-mc/sync/options.json` (holds `fov` from f1214 now;
   delete it to start over).

## Machine facts learned

- The launcher's e2e config is `target/e2e/config/bastion-mc/config.toml` (match newer,
  allow_x11, audio none, filtered). Real launches run under gamescope on :85.
- Screen capture of the rootless Xwayland root (ffmpeg x11grab) fails; use F2 in the game.
- Research downloads in `target/realms-research/` (client jars 1.20.1…26.2, SHA-1 verified;
  scripts in `target/realms-research/scripts/`, never run from the download directory).

## Research: Realms in the client, 1.20.1 – 26.3 (from the client jars, javap)

- Hosts: `pc.realms.minecraft.net` in every family. From 1.21.8 (not in 1.21.4) also
  `java.frontendlegacy.realms.minecraft-services.net`, used *instead* when the feature flag
  `realms_in_aks` (fetched from `GET /feature/v1` on pc.realms) is set. 26.3 adds
  `java.frontend.realms.minecraft-services.net` ("frontend v2": invite codes) whose requests
  carry the raw access token in an `x-mojangauth` header. All three chain to Microsoft TLS RSA
  Root G2 / DigiCert Global Root G2 (openssl s_client, 2026-11-14).
- Authentication: `Cookie: sid=token:<accessToken>:<undashed uuid>;user=<name>;version=<game
  version>` (separator `;`, no space), built in `RealmsClient.execute`; 26.x adds headers
  `Client-Ref`, `X-NetworkProtocolVersion`, `Is-Prerelease`.
- Why it could not be brokered: pc.realms is under `minecraft.net`, a pass-through service
  domain (TLS end to end), the broker terminated only mojang.com/minecraftservices.com and only
  knew `Authorization: Bearer`; the join answer is a bare `IP:port` on a port unknown before the
  game runs, and the confined launcher connects only to ports fixed before step 9.
- Join: `GET /worlds/v1/<id>/join/pc` (connect 5 s, read 30 s) → `{"address":"IP:port",
  "resourcePackUrl", "resourcePackHash"}` (+ `sessionRegionData`). The join task retries up to 40
  times on HTTP 503 or 277, sleeping `Retry-After` seconds (0–120, else 5) — 1.20.1 and 26.3.
- Errors the client shows: `{"errorCode": n, "errorMsg": "…"}` (`RealmsError.parse`).
- Paths per family: `target/realms-research/paths-*.txt` (summarised in realms-sync.md and in
  the comments of `src/realms/table.rs`).

## Research: options.txt across families

- DataVersions (client jar version.json `world_version`): 1.20.1 3465, 1.20.4 3700, 1.20.6
  3839, 1.21.1 3955, 1.21.4 4189, 1.21.8 4440, 1.21.11 4671, 26.1 4786, 26.2 4903, 26.3 5023.
- Format changes inside the range (26.3 DataFixers): menuBackgroundBlurriness float→int
  `Math.round(x·10)` at 3945; graphicsMode split at 4651 (+ preset "custom" at 4656);
  showNowPlayingToast → musicToast at 4763; preferredGraphicsBackend reset at 4899 (not synced).
- Real files: `tests/data/options/{1.20.1,1.21.4,26.3}.txt`.
