# Minecraft: Bedrock Edition

<!-- Written by the risk package against docs/notes/round5-contract.md; checked against the merged
code and brought up to date by the integrator (docs/notes/bedrock-integration.md, which answers the
re-check list of docs/notes/bedrock-risk.md, section 5). -->

bastion-mc can run Minecraft: Bedrock Edition: the Android build of the game ("Minecraft for
Android"), started by the `mcpelauncher-client` program you installed, inside bastion-mc's
sandbox. It exists mainly for the Bedrock-only steps of Mojang's cape promotions (an event
server, a challenge in Bedrock survival, signing in to Bedrock once), but it is the whole game.
Like the rest of bastion-mc it is pre-release, and the real game has never run under it: there
was no Google Play purchase, no game file and no Microsoft account on the machine it was built
on. [What was verified](#what-was-verified-and-against-what) says exactly what ran.

**Read [What bastion-mc protects, and what it cannot](#what-bastion-mc-protects-and-what-it-cannot)
before you sign in.** On Java, bastion-mc keeps your account's tokens out of the game. On
Bedrock it cannot: the game's own sign-in code needs your Xbox sign-in, so the game holds it
for the whole session.

## What this is, and what it is not

* **It is** the Android build, run by an installed, root-owned `mcpelauncher-client` from the
  [mcpelauncher](https://github.com/minecraft-linux/mcpelauncher-manifest) project, on game
  files that `mcpelauncher-ui` downloaded from **your own Google Play purchase** and that you
  imported into bastion-mc. bastion-mc adds the sandbox, the network filter, the sign-in outside
  the game and the clean-up after each session.
* **It is not a downloader.** bastion-mc contains no Google Play client and no Minecraft files,
  and it never downloads or builds `mcpelauncher-client`.
* **It is not the Windows edition**, in any form. Projects such as BedrockOnLinux run the
  Windows (GDK) build, and that build only runs on Linux after keys are extracted from
  Microsoft's licence data and the game is decrypted outside Microsoft's protected licensing
  path. That is circumventing a technical protection measure. Microsoft's Services Agreement
  forbids it ("circumvent or bypass any technological protection measures … decrypt"), and US
  copyright law (17 U.S.C. §1201) and its EU counterparts treat decrypting a work without the
  owner's authority as circumvention. bastion-mc does not do it, does not wrap a tool that does,
  and has no switch for it. (This is not legal advice.)

## What you need

* **An x86_64 computer.** The packages below are built for x86_64 only.
* **`mcpelauncher-linux` and `mcpelauncher-ui` from the AUR** (both version 1.8.7 on
  2026-11-14). `mcpelauncher-linux` is the client bastion-mc runs; `mcpelauncher-ui` is the
  download tool, and depends on it. They are source builds of about thirty git repositories:
  read each PKGBUILD before you build it, as with any AUR package, then build and install with
  `makepkg -si` (or your AUR helper). Installed this way, the client is a root-owned file in
  `/usr`, which is what bastion-mc requires. It refuses a Flatpak or AppImage client: those
  files live where your user account can change them, and the Flatpak brings a sandbox of its
  own that cannot run inside bastion-mc's.
* **Minecraft bought on Google Play** (`com.mojang.minecraftpe`). It is a separate purchase:
  owning Minecraft for Windows, Xbox, or the Java & Bedrock bundle does not count. The
  mcpelauncher project's own words: "Ownership on the Google Play Store, not any other Store,
  is a hard requirement." **Use a Google account of its own for this purchase**, not the one
  with your mail and photos. Two reasons: `mcpelauncher-ui` keeps a Google sign-in on your disk
  (below), and whether Google ever acts against accounts used with unofficial clients is not
  known. Your Xbox identity is separate from that Google account.
* **Your Microsoft account**: the one that owns your Java profile, if you want the capes on
  Java. You sign in to it in each Bedrock session.
* **Your decision about the licence substitution**, below.
* For capes on Java: bastion-mc's own Java sign-in (README, "Accounts"), which needs your Azure
  application and Mojang's approval of it.
* For the sign-in to come back from your browser by itself: bastion-mc installed from its Arch
  package (it brings the desktop entry) and desktop-file-utils (its pacman hook registers the
  entry; most desktops have it). Without them you paste the address by hand
  ([Signing in](#signing-in-every-session)).

## The licence substitution, and its switch

This is the one licence-related fact about this setup, and you decide whether you accept it.

When the game starts, it asks Google Play whether this copy is licensed. There is no Google Play
on Linux, so `mcpelauncher-client` answers that question itself, with "licensed". The real check
happened earlier: `mcpelauncher-ui` downloads the full game only after Google said that your
Google account bought it. When it cannot confirm that, it starts the client in a mode that runs
only free trial versions. (Read in the projects' source code: the client's `src/jni/store.cpp`
and `src/main.cpp`, the UI's `googleplayapi.cpp` and `gamelauncher.cpp`.)

What bastion-mc does about it: nothing. It decrypts nothing, patches no game file and answers
no licence query; it runs the client you installed, on files you imported. It also **cannot
check that a version you import was bought**: it has no Google client, by design. Import only
versions that `mcpelauncher-ui` downloaded from your own purchase. (`mcpelauncher-client`
itself changes some of the game's code in memory to make it run on Linux; that is its code, not
bastion-mc's, and no file changes.)

Every Bedrock launch is refused until you accept this in `~/.config/bastion-mc/config.toml`:

```toml
[bedrock]
accept_licence_substitution = true
```

## Getting the game with mcpelauncher-ui

Start `mcpelauncher-ui` (it is not part of bastion-mc and does not run in its sandbox), sign in
with the Google account that bought Minecraft, and let it download a version. You do not need
to start the game from there. If you do, none of bastion-mc's protection applies to that
session, and the game keeps its Xbox sign-in in `mcpelauncher-ui`'s data directory, where
bastion-mc's `scram` does not reach it.

**The Google sign-in.** `mcpelauncher-ui` keeps it in its own settings file. Its source shows
the token encrypted with a key that is stored in the same settings, unless its sign-in window
supplies a key (that window's source was not available to check): treat it as readable by any
program running as you. bastion-mc never sees it and never removes it, not even with `scram`.
Sign out in `mcpelauncher-ui` once your versions are downloaded: you need the sign-in only to
download.

Versions: mcpelauncher 1.8.7 knew x86_64 builds up to 1.26.62.1 (release) and 1.26.70.24
(beta) on 2026-11-14.

## Importing a version

```sh
bastion-mc bedrock client                     # the installed client bastion-mc would run, and why (or why not)
bastion-mc bedrock versions                   # what is imported, and what mcpelauncher-ui has that is not
bastion-mc bedrock import ~/.local/share/mcpelauncher/versions/<version>
bastion-mc bedrock versions --verify          # check every file again, as each launch does
bastion-mc bedrock remove <id>
```

With the Flatpak build of `mcpelauncher-ui` the versions are under
`~/.var/app/io.mrarm.mcpelauncher/data/mcpelauncher/versions/` (`bedrock versions` lists both
places).

Import copies one version directory into bastion-mc's store
(`~/.local/share/bastion-mc/bedrock/versions/<id>/`), and only what the client reads:
`AndroidManifest.xml`, `lib/x86_64/` and `assets/`. It takes x86_64 builds of
`com.mojang.minecraftpe` only. It refuses symbolic links, hard links, device files, names that are
not plain, and trees beyond its limits (8 GiB, 2 GiB for one file, 200,000 entries, 24 levels below
`assets/`, 64 libraries — guesses, since no real version could be measured), and it accepts only a
version directory, never an APK. It records the SHA-256 of every file, last, and leaves the copy
read-only. The version is named by its version name (`1.26.62.1`), or `--id`; an imported version
is never replaced (remove it first). Each launch checks the version against that record again, from
the very directory it then gives the game, read-only; a changed, added or missing file stops the
launch with "INTEGRITY VIOLATION" and its name. `bedrock remove` is refused while a session uses
the version.

## Creating an instance

```sh
bastion-mc instance new bedrock --bedrock 1.26.62.1
bastion-mc launch bedrock
```

The instance's `instance.toml`:

```toml
kind = "bedrock"
# width = 1280          # the window the client opens with, both or neither
# height = 720

[bedrock]
version = "1.26.62.1"   # an id in the store (`bastion-mc bedrock versions`)
servers = [{ host = "play.example.org", port = 19132 }]   # added to [bedrock] servers

[network]
# mode = "filtered"     # or "none": offline single-player, nothing reachable ("open" is refused)
```

Java keys mean nothing in a Bedrock instance and are refused with the reason (`minecraft`,
`memory_mb` — the session's memory ceiling is `[sandbox] memory_max_mb` —, `[network] servers`,
`[companion]`). Java-only commands (`install`, `profile …`, `pack …`) refuse a Bedrock instance
and say so. One session per instance runs at a time.
`bastion-mc doctor` has a Bedrock section: whether the licence substitution is accepted,
whether the client was found and is root-owned, which versions are imported, and whether a
session that did not end properly left its sign-in behind.

## Playing

`bastion-mc launch <instance>`, or Launch in the front end. What happens:

* The game runs in the sandbox Java games run in: its own namespaces, an empty file system with
  `/usr` read-only, no home directory, none of your files; it may execute only the client, its
  libraries, the dynamic loader and bastion-mc's sign-in stub; a system-call allowlist of the
  client's own (with the same tripwire as Java's; new namespaces and the kernel keyrings are always
  refused); your desktop only through the game's own display connection (the Wayland security
  context, or a nested gamescope on X11); sound as for Java; memory and process limits. The
  client's mod loader finds an empty, read-only mods directory: native mods are not supported.
* The network is filtered (the default): the game reaches Microsoft's and Mojang's Bedrock
  services by name, the Bedrock servers you listed, and nothing else (below). With
  `[network] mode = "none"` in the instance nothing is reachable (single-player, no sign-in);
  `"open"` is refused for Bedrock.
* No Java credential exists in a Bedrock session: the Java sign-in is skipped, and the sandbox
  can reach neither bastion-mc's token store nor the kernel keyring. `launch --demo` therefore
  changes nothing for Bedrock (the front end does not pass it); `--burner`, `--join` and
  `--repair` are Java features and are refused with what to do instead.
* The game's data directory belongs to this session alone: a fresh directory in bastion-mc's cache,
  deleted when the session ends, with the sign-in in it. Your worlds and the game's own settings
  are kept (`persist_worlds = true`, the default; see [Worlds](#worlds)).

### Signing in, every session

The game's sign-in is deleted at the end of every session, so each session that needs your
Microsoft account (servers, Realms, events, capes) starts with a sign-in. Single-player worlds
work without one.

When the game wants you to sign in, bastion-mc shows Microsoft's sign-in page **outside the
game**: the front end's *Microsoft sign-in for Bedrock* panel opens it in your browser (*Open the
sign-in page*; only `https://login.live.com/…` is ever offered), and a terminal launch prints its
address on a line of its own. Sign in there, with two-factor steps or a passkey as usual. Never
type your password into the game: it never asks for it.

At the end, Microsoft sends your browser to an address that starts with

```
ms-xal-0000000048183522://auth?code=
```

— an address for the game, not for the browser. **Where bastion-mc's handler for these links is
registered, the browser hands it back by itself**: the Arch package installs a desktop entry for
the `ms-xal-0000000048183522` scheme (`/usr/share/applications/bastion-mc-bedrock-signin.desktop`,
hidden from menus), and desktop-file-utils' pacman hook registers it. Your browser may ask once
whether to open such links with bastion-mc: allow it. It then runs
`bastion-mc bedrock hand-back <address>`, which hands the address to the running Bedrock session
and opens nothing; the game continues. To check the registration:
`xdg-mime query default x-scheme-handler/ms-xal-0000000048183522` names
`bastion-mc-bedrock-signin.desktop`.

The handler reaches one session at a time: a Bedrock session that starts while no other one holds
it. One started while another runs takes pasted addresses only, for its whole session, and its
prompt and launch log say so. Without `XDG_RUNTIME_DIR` (no desktop session) there is no handler
either.

**If nothing happens — or without the handler — paste the address.** Your browser may not show it
at all: Firefox (checked with version 159) stays on the page before, with nothing in the address
bar and nothing in the console. To get it:

1. Before the last step of the sign-in (or before you start it), open the browser's developer
   tools with **F12** and choose the **Network** tab.
2. Sign in. The last request in the list has failed (in Firefox its transfer column shows an
   error beginning `NS_ERROR_UNKNOWN`; its status is 302). Select it.
3. In its **response headers**, the `Location` header holds the address. Copy its value.
4. Paste it into the field of the front end's panel and press *Send* (or Enter) — or, on a
   terminal launch, paste it into the terminal and press Enter.

Either way, bastion-mc checks it (it must be the game's address, carry a sign-in code, and belong
to *this* sign-in) and hands the game only the code and the sign-in's state, nothing else. The
prompt says why an address was not accepted; you have 10 tries (an address the handler brought
that was refused does not count: any web page can make a browser call the handler) and 15
minutes. *Cancel* (or typing `cancel` on the terminal) refuses the sign-in, and the game is told
that it failed. If you did not press *Sign in* in the game, cancel. One prompt at a time, at most
6 per session, and after one ends without an address the next waits 10 seconds: a game cannot
keep a prompt in front of you.

What bastion-mc checks: it shows only Microsoft's sign-in page for Minecraft for Android's own
client (`login.live.com`, client id `0000000048183522`), and returns only an address of that
client's own form. A game that asks for any other page is refused. Your password, two-factor
codes and the sign-in page's cookies never enter the game's sandbox. The address is accepted
only from outside the game — the terminal of a terminal launch, the front end, or the desktop's
handler (a socket in your private runtime directory, which the game's sandbox cannot reach) — so
the game cannot answer its own prompt.

### Servers

The game can reach the Bedrock servers you list, and no others:

```toml
[bedrock]
servers = [{ host = "play.example.org", port = 19132 }]   # every Bedrock instance
```

An instance adds its own in its `instance.toml` (above). The port defaults to 19132; ports from
1024 up; at most 64 servers and 16 different ports in all; an IPv4 address works too, an IPv6
address does not (use a name: it may resolve to IPv6). The launcher looks each name up once per
session, and the game's datagrams go to the address it found and nowhere else. When the game
looks up a name that is not listed, the launcher says so once, with what to add
(`add { host = "…" } … to [bedrock] servers`): add it and start again. A server typed into the
game as an IP address that is not listed never reaches the launcher (the game's datagrams have
no route), so there is nothing to say about it: list it. Servers on a private or loopback address
(a LAN server, one on this computer), LAN games and friends' worlds (they use NetherNet, WebRTC)
are refused in this version, with a message.

### Realms and event servers

The addresses of Realms and of Mojang's event servers are not known in advance: the game gets
them from Microsoft's services when you join. bastion-mc reads those answers, holds the join,
and asks you **outside the game** whether to go ahead, with the Realm or event and the server's
address. Off by default:

```toml
[realms]
approve = "code"   # or "totp"; "off" (the default) refuses Realms and event-server joins
```

The launcher shows the Realm (or event) and the server's address with a six-digit code: approve
in the front end's Realms panel (*Approve join*, which sends the code shown; *Refuse*), or type
the code on the terminal of a terminal launch and press Enter (`cancel` refuses; when a sign-in
waits at the same time, start the line with `realms `). There is no phone in a Bedrock session:
the code is typed outside the game, where the game cannot type. `"totp"` acts as `"code"` in a
Bedrock session — the TOTP secret is in bastion-mc's token store, which a Bedrock session never
opens — and the launch log says so. If you did not ask to join, do nothing: the join is refused.
Approved, the game gets a one-time pass that leads to that one server (one join, at most four
connections within 30 seconds). A Realm that uses NetherNet is refused, with a message.

### Telemetry

Microsoft's telemetry hosts are reachable by default, because whether a challenge (such as
"place a copper golem statue") is credited without them is not known:

```toml
[bedrock]
telemetry = false   # the game cannot reach Microsoft's telemetry; a challenge cape may then not be granted
```

With `false` the relay refuses the telemetry hosts, and the client is also told to keep the
game's telemetry from starting (`MCPELAUNCHER_DISABLE_TELEMETRY=1`, its own switch).

### Worlds

Kept in the instance's `bedrock-data/` between sessions, and nothing else: the worlds
(`games/com.mojang/minecraftWorlds/`), the game's own settings (`games/com.mojang/minecraftpe/`,
with `options.txt`), and the client's settings file (`mcpelauncher-client-settings.txt`: window,
scale, overlay switches — none of its keys names a file or a program). They have mcpelauncher's own
layout, so worlds can be copied in and out there; anything else copied into `bedrock-data/` is not
shown to the game. Everything else the game writes — global resource packs, skin packs and world
templates included — ends with the session. `scram` keeps the worlds too: they are not meant to
hold a credential (not checked against the real game). `persist_worlds = false` keeps nothing at
all: every session starts empty.

## Capes: from Bedrock to your Java profile

Mojang has repeatedly put cross-edition capes behind something only Bedrock can do: event
servers (Cherry Blossom 2023, MCC 15th Year 2024, Yearn 2025), signing in to Bedrock once (the
Common Cape, until 2026-05-05), challenges in Bedrock survival (Copper 2025, Zombie Horse 2025).
Many other capes need no Bedrock at all (codes at minecraft.net/redeem, other games). No
Bedrock-only promotion was running on 2026-11-14.

1. Sign in to Bedrock with the **same Microsoft account** that owns your Java profile: a cape
   belongs to the account.
2. Do what the promotion asks. If it is an event server or a Realm, set up the approval above;
   if it is a challenge, leave telemetry on.
3. When the session ends the launcher says what comes next: the cape shows on your Java profile
   later. Mojang has announced anything from "as soon as the sign-in completes" to "within 30
   days", depending on the promotion.
4. On Java: `bastion-mc skin show` lists the capes your profile owns, and

   ```sh
   bastion-mc cape set "Zombie Horse"     # the name as skin show lists it; or: none
   ```

   wears one.

**The profile canary and a new cape.** The canary never treats a newly owned cape as a change:
nobody breaks into an account to give it a cape. At your next Java sign-in, launch or
`skin`/`cape` command it names the new cape ("new cape(s) on the account: …"), pins it, and
carries on; there is nothing to accept. A change it does report after a Bedrock session — a
different player name or skin, another cape worn — is either one you made yourself
(`bastion-mc canary accept`) or the alarm the canary exists for (`bastion-mc wiki scram`): the
Bedrock game held your Xbox sign-in, and may be able to turn it into a Java token (not verified).

Not verified, because it needs your accounts: that a cape claimed in Bedrock appears in the
Java profile's list of capes where `cape set` finds it (that is how Java capes work, and Mojang
says these capes are wearable on Java), and that bastion-mc's own Java sign-in works against
Microsoft (it has never run live).

## What bastion-mc protects, and what it cannot

**The game holds your Xbox identity.** Once you sign in, the game holds a refresh token of your
Microsoft account for Minecraft for Android, the key it made to sign its Xbox requests, and the
Xbox and Minecraft tokens made with them. With those, whoever controls the game can, for as long
as the refresh token works:

* join any server as you, and use your Realms (with the owner's rights, if you own a Realm);
* spend your **Minecoins** on Marketplace items (buying Minecoins with money does not work in
  this build: every in-app purchase fails);
* change your Bedrock character and its cape, and act on your Xbox friends and presence;
* possibly — not verified — get a token for your Java profile (skin, cape, name).

"Whoever controls the game" means: the game itself if it has a flaw, a hostile server, Realm or
add-on that exploits one (Bedrock is native code with a script engine for add-ons), or the
client program, if it is compromised. Nothing can take the sign-in away from the game, because
the game's own sign-in code needs it — unlike Java, where the launcher keeps the token.

**What bastion-mc does about it:**

* The sandbox keeps the game from your files, your desktop beyond its own window, your devices
  and your other programs, and limits its memory and processes.
* The network filter narrows where the game can send anything: Microsoft's and Mojang's Bedrock
  services, the servers you listed, Realms and event servers you approved.
* Sign-in pages run outside the game: your password and two-factor codes never reach it, and the
  game cannot make the launcher open any other page.
* No Java credential exists in a Bedrock session.
* The game's sign-in lives one session: its data directory is deleted when the session ends,
  and, after a crash, at the next start or by `scram`.
* `doctor` reports a sign-in that a crashed session left behind; `scram` removes it; the profile
  canary watches your Java profile.

**What it cannot do:**

* Stop the game from using your Xbox sign-in during the session, or from copying it out through
  any connection it is allowed: to an Xbox or Minecraft service, to a server you listed, to a
  Realm you approved. A copied refresh token works until you sign out everywhere.
* See what happens on the Bedrock side. The canary watches the Java profile only: Minecoins
  spent, Realms changed, a different Bedrock character — check those in the game, on
  minecraft.net and in your Xbox account.
* Check that the game was bought, audit `mcpelauncher-client` (unreviewed native code that
  changes the game in memory), or protect what `mcpelauncher-ui` keeps (your Google sign-in) or
  what the game kept when you played from `mcpelauncher-ui` directly.
* Say whether Google or Microsoft act against accounts used with unofficial clients: not known.

**If you think the game or a server you joined was hostile:** sign out everywhere at Microsoft.
That ends the game's sign-in and bastion-mc's Java sign-in alike. It is on the "Manage how I sign
in" page of your Microsoft account:

    https://account.live.com/proofs/manage/additional

Microsoft says it takes effect "within 24 hours" and does not cover Xbox consoles (sign out on
the console). `bastion-mc wiki scram` has the whole procedure.

## Emergency: scram

`bastion-mc scram` treats a Bedrock session like a Java launch, and its game's sign-in like a
credential:

* A running Bedrock session is stopped first, with its game; the sign-in in the game's memory
  ends with it.
* What killed or crashed sessions left on disk (their data directory, with the game's sign-in)
  is deleted with the credentials, after bastion-mc's own, and the deletion is confirmed by
  looking again.
* A session it cannot stop, or one it cannot find because its marker was deleted, is reported
  on a line that starts with `COULD NOT`: end it yourself and sign out everywhere. Its directory
  is not deleted under it.
* **Kept:** Bedrock worlds and the game's own settings, and imported versions. They are not
  meant to hold a sign-in (not checked against the real game). Delete the instance yourself if
  you want its worlds gone.
* Nothing is revoked at Microsoft: that is the runbook's next step, sign out everywhere.

## Troubleshooting

* **"Bedrock is off until you accept how its licence check works"**: the switch above.
* **"mcpelauncher-client: …"** (in the launch's refusal or in `doctor`): the client was not
  found, is not root-owned, has a path component you can write to, or is a Flatpak or AppImage.
  Install `mcpelauncher-linux` from the AUR, or point `[bedrock] client` at the installed
  program.
* **No version, or a version that fails its check**: `bastion-mc bedrock versions`; import it
  again from `mcpelauncher-ui`'s directory.
* **The game asks you to sign in every time**: by design; the sign-in is deleted after each
  session.
* **The sign-in page did not open**: the address is in the launcher's output; open it in your
  browser yourself.
* **After the sign-in nothing happens in the game**: the browser did not hand the address back.
  Check that the handler is registered (`xdg-mime query default
  x-scheme-handler/ms-xal-0000000048183522` should name `bastion-mc-bedrock-signin.desktop`; the
  desktop-file-utils package's pacman hook registers it), that you allowed the browser to open the
  link, and that no other Bedrock session was running (the first one holds the handler). Then
  paste the address instead (above). `bastion-mc bedrock hand-back '<address>'` in a terminal does
  what the handler does, and says what happened.
* **A server cannot be reached**: look for the line that names it, and add it to `servers`.
* **A Realm or event server is refused**: `[realms] approve` is `"off"`, or the join was not
  approved in time, or the Realm uses NetherNet.
* **LAN games or a friend's world**: not supported in this version.
* **The game ends at once, or a feature fails, and the system-call filter is the suspect**: the
  client's allowlist was derived from its source code, not measured on the real client. Run one
  session with `[bedrock] seccomp = "log"`: calls outside the allowlist are then let through and
  logged by the kernel (`journalctl -k -g 'type=1326'`; `syscall=` is the x86-64 number); the
  tripwire and the refusal of namespaces, keyrings, io_uring, userfaultfd and perf events still
  hold. Put what it names into `[bedrock] extra_syscalls = ["name", …]`, set `seccomp` back to
  `"enforce"`, and report the calls. (A kill by the tripwire, SIGSYS, is not this: the launch log
  says to treat the server and content as hostile.)
* **`doctor` warns about a leftover session directory**: a session did not end properly. The
  next Bedrock start deletes it, and so does `bastion-mc scram`.
* **Xbox invitations do not arrive, and friends cannot join your world**: known limits of
  mcpelauncher, not of bastion-mc (its README: invitations cannot be received, and others "may
  be unable to join worlds hosted by this launcher").

## What was verified, and against what

On the development machine (Arch Linux, kernel 7.2), on 2026-11-14, with no
`mcpelauncher-client`, no game file, no Google Play purchase and no Microsoft account — so none
of this ran with the real game:

| Part | Verified against | Not run |
|---|---|---|
| The licence gate, `doctor`'s Bedrock section | unit tests; `doctor` in isolated directories with a stand-in store and a stand-in leftover session | the client's discovery against the real package |
| Finding the client, importing and checking versions | unit tests on fake install trees and synthetic versions (links, devices, limits, tampering); the `bedrock` commands end to end with the real binary; `/usr/bin/true` standing in for a root-owned client | a real `mcpelauncher-linux` package, a real downloaded version |
| A whole session | the launch end to end with a stand-in client (a program the test compiles, started as the client in the real sandbox): its arguments, the read-only version opened from the store, the data split (a world kept, tokens and a planted library gone, a crashed session's directory deleted at the next start), the stub over the installed webview, no home, no keyring, no namespaces, HTTPS through the relay with the system's libcurl, the launcher confining itself | the real client and game, a display, sound, a GPU, resource limits |
| The sign-in | the stand-in running `mcpelauncher-webview` inside the sandbox under the client's own filters: the stub reached the launcher, the page was offered, another sign-in's address was refused, the right one came back to the stand-in rebuilt — pasted at the trusted input, and handed back by the real `bastion-mc bedrock hand-back` with the launcher confined; a foreign address, no session; the front end's prompt in its emulator; the desktop entry resolved by `xdg-mime` and `gio` in isolated directories, and `gio open` running the command; Firefox 159 against a stand-in redirect page (where the final address shows) | Microsoft's real sign-in, two-factor, passkeys, the game's sign-in library accepting the address; a real browser calling the handler |
| The network | a sandboxed client (Python, the system's curl) with the real relay: RakNet to stand-in servers, refusals, a Realms and an event-server join through stand-in services over TLS approved through the trusted input, the per-launch authority accepted by curl, the relay working after the launcher confined itself | a real Bedrock server, Realm or event server; the game's own TLS stack |
| `scram` for Bedrock | tests with stand-in sessions (processes that hold a session directory, with and without a marker, one that cannot be stopped) and stand-in token files | a real game's token files |
| The canary and new capes | unit tests with stand-in profiles | a real profile with a cape claimed in Bedrock |

## What is blocked on you

* Buying Minecraft on Google Play and downloading it with `mcpelauncher-ui`.
* Installing `mcpelauncher-linux` and `mcpelauncher-ui` from the AUR.
* The first real session: the client in the sandbox (and, if the system-call filter stops it,
  one run with `seccomp = "log"`), the Microsoft sign-in with two-factor steps or a passkey, a
  real server, a Realm, an event server.
* Whether a challenge is credited with telemetry on, or off.
* A cape claimed in Bedrock appearing in `bastion-mc skin show`, and `cape set` wearing it —
  which also needs bastion-mc's own Java sign-in, never run live.
