# TODO — where the work stands

Snapshot of 2026-11-13, after the third round was merged into this tree. Everything under
"Done in the third round" and "Done in the second pass" is committed.

The third round — the "workshop HUD" front end, the first-party companion Fabric mod, the
UDP relay for voice chat with the fix of review finding B-M3, verified mod installs from
Modrinth, and the fixes for the two independent reviews — was built in separate working
copies and is **merged** (merge commits `3025219`, `aef85df`, `263498c`, `2dd827a`,
`35d57cd`; integration fixes `1dbb587`, `c383196`). A project website for GitHub Pages is
being built separately and is not in this tree yet.

How to read this file: **Open** is what is left and why; **Blocked on you** needs
something only you have; **Done in the third round** and **Done in the second pass** are
the record of what changed, with where it is and how it was checked; **Independent review**
is the record of the review round; **Decided against / deferred** keeps the reasoning for
the big ideas so they are not re-argued from scratch.

Verification state of the merged tree (the integrator's runs, 2026-11-13, with separate XDG
directories and only private displays): `cargo fmt --check` clean (re-checked on the final
commit); `cargo clippy --workspace --all-targets` without warnings; `cargo test
--workspace` 581 passed, 0 failed (README "Status" says what that number counts), and the
same with `BASTION_NET_TESTS=1 BASTION_X11_TESTS=1`; the nested-compositor suite 4/4
against gamescope 3.17 with the clipboard test; the Wayland suite 2/2 against headless
sway; `tests/nested_game.rs` (vanilla) and `tests/companion_game.rs` (the signed Fabric
profile, in the demo world) passed; two demo launches through the nested compositor — a
signed Fabric profile with Simple Voice Chat and a voice rule, and one with ViaFabric and
Simple Voice Chat — loaded their mods, native libraries included, and stopped with exit
status 130; `cargo deny check` (offline): advisories, bans, licences and sources ok. The
README's quick start (`instance new`, `install`, `launch --demo`, `doctor`, `status`,
`versions`, `config example`) was run on the final commit by the README's writer, and the
fuzz crate compiles there (`cargo check --bins` in `fuzz/`, all twelve targets). On the
final commit (2026-11-14) the integrator also ran `makepkg` from a tarball of the tree (both
packages build, `check()` passes, both binaries pass the hardening check, the documents are
installed as a tree) and the front end's live test (2 passed: a real demo game started,
followed phase by phase and stopped through the window's own controls). **Not run on the
merged tree:** anything that needs an account, `age`, root or another kernel, and the
libFuzzer targets themselves.

## Open

### Small, concrete
- [ ] **Runbook as QR codes / print view** (§8 of the old list). Not built: needs a QR
      encoder (a dependency, or ~300 lines). The text is in the package docs and in
      `bastion-mc wiki scram`.
- [ ] **`README.md` "Status" numbers** drift with every test added; re-count before a
      release (the README says how: the `test result` lines of `cargo test --workspace`).
- [ ] **Java runtimes from Arch's jre packages are not found** (review of the documents).
      The launcher recognises a runtime by its `release` file (`src/java.rs`, `read_release`
      and `discover_in`), and for Java 17 and later Arch ships that file only in the jdk
      packages: `jre21-openjdk` is installed on the development machine and neither `launch`
      nor `doctor` lists it, and the package file lists of `jre17-openjdk`, `jre25-openjdk`
      and `jre-openjdk` have no `release` (read on 2026-11-13; `jre8-openjdk` has one,
      through `jre8-openjdk-headless`). Yet the refusal in `java::select` recommends
      `jre<N>-openjdk` or `jre-openjdk`, `doctor` recommends `jre21-openjdk`, and the
      PKGBUILD's `depends=('java-runtime')` and its `optdepends` (`jre17-openjdk`,
      `jre21-openjdk`, `jre-openjdk: current Minecraft releases`) can all be met by runtimes
      the launcher ignores; `jre-openjdk` is moreover Java 27, while 26.3 declares Java 25 and
      the default `java.match = "exact"` refuses 27 for it. Either determine the release
      another way (the `java-NN-openjdk` directory name, or the runtime's `lib/modules`), or
      name jdk packages everywhere (`jdk25-openjdk` for 26.3). Until then the README names
      `jdk25-openjdk`.
- [ ] **Remove `target/third-party/`, `target/tools/`, `target/probe/`** when you no longer
      need them (all three are reproducible: `tests/support/fetch.sh`, `cargo install
      --root target/tools …`, the makepkg recipe). Left in place because the display tests
      need the first one.
- [ ] **Stale markers**: a launcher killed with SIGKILL leaves `cache/running/<pid>` and its
      scratch directory; both are swept by the next launch or by `scram` (tested). The same
      holds for the companion channel's socket name in `$XDG_RUNTIME_DIR/bastion-mc/` when
      the launcher could not establish that the program in the sandbox exists (the launch
      then says so: a warning in the log and to the front end), or was killed or crashed
      before that point: the next launch that adds the companion sweeps it (tested). Nothing to do unless
      you want a `bastion-mc clean` command.
- [ ] **Create the companion socket just before the sandbox starts.** Today
      `companion::plan` creates it while the sandbox is assembled, before the sign-in, the
      resource-limit probe and the nested compositor's start, so its name exists for seconds
      in `$XDG_RUNTIME_DIR/bastion-mc/` (other programs of yours could connect and be logged
      as the game). Creating the endpoint right before `sandbox::spawn` shortens that to the
      moment bubblewrap needs to bind it (review of the documents).
- [ ] **Leftover Wayland socket names are not swept** (integration review). The game's
      security-context socket (`$XDG_RUNTIME_DIR/bastion-mc/wl-*`) loses its name in step 8
      only when the program in the sandbox could be identified (`src/launch.rs`); otherwise
      the name stays, and nothing removes it later: `src/sandbox/display.rs` and
      `src/sandbox/wayland.rs` have no sweep like the companion channel's. The integrator
      judged it harmless (once the launcher has exited, the compositor no longer accepts
      connections on it); a sweep would tidy it.
- [ ] **The front end shows the launcher's warnings as ordinary status lines** (integration
      review). The event protocol has no warning flag: under `--events`, `Reporter::warn`
      sends the same `Status` event as any progress note (`src/events.rs`), so a line such as
      "limits: WARNING: … NOT in force" appears in the window's status line and console like
      any other. The protection read-out still shows each aspect's level. A warning event
      (or a flag on `Status`) and a way to set such lines apart are not built.
- [ ] **No test of the default suite runs the voice relay's own code in a confined
      process** (integration review). The test
      `voice::the_relay_can_still_open_flows_after_the_launcher_has_confined_itself` does
      what the relay does for a new flow with a plain UDP socket; the relay's own sockets
      (`GameSocket` with `IP_PKTINFO`, `open_upstream` in `src/egress/udp.rs`) ran after
      confinement only in the real launch of THREAT_MODEL §7 E22. A test that serves a flow
      from a confined child would close that.
- [ ] **Sandboxes killed at start when many start at once** (voice-chat.md §7, flake 2): with
      four copies of a sandbox test binary running, now and then a payload ends with status
      137 and no output, or a whole test binary is killed. Reproduced with plain bubblewrap
      and no code of this project (`seq 1920 | xargs -P 48 …`: 2 of 1920); who sends the
      signal was not found (no kernel log entry, no tracing tools on the machine). A real
      launch starts one or two sandboxes, far below that level; a sandbox test can fail this
      way when several suites run at the same moment — run it again before looking for a
      cause in the code.
- [ ] **Texture gateway memory** (review, adjacent to A-F5): the broker's buffers now come
      out of one 64 MiB budget, but the texture gateway in `src/egress/mod.rs` still holds
      up to 4 MiB per connection (256 connections) outside it. In practice Mojang's
      texture host serves skins of a few kilobytes; the bound is what the code allows.
      Either put it under the same budget or lower the per-texture cap.
- [ ] **A memory kill can be reported as "game exited with status 1"** (mod-catalog and
      voice-chat notes). After the kernel's out-of-memory kill inside the scope, the user
      manager (`DefaultOOMPolicy=stop`) stops the scope, and its SIGTERM can reach
      bubblewrap before bubblewrap reports status 137; `launch.rs` turns a death by signal
      into status 1. `OOMPolicy=continue` on the scope (`src/sandbox/cgroup.rs`) made the
      `systemd-run` reproduction deterministic (137 in 48 of 48 runs); not tried in the
      launcher. The test accepts either outcome since the review round, which hides the same
      race in a real launch.
- [ ] **Front end, small** (gui.md §7): the launch page's panels are stretched to one line
      with the block around the gauge only when their words are of usual length (otherwise
      each is as high as its content); the 5-per-second redraw while a game runs happens on
      the launch page only, and whether it should stop while the window is hidden was not
      looked into; a version name longer than 48 characters, or one that cleaning would
      change, is not offered in the "new instance" form, and the form does not say what it
      left out.
- [ ] **Companion mod, small** (companion-mod.md part 5): `bastion-mc doctor` does not
      mention it; the `instance.toml` template written by `instance new` and `profile new`
      mentions voice chat but not `[companion]`; the front end has no indicator of its own
      for it (its lines appear in the console like any other).
- [ ] **Voice chat, small** (voice-chat.md §3): the front end says nothing about voice — a
      line such as "voice: relayed to play.example.org:24454 while connected" in its
      protection read-out would be honest, and was not built; `doctor` reports the Landlock
      UDP rule but does not test the relay.
- [ ] **Mod catalog, small** (mod-catalog.md §7): `profile check` exits 0 whatever it
      flags (a `--strict` could make it a gate); `profile check` does not ask the file
      server for each file's date (one request per jar, and it would tell the file server
      which jars are in the profile — so "fresh" there goes by the build's date, and the
      output says so); no `profile remove`, no "update everything", no `<mod>@<version>` to
      pin a build (a downgrade is a manual copy plus `profile lock`); a replaced jar is
      deleted, where keeping it aside until the next signature would make a wrong
      replacement recoverable; not offered through `--events`, so the front end has no
      button for it; the user agent has no contact address (Modrinth recommends one; the
      project has no public home yet, the same blocker as the PKGBUILD's `url`); registry
      file names with spaces, brackets or `%` are refused rather than renamed; whether a
      kept dependency satisfies a version *range* is left to the loader; the alias table is
      compiled in, and its live test failing (a project renamed or removed) is the cue to
      update it.

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
      needs you at the keyboard. Then pick defaults for `nested_compositor_args`. The same
      session can try the companion mod by hand: hold C in a world, turn the wheel, switch
      fullbright on in a cave.
- [ ] **Login-protocol proxy**, **signed server allowlist**, **approval prompts through the
      companion mod**, **frozen `/usr`**: see "Decided against / deferred". Each is days
      of work; say which.
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
- [ ] **Voice for servers with a separate voice host** (`voice_host` pointing elsewhere, for
      example behind a TCP-only proxy): not supported. Supporting it means relaying
      datagrams to an address that no TCP connection vouches for — a weaker design (a
      second name resolution, a second destination) that should be a deliberate decision,
      not an extension of this one.
- [ ] **Further companion features**: none were added. Candidates that fit "harmless
      client-side convenience, nothing servers commonly forbid": a toggle mode for zoom,
      the smooth camera while zoomed, a clock line in the HUD read-out. Deliberately not
      candidates: free look, a minimap, anything that automates input or shows what the
      unmodified game does not show.
- [ ] **The companion mod beyond the tested combination**: Minecraft 26.3 with Fabric Loader
      0.20.1 only. `force` on another Minecraft version was never tried in a real game; a
      Fabric Loader too old to know `fabric.addMods` would ignore it silently (the launcher
      would say "added" and no hello would follow); Quilt is not supported. Adding a
      version is a procedure (`docs/COMPANION.md`, "Adding a tested Minecraft version").

### Not verified here, by package (and why)
- [ ] **Front end** (gui.md §7): no click was ever made in a real window — clicks cannot be
      automated on the development machine and the window must not open on your desktop,
      so everything about pressing things is from iced's simulator and emulator; `scram
      --yes` was never run from the window, because it would remove the real sign-ins of
      this machine (the scram path is tested against stand-ins that follow the launcher's
      contract); Wayland was not used (X11 through Xwayland in a headless compositor only);
      the GPU renderer was seen on the opening page only, and motion was never watched (the
      gauge is tested as a function of time); other fonts only as an experiment (FreeSans
      and FreeMono); high-density displays only as scale-2 snapshots; keyboard-only use not
      examined.
- [ ] **Companion mod** (companion-mod.md parts 4 and 5): no person at the keyboard (the key
      press was synthetic, through XTEST, on a US layout); nobody looked at a dark place
      with fullbright on (the input to the game's light-map shader was measured, what the
      shader does with it was read, not watched); no other Minecraft version; the Wayland
      path with a headless sway only and no key press there; no multiplayer; reproducibility
      across compilers (one JDK is installed; two builds with it give the same bytes).
- [ ] **Mod catalog** (mod-catalog.md §7): ViaFabric translating a connection and Simple
      Voice Chat carrying voice (they need an account, a server, UDP and a microphone; only
      loading was observed); a Quilt profile (the rule is unit-tested only); the cargo-fuzz
      targets `registry` and `jar` (compile-checked only); the jar scan near its 512 MiB
      limit; HTTP 429 and 410 (only the wording); download deadlines with a slow server;
      Modrinth through Tor; game versions other than 26.3 in live lookups; the file-date
      rule on a real fresh file (tested against scripted answers and a stand-in file server
      over TLS); how often Modrinth's file server rewrites its storage (one such day was
      found); what `date_published` is for a draft or scheduled build (undocumented; needs a
      project of one's own); a file-server answer without `Last-Modified` (stand-in only).
- [ ] **Voice chat** (voice-chat.md §3): a real voice session (needs a server running the
      mod and a signed-in account); capture and playback (sound is off in the test
      configuration); datagrams sent by the game itself in a real launch — the launcher's
      side was exercised with a stand-in client in the game's network namespace, and the
      Landlock UDP rule and the seccomp filter by `tests/voice.rs`; the B-M3 fix against a
      real SOCKS5 proxy such as Tor; kernels without Landlock ABI 10; other voice mods
      (anything that sends plain UDP to the game server's own address should work the same,
      anything with another host, STUN/ICE or WebRTC will not); the cargo-fuzz target `udp`
      (compile-checked only); Simple Voice Chat's own network code (read from its class
      files, not run).

## Blocked on you (hardware, accounts, identity)
- [ ] **Live Microsoft → Xbox → Minecraft sign-in.** Needs your Azure client ID and
      Mojang's approval of it. First things to run then: `bastion-mc login`,
      `launch <id> --join <server>` in filtered mode.
- [ ] **Everything that needs a signed-in account**, all built and tested against scripted
      replies / a stand-in Mojang only: the broker against the real API (join, chat signing,
      attributes, block list), `skin show|set|reset|export`, `cape set`, the canary on a
      real profile, `export-proofs` with real history, multi-account with two real accounts,
      joining a real multiplayer server — and with that ViaFabric translating a connection,
      the companion mod on a server, and Simple Voice Chat carrying voice through the relay
      (which also needs a server running the mod and a microphone with `audio = "pulse"`).
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
      its reporting are tested by asking this kernel (ABI 10) for less. A kernel without
      ABI 10 would also show the voice relay without its second, Landlock layer for UDP in
      the sandbox.
- [ ] **A person at the keyboard**: clicks in the real front-end window, playability inside
      gamescope, and the companion mod by feel (see Open).
- [ ] **A real SOCKS5 proxy (Tor)**: the B-M3 fix and Modrinth through the proxy are tested
      against a stand-in proxy only.
- [ ] **`scram --yes` from the window against the real launcher**: never run here, because it
      removes the real sign-ins of this machine; worth doing once on a machine or account
      whose sign-ins you can lose.
- [ ] **PKGBUILD**: the real `url` (a value is there, still marked TODO), your
      release-signing key in `validpgpkeys`, a signed tag. Everything else in it is done
      (auditable build, reproducibility notes, vendoring recipe, the second pass's documents
      installed — see Open for the documents of the third round).
- [ ] **Coverage-guided fuzzing**: `rustup toolchain install nightly && cargo install
      cargo-fuzz`, then `fuzz/README.md`. The twelve targets compile (checked on the merged
      tree); they have never run.
- [ ] **`mc-vault` account / `ptrace_scope=3`**: documented (`docs/MC_VAULT_ACCOUNT.md`),
      needs root; not exercised.
- [ ] **Minecraft help-centre wording in the runbook**: their site needs a browser; only
      the request-form address was checked to exist. Read it once and adjust phase 3.

## Done in the third round

The record of each package — what was verified, how, and what was not — is its notes file
in `docs/notes/` (index: `docs/README.md`). Where a note and the code disagree, the code
is right.

### The front end ("workshop HUD") — `docs/notes/gui.md`
- New interface: a rail and five pages (Launch, Accounts, Instances, Console, Security);
  the gauge around the launch control; account cards; the protection read-out, where a
  level is always a word and a shape, never a colour alone; the sign-in panel. Nothing is
  drawn outside the place it was given: every paragraph of every page in every state is
  measured at thirteen window sizes, also with 3000-letter texts that cannot be broken.
  No timer runs while nothing moves.
- **B-M2 fixed**: "scram complete" only for exit status 0, no line starting `COULD NOT`
  and a report that says something; anything else is SCRAM INCOMPLETE on every page.
- **B-M1, the front end's half, fixed**: nothing in the front end ends the launcher that
  runs a scram (no kill, a process group of its own, standard error to `/dev/null`); after
  30 minutes the window stops waiting, leaves it running and says so.
- **B-L2, the front end's half, fixed**: a sign-in this window started is ended before the
  scram starts, and nothing that stores a token can start while a scram runs. Found along
  the way: a window closed between "Yes, scram" and the scram's start took the scram with
  it; it now waits until the scram has started.
- Threat model rows G2, G3, G4 extended, G6 rewritten, G7 new (gui.md §5; now in the
  threat model). New
  dependency in the front end's tree only: lyon, through iced's `canvas` feature (six
  crates); `cargo tree -p bastion-mc` has none of the front end's drawing crates.
- Verified by its engineers: 118 unit tests and 4 emulator tests; the live test on a
  private display against the real launcher and a real demo game, twice in one run; the
  real window with the GPU and the software renderer, closed by `WM_DELETE_WINDOW`; 0
  clock ticks of CPU in 15 idle seconds; `cargo deny check` ok.

### The companion mod — `docs/notes/companion-mod.md`, `docs/COMPANION.md`
- `bastion-companion`: zoom (on by default), a HUD read-out (off), fullbright (off). An
  interface-only convenience that enforces nothing; added by the launcher, read-only and
  outside `mods/`, through `fabric.addMods`, to Fabric profiles of tested versions (26.3);
  settings under `[companion]`, per instance in `instance.toml`.
- The channel: one socket, mod to launcher only; every line bounded and checked, logged as
  game output, never acted on; read only after the launcher has confined itself; a
  generated-input harness (`companion`).
- Hermetic build: `build.rs` compiles it with the system `javac --release 17` — no Gradle,
  no network, no Minecraft or Fabric code as input (compile-only stand-ins for seven types,
  compared with the real jars by an opt-in test); two builds give the same bytes.
- Verified in the real game on the nested-compositor, direct-X11 and Wayland paths, with
  the effect of each feature measured in a running demo world; on the X11 paths also with
  the zoom key really pressed (XTEST, private display).
- With this, the old §7 idea of a Fabric companion mod exists as what "Decided against /
  deferred" said it could only be: interface only. Approval prompts are not built.

### The mod catalog — `docs/notes/mod-catalog.md`, `docs/MODS.md`
- `profile add` and `profile check` against Modrinth: aliases by project id, releases only
  unless `--prerelease`, a 48-hour quarantine that also reads the file server's own date
  for each file, size + SHA-512 + SHA-1, all or nothing, nothing signed, the capability
  scan printed before signing. `profile lock` keeps a recorded `source` while the file is
  unchanged; the capability report holds every jar, also two that go by one identity.
- Verified: scripted registry answers (a 28-case matrix of refusals that leave everything
  as it was) and a stand-in registry over TLS (an add and a check end to end, a file that
  does not match, a fresh file, an untrusted certificate, a deadline, the SOCKS5 proxy),
  live lookups of the six aliases,
  three real launches with
  ViaFabric, Simple Voice Chat, Sodium, Lithium and Mod Menu (they load; update checks and
  Sodium's `lspci` are refused by the sandbox).
- Threat model rows C8–C12, N4, N5 (proposed in mod-catalog.md §6; now in the threat model).

### Voice chat and B-M3 — `docs/notes/voice-chat.md`
- A UDP relay per server rule (`voice = N` or `udp = [...]`), tied to an open game
  connection and to the address it uses; 1400 bytes per datagram, 100 and 500 per second,
  8 flows, 30 s without a datagram from the game; Landlock UDP rules in the sandbox for
  exactly those ports; IPv4 first for such servers. A server table with an unknown key is
  an error now (it used to be ignored), and `port = 0` is refused.
- **B-M3 fixed**: with `launcher_proxy` set, the connections to Mojang's services that the
  relay passes through go through the proxy (CONNECT by name); the confined launcher then
  keeps no direct port 443; the launch log says per network mode what is and is not
  proxied.
- Verified: 14 tests in `tests/voice.rs` (13 in the real sandbox), unit tests of the
  decision table, the `udp` harness, the relay inside a real launch with a stand-in client
  — which found that "open" meant "not yet closed by both sides" (fixed: the first hang-up
  ends it) —, and Simple Voice Chat loading with its native libraries.

### Integration (`1dbb587`, `c383196`, `c588bea`, and the commit after the documents)
- `profile add` says, after adding Simple Voice Chat, what becomes of its UDP in the
  instance's network mode: in `filtered` a server rule must name the port, datagrams go
  only while a game connection to that server is open and only to its address (so a
  separate `voice_host` does not work); in `open` UDP is not restricted at all; in `none`
  voice cannot work. Installing the mod opens nothing (`voice_chat_hint` in
  `src/profile/catalog.rs`; tested per mode, and with an instance in `open` mode).
- Review of the merge points (four reviewers, one skeptic per serious finding; none was a
  defect of a single package): a display-output hold given up unreleased (step 9 refused)
  now discards the held output instead of letting an unconfined launcher read it; the
  launch warns when the companion socket keeps its name; a default-suite test serves and
  reads the companion channel from a process confined as `launcher.confine = "require"`
  does it; the system calls the relays need after confinement are pinned in the filter's
  test; the front end waits 90 s for the launcher to stop (its own shutdown is bounded at
  about a minute) and keeps protection details up to 1000 characters.
- From the review of the documents: the Java refusal, `doctor` and the PKGBUILD name the
  packages the launcher can find (`jdk<N>-openjdk`; `jre8-openjdk-headless` for Java 8);
  `doctor` counts the built-in certificate pins; the confinement line in the launch log
  says that /etc, /usr and /proc stay readable; a game ended by a signal is logged as such;
  the package installs every document the README links to, as a tree.
- PKGBUILD: `makedepends` `java-environment>=17` (the companion jar is compiled with
  `--release 17`).
- `src/confine.rs` header: the companion channel and relayed datagrams are among what the
  confined launcher still handles; UDP is not restricted by it.
- `tests/sandbox.rs`: the keyring test's key has a per-process name (two runs at once
  removed each other's key). `tests/voice.rs`: the test of UDP after confinement applies
  the whole reworked confinement, Landlock and the system-call filter.
- `docs/notes/broker-test-race.patch` dropped: the review fixes had already made that test
  independent of the race.
- Documents (this pass): the README as a front page with the merged counts,
  `docs/README.md` as the index of `docs/`, three screenshots in `docs/images/`, the new
  `docs/MODS.md` and `docs/COMPANION.md`, and the threat model brought up to the round
  (companion J19, voice relay J20, B-M3 in N1, mod catalog C8–C12, N4, N5, front end
  G2–G4, G6, G7, experiments E19–E25). Then two reviews of the documents (accuracy against
  the code, and a newcomer following the README in an empty data root) and their fixes,
  finding by finding, in `docs/notes/docs-pass.md`.

## Done in the second pass

Verification state at the end of the second pass: `cargo fmt --check` clean, `cargo clippy
--workspace --all-targets` clean, `cargo test --workspace` green, also with
`BASTION_NET_TESTS=1 BASTION_X11_TESTS=1`; nested-compositor and Wayland suites green
against real gamescope 3.17 and sway 1.12; GUI live test green; `makepkg` from a tarball
builds both packages and their `check()` passes; `cargo deny check` ok.

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

**Fixed (25 of 27, and the adjacent one)** in the review round; the remaining two (B-M2,
B-M3) and the front-end halves of B-M1 and B-L2 in the third round — so all 27 now. The
join residual behind A-F10 stays (it needs the login-protocol proxy).

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
- *Third round* (B-M2, B-M3, the front-end halves of B-M1 and B-L2): see "Done in the
  third round".

**Left, and why** (as recorded at the end of the review round).

- B-M2 (the front end reporting an incomplete scram as done) and B-M3 (pass-through
  connections bypassing `launcher_proxy`) belong to the front end and to the egress relay;
  so do the front-end halves of B-M1 and B-L2. The launcher's contract for them is
  unchanged: exit status 1 and `COULD NOT …` lines when scram is incomplete. (All four
  were done in the third round.)
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
root, or another kernel. (`cargo deny check` and the GUI live test were run in the third
round; see the verification state at the top.)

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
  *Third round:* built as exactly that — an interface-only convenience (zoom, HUD read-out,
  fullbright) whose one-way channel the launcher logs and never acts on. Approval prompts
  are still not built. Their design keeps the rule above: the decision is made and entered
  outside the sandbox, in the launcher's own interface; the mod can at most tell the player
  that the launcher is asking, and nothing arriving on the channel may create a prompt or
  count as consent (`docs/notes/companion-mod.md` part 2).
- **Hardware normalisation.** GPU renderer string cannot be normalised by mounts, so the
  result stays fingerprintable. `/proc/cpuinfo` and memory size are real. Documented (§5.9).
- **Frozen `/usr` + JVM (EROFS/composefs).** Needs a root component.
- **Security-key dead man's switch**, **CET for Rust code** (nightly only), **seccomp user
  notification** (unsafe for pointer arguments), **padding of auth traffic** (rustls 0.23
  has no record-padding setting; destinations are visible anyway), **cross-registry hash
  consensus** (CurseForge needs an API key; a compromised upstream publishes the same hash
  everywhere). Not planned unless something changes. (Since the third round `profile
  check` asks Modrinth alone; consensus across registries is still not built, for the
  same reasons.)
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
# the companion mod in the real game (BASTION_COMPANION_WORLD=1: in the demo world as well):
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=fab \
    BASTION_COMPANION_WORLD=1 BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt) \
    cargo test --test companion_game -- --nocapture
```
