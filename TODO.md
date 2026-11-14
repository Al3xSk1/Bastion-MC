# TODO — where the work stands

Snapshot of 2026-11-13, after the second pass ("finish the TODO list"). Everything below
under "Done in the second pass" is committed.

**A third round is in progress and is NOT in this tree yet.** It is being built in separate
working copies (`target/agents/`, plan in `target/agents/INTEGRATION.md`): an original
HUD-style front end, a first-party companion Fabric mod (zoom and other conveniences), a
UDP relay for voice chat, verified mod installs from Modrinth (ViaFabric, Simple Voice
Chat), the fixes for the two independent reviews, and a project website for GitHub Pages.
(The review fixes are the exception once their branch is merged: the section "Independent
review" below, and the entries marked "review" under Open, are written for the tree that
has them.)

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
- [ ] **Texture gateway memory** (review, adjacent to A-F5): the broker's buffers now come
      out of one 64 MiB budget, but the texture gateway in `src/egress/mod.rs` still holds
      up to 4 MiB per connection (256 connections) outside it. In practice Mojang's
      texture host serves skins of a few kilobytes; the bound is what the code allows.
      Either put it under the same budget or lower the per-texture cap.
- [ ] **`network.launcher_proxy` and pass-through connections** (review B-M3, belongs to
      the egress relay): until that fix is in, README "Limits, proxy" says that
      `*.minecraft.net` connections do not go through the proxy. Remove that sentence
      with the fix.
- [ ] **`cargo deny check`** was not re-run after `flate2` became a direct dependency
      (no `cargo-deny` in the working copy the fixes were made in). `Cargo.lock` gained
      one dependency edge and no crate, so its result should be unchanged; run it once.

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
- [ ] **Signals from the confined launcher** (review, residual of A-F1): once the game runs
      the launcher can no longer read files, run programs or reach the keyring — but it
      can still signal every process of yours, because stopping the game is done with a
      signal and the game is outside the launcher's Landlock domain. Scoping signals
      (Landlock can) needs another way to stop the sandbox first: a pipe that the
      in-sandbox init watches, with the parent-death signal as the backstop. Not started.
- [ ] **Archives with a comment or in zip64 format are refused as packs** (review B-L7): the
      strict reader accepts one unambiguous shape. If a pack you care about is refused,
      the choice is between re-zipping it (what the message says) and teaching the reader
      one more shape together with a proof that `java.util.zip.ZipFile` reads it the same.

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
      hardware plugins** (`vault reseal`, `doctor`'s identity and recipient classification).
- [ ] **The root-owned recipients file** (`/etc/bastion-mc/age-recipients`, review B-M4):
      needs root and a real `age`. The launcher's side is tested with a stand-in path and a
      stand-in program; the commands are in `docs/MC_VAULT_ACCOUNT.md`.
- [ ] **A signed-in launch with the reordered start-up** (review A-F1, A-F6): the launcher now
      confines itself — keyring gone, no program execution — before it reads anything
      from the game. That nothing after that point needs the token store was established
      by reading the code; every launch run here was a demo or burner launch (no token).
- [ ] **A kernel with Landlock ABI 8 or older** (review A-F2): the partial confinement and
      its reporting are tested by asking this kernel (ABI 10) for less.
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
  rebuilt from image chunks only. (Since the review: decoded to pixels and encoded again,
  B-L10 — the chunk-level rebuild let non-image bytes through.)
- `docs/MC_VAULT_ACCOUNT.md`; `tests/keyring.rs` (persistent keyring across sessions).

### §4 proposals
- 4.1 hardware vault key: age-plugin route — `vault reseal`, `identity_kind` + doctor line
  (hardware-backed vs software key), README guidance incl. a second recipient. (Since the
  review: the claims are narrower and exact, every identity and recipient in a file is
  classified, and root can keep the recipients list — B-M4, B-L12.)
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

Two reviewers read the newest security code fresh on 2026-11-13, without building or
running anything. Neither found a way for the game to obtain the real token or to get the
credential attached to a request outside the broker's table; no critical or high findings.
27 findings: six medium, twenty low, one informational, plus one adjacent remark. The
findings are in `docs/notes/review-findings.md`; what was done about each — how it was
verified, what changed, which test covers it — is in `docs/notes/review-fixes.md`.

**Verified first.** Every finding that was looked at here holds: each defect was confirmed in
the code the reviewers read, and where it was cheap it was made to happen (Landlock alone
does not keep a process from the kernel keyring; an ABI-8 ruleset leaves pathname sockets
reachable; a token that only another login session holds survives scram on this kernel;
`java.util.zip.ZipFile` lists members the old pack check never saw; gamescope without the
cut-off bridges the clipboard). Three "plausible, not run" scenarios could not be produced
here and are fixed without having been observed: a systemd user manager without a
delegated controller (A-F7), a real compositor that takes a security context and filters
nothing (A-F8, produced with a stand-in), a real `age` recipients file being widened (B-M4).

**Fixed (25 of 27, and the adjacent one).**

- *The launcher's own confinement* (A-F1, A-F2, A-F3, A-F6): a system-call filter on all
  threads next to Landlock — no kernel keyring, no `execve`, no new namespaces, the
  tripwire list kills, the x32 and foreign-architecture entry points are closed. "Confined"
  now means the whole of it: `launcher.confine = "require"` refuses a kernel that cannot do
  all of it, `auto` says what is missing in the log, in `doctor` and in the front end's
  shield. The instance's settings files moved to `instances/<id>/persist/`, the only part
  of an instance the confined launcher can write (existing instances are migrated at the
  next launch; a burner session keeps nothing writable). The launcher confines itself
  *before* it reads anything the game sends, and reports its protections afterwards.
- *Looked at instead of assumed* (A-F7, A-F8, B-L13): resource limits are read back from
  the scope's control group; the game's Wayland socket is listed by connecting to it; the
  clipboard cut-off reports from inside gamescope. In the `require` modes a failed check
  stops the launch; in `auto` it is said and the shield is not shown as on.
- *Bounds* (A-F4, A-F5, A-adjacent, B-L11): the Wayland setup conversation, the broker's
  buffers (one 64 MiB budget), the launch log (a share per source), a burner session's copy.
- *scram* (B-M1, B-L2, B-L3, B-L5, B-L6): writers are stopped first — launches and every
  command that can store a token —, credentials are removed next, game directories last
  and for at most 15 seconds. A process is "killed" when the kernel confirmed it (pidfd),
  a token is "removed" when the kernel's key list no longer shows it, the vault is empty
  when its directory was read again; everything else is a line starting `COULD NOT` and
  exit status 1 — including markers of running commands that could not be read (not
  "nothing was running"). A second, critical read of the whole package before it was
  handed in found that, a false "started while this was running" line, the vault's
  second look missing (the module said it existed), and a garbled configuration message;
  all repaired — three with a test that failed before; the vault's second look exists for
  an entry a listing skips, which could not be arranged in a test.
- *Files the game shapes* (B-L4, A-F9): deleting the game root can no longer be stopped by
  permission bits or depth, and is only reported when it worked; persisted JSON comes out
  as printable ASCII.
- *The account* (B-L1, B-L10): the canary warns in every command and pins only what a
  command changed; a skin is decoded to its pixels and encoded again before upload.
- *Packs* (B-L7, B-L8, B-L9): a strict, bounded reader of the zip directory that refuses
  what two readers could take for different things; one set of bytes is checked, hashed
  and recorded.
- *The age vault* (B-M4, B-L12): a root-owned `/etc/bastion-mc/age-recipients` that the
  configuration cannot widen or leave; `doctor` and the documents say exactly what a device
  identity does and does not protect.
- *Wording* (A-F10): the threat model no longer understates what a mod can do with joins.

**Left, and why.**

- B-M2 (the front end reporting an incomplete scram as done) and B-M3 (pass-through
  connections bypassing `launcher_proxy`) belong to the front end and to the egress relay;
  so do the front-end halves of B-M1 and B-L2. The launcher's contract for them is
  unchanged: exit status 1 and `COULD NOT …` lines when scram is incomplete.
- The join residual itself (A-F10) needs the login-protocol proxy; see below.
- Signals from the confined launcher, the texture gateway's buffers, and the items under
  "Blocked on you" that carry the word "review": see Open.

**Tests that failed along the way for reasons of their own** (all four are in
`docs/notes/review-fixes.md`): the memory-hog test depended on who wins a race between
bubblewrap and systemd's `OOMPolicy=stop`; the broker's sandboxed-JVM probe asked for a
join before the relay had accepted its game connection; the confinement test put a socket
at a path that became one byte too long once process ids had seven digits; and the opt-in
X11 test (`BASTION_X11_TESTS=1`) assumed that the session's display is handed over
directly, which stopped being the plan on this machine when gamescope was installed
system-wide.

**Verification state of the review round** (2026-11-13, on the branch that holds the fixes):
`cargo fmt --check` clean, `cargo clippy --workspace --all-targets` without warnings,
`cargo test --workspace` green (375 tests), the launcher's tests also with
`BASTION_NET_TESTS=1 BASTION_X11_TESTS=1` on a private X display; the Wayland suite against
headless sway 1.12 and the nested suite against gamescope 3.17 (with the clipboard test and
its control run) green; four real demo launches of Minecraft 26.3 — through gamescope,
through a security context on headless sway, as a burner session, and the signed Fabric
profile — each started, ran and stopped cleanly with the launcher confined
(THREAT_MODEL §7 E16), and once more through gamescope on the final tree; the fuzz crate
compiles (`cargo check`, all targets). Not run:
`makepkg`, `cargo deny check`, the GUI live test, anything that needs an account, `age`,
root, or another kernel.

## Decided against / deferred, with the reason

- **Login-protocol proxy (old §4.2).** Would remove the last thing code in the JVM can do
  with the account under the broker: a join per game connection it opens — which is good
  for a login on *any* server whose hash the mod has learned, not only the one the
  connection goes to (review A-F10; THREAT_MODEL J13). Requires
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
