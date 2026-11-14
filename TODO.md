# TODO — where the work stands

Snapshot of 2026-11-13, after the second pass ("finish the TODO list"). Everything below
under "Done in the second pass" is committed.

**A third round is in progress and is NOT in this tree yet.** It is being built in separate
working copies (`target/agents/`, plan in `target/agents/INTEGRATION.md`): an original
HUD-style front end, a first-party companion Fabric mod (zoom and other conveniences), a
UDP relay for voice chat, verified mod installs from Modrinth (ViaFabric, Simple Voice
Chat), the fixes for the two independent reviews, and a project website for GitHub Pages.

How to read this file: **Open** is what is left and why; **Blocked on you** needs
something only you have; **Done in the second pass** is the record of what changed, with
where it is and how it was checked; **Decided against / deferred** keeps the reasoning for
the big ideas so they are not re-argued from scratch.

Verification state at the end of the pass: `cargo fmt --check` clean, `cargo clippy
--workspace --all-targets` clean, `cargo test --workspace` green (see the count in
README "Status"), also with `BASTION_NET_TESTS=1 BASTION_X11_TESTS=1`; nested-compositor and
Wayland suites green against real gamescope 3.17 and sway 1.12; GUI live test green;
`makepkg` from a tarball builds both packages and their `check()` passes; `cargo deny
check` ok.

## Open

### Small, concrete
- [ ] **Runbook as QR codes / print view** (§8 of the old list). Not built: needs a QR
      encoder (a dependency, or ~300 lines). The text is in the package docs and in
      `bastion-mc wiki scram`.
- [ ] **`README.md` "Status" numbers** drift with every test added; re-count before a release.
- [ ] **Remove `target/third-party/`, `target/tools/`, `target/probe/`** when you no longer
      need them (all three are reproducible: `tests/support/fetch.sh`, `cargo install
      --root target/tools …`, the makepkg recipe). Left in place because the display tests
      need the first one.
- [ ] **Stale markers**: a launcher killed with SIGKILL leaves `cache/running/<pid>` and its
      scratch directory; both are swept by the next launch or by `scram` (tested). Nothing
      to do unless you want a `bastion-mc clean` command.

### Needs a design decision first
- [ ] **Wayland compositors without `wp_security_context_v1`** (Mutter/GNOME, Weston,
      Wayfire — per wayland.app's table, 2026-11-13). `auto` falls back to the session
      socket with a warning. Probed: `gamescope --backend wayland` runs fine inside the
      compositor sandbox under headless sway. **But** on that backend gamescope bridges the
      clipboard through Wayland's data-device protocol, which `native/noclip.c` (SDL calls
      only) does not cut. Options: (a) accept and document the clipboard there; (b) a
      filtering Wayland proxy in front of gamescope that withholds the data-device and
      primary-selection globals; (c) patch/flag upstream. Until decided, not wired in.
- [ ] **Playability inside gamescope** (pointer capture, full screen `-f`, refresh rate):
      needs you at the keyboard. Then pick defaults for `nested_compositor_args`.
- [ ] **Login-protocol proxy**, **signed server allowlist**, **Fabric companion mod**,
      **frozen `/usr`**: see "Decided against / deferred". Each is days of work; say which.

## Blocked on you (hardware, accounts, identity)
- [ ] **Live Microsoft → Xbox → Minecraft sign-in.** Needs your Azure client ID and
      Mojang's approval of it. First things to run then: `bastion-mc login`,
      `launch <id> --join <server>` in filtered mode.
- [ ] **Everything that needs a signed-in account**, all built and tested against scripted
      replies / a stand-in Mojang only: the broker against the real API (join, chat signing,
      attributes, block list), `skin show|set|reset|export`, `cape set`, the canary on a
      real profile, `export-proofs` with real history, multi-account with two real accounts.
- [ ] **A real Wayland desktop session** (game and GUI), **NVIDIA**, **Java 8-era versions
      end to end with the broker** (TLS 1.2 fallback flag), **a real `age` binary and
      hardware plugins** (`vault reseal`, `doctor`'s identity classification).
- [ ] **PKGBUILD**: the real `url`, your release-signing key in `validpgpkeys`, a signed
      tag. Everything else in it is done (auditable build, reproducibility notes, vendoring
      recipe, new docs installed).
- [ ] **Coverage-guided fuzzing**: `rustup toolchain install nightly && cargo install
      cargo-fuzz`, then `fuzz/README.md`. The targets compile; they have never run.
- [ ] **`mc-vault` account / `ptrace_scope=3`**: documented (`docs/MC_VAULT_ACCOUNT.md`),
      needs root; not exercised.
- [ ] **Minecraft help-centre wording in the runbook**: their site needs a browser; only
      the request-form address was checked to exist. Read it once and adjust phase 3.

## Done in the second pass

### §1 nested compositor — all open points closed
- Clipboard: `native/noclip.c` (built by `build.rs` with the system C compiler, honours
  `CC`/`CFLAGS`, embedded, `LD_PRELOAD`ed into gamescope only; removes itself from the
  environment). `tests/nested.rs::no_selection_crosses_the_nested_compositor` asserts
  CLIPBOARD and PRIMARY in both directions. Control run without it: CLIPBOARD crossed both
  ways, PRIMARY game → session.
- `network.mode = "open"`: was broken (libxcb tries the abstract socket, Landlock scoping
  refuses, no fallback). Fix: the game's `DISPLAY` is the socket path. Same fix applied to
  the direct-X11 path with a display-number-less cookie, so **abstract-socket scoping is
  now on in every mode** (the open-mode exception is gone).
- Wrong assertion in `tests/nested.rs` fixed; doctor reports "Wayland security context" and
  "nested compositor" explicitly; PKGBUILD optdepends gamescope; `CPUQuota`
  (`sandbox.cpu_quota_percent`, fixed 200 % for the compositor scope; `cpu.max` read back).

### §2 wrap-up
- Docs: README, THREAT_MODEL (rows P6/P7/J2/J10/J13/D1/D2/N1/C7, new §4.6 "the account
  itself", §5, §6, §7 E11–E15, §8 rewritten as built / not built), TOKEN_INJECTION
  (placeholder table), `examples/config.toml` (every new key; a test asserts it parses to
  exactly the defaults), `src/auth/mod.rs` header, new `docs/MC_VAULT_ACCOUNT.md`.
- Helper scripts in `tests/support/`; `fetch.sh` fails closed on hash or signature.
- Fixed along the way: `tests/egress.rs` live test (its sandbox had no trust store —
  pre-existing failure with `BASTION_NET_TESTS=1`).

### §3 phase 2
- Multi-account: `auth::registry`, `account list|default`, `Accounts` event, GUI picker,
  per-account age keys (`[auth.age.accounts.<label>]`).
- Skins/capes: full profile model, `skin show|set|reset|export`, `cape set`; uploads are
  rebuilt from image chunks only.
- `docs/MC_VAULT_ACCOUNT.md`; `tests/keyring.rs` (persistent keyring across sessions).

### §4 proposals
- 4.1 hardware vault key: age-plugin route — `vault reseal`, `identity_kind` + doctor line
  (hardware-backed vs software key), README guidance incl. a second recipient.
- 4.3 profile canary: `auth::canary`, `canary show|accept`, `auth.canary`, `canary_wipe`.
- 4.4: `CPUQuota` done; J1 via `open_tree` not pursued (expected not to help, see old note).
- 4.2 and 4.5: deferred, below.

### §5
- Landlock ABI 10: UDP rules for the sandbox (`landlock::restrict_udp`). Not for the
  launcher's own confinement (its DNS responder answers to ephemeral ports).
- Fuzzing: `src/fuzzing.rs` harnesses for all listed parsers + profile/PNG;
  `tests/robustness.rs` (stable, mutation-based; 300 000 iterations per harness passed);
  `fuzz/` cargo-fuzz targets (compile-checked only); GUI log folding has its own test.
- Independent review: two fresh reviewers; results and what was changed are in the
  section "Independent review" below.
- Dependency audit: `deny.toml`, `cargo deny check` ok; aws-lc-sys ships no prebuilt
  objects. PKGBUILD: `cargo auditable`, vendoring recipe, reproducibility notes.
- Launch-log retention (20 per instance); GUI protection indicator (`Protection` event).

### §6
- Anti-fingerprinting: interface/MAC test added; hostname/machine-id were already synthetic.
- `--burner` launch (the "spectator" idea): offline, copies, nothing kept; verified live.
- `memfd_secret` token memory with fallback; doctor says which.
- SOCKS5 upstream (`network.launcher_proxy`), tested against a stand-in proxy.
- J13 regression matrix for the broker.
- Bytecode capability report: `profile audit`, and `profile lock` says what an updated mod
  newly refers to (`profile::capabilities`; cross-checked against javap on Fabric API).
- `cargo auditable` in the PKGBUILD.

### §7 (the parts that do not need the mod)
- Local resource/shader packs: read-only in the sandbox, `pack import|remove|list|adopt`,
  verified at launch (`profile::packs`). Verified live with a real client.

### §8 scram
- `docs/SCRAM_RUNBOOK.md` (links from Microsoft's own articles, checked 2026-11-13),
  exact-URL allowlist with tests, `scram [--yes]`, `wiki scram [--links]`,
  `export-proofs`, running-launch markers, activity log, GUI emergency panel.

## Independent review

Two reviewers read the newest security code fresh on 2026-11-13. Neither found a way for the
game to obtain the real token or to get the credential attached to a request outside the
broker's table; no critical or high findings. Six medium findings — `scram` deletes leftover
game directories before it removes credentials; the front end can report an incomplete scram
as done; the self-confined launcher can still reach the refresh token in the kernel keyring;
confinement is reported complete on Landlock ABI 8 although pathname sockets stay reachable;
pass-through Mojang connections bypass `network.launcher_proxy`; the age vault trusts a
recipients file that programs running as you can edit — and some twenty low ones. They are
being verified and fixed in the third round; none of the fixes is in this tree yet.

## Decided against / deferred, with the reason

- **Login-protocol proxy (old §4.2).** Would remove the last thing code in the JVM can do
  with the account under the broker (one join per open game connection). Requires
  terminating Minecraft's protocol encryption per protocol version. Large; also the only
  place a packet-rate tripwire could see more than sizes and timing. Not started.
- **Out-of-band approval / signed server allowlist (old §4.5).** A factor stored on this
  machine is readable by the user it guards against. Meaningful only with a key that is
  off-machine or in hardware. Design exists (mod-manifest model); not started.
- **Fabric companion mod (old §7).** Enforcement must live in the launcher; the mod would
  be interface only. The launcher-side gates exist for packs; servers and account requests
  are per-config-switch today. Per-request approval needs a prompt channel first.
- **Hardware normalisation.** GPU renderer string cannot be normalised by mounts, so the
  result stays fingerprintable. `/proc/cpuinfo` and memory size are real. Documented (§5.9).
- **Frozen `/usr` + JVM (EROFS/composefs).** Needs a root component.
- **Security-key dead man's switch**, **CET for Rust code** (nightly only), **seccomp user
  notification** (unsafe for pointer arguments), **padding of auth traffic** (rustls 0.23
  has no record-padding setting; destinations are visible anyway), **cross-registry hash
  consensus** (CurseForge needs an API key; a compromised upstream publishes the same hash
  everywhere). Not planned unless something changes.
- **Packet-rate tripwire.** Without the login proxy the relay sees an encrypted stream;
  false positives from chunk loading. Covert channels to an allowlisted server are stated
  as not defended (THREAT_MODEL §5.8).

## Test recipes for the display work
```sh
tests/support/fetch.sh                              # once; verifies hashes and signatures
RT=/run/user/$(id -u)/bmc-test; mkdir -p "$RT"      # short: UNIX socket paths are limited to 108 bytes
tests/support/sway-headless.sh "$RT/wl" gles2 &
BASTION_WAYLAND_TESTS=1 WAYLAND_DISPLAY=$RT/wl/wayland-1 BASTION_WAYLAND_COMPOSITOR=$RT/wl/wayland-1 \
    cargo test --test wayland
tests/support/private-x11.sh "$RT/x" & sleep 2; ln -s "$RT/x/x11/X0" /tmp/.X11-unix/X77
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_NESTED_TESTS=1 BASTION_NESTED_CLIPBOARD_TEST=1 \
    BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt) cargo test --test nested -- --test-threads=1
# real game, invisible (on the private display):
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=vanilla-latest \
    BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt) cargo test --test nested_game -- --nocapture
```
