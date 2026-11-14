# Minecraft: Bedrock Edition

<!-- Written against docs/notes/round5-contract.md before the five packages were merged. The
statements to check against the merged code are listed in docs/notes/bedrock-risk.md, section 5. -->

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
bastion-mc bedrock import ~/.local/share/mcpelauncher/versions/<version>
bastion-mc bedrock versions                   # what is imported, by id
bastion-mc bedrock remove <id>
```

With the Flatpak build of `mcpelauncher-ui` the versions are under
`~/.var/app/io.mrarm.mcpelauncher/data/mcpelauncher/versions/`.

Import copies one version directory into bastion-mc's store
(`~/.local/share/bastion-mc/bedrock/versions/<id>/`) and records the SHA-256 of every file.
It refuses symbolic links, hard links, device files, paths that lead outside the directory and
trees that are too large, and it accepts only a version directory, never an APK. Each launch
checks the version against that record again, from the very directory it then gives the game,
read-only.

## Creating an instance

```sh
bastion-mc instance new bedrock --bedrock 1.26.62.1
bastion-mc launch bedrock
```

The instance's `instance.toml`:

```toml
kind = "bedrock"
[bedrock]
version = "1.26.62.1"
servers = [{ host = "play.example.org", port = 19132 }]
```

Java-only commands (`install`, `profile …`) refuse a Bedrock instance and say so.
`bastion-mc doctor` has a Bedrock section: whether the licence substitution is accepted,
whether the client was found and is root-owned, which versions are imported, and whether a
session that did not end properly left its sign-in behind.

## Playing

`bastion-mc launch <instance>`, or Launch in the front end. What happens:

* The game runs in the sandbox Java games run in: its own namespaces, an empty file system with
  `/usr` read-only, no home directory, none of your files; it may execute only the client, its
  libraries, the dynamic loader and bastion-mc's sign-in stub; a system-call allowlist; your
  desktop only through the
  game's own display connection (the Wayland security context, or a nested gamescope on X11);
  sound as for Java; memory and process limits. The client's mod loader gets no mods
  directory: native mods are not supported.
* The network is always filtered: the game reaches Microsoft's and Mojang's Bedrock services by
  name, the Bedrock servers you listed, and nothing else (below).
* No Java credential exists in a Bedrock session: the Java sign-in is skipped, and the sandbox
  can reach neither bastion-mc's token store nor the kernel keyring.
* When the session ends, the game's data directory is deleted, with the sign-in in it. Your
  worlds and the game's own settings are kept (`persist_worlds = true`, the default).

### Signing in, every session

The game's sign-in is deleted at the end of every session, so each session that needs your
Microsoft account (servers, Realms, events, capes) starts with a sign-in. Single-player worlds
work without one.

When the game wants you to sign in, bastion-mc shows Microsoft's sign-in page **outside the
game**: in your browser, which the front end opens for you, or at an address the launcher
prints on the terminal (and, if the launch could start it, in a window of bastion-mc's own
sandboxed browser). Sign in there, with two-factor steps as usual. Never type your password into
the game: it never asks for it.

If the page was in your own browser: when you are signed in, the browser ends at an address
that starts with

```
ms-xal-0000000048183522://auth?code=
```

and it says it cannot open it. That is expected: no program on your computer handles that
address, and none should. Copy the whole address from the address bar and paste it where the
launcher asks: at its prompt on the terminal, or in the front end's sign-in prompt. The game
gets that address and nothing else. The code in it is useless to anyone without the secret the
game made for this sign-in.

What bastion-mc checks: it shows only Microsoft's sign-in page for Minecraft for Android's own
client (`login.live.com`, client id `0000000048183522`), and returns only an address of that
client's own form. A game that asks for any other page is refused. Your password, two-factor
codes and the sign-in page's cookies never enter the game's sandbox. A pasted answer is
accepted only from the terminal of a terminal launch or from the front end: the game cannot
answer a prompt.

### Servers

The game can reach the Bedrock servers you list, and no others:

```toml
[bedrock]
servers = [{ host = "play.example.org", port = 19132 }]   # every Bedrock instance
```

An instance adds its own in its `instance.toml` (above). The port defaults to 19132. When the
game tries a server that is not listed, the launcher says so once, with the name and port:
add it and start again. LAN games and friends' worlds (they use NetherNet, WebRTC) are refused
in this version, with a message.

### Realms and event servers

The addresses of Realms and of Mojang's event servers are not known in advance: the game gets
them from Microsoft's services when you join. bastion-mc reads those answers, holds the join,
and asks you **outside the game** whether to go ahead, with the Realm or event and the server's
address. Off by default:

```toml
[realms]
approve = "code"   # or "totp"; "off" (the default) refuses Realms and event-server joins
```

With `"code"` the launcher shows a code and waits for you to type it at its prompt or in the
front end; with `"totp"`, the code of your authenticator app (`bastion-mc realms totp-setup`).
If you did not ask to join, do nothing: the join is refused. A Realm that uses NetherNet is
refused, with a message.

### Telemetry

Microsoft's telemetry hosts are reachable by default, because whether a challenge (such as
"place a copper golem statue") is credited without them is not known:

```toml
[bedrock]
telemetry = false   # the game cannot reach Microsoft's telemetry; a challenge cape may then not be granted
```

### Worlds

Worlds and the game's own settings (`games/com.mojang`) are kept in the instance between
sessions, and `scram` keeps them too: they are not meant to hold a credential.
`persist_worlds = false` deletes them with each session.

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
* **A server cannot be reached**: look for the line that names it, and add it to `servers`.
* **A Realm or event server is refused**: `[realms] approve` is `"off"`, or the join was not
  approved in time, or the Realm uses NetherNet.
* **LAN games or a friend's world**: not supported in this version.
* **The game ends at once, with a message about the system-call filter**: the client made a
  system call that its allowlist does not have. Run it once with `[bedrock] seccomp = "log"`:
  the filter then reports instead of stopping the game. Set it back to `"enforce"` afterwards,
  and report the calls the log names.
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
| `scram` for Bedrock | tests with stand-in sessions (processes that hold a session directory, with and without a marker, one that cannot be stopped) and stand-in token files | a real game's token files |
| The canary and new capes | unit tests with stand-in profiles | a real profile with a cape claimed in Bedrock |
| The sandbox, the network filter, the sign-in, import | the other work packages' tests, against a stand-in client, stand-in servers and a stand-in sign-in page (their notes) | the real client and game, Microsoft's sign-in, a real server, Realm or event |

## What is blocked on you

* Buying Minecraft on Google Play and downloading it with `mcpelauncher-ui`.
* Installing `mcpelauncher-linux` and `mcpelauncher-ui` from the AUR.
* The first real session: the client in the sandbox (and, if the system-call filter stops it,
  one run with `seccomp = "log"`), the Microsoft sign-in with two-factor steps or a passkey, a
  real server, a Realm, an event server.
* Whether a challenge is credited with telemetry on, or off.
* A cape claimed in Bedrock appearing in `bastion-mc skin show`, and `cape set` wearing it —
  which also needs bastion-mc's own Java sign-in, never run live.
