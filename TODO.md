# TODO — where the work stands

Snapshot of 2026-11-13. Nothing is committed yet; the whole tree is untracked.

## 1. Current task: THREAT_MODEL.md §8 "Hardening not yet built"

| # | Item | State |
|---|---|---|
| 1 | Credential broker (TLS-terminating, placeholder token in the JVM) | **done**, tested with a stand-in Mojang + real sandboxed JVM, live demo launch |
| 2 | Wayland `wp_security_context_v1` | **done**, see below |
| 3 | Nested compositor on X11 (gamescope) | **done, with open points** (below) |
| 4 | cgroup limits via transient systemd scope | **done**, OOM kill confirmed in the journal |
| 5 | Launcher self-confinement (Landlock, all threads) | **done**, live launch |
| 6 | Strict re-serialisation of `options.txt` / `servers.dat` | **done** (`src/persist.rs`) |

### 2 — Wayland security context
- Code: `src/sandbox/wayland.rs` (hand-rolled wire client), `display.rs::add_wayland`,
  `launch.rs` (context held until exit, socket name unlinked once the sandbox runs),
  config `sandbox.wayland_security_context = auto|require|off`.
- Verified: 4 unit tests (stand-in compositor); `tests/wayland.rs` against a real headless
  sway (20 privileged globals withheld, e.g. screencopy, virtual keyboard/pointer, data
  control, layer shell); real Minecraft 26.3 rendered through it (`tests/nested_game.rs`).
- Not verified: a real Wayland *desktop* session (KDE/GNOME/sway on hardware).
- Note: sway still offers `ext_*_image_capture_source_manager_v1` to sandboxed clients
  (harmless without `ext_image_copy_capture_manager_v1`, which it withholds).

### 3 — Nested compositor
- Code: `src/sandbox/nested.rs` (gamescope in its own bwrap + Landlock + seccomp sandbox,
  own systemd scope, `__display-ready` child announces the inner `DISPLAY`),
  `display.rs::use_x11`, `launch.rs` (start before the game, stop the game if the
  compositor dies, stop the compositor after the game), `Mount::RoOverlay` (tests only),
  `LandlockPolicy.socket_dirs`, config `sandbox.nested_compositor = auto|require|off`,
  `nested_compositor_args`, `nested_compositor_layers` (serde-skipped, tests only).
  Xwayland needs `setgid`/`setuid` (no-ops in the sandbox) — allowed for the compositor only.
- Verified: 5 unit tests; `tests/nested.rs` with real gamescope 3.17 (from extracted,
  signature-checked Arch packages — gamescope is not installed system-wide): inner display
  only, no session cookie, session root-window marker invisible, hardware GL; real
  Minecraft 26.3 rendered inside it on the X11 session.
- **Open:**
  - [ ] **Finding: gamescope bridges the CLIPBOARD both ways** (session → game and game →
        session, `the_clipboard_and_the_nested_compositor`). No gamescope option turns it off
        (checked `--help`; it uses SDL clipboard calls). Decide: document in THREAT_MODEL as a
        remaining channel, and/or find a way to cut it (e.g. a compositor build/flag, an
        SDL hint, or denying the selection at the X level). Not yet tested: PRIMARY selection.
  - [ ] Test bug: `tests/nested.rs:175` asserts the inner display number is not present in
        the host's `/tmp/.X11-unix`; wrong when run on a private display (inner `:0` lives in
        a private dir). Remove or rewrite that assertion.
  - [ ] Test `network.mode = "open"` + nested compositor: libxcb tries the abstract socket
        first; with abstract scoping on it may fail hard, and in the host netns `@/tmp/.X11-unix/XN`
        could be a *host* display.
  - [ ] Doctor: report "nested compositor available / not" and "security context supported /
        not" explicitly (currently only via the display plan notes).
  - [ ] PKGBUILD: `optdepends=('gamescope: isolate the game on X11 sessions')`.
  - [ ] Consider `CPUQuota=` for both scopes.

## 2. Wrap-up for the current task
- [ ] Docs: README (broker, limits, self-confinement, sanitisers, security context,
      nested compositor, new config keys), THREAT_MODEL (§5 / §8 lists, new rows, X11 row
      now "nested compositor or refuse", clipboard finding), TOKEN_INJECTION (placeholder
      token), `examples/config.toml` (`[launcher] confine`, `sandbox.cgroup/memory_max_mb/
      tasks_max/wayland_security_context/nested_compositor*`, `[network.broker]`),
      `src/auth/mod.rs` header ("launcher holds no credential" is false with the broker).
- [ ] Verification pass: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`,
      `cargo test --workspace` (+ `BASTION_NET_TESTS=1`), GUI live test, release build,
      `makepkg` from a tarball (recipe in the e2e memory note).
- [ ] Remove `target/third-party/` when done (signed Arch packages extracted for testing).
- [ ] Move the test helper scripts (`sway-headless.sh`, `private-x11.sh`, `gamescope-explore.sh`)
      out of `target/` into `tests/support/`, plus a fetch script that checks each package
      against the pacman sync DB hash and its signature. Today a `cargo clean` deletes the
      only way to re-run the display tests.
- [ ] First git commit (only when you say so). Everything is untracked, so one bad
      command could lose all of it.

### Test recipes for the display work
All third-party binaries run inside bubblewrap from `target/third-party/pkgs/*`.
```sh
# headless Wayland compositor (socket: $RT/wayland-1)
target/third-party/scripts/sway-headless.sh "$RT" [pixman|gles2] &
BASTION_WAYLAND_TESTS=1 WAYLAND_DISPLAY=$RT/wayland-1 BASTION_WAYLAND_COMPOSITOR=$RT/wayland-1 \
    cargo test --test wayland
# invisible private X display: link $RT/x11/X0 to a spare number
target/third-party/scripts/private-x11.sh "$RT" & ln -s "$RT/x11/X0" /tmp/.X11-unix/X77
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_NESTED_TESTS=1 BASTION_NESTED_CLIPBOARD_TEST=1 \
    BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt) cargo test --test nested -- --test-threads=1
# real game (opens a window unless on the private display)
BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=vanilla-latest \
    BASTION_NESTED_LAYERS=... cargo test --test nested_game -- --nocapture
```

## 3. Paused phase-2 work (from the request before the GUI)
Done since: credential broker, `servers.dat`/`options.txt` sanitisers.
- [ ] Multiple account profiles (registry, `--account`, per-account age vaults, GUI picker)
- [ ] Skin manager and cape viewing (read-only profile, cape selection; textures via the
      existing gateway)
- [ ] Dedicated `mc-vault` system account + `kernel.yama.ptrace_scope=3` guidance (docs)
- [ ] Persistent-keyring test

## 4. New proposals (2026-11-13) — not started, with first assessment

1. **Hardware-backed vault key (TPM 2.0 sealing / FIDO2 hmac-secret).**
   Cheapest route: age plugins (`age-plugin-tpm`, `age-plugin-yubikey`,
   `age-plugin-fido2-hmac`) behind the existing age vault, instead of linking tss-esapi.
   Stops offline/disk extraction and silent decryption. It does *not* stop a same-UID
   attacker who waits for the next launch and reads the token from the launcher (that is
   what the `mc-vault` account + ptrace_scope item addresses). PCR 0/7 sealing breaks on
   firmware/Secure Boot updates — needs a re-seal path.
2. **Login-protocol proxy (launcher performs the `join`).** The shared secret in Encryption
   Response is RSA-encrypted to the server, so the relay cannot compute the server hash by
   watching: the launcher must terminate the protocol encryption itself (be the client to
   the server, present its own key to the game, AES-CFB8 both ways, compression,
   per-version login/configuration states). Large, version-sensitive. Gain over today's
   `JoinGate` (one join per relayed connection to an allowlisted server): code in the JVM
   could no longer join with an arbitrary server hash during that window.
3. **Profile canary (cape/skin/name pinned, checked before launch).** Feasible, read-only;
   fits with the skin/cape manager. Needs an explicit "accept new state" command so the
   user's own changes are not alarms; "wipe tokens" on mismatch should be opt-in.
4. **Kernel/display leak vectors.**
   - Launcher Landlock: already built (no home reads; writes only instance dir + cache).
     Possible refinement: narrower write set (staging dir for copy-back).
   - J1 via `open_tree(OPEN_TREE_CLONE)` + `move_mount`: expected **not** to help —
     mountinfo's root field is the path inside the source filesystem however the mount was
     made. Verify before building; alternatives: serve files from memfd/tmpfs copies, or a
     separate filesystem/image with a neutral layout.
   - systemd scope: already built (`MemoryMax`, `MemorySwapMax=0`, `TasksMax`); add `CPUQuota`.
     io_uring is already denied by seccomp.
5. **Out-of-band approval for new servers/mods.** A TOTP secret on this machine is readable
   by the same user it is meant to stop, and `config.toml` is user-writable. The meaningful
   form is the mod-manifest model applied to the server allowlist: allowlist signed with a
   key that is off-machine or hardware-held (ties in with item 1).

## 5. Further additions (my suggestions)

### Not yet verified (blocked on hardware or accounts you have)
- [ ] Live Microsoft → Xbox → Minecraft sign-in. Needs your Azure client ID and Mojang's
      approval of it.
- [ ] Broker paths that use the real token against real Mojang: join, chat signing,
      attributes, blocklist. Needs a signed-in account and an allowlisted server.
- [ ] Java 8-era versions with the broker (the TLS 1.2 fallback flag), run end to end.
- [ ] A real Wayland desktop session, for both the game and the GUI. NVIDIA. A real `age`
      binary, plus age plugins if §4.1 goes ahead.
- [ ] Playability inside gamescope: mouse look and capture, fullscreen (`-f`), keyboard
      grab, refresh rate, resizing. Pick defaults for `nested_compositor_args` from that.

### Hardening gaps the recent work exposed
- [ ] **Wayland sessions whose compositor lacks `wp_security_context_v1`.** Find out which
      desktops have it. On those that don't, `auto` falls back to the session socket with a
      warning. Option: use nested gamescope there too (`--backend wayland`), the same way
      it's used on X11.
- [ ] Check gamescope's PRIMARY selection and drag-and-drop, besides CLIPBOARD (see the
      clipboard finding above).
- [ ] Landlock: the crate is pinned to ABI 9 while the kernel offers 10. Check what 10 adds
      and adopt it if it's useful.

### Assurance
- [ ] Fuzz targets (`cargo fuzz`) for every parser that reads untrusted bytes: NBT, the
      options grammar, the SNI parser, the DNS responder, the broker's HTTP and join body,
      the Wayland event reader, Xauthority, the log4j XML folding in the GUI.
- [ ] An independent review of the new security code, read fresh rather than against my
      reasoning: `egress/broker.rs`, `sandbox/wayland.rs`, `sandbox/nested.rs`, `confine.rs`,
      `persist.rs`, `sandbox/cgroup.rs`.
- [ ] Dependency audit: `cargo deny` for advisories, licences and duplicate crates, plus a
      look at what aws-lc-sys builds. Vendor the crates so `makepkg` builds offline and
      reproducibly (`cargo vendor`, `--frozen`).

### Packaging and housekeeping
- [ ] PKGBUILD: replace the placeholder source with a real release URL and a signature
      (`validpgpkeys`), and set `SOURCE_DATE_EPOCH` for reproducible builds.
- [ ] Launch logs: limit how many `launch-*.log` files each instance keeps (each is capped
      in size, but nothing limits their number).
- [ ] GUI: show which display protection is active (security context, nested compositor,
      plain X11), and whether the broker is on, next to the Play button. Today that only
      appears as console lines.
