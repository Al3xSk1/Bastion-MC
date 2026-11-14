# Round 5, package "risk": the gate, scram, the canary, doctor and the user guide

Clone `target/agents/r5-risk`, branch `work`, from the round-5 base `3f78f31`. Written on
2026-11-14. Hand-over log: `bedrock-risk-progress.md`. User guide: `docs/BEDROCK.md`.

**Honest limits first.** There is no `mcpelauncher-client`, no game file, no Google Play
purchase and no Microsoft account on this machine, and none was installed or used. Everything
below was verified against stand-ins written for the purpose: directories that stand for a
session's data directory and for imported versions, files that stand for the game's token
store, processes (this project's test binaries) that stand for a running Bedrock session, and
profiles that stand for Mojang's answers. Where the real game keeps its tokens was read in the
client's source code, not observed. §3 lists exactly what ran.

Contents: 1. user documentation (README text, docs index line; the guide itself is
`docs/BEDROCK.md`) · 2. threat model (assets, adversaries, rows B1–B13, §5/§6/§7 additions) ·
2.5 the runbook ("sign out everywhere") · 3. what was verified, against what · 4. what was
built, entry points, integration · 5. statements to re-check against the other packages ·
6. contract changes · 7. open items.

---

## 1. User documentation

The user guide is **`docs/BEDROCK.md`** (new file): what this is and is not (the Windows/GDK
paragraph), what is needed, the licence substitution and its switch, `mcpelauncher-ui` and its
Google sign-in, importing, the instance, playing, signing in every session, servers, Realms and
event servers, telemetry, worlds, capes from Bedrock to Java, what bastion-mc protects and what
it cannot, scram, troubleshooting, what was verified, what is blocked on the user.

### 1.1 README text (for the integrator; README.md is not edited this round)

**Intro**, after the paragraph about the companion mod:

> It can also run Minecraft: Bedrock Edition — the Android build, started by your installed
> `mcpelauncher-client` from your own Google Play purchase — in the same sandbox, mainly for
> Mojang's Bedrock-only cape events. There, unlike on Java, the game itself holds your Xbox
> sign-in for the session; [`docs/BEDROCK.md`](docs/BEDROCK.md) says what that means.

**Status table**, a row:

> | Minecraft: Bedrock Edition (the Android build, run by your installed `mcpelauncher-client`) |
> against stand-ins only: the licence gate, `doctor`'s Bedrock section, scram with stand-in
> sessions (running, crashed, unmarked, unstoppable) and token files, the canary with stand-in
> profiles; the sandbox, network filter, sign-in bridge and import against a stand-in client,
> stand-in servers and a stand-in sign-in page (the packages' notes). **Not run with the real
> client or game**: no Google Play purchase, game file or Microsoft account on the development
> machine |

**Contents**: `- [Minecraft: Bedrock Edition](#minecraft-bedrock-edition)` after "Settings sync".

**A section**, after "Settings sync":

> ## Minecraft: Bedrock Edition
>
> bastion-mc runs the Android build of Bedrock: the `mcpelauncher-client` you installed from the
> AUR (`mcpelauncher-linux`), on game files that `mcpelauncher-ui` downloaded from **your own
> Google Play purchase** (a separate purchase; buy it with a Google account of its own). It
> does not run the Windows edition in any form: that edition runs on Linux only by decrypting
> it outside Microsoft's protected path, which is circumvention.
>
> ```toml
> [bedrock]
> accept_licence_substitution = true   # every Bedrock launch is refused without it
> ```
>
> `mcpelauncher-client` answers the game's Google Play licence query itself, after
> `mcpelauncher-ui` checked with Google that you bought the game; bastion-mc decrypts, patches
> and answers nothing, and cannot check that a version you import was bought. Then:
>
> ```sh
> bastion-mc bedrock import ~/.local/share/mcpelauncher/versions/<version>
> bastion-mc instance new bedrock --bedrock <version-id>
> bastion-mc launch bedrock
> ```
>
> The game runs in the same sandbox as Java games, the network is always filtered (Microsoft's
> and Mojang's Bedrock services by name, the servers you list, Realms and event servers only
> after you approve each join outside the game), and Microsoft's sign-in page opens outside the
> game, every session: the game's sign-in is deleted when the session ends. No Java credential
> exists in a Bedrock session.
>
> **What it cannot do:** the game's own sign-in code needs your Xbox sign-in, so the game holds
> it for the whole session. A game taken over — by a flaw that a hostile server or add-on
> exploits — can use it and copy it out through any allowed connection: your Realms, your
> Minecoins, your Bedrock character are at stake until you sign out everywhere at Microsoft.
> Capes earned in Bedrock show on your Java profile later; `bastion-mc skin show` lists them,
> `bastion-mc cape set` wears one, and the profile canary names a new cape and pins it without
> an alarm. Everything else, and what was verified (none of it with the real game), is in
> [`docs/BEDROCK.md`](docs/BEDROCK.md).

**Emergency section**, a paragraph after the one about the phone's services:

> Bedrock sessions are stopped like Java launches; what a killed or crashed Bedrock session left
> on disk — its data directory, with the game's Xbox sign-in — is deleted with the credentials
> and the deletion confirmed. A Bedrock session that cannot be stopped, or that runs without a
> marker, is a `COULD NOT` line that says to end it and to sign out everywhere: a running
> Bedrock game holds your sign-in. Bedrock worlds and imported versions are kept, and the output
> says so.

**Configuration section**: `[bedrock]` (`accept_licence_substitution`, `client`, `telemetry`,
`persist_worlds`, `seccomp`, `servers`) is in `examples/config.toml` already (base).

**Tests section**: the test counts change by the tests in §3.

### 1.2 docs/README.md (index), a line under "For people who use it"

> - [BEDROCK.md](BEDROCK.md) — Minecraft: Bedrock Edition, the Android build in bastion-mc's
>   sandbox: what it is and is not (no Windows edition), what you need (the AUR packages, a
>   Google Play purchase), the licence substitution, signing in every session, servers, Realms
>   and events, capes from Bedrock to Java, and what bastion-mc cannot protect (the game holds
>   your Xbox sign-in); for anyone who wants to play Bedrock under it.

And under the engineering notes, a round-5 block: `round5-contract.md`, `bedrock-research.md`,
and each package's `bedrock-<package>.md` / `-progress.md`.

---

## 2. Threat model (for `docs/THREAT_MODEL.md`)

### 2.1 Assets (§1), a row

> | The account's **Xbox identity in the Bedrock game**: a refresh token of the Microsoft account
> for Minecraft for Android (client `0000000048183522`, Xbox user-auth scope), the key the game
> made to sign its Xbox requests, Xbox and Minecraft tokens | the refresh token: weeks–months.
> On this machine: one session — in the game's memory, and in the session's data directory
> until it is deleted (end of session; after a crash, the next Bedrock start or `scram`) | Its
> holder can join servers and Realms as you (with the owner's rights on your Realms), spend
> your Minecoins on Marketplace items, change your Bedrock character and cape, act on your Xbox
> friends and presence — and possibly (not verified) obtain a Java token. Unlike the Java
> tokens, **the game holds it**: nothing can broker it (B5). |

### 2.2 Adversaries (§2), a paragraph under the table

> For Bedrock (§4.8): **S** also means a hostile Bedrock server, Realm, event server or add-on.
> Bedrock is native C++ with a JavaScript engine for add-ons; a memory-safety bug there gives
> native code in the game, the capability of **M**. **M** is native code loaded by mcpelauncher's
> own mod loader, which gets no mods directory. **C** also means the `mcpelauncher-client` build
> (about thirty repositories, built from the AUR on your machine) and the game files imported
> from `mcpelauncher-ui`.

### 2.3 Trust boundaries (§3), a sentence after the diagram

> In a Bedrock session the process in the sandbox is `mcpelauncher-client`, which loads the
> game's `libminecraftpe.so` with its own linker; the game's data directory is per-session
> scratch, the client's sign-in window is replaced by the launcher's stub (the launcher binary
> itself), and the launcher holds no credential of any kind during that session (§4.8).

### 2.4 A new section "4.8 Minecraft: Bedrock Edition (the Android build)"

Intro paragraph:

> bastion-mc runs the user's installed `mcpelauncher-client` on game files the user downloaded
> with `mcpelauncher-ui` from their own Google Play purchase (`docs/BEDROCK.md`). The difference
> from Java that everything below follows from: **the game's own sign-in library needs the
> account's Xbox identity, so the game holds it**. The launcher cannot keep it out (as the
> credential broker does on Java); it limits where the game can send it, keeps it for one
> session only, keeps sign-in pages and Java credentials out of the game, and removes what is
> left. None of this has run with the real client or game (§7 E33, E34).

| # | Attack | Defence | Verified by | Residual |
|---|---|---|---|---|
| B1 | **Licence check and DRM**: bastion-mc used to defeat or answer a licence check, or to decrypt the game | No circumvention in bastion-mc's code: it decrypts nothing, patches no game file, answers no licence query and contains no Google Play client. `mcpelauncher-client` answers the game's Google Play licence query itself (`hasVerifiedGooglePlayStoreLicense = true`), after `mcpelauncher-ui` asked Google when it downloaded the game (it starts the client with `--free-only`, trial builds only, when Google did not confirm). Every Bedrock launch is refused until the user accepts that (`[bedrock] accept_licence_substitution`), and the refusal says what is accepted and what bastion-mc does not do. The Windows (GDK) build is not supported in any form | `bedrock::tests::bedrock_is_refused_until_the_licence_substitution_is_accepted`, `bedrock::policy::tests::the_gate_names_what_is_accepted_and_what_bastion_does_not_do`; the Bedrock launch package's test that the gate comes first (re-check) | bastion-mc cannot check that an imported version was bought: it has no Google client, by design, and a version that was not bought runs as well. `mcpelauncher-client` changes the game's code in memory to run it (its `CorePatches` and others; the patch of a UI licence-key check exists in 32-bit x86 builds only, not in the x86_64 build used here) |
| B2 | **The client program** (C): a tampered or user-writable `mcpelauncher-client` | Installed by a system package: root-owned, no writable path component (the JDK's rule); Flatpak and AppImage refused; bastion-mc never downloads or builds one | the store package's `runtime` tests (re-check) | Unaudited native code, built from the AUR by the user; bastion-mc trusts what the package manager installed, as it trusts the JDK. An AUR PKGBUILD is user-submitted: read it before building |
| B3 | **The game files** (C): changed in the store, or swapped between check and use | Imported with a per-file SHA-256 manifest; links, hard links, devices, escapes and oversize trees refused; never an APK; checked again at each launch and bound by the descriptor that was checked | the store package's tests (re-check) | The manifest proves the files did not change since import, not that Mojang built them (unless the store package checks a signer: re-check) |
| B4 | **Hostile game, server or content (S), native mods (M), against your files and session** | The Java sandbox, with a profile of its own: namespaces, empty root, `/usr` read-only, synthetic `/etc`, no home; Landlock lets only the client, its libraries, the loader and the stub execute; a system-call allowlist of its own with the tripwire, namespace creation denied; the display only through the security context or the nested compositor; no `/dev/input`, no hidraw; cgroup limits. The client's mod loader gets no mods directory | the launch package's stand-in client tests (re-check) | The allowlist was never profiled with the real client (`[bedrock] seccomp = "log"` exists for that, on your machine). Kernel and GPU-driver surface (§5.5); another program of yours reading the game's memory (P7) |
| B5 | **The game's Xbox identity carried away** (S, M) | Cannot be prevented: the game's sign-in library needs the refresh token and its key inside the game. What narrows it: the filtered network only — HTTPS by server name to Microsoft's and Mojang's Bedrock services, RakNet only to listed servers, Realms and event servers only after an approval outside the game, LAN and NetherNet refused, the Java services denied; and no Java credential in the session (B7) | `bedrock::net::tests::*`, the network package's tests (re-check) | **The game holds the account's Xbox identity for the whole session.** A game taken over can use it, and copy it out through any allowed connection (an Xbox or Minecraft service, a listed server, an approved Realm); the copy works until "sign out everywhere" (which Microsoft says takes up to 24 hours). At stake: joining as you, your Realms (with the owner's rights), your Minecoins, your Bedrock character and cape, your Xbox friends; possibly your Java profile (not verified). The name suffixes (`*.xboxlive.com`, `*.minecraft-services.net`, `*.playfabapi.com`) are broad: every host under them is reachable. Telemetry is reachable by default (whether challenge credit needs it is unknown) |
| B6 | **The game's sign-in at rest**, for another program of yours (U) or a later session | The token store of the game's sign-in library is the root of the client's data directory (client source: `XboxLocalStorage.getPath` → `PathHelper::getPrimaryDataDirectory()`), so each session's data directory is per-session scratch: `bedrock::policy::SessionDir`, `$XDG_CACHE_HOME/bastion-mc/bedrock-sessions/<instance>-<16 hex>`, 0700, locked by its launch, removed at the end of the session (the careful way, bounded). What a killed or crashed session left is removed at the next Bedrock start (`remove_leftovers`, which leaves locked directories alone, and refuses a directory of sessions that is a link or open to others) and by `scram`; `doctor` warns while one is there. Worlds and the game's settings (`games/com.mojang`) persist | `bedrock::policy::tests::*` (9, and the child half of one), `tests/scram.rs` (4 Bedrock tests) | Where the real game writes its tokens was read from the client's source, not observed: if the game also writes a token into `games/com.mojang`, it persists with the worlds. During a session, and after a crash until the next start or scram, the token store is a file of yours (readable by U, like any program's tokens). Deleting is not a secure wipe |
| B7 | **A Java credential in a Bedrock session** | None exists: the launch skips the Java sign-in; the sandbox has no path to the token store or vault and no keyring (the system-call filter denies `keyctl`); the relay denies `api.minecraftservices.com`, `sessionserver.mojang.com`, `textures.minecraft.net` | `bedrock::net::tests::the_java_services_are_denied_and_never_reached_by_a_suffix`; the launch package's tests (re-check) | If the game can turn its Xbox identity into a Java token (not verified), it cannot use it from the sandbox; a copy taken out (B5) can be used elsewhere |
| B8 | **Sign-in pages**, used to phish, or to bring a browser into the sandbox | The client's sign-in window is replaced by the launcher's stub at every path where the client looks for one; the launcher accepts only `https://login.live.com/oauth20_authorize.srf` for client `0000000048183522` with its own redirect, shows the page outside the game, takes the answer only from the terminal or the front end, and returns only a final address of that redirect's form; no browser engine runs in the game's sandbox (namespace creation is denied there) | the sign-in package's tests (re-check) | You still have to look at what you sign in to. The code in the final address goes to the game — that is the point; with PKCE it is useless without the verifier the game holds |
| B9 | **Realms and event-server joins** steered by code in the game | The join answers are read by the launcher (TLS terminated with a per-launch CA that only this sandbox's trust store contains), rewritten to one-time pass addresses and approved outside the game (`[realms] approve`; `off` refuses); NetherNet answers refused | the network package's tests (re-check) | As J22: an approval is as good as your reading of the prompt. For the session, the game's trust store trusts the per-launch CA |
| B10 | **A Bedrock session running when you scram** | Marked as a Bedrock launch (`running::Marker::bedrock`, `bedrock:<instance>`), found by pid and start time, stopped through a pidfd like a Java launch; its game dies with it. Then — after the launcher's own credentials and after the last round of stopping — what Bedrock sessions left on disk is deleted for a bounded time and the deletion confirmed by reading the directory again. A session that cannot be stopped, or runs without a marker, is a `COULD NOT` line that says to end it and to sign out everywhere; its directory is not deleted under it; "no launch was in progress" is not printed next to it. Worlds and imported versions are kept, and the output says so | `running::tests::a_bedrock_session_is_marked_as_a_launch_of_its_own_kind_and_stopped_like_one`, `tests/scram.rs::a_bedrock_session_is_stopped_and_its_sign_in_on_disk_goes_with_the_credentials`, `…::a_crashed_sessions_sign_in_is_removed_and_a_running_session_without_marker_is_reported`, `…::a_bedrock_session_that_cannot_be_stopped_says_to_sign_out_everywhere`, `…::an_unreadable_directory_of_bedrock_sessions_is_not_taken_for_an_empty_one` | Revokes nothing at Microsoft. A launcher older than this one takes a `bedrock:` marker for a stale one (one version at a time is supported). `mcpelauncher-ui`'s own data — its Google sign-in, and the game's sign-in from sessions started there — is not touched |
| B11 | **Noticing it afterwards** | After a signed-in session the launcher says where a new cape shows and what the canary will do; the canary never alarms on a gained cape, names it at the next Java sign-in and pins it; every other change is a difference as before (A1); the runbook's "sign out everywhere" ends the game's sign-in and the launcher's alike | `bedrock::policy::tests::after_a_signed_in_session_the_user_learns_where_the_cape_shows_and_what_the_canary_does`, `launch::canary_tests::a_gained_cape_is_named_pinned_and_never_stops_a_launch`, `auth::canary::tests::first_sight_then_unchanged_then_each_kind_of_change` | The canary sees the Java profile only, and only at the next Java sign-in: nothing watches the Bedrock side (Minecoins, Realms, character, Xbox friends) |
| B12 | **Account enforcement, and the Google credential** | Not the launcher's to defend. The guide advises a Google account of its own for the purchase, and signing out of `mcpelauncher-ui` once versions are downloaded | — | Whether Google or Microsoft act against accounts used with unofficial clients is unknown. `mcpelauncher-ui` keeps a Google sign-in on disk, encrypted with a key kept in the same settings unless its sign-in window supplies one (read in its source; that window's source was not available), outside bastion-mc and `scram` |
| B13 | **The client's legacy Microsoft sign-in path**: `mcpelauncher-client` starts `msa-daemon` when it finds one (game versions before 1.16.1x, and its telemetry upload's authentication) | Landlock lets the sandbox execute only the client, its libraries, the loader and the stub: a daemon installed on the host is not started inside | the launch package (re-check: an `exec` of `/usr/bin/msa-daemon` from the sandbox is refused) | Not tested here |

### 2.5 "Not defended" (§5), items

> 17. **Bedrock: the game holds your Xbox identity** for the whole session, and any allowed
>     connection can carry a copy out; only "sign out everywhere" ends a copy (B5).
> 18. **Bedrock: the licence check is answered by `mcpelauncher-client`**, and bastion-mc cannot
>     tell a bought version from one that was not (B1).
> 19. **Bedrock: what the launcher cannot see.** The Bedrock side of the account — Minecoins,
>     Realms, the character — and `mcpelauncher-ui`'s Google sign-in (B11, B12).

### 2.6 Deviations (§6), a row

> | Research §5.2 item 5: "a newly owned cape *will* trip the canary, and `canary accept` records
> it" | a gained cape is never a difference (as before this round); it is named at the next Java
> sign-in and pinned | Gaining a cape is not tampering, and an alarm at every event would train
> users to accept alarms. The note names the cape so that its arrival is visible |

### 2.7 Experiments (§7), rows

> | E33 | `bastion-mc doctor` in isolated directories (2026-11-14) with
> `[bedrock] accept_licence_substitution = true`, a stand-in imported version
> (`bedrock/versions/1.26.62.1`), a stand-in leftover session directory, no client installed |
> The Bedrock section: the gate accepted (info); `FAIL mcpelauncher-client: … not built yet` (the
> base placeholder of discovery); `ok imported versions: 1.26.62.1`; `WARN the game's sign-in: 1
> director(ies) of Bedrock sessions that did not end properly …`; the filter, telemetry, worlds,
> Realms and servers lines. The leftover directory was still there afterwards (doctor reports,
> it does not delete) |
> | E34 | `scram` with stand-in Bedrock sessions (`tests/scram.rs`, 2026-11-14): a test binary
> holding a `bedrock:` marker and a locked session directory with a stand-in token file; one
> without a marker; one whose marker names a thread (cannot be stopped); a crashed session's
> directory | Killed by SIGKILL and its directory deleted after the vault and before the Java
> leftovers (order checked in the output); the unmarked one reported with "one that left no
> marker" and "sign out everywhere", its directory kept until it was gone, then deleted by a
> second run; the unstoppable one reported once as such, with "sign out everywhere"; worlds
> under `instances/` untouched |

### 2.8 The runbook: "sign out everywhere" (for `docs/SCRAM_RUNBOOK.md`)

The page is **already in the runbook and in `crates/proto` `runbook::LINKS`**:
`https://account.live.com/proofs/manage/additional` ("Manage how I sign in": *Sign out
everywhere*, recovery code, sign-in methods) and Microsoft's article
`https://support.microsoft.com/en-us/accounts-billing/manage/how-to-sign-out-of-your-microsoft-account-everywhere`,
both checked on 2026-11-13 by the round that wrote the runbook. **No change to `LINKS` is
needed or proposed**; the step needs Bedrock sentences only. Proposed text (the runbook's test
requires the text and `LINKS` to name the same addresses; these additions name none):

Phase 1, after "…and forgets which player each local account was.":

> If you played Bedrock through bastion-mc, it also stops a running Bedrock session — its game
> holds your Xbox sign-in — and deletes what killed or crashed Bedrock sessions left on disk.
> It keeps Bedrock worlds.

"What it does **not** do", a bullet:

> - A Bedrock game held your Xbox sign-in itself — a refresh token of your Microsoft account —
>   in every session you signed in. If a Bedrock session may have been hostile (a strange
>   server, an add-on, a crash you cannot explain), assume that it copied that sign-in: only
>   signing out everywhere (phase 2) ends it.

Phase 2, step 2, a sentence in the *Sign out everywhere* bullet:

> This also ends the sign-in a Bedrock game held — in bastion-mc's sessions, or in
> mcpelauncher-ui's — and that of every other app.

Phase 3, after the Minecraft-side paragraph:

> **Minecoins spent, or a Realm changed, that you did not do**: Minecraft Support, with the same
> form.

"Afterwards", a bullet:

> - Bedrock: the next session signs in again, as every session does. Look at your Minecoins,
>   your Realms and your Bedrock character in the game: the profile canary watches the Java
>   profile only.

---

## 3. What was verified by running, against what

All on this machine (Arch Linux, kernel 7.2.14), 2026-11-14, in the clone, with the four XDG
variables and `TMPDIR` pointed under `target/`, `DBUS_SESSION_BUS_ADDRESS` unset.

| Claim | How |
|---|---|
| The gate refuses without the switch and names what is accepted | `bedrock::policy::tests::the_gate_names_what_is_accepted_and_what_bastion_does_not_do`, the base's `bedrock::tests::*` |
| A session directory is new, 0700, locked while its value lives, never reused, removed with what was written into it (also a subdirectory without permission bits), and removed on drop | `bedrock::policy::tests::a_session_directory_is_new_private_locked_and_removed_with_what_the_game_wrote`, `…_is_removed_on_drop_when_the_launch_ends_some_other_way` |
| Leftovers of killed sessions are removed, a directory locked by a live process is not (a child process held the lock), a link is removed as a link, a file name with an escape sequence is shown sanitised, the lock ends with its process | `…::leftovers_of_killed_sessions_are_removed_and_running_ones_are_not` |
| What cannot be removed in time, or at all, is reported and not counted; an unreadable directory is an error, not "nothing there" | `…::what_cannot_be_removed_in_time_or_at_all_is_reported_not_counted` |
| A directory of sessions that is a link (to a directory with files in it) or writable by others is refused, and nothing is deleted, also not where the link points | `…::a_directory_of_sessions_that_is_a_link_or_open_to_others_is_refused_and_nothing_is_deleted` |
| `doctor`'s Bedrock section: four info lines with Bedrock off, nine lines with it on (the client `FAIL`, filter `log` a `WARN`), versions listed and counted, a leftover's warning also with Bedrock off, nothing deleted | `…::doctor_says_little_while_bedrock_is_off_and_everything_once_it_is_on`, `…::doctor_lists_imported_versions_and_warns_about_leftover_sign_ins`; the real binary in isolated directories on display `:77` with `XAUTHORITY=/dev/null` (E33; sound, cgroup and the nested compositor off in that configuration) |
| The lines after a session: the cape commands, the canary's behaviour, nothing without a sign-in; through a capturing reporter | `…::after_a_signed_in_session_the_user_learns_where_the_cape_shows_and_what_the_canary_does` |
| A Bedrock marker is written, read and refused when malformed; it holds up the Java leftover sweep; a process with it is killed and confirmed | `running::tests::a_bedrock_session_is_marked_as_a_launch_of_its_own_kind_and_stopped_like_one` |
| Gained capes are named until pinned; the launch's verdict names one with the command that wears it, is routine, pins it, and a rename next to a gained cape is still refused | `auth::canary::tests::first_sight_then_unchanged_then_each_kind_of_change`, `launch::canary_tests::a_gained_cape_is_named_pinned_and_never_stops_a_launch` |
| scram with Bedrock sessions (E34) and without Bedrock (nothing said about it); "no launch was in progress" is not printed next to an unmarked Java launch either | `tests/scram.rs` (15 tests, 4 of them new, 2 with an added assertion) |
| The scram confirmation names the Bedrock leftovers and that worlds are kept; a wrong answer changes nothing | the real binary in isolated directories: `echo no \| bastion-mc scram` → the text, "cancelled; nothing was changed", exit 1; `tests/scram.rs::the_command_asks_first_and_a_wrong_answer_changes_nothing` |
| Facts from the projects' sources (read only, nothing built or run there) | `mcpe-mcpelauncher-client`: `src/jni/xbox_live.cpp` (the token store's path), `src/jni/store.cpp` (the licence answer; every purchase fails), `src/main.cpp` (`--free-only`; mods from `<data>/mods` and `--mods`), `src/xbox_live_helper.cpp` (`msa-daemon`); `mcpe-mcpelauncher-ui-qt`: `googleplayapi.cpp` (`validateLicense`), `gamelauncher.cpp` (`--free-only` when not verified), `googleloginhelper.cpp` (the Google sign-in in its settings) |
| The AUR packages | the AUR's RPC interface and the two PKGBUILDs (`curl -A curl`, read only): `mcpelauncher-linux` and `mcpelauncher-ui` 1.8.7-1, `arch=('x86_64')`, source builds, `mcpelauncher-linux` depends on `qt6-webengine` (its sign-in window), `mcpelauncher-ui` on `mcpelauncher-linux` |

The full suite and the checks of §4 of the progress file ran on the finished branch (see the
final run recorded in `bedrock-risk-progress.md`).

**Not verified, and why:**

* Anything with the real `mcpelauncher-client` or game: neither exists here, and installing
  them is not allowed (and needs a Google Play purchase).
* Where the real game writes its tokens (read in the client's source only), and that nothing of
  the sign-in ends up in `games/com.mojang`.
* That the launch package uses `SessionDir` and `Marker::bedrock`, and calls `remove_leftovers`
  and `report_after_session`: their code was not visible to this package (§4).
* `doctor`'s client line against a real discovery: the base's placeholder always fails.
* A cape claimed in Bedrock reaching the Java profile; bastion-mc's Java sign-in (never live).
* Microsoft's sign-out pages were not fetched again this round (rule: no Microsoft services
  from here; the runbook round checked them on 2026-11-13).
* The behaviour of browsers at the end of the Microsoft sign-in (§5, item 7).

---

## 4. What was built, entry points, integration

Files: `src/bedrock/policy.rs` (mine), `src/running.rs`, `src/scram.rs`, `src/auth/canary.rs`
(mine this round); one block each in the shared `src/doctor.rs`, `src/launch.rs`
(`canary_verdict`'s `Unchanged` arm) and `src/cli.rs` (the scram confirmation string);
`tests/scram.rs`; `docs/BEDROCK.md`; these notes. No new dependency, no new configuration key,
no new event, no change to `crates/proto` or `gui/`.

**Entry points** (all in `bastion_mc::bedrock::policy` unless named otherwise):

```rust
pub fn gate(cfg: &BedrockConfig) -> Result<()>;                       // first thing of a Bedrock launch
pub fn sessions_dir(paths: &Paths) -> PathBuf;                         // $XDG_CACHE_HOME/bastion-mc/bedrock-sessions
pub struct SessionDir;                                                 // one session's data directory (the client's -dd)
impl SessionDir {
    pub fn create(paths: &Paths, instance: &str) -> Result<SessionDir>; // <instance>-<16 hex>, 0700, locked
    pub fn path(&self) -> &Path;
    pub fn remove(&mut self) -> std::result::Result<(), String>;        // bounded (30 s); then the lock goes; also on drop
}
pub fn remove_leftovers(paths: &Paths, deadline: Option<Instant>) -> std::result::Result<Leftovers, String>;
pub struct Leftovers { pub removed: Vec<String>, pub failed: Vec<String>, pub in_use: Vec<String> }
pub fn set_up(paths: &Paths) -> bool;                                  // scram's "was Bedrock used here"
pub fn doctor(paths: &Paths, cfg: &Config) -> Vec<Line>;               // Line { level: Level, what, detail }
pub fn after_session(signed_in: bool) -> Vec<String>;
pub fn report_after_session(reporter: &Reporter, signed_in: bool);
// running.rs
pub enum Activity { Launch(String), Bedrock(String), Command(String) }  // + is_launch()
impl Marker { pub fn bedrock(paths: &Paths, instance: &str) -> Result<Marker> }
// auth/canary.rs
pub fn gained(paths: &Paths, account: &str, profile: &Profile) -> Result<Vec<String>>;
impl Snapshot { pub fn gained(&self, now: &Snapshot) -> Vec<String> }
```

**Wiring the integrator must do in the launch package's Bedrock launch** (its code was not
visible here):

1. `bedrock::policy::gate(&cfg.bedrock)?` before anything else.
2. `let _running = running::Marker::bedrock(paths, &inst.id)?;` instead of `Marker::create`
   (if the dispatch comes after `launch()`'s own `Marker::create`, replace that call for
   Bedrock instances; one marker per process — a second `Marker` for the same pid overwrites the
   first file and the first one's drop removes it).
3. Before the session: `bedrock::policy::remove_leftovers(paths, Some(now + 20 s))`, reporting
   `failed` (a warning, as `launch()` does for the Java sweep). Locked directories are other
   running sessions: leave them.
4. The data directory: `let mut data = bedrock::policy::SessionDir::create(paths, &inst.id)?;`
   after the marker; mount `data.path()` at `/bastion/data`, and the instance's persistent
   `games/com.mojang` (worlds and settings) inside it when `persist_worlds`. If the launch
   package chose another place for the per-session directory, either switch to this, or point
   `sessions_dir` at that place (scram, doctor and the leftover removal all go through it).
5. At the end: `data.remove()`. After step 9 that needs the Bedrock launch's `Confinement` to
   list the cache as writable, as the Java launch's does (`keep.write.push(paths.cache.clone())`
   in `launch.rs`), or at least `sessions_dir(paths)`; otherwise the removal fails and the
   directory waits for the next start or `scram`. Report an `Err` from `data.remove()` as a
   warning that names the directory and says that the next start or `scram` removes it.
   Then `bedrock::policy::report_after_session(reporter, signed_in)`, where `signed_in` comes
   from the sign-in package's bridge server (whether a sign-in completed in this session), or
   `true` if it cannot tell.
6. `doctor`: `client_line` calls `runtime::discover(&cfg.bedrock)` (the store package's, in
   place). `imported_versions` is a placeholder that only looks at names: switch it to the
   store package's listing (which checks manifests), e.g. `bedrock::store::list(paths)`.

**Expected merge conflicts:** `src/launch.rs` (the `Unchanged` arm of `canary_verdict`, far from
the dispatch block), `src/doctor.rs` (one block before "trust"), `src/cli.rs` (the scram
confirmation string), `src/running.rs` (if another package touched it), `tests/scram.rs` (a role
block and four tests appended).

---

## 5. Statements to re-check against the other packages' final behaviour

In `docs/BEDROCK.md` unless named otherwise; the package that decides is in brackets.

1. The commands `bastion-mc bedrock import <dir>`, `bedrock versions`, `bedrock remove <id>`:
   names, arguments, what they print [store].
2. `mcpelauncher-ui`'s version directories: `~/.local/share/mcpelauncher/versions/<version>`,
   Flatpak `~/.var/app/io.mrarm.mcpelauncher/data/mcpelauncher/versions/` [store].
3. What import refuses (links, hard links, devices, escapes, oversize; never an APK), that each
   launch checks the version again from the directory it binds [store, launch]; whether a
   signer of the game files is checked (threat row B3) [store].
4. `instance new <name> --bedrock <version-id>`, the `instance.toml` shape, and that `install`
   and `profile …` refuse a Bedrock instance with a sentence [launch].
5. The sandbox as described under "Playing" (execute: client, libraries, loader, stub; no mods
   directory; display; sound "as for Java"; no `/dev/input`) and threat row B4 [launch].
6. "When the session ends, the game's data directory is deleted"; worlds and settings
   (`games/com.mojang`) kept in the instance; `persist_worlds = false` deletes them with each
   session; and scram's closing line "in each Bedrock instance under …/instances" [launch].
7. The sign-in: the front end opens the page in your browser; the terminal prints the address;
   the optional window of bastion-mc's sandboxed browser; where to paste (terminal prompt,
   front end prompt); the final address `ms-xal-0000000048183522://auth?code=…`. **The browser
   behaviour is unverified**: "it says it cannot open it" and "copy the whole address from the
   address bar" hold for Firefox as far as known here; a Chromium-based browser may show
   nothing at all when a redirect goes to an unhandled scheme, and the user then cannot copy
   the address without developer tools. The sign-in package's own instructions must decide this
   paragraph [sign-in].
8. "A pasted answer is accepted only from the terminal of a terminal launch or from the front
   end" [sign-in].
9. Servers: `[bedrock] servers`, an instance's own `servers`, the default port, and "when the
   game tries a server that is not listed, the launcher says so once, with the name and port"
   [network].
10. LAN games and friends' worlds (NetherNet) refused "with a message" [network].
11. Realms and event servers: the approval flow for Bedrock (`"code"`: typed at the launcher's
    prompt or in the front end; `"totp"`), NetherNet Realms refused with a message, event
    servers handled like Realms [network, sign-in]. The Java Realms section of the README says
    approvals are typed into the phone; Bedrock has no phone.
12. Featured servers (the game's built-in list) are not mentioned in the guide: say how they
    behave once known [network].
13. Telemetry: what `telemetry = false` blocks (the base's `TELEMETRY_*` lists) [network].
14. `doctor`'s client line text against the real `Client` (lib dirs, webview paths) [store].
15. The after-session lines are shown at the end of every signed-in Bedrock session, as Status
    events in the front end [launch, sign-in].
16. Threat rows B2, B3, B4, B7, B8, B9, B13 name other packages' tests: fill in their names.
17. The "Verified" table in the guide: fill in what the other packages ran.

---

## 6. Where the contract (or the research) did not fit, and what I did

1. **The token store is the data directory itself.** The contract has "the token store as a
   per-session directory inside" `/bastion/data` and asks to "find it in the client's source and
   name it". The client's source answers the game's question for its local storage with the
   root of the data directory (`src/jni/xbox_live.cpp`). The file names are the game's own and
   unknown here. So I treat the whole per-session data directory as the token store and assume
   only `games/com.mojang` persists. If the launch package found a narrower place, scram and
   doctor still work as long as the per-session directories live under `sessions_dir`.
2. **No host location was given for per-session directories.** I defined
   `bedrock::policy::sessions_dir` (`$XDG_CACHE_HOME/bastion-mc/bedrock-sessions/`) and
   `SessionDir`. Not under `scratch/`: scram sweeps that last, bounded, and with the message
   "not the launcher's credentials", which would be false for a Bedrock directory.
3. **"A running Bedrock session handled as a Java one."** It is stopped exactly like one, but
   with a marker of its own kind (`bedrock:<instance>`, `Activity::Bedrock`), because what scram
   must say differs: an unstoppable Bedrock session means "sign out everywhere", not "a session
   token for up to 24 hours". `Activity::is_launch()` keeps the Java sweep from running under a
   Bedrock session (it has nested-compositor directories in the cache too).
4. **Scram's order for Bedrock.** The leftovers are removed after step 3 (the second stop pass),
   not inside step 2: removing them in step 2 made a session that started during scram and was
   stopped in the second pass leave a stale `COULD NOT` from the first. And "no launch was in
   progress" was printed next to a `COULD NOT` about an unmarked launch — for Java too, before
   this round; it is now dropped when an unmarked launch (Java or Bedrock) is found.
5. **The research's canary premise was wrong.** Research §5.2 item 5 says a newly owned cape
   trips the canary and `canary accept` records it. The canary has never treated a gained cape
   as a difference (`Snapshot::differences`), and `canary_verdict` pins gained capes on an
   unchanged profile. So the after-session prompt says a new cape needs nothing, and names what
   does; and I added a note that names gained capes (a small change in `launch.rs`'s
   `canary_verdict`, a shared file).
6. **There is no `cape list` command** (the task names one). `skin show` lists the capes; the
   guide and the prompt use it.
7. **Worlds in scram.** Decided: kept, and said, in the output and the confirmation prompt.
   "Unless asked" is the user deleting the instance; no scram option was added (§7).
8. **The runbook link** is already in `runbook::LINKS`; no change proposed (§2.8).

---

## 7. Open items

* **The real game**: where it writes its tokens, whether anything of the sign-in persists in
  `games/com.mojang`, whether a session that did not use the bridge is signed in all the same
  (that would mean the split failed). Only the user can run it.
* **A tripwire for a failed split (proposal, not built):** the relay sees the names the game
  contacts. A session whose game reaches `xsts.auth.xboxlive.com` or `sisu.xboxlive.com`
  without a sign-in through the bridge in that session had a token from somewhere: the launch
  could say so at the end. Needs the network and sign-in packages.
* **`scram --bedrock-data`** (proposal, not built): delete Bedrock worlds and settings too, for
  a user who suspects the game kept a sign-in there. Needs the launch package's layout.
* **`doctor`'s versions line** is a placeholder (names only) until the store package's listing.
* **The README, THREAT_MODEL and runbook texts** above wait for the website refresh to finish;
  the integrator folds them in.
* **The browser at the end of the sign-in** (§5, item 7): decide the paste instructions with the
  sign-in package, or prefer its sandboxed-browser window, which catches the redirect itself.
