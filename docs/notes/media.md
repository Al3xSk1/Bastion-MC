# media: Spotify and clips on the phone

Round 4 package "media" (docs/notes/round4-contract.md): `src/spotify/`, `src/clips/`,
`bastion-mc spotify login | logout | status`, `[spotify]`, `[clips]`, the phone apps
`spotify` and `clips`. Hand-over file: `media-progress.md`.

## For the README (user-facing text)

### Spotify on the phone

The phone's Spotify app controls the **Spotify desktop app on this machine** — play, pause,
skip, seek, volume, shuffle, repeat, open a track, album, playlist, artist, episode or show —
and shows what is playing, with the cover. This is on by default and needs no account:
the launcher talks to the desktop app over MPRIS (D-Bus), through `xdg-dbus-proxy` in a
sandbox of its own that passes the Spotify app's player interface and nothing else of
your session bus. Nothing leaves the machine, except the cover: when the phone asks for it,
the launcher fetches it from Spotify's image server (`i.scdn.co`) — the same image the
desktop app shows — and the phone gets it re-encoded (`[spotify] album_art = false` turns
that off).

With the **Spotify Web API** the phone can also search, add to the queue, list your Spotify
devices and move playback to one of them, and control playback when the desktop app is not
running (Spotify allows playback control through the Web API only with Premium; the phone
says so when it is missing). It needs your own Spotify application — this launcher ships no
client ID and no secret:

1. Create an app at <https://developer.spotify.com/dashboard>; register the redirect URI
   `http://127.0.0.1/callback` (Spotify then accepts the port the sign-in uses).
2. Put its client ID in `config.toml`: `[spotify]` `client_id = "…"` (32 hex characters).
3. `bastion-mc spotify login` (add `--open` to have your browser opened) and sign in. The
   refresh token goes to the launcher's token store (kernel keyring or age vault), under a
   name of its own.

`bastion-mc spotify status` says what is set up and whether it answers (the desktop app
through the proxy, one Web API read; `--offline` checks only the configuration and the
store). `bastion-mc spotify logout` forgets the stored token (to revoke the grant itself:
your Spotify account settings, "Manage apps"). `bastion-mc scram` removes it with the rest.

Scopes asked for: `user-read-playback-state user-modify-playback-state` — nothing about your
account, library or playlists.

### Clips

With `[clips] enabled = true` (off by default: recording costs a CPU core) the launcher
records the game while it runs and keeps the last `buffer_seconds` (30 by default, at most
120) in memory. The phone's clips app saves them as a clip — a `.ts` video (H.264, at most
1280 wide, 30 frames a second) with a thumbnail next to it — into
`~/.local/share/bastion-mc/clips` (or `[clips] folder`), lists them, shows thumbnails,
renames and deletes them. There is no sound in clips. Limits: saves per minute (4), the
number of clips (50) and their total size (2 GiB) — when a save goes over, the oldest of the
clips this feature saved are removed; nothing else in the folder is ever touched.

Recording works with the nested compositor (gamescope, the default on X11 when it is
installed) and on X11 without it. It does **not** work with a Wayland display: nothing
outside the compositor can read the game's window there.

```toml
[spotify]
mpris = true          # the desktop app over MPRIS
album_art = true      # covers from i.scdn.co when the phone asks
# client_id = "…"     # your Spotify application: the Web API
# redirect_port = 43117

[clips]
enabled = false
buffer_seconds = 30   # 5–120
fps = 30              # 5–30
max_width = 1280      # 320–1280
saves_per_minute = 4  # 1–6
max_clips = 50        # 1–500
max_total_mb = 2048   # 16–65536
# folder = "/home/you/Videos/minecraft"
```

## How it works

### Spotify

* **Before step 9** (`spotify::prepare`, right after the launch log opens): the session bus
  socket is found (`DBUS_SESSION_BUS_ADDRESS`, only `unix:path=`; else
  `$XDG_RUNTIME_DIR/bus`), `xdg-dbus-proxy` is started in bubblewrap (`mpris::Proxy`: the
  bus socket and a directory to listen in, nothing else; Landlock and the game's seccomp
  policy; no network) with `--filter`,
  `--call=org.mpris.MediaPlayer2.spotify=org.mpris.MediaPlayer2.Player.*@/org/mpris/MediaPlayer2`,
  `--call=…=org.freedesktop.DBus.Properties.*@/org/mpris/MediaPlayer2` and `--broadcast`
  rules for `PropertiesChanged` and `Seeked` on that path, and `--fd=0` (ready byte on its
  stdin socket pair; it exits when the launcher's end closes). The launcher connects once,
  removes the socket's name, authenticates (`EXTERNAL`), says `Hello`, registers three match
  rules and reads the player's properties.
* With a client ID and a stored token: the **token keeper** (`bastion-mc __spotify-keeper`,
  `spotify::keeper`) is started; it loads the refresh token from the store, refreshes when
  the launcher asks (one byte), stores rotated refresh tokens, answers with an access token
  and its lifetime only. It reuses an access token until five minutes before it runs out,
  refreshes at most every 20 s and 60 times a session, registers a running marker (scram
  stops it), and ends with the launcher's socket. The first token is fetched now (at most
  12 s; the launch goes on without the Web API if Spotify does not answer).
* `confinement_needs`: TCP 443 (or the SOCKS5 proxy's port, `network.launcher_proxy`) when
  the Web API or album art is on. Nothing else.
* **After step 9** (`into_app`): an actor serves the phone. Transport verbs go to the desktop
  app when it runs (no network), else to the Web API; `queue`, `search`, `devices`,
  `transfer` are Web API only. Arguments are checked against the vocabulary before anything
  is sent (`spotify::check`): `open` takes the six documented `spotify:` URI kinds with a
  22-character base-62 id, `queue` tracks and episodes only, `transfer` only a device id
  from the last `devices` answer, `search` 1–200 characters of text that only ever becomes a
  percent-encoded query value. Web API calls are counted against local budgets per class
  (control 30/min, read 20/min, search 10/min, queue 10/min, art 20/min) and 60/min in all,
  and a `429` pauses everything for `Retry-After` (1 s – 1 h). A `401` refreshes once.
  `403 PREMIUM_REQUIRED` and `404 NO_ACTIVE_DEVICE` become plain messages on the phone.
  The Web API state is polled every 15 s for 3 minutes after the phone's last word.
* Album art: the image **id** is taken from `mpris:artUrl` (or the Web API's album images)
  — `https://i.scdn.co/image/<hex>` or the older `open.spotify.com/image/<hex>` — and the
  launcher fetches `https://i.scdn.co/image/<id>` itself, decodes the JPEG (`zune-jpeg`),
  box-filters it into 320×320 and encodes a PNG (`spotify::picture`): frame slot 0. Fetched
  when the phone says `art`, and from then on when the track changes.

### Clips

* **Before step 9**, once the game exists (`clips::prepare`): a buffer directory
  `$XDG_RUNTIME_DIR/bastion-mc/clips-<12 hex>` (memory; locked while in use; unlocked ones
  older than a minute are swept), and the recorder: bubblewrap with `/usr` read-only, a
  private `/tmp`, no network, the display socket (read-only bind) and, on the session's X
  display, its cookie reduced like the game's; the buffer directory writable; executable:
  this program (as the helper) and `/usr/bin/ffmpeg`; Landlock and the game's seccomp
  policy from the same init stage; with resource limits on, a systemd scope of its own
  (1 GiB, 256 tasks, one CPU) — asked for, not read back.
* The helper (`bastion-mc __clip-recorder`, inside that sandbox) speaks just enough X11
  (`clips::x11`) to find the game's window: with the nested compositor, the one viewable
  window on the inner display; on the session's display, the window whose client is the
  game's process (X-Resource), or — where the server cannot see processes — the one that
  names the sandbox's host name (`bastion`) as its machine and process 2 as its owner. It runs
  ffmpeg on it (`x11grab -window_id`: the window's own pixels; libx264 veryfast/zerolatency,
  CRF 23, at most 6 Mbit/s, a keyframe a second; scaled into 1280×1080 at most; the segment
  muxer with one-second MPEG-TS segments, a wrap of `buffer_seconds + 6` files and a CSV list
  of the newest `buffer_seconds + 2`; a 320-px JPEG thumbnail every 2 s, written atomically).
  When the window changes size it starts ffmpeg afresh (at most five times a minute, then it
  gives up and says so). It reports one-word status lines, read only after step 9.
* `confinement_needs`: the clips folder and the buffer directory (write). Nothing else.
* **Saving** (after step 9, on a blocking thread; `clips::save_blocking`): waits up to 1.5 s
  for the segment being recorded to complete, reads the list strictly, opens the newest
  segments covering the request by name relative to the buffer directory (no links, regular
  files, at most 64 MiB each, whole 188-byte packets, sync bytes), concatenates them into
  `.<stem>.ts.part` (`O_EXCL|O_NOFOLLOW`) renumbering every stream's continuity counter,
  syncs, renames it to `<stem>.ts` (`RENAME_NOREPLACE`), and writes `<stem>.png` from the
  recorder's latest thumbnail, decoded and re-encoded. The index (`.bastion-clips.json`)
  gets the entry; then the caps are applied (oldest of this feature's clips first, each
  removed only if it is still a regular file of the recorded size).
* The app: `status`, `save [seconds]` (1 – buffer, default the whole buffer; plus
  `saves_per_minute`), `list` (newest 32), `delete <id>`, `rename <id> -- <text>` (letters,
  digits, `_`, `-`, spaces; 48 characters; the files follow, never replacing one),
  `thumb <id>` (slot = its place in the last list; only a PNG within 320×320 is sent).

## Threat model (who can do what through each new channel)

**The game, through the phone (both apps).** Everything the phone sends is the game's:
hostile by assumption. Through `spotify` it can control the user's playback (the desktop
app, or any Spotify device via the Web API), search, queue, move playback to one of the
user's listed devices, and read what is playing, the user's Spotify device names and search
results — within the dispatcher's limits and the app's own budgets. It cannot get a token,
a URL, a path or an image id it could use elsewhere; it cannot open anything but the six
URI kinds; it cannot name a device the launcher did not list. Through `clips` it can save
(bounded), list, view thumbnails of, rename and delete **clips this feature made** — so it
can delete those clips — and fill the clips folder up to `max_total_mb`. It never names a
path: a clip is a 12-digit id the launcher made up.

**The session bus (through xdg-dbus-proxy).** Whatever owns `org.mpris.MediaPlayer2.spotify`
— normally the Spotify app, but any program of the user can take the name — can send the
launcher replies and signals: parsed strictly (`spotify::dbus`, fuzzed; 1 MiB, 65 536
values, depth 32), and what reaches the phone is cleaned text (no control or bidirectional
characters, bounded) and numbers. Such a program can show the phone made-up titles; it
gets nothing back but the commands above. The launcher itself can reach only the player and
properties interfaces of that one name on that one path — the proxy refuses the rest
(`tests/spotify.rs` checks `Quit`, another object and another name). The proxy has the
session bus (it is the filter); it parses only bus traffic, never anything of the game, and
runs in a sandbox of its own. Residual: a compromised proxy has what any program of the
user has on the session bus.

**The Spotify Web API.** Requests go only to `accounts.spotify.com`, `api.spotify.com` and
`i.scdn.co` (`HttpClient::only`), TLS 1.3, no redirects, bounded bodies, through the
launcher proxy if set. A hostile game can make the launcher send requests in the user's name
within the budgets (a few per minute per class): Spotify sees them as the user's app.
Replies are parsed into typed values and cleaned (`spotify::web`, fuzzed).

**The token keeper.** The refresh token lives in the keeper (unconfined: it needs the
keyring or `age`), not in the launcher, so the launcher's promise that it holds no refresh
token while the game runs stays true. A compromised launcher can ask the keeper for fresh
access tokens — which it holds anyway — at most every 20 s and 60 times a session; it cannot
get the refresh token. The keeper reads only the launcher's one-byte requests and Spotify's
token replies (parsed strictly).

**Album art.** Fetched only when the phone asks; Spotify's CDN learns the address and which
cover — what the desktop app has just fetched too. Off with `album_art = false`, and in
burner sessions. The JPEG is decoded in the launcher (`zune-jpeg`, portable decoder only;
fuzzed): only pixels reach the game.

**Clips: the recorder.** It processes what the game draws. A recorder compromised through
ffmpeg could: read the display it was given — with the nested compositor, the game's own
display (nothing of the user's); **on the session's X display, the whole X session, like
the game itself in that mode** (X11 has no isolation; the game already has the same socket);
write the buffer directory (bounded by the scope's memory limit where scopes are on); and
nothing else (no network, no home directory, no other program). What it writes is read by
the confined launcher only through the strict list parser, the packet checks and the JPEG
decoder (fuzzed). **Residual: the clip's bytes are passed through** (the launcher checks
packet structure, not the video): a compromised recorder could put a file into the clips
folder that attacks the video player the user opens it with.

**Clips: what is recorded.** With `-window_id` the recording is the game window's own
pixels. With a compositing window manager, other windows that overlap it are not recorded;
without one, X11 returns what is on screen in the window's area, which can include
overlapping windows. With the nested compositor nothing else is on that display. No sound.

**The confined launcher** gains two writable directories (the clips folder, the clip
buffer) and TCP 443 when the Web API or album art is on — `Service::confinement_needs`,
logged in the confinement line. It executes nothing for either feature after step 9 (the
recorder, the proxy and the keeper were started before; saving is concatenation).

## Verified by running (2026-11-14, this machine)

* `cargo test --lib`: 30 Spotify unit tests (D-Bus marshalling both byte orders, malformed
  and deep messages, signatures, names; MPRIS property cleaning, art ids, signal origin
  checks; Web API state/devices/search/token/error parsing, Premium/no-device, budgets and
  429, query encoding; keeper reuse/rate limits/wire format; pictures; URIs; config) and 16
  clips unit tests (list parser, joiner, names, index, caps, rename, X11 replies, status
  lines, ffmpeg arguments, config, the script driver).
* `tests/spotify.rs` (10 tests) — **against stand-ins only**: a private `dbus-daemon` (own
  config and socket) and `tests/support/mpris-standin.py` (GDBus, an independent D-Bus
  implementation) owning `org.mpris.MediaPlayer2.spotify`; the real `xdg-dbus-proxy` in its
  bubblewrap sandbox: properties, all transport commands, `SetPosition`, `OpenUri`, signals,
  the filter (root interface, other object, other name refused; another player neither
  listed nor reachable), the app quitting and returning; a **confined child** (Landlock +
  seccomp, `Demand::Whole`) that drives the player after confining itself and cannot open a
  new bus connection. The Web API against a stand-in HTTPS server (hyper + rustls, a test CA
  trusted only by the test's client, a certificate for the three hosts, scripted replies):
  `200`, `204`, `401` → one refresh with the rotated token, `403 PREMIUM_REQUIRED`,
  `404 NO_ACTIVE_DEVICE`, `429` with `Retry-After` (nothing sent during the pause),
  request shapes (bodies, query values, `content-length: 0`), local limits; the sign-in
  (URL, simulated browser callback on the loopback port, code exchange whose verifier
  hashes to the challenge); the whole spotify app (MPRIS while the stand-in runs, cover
  fetched from the stand-in image host and re-encoded to 320×320, the next cover on a track
  change, devices, a made-up device refused, then the Web API with Premium refused); the
  keeper as a real process of the binary (with `token_store = "none"`: it answers "not
  signed in" and ends with its socket); `bastion-mc spotify status` through the proxy.
* `tests/clips.rs` (6 tests): saving from segments ffmpeg made with the recorder's settings
  (ffprobe: H.264 480×270, duration within 0.25 s, frames within 2, a clean decode of the
  joined stream; thumbnail 320×180); a damaged buffer (a link, non-packets) refused with
  nothing left behind; a **confined child** (baseline + exactly the service's two
  directories, `Demand::Whole`) saves a 4 s clip and can neither write elsewhere (EACCES)
  nor execute; the clips app's verbs and every limit; and — opt-in, `BASTION_CLIPS_DISPLAY=:84`
  — **the recorder sandbox recording an ffplay window on my private display** (sway
  Xwayland): found the window, clip 4.0 s / 120 frames / 1276×696.
* `tests/clips_game.rs` (opt-in) — **the real game**, Minecraft 26.3 in demo mode, on my
  private display :84, `memory_mb = 3072`, stopped by the test: once through the **nested
  compositor** (gamescope 3.17) and once on the **session's X display** (direct X11; the
  window found by the sandbox signature since that X server cannot see processes; the
  recorder restarted when sway resized the window). Both: the launcher confined itself
  whole (`[protection] launcher On`) with the clips folder and buffer added; the phone
  script (through the confined launcher) saved an 8 s clip — ffprobe 8.0 s, 240 frames,
  480×270 (nested) / 1276×696 (direct) — thumbnail 320×180 / 320×175, rename, `save 999`
  refused, buffer segments gone afterwards; a frame from the middle of each clip shows the
  game (pixel spread 37 / 34; inspected by eye: the title screen). MPRIS through the proxy
  in the real launch: `volume`, `next`, `open` reached the stand-in after step 9. The direct
  run found a real bug (a restarted ffmpeg refused to overwrite the last thumbnail), fixed.
* Fuzz harnesses `dbus`, `spotify`, `keeper`, `picture`, `clips`, `x11` (src/fuzzing.rs,
  tests/robustness.rs, fuzz/fuzz_targets): 4000 mutations each per test run; once 150 000
  each with seeds 7 and 99 (that run found a harness false positive — NaN in a value
  comparison — fixed by comparing bytes).
* `cargo deny check` with `zune-jpeg` 0.5.15 (+ `zune-core` 0.5.3): advisories, bans,
  licences, sources ok. `cargo fmt --all`, `cargo clippy --workspace --all-targets`: clean.

## Not verified

* **The real Spotify desktop app** — never started (it is the user's). Its MPRIS behaviour
  (Spotify's `mpris:length` type, `Position` accuracy, `Volume`/`LoopStatus` support,
  `OpenUri`) is assumed from documentation and the stand-in, which imitates it.
* **The real Spotify Web API and accounts service** — blocked on the user: it needs their
  client ID (and a Premium account for playback control). Whether Spotify rotates or keeps
  PKCE refresh tokens is handled both ways but not observed.
* **Album art from the real `i.scdn.co`** — not fetched (the real-game test runs with art
  off; the art path ran only against the stand-in).
* **The Spotify token in the real kernel keyring or age vault** — `TokenStore::*_spotify`
  never ran against the keyring: its one fixed name would touch a real user entry. The
  scram removal of that key is unit-tested for parsing only; scram's tests cannot reach it
  (they run limited to named accounts).
* **X-Resource PID matching on a host X server** (Xorg/KWin): untested — the only X servers
  here run in their own PID namespace and report PID 0; the fallback (sandbox signature)
  ran. A reparenting window manager (frames) is handled in code, not tested.
* **The dispatcher** (mod-platform's protocol 2) and **the phone UI** (mod-ui): my apps
  were driven through `phone::Outbox` directly and through the launch's script driver.
* The recorder's resource scope: requested; its limits are not read back.
* Sessions longer than an hour (an in-session refresh through a real keeper).

## Contract differences and integration notes

* **State extras** (the phone ignores unknown fields): `spotify` adds `uri`, `device` (the
  Web API's active device name), `art` (a cover can be asked for), `web` (`on`/`off`),
  `desktop` (`running`/`not running`/`unavailable`); `clips` adds `recorder`
  (`starting`/`waiting`/`recording`/`stopped`/`off`), `window` ([w, h]), `count`,
  `total_bytes`. Types: `created` is unix seconds, `seconds` a number with one decimal,
  `folder` is shown as `~/…` (or `…/<name>` outside home) — never the real path, which names
  the user.
* `art` sends frame slot 0 when asked; afterwards the app sends the new cover by itself on a
  track change. `save` without an argument saves the whole buffer.
* **launch.rs** (my blocks): a shared `services: Vec<Box<dyn phone::Service>>`, Spotify
  prepared after the log opens, clips after the game exists (before step 9),
  `confinement_needs` folded into `keep` (and named in the confinement log line), and after
  step 9 `into_app` → `phone_apps`, **held for the session until the dispatcher takes
  them** — or driven by `ClipsConfig::script` (test support, `#[serde(skip)]`). At the end
  the apps are dropped (the recorder, proxy and keeper stop with them).
* Small additions in shared files: `nested::Session::socket()`; `HttpClient::only()` (and
  the `BUILTIN_HOSTS` doc); `msa::parse_callback_at` (path-aware; `parse_callback` keeps
  its behaviour), `msa::read_head`/`page` made `pub(crate)`; `TokenStore::{has,load,save,
  delete}_spotify` and `store::spotify_keyutils_remove`; scram removes the Spotify key (full
  runs only); `main.rs` dispatches `__clip-recorder` (before hardening, inside its sandbox)
  and `__spotify-keeper` (after hardening); `config.rs` `[spotify]`/`[clips]`;
  `examples/config.toml`; cli `spotify login|logout|status` (logout is beyond the brief's
  `login|status`: the one way to forget the token short of scram).
* Docs the integrator may want to adjust: `confine.rs`'s header (the confined launcher now
  also reads the clip buffer and writes the clips folder when clips are on, and keeps a D-Bus
  connection to the proxy); `auth/mod.rs` (a token keeper process exists during a session
  with the Web API on).
* Dependency: `zune-jpeg` (reason in Cargo.toml: JPEG album art and thumbnails must be
  decoded and re-encoded; portable decoder only).

## Open

* Live Web API check with the user's client ID (`bastion-mc spotify login`, then a launch
  with the phone): blocked on the user.
* Exercise the Spotify token in the real keyring once with the user's consent.
* XRes matching on the user's KDE/X11 session (direct X11 mode is not the default there:
  gamescope is installed, so the nested path — verified — is what runs).
* VAAPI encoding (not done; x264 only).
