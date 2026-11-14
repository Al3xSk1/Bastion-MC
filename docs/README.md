# Documentation index

Everything under `docs/`, one line each: what it is for, and who should read it. The front
page — what bastion-mc is, what was verified and what was not — is the
[README](../README.md); what is open, blocked or decided is [TODO.md](../TODO.md).
bastion-mc is pre-release.

## For people who use it

- [THREAT_MODEL.md](THREAT_MODEL.md) — what is defended against whom, the test behind each claim, what is **not** defended (§5) and the experiments behind the claims (§7); for everyone, before relying on the launcher.
- [TOKEN_INJECTION.md](TOKEN_INJECTION.md) — how the access token (or, with the credential broker on, a placeholder) reaches the game without appearing on a command line, in the environment or in a file, and exactly where that guarantee ends; for readers who want the token's path in detail.
- [MC_VAULT_ACCOUNT.md](MC_VAULT_ACCOUNT.md) — keeping other programs of yours away from the game and the token (`ptrace_scope=3`, a dedicated UNIX account, a root-owned list of vault recipients); for anyone who runs software they do not fully trust under the same account.
- [SCRAM_RUNBOOK.md](SCRAM_RUNBOOK.md) — the emergency runbook for a compromised account: what `bastion-mc scram` removes (also the Spotify token, the Realms TOTP secret and the phone browser's profile) and what it cannot, then the account steps; the same text `bastion-mc wiki scram` prints; read it once before you need it.
- [MODS.md](MODS.md) — signed profiles, `profile add` and `profile check` against Modrinth, the aliases, ViaFabric, Simple Voice Chat and the server rule that lets its UDP through; for anyone who plays with mods.
- [COMPANION.md](COMPANION.md) — the launcher's own Fabric mod (the Minecraft versions it runs on; zoom, HUD read-out, fullbright): settings, when it is added, its channel to the launcher, how it is built, what was verified; for Fabric players and for anyone reviewing what the launcher adds to the game.
- [PHONE.md](PHONE.md) — the in-game phone and its services (Spotify, Discord and the browser, clips, Realms, settings sync): what each app does, its settings and limits, and what the game can see; for anyone who turns one of them on.

## Engineering notes (`notes/`)

Each work package of the review round, the third round and the fourth round left a notes
file — its deliverable: user documentation, threat-model rows, what was verified and how,
what is open — and a hand-over log (`*-progress.md`) with the state of the work, the runs and
what was learned. They describe each branch as it was handed in; where a note and the code
disagree, the code is right. For maintainers and reviewers.

- [notes/review-findings.md](notes/review-findings.md) — the 27 findings of the two independent security reviews of 2026-11-13.
- [notes/review-fixes.md](notes/review-fixes.md) — finding by finding: how it was verified, what changed, which test covers it, or why nothing changed.
- [notes/review-fixes-progress.md](notes/review-fixes-progress.md) — hand-over log of the review fixes, with the final verification of that round.
- [notes/gui.md](notes/gui.md) — the "workshop HUD" front end: its pages, design system and layout rules, the front end's halves of findings B-M2, B-M1 and B-L2, threat-model rows, tests and snapshots, what is open.
- [notes/gui-progress.md](notes/gui-progress.md) — hand-over log of the front end, with the live test and the real-window runs.
- [notes/companion-mod.md](notes/companion-mod.md) — the companion mod as the third round built it (Minecraft 26.3 only): user text, threat-model rows, the build and the injection, what was verified in the real game and what was not, a design for approval prompts that was not built (the fourth round's Realms approval is in notes/realms-sync.md).
- [notes/companion-mod-progress.md](notes/companion-mod-progress.md) — hand-over log of the companion mod, with every real-game run.
- [notes/mod-catalog.md](notes/mod-catalog.md) — `profile add` and `profile check`: the verification model and where it ends, what Modrinth learns, the aliases, real launches with the mods it fetched, threat-model rows, what is open.
- [notes/mod-catalog-progress.md](notes/mod-catalog-progress.md) — hand-over log of the mod catalog, with the review that followed the take-over.
- [notes/voice-chat.md](notes/voice-chat.md) — the UDP relay for voice chat and the fix of finding B-M3 (`launcher_proxy`): rule syntax, limits, threat-model rows, experiments, what is open.
- [notes/voice-chat-progress.md](notes/voice-chat-progress.md) — hand-over log of the voice-chat package, including the flaky tests it ran into and their causes.
- [notes/docs-pass.md](notes/docs-pass.md) — the two reviews of these documents (against the code, and by a newcomer following the README), finding by finding with what was done, and the code problems found while writing them.

The fourth round (the phone and its services) was built by five engineers in parallel against
one contract:

- [notes/round4-contract.md](notes/round4-contract.md) — what was asked for, the rules every package kept (no secret enters the game, the launcher enforces and the mod is interface only, every helper in a sandbox of its own, the defaults), the phone's protocol and each app's vocabulary.
- [notes/mod-platform.md](notes/mod-platform.md) — the companion mod on the ten families of Minecraft versions from 1.20.1 to 26.3 (adapters, stand-ins generated from the published files, the build in passes), protocol 2 of its channel, the dispatcher, the demo service; the real-game rounds on every family.
- [notes/mod-platform-progress.md](notes/mod-platform-progress.md) — hand-over log of the platform, with how to repeat the real-game runs.
- [notes/mod-ui.md](notes/mod-ui.md) — the phone, its apps' screens, the home screen and the dark theme: input rules, layout rules, the stand-in-game harness, the real-game sweeps and what was found in them.
- [notes/mod-ui-progress.md](notes/mod-ui-progress.md) — hand-over log of the phone's interface.
- [notes/r4-mod-requests.md](notes/r4-mod-requests.md) — the requests between the two packages that shared the mod (what the interface needed of the platform, and the answers).
- [notes/browser.md](notes/browser.md) — the phone's browser: Firefox in its sandbox, the relay and the site budgets, the `discord` preset, `browser login`, threat-model rows, what ran against stand-in sites and the internet, what is open.
- [notes/browser-progress.md](notes/browser-progress.md) — hand-over log of the browser, with how to repeat its checks.
- [notes/media.md](notes/media.md) — Spotify (MPRIS through a filtering D-Bus proxy, the Web API, the token keeper) and clips (the recorder in its sandbox, saving, limits): threat-model text, what ran against stand-ins and in the real game, what is open.
- [notes/media-progress.md](notes/media-progress.md) — hand-over log of Spotify and clips.
- [notes/realms-sync.md](notes/realms-sync.md) — Realms through the broker with an approval outside the game and a one-time pass, and settings sync across instances: threat-model rows, decisions, what ran against a stand-in Realms service and in the real game, what is open.
- [notes/realms-sync-progress.md](notes/realms-sync-progress.md) — hand-over log of Realms and settings sync, with what was read from the game's own client classes; [notes/realms-sync-evidence/](notes/realms-sync-evidence/) holds excerpts of the three launch logs of the settings-sync run, the synced copy after it and a screenshot of the game's options.

## Images (`images/`)

Three of the front end's own test snapshots: rendered in software from test states (the
names, paths and protection levels in them are test data), from the code that was merged in
the third round. The README links to them; the website's converter does not take images in
the README.

- [images/gui-launch-running.png](images/gui-launch-running.png) — the Launch page while a game runs:

  ![The Launch page of bastion-mc-gui: on the left a navigation rail with five pages and the account in use; in the middle a ring gauge reading RUNNING 2:31 with a Stop button below it; beside it the instance and the five stages of the launch; on the right the protection read-out, with display, network and launcher in force, credential reduced and limits not protected.](images/gui-launch-running.png)

- [images/gui-accounts.png](images/gui-accounts.png) — the Accounts page:

  ![The Accounts page of bastion-mc-gui: three account cards side by side, each with its label, sign-in state and where its token rests (kernel keyring or age vault), with Sign in, Device code, Sign out, Switch to and Make default buttons, and a tile for adding a new account label.](images/gui-accounts.png)

- [images/gui-security-scram-incomplete.png](images/gui-security-scram-incomplete.png) — the Security page after a scram the launcher reported as incomplete:

  ![The Security page of bastion-mc-gui after an incomplete scram: a red banner that begins SCRAM INCOMPLETE, the emergency panel with a red block listing the two lines that start with COULD NOT, and below it the launcher's full report with what was removed and what could not be.](images/gui-security-scram-incomplete.png)
