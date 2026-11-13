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

## 6. More proposals (2026-11-13, second batch) — not started, with first assessment

Each entry is the idea, what already exists, and the catch I see. "Check" marks a claim I
haven't verified yet.

### Supply chain of modpacks
- [ ] **Cross-registry consensus at authoring time** (`profile lock`): compare each jar's
      SHA-256 against Modrinth, CurseForge and GitHub Releases. Flag jars found on only one
      source, or uploaded under 48 h ago (freshness quarantine). Opt-in, and only when
      authoring: the launcher itself still contacts nobody. Catch: CurseForge's API
      requires an API key (check its terms). A compromised upstream account publishes the
      same bad hash everywhere it is listed.
- [ ] **Bytecode capability diff on mod updates.** Parse the class constant pools and diff
      references to `Runtime`/`ProcessBuilder`, `Unsafe`, sockets and URL connections,
      reflection, `System.load*`, `MethodHandles`. Point at the exact class. Must recurse
      into jar-in-jar (Fabric `META-INF/jars`) and flag bundled native libraries. Catch:
      reflection over built strings, invokedynamic and class loading from bytes hide
      capabilities from a static scan. It's a signal for review, not proof.

### Anti-fingerprinting
- [ ] Hostname and MAC address: largely done for `filtered` and `none` network modes. The
      UTS hostname is a fixed `bastion`, the same for every user, and `/etc/machine-id` is
      random per launch. The private netns holds only `lo`, so there is no hardware MAC to
      see. `open` mode shares the host's interfaces; document it, or refuse it for
      fingerprint-sensitive instances. Add a test that `NetworkInterface` enumeration
      inside the sandbox shows only loopback.
- [ ] Hardware normalisation: CPU count via cpuset or affinity in the scope; a synthetic
      `/proc/cpuinfo` and `/proc/meminfo` bound over the real ones (check that bwrap allows
      this on a fresh procfs); JVM properties are already ours. Catch: the OpenGL/Vulkan
      renderer string comes from the real driver through `/dev/dri` and can't be normalised
      by mounts. Telemetry carrying hardware info is already refused by the broker's
      defaults.

### Physical presence
- [ ] **Security-key dead man's switch:** watch udev, through a netlink socket opened
      before self-confinement, for removal of a configured key. On removal, kill the
      sandbox, drop the broker's token and exit. Catch: the confined launcher cannot run
      `loginctl lock-session`, so locking needs D-Bus (`org.freedesktop.login1`) set up
      before confinement. Killing the game does **not** revoke the minted Minecraft token
      at Mojang. With the broker, though, the token never left the launcher, so it dies
      with the process.

### Binary and kernel hardening
- [ ] **Shadow stack (CET) for the launcher:** a nightly-only release profile with
      `-Z cf-protection`. Every linked object must be marked, including AWS-LC's assembly,
      and glibc's tunables must enable it at run time. Check whether user-space IBT is
      supported by the kernel at all (I believe only shadow stacks are). MTE is ARM-only
      and irrelevant on x86.
- [ ] **Seccomp user notification:** limited value here. The kernel documentation warns it
      must not be used to decide security policy based on pointer arguments (paths,
      sockaddrs) because of TOCTOU races. Landlock already enforces paths and TCP ports.
      Possible use: logging or tripwire decisions on syscalls whose arguments are plain
      integers.

### Zero-credential mode
- [ ] **`--spectator` / burner launch:** demo mode already mints no token. Add one flag
      that also forces `network.mode = "none"`, a throwaway copy of the instance (nothing
      persisted back), and no broker. Meant for replays, untrusted maps and resource packs.
      Catch: full (non-demo) play without a session would sidestep Mojang's ownership
      check. Allow it only for an account verified as owning the game at its last sign-in,
      and pass only a placeholder token.

### Secrets in memory
- [ ] **`memfd_secret` for the refresh token and the session token:** this removes those
      pages from the kernel's direct map, which protects against `/proc/kcore` reads and
      kernel bugs that leak through the direct map. Correction to the proposal: it does
      **not** stop root or kernel modules outright, cold boot, or DMA without an IOMMU (the
      pages are ordinary cleartext RAM). Check: secretmem may be disabled by default and
      need `secretmem.enable=1`; fall back to the current mlock + wipe. Copies inside
      rustls/hyper buffers stay outside our control.

### Network metadata
- [ ] **Padding auth traffic:** low value. The observer already sees the destination
      hosts (IP and SNI) of every sign-in step, and rustls offers no TLS record-padding
      control (check). Not planned unless that changes.
- [ ] **Tor/SOCKS5 upstream for launcher-side traffic:** route sign-in (and, for full
      effect, the broker's upstream calls) through a local SOCKS5 proxy, resolving names on
      the proxy. Pins and TLS 1.3 stay end to end. Catch: Microsoft may challenge or block
      Tor exits. Multiplayer servers still see your real IP, so only Microsoft/Mojang's
      logs are decoupled.

### Exfiltration through allowed endpoints (J13)
- [ ] Skin/cape/profile uploads from the sandbox are already refused: the broker denies
      every state-changing request that isn't on its allowlist. Add explicit regression
      tests for `POST/PUT/DELETE /minecraft/profile/skins|capes|name`.
- [ ] **Packet-rate tripwire on game connections:** after login the Minecraft stream is
      encrypted, so the relay sees only sizes and timing. Inspecting payloads needs the
      login-protocol proxy (§4.2). Expect false positives (chunk loading, resource packs).
      Covert channels inside chat or movement to an allowlisted server can't be closed by
      the launcher; record that in the threat model.

### Build attestation
- [ ] `cargo auditable build` in the PKGBUILD, which embeds the dependency list in the
      binary. This joins the reproducible/vendored build items in §5: `--frozen --offline`,
      `SOURCE_DATE_EPOCH`, then compare hashes from two independent builds.

### Frozen runtime
- [ ] **Verified, immutable `/usr` and JVM for the sandbox (EROFS or composefs + signed
      digest):** mounting EROFS isn't allowed in a user namespace, so this needs a root
      part, against the unprivileged design. A pacman hook could build the image after each
      transaction, with a systemd mount unit. Cheaper partial step: copy the JVM home into
      the verified store (fs-verity) and bind it by descriptor. Note: pacman replaces files
      by rename, so open or mapped libraries keep their old version; the real risk is a
      mixed-version set of later `open`s mid-session.

## 7. Security companion mod for Fabric instances (proposal 2026-11-13) — not started

**Idea:** a small Fabric mod, added to every Fabric instance by the launcher, that talks to
the launcher. Risky actions in the game would need a second factor (one-time password,
hardware-key touch, or phone confirmation) before they happen: installing or accepting
resource packs, and other concerning actions.

**The design constraint that decides everything:** the mod runs inside the same JVM as the
mods it is meant to guard against. Any other mod can patch it (Mixin), call around it
(reflection), fake its prompts, or read an OTP as it is typed into the game window. So:
- **Enforcement lives in the launcher, outside the sandbox.** The mod is only the
  in-game *interface*: it announces "the game wants X", and shows "waiting for approval"
  and the result. If a hostile mod disables it, the action is still blocked, because the
  launcher never received an approval.
- **The second factor is never entered inside the game.** Use a launcher or GUI prompt,
  a security-key touch, or a phone. A TOTP secret kept on this machine is readable by the
  same user it guards against (see §4.5), so it only adds friction. Hardware-backed
  factors (FIDO2 user presence, §4.1) are the meaningful ones.

### What the launcher can actually gate
- [ ] **Server resource packs:** the download goes through the relay. Hold the connection
      (or refuse it) until approved, and record the pack's hash. Approved packs are cached
      in the verified store and served from there afterwards.
- [ ] **Local resource packs and shaders:** `resourcepacks/` is writable by the game today.
      Make it read-only in the sandbox, add packs through a launcher import command with
      approval and hashing, and verify them at launch, like `mods/`.
- [ ] **Joining a server not on the allowlist:** a request to extend the session's
      allowlist, with approval; persistent additions need the signed allowlist (§4.5).
- [ ] **Account-affecting requests through the broker** (friends, settings, reports,
      anything currently refused by default): approve per request instead of only
      per config switch.
- [ ] **Clickable chat links:** the game can't open them inside the sandbox. Forward
      them to the launcher, which shows the full URL and opens it only after confirmation.

### Mod ↔ launcher channel
- [ ] One UNIX socket bound into the sandbox (Landlock `unix_sockets`), a small
      line-based protocol. Every message from the mod is **untrusted**: sanitised,
      length-limited, rate-limited, and never able to approve anything itself.
- [ ] Prompts are shown by the launcher side: a desktop notification, the GUI, or the
      terminal. They name exactly what will happen (host, pack hash, server).
- [ ] Optional for MVP: the mod's in-game screens. Without the mod the system still works;
      blocked actions just fail with a launcher-side message.

### Shipping the mod
- [ ] Build it from source in this repository, like `BastionBootstrap.java`, with no
      dependency beyond Fabric Loader (no Fabric API). Hash it at build time and bind it
      into the sandbox by descriptor, outside the signed `mods/` set, so curated manifests
      stay exactly as signed.
- [ ] Maintenance cost: Mixin targets change with Minecraft versions, so it needs a
      version matrix and tests per supported release. Vanilla instances can't load mods,
      so there the launcher-side gates apply without the in-game interface.
