# Bedrock sign-in package: hand-over file

Package: sign-in (round 5). Clone: `target/agents/r5-signin`, branch `work`.
Notes for the integrator and the user: `docs/notes/bedrock-signin.md` (user text, threat-model
rows, what was verified and what was not, open items, integration and contract changes).

## Done (committed)

- proto: `answer` module (grammar of `answer <kind> <text>`, `AnswerKind`, `SignInPage`,
  `SignInState`), events `SignInPage` and `Answerable` (backward compatible, no protocol bump).
- `events::input`: answer registry (one subscriber per kind), event-mode control loop (`stop`
  unchanged), terminal reader thread (human mode; poll + blocking read; reads only as the
  terminal's foreground job), `PendingHandle`, harness `answers`. `Reporter::answers(kind)`,
  `Reporter::answer_for_test` (capturing reporter only); human texts for the sign-in prompt and
  for Realms prompts answered here.
- `bedrock::signin`: `check` (start URL / final URL; scope, PKCE S256, refused override
  parameters), `wire` (frames, objects only), `server` (session task: one prompt, cap, cooldown,
  expiry, cancel, peer gone, accept pacing), `stub` (client; 30-call syscall list), harnesses
  `signin_frame`, `signin_url`. `main.rs` dispatch on argv[0].
- `realms::answers`: `tracking` announcer wrapper + `serve` pump into `Approvals`.
- GUI: `view/signin.rs` prompt (open page, copy link, paste field with focus, Send, Cancel,
  pre-check), Realms answered here (`view/realms.rs`), `Handle::answer` writing answer lines,
  app/view/emulated tests.
- Experiments: pty probe of the foreground check; Firefox 159 with a stand-in redirect
  (address bar and console show nothing; the Network panel's 302 `Location` does) — the prompt
  texts follow that.
- Browser helper (optional item 4): decided against, reasons in the notes (section 4).

## Final verification (last run)

`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets` no warnings;
`cargo-deny deny --offline --locked check`: advisories, bans, licenses, sources ok (one existing
warning about an unmatched licence allowance in `deny.toml`); `cargo test --workspace` (isolated
XDG root under `target/final-xdg`, no session bus, DISPLAY=:77, XAUTHORITY=/dev/null): 38 test
binaries, 830 passed, 0 failed, 2 ignored (the pty probe and the usage snippet in
`realms/answers.rs`), exit 0.

## If resumed

Nothing is half-done. Possible next steps (not started): the scheme-handler delivery of the
final address (notes §4.1), and re-checking `check::SCOPE` against the first real sign-in.
