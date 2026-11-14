# Bedrock sign-in: the bridge, trusted answers, the front end's prompts

Package: sign-in, round 5 (`docs/notes/round5-contract.md`). Branch `work` of
`target/agents/r5-signin`. The hand-over file is `bedrock-signin-progress.md`.

**Honest limits first.** This machine has no `mcpelauncher-client`, no game files, no
Google Play purchase and no Microsoft account, and none may be installed or contacted.
Everything below was run against stand-ins written for this package: a stand-in "client"
(the tests start the real launcher binary under the webview's name inside bubblewrap), stand-in
sign-in requests and final addresses in the shape the research expects, a stand-in launcher
for the front end, and a stand-in redirect page for Firefox. Section 3 says exactly what ran.

---

## 1. For the user (text for the README and `docs/BEDROCK.md`)

### Signing in to Bedrock

Bedrock signs in **inside the game**, with Minecraft for Android's own sign-in library: the game
holds your Xbox identity for the session (and bastion-mc wipes the game's token store when the
session ends). What bastion-mc controls is the **sign-in page**. When you press *Sign in* in the
game, the game does not get a browser: it gets bastion-mc's bridge, and the page is shown
**outside the game**.

- **In the window (`bastion-mc-gui`)**: the Launch page shows *Microsoft sign-in for Bedrock*.
  *Open sign-in page* opens Microsoft's page (always `https://login.live.com/…`, nothing else
  is ever offered) in your usual browser. Sign in there — password, second factor, passkey:
  all of it happens in your browser, never in the game.
- **On a terminal launch**: the same page's address is printed on a line of its own; open it
  in any browser.

At the end, Microsoft sends your browser to an address that starts with
`ms-xal-0000000048183522://auth?code=` — an address for the game, which your browser cannot
open. **Your browser may not show it at all**: Firefox (checked with version 159) stays on the
previous page, with nothing in the address bar and nothing in the console. To get it:

1. Before the last step of the sign-in (or before you start it), open the browser's developer
   tools with **F12** and choose the **Network** tab.
2. Sign in. The last request in the list has failed (in Firefox its transfer column shows an
   error beginning `NS_ERROR_UNKNOWN`; its status is 302). Select it.
3. In its **response headers**, the `Location` header holds the address. Copy its value.
4. Paste it into the field in the window and press *Send* (or Enter) — or, on a terminal launch,
   paste it into the terminal and press Enter.

bastion-mc then checks the address (it must be the game's address, carry a sign-in code, and
belong to *this* sign-in) and hands the game only the code and the sign-in's state, nothing
else you pasted. The prompt says why an address was not accepted; you have 10 tries and 15
minutes. *Cancel* (or typing `cancel` on the terminal) refuses the sign-in; the game is told
that it failed. If you did not press *Sign in* in the game, cancel.

Limits: one sign-in prompt at a time, at most 6 per session, and after one ends without an
address the next waits 10 seconds — a game cannot keep a prompt in front of you.

What reaches the game: only that one address (code and state). What never does: your
password, second factor, browser cookies, or any page the game chose — bastion-mc shows only
Microsoft's sign-in page for Minecraft's own client, asking only for Xbox sign-in.

### Realms joins in a Bedrock session

A Bedrock session has no phone to type a Realms code into. A Realms join that waits for your
approval (`[realms] approve`) is therefore approved **outside the game**: in the window, the
Realms panel offers *Approve join* and *Refuse* (in `code` mode the shown code is sent with
your approval; in `totp` mode you type your authenticator's code into the panel). On a terminal
launch, type the code shown and press Enter (`cancel` refuses). When a sign-in and a Realms join
wait at the same time, start the terminal line with `signin ` or `realms `.

### For other front ends: the answer lines

Besides `stop`, a front end may write lines `answer <kind> <text>` to the launcher's standard
input in event mode: `kind` is `signin` or `realms`; `text` is printable ASCII (spaces inside,
none at either end), at most 4096 bytes; `cancel` refuses the prompt. Over-long lines are
dropped whole, never cut. The launcher announces each kind it takes answers for with the event
`{"event":"answerable","kind":…}`, and a Bedrock sign-in with
`{"event":"sign_in_page","url":…,"end":…,"expires_in":…,"attempts_left":…,"state":"waiting"|"done"|"cancelled"|"expired"|"failed","note":…}`.
The grammar is `bastion_mc_proto::answer` (one definition for both sides).

---

## 2. Threat model rows (for `docs/THREAT_MODEL.md`, table H's Bedrock rows)

| # | Threat | What stops it | What is left |
|---|---|---|---|
| B-S1 | The game makes the launcher show a page that phishes for the password, or another application's consent (mail, files, the account) under the guise of "sign in to Minecraft". | The bridge shows only `https://login.live.com/oauth20_authorize.srf` with `client_id` 0000000048183522, `redirect_uri` `ms-xal-0000000048183522://auth`, `response_type=code`, `scope` exactly `service::user.auth.xboxlive.com::MBI_SSL`, a PKCE challenge with `S256`, a state; no `request`/`request_uri`/`resource`/return-address parameters; parameter names plain, case-insensitively unique, no `;` (`bedrock::signin::check`). The front end opens only Microsoft sign-in hosts. | Cosmetic parameters of the page (`login_hint`, `display`, `prompt`, `mkt`, …) are the game's choice. Whether login.live.com honours some parameter not on the refused list in a harmful way is not known. |
| B-S2 | The game answers its own prompt (types the "final address"). | Answers come only from the launcher's standard input in event mode (the front end's pipe) or its terminal. The game's standard streams are the launcher's own sockets; the sandbox starts a new session (no controlling terminal, so no `TIOCSTI`), and both the game's and the confined launcher's filters deny the terminal-injection ioctls. | The user can be talked into pasting something by the game's screen. The state check limits that to an address of this very sign-in, which the game could have had anyway. |
| B-S3 | Prompt spam: the game opens sign-in prompts until the user gives in. | One prompt at a time; 6 per session; 10 s after one ends without an address; 15 min each; connections paced (4/s, burst 8) and read by at most 4 tasks for 5 s each. | A game can show 6 prompts per session. |
| B-S4 | Hostile bytes on the bridge socket (the game can connect without the stub). | Frames ≤ 16 KiB, length checked before allocation, exact JSON object shapes (no arrays, no unknown or repeated fields), URLs parsed by a strict parser of their own; fuzz harnesses `signin_frame`, `signin_url`. | — |
| B-S5 | The sign-in code is exposed on the way back. | The code is single-use and useless without the PKCE verifier that only the game holds; it is never logged, never put in an event, and only the rebuilt `code`+`state` address goes to the game. | The terminal echoes the pasted address into its scrollback; the front end shows it in its field. After use the code is worthless. |
| B-S6 | The stub inside the sandbox is a way out. | The stub is the launcher's binary run under the webview's name: it reads no configuration, no environment, writes no file, and needs 30 system calls (checked under an allowlist of exactly those). It is not a boundary: everything is checked by the launcher. | It runs under the client's own filter: the launch package must allow those 30 calls. |
| B-S7 | A same-user program outside the sandbox uses the bridge (it can reach the 0700 runtime directory only as the same user) to phish an Xbox sign-in through bastion-mc's prompt, with a PKCE pair of its own. | Nothing beyond the checks above: same-user code is outside what bastion-mc can stop (P7). | The prompt cannot tell a request from the game from one by another program of the same user. |
| B-S8 | A terminal launch in the background is stopped by its own terminal reader (`SIGTTIN`), relay and all. | The reader reads only while the launcher is the terminal's foreground job (`tcgetpgrp`). | A job put into the background between the check and the read is stopped like any program, until it is brought back. |
| B-R1 | Realms without the phone: the game approves its own join. | The approval comes from the trusted input (above), never the game; in `code` mode the code binds it to the prompt shown; all of `realms::approval`'s bounds hold. | In `totp` mode a code approves whatever join waits when it arrives (as before); the game can no longer see the code being typed, as it could on the phone. |

Residual risks, stated plainly: the bridge narrows what the game can show, not what it gets:
the game's sign-in library receives the Xbox identity by design, and a compromised game keeps
it for the session (the contract's B-asset). The paste step is the weakest part for people:
it needs the developer tools (section 4 has the better ways).

---

## 3. What was verified, against what — and what was not

Verified by running, on this machine (Arch, kernel 7.2, rustc 1.100, bubblewrap from Arch,
Firefox 159.0.1), all with `env -u DBUS_SESSION_BUS_ADDRESS` and private XDG directories:

- **Answer grammar** (`crates/proto/src/answer.rs`, 5 tests): lines written by `answer_line`
  parse back to the same answer; the stop line is exactly what it was (`stop\r`, ` stop` are
  not stop); hostile lines (escapes, non-ASCII, over-long, unknown kinds) are refused with a
  reason.
- **Trusted input** (`src/events/input.rs`, 7 tests and an ignored probe; 2 text tests in `events.rs`): one
  subscriber per kind, answers in order, a bounded queue; the control loop routes answers, stops
  exactly on `stop`/end of input, drops over-long lines whole, and gives the same answers however
  the bytes are cut; the terminal router sends a bare line to the one waiting prompt and asks for
  `signin `/`realms ` when two wait; the terminal thread delivers typed lines and ends with its
  input or its command. The real binary in event mode (`tests/events.rs`): answer lines nothing
  asked for, broken and over-long ones are noted as status events, and `stop` after them still
  cancels the command.
- **Foreground check** (manual experiment with a pseudo-terminal, `script` from util-linux): the
  probe test `probe_foreground_of_standard_input` printed `foreground=true` as a foreground job,
  `foreground=false` as a background job with job control on (`sh -c 'set -m; … & wait'`), and
  `true` for a pipe.
- **URL checks and frames** (`check.rs` 7 tests, `wire.rs` 3 tests): every rule in B-S1, the
  final address rules (prefix, code, state, error reports, the 1023-byte limit of the client),
  rebuilt address is a fixed point; arrays are not taken for requests (found by a test and
  fixed: serde's derived struct reader accepts `["a","b"]`).
- **Server** (`server/tests.rs`, 12 tests, 11 of them over a real socket): prompt shown and rebuilt address
  returned; refusals answered, logged once per reason, said to the user once; broken frames and
  silent connections; one prompt at a time and the per-session cap; the cooldown; expiry; the
  peer closing withdraws the prompt; wrong answers say why and the last allowed one ends it; shutdown
  withdraws; a flood of 40 idle connections does not delay the answer; the stub's code
  in-process prints the address and exits 0, or prints nothing on standard output and exits 1.
- **The real binary as the stub** (`tests/signin_stub.rs`, 4 tests, bubblewrap with an empty
  root, `/usr`, no network): started as `/bastion/bin/mcpelauncher-webview` (the launcher binary
  bound read-only there) with the socket directory bound **read-only** at `/bastion/run` —
  connecting works through the read-only bind; the address comes back on one line; a refused
  page leaves standard output empty and says why on standard error; killing the stub withdraws
  the prompt; and the stub completes a sign-in under a seccomp allowlist of exactly the 30 calls
  its module header lists (everything else `ENOSYS`; bubblewrap `--as-pid-1 --seccomp`).
- **Realms glue** (`realms/answers.rs`, 3 tests with the real `Approvals`): a code from the
  trusted input approves the join it was shown with, a wrong code costs an attempt and says so,
  `cancel` refuses, and a waiting prompt is known to the terminal router.
- **Fuzzing**: harnesses `signin_frame`, `signin_url`, `answers` registered in
  `src/fuzzing.rs` with seeds in `tests/robustness.rs`; default run (4000 mutations each) and a
  longer one: 200 000 mutations each for `signin_frame` and `signin_url` (seed 7), 50 000 for
  `answers` (seed 11) — all passed. The libFuzzer targets type-check (`cargo check` of `fuzz/`
  on stable); they were not run (no nightly toolchain here).
- **Front end**: app logic (5 tests: answering with the pasted address and retrying after a
  verdict, plainly wrong pastes not sent, Realms approved here with the shown or typed code, a
  Java session's Realms prompt untouched, an answer that cannot be written is reported); view
  (3 tests, and 7 new states plus one with endless text in the layout tests at 4 window sizes
  and 13 widths: nothing clipped, overlapping or out of reach); the emulated application with a
  stand-in launcher (`gui/tests/emulated.rs`): the prompt opens with the field focused, the
  typed address is written as `answer signin <address>`, a verdict reopens it, the second send
  is accepted, and *Approve join* writes `answer realms 123456`. Snapshots of the new states
  were looked at (`target/gui-snapshots/launch-bedrock-*.png`).
- **Where browsers show the final address** (experiment, Firefox 159 in bubblewrap, throwaway
  profile, no network, the invisible displays `:77` and a private one of my own for clicks):
  after a JavaScript navigation and after an HTTP 302 to `ms-xal-0000000048183522://auth?…`
  from a stand-in page, Firefox stays on the page before — nothing in the address bar, and no
  console message (with the console mirrored to standard output). Its developer tools' Network
  panel lists the failed request (status 302, a transfer error beginning `NS_ERROR_UNKNOWN`) and shows the address
  as the response's `Location` header. The prompts say so; an earlier text that sent users to
  the address bar was wrong and was corrected.

Not verified (and why):

- **Anything with the real client or Microsoft**: that `mcpelauncher-client` execs the stub and
  takes its line (read in its source, `xal_webview_qt.cpp`: one 1024-byte read of standard
  output, then standard error, then `waitpid`); that the game's sign-in library accepts the
  rebuilt address (`code` and `state` only); the real start address's parameters — above all its
  `scope` — which Xbox's sign-in service (SISU) makes, not the game; how Microsoft's last step
  redirects (HTTP 302 or script; only the 302 shows in the Network panel); two-factor, passkeys,
  account creation. No client, no account, no Microsoft contact allowed.
- **Chromium-based browsers**: not installed; where they show the address is not checked.
- **The Bedrock sandbox itself** (launch package): the stub under the client's real Landlock and
  seccomp profile, the mounts at the webview paths, `serve` wired into the launch after step 9.
- **A terminal launch end to end**: no command in this branch takes answers (the Bedrock launch
  is the launch package's), so the terminal reader was run by its tests and the probe, not by a
  real launch.
- **The background race** of B-S8 (a job backgrounded between the check and the read).

---

## 4. What remains open

1. **The paste step is poor for people** (developer tools). Two better ways, neither built:
   - *A scheme handler*: the package ships a desktop entry for
     `x-scheme-handler/ms-xal-0000000048183522` that hands the address to the running launcher
     (through a socket bound before step 9, accepted on after). The page stays in the user's own
     browser, nothing depends on Microsoft's hosts, and a web page cannot inject an address: the
     state check refuses anything but this sign-in's. Needs packaging and a product decision
     (registering a handler system-wide).
   - *The sandboxed browser helper* (optional item 4 of the package) — **not built**, because it
     could not be made robust here: (a) the hosts Microsoft's sign-in page needs (page assets,
     fraud protection, captcha, passkeys, account creation) are unknown and cannot be learned
     without contacting Microsoft, so its allowlist would be a guess that breaks real sign-ins
     in ways that are hard to see; (b) the confined launcher can start nothing after step 9, so
     a visible Firefox would have to be started at every Bedrock launch, before the game asks for
     anything; (c) the existing visible mode (`browser login`) has no remote control, and the
     redirect would have to be caught through WebDriver BiDi events whose behaviour for an
     unknown scheme was not established (the stand-in experiment above shows what Firefox does
     *visibly*, not what BiDi reports).
2. **The scope check** may need care after the first real sign-in: a page that asks for more than
   `service::user.auth.xboxlive.com::MBI_SSL` is refused with the scope named in the warning. If
   the real page carries another scope, it must be reviewed (what it grants) before it is added
   to `check::SCOPE`.
3. **The 1023-byte limit** of the final address is the client's (one 1024-byte read). If real
   addresses were longer, the client could not read them from any webview; bastion-mc refuses
   them with that reason instead of letting them be cut.
4. **Realms in Bedrock** needs the network package's broker to subscribe (`realms::answers`); the
   wiring is the integrator's (section 5).
5. A sign-in prompt's outcome leaves the page at once (status line and console keep it); a
   Realms panel keeps its outcome until the launch ends (round-4 design) and, together with a
   waiting sign-in, pushes the launch control below the fold at the opening window size (it
   scrolls; the layout tests check it is reachable).

---

## 5. For the integrator

**Entry points** (all in the launcher crate):

- `main.rs`: `bedrock::signin::is_stub(args.first())` → `bedrock::signin::stub::main(&args[1..])`,
  first thing in `main` (done in this branch).
- Before step 9 (launch package): `let bridge = bedrock::signin::Bridge::prepare(&run_dir)?;` and
  bind `bridge.socket_dir()` at `SANDBOX_RUN_DIR` — **read-only is enough** (tested).
- After step 9, inside the launch's runtime:
  `let signin = bedrock::signin::serve(bridge, &reporter, log)?;` with
  `log: bedrock::signin::server::Log = Arc::new(move |line: &str| launch_log.line(Source::Launcher, line))`;
  at the end of the session `signin.shutdown().await` (dropping it also ends it). It binds,
  opens and executes nothing: it only accepts on the listener.
- Realms in a Bedrock session (network package / integrator), where the session's `Approvals`
  is built:
  `let answers = reporter.answers(AnswerKind::Realms)?;`
  `let announce = realms::answers::tracking(announce, answers.pending_handle());`
  `let approvals = Approvals::new(mode, totp, pass, announce);`
  `let answering = realms::answers::serve(answers, approvals.clone(), reporter.clone());`
  (abort `answering` at the end). The general API is `Reporter::answers(kind) -> AnswerReceiver`
  (`recv`, `set_pending`, `pending_handle`); `Reporter::answer_for_test` works only on a capturing
  reporter.

**Requirements on the launch package's sandbox**: mount the launcher's own executable read-only
at `SANDBOX_STUB` and at every `Client::webview_paths` entry (the client finds it by the file name
`mcpelauncher-webview`, which is what the dispatch checks); Landlock must let the client execute
it and connect to `/bastion/run/signin.sock`; the client's seccomp filter must allow the stub's
30 calls (`stub.rs` header); the client's standard streams must never be the launcher's terminal
or control pipe, and `--new-session` stays.

**Shared files touched** (one contiguous block each): `src/main.rs` (stub dispatch and header),
`src/events.rs` (registry in `Inner`, `answers`, `answer_for_test`, control loop, the human
texts for `SignInPage` and Realms-answered-here), `src/realms/mod.rs` (`pub mod answers;`),
`src/fuzzing.rs` (three `ALL` entries), `tests/robustness.rs` (names, seeds, four tests),
`tests/events.rs` (answer lines before `stop`), `crates/proto/src/lib.rs` (header, `pub mod
answer` and re-exports, two `Event` variants, tests), `gui/` (new `view/signin.rs`; `app.rs`
fields, messages, handlers, tests; `backend.rs` `Handle::answer`; `view/realms.rs` answer-here;
`view/launch.rs` prompts above the page; `view/tests.rs`; `tests/emulated.rs`;
`tests/support/mod.rs` `type_text`), `fuzz/` (three targets, README paragraph). New files:
`src/events/input.rs`, `src/bedrock/signin/{check,wire,server,stub}.rs`,
`src/bedrock/signin/server/tests.rs`, `src/realms/answers.rs`, `crates/proto/src/answer.rs`,
`tests/signin_stub.rs`. No new dependency; no configuration key.

**Contract changes** (where `round5-contract.md` / the base did not fit):

- `signin.rs` gained submodules and re-exports; its constants and `Bridge` are unchanged.
- The checks go beyond the contract's list: `response_type=code`, `scope` exactly Xbox sign-in,
  PKCE with `S256`, a state, refused override and return-address parameters, case-insensitive
  unique names, no `;`. The contract should list them.
- The final address goes back **rebuilt** from `code` and `state` (a canonical form of the final
  URL), and at most 1023 bytes long (the client's single 1024-byte read), not up to `MAX_FRAME`.
- On failure the stub writes a short reason to standard error (the client puts it into the
  game's log); standard output stays empty, as the contract says.
- "Logged once": once per reason, with the totals at the end of the session.
- A new event `Answerable { kind }` tells a front end which prompts are answered through it — the
  contract did not say how the window would know that a Realms prompt has no phone.
- `realms::answers` (glue from the trusted input to `Approvals`) lives in this package although
  the approval wiring is the network package's, so that the API is tested against the real
  `Approvals`.
- The optional browser helper was not built (section 4).
