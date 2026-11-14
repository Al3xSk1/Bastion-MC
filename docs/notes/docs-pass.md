# Documentation pass: the reviews and what was done about them

The fixer's record of the documentation pass for the merged third round (branch `docs`,
on top of `c383196`; no code was changed in this pass). Two writers rewrote the README,
`TODO.md`, `docs/README.md`, `docs/MODS.md`, `docs/COMPANION.md` and folded round 3 into
the threat model; two reviewers then read the result, one against the code ("accuracy"),
one as a newcomer following the README in an empty data root on a private display
("newcomer"). They reported 33 findings.

Each finding was checked again before anything was changed: against the code, the help
text, the integrator's logs, or by running the command — always with the four XDG
variables pointing into `target/fixer-xdg` and on the private display `:77`. Where a
finding and the code disagree, the code wins. All 33 held (newcomer-1 with a correction);
three of them are about the website and need no change in a document.

## Findings

**accuracy-1** (README, 48-hour rule). Held: without `--allow-fresh`, `file_date` in
`src/profile/catalog.rs` returns `Error::Policy` (exit 5, "Nothing was changed."), and
`choose` says a refused file is an error, never a reason to fall back to an older build.
README now says that fresh bytes are refused and the command fails, and that
`--allow-fresh` lifts both rules. `docs/MODS.md` and row C9 were already right.

**accuracy-2** (README, what `BASTION_NET_TESTS` runs). Held: the variable gates
`tests/egress.rs` (`live_https_and_texture_gateway_through_the_relay`, `live_srv_lookup`,
the DNS-failure control in the pass-through test), `tests/install.rs` and the two `live_`
tests of `tests/catalog.rs`; `tests/broker.rs` uses a stand-in Mojang and never reads it,
and the egress harness runs without a broker. Table row fixed.

**accuracy-3** (README, "only Java 27"). Held, and the reason is wider: `/usr/lib/jvm`
holds `java-21-openjdk` (`jre21-openjdk`, installed 2026-11-13 19:51 by `pacman.log`,
three hours before the README's run) and `java-27-openjdk`; the launcher sees only 27
because the Java 21 directory has no `release` file (newcomer-1). `doctor` here: "system
JVMs: Java 27". The README's note on that run now says so.

**accuracy-4** (THREAT_MODEL §7, "only Java 27 is installed"). Held. Now "no Java 8
runtime is installed here, only 21 and 27".

**accuracy-5** (COMPANION, "two or three lines"). Held: `HudReadout.describe` returns the
frame rate alone (no player, or reduced debug information) or three lines. Fixed. The
class's own comment still says "Two or three lines" (code problem below).

**accuracy-6** (COMPANION, the untested remark "ends" the line). Held: in
`Plan::describe` the remark is followed by ". It is a convenience and enforces nothing."
Fixed.

**accuracy-7** (README, "runs the voice relay in a process confined that way"). Held:
`confined_child` in `tests/voice.rs` applies `confine::apply` and then uses a plain
`UdpSocket`; the relay's `GameSocket` and `open_upstream` do not run there. The Status row
now says what the test does and that the relay's own code ran confined only in §7 E22;
the gap is in `TODO.md` (Open). The voice row and J20 were already right.

**accuracy-8 / newcomer-2** (MODS, `profile add main …`). Held: `draft` in
`src/profile/catalog.rs` refuses an instance without a manifest (exit 2), and the
newcomer saw exactly that for the vanilla `main`. Both commands use `perf` now, with a
comment that it is a Fabric profile made with `profile new`.

**accuracy-9** (README and TODO, "28 refusals over TLS"). Held: the matrix
(`every_refusal_leaves_mods_the_manifest_and_the_signature_as_they_were`, 28 scenarios,
counted) runs against `modrinth::testing::Scripted`; the TLS stand-in is
`tests/catalog.rs` (an add and a check end to end, reconnection, a mismatching file, an
untrusted certificate, a fresh file, a deadline, the SOCKS5 proxy, two live tests). Both
places now name the two separately.

**accuracy-10** (THREAT_MODEL C7, G1, "about 230 crates"). Held: `cargo tree --offline -p
bastion-mc-gui` gives 203 crates for normal dependencies, 210 with build dependencies and
229 counting dev-dependencies, which are not linked. Now "about 200 crates (about 230
counting its test-only dependencies)". The launcher's "about 100" (99) stays.

**accuracy-11** (README intro, MODS opening, absolute "every mod"). Held: the companion
mod is added outside `mods/`. README: "every mod in `mods/`"; MODS names the one
exception in its first sentence.

**accuracy-12** (README and TOKEN_INJECTION, "no file access outside persist/ and the
cache"). Held: `src/confine.rs` keeps read-only access to `/etc`, `/usr` and `/proc`, and
`Confinement::supervisor` also the name resolver's configuration (the file, or its
daemon's directory under `/run`, when `/etc/resolv.conf` links there). Fixed in the
README, in TOKEN_INJECTION's table and in J18's defence column (its residual already said
so). The quick start's sample line "files only under the instance's persist/ and the
cache" is the launcher's own output, quoted word for word, and stays; the wording of that
log line is a code problem.

**accuracy-13** (TODO, missing open items). Held. Added under Open: the front end shows
the launcher's warnings as ordinary status lines (`Reporter::warn` sends a `Status` event
under `--events`; `gui/src/app.rs` puts a `Status` into the status line and the console);
no test of the default suite runs the voice relay's own code in a confined process;
leftover `wl-*` names are never swept (`src/launch.rs` unlinks only when the program's pid
is known; neither `src/sandbox/display.rs` nor `wayland.rs` sweeps); the flake "sandboxes
killed at start when many start at once" (voice-chat.md §7, (2)). The fifth item of the
integrator's list, `voice_host`, was already there.

**accuracy-14** (COMPANION, J19, TODO: when the socket name stays). Held: `Endpoint`
removes its name on drop, so a launch that ends with an error before the launcher has
confined itself removes it. It stays when step 8 cannot identify the program in the
sandbox (the launcher then confines itself and can no longer remove it) or when the
launcher is killed or crashes before that (release builds abort on a panic). All three
places now say that.

**accuracy-15** (README, the rail's squares). Held: `attention` in
`gui/src/view/mod.rs` marks Launch during a launch, Accounts while a sign-in waits, and
Security for a scram that is running, incomplete or could not be started and for any
protection off or reduced. Fixed.

**accuracy-16** (README, "the front end's tests render in software"). Held:
`.cargo/config.toml` selects the software renderer for `iced_test`'s simulator only; the
emulator tests pick at run time and create a headless GPU context where there is a GPU.
The Status row now says that 32 of the 118 unit tests lay out, click and render widgets in
software, and the Tests section separates the widget tests (snapshots) from the emulator
tests (the shader cache).

**accuracy-17** (README, "before the game starts"). Held: the display line is logged in
step 6, the limits and "confined itself" lines in step 9, after `sandbox::spawn` in step
8 — all before step 10 reads anything from the game (`target/readme-run` output). Now:
"Before anything the game prints is shown, the launch says what it set up and what it saw
in force".

**accuracy-18** (README, MODS, C8: SHA-1 always checked). Held: `VersionFile::sha1` is an
`Option`, the download compares it only when listed, and the plan prints it only then.
Fixed in all three, and in the README's description of the plan.

**accuracy-19** (README, timers). Held: `tick_interval` runs both gauge rates only while
the Launch page is shown. Fixed.

**newcomer-1** (README, `jre25-openjdk`). Held, with a correction: what is missing is the
`release` file by which `discover_in` recognises a runtime, and it is missing from Arch's
jre packages for Java 17 and later — not for Java 8. Checked: `jre21-openjdk` here
(`pacman -Ql`, the directory, and `doctor`, which lists Java 27 only); archlinux.org file
lists read on 2026-11-13: no `release` in `jre17-openjdk`, `jre25-openjdk`,
`jre-openjdk`; `release`, `bin/java` and `bin/javac` in `jdk17-openjdk`,
`jdk21-openjdk`, `jdk25-openjdk`; `jdk-openjdk` has it here; `jre8-openjdk-headless`, a
dependency of `jre8-openjdk`, ships `java-8-openjdk/release`, which `read_release` finds
from the `jre` home's parent (read, not run). The README now asks for `jdk25-openjdk`,
explains the `release` file, names the jdk packages and the Java 8 exception, and says
that the launcher's own refusal still suggests the jre package. The sentence "Building
needs `cargo`, a C compiler (the one Rust links with) and a JDK (`javac`)." is kept word
for word (the website quotes it).

**newcomer-3** (TODO, the PKGBUILD's Java hint names `jre25-openjdk`). Held. The item is
now "Java runtimes from Arch's jre packages are not found", with the evidence, the places
in the code and the PKGBUILD, and two ways to fix it.

**newcomer-4** (README, `makepkg -si` as an alternative). Held: the PKGBUILD's `url` is
marked TODO, `source=("git+${url}.git")` has no tag, and its header calls it a template.
The alternative is gone from the quick start; one sentence says that the PKGBUILD builds
what its `source=` points to, not this checkout. This breaks the website's REQUIRE of
"cargo build --release # or: cd packaging/arch && makepkg -si" (getting-started.html) on
purpose: that page should not offer the line either.

**newcomer-5** (README, MODS, `key generate` into a missing directory). Held:
`key generate` opens with `create_new` and `O_NOFOLLOW` and creates no directory;
reproduced here (exit 1, "No such file or directory"). Both now start with
`mkdir -m 700 -p ~/secure`.

**newcomer-6** (README, MODS, `launch perf` without a client ID). Held: `launch` refuses
without `auth.client_id` unless `--demo` or `--burner`. Both now say "add --demo until
your client ID is approved".

**newcomer-7** (README, where `config.toml` goes). Held. The quick start now says where it
goes, that it does not exist until you create it, and that `config example` and
`config path` help (both run here; `config example` is identical to
`examples/config.toml`).

**newcomer-8** (README, `bastion-mc-gui` after `cargo build --release`). Held: the root
package is the launcher (`Cargo.toml`). Now "built by `cargo build --release
--workspace`".

**newcomer-9** (README, `PATH` explained after the block). Held. The sentence comes before
the block, and the block sets `PATH`.

**newcomer-10** (`--help` never mentioned). Held (`bastion-mc help` is an unknown command;
`bastion-mc --help` prints the full list, checked here). A `bastion-mc --help` line is in
the build block.

**newcomer-11** (README, MODS, `config/ViaFabric/viafabric.yml`). Held: the file is in the
instance's `config/`, written at the mod's first start. Both now give the path under
`~/.local/share/bastion-mc/instances/<id>/`.

**newcomer-12, -13, -14** (website). Hold, and need no change in a document: the
README's heading "Display isolation" gives the anchor `display-isolation`, so the
website's links to `manual.html#the-display` (index.html, getting-started.html) need
updating; the REQUIRE statements listed below need re-reading, and security.html's
sentence that UDP does not exist in the sandbox is now false (§5.7).

## Also changed, reading the README through

* Configuration: the file was described as "mode 0600"; `util::require_private` refuses a
  file that is not yours, is a link, or that group or others can write (the newcomer's
  0644 file was accepted). The README now states that rule, with 0600 as an example.
* The Java paragraph of the quick start put in a clearer order, `config.toml`'s location
  after the build block, and a few paragraphs reflowed (the intro, the launcher's
  confinement, `profile add`, ViaFabric; the threat model's "not exercised" list).
* `docs/README.md` lists this file.

## Rendering

The website's converter (a6-site at `4ff0fb5`, `site/build.py --check`, on a copy of this
tree) reports no error that names `README.md` or a file under `docs/` with a line
number; with `--keep-going` the documents' pages are written and their links check.
`docs/MODS.md` and `docs/COMPANION.md`, which the site does not render, were converted too
with a copy of `build.py` that lists them: no error. What fails are REQUIRE statements in
the website's own pages, which quote older wording — 14 before this pass, 15 after it (the
new one is the `makepkg` line above):

* README.md: "7.0 lets the launcher confine itself" (index.html, security.html), "7.0 lets
  the launcher confine itself, 7.2 adds UDP rules" and "the window run on the X11 session;
  a real demo launch driven through its backend worker" and "cargo build --release # or:
  cd packaging/arch && makepkg -si" (getting-started.html).
* docs/THREAT_MODEL.md, all in security.html: "NVIDIA; Wayland; launching Java 8-era
  versions"; the J13 sentence ending "Replies that contain the token are withheld."; "Code
  in the JVM can still ask for what is switched on: one join per game connection …"; "A
  user-level launcher cannot defend the user from themself; … With gamescope the game is on
  a display of its own."; "on a display of its own. The session token inside the JVM …
  (one join per game connection, account reads)."; "account reads). Microphone with Pulse
  audio enabled (D3). …"; "syscalls/ioctls. Denial of service, partly: …"; "connection
  count are bounded.) Realms and UDP: … UDP (voice-chat mods, LAN discovery) does not exist
  in the sandbox."; "are the real ones. The account at Microsoft. … 6. Where the
  implementation deviates"; "removes every refresh token from the kernel keyring … No
  network needed.".

The other failures of a `--keep-going` run are about the website alone: three pages not
built (because of those statements) and four images and two drawings published but not
used.

## Test counts

README "Status" and `TODO.md` say 576, and that is what
`target/agents/integration/target/int-logs/test.log` holds for `c383196`, the code of this
tree: 30 `test result` lines (27 test binaries, 3 doc-test runs, 5 of the 30 empty), 576
passed, 0 failed, 0 ignored — the launcher 446 (319 unit, 127 integration), the front end
124 (118, 4, 2), the event protocol 6. `int-logs/run1/optin-net-x11.log` (the same tree,
with `BASTION_NET_TESTS=1 BASTION_X11_TESTS=1`) also sums to 576.

## When these documents are merged onto the integration branch

The integration branch has moved past this tree (`c588bea`, after the review of the merge
points). There, these places are no longer exact and need re-reading at the merge (from
the accuracy reviewer, checked against `git show --stat c588bea`):

* the test totals: 580 after the fixes (`int-logs/test2.log`), 581 with the network and
  X11 tests (`int-logs/optin-net-x11.log`, unit tests 322, `tests/companion.rs` 10) —
  README "Status", "What 576 tests counts", `TODO.md`'s verification state;
* the Simple Voice Chat hint now follows the instance's network mode — README "ViaFabric
  and Simple Voice Chat", `docs/MODS.md` (after the server-rule bullets), `TODO.md`
  ("Integration");
* step 8 now logs a warning when the companion socket keeps its name — `docs/COMPANION.md`
  ("The channel to the launcher"), J19's residual, `TODO.md` ("Stale markers");
* `tests/companion.rs` now has the channel served and read by a process confined as
  `launcher.confine = "require"` does it (default suite) — J19's "Verified by" and
  COMPANION.md's verification section could cite it;
* the front end's stop grace went from 30 to 90 s and the bound on protection details
  from 240 to 1000 characters; no document names either number (G4's "15 s" is the
  launcher's grace for the game, unchanged), so nothing needs re-reading there unless G2
  is extended;
* the `Protection` event is now documented in `crates/proto` as sent after the game was
  started and checked — the README's quick start and §7 E16 already describe that order.

## Code problems, for the integrator

Copied from the writers' and reviewers' reports, then the ones found in this pass. None
was changed here (documents only).

### From the README writer

* `packaging/arch/PKGBUILD` optdepends says 'jre-openjdk: current Minecraft releases'.
  jre-openjdk is Java 27, Minecraft 26.3 declares Java 25 (the launch prints 'game
  declares Java 25'), and with the default java.match = "exact" `src/java.rs` refuses Java
  27 for it. jre25-openjdk (in extra) is the runtime 26.x needs. *(Fixer: and the launcher
  does not find `jre25-openjdk` either — see the newcomer reviewer's item; the package to
  name is `jdk25-openjdk`.)*
* `packaging/arch/PKGBUILD` package_bastion-mc installs README.md, THREAT_MODEL.md,
  TOKEN_INJECTION.md, SCRAM_RUNBOOK.md and MC_VAULT_ACCOUNT.md, but not docs/MODS.md,
  docs/COMPANION.md or docs/README.md (nor docs/images/), all of which the installed
  README links to.
* `src/doctor.rs` lines 521-523 report 'N pinned host pattern(s)' using
  cfg.tls.pins.len(), which counts only [tls.pins]. With the default built-in pins doctor
  prints '0 pinned host pattern(s)', although launch.rs line 111 builds the PinSet with
  the built-in pins (tls.builtin_pins = true).
* The `src/paths.rs` module header says 'Nothing is ever written outside these four roots'
  and lists natives/ under the cache. In fact the launcher creates
  $XDG_RUNTIME_DIR/bastion-mc/wl-<random> (sandbox/display.rs line 170, falling back to the
  compositor socket's directory or /tmp) and companion-<random> (companion/channel.rs),
  and no cache natives/ directory is used. The README's layout now lists the runtime
  directory.
* `src/launch.rs` line 1443, status.code().unwrap_or(1): a game ended by a signal is
  logged as 'game exited with status 1'. This includes the scope SIGTERMed by systemd
  after an out-of-memory kill under DefaultOOMPolicy=stop. The catalog and voice notes
  suggest OOMPolicy=continue in src/sandbox/cgroup.rs; it was not applied. Recorded under
  Open in TODO.md.
* Note versus code: gui.md §1 says to keep the README's process diagram unchanged, but
  that diagram listed `install` among the commands the front end starts, and
  gui/src/backend.rs never starts install. It starts status, account default, login,
  logout, versions, instance new, launch and scram --yes. The README was corrected; the
  code is right.

### From the companion-documents writer

* `src/launch.rs`:723 calls companion::plan, which creates the channel socket, before
  sign-in (step 7, from line 811), the cgroup preflight (line 990) and the nested
  compositor start (line 1042); release_name follows only after sandbox::spawn (line
  1154). So the socket's name sits in $XDG_RUNTIME_DIR/bastion-mc/companion-*/ for
  seconds, not 'about a second' as companion-mod.md says. Other programs of the same user
  can connect in that window and be logged as the game. Low impact: the channel is
  informational, and such programs can do far more. Creating the endpoint just before
  sandbox::spawn would shorten the window. COMPANION.md and J19 describe the window as
  the code has it.
* `src/profile/author.rs`: the doc comment on `lock` says 'bump the serial and discard
  the old signature', but lock_with bumps the serial only when the mod list changed (the
  signature is always removed). MODS.md follows the code.
* `packaging/arch/PKGBUILD` (line 166) installs README.md, THREAT_MODEL.md,
  TOKEN_INJECTION.md, SCRAM_RUNBOOK.md and MC_VAULT_ACCOUNT.md; the new docs/COMPANION.md
  and docs/MODS.md are not installed. TODO.md lists it.

### From the accuracy reviewer

* The integration branch moved past this clone (`c588bea`); the places to re-read are
  listed in the section above.
* `java/companion/src/bastion/companion/feature/HudReadout.java`:12 says 'Two or three
  lines', but describe() (L107-122) returns one line (reduced debug info, or no player)
  or three.
* `src/paths.rs`:1-7 says 'Nothing is ever written outside these four roots' and lists
  'natives/' in the cache. In fact the launcher binds sockets under
  $XDG_RUNTIME_DIR/bastion-mc (companion-*: src/companion/channel.rs Endpoint::create;
  wl-*: src/sandbox/display.rs:171-172), and the cache holds scratch/ and running/
  (paths.rs:96, running.rs:53); natives-* appear only as leftovers.
* `src/launch.rs`:1241: the confinement log line says 'files only under the instance's
  persist/ and the cache'. The confinement keeps read access to /etc, /usr and /proc
  (src/confine.rs:15-17; THREAT_MODEL J18 residual). README quotes this line verbatim, so
  the fix belongs in the code (e.g. 'writes only under …; also reads /etc, /usr, /proc').
* `src/launch.rs`:1143-1145, comment on the Wayland socket: 'If that cannot be
  established, the name stays until the launcher exits'. By then the launcher has
  confined itself and cannot remove it, and wl-* names are never swept (INTEGRATION.md
  item 4; c588bea reworded these comments).
* Not code, for the integrator and the website team: `site/build.py --check` fails only on
  REQUIRE statements in site/src/pages/{index,getting-started,security}.html that quote
  baseline texts; some of them are now false (see "Rendering"). docs/COMPANION.md and
  docs/MODS.md are not among the documents the site renders, so README links to them
  become repository links.

### From the newcomer reviewer

* The launcher cannot find Java runtimes installed from Arch's JRE packages.
  `src/java.rs`:119-140 (discover_in) skips any JVM directory without a `release` file
  (read_release, src/java.rs:39-50, looks only in the home and its parent). Arch's
  jre*-openjdk packages do not ship that file: jre21-openjdk installed here has none and
  is not listed by `launch` or `doctor`, and the archlinux.org file lists of
  jre25-openjdk and jre-openjdk have none. Only jdk* packages are found. Yet the refusal
  at src/java.rs:169 recommends "jre{N}-openjdk, or jre-openjdk", doctor.rs:466
  recommends jre21-openjdk, and the PKGBUILD's depends=('java-runtime') and optdepends
  (jre8/17/21-openjdk, jre-openjdk; packaging/arch/PKGBUILD:153-160) can all be met by
  runtimes the launcher ignores. Options: determine the release some other way (the
  java-NN-openjdk directory name, the jimage header of lib/modules, or `java -version` run
  in the sandbox), or recommend and depend on jdk packages (java-environment) everywhere.
  Until then README.md and TODO.md must name a JDK package.

### Found in this pass

* The newcomer reviewer's Java item, made exact: the gap is for Java 17 and later. For
  Java 8 the `release` file is in `jre8-openjdk-headless`, a dependency of `jre8-openjdk`,
  and `read_release` finds it from the `jre` home's parent, so the PKGBUILD's
  `jre8-openjdk` line works (from the file list and the code; not run). `jdk17-openjdk`,
  `jdk21-openjdk`, `jdk25-openjdk` and `jdk-openjdk` all ship `release`. `doctor` here
  lists only Java 27 although `/usr/lib/jvm/java-21-openjdk` exists.
* `src/companion/jar.rs`:100 tells a user without `javac` to install `jdk-openjdk`
  (right), while `src/java.rs`:169 and `src/doctor.rs`:466 name jre packages for the
  runtime (see above): the messages disagree about which package family to install.

## What is still open after this pass

* The website: the 15 REQUIRE statements above, the anchor `#the-display`, and
  security.html's UDP sentence.
* The code problems above, in particular the Java discovery, which the README now works
  around by naming `jdk25-openjdk`.
* `docs/notes/voice-chat.md` §7 still tells the integrator to apply
  `docs/notes/broker-test-race.patch`, which `1dbb587` removed (a notes file of that
  package; not changed here).
* Process 742946 (`/usr/bin/bwrap --args 103 -- /bastion/init __sandbox-init 135`, an
  orphan of the voice package's test runs, voice-chat.md §7) was still running at the end
  of this pass; `kill 742946` removes it.
