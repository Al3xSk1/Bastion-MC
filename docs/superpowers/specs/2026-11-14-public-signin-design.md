# Public sign-in: bastion-mc's own Microsoft app, besides bring-your-own

Status: design, 2026-11-14. Not built.

**Intent.** The user's words: "add a public authentication method in addition to the bring
your own token method". Today every user must register an Azure application and wait for
Mojang's approval before signing in (README "Your own OAuth application": "There is no
shared client ID, by design"). A public method lets people sign in without that.

**Decided with the owner.** The public method is **bastion-mc's own app registration**: the
owner registers one Azure application for bastion-mc, has Mojang approve it for the
Minecraft API, and its client ID ships with bastion-mc. Rejected: reusing Mojang's official
launcher client ID (it would pose as Mojang's launcher and can be blocked at any time).

**Goals.** Sign-in works with no configuration once the public ID is built in; bring-your-own
stays exactly as it is and wins when configured; the user can always see which app a sign-in
goes to; nothing about tokens or their storage changes.

**Non-goals.** Any change to the token chain (MSA → Xbox → XSTS → Minecraft), the token store
or the broker; a client secret (it is a public client); telemetry of any kind.

**Design.**
- A compile-time `PUBLIC_CLIENT_ID: Option<&str>` in the launcher, `None` until the owner
  provides the approved ID. The effective client is `[auth] client_id` when set ("your own
  app"), else the public ID ("bastion-mc's app"), else none (today's message, now naming both
  ways).
- The account summary in the event protocol gains which app is in use (backward compatible:
  a new field; `client_id_configured` stays true for either). The front end and `bastion-mc
  status` say "signing in through bastion-mc's app" or "through your own app (…)".
- The default flow stays the browser flow with PKCE on a loopback redirect; the device-code
  flow stays opt-in (`--device`), since a widely used public client makes device-code phishing
  more attractive.
- Documents: README (signing in: the public app by default, your own app for those who want
  it, with the reasons someone might), THREAT_MODEL (what a shared public client changes: one
  registration for everyone, which Microsoft or Mojang can block; its owner can change the
  app's name and settings but never receives anyone's tokens; device-code phishing), and the
  owner's one-time steps (register the app: personal Microsoft accounts only, public client
  flows on, redirect `http://localhost`; request Minecraft API access; then the ID goes into
  the constant).
- Tests: the selection (own over public over none), the messages, the event field, and the
  config test that today asserts there is no built-in ID, changed to assert the selection.

**Left to the builder.** Where the constant lives and its name; the exact wording.

**What only the owner can do.** Register the application, ask Mojang for approval, and send
the client ID. Until then the public method reports itself as not available in this build.
