# Mods: signed profiles, Modrinth, ViaFabric and voice chat

In this launcher a mod runs only if it is listed in a manifest that you signed, with its
exact size and SHA-256 (the launcher's own companion mod is the one exception, below);
`mods/` must hold those jars and nothing else. Everything below is
an aid for *deciding* what to sign: fetching jars with verified hashes, asking a registry
what it knows about them, a static scan of what their class files refer to. None of it
decides for you, and none of it makes a mod safe to run — the sandbox is what contains a
mod, whatever it does ([`THREAT_MODEL.md`](THREAT_MODEL.md), §4.2). bastion-mc is
pre-release.

## Signed profiles in practice

Once, a signing key and its trust:

```sh
mkdir -m 700 -p ~/secure                           # key generate does not create directories
bastion-mc key generate ~/secure/bastion.key       # file mode 0600; ideally kept off this machine
bastion-mc key trust me ed25519:…                  # the public key it printed
bastion-mc key list                                # trusted keys, and which store counts
```

If the root-owned directory `/etc/bastion-mc/trusted-keys.d` holds at least one key, only
those keys count and per-user keys are ignored; otherwise the per-user store does, and any
program running as you can add a key to it (row C4). The signature is worth what the place
where the key is kept is worth.

Then, per profile:

```sh
bastion-mc profile new perf --minecraft 26.3 --fabric      # a draft with the loader pinned
bastion-mc profile add perf sodium lithium modmenu         # or copy jars into its mods/ by hand
bastion-mc profile lock perf                               # record what is in mods/ now
bastion-mc profile audit perf                              # what each jar's classes refer to
$EDITOR ~/.local/share/bastion-mc/instances/perf/profile.json   # review it
bastion-mc profile sign perf --key ~/secure/bastion.key
bastion-mc profile verify perf                             # what a launch will check, now
bastion-mc launch perf                                     # add --demo until your client ID is approved
```

* **`profile new`** writes a draft manifest for a Fabric or Quilt profile and pins the loader's
  libraries by SHA-256. It tells you which hashes it could cross-check and which are trust
  on first use; your signature freezes them (row C6).
* **`profile lock`** records every jar in `mods/` — file name, size, SHA-256 — bumps the
  manifest's serial when that list changed, and removes the old signature. It also compares
  a static scan of the jars with the previous lock and says what an updated mod newly
  refers to.
* **`profile audit`** lists, per jar, the sensitive Java APIs its classes refer to by name
  (network, native code, starting programs, reflection and others) and the native libraries
  it bundles. It is a signal for review: reflection, classes defined at run time and native
  code can hide anything else.
* **`profile sign`** signs the manifest (Ed25519, domain-separated).
* **`profile verify`** checks the signature and `mods/` exactly as a launch would.
* **At launch** the signature, the serial (no rollback to an older signed manifest), every
  loader library and every jar are verified; an extra, missing or modified file stops the
  launch before the game starts (exit status 5 for a policy refusal, 4 for a signature or
  integrity failure; §7 E8 of the threat model lists nine such cases, all refused).

The one jar in a Fabric session that is not in your manifest is the launcher's own
companion mod, added read-only from outside `mods/` and named with its SHA-256 in every
launch log ([`COMPANION.md`](COMPANION.md)); `companion.enabled = false` keeps it out.

## Fetching mods from Modrinth: `profile add`

```text
bastion-mc profile add <id> <mod>... [--prerelease] [--allow-fresh] [--dry-run]
```

`<mod>` is a Modrinth slug, a project id, or one of the built-in aliases (below); 3 to 64
characters of `A–Z a–z 0–9 . _ -`, at most 16 per command. A slug with other characters
must be given by its project id.

For each mod, `profile add` takes the newest **release** that Modrinth lists for the
profile's Minecraft version and loader, resolves what that build marks as **required**, and
prints the plan before anything is downloaded: project, slug, project id and licence;
version, channel and publication date; file name, size, SHA-512 and, where the registry
lists one, SHA-1; the address it comes from. Then it downloads into a staging directory
next to `mods/`, checks the size, the SHA-512, the SHA-1 where there is one, and the file's
own date, and only when every file is in hand touches `mods/`.
It ends exactly where `profile lock` ends — the manifest lists the new jars, each with the
address it came from, the serial is bumped, the old signature is gone — and it prints the
static scan of what was added. **Nothing is signed.**

The rules it applies:

* **Releases only.** A beta or alpha is taken only with `--prerelease`, and only for a
  project that has no release for that Minecraft version (its newest beta, failing that its
  newest alpha).
* **48 hours.** A build published less than 48 hours ago is passed over: the next older
  release is taken, and the output names what was skipped. Modrinth dates builds, not
  files, and a file can be put into a build after its publication; so the date Modrinth's
  file server states for each downloaded file (`Last-Modified`) is read too, and bytes it
  has held for less than 48 hours are refused however old their build is. `--allow-fresh`
  lifts both. Age is measured against the earlier of this machine's clock and the `Date` of
  the registry's own answers, so a wrong local clock can only make the rule stricter. The
  point is to leave time for a bad upload to be noticed and withdrawn: it is a delay, not a
  verdict. (The file server's date also moves when Modrinth rewrites its storage — it did
  on 2026-10-19 for files of weeks before — and a rewrite cannot be told from a new upload:
  for two days after one, old files are refused as well, and the message gives both
  readings.)
* **Dependencies.** Required ones are added too — unless a build of that project that
  Modrinth lists for this Minecraft version and loader is already in `mods/` — up to four
  levels deep. Optional ones are only named.
* **Replacing.** A jar is removed only if Modrinth identifies it, by its hash, as a build of
  the same project, both jars declare the same mod id, *and* it is older than the build that
  takes its place — or, for a required dependency, not listed for this Minecraft version at
  all. A build that is not older stays: nothing leads to a downgrade. A jar Modrinth does
  not know is never removed; if it is in the way, the command stops and says so. A replaced
  jar is deleted, not kept aside.
* **All or nothing.** A download that does not match, bytes that are too new, an unsafe file
  name, an address outside `cdn.modrinth.com`, a malformed answer: the command fails, and
  `mods/`, the manifest and the signature are as they were. (Once the install has started, an
  I/O error can leave `mods/` partly updated; the message says so and names `profile lock`
  as the way to record what is there.)

`--dry-run` resolves and prints the plan and downloads nothing; without a download it goes
by the builds' dates alone, and says so. A Quilt profile takes builds listed for `quilt` or
for `fabric`, whichever is newer.

Bounds: 16 mods asked for, 24 projects looked at, dependencies 4 levels deep, 96 requests
per command, 60 seconds per API request, downloads 2 minutes plus 1 second per 64 KiB (at
most an hour) and at most 512 MiB per file. A transfer that dies is tried once more from the
start; wrong bytes are not.

| Exit status | Meaning |
|---|---|
| 0 | done, or nothing to do |
| 1 | an answer that cannot be read, or no build for this Minecraft version and loader |
| 2 | usage: an unknown project, not a curated profile, a bad name |
| 4 | a download did not match its size or hash |
| 5 | refused by a rule: only pre-releases, only fresh builds, a file the file server has held for less than 48 hours, an unsafe file name or address, a file too large, a clash with a jar in `mods/`, too many dependencies |
| 7 | a network or TLS failure, or an HTTP error from the registry |

Neither `profile add` nor `profile check` is available with `--events`: the graphical front
end has no button for them.

File names outside `A–Z a–z 0–9 . _ + -` are refused rather than renamed. A survey of the
newest builds of the 300 most-downloaded Fabric mods on 2026-11-14 found two such names; a
mod like that, or one that is not on Modrinth, is downloaded by hand and recorded with
`profile lock` as before.

## Asking Modrinth about what is there: `profile check`

```text
bastion-mc profile check <id>
```

It hashes every jar in `mods/` and asks Modrinth which project and version each one is:
**known** (project, version, when it was published), **unknown**, or **fresh** (its build is
less than 48 hours old). It also flags pre-release builds, builds listed for another
Minecraft version or loader, projects that are no longer public, and jars that are not what
the manifest lists. It downloads nothing, so "fresh" goes by the build's date, not the
file's; it changes nothing, and it exits 0 whatever it flags — it is a report, not a gate.

"Known" means that Modrinth lists exactly these bytes under that project. It does not mean
harmless: whoever controls a project's account publishes a bad build together with its hash.
Only Modrinth is asked (CurseForge's API needs a key; GitHub releases are not consulted), so
"unknown" also covers every mod published only elsewhere and every jar you built yourself.
The command prints this caveat with every report.

## What Modrinth learns

| What | When |
|---|---|
| Your address — the proxy's, with `network.launcher_proxy`, which then also resolves the names | every request |
| `User-Agent: bastion-mc/<version>` (Modrinth's API rules ask for a user agent that names the program; nothing about you or the machine) | every request |
| The names you asked for, and the ids of their dependencies | `profile add` |
| The profile's Minecraft version and loader | `profile add` |
| The SHA-512 of every jar in `mods/` — private builds included, by their hash only | `profile add` (when `mods/` is not empty) and `profile check` |
| Which files are downloaded | `profile add` |

No account, cookie or token is involved, and no instance name or local file name is sent.
Nothing from the registry is kept between commands except the address each added file came
from, in the manifest. A launch contacts neither of Modrinth's hosts, and the relay does
not let the game reach them: in a real launch Mod Menu's update check was refused
(`egress: DENY name lookup "api.modrinth.com"`).

## What this does not establish

* **That a mod is benign.** The hash comes from the party that serves the file. A
  compromised project account — or Modrinth itself — serves a bad jar with a matching hash,
  and `profile check` calls it "known". The decision to run a jar is your signature.
* **That a build is its author's.** Modrinth has no author signatures to verify.
* **Agreement between registries.** Only Modrinth is consulted; a build that differs
  between registries is not noticed.
* **That a kept dependency fits.** Whether it satisfies a version range is checked by the mod
  loader when the game starts, not here.
* **Detection.** The 48 hours are a delay: a bad build that nobody noticed in that time is
  taken. The dates are the registry's and its file server's. They say when the registry
  received a build and stored a file, not since when either was public: a project member can
  add a file to a published build and later make it the one that is downloaded, and whether
  a build prepared as a draft or scheduled keeps its creation date is not documented and was
  not tested.
* **When a build's metadata was written.** The list of what a build requires can be edited
  after publication, and nothing dates the edit: a dependency added yesterday to a month-old
  build is followed. It still has to pass every rule above, and it is shown in the plan
  before anything is downloaded.
* **Pinning.** Modrinth's hosts are not pinned (the system's CA store, as for Fabric's and
  Quilt's hosts); `[tls.pins]` can pin them.
* **The static scan** is a signal for review, not a verdict.

The threat model's rows for all of this are C8 to C12, N4 and N5 (§4.4).

## Aliases

An alias names its project by its permanent **id**, not by its slug: Modrinth's
documentation says slugs can change at any moment, and a slug that was given up can be
registered by somebody else. When a project's slug differs from the one recorded, the
output says so. As recorded on 2026-11-14 (UTC):

| Alias | Project id | Project | Licence, as Modrinth states it | Fabric builds for Minecraft 26.3 then |
|---|---|---|---|---|
| `viafabric` | `YlKdE5VK` | ViaFabric | GPL-3.0-only | no release; an alpha (`0.4.22+188-26.x`) |
| `simple-voice-chat`, `voicechat` | `9eGKb6K1` | Simple Voice Chat | LicenseRef-All-Rights-Reserved | no release; a beta (`fabric-2.6.27+26.3`) |
| `fabric-api` | `P7dR8mSH` | Fabric API | Apache-2.0 | release `0.166.2+26.3` |
| `sodium` | `AANobbMI` | Sodium | LicenseRef-Polyform-Shield-1.0.0 | release `mc26.3-0.9.2-fabric` (and newer betas) |
| `lithium` | `gvQqBUqZ` | Lithium | LGPL-3.0-only | release `mc26.3-0.26.3-fabric` |
| `modmenu` | `mOgUt4GM` | Mod Menu | MIT | releases `21.0.0` and `21.0.1`; requires Text Placeholder API and Fabric API |

The last column is a snapshot of that day and will age. The table is compiled into the
launcher; an opt-in test (`BASTION_NET_TESTS=1 cargo test --test catalog live_`) fails when
a project is renamed or removed, which is the cue to update it. The launcher downloads these
mods at your request from where their authors publish them, and bundles, mirrors and
redistributes none of them. The licence is printed with every plan and every check.

## ViaFabric

```sh
bastion-mc profile add perf viafabric --prerelease       # perf: a Fabric profile made with profile new
```

`--prerelease` because, on 2026-11-14, ViaFabric had only alpha builds for Minecraft 26.3.
ViaFabric lets the client join servers of other Minecraft versions; it requires Fabric API
and embeds ViaVersion (GPL-3.0-or-later). Its client-side translation is off until you
switch it on (`enable-client-side` in the instance's `config/ViaFabric/viafabric.yml`, that
is `~/.local/share/bastion-mc/instances/<id>/config/…`, which the mod writes at its first
start; or in the game, as that file says). It translates on the game connection itself, which the relay carries for an
allowlisted server like any other; it needs nothing more from the launcher.

In real demo launches in the sandbox (§7 E23 of the threat model) it loaded, ViaVersion
reported the versions it supports (1.8.x to 26.3), and its update check was refused by the
relay (`egress: DENY name lookup "update.viaversion.com" (not allowlisted)`); the mod logged
that it could not check for updates and carried on. It was not tried against a server:
there is no account on the development machine.

## Simple Voice Chat

```sh
bastion-mc profile add perf simple-voice-chat --prerelease     # or: voicechat
```

`--prerelease` because, on 2026-11-14, Simple Voice Chat had only beta builds for Minecraft
26.3. It is "all rights reserved": a reason never to ship it with the launcher, and no
obstacle to you downloading it from Modrinth. It carries four native libraries (Opus,
RNNoise, Speex, LAME) that it unpacks into the sandbox's `/tmp` and loads there; `profile
audit` lists them. In real demo launches it loaded and initialised all four inside the
sandbox (§7 E20, E23). Two more things are needed before it does anything:

**Sound.** The mod needs the microphone, which means `audio = "pulse"` under `[sandbox]` (the
default; see the README's section on sound for the one-time PipeWire setting). With audio
on, the game can record your microphone whether or not you use voice chat (row D3).

**UDP to the server's voice port.** Voice-chat mods send sound as UDP datagrams to a port of
the game server's machine. In the default `filtered` network mode the game has no UDP at
all, so the mod loads but stays silent. To let it talk, name the server's voice port **in
that server's rule** — in `config.toml` or, for one instance, in that instance's
`instance.toml`, both under `[network]`:

```toml
[network]
servers = [
    "survival.example.org",                              # no voice: nothing changes
    { host = "play.example.org", voice = 24454 },        # Simple Voice Chat's default port
    { host = "lan-box", port = 25570, allow_private = true, udp = [24454, 24455] },
]
```

* `voice = N` and `udp = [N, …]` mean the same thing; both may be given. At most four ports
  per server, sixteen different ones over all rules, each 1024 or higher.
* The port is the one the *server's* voice chat listens on — ask the server's operator if it
  is not 24454. Some hosts run voice on the game port; then it is `voice = 25565`.
* There is no switch that allows UDP for all servers. `launch --join` alone allows none: it
  allowlists the game connection for one session and nothing more. A mistyped key in a
  server's table is an error, not ignored.
* After `profile add` has added Simple Voice Chat it reminds you of this: installing the mod
  opens nothing.

What the launcher then does:

* It relays datagrams to that port **only while a game connection to that server is open**,
  and only **to the address that game connection uses** — it never looks up a name for a
  datagram. Voice cannot go anywhere the game is not already connected to, and it stops when
  you leave the server: as soon as either the game or the server hangs up, nothing more is
  relayed.
* Datagrams are limited to 1400 bytes and to a voice-sized rate: per flow (one socket of the
  mod talking to one server port), 100 a second from you and 500 a second to you; 8 flows at
  once; a flow ends after 30 seconds without a datagram from the game. Nothing is inspected
  or decoded: the launcher does not know that it is voice.
* The launch log says what is open (`network: game server play.example.org:25565 … + UDP
  port(s) 24454 of the same address, while a game connection to it is open`), logs each flow
  once when it starts and once when it ends (`egress: ALLOW udp …`, `egress: done  udp …
  ↑1203 ↓5210 datagrams, 0 refused`), each kind of refusal once per destination
  (`egress: DENY  udp …`), and the totals at the end (`egress summary, UDP: …`).

If voice does not connect, the log tells two cases apart. A line `egress: DENY  udp … (the
server's address reports that datagrams to this port do not arrive (nothing listens
there?))` means the rule names a port on which the server does not listen. No `udp` line at
all means the server announced a port that the rule does not name: such a datagram is
refused inside the sandbox (the mod's socket gets "Permission denied" on a kernel with
Landlock ABI 10) and never reaches the launcher, which has nothing to log.

Not supported:

* A server that tells clients to use a *different* host for voice than for the game
  (`voice_host` in the server's voice-chat configuration) — by design: that would mean
  relaying datagrams to an address that no game connection vouches for.
* The mod's own voice server for single-player and LAN worlds (`run_local_server` in its
  settings) would bind a UDP port of its choosing, which the sandbox refuses in filtered
  mode (§7 E15). That the mod does this was read in the settings it generated; it was not
  tried.
* Voice does not go through `network.launcher_proxy`, and neither does the game connection:
  the server sees your address.

What was verified: the relay in the real sandbox with stand-in game and voice servers on
this machine (round trip with the right source address, every refusal, the limits, expiry,
either side hanging up, the Landlock rule, a real JVM), and inside a real launch of
Minecraft 26.3 with a stand-in client placed in the game's network namespace after the
launcher had confined itself (§7 E22). **Not verified:** a real voice session (that needs a
server running the mod and a signed-in account), capture and playback (sound is off in the
test configuration), and other voice mods, which were not looked at. By the relay's design,
a mod that sends plain UDP to a port of the game server's own address is relayed the same
way, and one that uses another host, STUN, ICE or WebRTC is not. The threat model's row for
this path is J20.

## What none of this proves

A mod that is malicious as published passes every check on this page: the registry lists
it, its hashes match, its build is old enough, and your signature makes it part of the
profile. `profile add` and `profile check` say what the registry lists, not what a jar does,
and `profile audit` says what its classes name, not what they will do. What stands between
such a mod and your account and files is the sandbox, the egress relay and the credential
broker — and what they do not cover is listed in §5 of the threat model.
