# Round 4 contract: the phone and its services

Written by the integrator on 2026-11-13 before five engineers started in parallel. Every
package builds against this file and against `src/phone/mod.rs`. Change neither on your own;
if the contract is wrong for your package, implement the nearest thing that fits, and say
exactly what should change in your notes file (the integrator reconciles).

## What the user asked for (their words, condensed)

- A **Spotify interconnect**: play and manage Spotify from inside the game. The user chose
  both the **desktop app over MPRIS** and the **Spotify Web API** (with their own developer
  client ID).
- A **web browser in the game**; **Discord in an embedded browser**; a **pre-listed set of
  websites in config**; **limits on API calls**. The user chose a browser that runs in a
  **separate sandbox** (not Chromium inside the game).
- **Realms** support with an **"OTP override"**, settable: either a **one-time code the
  launcher shows** that the user types in-game, or a **TOTP code** from an authenticator app.
- It must **work across numerous Minecraft versions**.
- It should **feel like pulling out a phone** in the game to manage Spotify, Discord etc.
- **Clipping** built into the game, managed from the phone.
- **Redo the Minecraft home screen**; **dark mode**.
- **Settings sync across instances.**

## Invariants (not negotiable)

1. **Secrets never enter the game.** Spotify tokens, the browser's profile (the Discord
   session), the TOTP secret and the Microsoft token stay outside the game's sandbox. The
   game sees only what the phone shows: state, pictures, notices.
2. **The launcher enforces; the mod is interface only.** Every command from the phone is
   untrusted input: parsed strictly (`phone::Command::parse`), rate-limited per app and verb
   (`VerbLimit`), and able to do only what its app's fixed vocabulary allows. No argument is
   ever used as a path, a URL, a command line or code. (A browser app may navigate only to
   the configured sites; a Spotify app may open only `spotify:` URIs of the documented kinds.)
3. **The confined launcher stays confined.** After step 9 the launcher executes nothing, opens
   no new pathname socket (the resolver's excepted), binds no TCP socket and writes only what
   its `Confinement` lists. Every helper a service needs (headless browser, `xdg-dbus-proxy`,
   the clip recorder) is started, and every socket connected, before step 9.
   `Service::confinement_needs` adds directories and TCP ports — nothing else exists.
4. **Each helper runs in a sandbox of its own** (bubblewrap, no network unless the feature is
   network, no home directory, only its own files); nothing of the game's sandbox is shared
   with it except what the feature needs (the recorder: the game's display, to read).
5. **Defaults.** On by default: the phone, the new home screen and the dark theme (they are
   the companion mod's, which is on by default for Fabric), and settings sync. Off until the
   user configures them: the browser (it needs `[browser] sites`), the Spotify Web API (it
   needs a client ID), clips (recording costs a CPU core), Realms approval (needs an account).
   Spotify over MPRIS is on by default (no account, nothing leaves the machine).
6. **Honest limits.** Every notes file states what was verified by running, against what,
   and what was not; every new channel gets threat-model text (who can do what through it).
7. **Numerous Minecraft versions.** The mod's phone, home screen and dark theme work on every
   Fabric release family from **1.20.1 to 26.3** (verified in the real game on a
   representative of each family); the launcher's services do not depend on the version;
   settings sync translates `options.txt` between versions.

## The phone protocol (companion channel, protocol 2)

Transport: the existing companion UNIX socket (`src/companion/channel.rs`). A mod that speaks
protocol 2 starts as today with one text line, `hello 2 <mod-version> <features>\n`. After
that line both directions use framed messages:

    u32 length, big-endian (counts the kind byte and the payload) | u8 kind | payload

| kind | direction | payload | limit |
|---|---|---|---|
| `0x01` COMMAND | mod → launcher | one command line, UTF-8 (grammar below) | 1 KiB |
| `0x02` DIAG | mod → launcher | as protocol 1's `diag <feature> <text>` | 512 B |
| `0x81` STATE | launcher → mod | app name (ASCII), `0x00`, a JSON object (UTF-8) | 64 KiB |
| `0x82` FRAME | launcher → mod | app name, `0x00`, `u8` slot, `u16` width, `u16` height, `u8` format (1 PNG, 2 JPEG, 3 RGBA8), the image bytes | 4 MiB |
| `0x83` NOTICE | launcher → mod | app name, `0x00`, printable text | 256 B |

A mod that says `hello 1` is served exactly as before (logged, never answered). Anything that
breaks the framing ends the connection (logged once). Global bounds on the mod's side of the
socket, besides each app's limits: at most 50 messages and 200 KiB per second.

**Command grammar** (`phone::Command::parse`): `<app> <verb> [arg]... [-- <text>]`. Names
`[a-z0-9-]{1,16}`; at most 8 arguments, printable ASCII without spaces, ≤ 64 bytes, never
`--`; free text ≤ 512 bytes of UTF-8 without control characters or bidirectional overrides.

**Rust side:** a service implements `phone::Service` (prepared before step 9) and
`phone::PhoneApp` (used after it); it sends with `phone::Outbox` (`state`, `frame`, `notice`;
never blocks, refuses oversize). The dispatcher (owned by mod-platform) routes commands by
app name, applies each app's `limits()` and calls `hello` when the phone connects.

## App vocabularies

Verbs not listed are refused by the dispatcher. Each app's state is one JSON object; the
fields below are the minimum (more may be added; the phone ignores unknown fields).

### `phone` (mod-platform)
Sent by the dispatcher at `hello`: `{"apps": ["spotify", ...], "theme": "dark"}` — only the
apps whose service is prepared for this launch.

### `spotify` (media)
Verbs: `play`, `pause`, `toggle`, `next`, `previous`, `seek <ms>`, `volume <0-100>`,
`shuffle <on|off>`, `repeat <off|track|context>`, `open <spotify-uri>` (only
`spotify:{track,album,playlist,artist,episode,show}:<base62>`), `queue <spotify-uri>` (track
or episode), `search -- <text>`, `devices`, `transfer <device-id>`, `art`.
State: `{"source": "mpris"|"web"|"none", "playing", "track", "artist", "album", "duration_ms",
"position_ms", "volume", "shuffle", "repeat", "results": [{"uri","title","subtitle"}],
"devices": [{"id","name","active"}], "error"}`. Frame slot 0: album art, at most 320×320.

### `browser` (browser)
Verbs: `open <site-name>` (a configured site), `close`, `back`, `forward`, `reload`,
`size <w> <h>` (bounded viewport), `mouse <move|down|up> <x> <y> [left|right|middle]`,
`scroll <dx> <dy>`, `key <name>` (a fixed list: Enter, Backspace, Tab, Escape, Delete,
Home, End, PageUp, PageDown, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, and Ctrl+A/C/V/X/Z
as `ctrl-a` …), `text -- <utf8>`, `frames <on|off>`.
State: `{"sites": [{"name", "title"}], "open": <name>|null, "host", "loading", "title",
"error", "signed_in_hint"}`. Frame slot 0: the page (JPEG), at most 15 per second, only while
`frames on`.

### `clips` (media)
Verbs: `status`, `save [seconds]` (bounded by the buffer), `list`, `delete <id>`,
`rename <id> -- <text>`, `thumb <id>`.
State: `{"recording", "buffer_seconds", "clips": [{"id", "name", "created", "seconds",
"bytes"}], "folder", "error"}`. Frame slots: thumbnails (slot = index in the last list, ≤ 32).

### `realms` (realms-sync)
Verbs: `code -- <digits>` (answers the current prompt), `cancel`.
State: `{"mode": "code"|"totp"|"off", "prompt": null | {"world", "address", "expires_in",
"attempts_left"}, "last": "approved"|"refused"|"expired"|null}`.

### `settings` (realms-sync)
Verbs: `status`, `sync-now`.
State: `{"sync": true|false, "last_sync", "keys", "instances", "error"}`.

## Packages and file ownership

| package | clone | owns |
|---|---|---|
| mod-platform | `r4-mod` | version support of the companion mod (adapters per Minecraft family, mixin selection at run time, stubs per family, the hermetic build: `src/companion/jar.rs`, `build.rs`, `java/stubs/`), protocol 2 on both sides (Java channel; `src/companion/channel.rs`), the dispatcher and global limits (`src/phone/dispatch.rs`), a `demo` service for UI tests, everything under `java/companion/**/platform/` |
| mod-ui | `r4-mod` | the phone and its apps' screens, the home (title) screen, the dark theme: `java/companion/**/ui/`, `**/phone/`, `**/home/`, `**/theme/` — built only on the platform API |
| browser | `r4-browser` | `src/browser/`, the headless browser's sandbox and its network policy, `bastion-mc browser login <site>`, `[browser]` |
| media | `r4-media` | `src/spotify/`, `src/clips/`, `bastion-mc spotify login|status`, `[spotify]`, `[clips]` |
| realms-sync | `r4-realms` | the Realms parts of the broker, `src/realms/`, `src/sync/`, `bastion-mc realms totp-setup`, `[realms]`, `[sync]`, the approval prompt in the launcher's own output (and its event for the front end) |

Shared files (anyone may add a small block; the integrator merges): `src/launch.rs` (one
block per service: prepare before step 9, `into_app` after step 9), `src/config.rs`,
`src/cli.rs` (help text and dispatch), `examples/config.toml`, `src/lib.rs`, `crates/proto`
(new events), `src/fuzzing.rs` `ALL` and `tests/robustness.rs`, `Cargo.toml` (a new crate
needs a reason in your notes and a passing `cargo deny check`), `gui/` (only new view files
and the smallest wiring). Nobody edits `README.md`, `TODO.md` or `docs/*.md` this round:
each package writes `docs/notes/<package>.md` (user text, threat-model text, what was
verified, what is open) and keeps `docs/notes/<package>-progress.md` as its hand-over file.
