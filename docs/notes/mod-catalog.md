# Mod catalog: `profile add`, `profile check`

Notes of work package A4, written 2026-11-14 (UTC). Three parts, as asked: user
documentation ready for the README (§1–§2), the threat-model rows this implies with their
residual risks (§6), and what is open or unverified (§7). In between: how verification
works and where it ends (§3), what the registry learns (§4), the alias table with what
exists for Minecraft 26.3 today (§5), and what the real game did with ViaFabric and Simple
Voice Chat in the sandbox (§5.2). §8 is for whoever merges this.

The package was built by one engineer and reviewed, corrected and finished by a second
(the first was cut off). §9 lists what the review found and changed; everything else in
these notes describes the code as it is now. The hand-over file is
`mod-catalog-progress.md`.

Code: `src/profile/modrinth.rs` (registry client), `src/profile/catalog.rs` (the two
commands), changes to `src/profile/author.rs` (`lock_with`) and
`src/profile/capabilities.rs` (`scan_mods`), `tests/catalog.rs`.

---

## 1. For the README — "Curated (modded) profiles"

This replaces the `cp ~/Downloads/sodium-*.jar …` step of the current section. Copying jars
by hand and running `profile lock` keeps working, and is still the way for a mod that is
not on Modrinth.

```sh
bastion-mc key generate ~/secure/bastion.key       # ideally kept off this machine
bastion-mc key trust me ed25519:…                  # the public key it printed

bastion-mc profile new perf --minecraft 26.3 --fabric
bastion-mc profile add perf sodium lithium modmenu     # from Modrinth, dependencies included
bastion-mc profile check perf                          # what Modrinth knows about each jar
$EDITOR ~/.local/share/bastion-mc/instances/perf/profile.json    # review it
bastion-mc profile sign perf --key ~/secure/bastion.key
bastion-mc launch perf
```

`profile add <id> <mod>…` fetches mods from [Modrinth](https://modrinth.com) into the
profile's draft. `<mod>` is a Modrinth slug, a project id, or a built-in alias:
`viafabric`, `simple-voice-chat` (also `voicechat`), `fabric-api`, `sodium`, `lithium`,
`modmenu`. For each one it takes the newest **release** that Modrinth lists for the
profile's Minecraft version and loader, resolves what that build marks as **required**,
and prints, before anything is downloaded: project, slug, project id and licence; version,
channel and publication date; file name, size, SHA-512 and SHA-1; the address it comes from.
Then it downloads into a staging directory, checks size, both hashes and the file's own
date, and only when every file is in hand touches `mods/`. It ends exactly where `profile lock` ends: the
manifest lists the new jars (each with a `source`), the serial is bumped, the old signature
is gone. **Nothing is signed.** The static scan of what the new jars refer to (network,
native code, starting programs, reflection …) is printed, so you see what a new mod can
reach before you sign for it.

Rules it applies:

* **Releases only.** A beta or alpha is taken only with `--prerelease`, and only for a
  project that has no release for that Minecraft version.
* **48-hour quarantine.** A build published less than 48 hours ago is passed over: the
  next older release is taken, and the output names what was skipped. `--allow-fresh`
  takes it anyway. (The point is to leave time for a bad upload to be noticed and
  withdrawn. It is a delay, not a verdict.) Modrinth dates builds, not files, and a file
  can be put into a build after its publication. So the date Modrinth's file server
  states for each downloaded file is read as well, and bytes it has held for less than 48
  hours are refused however old the build is. (The file server's date also moves when
  Modrinth rewrites its storage, which cannot be told from a new upload: for two days
  after that the same refusal meets old files, and says so.)
* **Dependencies.** Required ones are added too, unless a build of that project which
  Modrinth lists for this Minecraft version and loader is already in `mods/`. Optional
  ones are only named.
* **Replacing.** A jar is removed only if Modrinth identifies it, by its hash, as a build
  of the same project, both jars declare the same mod id, *and* it is older than the build
  that takes its place — or, for a required dependency, is not listed for this profile's
  Minecraft version at all. A build that is not older stays: nothing a dependency asks for
  leads to a downgrade. A jar Modrinth does not know is never removed; if it is in the
  way, the command stops and says so.
* **All or nothing.** A download that does not match, bytes that are too new, an unsafe
  file name, an address outside `cdn.modrinth.com`, a malformed answer: the command fails
  and `mods/`, the manifest and the signature are exactly as they were.

`--dry-run` shows the plan and downloads nothing (so it goes by the dates of the builds;
a file's own date is known only once it is downloaded).

`profile check <id>` hashes every jar in `mods/` and asks Modrinth which project and
version each one is: **known** (project, version, when published), **unknown**, **fresh**
(its build is under 48 hours old; this command downloads nothing and does not see a
file's own date). It also points out pre-releases, builds listed for another Minecraft
version, projects that are no longer public, and jars that are not what the manifest lists.
"Known" means Modrinth lists exactly these bytes under that project — not that they are
harmless: whoever controls a project's account publishes a bad build together with its
hash. Only Modrinth is asked (CurseForge needs an API key; GitHub releases are not
consulted), so "unknown" also covers every mod published only elsewhere.

**What Modrinth learns** from these two commands: your address (the proxy's, with
`network.launcher_proxy`), the user agent `bastion-mc/<version>`, the names you asked for,
the profile's Minecraft version and loader, and the SHA-512 of every jar in `mods/`. No
account, cookie or token is involved. A launch contacts neither of the two hosts, and the
relay does not let the game reach them.

### ViaFabric and Simple Voice Chat

```sh
bastion-mc profile add main viafabric simple-voice-chat --prerelease
```

`--prerelease` because on 2026-11-14 neither has a *release* build for Minecraft 26.3
(ViaFabric: alphas; Simple Voice Chat: betas). Both load in the sandbox, with their native
libraries (§5.2). ViaFabric lets the client join servers of other Minecraft versions; its
client-side translation is off until you switch it on (`enable-client-side` in
`config/ViaFabric/viafabric.yml`, or in the game, as that file says). Simple Voice Chat
needs a microphone (`audio = "pulse"`; see the sound section) and UDP to the server's
voice port, which filtered network mode does not offer on its own (see the UDP work
package). Neither was tried against a server here: only loading was.

Suggested additions to the README's other tables: Source map → `src/profile/` gains
"`modrinth.rs` (registry client), `catalog.rs` (`profile add`, `profile check`)"; Tests →
"`BASTION_NET_TESTS=1` also contacts Modrinth".

## 2. Command reference

```
bastion-mc profile add <id> <mod>... [--prerelease] [--allow-fresh] [--dry-run]
bastion-mc profile check <id>
```

| | |
|---|---|
| `<mod>` | alias, Modrinth slug, or project id; 3–64 characters from `[A-Za-z0-9._-]`. Slugs with other characters (Modrinth allows a few) must be named by id. At most 16 per command. |
| `--prerelease` | where a project has no release for this Minecraft version: its newest beta, failing that its newest alpha |
| `--allow-fresh` | also take builds published, and files stored on the file server, less than 48 hours ago |
| `--dry-run` | resolve and print; download nothing, change nothing |
| exit 0 | done, or nothing to do (`profile check` exits 0 whatever it flags: it is a report) |
| exit 2 | usage: unknown project, not a curated profile, bad name |
| exit 5 | refused by a rule: only pre-releases, only fresh builds, a file the file server has held for less than 48 hours, unsafe file name or address, file too large, a clash with a jar in `mods/`, too many dependencies |
| exit 4 | a download did not match its size or hash |
| exit 7 | network or TLS failure, or an HTTP error from the registry |
| exit 1 | an answer that cannot be read, or no build for this Minecraft version and loader |

Not available with `--events` (no `profile` command is).

Bounds: 16 mods asked for, 24 projects looked at, dependencies 4 levels deep, 96 requests
per command (Modrinth allows 300 a minute), 60 s per API request, downloads 2 min + 1 s per
64 KiB (at most an hour) and at most 512 MiB per file, one retry for a transfer that breaks
off, none for wrong bytes.

A Quilt profile takes builds listed for `quilt` or for `fabric` (the newest of either).

Refused by design, with real examples (survey of 2026-11-14): file names outside
`[A-Za-z0-9._+-]` — 2 of the newest builds of the 300 most-downloaded Fabric mods
(`Geophilic v3.7.mod.jar`, `Resourcify (26.3-fabric)-1.8.7.jar`), and Fabric API's builds for
Minecraft 1.14 and 1.15, whose registry file names contain a literal `%2B`. Such a mod is
downloaded by hand and recorded with `profile lock`. (The survey is the first engineer's.
The second looked up the Resourcify example and Fabric API's build for 1.14 again — both
as stated — and did not repeat the count over 300 mods.)

## 3. The verification model, and where it ends

What is checked, in order:

1. **Transport.** `api.modrinth.com` and `cdn.modrinth.com` through `net::http`: TLS 1.3
   only, system CA store, no redirects, bounded bodies, no decompression. The two hosts are
   on the allowlist of a client that only these two commands build (`modrinth::Live::new`);
   `BUILTIN_HOSTS` is unchanged. `network.launcher_proxy` is honoured.
2. **Answers.** Every JSON answer is read strictly and within limits (bytes per endpoint,
   entries per list, base62 identifiers, one RFC 3339 date form). An answer must be about
   what was asked (that project id, those project ids, that version id, those hashes — an
   entry for a hash that was not asked about ends the reading of the answer). Text that is
   shown is stripped of control and bidi characters and cut.
3. **The file's description.** Name must pass `paths::validate_file_name` and end in
   `.jar`; size 1 byte to 512 MiB; SHA-512 128 hex digits; address exactly
   `https://cdn.modrinth.com/data/<the version's project id>/versions/<x>/<the file name>`.
   A selected build whose file fails is an error — no silent fall-back to an older build.
4. **The bytes.** Hashed while written into `<instance>/.catalog-<random>/`; size, SHA-512
   and SHA-1 must be the ones the API gave, else the file is deleted and the command fails.
5. **The file's own date.** The file server's `Last-Modified` for the answer that
   delivered the file: since when it has held its copy. If that is more than an hour after
   the build's publication date *and* less than 48 hours ago, the command fails (unless
   `--allow-fresh`): the bytes cannot be shown to have been there for 48 hours. A late date
   that is not recent is not remarked upon — it proves nothing (see below). Without a
   readable date the build's date is all there is, and the output says so.
6. **The jar.** Must be readable as a jar. A replacement must declare the same mod id as
   the jar it replaces; nothing may bring a mod id a remaining jar already provides.
7. **Install.** First what can be known to fail: `mods/` must be a plain directory of the
   user's, on the file system the files were staged on (a hard link does not cross file
   systems or btrfs subvolumes; such a layout is refused with nothing changed). Then the
   signature is removed, replaced jars are removed, new files are hard-linked into `mods/`
   (a hard link never overwrites), and `author::lock_with` rescans `mods/` — where the
   SHA-256 it records for each new file must equal the one computed during download. An
   I/O error after the first removal leaves `mods/` partly updated; the message says so
   and names `profile lock` as the way to record what is there.

What the file server's date is, observed on 2026-11-14 (fifteen real files of seven
projects, `Last-Modified` against the build's `date_published`): for ten files it was 0.3
to 3 seconds after the build's date — the upload. For the other five (Mod Menu 21.0.0 and
21.0.0-beta.1, Text Placeholder API 3.2.0, Lithium 0.26.1 and 0.26.2: builds published
between 2026-09-09 and 2026-09-28) it was 2026-10-19 between 10:15:25 and 10:16:41 UTC for
all of them: the file server rewrote its storage that day. So the date is "when the file
server last wrote its copy", never earlier than the upload and sometimes much later. That
is why only a *recent* late date has a consequence, why the refusal names both readings
("either the file was put into the build after its publication, or the file server has
rewritten its storage"), and why such a rewrite costs two days in which old files, too,
need `--allow-fresh` or patience. `--allow-fresh` lifts the 48 hours for builds and files
alike, so with it the newest build is taken even if it is fresh: the plan shows which.

What this does **not** establish:

* **That a mod is benign.** The hash comes from the party that serves the file. A
  compromised project account — or Modrinth itself — serves a bad jar with a matching hash,
  and `profile check` will call it "known". The decision to run a jar is the signature.
* **That the build is the author's.** No author signature exists on Modrinth to verify.
* **Consensus.** Only Modrinth is consulted. CurseForge needs an API key; GitHub releases
  are not consulted. A build that differs between registries is not noticed.
* **Version ranges.** Whether a kept dependency satisfies its dependent is checked by the
  mod loader at start, not here ("suitable" means: listed for this Minecraft version and
  loader, and not older than an exact build the dependent names).
* **The quarantine as detection.** A bad build that nobody noticed for 48 hours is taken.
  Publication dates are the registry's, file dates its file server's. Age is measured
  against the earlier of the local clock and the `Date` header of the registry's answers,
  so a wrong local clock cannot loosen the rule — a registry that lies about dates can.
* **That the bytes were public for 48 hours.** The two dates say when the registry
  received a build and when its file server stored a file, not since when either could be
  seen. Modrinth's API lets a member of a project add a file to a published build
  (`POST /version/{id}/file`) and choose the build's primary file (`primary_file` in
  `PATCH /version/{id}`): a file can wait in a build as an additional file for two days
  and then be made the one that is downloaded. And whether a build that was prepared as a
  draft, or scheduled, keeps the date of its creation is not stated in the API
  documentation and was not tested (it needs a project of one's own). What the file date
  does close is the immediate case: a file uploaded today into a build of last month.
* **That a late file date means a late file.** See above: the file server re-dates its
  copies for reasons of its own. Nothing is concluded from a date that is not recent.
* **File dates outside `profile add`.** `profile check` and `--dry-run` download nothing
  and go by the build's date alone; both say so in their output.
* **When a build's metadata was written.** The list of what a build requires, its channel
  and the game versions it is listed for can be edited after publication
  (`PATCH /version/{id}`), and no date in the API says when. A required dependency that
  was added yesterday to a month-old build is followed like any other. What stands
  against that: the dependency's own build and file are held to every rule above, each
  project that would be added is printed with its title, slug, id, licence and file
  before anything is downloaded, and nothing runs before the manifest is signed.
* **Pinning.** Modrinth's hosts are not pinned (system CA store, like Fabric's and Quilt's
  hosts); `[tls.pins]` can pin them.
* **The static scan.** As before: a signal for review; reflection, run-time class
  definition and native code hide whatever they like.

## 4. What the registry learns about the user

| Datum | When | Why |
|---|---|---|
| Source address (or the SOCKS5 proxy's, with `network.launcher_proxy`; the name is then resolved by the proxy) | every request | — |
| `User-Agent: bastion-mc/<version>` | every request | Modrinth's API rules require a user agent that identifies the program. Contains nothing about the user or machine |
| Slugs / project ids asked for, and ids of their required and optional dependencies | `add` | lookups |
| Minecraft version and loader of the profile | `add` | filter of the version list |
| SHA-512 of **every jar in `mods/`** | `add` (if `mods/` is not empty; not before every name on the command line has passed the local check) and `check` | to recognise what is present; includes jars that are private builds — only their hash |
| Which files are downloaded | `add` | `cdn.modrinth.com` |

Not sent: account, token, cookie, hardware or OS details, instance names, file names of
local jars. Nothing from the registry is cached between commands; what is kept is the
`source` address of each added file, in the manifest. Observed on
the wire by `tests/catalog.rs::a_mod_is_added_and_checked_over_tls_through_the_launchers_own_client`.

## 5. Aliases and what exists for Minecraft 26.3

An alias names its project by **id**: Modrinth's API documentation says slugs "can change
at any moment", so a slug that was given up can be registered by somebody else. Looked up
live on 2026-11-14 (`BASTION_NET_TESTS=1 cargo test --test catalog live_ -- --nocapture`),
by the first engineer and again at 01:50 UTC by the second: the same answers.

### 5.1 Table

| Alias | Project id | Slug, title | Licence (as Modrinth states it) | Fabric builds for 26.3 | What `profile add` does today |
|---|---|---|---|---|---|
| `viafabric` | `YlKdE5VK` | viafabric, ViaFabric | GPL-3.0-only | **no release**; alpha 0.4.22+188-26.x (2026-10-22) | refuses; with `--prerelease` takes the alpha. Requires Fabric API; embeds ViaVersion (GPL-3.0-or-later) |
| `simple-voice-chat`, `voicechat` | `9eGKb6K1` | simple-voice-chat, Simple Voice Chat | LicenseRef-All-Rights-Reserved ("All Rights Reserved" in the jar) | **no release**; beta fabric-2.6.27+26.3 (2026-10-28) | refuses; with `--prerelease` takes the beta. No required dependency (it carries the Fabric API modules it needs) |
| `fabric-api` | `P7dR8mSH` | fabric-api, Fabric API | Apache-2.0 | release 0.166.2+26.3 (2026-11-11 13:38 UTC) | takes it (out of quarantine since 2026-11-13 13:38 UTC) |
| `sodium` | `AANobbMI` | sodium, Sodium | LicenseRef-Polyform-Shield-1.0.0 | release mc26.3-0.9.2-fabric (2026-09-15); newer betas up to 0.9.3-beta.5 (2026-11-12) | takes release 0.9.2 |
| `lithium` | `gvQqBUqZ` | lithium, Lithium | LGPL-3.0-only | release mc26.3-0.26.3-fabric (2026-10-28) | takes it |
| `modmenu` | `mOgUt4GM` | modmenu, Mod Menu | MIT | releases 21.0.1 (2026-11-12 11:08 UTC), 21.0.0 (2026-09-23) | takes 21.0.0 until 2026-11-14 11:08 UTC (21.0.1 is in quarantine), plus required Text Placeholder API 3.2.0+26.3 (`placeholder-api`, `eXts2L7r`, LGPL-3.0-only) and Fabric API |

All six projects have a Fabric build for 26.3; two of them have no *release* build.
The last column was confirmed with `profile add … --dry-run` for all six at 02:15 UTC;
ViaVersion's licence was read from its project (`P1OZGk5p`), and "All Rights Reserved" from
the `fabric.mod.json` inside the Simple Voice Chat jar (ViaFabric's says `GPL-3.0`).

**Licences.** The launcher downloads these mods at the user's request from the place
their authors publish them, and bundles, mirrors and redistributes none: no mod jar is in
the source tree or the packages, tests use jars generated in the test, and what the runs
below downloaded sits under `target/` (not tracked). The licence Modrinth states is printed
with every plan and every check, so the user sees it before signing. Simple Voice Chat is
all rights reserved and Sodium is under the Polyform Shield licence — both are a reason
never to ship them, and no obstacle to a user downloading them.

### 5.2 The real game (Minecraft 26.3, demo mode, sandbox, 2026-11-14)

Three launches on the private display, all through the system's gamescope 3.17 (nested
compositor), filtered network, `audio = "none"`, stopped with SIGINT 25 s after the title
screen's textures were loaded (exit 130, "game stopped on request"). No input was sent to
the game. Instances were copies of `fab` in the test data root. Launches 1 and 2 were run
by the first engineer (01:27 and 01:31 UTC) with the tree as it was then; launch 3
repeats launch 1 after the review. The launch logs are kept under
`target/e2e/state/bastion-mc/logs/{fabcat,fabrep,fabv3}/` of the work clone (not tracked);
the second engineer read all three against what is written here.

**Launch 1 — `fabcat`: Fabric API 0.166.2 (was there) + ViaFabric 0.4.22+188-26.x +
Simple Voice Chat 2.6.27**, added with
`profile add fabcat viafabric simple-voice-chat --prerelease`, then `profile lock` (no
change: serial stayed 3), `profile sign`, `launch fabcat --demo`.

* `mods: 3 jar(s) match the signed manifest`; Fabric Loader: `Loading 54 mods`, among
  them `viafabric 0.4.22+188-26.x` (with `viafabric-mc26-2` and `viaversion
  5.12.1-SNAPSHOT`) and `voicechat 2.6.27+26.3` (with `voicechat_api 2.6.27`).
* ViaFabric: `ViaVersion detected lowest supported version by the proxy: 1.8.x (47)`,
  `Highest supported version by the proxy: 26.3 (777)`, mappings loaded.
* Simple Voice Chat: `Successfully registered ViaVersion mappings`, then
  `Successfully initialized Opus`, `… RNNoise`, `… Speex`, `… LAME` — all four bundled
  **native libraries loaded** inside the sandbox.
* **Refused:** ViaVersion's update check. `egress: DENY name lookup
  "update.viaversion.com" (not allowlisted)`; the mod logs `Could not check for updates,
  check your connection.` and carries on. Summary: `5 connection(s) relayed, 0 denied,
  2 name lookup(s) refused`; `5 request(s) forwarded to Mojang, 0 of them with your
  credential`. Simple Voice Chat attempted nothing that was refused in this run.
* Both wrote their settings into the instance's `config/` (`ViaFabric/viaversion.yml`,
  `ViaFabric/viafabric.yml`, `voicechat/*.properties`), which is where they belong.
* Errors in the log that are not theirs (the unmodded `fab` run has them too, or they come
  from the test configuration): narrator (`flite` missing), `Failed to open OpenAL device`
  (`audio = "none"`), Realms and `/player/attributes` (no account).

The static scan printed by `profile add` matches what was then observed: ViaFabric
"network" names `com.viaversion.viaversion.update.UpdateUtil`; Simple Voice Chat lists
"native code" and the four `natives/linux-x64/lib*.so`.

**Launch 2 — `fabrep`: Fabric API replaced (0.165.2 → 0.166.2) + Mod Menu 21.0.0 +
Text Placeholder API + Sodium 0.9.2 + Lithium 0.26.3.** `Loading 53 mods`; hardware
OpenGL (radeonsi), window created, resources loaded.

* **Refused:** Sodium runs `lspci` to name the graphics card —
  `Cannot run program "lspci": Exec failed, error: 13 (Permission denied)` (Landlock);
  it continues with `name=<unknown>`. The scan had listed "runs programs —
  …GraphicsAdapterProbe".
* **Refused:** Mod Menu's update checker — `egress: DENY name lookup "api.modrinth.com"`
  and `"meta.fabricmc.net"`; it logs `Error checking for updates` and `Failed Fabric Loader
  update check!`. (So the registry is reachable by the launcher's authoring commands and
  not by the game.) Summary: `5 connection(s) relayed, 0 denied, 4 name lookup(s) refused`.
* Sodium reports the workaround `NO_ERROR_CONTEXT_UNSUPPORTED`; whether that comes from
  the nested compositor or from the driver was not looked into.

**Launch 3 — `fabv3` (02:06 UTC, the binary of commit `7a1f600`: every change of §9 but
the last rewording of the file-date rule and the two "smaller changes", none of which
touches what is installed or a launch): the same three jars as launch 1**, added to a
fresh copy of `fab` with
`profile add fabv3 viafabric simple-voice-chat --prerelease` (no remark about file dates:
the file server dates both files within seconds of their builds), then `profile check`
(3 known, 2 flagged as pre-releases), `profile lock` (serial stayed 3), `profile sign`,
`profile verify`, `launch fabv3 --demo`. The log says what launch 1's says, line for line
where it matters: `mods: 3 jar(s) match the signed manifest`; `Loading 54 mods` with
`viafabric 0.4.22+188-26.x` (`viafabric-mc26-2`, `viaversion 5.12.1-SNAPSHOT`) and
`voicechat 2.6.27+26.3` (`voicechat_api 2.6.27`); `ViaVersion detected lowest supported
version by the proxy: 1.8.x (47)` / `Highest supported version by the proxy: 26.3 (777)`;
`Successfully initialized Opus`, `RNNoise`, `Speex`, `LAME`; one refusal,
`egress: DENY  name lookup "update.viaversion.com" (not allowlisted)`, answered by the
mod with `Could not check for updates, check your connection.`; `5 connection(s) relayed,
0 denied, 2 name lookup(s) refused` (the relay prints one line per refused name and
counts every lookup); `5 request(s) forwarded to Mojang, 0 of them with your credential`;
`game stopped on request`. The title screen's textures were loaded 16 s after the start.

**Nothing needed a launcher change to load.** For the two mods to do their job:

* **Simple Voice Chat needs UDP** to a voice port of the game server. Filtered mode has no
  UDP except DNS to the launcher (THREAT_MODEL §5.7, E15). That is the UDP work package.
  What this run can contribute are the comments in the settings the mod generated
  (`config/voicechat/`):
  `port=24454` — "Audio packets are always transmitted via the UDP protocol on the port
  number specified here, independently of other networking used for the game server";
  `voice_host=` — "The hostname that clients should use to connect to the voice chat. This
  may also include a port" (so a server operator can point clients at **another host and
  port** than the game server's: a destination chosen by the server, to be treated as
  untrusted by whatever relays it);
  `run_local_server=true` — "If the voice chat should work in singleplayer or in worlds
  shared over LAN" (a voice server of the mod's own inside the sandbox; by E15 a `bind` to
  a chosen UDP port is `EACCES` there).
  **Neither a connection attempt nor that bind was observed**: no world was entered and no
  server joined.
* **Simple Voice Chat needs the microphone:** `audio = "pulse"`, with the residual of D3
  (recording is possible by design).
* **ViaFabric** asked for nothing else in this run. By its design it translates on the
  game connection itself, which the relay already carries for an allowlisted server — not
  exercised (no account, no server). Its update check stays refused; the generated
  `viaversion.yml` has the comment "Should ViaVersion check for updates?" but no key under
  it in this build, so there was nothing to switch off.

## 6. Threat model: rows this adds or changes

Proposed for §4.4 (network and supply chain). "Verified by" names tests in this branch.

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| C8 | The jar that lands in `mods/` is not the one the registry lists: a broken or tampered transfer, a cache or mirror serving something else, a redirect | `profile add` takes size, SHA-512 and SHA-1 from the API and the bytes from `cdn.modrinth.com` only (TLS 1.3, no redirects); hashes while writing into a staging directory; installs only on a full match, after every file of the command verified | `profile::modrinth::tests::a_download_is_kept_only_if_size_and_hashes_match`, `catalog::a_file_that_does_not_match_never_reaches_mods_over_tls`, `profile::catalog::tests::every_refusal_leaves_mods_the_manifest_and_the_signature_as_they_were` (28 cases) | **The hash and the file come from the same party.** A compromised project account, or the registry, serves a bad jar with a matching hash. No author signature exists to check. Modrinth's hosts are unpinned |
| C9 | A compromised upstream account publishes a malicious update, and the user picks it up at once | Releases only unless `--prerelease`; builds younger than 48 hours passed over unless `--allow-fresh`; because a file can be put into a build that is already published, the file server's own date for each downloaded file (`Last-Modified`) is read too, and bytes it has held for less than 48 hours are refused whatever the age of their build; ages measured against the earlier of the local clock and the registry's `Date`; the capability scan of what was added (and what an update gained) printed before signing; nothing is ever signed automatically; `profile check` flags fresh builds | `…::a_build_younger_than_48_hours_is_passed_over_unless_allowed`, `…::bytes_the_file_server_has_held_for_less_than_48_hours_are_refused_whatever_the_age_of_the_build`, `catalog::a_file_the_file_server_has_held_for_less_than_48_hours_is_refused_over_tls`, `…::the_age_of_a_build_is_measured_against_the_earlier_of_two_clocks`, `…::pre_releases_are_refused_unless_asked_for`, `…::an_older_build_of_the_same_project_is_replaced_and_no_other_jar_is_touched`; live: Modrinth's file server dates its files (fifteen compared, §3), and `profile add` of Mod Menu — two files the file server re-dated on 2026-10-19 — goes through without a remark | A delay, not detection: a bad build nobody noticed for 48 hours is taken. The dates say when the registry received a build and stored a file, not since when they were public: a file can wait in a build as an additional file and be made the primary one later; whether a build prepared as a draft or scheduled keeps its creation date is undocumented and untested. `profile check` and `--dry-run` see the build's date only. A file server that states no date leaves the build's date as the only one (the output says so). The file server's date moves when it rewrites its storage: for two days after that, old files are refused as well (fail closed; the message names both readings). A build's list of required dependencies can be edited after publication and nothing dates the edit: a dependency added to an old build is followed (and shown in the plan). Dates are the registry's. Both flags switch the rule off |
| C10 | Hostile or malformed registry answers: path traversal in a file name, a download address on another host, oversized bodies and lists, terminal escape sequences in titles, a dependency graph built to make the launcher crawl | Strict, bounded parsing; file name through `validate_file_name`; address fixed to `https://cdn.modrinth.com/data/<project>/versions/…/<file>`; answers must match the question (project, list of projects, version, hashes — a hash lookup is held to the hashes asked while it is read); display text cleaned; 16/24/4/96 bounds on mods, projects, depth, requests; a deadline per request | `profile::modrinth::tests::*` (15), `…::dependency_chains_and_fans_are_bounded`, `robustness::mod_registry_answers` and, for the jar scan that now reads what the registry delivered, `robustness::mod_jars_and_class_files` (300 000 mutations each, two seeds, after the last parser change), `catalog::an_exchange_that_gets_no_answer_ends_at_its_deadline` | Coverage-guided fuzzing of the two harnesses has not run (no nightly toolchain); the targets compile. A jar close to the 512 MiB limit that is all directory entries costs the scan seconds and memory in proportion before it is refused (600 000 entries: 1.7 s, 124 MiB) |
| C11 | A project slug is given up and re-registered by somebody else (or a look-alike title) | Built-in aliases name projects by permanent id and remark on a changed slug; every plan shows title, slug, id and licence; an answer for another project than the one asked for is refused | `…::an_alias_names_its_project_by_id` | For any other name the user types, whoever holds that slug today is what is fetched. Titles are the registry's text |
| C12 | The registry's answer to a hash lookup is used to make the launcher delete the user's jars | A jar is replaced only if the registry calls it a build of the project being added, both jars declare the same mod id, **and** it is older than its replacement (for a required dependency also: not listed for this profile's Minecraft version); a build that is not older stays, so no answer and no dependency leads to a downgrade; unknown jars are never removed; new files are hard-linked, never written over existing ones; name or mod-id clashes stop the command | `…::a_jar_is_not_removed_on_the_registrys_word_alone`, `…::a_jar_the_registry_does_not_know_is_never_removed_or_written_over`, `…::a_build_that_is_not_older_is_never_replaced_by_an_older_one`, `…::a_dependency_built_for_another_minecraft_version_is_replaced_and_the_output_says_why` | A replaced jar is deleted, not kept aside. A registry that lies about which hash is which build can still have an older build of the same mod (same mod id) replaced by a newer one |
| N4 | The new hosts widen what a launch, or the game, can reach | The two hosts are on the allowlist of a client built by `profile add`/`profile check` only; `BUILTIN_HOSTS` and the game's relay do not know them | `profile::modrinth::tests::the_registry_hosts_are_reachable_only_through_the_client_built_for_it`, `…::the_live_client_refuses_everything_but_its_two_hosts_before_connecting`; real launch: `egress: DENY name lookup "api.modrinth.com"` when Mod Menu asked | — |
| N5 | The registry profiles the user | No account or token; a fixed user agent; `network.launcher_proxy` honoured (`catalog::the_registry_is_reached_through_the_configured_socks5_proxy`) | as named | It learns address, the mods asked for, the Minecraft version and the SHA-512 of every jar in `mods/` (§4) |

Changes to existing text:

* **C2 / C6**: unchanged — a jar added this way is still nothing until the manifest is
  signed; `mods/` must still equal the signed list exactly.
* **§5 "Not defended"**, new item: *A mod that is malicious as published.* `profile add`
  and `profile check` say what the registry lists, not what a jar does.
* **§8**, "cross-registry hash consensus and a bytecode capability diff for mod updates":
  the capability diff exists (`profile lock`, `profile add`); a registry cross-check exists
  for Modrinth alone (`profile check`); consensus across registries does not.
* **§7 experiments**, two candidates: the launches of §5.2 (mods with native libraries
  load from the sealed mount; update checks of ViaVersion and Mod Menu and Sodium's `lspci`
  are refused and the mods carry on).
* **N1**: the registry client is the same `HttpClient` (same TLS policy, same tests); its
  two hosts are in no fixed list — only in the client those two commands build.
* **The capability report** (`profile lock`, `profile audit`, `profile add`) now shows
  every jar: two jars that go by one identity (the same mod id, or a jar without one whose
  file name starts like another's id) used to leave one entry
  (`profile::capabilities::tests::two_jars_that_go_by_one_identity_are_both_reported`).

## 7. Open, and not verified

Not verified here, and why:

* **ViaFabric translating a connection, Simple Voice Chat carrying voice.** Needs a
  signed-in account, a server, UDP and a microphone. Only loading and initialisation were
  observed (§5.2). No world was entered: no input was sent to the game.
* **A Quilt profile.** The rule (quilt or fabric builds) is unit-tested; no Quilt profile
  was created or launched.
* **The cargo-fuzz targets** `registry` and `jar`: compile-checked (`cargo check` in
  `fuzz/`), not run.
* **The jar scan against hostile archives beyond mutation.** Nine archives were made by
  hand and given to `profile audit` under a 6 GiB address-space limit: entry counts that
  lie (65 535; 2^40 in a zip64 record), 600 000 real entries, a 64 MiB class and a 300 MiB
  nested jar inside 364 KiB, jars nested six deep, an entry that lies about its size,
  random bytes, an empty and a truncated file. Each ended in under two seconds, the worst
  with 524 MiB of memory (the nested jar, read up to its 256 MiB limit), with an error or
  a report that names what could not be read. Not tried: an archive near the 512 MiB limit.
* **Rate limiting (HTTP 429), HTTP 410:** only the wording of the error is tested.
* **Download deadlines** for large files: the mechanism is tested with a server that never
  answers; the size-dependent value was not exercised with a slow server.
* **Modrinth through Tor:** the SOCKS5 path is tested against a stand-in proxy, not
  against Tor and the real registry.
* **Old game versions:** live lookups were made for Minecraft 26.3 only.
* **The file-date rule on a real fresh file.** No file whose file-server date was under
  48 hours old and later than its build was seen at the registry (none was looked for
  beyond the fifteen of §3), so the refusal itself is tested against scripted answers and
  a stand-in file server over TLS only. Live it is established that Modrinth's file server
  sends `Last-Modified`, what it is (§3), and that files it re-dated weeks ago are taken
  without a remark.
* **How often the file server rewrites its storage.** One such day was found
  (2026-10-19). Each one makes `profile add` refuse old files for two days.
* **What `date_published` is for a build that was a draft, or scheduled,** before it was
  listed: the API documentation does not say, and finding out needs a project of one's
  own. If it is the date of creation, a build can be prepared out of sight and be 48 hours
  "old" the moment it appears; the file date does not help there (the file is as old).
* **A file server answer without `Last-Modified`** was not seen live; the command then
  goes on with a remark (tested against the stand-in).

Open, by decision or for lack of time:

* `profile check` exits 0 whatever it flags. A `--strict` could make it usable as a gate.
* `profile check` does not ask the file server for the date of each file (one request per
  jar; it would also tell the file server which jars are in the profile, which the API
  already learns by hash). So "fresh" there goes by the build's date, and its output says so.
* No `profile remove`, no "update everything", no `<mod>@<version>` to pin a build; a
  downgrade is a manual copy plus `profile lock`.
* A replaced jar is deleted. Keeping it aside until the next signature would make a wrong
  replacement recoverable.
* Not offered through `--events`, so the graphical front end has no button for it.
* The user agent has no contact address (Modrinth recommends one); the project has no
  public home yet (same blocker as the PKGBUILD's `url`).
* Registry file names with spaces, brackets or `%` are refused rather than renamed.
* Whether a kept dependency satisfies a version *range* is left to the loader.
* The alias table is compiled in; `live_every_alias_names_the_project_it_was_recorded_for`
  fails when a project is renamed or removed, which is the cue to update it.

**About "the Fabric mod".** This package ships no Fabric mod of its own and the tree
contains none. The *Fabric companion mod* of TODO.md is still under "Decided against /
deferred" (TODO.md; THREAT_MODEL §8 "In-game companion (Fabric) for approvals"): it was not
built, by this package or before it. Third-party Fabric mods are not in the repository
either: `profile add` downloads them into `instances/<id>/mods/`.

## 8. For the integrator

* **New files:** `src/profile/modrinth.rs`, `src/profile/catalog.rs`, `tests/catalog.rs`,
  `fuzz/fuzz_targets/registry.rs`, `fuzz/fuzz_targets/jar.rs`, this file and
  `mod-catalog-progress.md`.
* **Shared files touched, each in one place:** `src/cli.rs` (nine lines of help under
  "profile", one arm at the top of `profile_command`), `src/fuzzing.rs` (two entries at
  the end of `ALL`: `registry`, `jar`), `tests/robustness.rs` (three helper functions
  before `seeds`, two `seeds` arms before the fallback, `"registry", "jar"` appended to
  the `tested` array, four tests before
  `the_starting_points_are_accepted_by_their_parsers`), `fuzz/Cargo.toml` (two `[[bin]]`
  at the end), `fuzz/README.md` (four lines). `src/profile/mod.rs`: two module lines.
* **`src/profile/author.rs`:** `lock` is now `lock_with(paths, id, &BTreeMap::new())`;
  `read_draft` became `pub(super)`. Behaviour change worth knowing: a `source` recorded in
  the manifest now survives `profile lock` as long as the file is byte for byte the same
  (before, any recorded source made every lock bump the serial and drop it).
* **`src/profile/capabilities.rs`:** `scan_mods` (five lines in its loop, one test) and a
  harness function `fuzz` before the tests. Behaviour change worth knowing: when two jars in `mods/` go by one identity, `profile
  lock`, `profile audit` and `capabilities.json` now hold both — the later one (by file
  name) under `<identity> (<file name>)` — where they held one before.
* **Not touched:** `Cargo.toml`, `Cargo.lock`, `src/config.rs`, `examples/config.toml`,
  `src/launch.rs`, `build.rs`, the proto crate, the GUI. No new dependency, no new
  configuration key.
* **Conflicts to expect:** the `tested` array line and the tail of `ALL` if others added
  harnesses; the help text if others added `profile` subcommands.
* **To do when merging docs:** §1 into the README (and re-count "Status"); §6 into
  THREAT_MODEL; TODO.md: "cross-registry hash consensus" is now partly built, and a line
  for the open items of §7.
* **For the UDP package:** the Simple Voice Chat facts of §5.2, all from the comments in
  the settings files the mod generated, none from an observed connection: the voice port
  is the server's setting (default 24454/udp, independent of the game port); `voice_host`
  lets a server name another host and port for clients to use; a local voice server in
  single-player and LAN worlds (`run_local_server=true`).
* **A test that is not this package's failed once**, in one of seven runs of the whole
  suite (alone it passed six times out of six):
  `sandbox::resource_limits_kill_a_memory_hog_inside_the_scope` expects the sandbox to end
  with exit code 137 and got no exit code — the process was ended by a signal. It is a race
  that the baseline has too, between bubblewrap and systemd. The kernel kills the payload
  inside the scope (the journal shows exactly that for the failed run: `Memory cgroup out of
  memory: Killed process … (python3.14)`), and then two things happen independently:
  bubblewrap exits with 137, and the user manager, whose `DefaultOOMPolicy` is `stop`
  (systemd 262), stops the scope — SIGTERM to whatever is left. On a busy machine (the
  failed scope lived 238 ms where the others lived 48) the SIGTERM reaches bubblewrap first.
  Reproduced without the launcher: `systemd-run --user --scope -p MemoryMax=128M -p
  MemorySwapMax=0 -- bash -c '<allocate 600 MiB in python>; sleep 0.05; exit 137'` ends with
  status 143 (SIGTERM); without the `sleep`, 137; with `-p OOMPolicy=continue`, 137 whatever
  the delay; with `-p OOMPolicy=kill`, SIGKILL for the whole scope. Not changed here
  (`tests/sandbox.rs` and `src/sandbox/cgroup.rs` are another package's). It is more than a
  flaky test: for the same reason a real launch whose game runs out of memory can end in
  `game exited with status 1` (what `launch.rs` makes of a process without an exit code)
  instead of 137 — read in the code, not observed. Adding `OOMPolicy=continue` to the
  scope's properties in `cgroup.rs` should make both deterministic (it does for the
  `systemd-run` reproduction; not tried in the launcher).
* **Things to say in the release notes**, because they change what a user sees: `profile
  add` can refuse a file "the file server has held its copy of … only since …" although
  its build is old — also, for two days, after Modrinth rewrites its storage (§3) — and it
  never replaces a build that is not older (§1 "Replacing").

## 9. The review after the take-over: what was found and changed

The first engineer's work was complete and green when it was taken over; nothing was
rewritten. A review of it (the code, Modrinth's `openapi.yaml`, the real registry, the
real game) led to four changes, each with tests, each a commit of its own:

| What was wrong | What it is now | Tests |
|---|---|---|
| A jar that `profile add` had just added could be missing from the capability report it prints before signing: reports were filed by identity, one per identity, and a hand-copied jar could go by the same one | `capabilities::scan_mods` keeps every jar | `capabilities::tests::two_jars_that_go_by_one_identity_are_both_reported`, `catalog::tests::what_was_added_is_scanned_and_shown_even_next_to_a_jar_that_goes_by_the_same_name` |
| For a required dependency a build in `mods/` was replaced whenever it was "not suitable" — also by an older one (a dependent naming an exact build that could not be taken caused a downgrade), and a newer build for another Minecraft version was removed under the words "an older build" | Only an older build is replaced; a build that is listed for this profile and not older always stays; the one remaining exception is named for what it is in the plan | `…::a_build_that_is_not_older_is_never_replaced_by_an_older_one`, `…::a_dependency_built_for_another_minecraft_version_is_replaced_and_the_output_says_why` |
| Two answers were not held to the question: a list of projects could contain projects that were not asked for; a hash lookup's entries were counted only after all of them had been read | Both refused; a hash lookup is checked entry by entry while it is read | `modrinth::tests::a_hash_lookup_is_accepted_only_for_what_was_asked`, `…::the_routes_ask_what_the_documentation_says_and_bound_what_they_take` |
| The 48-hour rule went by the date of the build, and Modrinth lets a project member put a new file into a build that is already published: a taken-over account got past the quarantine at once | The file server's date for the file itself is read with the download; bytes it has held for less than 48 hours are refused whatever the age of the build (§3 step 5, with what that date is and what the check costs). A first version of this also remarked on every file dated later than its build and called it "put into the build after its publication"; comparing more real files showed that to be wrong (the file server re-dates its copies), and it was taken out | `…::bytes_the_file_server_has_held_for_less_than_48_hours_are_refused_whatever_the_age_of_the_build`, `catalog::a_file_the_file_server_has_held_for_less_than_48_hours_is_refused_over_tls`, the refusal matrix; a real `profile add` of Mod Menu |

Smaller changes: a name on the command line that cannot be a slug or id is refused before
the first request (which carries the hash of every jar in `mods/`); the help text says
what the three flags do; `profile check` and `--dry-run` say that they go by the date of
the build; the capability scan of jars — which `profile add` now runs on what a registry
has just delivered — got a robustness harness (`jar`) and was probed with hostile
archives (§7): nothing had to be changed in it for that; a `mods/` directory on another
file system than the instance is refused before the signature and the replaced jars are
removed (it used to fail at the first hard link, after both were gone).

Checked and found right (so that nobody checks it a third time): the five API routes and
their parameters against the documentation; that every piece of registry text is cleaned
or validated before it is printed; that no failure before the install step leaves anything
behind (the 28-case matrix, plus the TLS tests); the alias ids and slugs against the live
registry; the statements of §5.2 about launches 1 and 2 against their logs.
