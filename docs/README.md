# Documentation index

Everything under `docs/`, one line each: what it is for, and who should read it. The front
page — what bastion-mc is, what was verified and what was not — is the
[README](../README.md); what is open, blocked or decided is [TODO.md](../TODO.md).
bastion-mc is pre-release.

## For people who use it

- [THREAT_MODEL.md](THREAT_MODEL.md) — what is defended against whom, the test behind each claim, what is **not** defended (§5) and the experiments behind the claims (§7); for everyone, before relying on the launcher.
- [TOKEN_INJECTION.md](TOKEN_INJECTION.md) — how the access token (or, with the credential broker on, a placeholder) reaches the game without appearing on a command line, in the environment or in a file, and exactly where that guarantee ends; for readers who want the token's path in detail.
- [MC_VAULT_ACCOUNT.md](MC_VAULT_ACCOUNT.md) — keeping other programs of yours away from the game and the token (`ptrace_scope=3`, a dedicated UNIX account, a root-owned list of vault recipients); for anyone who runs software they do not fully trust under the same account.
- [SCRAM_RUNBOOK.md](SCRAM_RUNBOOK.md) — the emergency runbook for a compromised account, the same text `bastion-mc wiki scram` prints; read it once before you need it.
- [MODS.md](MODS.md) — signed profiles, `profile add` and `profile check` against Modrinth, the aliases, ViaFabric, Simple Voice Chat and the server rule that lets its UDP through; for anyone who plays with mods.
- [COMPANION.md](COMPANION.md) — the launcher's own Fabric mod (zoom, HUD read-out, fullbright): settings, when it is added, its channel to the launcher, how it is built, what was verified; for Fabric players and for anyone reviewing what the launcher adds to the game.

## Engineering notes (`notes/`)

Each work package of the review round and of the third round left a notes file — its
deliverable: user documentation, threat-model rows, what was verified and how, what is
open — and a hand-over log (`*-progress.md`) with the state of the work, the runs and what
was learned. They describe each branch as it was handed in; where a note and the code
disagree, the code is right. For maintainers and reviewers.

- [notes/review-findings.md](notes/review-findings.md) — the 27 findings of the two independent security reviews of 2026-11-13.
- [notes/review-fixes.md](notes/review-fixes.md) — finding by finding: how it was verified, what changed, which test covers it, or why nothing changed.
- [notes/review-fixes-progress.md](notes/review-fixes-progress.md) — hand-over log of the review fixes, with the final verification of that round.
- [notes/gui.md](notes/gui.md) — the "workshop HUD" front end: its pages, design system and layout rules, the front end's halves of findings B-M2, B-M1 and B-L2, threat-model rows, tests and snapshots, what is open.
- [notes/gui-progress.md](notes/gui-progress.md) — hand-over log of the front end, with the live test and the real-window runs.
- [notes/companion-mod.md](notes/companion-mod.md) — the companion mod: user text, threat-model rows, the build and the injection, what was verified in the real game and what was not, the design for approval prompts (not built).
- [notes/companion-mod-progress.md](notes/companion-mod-progress.md) — hand-over log of the companion mod, with every real-game run.
- [notes/mod-catalog.md](notes/mod-catalog.md) — `profile add` and `profile check`: the verification model and where it ends, what Modrinth learns, the aliases, real launches with the mods it fetched, threat-model rows, what is open.
- [notes/mod-catalog-progress.md](notes/mod-catalog-progress.md) — hand-over log of the mod catalog, with the review that followed the take-over.
- [notes/voice-chat.md](notes/voice-chat.md) — the UDP relay for voice chat and the fix of finding B-M3 (`launcher_proxy`): rule syntax, limits, threat-model rows, experiments, what is open.
- [notes/voice-chat-progress.md](notes/voice-chat-progress.md) — hand-over log of the voice-chat package, including the flaky tests it ran into and their causes.
- [notes/docs-pass.md](notes/docs-pass.md) — the two reviews of these documents (against the code, and by a newcomer following the README), finding by finding with what was done, and the code problems found while writing them.

## Images (`images/`)

Three of the front end's own test snapshots: rendered in software from test states (the
names, paths and protection levels in them are test data), from the code that was merged.
The README links to them; the website's converter does not take images in the README.

- [images/gui-launch-running.png](images/gui-launch-running.png) — the Launch page while a game runs:

  ![The Launch page of bastion-mc-gui: on the left a navigation rail with five pages and the account in use; in the middle a ring gauge reading RUNNING 2:31 with a Stop button below it; beside it the instance and the five stages of the launch; on the right the protection read-out, with display, network and launcher in force, credential reduced and limits not protected.](images/gui-launch-running.png)

- [images/gui-accounts.png](images/gui-accounts.png) — the Accounts page:

  ![The Accounts page of bastion-mc-gui: three account cards side by side, each with its label, sign-in state and where its token rests (kernel keyring or age vault), with Sign in, Device code, Sign out, Switch to and Make default buttons, and a tile for adding a new account label.](images/gui-accounts.png)

- [images/gui-security-scram-incomplete.png](images/gui-security-scram-incomplete.png) — the Security page after a scram the launcher reported as incomplete:

  ![The Security page of bastion-mc-gui after an incomplete scram: a red banner that begins SCRAM INCOMPLETE, the emergency panel with a red block listing the two lines that start with COULD NOT, and below it the launcher's full report with what was removed and what could not be.](images/gui-security-scram-incomplete.png)
