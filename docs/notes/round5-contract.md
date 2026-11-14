# Round 5 contract: Minecraft Bedrock, the Android build in bastion's sandbox

Written by the integrator on 2026-11-14 before five engineers started in parallel. Every
package builds against this file and against `src/bedrock/` as committed in the base
(`mod.rs`, `policy.rs`, `runtime.rs`, `store.rs`, `net.rs`, `signin.rs`). Change neither on
your own: if the contract is wrong for your package, implement the nearest thing that fits
and say exactly what should change in your notes file (the integrator reconciles).

The research behind it is `docs/notes/bedrock-research.md` (read §0, §2.1, §3, §4 and §5 at
least). Its read-only source clones are in `/home/ski/RustroverProjects/bastion-mc/target/research/`
(mcpelauncher client, UI and manifest; gophertunnel and go-nethernet for the Bedrock and
Realms protocols; BedrockOnLinux). Read them; never build or run anything from there.

## What the user asked for, and decided

- "Minecraft Bedrock cape events exist. Ensuring that launching Bedrock is secure and capes
  can be obtained, we need to implement something similar [to BedrockOnLinux]. I don't want
  to fork BedrockOnLinux."
- BedrockOnLinux's Windows (GDK) route works only by extracting keys and decrypting the game
  outside Microsoft's protected path: DRM circumvention. **It is not built, in any form.**
- Decided (2026-11-14): **the Android build**: bastion runs the user's installed
  `mcpelauncher-client` in its own sandbox, on game files the user downloaded with
  `mcpelauncher-ui` from their own Google Play purchase. **Sign in every session**: the game's
  token store is wiped at exit. **Realms and event servers in the first version**, besides
  servers and single-player challenges. Microsoft telemetry allowed by default, with a switch.

## Invariants (not negotiable)

1. **No circumvention in bastion's code.** bastion decrypts nothing, patches no game file,
   answers no licence query and contains no Google Play client. The one licence-related fact
   — `mcpelauncher-client` answers the game's Google Play licence query after
   `mcpelauncher-ui` checked ownership — is accepted by the user with
   `[bedrock] accept_licence_substitution = true`; `policy::gate` refuses every launch
   without it.
2. **The client is installed, never fetched.** An `mcpelauncher-client` from a system
   package: root-owned, no writable path component (as `java.rs` requires of a JDK). bastion
   never downloads or builds one. Flatpak and AppImage installs are refused.
   `mcpelauncher-ui` (the Google sign-in and download tool) stays outside bastion.
3. **No Java credential exists in a Bedrock session.** The launch skips the Microsoft →
   Minecraft sign-in and never touches the token store or the vault; the sandbox has no path
   to bastion's stores and no keyring (the seccomp policy already denies `keyctl`).
4. **The game's Xbox tokens live one session.** The client's data directory is split: the
   token store (where the game's sign-in library keeps its tokens; find it in the client's
   source and name it) is per-session scratch, wiped at exit and, if a crashed session left
   one, before the next start. Worlds and the game's own settings persist
   (`persist_worlds = true` by default). Nothing a session wrote is ever executed.
5. **Sign-in pages never run in the game's sandbox.** The client's webview is replaced by
   bastion's stub (`signin.rs`): mounted at `/bastion/bin/mcpelauncher-webview` *and over
   every path where the client could find an installed webview* (`Client::webview_paths` —
   the client searches a path relative to its own directory before `PATH`, see
   `src/xal_webview_qt.cpp` in the research clone). No browser engine runs inside (the
   sandbox forbids namespaces, which every browser sandbox needs).
6. **Network.** Filtered mode only. HTTPS by SNI for the names in `net.rs`, RakNet over UDP
   to listed servers (one rule per server), Realms and event-server joins brokered and
   approved outside the game (`[realms] approve`; off → refused with the reason). LAN and
   NetherNet (WebRTC) are refused with a reason in this version. No other UDP.
7. **The confined launcher stays confined.** After step 9 the launcher executes nothing,
   binds or connects no new pathname socket, binds no TCP socket and writes only what its
   `Confinement` lists. Everything a Bedrock session needs — the bridge listener, relay
   sockets, the CA, any helper — is made before step 9.
8. **No native mods.** The client's mod loader gets no mods directory.
9. **Honest limits.** There is no `mcpelauncher-client`, no game, no Google Play purchase and
   no Microsoft account on this machine. Everything is verified against stand-ins; every
   notes file states what was run, against what, and what was not.

## Shared definitions (in the base)

- `bedrock::BedrockConfig` (`[bedrock]`): `accept_licence_substitution`, `client`,
  `telemetry`, `persist_worlds`, `seccomp` (`enforce`|`log`), `servers`. A package that
  needs another key adds it as one contiguous block at the end of the struct, with its
  default, a line in `examples/config.toml`, and a sentence in its notes.
- `bedrock::policy::gate`, `bedrock::runtime::{Client, discover}`,
  `bedrock::store::{OpenedVersion, open, open_dir_unverified, versions_dir}`,
  `bedrock::net::{NetPlan, plan}` and its name lists, `bedrock::signin::{Bridge, constants}`.

## The instance

`instance.toml` of a Bedrock instance (owned by the launch package):

```toml
kind = "bedrock"
[bedrock]
version = "1.26.62.1"          # an id in the store (`bastion-mc bedrock versions`)
servers = [{ host = "play.example.org", port = 19132 }]   # added to [bedrock] servers
```

Created by `bastion-mc instance new <name> --bedrock <version-id>`. Java-only commands
(`install`, `profile …`) refuse a Bedrock instance with a sentence that says so.

## The sandbox (launch package; research §5.3)

Namespaces as for Java (user, pid, ipc, uts, cgroup, net); an empty root; `/usr` read-only;
a synthetic `/etc` (passwd entry, `resolv.conf` → the launcher's resolver, the system CA
bundle **plus the network package's per-launch CA** for the Realms/gatherings broker).
Mounts: `/bastion/game` (the imported version, read-only, from `OpenedVersion::dir`),
`/bastion/data` (the persistent part: worlds and settings), the token store as a per-session
directory inside it, `/bastion/cache` (tmpfs), `/bastion/bin/mcpelauncher-webview` and the
masked webview paths (the stub), `/bastion/run` (the bridge's socket directory). No home,
nothing from `/run/user` except the display and audio sockets the existing decisions allow,
no `/dev/input`, no hidraw, no `/dev/snd` unless `audio = "alsa"`. Landlock: execute only the
client, its libraries, the dynamic loader and the stub (the game's `.so` files are mapped by
the client's own linker with `mmap`, so the game directory needs read access only). A
seccomp allowlist of its own for the client — the tripwire unchanged, namespace creation
denied — with `seccomp = "log"` for profiling the real client once (loud in the log).
Display: the Wayland security context, or the nested compositor on X11; never the session's
X display. Audio: as for Java. Resource limits: as for Java. Arguments:
`-dg /bastion/game -dd /bastion/data -dc /bastion/cache`, after checking them against the
client's `--help` in the research clone.

## The sign-in bridge (sign-in package; `signin.rs` has the wire format)

- **The stub is the launcher binary itself**: `main.rs` dispatches on
  `argv[0]`'s file name == `mcpelauncher-webview` before anything else (no config, no
  logging, no environment beyond what it needs) and runs only the bridge client. The launch
  package mounts the launcher's own executable read-only at the stub paths.
- The launcher side: `signin::Bridge` (base) listens; the sign-in package's server validates
  each request (`signin.rs`: start URL, client id, redirect, end prefix, final URL), allows
  one pending request and a small number per session, and gets the page shown **outside the
  game**: the user's browser (the front end opens it — `login.live.com` is already an
  allowed sign-in host — or the terminal prints it), then the final URL pasted back; and,
  if it can be done robustly, bastion's sandboxed browser helper with a visible window,
  started before step 9, which catches the redirect itself.
- Pasted answers come only from trusted channels: the terminal of a terminal launch, or the
  front end through the launcher's standard input in event mode. The sign-in package
  extends the event-mode input reader (`events.rs`, today only the stop line) to
  `answer <kind> <text>` lines (`kind`: `signin`, `realms`; bounded length, printable ASCII)
  and owns the new events and the front end's prompt. The game can never answer a prompt.

## The network (network package)

- The Bedrock profile in the relay: SNI exact/suffix allowlist and deny list from
  `NetPlan`; UDP RakNet per server rule (generalise `egress/udp.rs`: anchored to a name rule
  or an allowlisted literal, flows bounded in number, size and rate for gameplay, datagrams up
  to RakNet's MTU-discovery size); refused names and UDP destinations reported once each, so
  that the user can extend `servers`.
- Realms and event servers: TLS termination of the Realms API
  (`bedrock.frontendlegacy.realms.minecraft-services.net`) and of the gatherings/experience
  service that hands out event servers, with a per-launch CA (its PEM goes into the sandbox's
  trust store; the client's own certificate checks are stubbed by the client and its HTTP goes
  through the system libcurl — verify both in the research clone). Join answers are rewritten
  to one-time pass addresses and approved outside the game through the existing
  `realms::approval` (answers from the trusted input of the sign-in package's reader).
  `NETHERNET` join answers are refused with a reason. Tokens seen in terminated traffic are
  never logged.
- One entry point the launch calls before step 9 (document its signature in your notes; the
  integrator wires it into the launch package's code).

## Packages and file ownership

| package | clone | owns |
|---|---|---|
| store | `r5-store` | `bedrock::runtime` (discovery of the installed client: paths the packages use, ownership checks, `webview_paths`), `bedrock::store` + import (`bastion-mc bedrock import|versions|remove`, the manifest, the store layout), its harness |
| launch | `r5-launch` | `Kind::Bedrock` (`instance.rs`, `instance new --bedrock`), `src/bedrock/launch.rs` and the dispatch block in `launch.rs`, the sandbox profile (`sandbox/` additions: the client's mounts, Landlock, seccomp allowlist and log mode), the data/token split and wipe, worlds persistence, a stand-in client, protection read-out |
| network | `r5-net` | `bedrock::net` and everything in `egress/` for Bedrock: SNI profile, RakNet relay, the Realms/gatherings broker and CA, join rewrite and approval wiring, NetherNet/LAN refusal, discovery reports |
| sign-in | `r5-signin` | `bedrock::signin` (server, validation, the stub in `main.rs`), the event-mode `answer` input in `events.rs`, the new events in `crates/proto`, the front end's sign-in and answer prompts (`gui/`), the optional browser-helper path |
| risk | `r5-risk` | `bedrock::policy` (gate text, doctor lines), `scram` (Bedrock data and token stores; a running session), the canary prompt after a Bedrock session, `docs/BEDROCK.md` (the user guide), the threat-model and runbook text (in its notes, for the integrator) |

Shared files (a small, contiguous block each; the integrator merges): `src/launch.rs`,
`src/cli.rs` (help and dispatch), `src/config.rs`, `src/lib.rs`, `src/main.rs` (the stub
dispatch is the sign-in package's), `src/fuzzing.rs` `ALL` and `tests/robustness.rs`,
`examples/config.toml`, `crates/proto`, `gui/` (new view files, smallest wiring),
`Cargo.toml` (a new crate needs a reason in the notes and a passing `cargo deny check`).
Nobody edits `README.md`, `TODO.md` or `docs/THREAT_MODEL.md`, `docs/TOKEN_INJECTION.md`,
`docs/SCRAM_RUNBOOK.md` this round (a website refresh is changing them elsewhere): each
package writes `docs/notes/bedrock-<package>.md` (user text, threat-model rows, what was
verified, what is open) and keeps `docs/notes/bedrock-<package>-progress.md` as its
hand-over file. The risk package additionally writes `docs/BEDROCK.md`.

## Only the user can test

Google Play ownership and the download with `mcpelauncher-ui`; the real client and game in
the sandbox (and its seccomp profile, in log mode); the Microsoft sign-in, two-factor and
passkeys; the Dressing Room; joining a real server, Realm or event server; whether a
challenge registers with telemetry on or off; a cape claimed in Bedrock appearing in
`bastion-mc cape list` and being set with `cape set` (which also needs bastion's own Java
sign-in, never run live yet).
