# Emergency runbook: you think the account is compromised

This text is built into the launcher (`bastion-mc wiki scram`) so that you never have to
search the web for these pages while under pressure: search results for "minecraft account
hacked" are where the phishing pages are. Every link below was taken from Microsoft's own
help articles on the date given at the end, and the launcher will open these exact
addresses and no others.

## Read this first: which device are you on?

**If this computer itself might be compromised** (a mod or program you no longer trust ran
outside the sandbox, the browser behaves oddly, you typed your password into something):
stop here and do phases 2 and 3 **from a different device** — a phone on mobile data is
fine. A compromised machine can show you a fake browser page, and can show you a fake
version of this text. Run phase 1 on this machine anyway; it only removes things.

If only the *account* looks wrong (the profile canary fired, an e-mail about a sign-in you
did not make) and this machine is not in doubt, you can continue here.

## Phase 1 — this machine (one minute)

    bastion-mc scram

What it does: first it stops whatever could still store a sign-in — a running game and
its launcher (the session token is gone with them), a sign-in that is still waiting for
you, a skin, cape, canary or vault command, a Spotify sign-in or token keeper. Then it
removes every stored refresh token from the kernel keyring — those of your Minecraft
accounts and the Spotify token of the in-game phone — and the secret for approving Realms
joins; deletes the age vault files; deletes the in-game browser's profile, which holds the
sessions of the sites you signed in to there (a Discord session, for example); and forgets
which player each local account was. Last, and for at most 15 seconds, it deletes what
stopped games left behind. It prints what it did.

Read what it prints. A line that starts with `COULD NOT` means that something is still
there or could not be checked, and it says what to do — for example end a process
yourself, or run the command in another login session of yours that still holds a key
(or log out of it). Do that, then run `bastion-mc scram` again. One such line you may meet:
the browser's profile is not deleted while a `bastion-mc browser login` window still uses
it; close that window first. (The graphical front end runs the same command from its
Security page, and shows such a result as SCRAM INCOMPLETE.)

What it does **not** do:

- It revokes nothing at Microsoft. A refresh token that was *copied* before you ran this
  still works until you sign out everywhere (phase 2).
- A Minecraft session token that was already issued stays valid at Mojang until it expires
  (about 24 hours). Nobody can shorten that; with this launcher's credential broker the
  token never reached the game, which is the reason the broker exists.
- It does not "securely wipe" files. On SSDs and on copy-on-write file systems (btrfs)
  overwriting a file does not guarantee the old blocks are gone. What protects a deleted
  vault file is that it was encrypted and that the token in it gets revoked in phase 2.
- It signs you out of no website. A site the in-game browser was signed in to keeps that
  session on its side until you end it there — on Discord under Settings, Devices. Unlike a
  vault file, the browser's profile is not encrypted: if this machine may be compromised,
  end those sessions from a clean device. A Spotify app you allowed keeps its access until
  you remove it under "Manage apps" in your Spotify account.
- The profile canary's history is kept: it is evidence for phase 3, and not a credential.

## Phase 2 — the Microsoft account (ten minutes, from a clean device)

Do these in order. Sign in only by typing the address yourself or by using the links here.

1. **Change the password.**
   https://go.microsoft.com/fwlink/p/?linkid=2071806
   If you cannot sign in any more, reset it instead:
   https://go.microsoft.com/fwlink/p/?LinkID=238656

2. **Sign out everywhere, and make a new recovery code.** Both are on the "Manage how I
   sign in" page:
   https://account.live.com/proofs/manage/additional
   - *Sign out everywhere*: Microsoft says you are signed out of browsers and apps "within
     24 hours" — it is not instant — and that an Xbox console is **not** covered; sign out
     there on the console itself.
   - *Recovery code → Generate a new code*: a new code makes every earlier code stop
     working. That is the point: the intruder may have generated one. Print the new code;
     do not store it on a device you sign in from.
   - On the same page, look at every sign-in method listed. Remove e-mail addresses, phone
     numbers and authenticator entries you do not recognise. Prefer a passkey or a security
     key over SMS and e-mail codes.

3. **Remove apps you did not authorise.** Fake "verification" or "giveaway" apps that were
   granted access keep that access through a password change:
   https://www.microsoft.com/consent
   Remove everything you do not recognise. (Your own launcher application is listed here
   too, under the name you gave it in Azure. Removing it is harmless: you consent again at
   the next sign-in.)

4. **Look at what happened.**
   https://account.live.com/Activity
   For anything that was not you, choose "This wasn't me" / "Secure your account".

5. Then check the rest of the security overview:
   https://account.microsoft.com/security

Microsoft's own description of these steps:
https://support.microsoft.com/en-us/accounts-billing/manage/how-to-recover-a-hacked-or-compromised-microsoft-account
https://support.microsoft.com/en-us/accounts-billing/manage/how-to-sign-out-of-your-microsoft-account-everywhere
https://support.microsoft.com/en-us/accounts-billing/manage/how-to-get-a-microsoft-account-recovery-code

## Phase 3 — if you are locked out, or the Minecraft profile was changed

**Locked out of the Microsoft account** (password and security info were replaced): the
account recovery form.
https://go.microsoft.com/fwlink/?linkid=2288285
Microsoft's advice for it: fill it in from a device and a place you have used the account
from before; answer as much as you can (old passwords, exact e-mail subject lines, names
of contacts); guessing does not count against you. You need a *different*, working e-mail
address for the answer, which comes within 24 hours. You may try twice a day.
https://support.microsoft.com/en-us/accounts-billing/manage/help-with-the-microsoft-account-recovery-form

**The Minecraft side** (player name, skin or cape changed; account moved to someone else):
Minecraft Support can only help once you can show the account is yours.
https://help.minecraft.net/hc/en-us/requests/new
Have ready:

- the transaction ID from the purchase e-mail (or the gift/redeem code and where it was bought),
- the e-mail address the game was bought with, and roughly when,
- the player names the account has had, and its capes — `bastion-mc export-proofs FILE`
  writes what this launcher recorded: the player id, every name, skin and cape state the
  canary has seen with dates, and when you signed in and played. It contains no token.

## Afterwards

- Sign in to the launcher again (`bastion-mc login`) and check `bastion-mc skin show`.
- `bastion-mc canary accept` pins the restored profile.
- If you used the in-game phone's services: sign in to Spotify again with
  `bastion-mc spotify login`, make a new Realms secret with `bastion-mc realms totp-setup`
  and delete the old "bastion-mc:Realms" entry from your authenticator app, and sign in to
  each site again with `bastion-mc browser login <site>` once its old sessions are ended.
- If the refresh-token store is the kernel keyring, consider the age vault with a hardware
  key (README, "Where the refresh token rests").
- Find out how it happened before you play again: a password reused elsewhere, a
  "verification" link, or a program run outside the sandbox.

---

Links checked on 2026-11-13. Microsoft links: taken from the Microsoft Support articles
listed above ("How to recover a hacked or compromised Microsoft account", "How to sign out
of your Microsoft account everywhere", "How to get a Microsoft account recovery code",
"Help with the Microsoft account recovery form", "What is the Recent activity page?"),
and the consent page from Microsoft's "Sync Microsoft To Do with the Samsung reminder app"
article. The statements about timing ("within 24 hours", Xbox consoles excluded, earlier
recovery codes stop working, answer within 24 hours, two attempts a day) are from the same
articles. Minecraft's help centre could not be read by a program (it needs a browser), so
the support request address is the help centre's standard request form and was only
checked to exist; its wording was not verified.
