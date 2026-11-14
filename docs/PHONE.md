# The in-game phone

In a Fabric instance the launcher's companion mod gives the game a phone: press a key and it
slides up at the side of the screen, with apps for Spotify, a web browser (and Discord in
it), clips of the game, approving a Realms join, and keeping your game settings in step
across instances. Like the rest of the launcher it is pre-release.

**The phone is interface only, and it enforces nothing.** It is code inside the game, next
to every other mod, and any of them can read it, imitate it or use it. Everything that
protects you is on the launcher's side: each app can do only the few things its fixed list
of commands allows, at limited rates, and the secrets behind the apps — the Spotify tokens,
the browser's sign-ins, the Realms TOTP secret, your Microsoft token — never enter the game.
The rows for all of this are H1 to H23 and J21 to J24 of [`THREAT_MODEL.md`](THREAT_MODEL.md).

The phone works on every Fabric release from 1.20.1 to 26.3 that the companion mod is added
to ([`COMPANION.md`](COMPANION.md) says when that is). A vanilla instance has no phone.

## Pulling it out

Press **R** in a world and the phone slides up at the right of the screen; press **R** again,
or Escape on its home screen, to put it away. It also comes out on the launcher's home
screen (the game's title screen, redone). The game keeps running behind it — it is not a
pause menu, and in single player the world does not stop — and the mouse is free while the
phone is out. In a world, where the phone would cover the right end of the hotbar, it
stands on the hotbar row instead, so that hearts, food and the hotbar stay in sight.

Everything works with the mouse and with the keyboard alone: the arrow keys or Tab move the
highlight, Enter or Space uses it, Escape or Backspace goes back one step (and from the
phone's home screen puts it away). While a text field has the cursor — a search, a code, a
clip's name — keys are letters: R types an "r" there instead of putting the phone away.
Clicking a web page gives it the keyboard (a cyan frame says so); Escape takes it back.

The status bar shows the time, how many notices arrived (a short line from the launcher: a
clip saved, a join approved …) and whether the phone is connected to the launcher. When a
notice arrives while the phone is away, it peeks in at the edge of the screen for a few
seconds; a Realms join waiting for its code stays there until it is answered or expires.
The clock shows the game's time. The sandbox gives the game no time zone — no `TZ`
variable, no `/etc/localtime` — so it runs on UTC (seen in the run of 2026-11-14 described
below); `[sandbox] expose_timezone = true` gives the game your time zone, and with it tells
every mod roughly where you are.

**Why R.** No Minecraft version from 1.20.1 to 26.3 binds it by default (checked against
26.3's own list of key bindings, F3 combinations included, and against the defaults of the
1.20 and 1.21 versions), it is not one of Simple Voice Chat's keys (M, N, H, V), and it is
in the same place on QWERTY, QWERTZ and AZERTY keyboards. The phone's key is not one of the
game's key bindings: it does not appear in the game's Controls screen, it is changed only
with `phone_key` (below), and another mod that binds the same key gets the press too.

Only the apps the launcher set up for this launch appear on the phone's home screen:

| App | What you can do | When it is there |
|---|---|---|
| **Spotify** | see what is playing, with the cover; play and pause, next, previous, seek, volume, shuffle, repeat; with the Web API also search, play or queue a result, and pick the device that plays | whenever the launcher can start its D-Bus proxy to your session bus (on by default; the Spotify app need not be running) or the Web API is set up |
| **Discord** | Discord's web client, in the launcher's separate browser | with the browser on and a site named `discord` in its list (the built-in preset) |
| **Browser** | the sites you listed, drawn into the phone; clicks and typing go to the page; turn the phone sideways (the wide-screen button) to read | with `[browser] enabled = true` (without a site, or when Firefox cannot start, the app says why) |
| **Clips** | keep the last moments of the game as a video, see your clips with thumbnails, rename or delete one | with `[clips] enabled = true` |
| **Realms** | when the launcher holds a Realms join for your approval: type the code it shows (or your authenticator's), see how long it is valid and how many tries are left | in every launch except a burner session (with Realms off it says so) |
| **Settings** | whether your game settings are kept in step across instances, when that last happened, *sync now* | in every launch except a burner session |

Spotify, the browser and clips are prepared only when there is a phone to serve: the
companion mod added to the instance, with its channel to the launcher and the phone on.
Settings sync works in every instance, the phone or not. Realms approvals need the phone: a
launch without one refuses them up front (below).

In a `--burner` session: no browser (a signed-in browser is not put within reach of content
you do not trust), no settings sync and no Realms; Spotify over the desktop app only (no Web
API, no covers); clips as configured.

## The phone's own settings

Under `[companion]` in `~/.config/bastion-mc/config.toml`, and per instance in that
instance's `instance.toml`, like the companion's other keys:

| Key | Default | Values | What it decides |
|---|---|---|---|
| `phone` | `true` | `true`, `false` | The phone in the game. |
| `phone_key` | `"key.keyboard.r"` | a keyboard key by Minecraft's name for it, as in `options.txt`; the leading `key.keyboard.` may be left out (`"r"`, `"left.alt"`, `"f6"`) | The key that pulls the phone out and puts it away. The launcher checks the name's shape (lower-case letters and digits in parts separated by dots, at most 40 characters, not `unknown`, no mouse buttons); a name the game does not know leaves the phone without a key, and the game's log says so. |
| `home` | `true` | `true`, `false` | The redone home (title) screen. |
| `dark_menus` | `true` | `true`, `false` | Darker backgrounds behind the game's own menus. |

```toml
[companion]
phone = true
phone_key = "key.keyboard.r"   # "r" works too
home = true
dark_menus = true
```

`phone = false` takes the phone out of the game, and with it the services only a phone
uses: the launcher then starts no Firefox, no D-Bus proxy and no clip recorder for that
instance, and refuses Realms approvals. Settings sync goes on. To switch one service off and
keep the others, use its own setting — `mpris = false` under `[spotify]`, `enabled = false`
under `[browser]` or `[clips]`.

## The apps

### Spotify

The Spotify app controls the **Spotify desktop app on this machine** and shows what is
playing, with the cover. This is on by default and needs no account: the launcher talks to
the desktop app over MPRIS (D-Bus) through `xdg-dbus-proxy`, which runs in a sandbox of its
own and passes the Spotify app's player interface and nothing else of your session bus.
Nothing leaves the machine — except the cover: when the phone asks for it, the launcher
fetches it from Spotify's image server (`i.scdn.co`), the same image the desktop app shows,
and the phone gets it decoded and encoded again as a picture of at most 320×320.

With the **Spotify Web API** the phone can also search, add to the queue, list your Spotify
devices and move playback to one of them, and control playback when the desktop app is not
running (Spotify allows playback control through the Web API only with Premium; the phone
says so when it is missing). It needs your own Spotify application — the launcher ships no
client ID and no secret:

1. Create an app at <https://developer.spotify.com/dashboard> and register the redirect
   URI `http://127.0.0.1/callback` (Spotify then accepts the port the sign-in uses).
2. Put its client ID into `config.toml`: `client_id = "…"` under `[spotify]`.
3. Run `bastion-mc spotify login` (add `--open` to have your browser opened) and sign in.
   The refresh token goes to the launcher's token store (the kernel keyring or the age
   vault), under a name of its own. The scopes asked for are
   `user-read-playback-state user-modify-playback-state` — nothing about your account,
   library or playlists.

`bastion-mc spotify status` says what is set up and whether it answers (the desktop app
through the proxy, one Web API read; `--offline` checks only the configuration and the
store). `bastion-mc spotify logout` forgets the stored token; to revoke the grant itself, use
your Spotify account settings ("Manage apps"). `bastion-mc scram` removes the token with the
rest (in a full run).

During a launch the refresh token is held by a small process of the launcher's own, the
token keeper, which hands the launcher an access token when it asks; the launcher never holds
the refresh token while the game runs, and the game holds neither.

| Key under `[spotify]` | Default | Values | What it decides |
|---|---|---|---|
| `mpris` | `true` | `true`, `false` | The desktop app over MPRIS. With it on, every launch that serves the phone starts `xdg-dbus-proxy`, whether or not Spotify is running. |
| `album_art` | `true` | `true`, `false` | Covers from `i.scdn.co` when the phone asks for them (never in a burner session). |
| `client_id` | not set | your Spotify application's client ID: 32 lower-case hexadecimal characters | Turns the Web API on, once `bastion-mc spotify login` has stored a token. |
| `redirect_port` | not set (any free port) | 1024 to 65535 | A fixed port for the sign-in's redirect; then register `http://127.0.0.1:<port>/callback` instead. |

Needs `xdg-dbus-proxy` (`pacman -S xdg-dbus-proxy`) for the desktop app.

### The browser, and Discord

The phone's browser is the installed Firefox (`/usr/lib/firefox`, or the ESR or developer
edition), headless, in a sandbox of its own. It shows the sites you list, and nothing else.
The Discord app is the same browser, opened at the site named `discord`.

```toml
[browser]
enabled = true
sites = [
  { preset = "discord" },
  { name = "wiki", title = "Minecraft Wiki", start = "https://minecraft.wiki/",
    hosts = ["minecraft.wiki", "*.minecraft.wiki"] },
]
```

**Sign in outside the game**, never on the phone:

```sh
bastion-mc browser login discord
```

opens a Firefox window on your desktop (Wayland or X11) in the same sandbox, with the same
profile. Sign in there and close the window: the in-game browser starts with that session.
The window, too, reaches only the site's hosts. Firefox does not offer to save your password
(its password manager is off), so the profile keeps the site's session, not your password.
Ctrl-C instead of closing the window stops it without recording the sign-in. On X11 that
window is an ordinary client of your display: other programs on the same display can read
what you type into it, as they can for any window on X11.

Needs Firefox (`pacman -S firefox`).

| Key under `[browser]` | Default | Values | What it decides |
|---|---|---|---|
| `enabled` | `false` | `true`, `false` | Start the browser for launches that serve the phone. |
| `sites` | `[]` | at most 8 entries (below) | The sites the phone may open, in the order it lists them. |
| `frame_rate` | `10` | 1 to 15 | Pictures of the page per second sent to the phone — only while it shows the page, and only while the page changes. |
| `quality` | `60` | 30 to 90 | Their JPEG quality. |
| `width`, `height` | `800`, `600` | 160 to 1280 | The page size it starts with; the phone sets its own size within the same range. |
| `memory_max_mb` | `2048` | 512 to 65536 | The browser's own memory ceiling (a resource scope of its own, where the game's resource limits are in use). |

Each entry of `sites` is either a preset or a site of your own:

| Key of a site | Default | Values | What it decides |
|---|---|---|---|
| `preset` | — | `"discord"` | A built-in site: its start page and hosts are fixed. A preset entry may set `name`, `title` and the four limits, not `start` or `hosts`. |
| `name` | the preset's | 1 to 16 of `a-z`, `0-9`, `-`, not starting with `-`; unique | What the phone opens it by. The Discord app appears only for a site named `discord`. |
| `title` | the preset's, or the name | 1 to 32 characters, no control characters | What the phone shows. |
| `start` | — | an `https://` address of at most 512 printable characters, on port 443, whose host is one of the site's hosts | The page `open` goes to. |
| `hosts` | — | 1 to 32 host names: `example.com`, or `*.example.com` for every name below it (not the name itself; at least two labels after `*.`); no addresses | Where the site's pages may load from. |
| `requests_per_minute` | `6000` | 1 to 100000 | Everything the site's pages load (refused beyond). Discord's client loads about 700 files when it starts. |
| `api_calls_per_minute` | `300` | 1 to 100000 | Calls the site's scripts make — `fetch`, `XMLHttpRequest`, WebSocket, beacon (refused beyond). Sending a message is one. |
| `connections_per_minute` | `120` | 1 to 10000 | Connections the launcher's relay opens for the site (refused beyond). |
| `megabytes_per_minute` | `60` | 1 to 10000 | What the relay carries for the site, both ways together (slowed down beyond, not cut). |

The `discord` preset is `https://discord.com/app` with the hosts Discord's web client needs:
`discord.com`, `*.discord.com`, `discord.gg`, `*.discord.gg`, `discordapp.com`,
`*.discordapp.com`, `*.discordapp.net`, `hcaptcha.com`, `*.hcaptcha.com`. For a different host
list, list the site yourself. A link to a host the open site does not list is refused, and
the phone says so.

**What does not work:** voice and video calls, Discord's included (no microphone or camera
in the browser's sandbox, WebRTC off, no UDP); sites that are not listed, and hosts a listed
site does not name — including what a site embeds from elsewhere (on Discord: YouTube,
Spotify and Twitch players, GIFs loaded straight from Tenor, payments); uploading files (the
sandbox has none to choose); downloads you could open (they go to the sandbox's private
`/tmp` and vanish); extensions (blocked by policy); developer tools; passkeys and security
keys in the sign-in window (no USB devices in the sandbox); more than one browser at a time
(one profile: a second launch or a `browser login` while one runs is refused with a message).
Sites can tell that the in-game browser is remote-controlled (`navigator.webdriver` is true
while Firefox's remote control runs; Firefox 159 offers no way to hide it), so sites that
distrust automation may ask for more captchas there; the sign-in window has no remote
control.

The profile — your site sessions — is `~/.local/share/bastion-mc/browser/profile`. It is not
in the game's sandbox. `bastion-mc scram` deletes it, unless a browser is using it at that
moment (a `browser login` window): then scram says so and keeps it. Deleting it does not sign
you out at the site (Discord: Settings, Devices).

### Clips

With `[clips] enabled = true` (off by default: recording costs a CPU core) the launcher
records the game while it runs and keeps the last `buffer_seconds` in memory. The clips app
saves them as a clip — a `.ts` video (H.264, at most 1280 wide) with a thumbnail next to it
— into `~/.local/share/bastion-mc/clips` (or `folder`), lists them with thumbnails, renames
and deletes them. There is no sound in clips. When saving goes over the number of clips or
their total size, the oldest of the clips this feature saved are removed; nothing else in the
folder is ever touched.

Recording works with the nested compositor (gamescope, the default on X11 when it is
installed) and on X11 without it. It does **not** work with a Wayland display: nothing
outside the compositor can read the game's window there. On X11 without the nested
compositor and without a compositing window manager, a clip can show other windows that
overlap the game. Needs `ffmpeg` (`/usr/bin/ffmpeg`).

| Key under `[clips]` | Default | Values | What it decides |
|---|---|---|---|
| `enabled` | `false` | `true`, `false` | Record the game while it runs. |
| `buffer_seconds` | `30` | 5 to 120 | How much is kept, and the longest clip. |
| `fps` | `30` | 5 to 30 | Frames per second. |
| `max_width` | `1280` | 320 to 1280 | Width of the recording; narrower windows keep their size. |
| `saves_per_minute` | `4` | 1 to 6 | Saves allowed per minute. |
| `max_clips` | `50` | 1 to 500 | Clips kept; the oldest of this feature's go first. |
| `max_total_mb` | `2048` | 16 to 65536 | All of this feature's clips together, in MiB. |
| `folder` | `~/.local/share/bastion-mc/clips` | an absolute path without `..` | Where clips go. |

### Realms

Realms works in the filtered network once you decide how a join is approved:

```toml
[realms]
approve = "code"   # or "totp"; "off" (the default) refuses Realms joins, as before
```

* **What the game may then do with your account.** The credential broker also answers for
  Realms' hosts: the game may see your Realms list, its notifications, news and invitations,
  accept or decline an invitation (or redeem an invite code, 26.3), agree to the Realms terms
  and report region pings — with your credential, which the game still never holds.
  Everything an owner does to a Realm (configure it, switch, reset, upload, download or
  restore a world, invite, op or remove players, open, close, delete) is refused before it
  leaves your computer: manage your Realm from the official client.
* **A join needs your approval, outside the game.** When the game asks to join a Realm, the
  launcher asks Realms where its server is but does not hand that over. It shows a prompt in
  its own output — and in bastion-mc-gui, on the Launch page — with the Realm, the server
  address and, with `approve = "code"`, a six-digit code. Type that code into the phone's
  Realms app (with `"totp"`: the code your authenticator app shows). You have 120 seconds
  and 3 attempts; meanwhile the game shows "Connecting to the realm…". If you did not ask to
  join that Realm, do nothing: the join is refused.
* **One connection.** An approved join gives the game an address of the launcher's own, good
  for one connection within two minutes, which the launcher connects to exactly the server
  Realms named.
* **TOTP.** `bastion-mc realms totp-setup` makes a secret, keeps it in your token store (the
  kernel keyring by default) and shows it once, on the terminal, as an `otpauth://` URI and
  as base32 for your authenticator app (time-based, SHA-1, six digits, 30 seconds).
  `--replace` makes a new one. In `totp` mode a code is not tied to one prompt: whatever join
  waits when it arrives is approved, so `code` is the safer choice (J22 in the threat model).
* **What it needs.** A signed-in account (not demo mode), the filtered network with the
  credential broker, and the phone. A launch without a phone to type the code into — a
  vanilla instance, the companion not added, `phone = false` — refuses Realms approvals up
  front, with a warning, and Realms joins are refused for that session; so does a launch that
  cannot prepare the approvals (no TOTP secret stored, the helper that connects to a Realm's
  server does not start).
* **Known gaps.** A Realm with a resource pack probably fails to join: the pack's download
  address is not reachable from the sandbox. None of this was tried against the real Realms
  service (it needs a signed-in account with a Realm).

| Key under `[realms]` | Default | Values | What it decides |
|---|---|---|---|
| `approve` | `"off"` | `"off"`, `"code"`, `"totp"` | Whether Realms is brokered, and how a join is approved. |

### Settings

On by default (`[sync] options = true`). The settings that mean the same in every Minecraft
version from 1.20.1 to 26.3 follow you from instance to instance: video, sound volumes, mouse
and controls, key bindings, language, chat and accessibility. Change your field of view or a
key binding in one instance, and the next instance you start has it — also when it runs
another Minecraft version: where a setting changed its form between versions, it is
translated exactly as Minecraft itself translates it, or left alone.

Never synced: anything about one world or one server, the resource-pack list, privacy
choices (telemetry, server listing, presence) and this machine's devices (sound device,
graphics backend). A burner session takes no part. The phone's Settings app shows the state
and can sync during a session (*sync now*); otherwise what you changed is synced when the
game ends. The synced copy is `~/.local/share/bastion-mc/sync/options.json`. The companion
mod's own settings are in `config.toml` anyway.

| Key under `[sync]` | Default | Values | What it decides |
|---|---|---|---|
| `options` | `true` | `true`, `false` | Keep the listed settings of `options.txt` in step across instances. |

## Limits

Everything the phone sends is checked by the launcher, whoever sent it.

**The channel.** A command line is `<app> <verb> [argument]... [-- <text>]`: app and verb 1 to
16 of `a-z`, `0-9`, `-`; at most 8 arguments, each up to 64 printable ASCII characters
without spaces; free text up to 512 bytes of UTF-8 without control or bidirectional
characters; the whole line at most 1 KiB. From the game's side of the socket, all
connections together: at most 50 messages and 200 KiB a second, besides the channel's
older bounds (4 connections at once, 64 per session, 64 KiB a second each). From the
launcher: an app's state at most 64 KiB, a picture at most 4 MiB, a notice at most 256 bytes;
a phone that reads slowly gets the newest state and picture of each app and at most 32
waiting notices.

**Per app and command** — each command has a budget that refills at its rate per minute and
holds at most the number given as "at once"; a command beyond it is refused, and the launch
log says so once:

| App | Command | Per minute | At once |
|---|---|---|---|
| `spotify` | `play`, `pause`, `toggle`, `next`, `previous`, `seek` | 60 each | 10 |
| | `volume` | 120 | 20 |
| | `shuffle`, `repeat`, `open`, `queue`, `art` | 20 each | 5 |
| | `search` | 10 | 3 |
| | `devices`, `transfer` | 6 each | 2 |
| `browser` | `open`, `close`, `size` | 20 each | 5 |
| | `back`, `forward` | 60 each | 10 |
| | `reload` | 30 | 5 |
| | `mouse` | 1800 | 120 |
| | `scroll` | 600 | 60 |
| | `key` | 600 | 40 |
| | `text` | as fast as the phone sends typing (below) | |
| | `frames` | 60 | 10 |
| `clips` | `status` | 60 | 10 |
| | `save` (and `saves_per_minute`) | 6 | 2 |
| | `list` | 30 | 5 |
| | `delete`, `rename` | 20 each | 5 |
| | `thumb` | 120 | 32 |
| `realms` | `code`, `cancel` | 10 each | 3 |
| `settings` | `status` | 30 | 5 |
| | `sync-now` | 6 | 2 |

**Typing into a page.** The phone sends what you type in batches, one every tenth of a second
while you type, at most 400 bytes each, and the browser's `text` limit is set to that rate,
so that ordinary typing is not refused; the numbers are `LIMITS` in `src/browser/app.rs`.
Typing longer texts into a page was not tried in the game (the runs typed short words).

**What each command may name.** Spotify: `open` takes only `spotify:` URIs of six kinds
(track, album, playlist, artist, episode, show) with a 22-character id, `queue` only tracks
and episodes, `transfer` only a device id from the last `devices` answer, `search` 1 to 200
characters. The browser: `open` only a configured site by its name; `size` 160 to 1280
pixels either way; `mouse` a position inside the page and a button; `scroll` at most 5000
pixels; `key` one of Enter, Backspace, Tab, Escape, Delete, Home, End, PageUp, PageDown, the
four arrows and `ctrl-a`, `ctrl-c`, `ctrl-v`, `ctrl-x`, `ctrl-z`. Clips: a clip by the id the
launcher gave it (12 hexadecimal digits); a new name of letters, digits, `_`, `-` and
spaces, at most 48 characters; `save` at most the buffer's length. Realms: six digits.

**Budgets beyond the phone.** The browser: each site's requests, API calls, connections and
megabytes per minute (above). The Spotify Web API: control 30, reading 20, search 10, queue
10 and covers 20 calls a minute, 60 in all; when Spotify answers "too many requests",
everything waits as long as Spotify says (1 second to an hour). Clips: `saves_per_minute`,
`max_clips`, `max_total_mb`. Realms: a prompt lives 120 s and takes 3 attempts, one prompt at
a time, at most 20 a session, 10 s between a refusal and the next; in `totp` mode at most 10
wrong codes a session, and a code that was used is never accepted again.

## What leaves the machine

| What | Where to | When |
|---|---|---|
| Nothing (the desktop app is reached over your session bus) | — | Spotify over MPRIS |
| A request for a cover, by its image id: your address and which cover | `i.scdn.co` | when the phone asks for the cover, and from then on at each track change (`album_art = false` stops it) |
| Requests in your name: what you play, control, search for and queue | `accounts.spotify.com`, `api.spotify.com`, `i.scdn.co` | only with the Web API set up |
| Everything a browser sends to a site — with your session there, if you signed in | the hosts of the listed site that is open, and only those | while the site is open on the phone |
| Requests with your Minecraft credential, and one connection per approved join | Realms' hosts, and the server Realms names | only with `[realms] approve` set |
| Nothing (clips and the synced settings are files on this machine) | — | clips, settings sync |

The launcher's own connections — the browser's relay, the Spotify requests, covers — go
through `network.launcher_proxy` when one is set (the connection to a Realm's server does
not, like every game connection). The game's network mode does not apply to them: an
instance with `network.mode = "none"` and the browser on still reaches the listed sites
through the browser.

## What the game can see and do through the phone

* **Everything the phone shows is visible to every mod in the game:** what you listen to
  (titles, artists, covers, search results, the names of your Spotify devices), the open
  page as pictures (a Discord conversation on screen included), the page's host and title,
  clip names and the clips folder (as `~/…`), Realm names and server addresses.
* **Everything you type into the phone is visible to every mod in the game** — a search, a
  message typed into a page, a Realms code. Do not type passwords into the phone: sign in
  with `bastion-mc browser login <site>`.
* **Any code in the game can do whatever the phone can do**, at the same rates: control your
  Spotify playback and move it to another of your devices; open a listed site and click,
  scroll and type in it — in a page where you are signed in, as you, while the browser runs;
  save, rename and delete the clips this feature made; send a Realms code; start a settings
  sync. Any code in the game can also take the phone's connection over (the phone then sees
  nothing until it reconnects) and draw a fake phone or a fake prompt.
* **Secrets stay outside the game.** No Spotify token, browser profile or cookie, TOTP secret
  or Microsoft token is ever in what the launcher sends the phone; the game cannot name a
  path, an address, a URL or a command, only the fixed commands above.

What keeps this from mattering more is in [`THREAT_MODEL.md`](THREAT_MODEL.md) §4.7, row by
row, with what remains.

## How it works

**In the game.** The phone, the home screen and the dark menus are part of the companion mod
(`java/companion/src/bastion/companion/phone/`, `home/`, `theme/`, `ui/`), built only on the
mod's platform layer, which has an adapter for each family of Minecraft versions
([`COMPANION.md`](COMPANION.md)). The mod draws, takes your input and turns it into command
lines; it decides nothing.

**The channel, protocol 2.** The companion's UNIX socket to the launcher. The mod starts with
one text line, `hello 2 <mod version> <features>`; after it both directions carry frames: a
4-byte big-endian length (of the kind byte and the payload), a kind byte, the payload.

| Kind | From | Payload | At most |
|---|---|---|---|
| `0x01` COMMAND | the mod | one command line | 1 KiB |
| `0x02` DIAG | the mod | a diagnostic line, `diag <feature> <text>` | 512 bytes |
| `0x81` STATE | the launcher | the app's name, a 0 byte, one JSON object | 64 KiB |
| `0x82` FRAME | the launcher | the app's name, a 0 byte, a slot, width, height, format (PNG, JPEG or raw RGBA), the picture | 4 MiB |
| `0x83` NOTICE | the launcher | the app's name, a 0 byte, one printable line | 256 bytes |

A frame from the mod of any other kind, of length 0 or over its limit ends that connection
(said once in the launch log). The mod checks what it reads just as strictly: app names,
JSON objects bounded in size and depth, pictures decoded from memory with at most 2048
pixels a side; anything else closes the connection. The newest connection that says `hello 2` is the
phone — so that the mod can reconnect, and so that any code in the game can take it over. A
connection that says `hello 1` is served as before: logged, never answered.

**The dispatcher** (`src/phone/dispatch.rs`) routes each command to the app of that name,
refuses a verb the app does not have and a command over its verb's budget, counts every
refusal and logs each kind once. When the phone connects it sends the list of this launch's
apps and asks every app for its state. Sending never waits on the game: what does not fit
into the launcher's queue is dropped and counted.

**The services.** Each is prepared before the launcher confines itself (step 9 of a
launch), because afterwards it can start no program and open no new connection to a local
socket: Spotify's proxy and the token keeper, the browser's sandbox with Firefox, the clip
recorder, the Realms dialer. A service may add to the launcher's confinement only
directories it writes and TCP ports it connects to — the clips folder and buffer, the synced
settings' directory, port 443 or the proxy's — and the confinement line in the launch log
names them. After step 9 each service becomes an app behind the dispatcher. The launch log
says what was prepared:

```text
[launcher] spotify: desktop app (MPRIS): connected through xdg-dbus-proxy; the Spotify app is running; Web API: off (no [spotify] client_id); album art: on (i.scdn.co, when the phone asks)
[launcher] browser: Firefox 159.0.1 started in a sandbox of its own (profile …/browser/profile, no home directory, no display, nothing of the game's sandbox; network only through the launcher's relay, HTTPS to the open site's hosts); its own limits are in force (2048 MiB of memory, no swap, 1024 tasks; read back from the kernel). Sites: discord (9 hosts)
[launcher] phone: settings app — sync on, synced copy in …/sync
[launcher] phone: realms app — Realms joins are refused: [realms] approve = "off"
[launcher] clips: recording the game's window on the nested compositor's display into a 30 s buffer (30 fps, at most 1280 wide, no sound); in a resource scope of its own (1 GiB, one CPU), asked for; folder ~/…/clips
[game] companion channel #1: hello: mod version 0.1.0, protocol 2, features: zoom, phone, home, darkmenus
[launcher] phone: 5 app(s) (spotify, browser, settings, realms, clips), 1 connection(s) greeted, 27 command(s), 27 accepted
```

(From the end-to-end run of 2026-11-14, §7 E32 of the threat model; paths shortened.)

**The helpers**, each in a sandbox of its own unless said otherwise:

* `xdg-dbus-proxy` — bubblewrap with your session bus's socket and a directory to listen in,
  nothing else; Landlock and the game's system-call policy; no network. It passes calls to
  the player and properties interfaces of `org.mpris.MediaPlayer2.spotify` on
  `/org/mpris/MediaPlayer2`, and those signals, and refuses the rest.
* The Spotify token keeper (`bastion-mc __spotify-keeper`) — **in no sandbox**, with the
  launcher's process hardening: it has to reach the token store to save the refresh tokens
  Spotify rotates. It answers the launcher's one-byte requests with an access token,
  refreshes at most every 20 seconds and 60 times a session, and ends with the launcher.
* Firefox — headless, in bubblewrap with `/usr` read-only, a synthetic `/etc` with
  enterprise policies, the profile, private `/tmp` and `/dev/shm`; no home directory,
  display, D-Bus or sound; a stage of the launcher's (`__browser-stage`) that joins a fresh
  keyring, applies Landlock and a system-call denylist, and starts Firefox, whose own
  sandbox keeps working. Its network namespace has no route: name lookups and HTTPS go
  through the launcher's relay, which answers only for the open site's hosts and checks the
  server name of every connection. The launcher drives it over WebDriver BiDi and decides
  every request.
* The clip recorder — bubblewrap with `/usr` read-only, no network, the display socket, the
  buffer directory; it may run this program and `/usr/bin/ffmpeg` only. It records the game's
  window into one-second segments in memory; saving joins the newest segments, which the
  confined launcher can do without running anything.
* The Realms dialer (`__realms-dial`) — only when Realms is brokered: the host's network,
  nothing else of yours; it connects to public addresses only, at most 8 times a session,
  and hands each connected socket to the launcher.

## What was verified, and against what

On the development machine (Arch Linux, kernel 7.2.14, OpenJDK 27.0.1, Fabric Loader 0.20.1,
Firefox 159.0.1, gamescope 3.17), on 2026-11-14; the threat model's §7 has the details
(E26 to E32).

| Part | In the real game | Against stand-ins, or without the game | Not run |
|---|---|---|---|
| The phone and protocol 2 | all ten families' representatives (1.20.1, 1.20.4, 1.21.1, 1.21.4, 1.21.5, 1.21.8, 1.21.10, 26.1.2, 26.2, 26.3), with the demo service behind the phone — fake apps that log every command (E26, E27); with the real services on 26.3 once (E32) | the launcher's frame reader and dispatcher, with generated input; the phone's screens against a stand-in game: every command line checked against the launcher's parser, 5 504 frames on 13 window sizes checked for drawing outside its place, hostile states | versions of a family other than its representative; a person at the keyboard; mouse drags in the game; Wayland |
| Spotify | over MPRIS through the real `xdg-dbus-proxy` to a stand-in player on a private bus: commands from a script in the launcher (E29); from the phone, the track shown with its cover — fetched from the real `i.scdn.co` — and no command sent to the player (E32) | the Web API against a stand-in HTTPS server; the sign-in with a simulated browser; the token keeper as a real process with no stored token | the real Spotify app; the real Web API and accounts service; the token in the real keyring or vault; a session longer than an hour |
| The browser | a real launch with Firefox started before the confinement, driven through its app by a self-test, and no trace of the profile in the game's root — before the merge, and again on the merged tree once the test's switch counted as wanting the services (E28); from the phone, Discord's public login page through the relay (E32) | the real Firefox in its sandbox against stand-in HTTPS sites: open, pictures, typing, clicks, a form, refused hosts, budgets, frame rates, the sign-in window on a private display; `example.com` and Discord's login page over the internet | a signed-in Discord or any signed-in site; captchas, two-factor prompts, passkeys; other Firefox versions; the relay through a SOCKS5 proxy |
| Clips | 26.3 through the nested compositor and as a direct X11 client, from a script (E29); from the phone, a 30 s clip (E32) | joining segments that ffmpeg made with the recorder's settings; a damaged buffer refused; a confined process saving; the recorder on a private display | finding the game's window by its process on a host X server (only the fallback ran); the recorder's limits read back |
| Realms | — | the real broker against a stand-in Realms service over TLS; the real dialer and relay with a client in the game's place; the approvals, TOTP with the RFC test vectors | the real Realms service; any launch with approvals on (it needs an account; demo mode turns Realms off); `realms totp-setup` against the real keyring |
| Settings sync | 1.21.4, then 26.3, then 1.20.1, one setting carried across (E30) | merges into real `options.txt` files of three versions; translations both ways; a hostile file; concurrent updates | a version newer than 26.3 |

## What is blocked on you

These need your accounts or your consent, and were not done for that reason:

* **The Spotify Web API**: your own Spotify application's client ID, a sign-in with
  `bastion-mc spotify login`, and Premium for playback control.
* **The Spotify desktop app**: yours; the developers never started it, so its real MPRIS
  behaviour behind the phone is unobserved.
* **A signed-in Discord**, or any signed-in site: `bastion-mc browser login <site>` with your
  account.
* **Realms**: a signed-in account — which needs your Azure client ID and Mojang's approval
  of it — and a Realm.
* **The token store**: the Spotify token and the Realms TOTP secret were never written to or
  read from a real kernel keyring or age vault (that would touch your entries).

The first time you use any of these is the first real test of it.
