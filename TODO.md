# TODO — where the work stands

Snapshot of 2026-11-14, after the fifth round was merged into this tree. Everything under
"Done in the fifth round", "Done in the fourth round", "Done in the third round" and "Done
in the second pass" is committed.

The fifth round — **Minecraft: Bedrock Edition**, the Android build, run by your installed
`mcpelauncher-client` inside the launcher's sandbox: finding the client and a store of
imported versions, the launch and the client's sandbox profile, the network (HTTPS by name,
RakNet to listed servers, Realms and event-server joins approved outside the game), the
sign-in outside the game, and the account-risk controls (the licence gate, the per-session
data directory, scram, doctor, the canary) — was built by five engineers in separate working
copies against one contract (`docs/notes/round5-contract.md`, after the research in
`docs/notes/bedrock-research.md`), **integrated** with a sign-in handler for the desktop
added on top (`docs/notes/bedrock-integration.md`), and documented in this pass. Nothing of
it has run with the real client, the game, a Google Play purchase or a Microsoft account:
none was on the development machine. The fourth round — the in-game phone and its services
(Spotify, the browser with Discord, clips, Realms, settings sync), the redone home screen and
dark menus, and the companion mod on every Fabric release from 1.20.1 to 26.3 — was built the
same way against its own contract (`docs/notes/round4-contract.md`) and merged before it
(commits `a671183`, `afd43a9`, `18d3911`, `06e6ff0`; integration commits `d3f6931`,
`a533e43`, `c6f25a8`; then the documents). The third round — the "workshop HUD" front end,
the first-party companion Fabric mod, the UDP relay for voice chat with the fix of review
finding B-M3, verified mod installs from Modrinth, and the fixes for the two independent
reviews — was merged before it (commits `db878be`, `c65762d`, `b6bc924`, `6d76956`,
`74a1101`; integration fixes `edc7efc`, `785c2a6`). The project website (`site/`, published
to GitHub Pages by `.github/workflows/pages.yml`) and continuous integration
(`.github/workflows/ci.yml`) were merged after the fourth round.

Commit references are to this repository's history. Rounds 3 and 4 were built in separate
working copies and are committed here one step per merge or fix; each such commit's message
lists the commits of the working copy it contains. The fifth round is referred to by its
notes files, which name the steps of its integration.

How to read this file: **Open** is what is left and why; **Blocked on you** needs
something only you have; **Done in the fifth round**, **Done in the fourth round**, **Done
in the third round** and **Done in the second pass** are the record of what changed, with
where it is and how it was checked; **Independent review** is the record of the review
round; **Decided against / deferred** keeps the reasoning for the big ideas so they are not
re-argued from scratch.

Verification state of the merged tree of the fifth round (the integrator's runs,
2026-11-14, `docs/notes/bedrock-integration.md` section 6, and this documents pass's, with
separate XDG directories, a private runtime directory, only private displays, and never your
session bus): `cargo fmt --check` clean, also in `fuzz/`; `cargo clippy --workspace
--all-targets` without warnings; `cargo test --workspace` 986 passed, 0 failed, 2 ignored
(44 result lines; how such a number is counted: README "Status"), and the same again in this
pass, where one of six runs of the launcher's unit tests failed once on a flaky store test
(see Open); the Bedrock suites in it: `bedrock_launch` (three sessions of a stand-in client
in the real sandbox — the sign-in pasted at the trusted input in one, handed back by the real
`bastion-mc bedrock hand-back` with the launcher confined in another), `bedrock_sandbox` 3,
`signin_stub` 4, `bedrock_store` 3, `bedrock_net` 7, `bedrock_cli` 4; on the Java launch path
the opt-in suites again — nested 4, Wayland 2, `nested_game` 1, `companion_game` on three
instances, `browser_game` 1, `clips_game` 2 — and demo launches of three instances, which
reached their texture atlases with the launcher confined on all threads; the CI's steps
simulated with a clean target directory and a fresh home (every step passed), and once more
with bubblewrap, ffmpeg, xdg-dbus-proxy, Firefox and `systemd-run` hidden (only tests that
check root ownership failed, an artefact of the user namespace used to hide the programs);
`cargo deny check` ok, no dependency added in the round; `python3 -I site/build.py --check`
passes; `makepkg` from a `git archive` of the tree: the packages build, `check()` and the
hardening check pass, and the `bastion-mc` package holds `docs/BEDROCK.md` and the desktop
entry of the sign-in handler, which is valid and was registered as the scheme's default in
isolated XDG directories (`gio open` ran the command). **Not run:** anything with the real
`mcpelauncher-client`, a game file, a Google Play purchase or a Microsoft account; the tests
that reach Mojang's and Microsoft's live services (`BASTION_NET_TESTS=1`); the libFuzzer
targets; the fifth round's tree on GitHub.

Verification state of the merged tree of the fourth round (the integrator's runs,
2026-11-14, with separate XDG directories, only private displays, and never your session
bus): what each package ran on its own branch is under "Done in the fourth round". On the
merged tree: `cargo fmt --check` clean, `cargo clippy --workspace --all-targets` without
warnings, `cargo test --workspace` 768 passed, 0 failed; with `BASTION_NET_TESTS=1
BASTION_X11_TESTS=1 BASTION_BROWSER_TESTS=1` 768 passed; the nested-compositor suite 4/4
with the clipboard test; the Wayland suite 2/2; `tests/nested_game.rs` (vanilla 26.3);
`tests/companion_game.rs` on 26.3 (in the demo world), 1.21.4 and 1.20.1;
`tests/clips_game.rs` 2/2; `tests/browser_game.rs` (after `de5fb18`); demo launches of the
voice-chat and catalog instances and of a vanilla 1.21.4 instance (the bootstrap fix: the
same instance did not start with the third round's binary); `cargo deny check` ok. The phone
end to end with the real services (not the demo), in the real game (26.3, demo mode): the
phone listed Spotify, Discord, Browser, Clips, Realms and Settings; 27 commands from it
reached the services and were accepted; the Spotify app showed the track of a stand-in
player on a private D-Bus (never the session bus or the Spotify app), with its cover; the
Discord app showed Discord's public sign-in page, loaded by the sandboxed browser through
its relay (nobody signed in); a clip was saved (30 s, 900 frames, 1280x720). `makepkg` from
a `git archive` of the tree (with the CI workflows, the same code): both packages build,
`check()` passes, both binaries pass the hardening check. **Not run:** the libFuzzer
targets, and anything that needs an account (a Spotify client ID, a Discord sign-in, a
Realm), `age`, root or another kernel.

Verification state of the merged tree of the third round (the integrator's runs,
2026-11-13, with separate XDG directories and only private displays): `cargo fmt --check`
clean (re-checked on the final commit); `cargo clippy --workspace --all-targets` without
warnings; `cargo test --workspace` 581 passed, 0 failed (README "Status" says how such a
number is counted), and the same with `BASTION_NET_TESTS=1 BASTION_X11_TESTS=1`; the
nested-compositor suite 4/4 against gamescope 3.17 with the clipboard test; the Wayland
suite 2/2 against headless sway; `tests/nested_game.rs` (vanilla) and
`tests/companion_game.rs` (the signed Fabric profile, in the demo world) passed; two demo
launches through the nested compositor — a signed Fabric profile with Simple Voice Chat
and a voice rule, and one with ViaFabric and Simple Voice Chat — loaded their mods, native
libraries included, and stopped with exit status 130; `cargo deny check` (offline):
advisories, bans, licences and sources ok. The
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
- [ ] **Companion mod, small** (companion-mod.md part 5, mod-platform.md part 5): `bastion-mc
      doctor` does not mention it; the `instance.toml` template written by `instance new`
      and `profile new` mentions voice chat but not `[companion]` (still so after the fourth
      round, which added `phone`, `phone_key`, `home` and `dark_menus` there); the front end
      has no indicator of its own for it or for the phone — their lines appear in the
      console like any other, the Realms approval prompt being the one exception.
- [ ] **`doctor` and the phone's helpers** (browser.md §7): `doctor` reports neither whether
      Firefox is installed and its sandbox starts, nor `xdg-dbus-proxy` (Spotify over MPRIS),
      nor ffmpeg (clips). A launch says it in its log when one is missing (read in
      `src/browser/sandbox.rs`, `src/spotify/mpris.rs`, `src/clips/recorder.rs`).
- [ ] **Zoom, HUD read-out and fullbright on the other nine families** (mod-platform.md
      part 5): not ported; they reach the game by 26.3's names and switch themselves off
      elsewhere, saying so once. Porting means a family mixin for each hook and the lookups
      in that family's names; the stand-ins and the mixin plugin are in place for it.
- [ ] **Realms resource packs** (realms-sync.md §7): a Realm with a resource pack probably
      fails to join — the pack's download address is not reachable from the sandbox.
- [ ] **The browser's sign-in window on X11** (browser.md §7) is a client of your X server
      like any other window; running it in the nested compositor, as the game, would
      isolate it, at the cost of a second compositor for a short task.
- [ ] **Clips: hardware encoding** (media.md, Open): x264 only; VAAPI not done.
- [ ] **A flaky unit test of the Bedrock store** (found in the documents pass of the fifth
      round): `bedrock::store::tests::remove_refuses_while_a_launch_holds_the_version_and_removes_it_after`
      failed once in six runs of the launcher's unit tests — `remove` still found the version
      locked after the test had dropped both of its descriptors ("in use by a running launch
      (it holds the version open)") — and passed in the five others and in eight runs on its
      own. Not investigated further. One explanation that fits: the lock is shared by every
      copy of the descriptor, and a process that another test starts at that moment holds
      copies of the test process's descriptors until it executes its program (they are
      close-on-exec). If that is it, the store is right and the test needs a retry or the
      descriptor opened where no other test forks.
- [ ] **The integration suites on a GitHub runner** (`.github/workflows/integration.yml`,
      started by hand): never run. The runner's kernel (7.0) lacks Landlock ABI 10, so the
      UDP-rule tests will skip; what else differs from the development machine (AppArmor's
      user-namespace restriction is lifted by the workflow, no systemd user manager) the
      first run will show.
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

### Bedrock (the fifth round)
- [ ] **The real client, game and accounts have never run** under the launcher (none was on
      the development machine; what that leaves unverified is listed per package under "Not
      verified here", and what you can do about it under "Blocked on you"). When they do, in
      this order (`docs/notes/bedrock-integration.md` §8):
      - **Profile the client's system calls once**: one session with `[bedrock] seccomp =
        "log"`; fold what the kernel log names (`journalctl -k -g 'type=1326'`) into
        `seccomp::BEDROCK_ALLOW`, or your `[bedrock] extra_syscalls`. The allowlist was
        derived from the client's source and has never met the real client.
      - **Check that a second session asks for the sign-in again.** If it does not, the
        game keeps its sign-in somewhere the per-session directory does not cover, and the
        kept folders (`minecraftWorlds/`, `minecraftpe/`) hold sign-in material (threat model
        B16).
      - **Measure a real version**: its size, number of files and depth against
        `store::LIMITS`, and the time a launch spends checking it (single-threaded; hashing
        in parallel would shorten it if it is slow).
- [ ] **The sign-in's scope, after the first real sign-in** (bedrock-signin.md §4): a page that
      asks for more than `service::user.auth.xboxlive.com::MBI_SSL` is refused, with the scope
      named in the warning. If the real start address carries another scope, review what it
      grants before adding it to `check::SCOPE`.
- [ ] **The 1023-byte limit of the final address** (bedrock-signin.md §4): the client reads
      the address with one 1024-byte read, so a longer one could not reach it from any
      webview; the launcher refuses such an address with that reason instead of letting it
      be cut. Whether real addresses stay below it is seen at the first real sign-in.
- [ ] **LAN, same-machine servers and NetherNet are refused** (bedrock-net.md §4, §6): LAN and
      loopback servers need a per-server `allow_private` in the shared `BedrockServer` (a
      product decision); NetherNet (friends' worlds, NetherNet Realms) connects to addresses
      its signalling negotiates, which the relay cannot tie to a name or an approval.
- [ ] **The client's build downloads curl without a checksum** (threat model B3):
      mcpelauncher's manifest builds curl 8.21.0 statically from a download it does not
      check (`ext/curl.cmake`, `USE_OWN_CURL=ON` by default), and the AUR PKGBUILD of
      `mcpelauncher-linux` 1.8.7-1 keeps that default. Not the launcher's code; read in the
      manifest, never built here. Whether the client builds and runs against the system's
      libcurl (`USE_OWN_CURL=OFF`) was not tried.
- [ ] **Featured servers** (bedrock-net.md §4) come by host name from the gatherings
      discovery: you must list them, and the refusal line says how. The broker could learn
      their names from that answer and say so (not built).
- [ ] **Unlisted IP addresses typed into the game** never reach the relay (no route), so they
      cannot be reported; a local route for all addresses in the namespace would let the
      relay see, refuse and report them (a change to `sandbox::netns`, not made).
- [ ] **A tripwire for a failed token split** (bedrock-risk.md §7, a proposal): a session
      whose game reaches `xsts.auth.xboxlive.com` or `sisu.xboxlive.com` without a sign-in
      through the bridge had a token from somewhere; the launch could say so at its end.
      Not built.
- [ ] **`scram --bedrock-data`** (bedrock-risk.md §7, a proposal): delete Bedrock worlds and
      settings too, for a user who suspects the game kept a sign-in there. Not built; scram
      keeps them and says so.
- [ ] **`--join` for Bedrock**: the client takes `-u <minecraft:// URI>`, whose form for a
      server was not verified; `launch --join` refuses a Bedrock instance and says to list the
      server instead.
- [ ] **The `bedrock` commands with `--events`** (bedrock-store.md §5): not offered, so a front
      end cannot list or import versions; the window offers none of them.
- [ ] **The front end's demo switch** stays visible beside a Bedrock instance, where it changes
      nothing (bedrock-integration.md, decision 8).
- [ ] **A terminal launch of Bedrock end to end** (bedrock-integration.md §7): the launch takes
      only a root-owned client under `/usr`, which a stand-in cannot be, so the launch tests
      give the client to `launch_with`; the terminal reader was run by its unit tests and a
      pseudo-terminal probe, not by a real launch.

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
      fullbright on in a cave — and pull the phone out with R: how the slide feels, where it
      stands, typing on your keyboard layout, dragging the Spotify sliders.
- [ ] **Login-protocol proxy**, **signed server allowlist**, **approval prompts beyond
      Realms joins**, **frozen `/usr`**: see "Decided against / deferred". Each is days
      of work; say which.
- [ ] **Approving a Realms join of a Java launch from the front end** (realms-sync.md §7):
      it would let vanilla instances (no phone) approve joins. Since the fifth round the
      front end can send answers (`answer realms <code>` on the launcher's standard input, the
      trusted input of `src/events/input.rs`), and a Bedrock session takes its Realms codes
      that way (`realms::answers`); a Java launch still takes the code only from the phone.
      Whether a Java launch should take it from the front end too is the decision; wiring it
      is small (the Bedrock launch shows how).
- [ ] **Clips in a burner session**: `src/launch.rs` prepares the recorder for a `--burner`
      launch like for any other (the browser is not started there, Spotify has no network,
      settings sync and Realms take no part). A clip saved there stays in the clips folder:
      it is the launcher's file, not the game's. Whether a session for untrusted content
      should record at all is not decided.
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
- [ ] **Further companion features**: the fourth round added the phone, the home screen and
      the dark menus, nothing else. Candidates that fit "harmless client-side convenience,
      nothing servers commonly forbid": a toggle mode for zoom, the smooth camera while
      zoomed, a clock line in the HUD read-out. Deliberately not candidates: free look, a
      minimap, anything that automates input or shows what the unmodified game does not
      show.
- [ ] **The companion mod beyond the tested combinations**: the representative of each of
      the ten families (1.20.1, 1.20.4, 1.21.1, 1.21.4, 1.21.5, 1.21.8, 1.21.10, 26.1.2, 26.2,
      26.3) with Fabric Loader 0.20.1. The other versions of a family get the same adapter
      with `force`, built against identical stand-ins, and were never started; a Fabric
      Loader too old to know `fabric.addMods` would ignore it silently (the launcher would
      say "added" and no hello would follow); Quilt is not supported. Adding a version is a
      procedure: run `tests/companion_game.rs` against it, then list it in
      `companion::TESTED_MINECRAFT`.

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
      shader does with it was read, not watched); zoom, the HUD read-out and fullbright on
      26.3 only (elsewhere they switch themselves off); the Wayland path with a headless
      sway only and no key press there; no multiplayer; reproducibility across compilers
      (one JDK is installed; two builds with it give the same bytes).
- [ ] **Companion mod across versions, protocol 2, the dispatcher** (mod-platform.md part 4):
      the versions of a family other than its representative (identical stand-ins by
      construction; never started); typing, clicks and the wheel as devices in the
      platform's own runs (the phone key was really pressed on every family; mod-ui's
      driver typed and clicked through XTEST); the Wayland path and the game as a direct X11
      client (only the nested compositor); Fabric Loader other than 0.20.1, Quilt; the
      browser's 15 pictures a second through the channel (one 200 × 120 page was carried and
      drawn; the cost at that rate was not measured); JPEG pictures decoded in the game (only
      in the JVM harness: the demo sends none); the libFuzzer target `phone`.
- [ ] **The phone, the home screen, the dark menus** (mod-ui.md part 5): the real services
      behind the phone — every in-game check ran against the demo service, and no record
      of this round shows the phone in the game driving the real Spotify, browser, clips,
      Realms or settings services; mouse drags in the game (seek, volume, dragging on a web
      page: the harness only); the title screen with an account and the Mods button (the
      stand-in platform only: no account, no Mod Menu here); Wayland; GUI scales other than
      the game's automatic one at two window sizes and scale 1 on 26.3 (the layout rules are
      proven on 13 sizes by the harness, not in the game); a person at the keyboard (XTEST,
      US layout); the clock's time zone (the sandbox has none unless `expose_timezone`).
- [ ] **The browser** (browser.md §5): Discord signed in (no account was used; the hosts of
      the signed-in client come from Discord's own page configuration and content security
      policy, read without signing in, and whether 300 API calls and 60 MB a minute suit a
      busy session is a guess from the defaults); the phone in the game driving it (the app
      was driven through its interface, in tests and in the launch's self-test); captchas,
      two-factor prompts, passkeys; Firefox other than 159.0.1, and the firefox-esr and
      developer-edition paths (listed, not tried); the relay through `launcher_proxy`; the
      libFuzzer targets `websocket`, `bidi`, `browser_command`; Landlock's effect on
      Firefox probed separately (the stage refuses to start Firefox unless the kernel
      enforces the ruleset, and Firefox ran).
- [ ] **Spotify** (media.md, "Not verified"): the real Spotify desktop app (never started:
      it is yours; its MPRIS behaviour is taken from documentation and from the stand-in that
      imitates it); the real Web API and accounts service (your client ID, and Premium for
      playback control) — whether Spotify rotates PKCE refresh tokens is handled both ways
      but was not observed; album art from the real `i.scdn.co`; the token in the real
      keyring or age vault, and scram's removal of it (its one fixed name would touch a real
      entry; the key list's parsing is unit-tested); a session longer than an hour (a
      refresh through a real keeper); the dispatcher and the phone (the app was driven
      through its outbox and a script of commands); the libFuzzer targets `dbus`,
      `spotify`, `keeper`, `picture`.
- [ ] **Clips** (media.md): saving from the phone in the game (the real-game runs used a
      script of phone commands); finding the game's window by its process on a desktop X
      server (the private X servers here report process 0; the fallback by the sandbox's
      host name ran); a reparenting window manager (handled in code); the recorder's
      resource scope (asked for, its limits not read back); the libFuzzer targets `clips`,
      `x11`.
- [ ] **Realms and settings sync** (realms-sync.md §5): the live Realms service — the
      endpoint table comes from the game's own client classes (javap, 1.20.1 to 26.3), not
      from watching real traffic; which ports Realms servers use (hence the dialer);
      whether Realms answers the rebuilt cookie as it answers the client's; whether a
      Realm's server accepts the join made through the broker for a pass connection; a real
      launch with `approve` set (demo mode turns Realms off, by design); `realms
      totp-setup` against the real keyring (deliberately not run), and scram's removal of
      the TOTP key; the phone side (the apps were driven through their interface); settings
      sync in a real burner launch (unit-tested only) and with other settings than the field
      of view; the libFuzzer targets `realms`, `sync`.
- [ ] **Vanilla instances after the bootstrap move**: vanilla 1.21.4 started; the other
      versions from 1.20.1 to 1.21.11 were not started as vanilla instances, and the
      round's real-game runs of vanilla 26.3 were made before the move.
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
- [ ] **Bedrock: the client and the version store** (bedrock-store.md §3): a real
      `mcpelauncher-linux` package and its record in pacman's database, a version that
      `mcpelauncher-ui` downloaded, a real compiled `AndroidManifest.xml`, real Flatpak and
      AppImage installs, a mounted FUSE file system, device files in a download (a user cannot
      make them) — so the import's limits and the time a launch spends checking a real version
      are unmeasured; the libFuzzer target `bedrock_store`.
- [ ] **Bedrock: the launch and its sandbox** (bedrock-launch.md §3, bedrock-integration.md §7):
      the real client starting with these mounts, environment (PATH, HOME, the working
      directory `/`, the telemetry switch) and arguments; its allowlist complete; an empty
      read-only `mods/` accepted; SDL3/EGL, sound, the nested compositor or a Wayland security
      context with a Bedrock session (none ran with a display, a GPU or sound); resource limits
      (the tests use `cgroup = "off"`) and the default memory ceiling against the game; where
      the game keeps its Xbox tokens, and that nothing of the sign-in lands in the kept folders.
- [ ] **Bedrock: the network** (bedrock-net.md §3): a real Bedrock server, Realm or event
      server; the real Realms API and gatherings service (their paths and answers were read in
      a third-party client, gophertunnel); whether the game's TLS stack accepts the per-launch
      authority (shown with Arch's curl and OpenSSL); whether the client binds fixed UDP ports,
      pings and connects from different sockets, or opens a flow with something other than a
      ping or connection request; an approved join to an IPv6 address; the libFuzzer targets
      `raknet`, `bedrock_realms`, `bedrock_gatherings`.
- [ ] **Bedrock: the sign-in** (bedrock-signin.md §3, bedrock-integration.md §3): that the
      client executes the stub and takes its line, and that the game's sign-in library accepts
      the rebuilt address; Microsoft's real start address and its scope, its last redirect,
      two-factor steps, passkeys, account creation; a real browser following the last redirect
      into the desktop's handler (the path was exercised with `gio open`, not a browser);
      Chromium-based browsers (where they show the final address); the background race of a
      terminal launch; the libFuzzer targets `signin_frame`, `signin_url`, `answers`.
- [ ] **Bedrock: the gate, scram, doctor and the canary** (bedrock-risk.md §3): the real game's
      token files in scram; `doctor`'s client line against a real package; a cape claimed in
      Bedrock reaching the Java profile, and bastion-mc's Java sign-in (never live); Microsoft's
      sign-out pages were not fetched again in this round.
- [ ] **Bedrock: the Arch package installed** (bedrock-integration.md §7): `makepkg` built it;
      nothing was installed with pacman, so desktop-file-utils' hook was read, not run on it.

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
- [ ] **A Realm** (with the signed-in account): Realms through the broker against the live
      service — the list, a join held for its approval, approved in `code` and in `totp`
      mode, the pass connecting to the Realm's server; tested against a stand-in Realms
      service only. First things to run: `[realms] approve = "code"` in a Fabric instance
      with the phone, a join; then `bastion-mc realms totp-setup` and `approve = "totp"`.
- [ ] **Minecraft: Bedrock Edition, for real** (the fifth round; nothing of it has met the real
      client or game): `mcpelauncher-linux` and `mcpelauncher-ui` built from the AUR (read
      their PKGBUILDs first), Minecraft bought on Google Play — with a Google account of its
      own — and a version downloaded with `mcpelauncher-ui`; then `[bedrock]
      accept_licence_substitution = true`, `bastion-mc bedrock import`, `bastion-mc instance
      new … --bedrock`. First things to run: one session with `[bedrock] seccomp = "log"` (see
      Open); a session signed in with your Microsoft account (two-factor steps or a passkey;
      the browser handing the address back through the handler, and pasting it); a second
      session, which must ask for the sign-in again.
- [ ] **A Bedrock server, a Realm, an event** (in a signed-in Bedrock session): a listed
      server; a Realm join approved in the front end's panel and on the terminal (`[realms]
      approve = "code"`); an event server when Mojang runs one (none was running on
      2026-11-14); whether a challenge is credited with telemetry on, and off.
- [ ] **A cape claimed in Bedrock reaching your Java profile**: `bastion-mc skin show` listing
      it and `bastion-mc cape set` wearing it — which also needs bastion-mc's own Java sign-in
      (the first item of this list).
- [ ] **A Spotify client ID and a Spotify account** (Premium for playback control through
      the Web API): `bastion-mc spotify login`, then a launch with the phone — search,
      queue, devices, control without the desktop app; and the Spotify desktop app itself
      running, for MPRIS against the real app (only a stand-in player was used).
- [ ] **A Discord sign-in**: `bastion-mc browser login discord`, then a launch with the
      phone's browser — whether the preset's hosts are enough for the signed-in client, and
      whether its budgets (300 API calls and 60 MB a minute) suit a busy session.
- [ ] **The Spotify token and the Realms TOTP secret in the real keyring**, once, with your
      consent: their names are fixed and the same for everyone, so no test may touch them;
      storing them and scram's removal of them never ran here.
- [ ] **A real Wayland desktop session** (game and GUI; the phone, and `browser login` as a
      client of your compositor — clips cannot record there, by design), **NVIDIA**,
      **Java 8-era versions end to end with the broker** (TLS 1.2 fallback flag), **a real
      `age` binary and hardware plugins** (`vault reseal`, `doctor`'s identity and recipient
      classification).
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
      gamescope, the companion mod by feel, and the phone — how pulling it out feels, where
      it stands, typing on your keyboard layout, the sliders — which only XTEST events on a
      US layout have touched (see Open).
- [ ] **A real SOCKS5 proxy (Tor)**: the B-M3 fix and Modrinth through the proxy are tested
      against a stand-in proxy only.
- [ ] **`scram --yes` from the window against the real launcher**: never run here, because it
      removes the real sign-ins of this machine; worth doing once on a machine or account
      whose sign-ins you can lose.
- [ ] **PKGBUILD**: the real `url` (a value is there, still marked TODO), your
      release-signing key in `validpgpkeys`, a signed tag. Everything else in it is done
      (auditable build, reproducibility notes, vendoring recipe, the documents installed as a
      tree — `docs/PHONE.md`, `COMPANION.md` and `MODS.md` since the fourth round,
      `docs/BEDROCK.md` since the fifth —, the phone's optional packages, and the desktop entry
      of the Bedrock sign-in handler with desktop-file-utils as optional).
- [ ] **Coverage-guided fuzzing**: `rustup toolchain install nightly && cargo install
      cargo-fuzz`, then `fuzz/README.md`. The 31 targets (seven of them the fifth round's:
      `bedrock_store`, `raknet`, `bedrock_realms`, `bedrock_gatherings`, `signin_frame`,
      `signin_url`, `answers`) compile (`cargo check` in `fuzz/`, last run by the integrator of
      the fifth round on its merged tree); they have never run.
- [ ] **`mc-vault` account / `ptrace_scope=3`**: documented (`docs/MC_VAULT_ACCOUNT.md`),
      needs root; not exercised.
- [ ] **Minecraft help-centre wording in the runbook**: their site needs a browser; only
      the request-form address was checked to exist. Read it once and adjust phase 3.

## Done in the fifth round

Minecraft: Bedrock Edition, the Android build. The record of each package — what was
verified, how, and what was not — is its notes file in `docs/notes/` (index:
`docs/README.md`); the contract the five packages built against is
`docs/notes/round5-contract.md`, the research behind it `docs/notes/bedrock-research.md`, and
the integration `docs/notes/bedrock-integration.md`. Where a note and the code disagree, the
code is right. Nothing of this round has run with the real client, the game, a Google Play
purchase or a Microsoft account.

### The installed client and the version store — `docs/notes/bedrock-store.md`
- `bedrock::runtime` finds the installed `mcpelauncher-client` (`/usr/bin`, `/usr/local/bin`,
  or `[bedrock] client`) and holds it to a stricter form of the JDK's rule: root-owned, every
  directory and link hop on the way root-owned and writable by nobody else, not under the
  home, not on FUSE, an x86-64 program, not set-user-ID; Flatpak and AppImage installs
  refused, each with the reason; the client's data directory checked file by file; every
  place where the client could find a sign-in window listed; the version taken from pacman's
  database (the client is never run outside the sandbox).
- `bedrock::store`: `bastion-mc bedrock client | import <dir> [--id ID] | versions [--verify]
  | remove <id>`. The import copies only what the client reads (x86_64 builds only), refuses
  links, hard links, FIFOs, sockets, devices, names that are not plain, trees beyond its
  limits and any APK, works relative to directory descriptors, ends when a file changes while
  it is copied, writes a SHA-256 manifest last and leaves the copy read-only; every launch
  checks every file from the directory it then mounts; `remove` is refused while a launch
  holds the version (a shared `flock`) or a marked launch names it.
- Verified: 14 tests of the discovery on fake install trees, 19 of the store, unit tests of
  its compiled-XML, manifest and ELF readers, 3 of the commands end to end with the real
  binary (`tests/bedrock_store.rs`); `/usr/bin/true` as a root-owned program from a package;
  the `bedrock_store` harness, once 3 × 150 000 and 2 000 000 generated inputs.

### The launch and the client's sandbox — `docs/notes/bedrock-launch.md`
- `Kind::Bedrock` and `instance new <id> --bedrock <version>`; Java keys in a Bedrock
  instance refused with the reason; the Java-only commands refuse it; `launch` refuses
  `--burner`, `--join` and `--repair` and takes `--demo` with a note that it changes nothing.
- The client's sandbox profile: only the client, its libraries, the loader and the stub may
  run; the version read-only, by descriptor; a data root of the session's own, into which
  only the worlds, `games/com.mojang/minecraftpe` and the client's settings file are bound
  from the instance's `bedrock-data/` (none of the settings file's sixteen keys names a path
  or a program: checked in the client's source); `mods/` empty and read-only; a system-call
  allowlist of the client's own with the tripwire, and a log mode for profiling it that still
  refuses namespaces, keyrings, io_uring, userfaultfd and perf events; `[bedrock]
  extra_syscalls`; network `filtered` or `none`, never `open`; a sixth protection aspect,
  `sign-in`.
- Verified: `tests/bedrock_launch.rs` (a stand-in client the test compiles, run in the real
  sandbox), `tests/bedrock_sandbox.rs` (the client's filter in the kernel, 3),
  `tests/bedrock_cli.rs` (4), unit tests. Its branch: `cargo test --workspace` 799 passed.

### The network — `docs/notes/bedrock-net.md`
- The relay's Bedrock profile: HTTPS by name only (exact names, sub-domain suffixes, and a deny
  list that wins over them: the Java services and NetherNet's signalling); RakNet over UDP to
  the servers you list, a flow opened only by a RakNet ping or connection request, only to the
  address looked up for a listed server, within limits; Realms and event-server joins through
  a broker that terminates TLS for four hosts with a name-constrained authority made for each
  launch, holds the join answer and hands the game a one-time pass once you approved outside
  the game; NetherNet and private addresses refused with the reason; `prepare` before the
  sandbox, `start` after the launcher has confined itself.
- Verified: unit tests of the rules, the RakNet parser, the relay's table and the broker;
  `tests/bedrock_net.rs` (7): a sandboxed Python and curl client against a stand-in RakNet
  server and stand-in Realms and gatherings services over TLS, also with the launcher
  confined; harnesses `raknet`, `bedrock_realms`, `bedrock_gatherings` (once 60 000 inputs
  each with three seeds). Its branch: `cargo test --workspace` 817 passed.

### The sign-in outside the game — `docs/notes/bedrock-signin.md`
- The stub: the launcher's own binary under the name `mcpelauncher-webview`, where the client
  looks for its sign-in window. The bridge server checks each request — only Microsoft's page
  for Minecraft for Android's own client, its redirect, `response_type=code`, the Xbox scope,
  PKCE with S256, a state; no override or return-address parameters — and shows the page
  outside the game; the address is taken only from the trusted input (the front end's `answer
  signin` line, the terminal while the launcher is its foreground job), checked against this
  sign-in's state, and only its code and state go back to the game; prompts and connections
  are bounded. New events `SignInPage` and `Answerable`; the front end's *Microsoft sign-in for
  Bedrock* panel; `realms::answers`, which takes Realms codes typed outside the game.
- Verified: the answer grammar, the trusted input, the URL checks and frames, the server over
  real sockets (14 tests now, the handler's included), the real binary as the stub in
  bubblewrap under an allowlist of exactly its 30 system calls (`tests/signin_stub.rs`, 4),
  the front end (logic, views, the emulated application), Firefox 159 against a stand-in
  redirect page; harnesses `signin_frame`, `signin_url`, `answers`. Its branch: `cargo test
  --workspace` 830 passed.

### The gate, scram, doctor and the canary — `docs/notes/bedrock-risk.md`, `docs/BEDROCK.md`
- `policy::gate` (every Bedrock launch refused until `accept_licence_substitution = true`,
  with what is accepted and what the launcher does not do); the sessions' data directories
  (`bedrock-sessions/` in the cache, 0700, locked, removed at the session's end, and what a
  killed session left at the next Bedrock start); `doctor`'s Bedrock section; scram stops a
  Bedrock session (a marker of its own kind) and deletes what Bedrock sessions left, keeping
  the worlds and saying so; the canary names a gained cape and pins it, and the lines after a
  signed-in session say where a cape shows; the user guide `docs/BEDROCK.md` (written by this
  package, checked against the merged code by the integrator, and once more by this pass).
- Verified: unit tests; `tests/scram.rs` (15, four with stand-in Bedrock sessions); `doctor`
  and scram's confirmation in isolated directories. Its branch: the launcher 651 tests, the
  front end 126, the protocol 7, none failed.

### Integration — `docs/notes/bedrock-integration.md`
- The five packages merged and reconciled decision by decision (its section 2): one data
  directory per session (the risk package's) with the launch package's kept parts bound into
  it; the version held for the whole session, a duplicate of its descriptor mounted; every
  installed sign-in window covered by the stub; `PATH` exactly `/bastion/bin:/usr/bin` and the
  working directory `/`; the bridge served after the confinement, the stub's system calls held
  to the client's filter; the network package's `prepare`/`start` in place of the launch
  package's placeholder rules; Realms answers through one path (`totp` acts as `code` in a
  Bedrock session); the gate first, the `bedrock:` marker, leftovers removed before the
  session; `[bedrock] telemetry = false` also sets the client's own
  `MCPELAUNCHER_DISABLE_TELEMETRY=1`; the front end plays a Bedrock instance without a Java
  sign-in or demo mode. Found while integrating: `bedrock remove` read only `launch:`
  markers, and the `bedrock_store` harness had no libFuzzer target; both fixed.
- **The desktop's sign-in handler** (its section 3): the packaged desktop entry for
  `x-scheme-handler/ms-xal-0000000048183522` runs `bastion-mc bedrock hand-back <address>`,
  which hands the address to the running session through a socket in your runtime directory
  (0600 in a 0700 directory, bound before the confinement, the peer's user id checked); the
  session checks it as it checks a pasted one, and a refused one does not use up the prompt's
  tries; one session holds the handler, a second falls back to pasting and says so. The
  PKGBUILD installs the entry and `docs/BEDROCK.md`, and names desktop-file-utils as optional
  for its hook. Verified by unit tests, the end-to-end test (the real command, with the
  launcher confined) and `gio open` in isolated XDG directories.
- Verification of the merged tree: above, and its section 6.

### Documents (this pass)
- README: an introduction paragraph, Status rows for Bedrock and a recount, a section
  "Minecraft: Bedrock Edition", the front end's sixth aspect and sign-in panel, the Java-only
  statements about UDP and Realms qualified, the emergency paragraph, configuration, layout,
  tests (the new suites, and the temporary directory the suite assumes: re-measured), the
  documents and the source map. `docs/THREAT_MODEL.md`: the new asset and adversaries, the
  trust boundary of a Bedrock session, section 4.8 with rows B1 to B39 (the integration's
  consolidated table), section 5's items 17 to 19 and item 7's Bedrock sentence, a deviation,
  experiments E33 to E37, what was built and what not. `docs/SCRAM_RUNBOOK.md` (the Bedrock
  step; `runbook::LINKS` unchanged), `docs/TOKEN_INJECTION.md` (Bedrock sessions have no Java
  token), `docs/README.md` (the guide and the round's notes), `docs/BEDROCK.md` (one sentence
  corrected against the code; its maintainer comment taken out for the website), this file.
- The website: `bedrock.html`, and Bedrock on the overview, getting started and the security
  overview (`docs/notes/website.md`, section 12).

## Done in the fourth round

The record of each package — what was verified, how, and what was not — is its notes file
in `docs/notes/` (index: `docs/README.md`); the contract the five packages built against is
`docs/notes/round4-contract.md`. Where a note and the code disagree, the code is right.

### The companion mod on 1.20.1 to 26.3, protocol 2, the dispatcher — `docs/notes/mod-platform.md`
- Ten families of Minecraft versions (`java/stubs/tools/families.txt`), established from
  Mojang's manifest and mappings and Fabric's intermediary mappings: versions whose
  stand-ins declare the same signatures form one family. One adapter per family
  (`java/companion/platform/`), picked when the game starts; Mixin applies only that
  family's mixins. The launcher adds the mod by itself to each family's representative
  (`companion::TESTED_MINECRAFT`), to the other versions of a family with `force`.
- Stand-ins of Minecraft's classes per family (`java/stubs/minecraft/<family>/`: names and
  signatures only), generated offline by `java/stubs/tools` from the published files, each
  checked against its publisher's hash and recorded in `SOURCES.txt`; `regenerate.py
  --check` repeats it. The build (`src/companion/jar.rs`) compiles in passes, requires code
  shared by a group of families to give the same class files against each family, and stays
  reproducible.
- Protocol 2 of the companion channel (framed messages both ways after `hello 2`; at most 50
  messages and 200 KiB a second from the mod) and the dispatcher (`src/phone/dispatch.rs`:
  strict parsing, routing by exact app name, fixed vocabularies, a token bucket per verb,
  each kind of refusal logged once); the demo service (`src/phone/demo.rs`, switched on by
  `BASTION_PHONE_DEMO=1`, never by configuration). New `[companion]` keys `phone`,
  `phone_key`, `home`, `dark_menus`.
- Verified: in the real game on every family's representative, four rounds each — the title
  screen, the demo world, the demo world with the phone key pressed through XTEST, the
  title screen with the final jar and a dark menu background — all passed (the table is in
  the notes, part 4); unit and integration tests of the jar, the settings, protocol 2 and the
  dispatcher; the mod's reader of the launcher's frames under 4 000 mutated frames a run; the
  `phone` harness.

### The phone, the home screen, the dark menus — `docs/notes/mod-ui.md`
- The phone (key R; Spotify, Discord, Browser, Clips, Realms and Settings — only the apps
  the launcher announces), its notices and keyboard-only use; the redone title screen; one
  dark theme and darker backgrounds behind the game's own menus. Interface only, built on
  the platform API alone (`java/companion/src/bastion/companion/` `ui/`, `phone/`, `home/`,
  `theme/`).
- Verified: `tests/phone_ui.rs` (default suite) drives every app in a stand-in game — every
  command line parsed by the launcher's own parser and matched to the contract's
  vocabulary, every draw call on 13 window sizes inside its place (0 violations; with text
  drawn unshortened, as a mutation check, 13 131), 27 hostile and 300 mutated launcher
  messages; in the real game on all ten families at two window sizes, in the demo world on
  26.3 and 1.21.4, at GUI scale 1 on 26.3, against the demo service, with
  `tests/support/phone-shots.sh`, every screenshot looked at. What the real game showed
  wrong was fixed (the notes list it). The shared branch of the two mod packages: `cargo
  test --workspace` 601 passed.

### The phone's browser — `docs/notes/browser.md`
- Firefox, headless, in a bubblewrap sandbox of its own whose network namespace holds only
  the launcher's relay (names resolve only for the open site's hosts, a connection is taken
  only when its TLS server name is one of them, budgets per site); inside, the stage
  hardens itself (`no_new_privs`, a fresh session keyring, Landlock, a seccomp denylist that
  leaves Firefox's own sandbox working); WebDriver BiDi over a socket pair, every request
  decided by the launcher. `[browser]` (at most 8 sites of up to 32 hosts, the `discord`
  preset, four budgets per site), `bastion-mc browser login <site>`; no browser in burner
  sessions.
- Verified: the relay against stand-in sockets (unit tests); Firefox 159.0.1 in its sandbox
  against stand-in HTTPS sites (6 of 6), the internet without signing in (example.com,
  Discord's sign-in page), the sign-in window and the real command on a private display (as
  an X11 client and on the same compositor's Wayland socket); a real demo launch with the
  browser's self-test, and a walk through the game's sandbox that found no file of the
  profile; Firefox's own seccomp filters present in its content processes; 200 000
  mutations per new harness. Its branch: `cargo test --workspace` 622 passed.

### Spotify and clips — `docs/notes/media.md`
- Spotify: the desktop app over MPRIS through `xdg-dbus-proxy` in a sandbox of its own (the
  player and properties interfaces of the Spotify app's name on its object path, nothing
  else), album art fetched by the launcher and encoded again; the Web API with your own
  client ID (`bastion-mc spotify login|logout|status`, PKCE, local budgets per kind of call,
  429 honoured), its refresh token held by a token keeper process outside the launcher.
  `[spotify]`.
- Clips: ffmpeg in a sandbox of its own records the game's window into a buffer in memory;
  saving joins whole segments by name, checks their packets and never overwrites; limits on
  saves, number and size remove only clips this feature saved. `[clips]`, off by default.
- Verified: 30 Spotify and 16 clips unit tests; `tests/spotify.rs` against a private D-Bus
  daemon with a stand-in player and the real proxy, and against a stand-in Web API over TLS;
  `tests/clips.rs` (ffprobe, a confined child, the recorder on a private display); the real
  game in `tests/clips_game.rs`, through the nested compositor and on the session's X
  display — an 8-second clip saved by a script of phone commands, MPRIS commands through the
  proxy after the launcher had confined itself; 150 000 mutations per new harness once;
  `cargo deny check` with the new dependency `zune-jpeg`. Its branch: `cargo test
  --workspace` 655 passed.

### Realms and settings sync — `docs/notes/realms-sync.md`
- Realms: `[realms] approve = "off" | "code" | "totp"` (off by default). The broker
  terminates the Realms hosts and rebuilds the session cookie around the real token for the
  requests on a fixed table (owner operations refused locally); a join is held until the
  launcher's six-digit code (or a TOTP code: `bastion-mc realms totp-setup`) is typed into
  the phone; the approved join gets a one-time pass, which the relay connects — through a
  dialer started before the launcher confines itself — to exactly the address Realms named.
  The front end shows the prompt (a new event, `realms_approval`).
- Settings sync: a list of `options.txt` settings (`src/sync/keys.rs`) merged before the
  game starts and written back at exit (last writer wins per setting), translated between
  versions as Minecraft's own data fixers do where an exact inverse exists; the phone's
  Settings app; `[sync] options = true` by default.
- Verified: 17 sync and 21 Realms unit tests (the RFC 6238 and RFC 4226 test vectors among
  them); `tests/realms.rs` against a stand-in Realms service over TLS with the real broker,
  the real dialer in its sandbox and the real relay with a client in a private network
  namespace; the front end's prompt in its tests; real demo launches 1.21.4 → 26.3 → 1.20.1
  carrying a field of view changed in the game (`docs/notes/realms-sync-evidence/`). Its
  branch: `cargo test --workspace` 633 passed.

### Integration (`d3f6931`, `a533e43`, `c6f25a8`, and the documents)
- One list of phone services in `src/launch.rs`: Spotify, clips and the browser are started
  only when the launch's companion has the phone on (a phone to serve); settings sync is
  prepared for every launch but a burner session, and Realms only where the launch has the
  phone (its approval is answered there). The dispatcher serves the app of every prepared
  service on the companion channel.
- The launch bootstrap moved into a package of its own (`bastion.boot`): vanilla instances
  of 1.20.1 to 1.21.11 did not start before ("signer information does not match": the
  signed client jars of those versions keep their classes in the default package, where the
  bootstrap was). Found by the realms-sync package. Verified with vanilla 1.21.4 and with
  `tests/jvm.rs`, `broker.rs`, `companion.rs` and `voice.rs`.
- `scram` also deletes the phone browser's profile and its record of sign-ins (unless a
  browser still holds the profile: then `COULD NOT`), and a full scram removes the Realms
  TOTP secret from the keyring, confirmed against the kernel's key list as for the Spotify
  token, which it already removed. Test: `tests/scram.rs`,
  `scram_deletes_the_browsers_profile_unless_a_browser_holds_it`.
- Fixes found while the documents were written and reviewed (`de5fb18`, `8ad1e26`,
  `5dc136d`, `f6dcc1b`): the phone's services start only when the companion's phone is on,
  not whenever it has its channel; a Realms join in a launch without the phone is refused at
  once, with the reason, instead of waiting 120 s for an approval nothing could give; in
  network mode `none` no service goes online (no browser, no Spotify Web API or covers);
  burner sessions record no clips; the browser takes `text` at the phone's typing rate
  (600 a minute, 60 at once) and scrolls of up to 10000 pixels, Spotify `seek` and `volume`
  at a dragged slider's rate (240 a minute); scram's confirmation names everything it
  removes; the front end's runbook panel no longer says the window opens no other pages;
  `fuzz/Cargo.lock` holds the new decoder; the companion's log line says that phone
  commands go to the dispatcher; the browser's self-test (a test hook) runs without a
  phone again (`tests/browser_game.rs`); the PKGBUILD installs `docs/PHONE.md`,
  `COMPANION.md`, `MODS.md` and the README's images, and names firefox, xdg-dbus-proxy and
  ffmpeg as optional dependencies; the header of `tests/jvm.rs` names the bootstrap's new
  class.
- The website (`cce21f4`, brought up to rounds 3 and 4 before it was committed: the three
  hand-written pages rewritten where the facts had changed, `docs/PHONE.md`,
  `docs/COMPANION.md` and `docs/MODS.md` rendered, the threat model's table repairs retired,
  HTML comments in documents left out as GitHub leaves them out). What its review found in
  the documents was fixed in `42b879e`: Realms is brokered only in a launch with the phone,
  burner sessions record no clips, DNS is UDP too (§5 item 7), `audio = "alsa"` gives the
  game `/dev/snd` and its device ioctls (D4, the §3 drawing, `examples/config.toml`), the
  front end holds no token, and its scram line names everything scram removes.
- CI (`e0b93fe`): `.github/workflows/ci.yml` and the manual `integration.yml` (see Open).
  Simulating its test job found that `check()` needs fonts: `PKGBUILD` has
  `checkdepends=('adwaita-fonts')` now (without a monospace face two layout tests fail, and a
  clean chroot has no fonts at all). Its **first run on GitHub** (run 43580491851,
  2026-11-14, after the first push) passed formatting and lints, the fuzz check and `cargo
  deny`, and failed two jobs for what the local simulation could not show, both fixed in the
  commit after that run: the package job's `git archive` ran as root in the container on a
  checkout owned by the runner's user ("dubious ownership"; now `git -c safe.directory=…` for
  that command, reproduced and checked in a user namespace), and two unit tests of
  `src/spotify/picture.rs` ran `ffmpeg`, which the container lacks (they now read two sample
  JPEGs from `src/spotify/testdata/`; the PKGBUILD's `check()` in a clean chroot would have
  failed the same way). Its **second run** (run 43581607682, 2026-11-14, on that fix — the
  tree the fifth round started from) **passed all five jobs**: lints, fuzz, the tests in the
  Arch container (the launcher's unit tests, the protocol, the front end, the website), `cargo
  deny`, and `makepkg`. The manual `integration.yml` has still never run (Open).
- Documents (this pass): the README (the phone and each app, the home screen and dark menus,
  Realms, settings sync, the versions, the new commands, configuration sections and tests,
  one Status row per part), `docs/README.md`, `docs/SCRAM_RUNBOOK.md` (what scram now
  removes, and the sessions a site keeps), this file. `docs/PHONE.md`, `docs/COMPANION.md`,
  `docs/THREAT_MODEL.md` and `docs/TOKEN_INJECTION.md` were a second writer's part of the
  same pass.

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

### Integration (`edc7efc`, `785c2a6`, `70357c6`, and the commit after the documents)
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
  off-machine or in hardware. Design exists (mod-manifest model); not started. (The Realms
  TOTP approval of the fourth round guards against code in the game, not against other
  programs of yours: its secret rests in the same token store as the refresh tokens.)
- **Fabric companion mod (old §7).** Enforcement must live in the launcher; the mod would
  be interface only. The launcher-side gates exist for packs; servers and account requests
  are per-config-switch today. Per-request approval needs a prompt channel first.
  *Third round:* built as exactly that — an interface-only convenience (zoom, HUD read-out,
  fullbright) whose one-way channel the launcher logs and never acts on. Approval prompts
  are still not built. Their design keeps the rule above: the decision is made and entered
  outside the sandbox, in the launcher's own interface; the mod can at most tell the player
  that the launcher is asking, and nothing arriving on the channel may create a prompt or
  count as consent (`docs/notes/companion-mod.md` part 2).
  *Fourth round:* the channel became two-way for the phone: the launcher now acts on
  commands from the game, but only within each app's fixed vocabulary and limits, and every
  secret stays with the launcher's services. One approval was built, for Realms joins, and
  it keeps only part of the rule above: the prompt comes from the launcher (from Realms'
  answer to a join, never from the channel) and is shown outside the game, but the code is
  typed into the phone — inside the game, where any mod can read it. In `code` mode the code
  is bound to its prompt; in `totp` mode it is not (`docs/notes/realms-sync.md`). Entering
  it outside the game would need a front end that sends the launcher more than `stop` (see
  Open).
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
# the companion mod in the real game (BASTION_COMPANION_WORLD=1: in the demo world as well;
# BASTION_E2E_INSTANCE: a signed Fabric profile of one of the ten versions; the phone gets the demo service):
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=fab \
    BASTION_COMPANION_WORLD=1 BASTION_NESTED_LAYERS=$(cat target/third-party/layers.txt) \
    cargo test --test companion_game -- --nocapture
# the phone, the home screen and the dark menus, one screenshot per step (demo service; it edits the instance):
tests/support/phone-shots.sh $PWD/target/e2e fab 77 target/shots 854 480 world
# the phone's browser: Firefox against stand-in sites (+ the internet, + the sign-in window on the private display)
BASTION_BROWSER_TESTS=1 BASTION_NET_TESTS=1 BASTION_BROWSER_LOGIN_DISPLAY=:77 \
    env -u DISPLAY -u WAYLAND_DISPLAY cargo test --test browser -- --test-threads=1
# the browser in a real demo launch (BASTION_E2E_INSTANCE defaults to vanilla-latest)
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_BROWSER_TESTS=1 \
    cargo test --test browser_game
# the clip recorder on the private display; then clips and Spotify in a real demo launch
BASTION_CLIPS_DISPLAY=:77 env -u DISPLAY -u WAYLAND_DISPLAY cargo test --test clips -- --nocapture
DISPLAY=:77 XAUTHORITY=/dev/null BASTION_E2E_ROOT=$PWD/target/e2e BASTION_E2E_INSTANCE=vanilla-latest \
    cargo test --test clips_game -- --nocapture --test-threads=1
```
