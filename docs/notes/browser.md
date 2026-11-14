# The phone's web browser (round 4, package "browser")

The phone in the game has a browser: the installed Firefox, headless, in a sandbox of its own,
able to show only the sites listed in `[browser] sites` — Discord first — each within budgets
for requests, API calls, connections and bytes. You sign in outside the game, in a window on
your desktop; the game never sees a password, a cookie or a token.

Code: `src/browser/` (config, relay, budget, sandbox, stage, ws, bidi, driver, app, login,
mod), wiring in `src/launch.rs`, `src/cli.rs`, `src/config.rs`, `src/main.rs`, `src/lib.rs`,
`examples/config.toml`; fuzz harnesses in `src/fuzzing.rs`, `tests/robustness.rs`,
`fuzz/fuzz_targets/{websocket,bidi,browser_command}.rs`; tests in `tests/browser.rs`,
`tests/browser_game.rs`, `tests/support/{browser_sites.rs,x11-chord.py,x11-shot.py}`.

## 1. User text (for the README)

### The phone's browser

The phone (Fabric instances with the companion mod) has a web browser. It shows sites you
list in `config.toml`, and nothing else:

```toml
[browser]
enabled = true
sites = [
  { preset = "discord" },
  { name = "wiki", title = "Minecraft Wiki", start = "https://minecraft.wiki/",
    hosts = ["minecraft.wiki", "*.minecraft.wiki"] },
]
```

* At most 8 sites; each names the host names its pages may load from (`example.com`, or
  `*.example.com` for every name below it), up to 32. The phone opens a site by its name,
  at its `start` page; links that lead to a host the site does not list are refused (the phone
  says so). Only `https://` pages.
* The built-in `discord` preset is `discord.com` and the hosts Discord's web client needs
  (`*.discord.com`, `discord.gg`, `*.discord.gg`, `discordapp.com`, `*.discordapp.com`,
  `*.discordapp.net`, `hcaptcha.com`, `*.hcaptcha.com`). Preset entries may rename the site,
  retitle it and change its limits; for a different host list, list the site yourself.
* Limits per site (defaults): `requests_per_minute = 6000` (everything its pages load;
  Discord's client loads about 700 files when it starts), `api_calls_per_minute = 300`
  (fetch/XMLHttpRequest/WebSocket/beacon calls — sending a message is one),
  `connections_per_minute = 120`, `megabytes_per_minute = 60`. Requests, calls and
  connections beyond a limit are refused until the minute's allowance refills (the phone
  says so); bytes beyond it are slowed down, not cut.
* `frame_rate` (1–15, default 10) is how many pictures a second the phone gets — only while
  it shows the browser and only while the page changes; `quality` (30–90) is their JPEG
  quality; `width`/`height` (160–1280) the page size it starts with (the phone may change it);
  `memory_max_mb` (default 2048) the browser's own memory ceiling where the game's resource
  limits are in use.

**Sign in outside the game**, never on the phone:

    bastion-mc browser login discord

opens a Firefox window on your desktop (Wayland or X11) in the same sandbox and with the same
profile. Sign in there and close the window: the in-game browser starts with that session.
The window, too, reaches only the site's hosts. Firefox does not offer to save your password
(the password manager is off), so the profile keeps the site's session, not your password.
Ctrl-C instead of closing the window stops it without recording the sign-in.

**What does not work**

* **Voice and video calls** (Discord's included): there is no microphone and no camera in the
  browser's sandbox, WebRTC is switched off, and UDP is not relayed at all.
* **Sites that are not listed**, and hosts a listed site does not name — including the
  third-party content a site embeds (on Discord: YouTube/Spotify/Twitch players, GIFs Discord
  loads straight from Tenor, payments). Add a site of your own if you want such hosts.
* Uploading files (the sandbox has none to choose), downloads you could open (they go to the
  sandbox's private `/tmp` and vanish), browser extensions (blocked by policy), developer
  tools, passkeys/security keys in the sign-in window (no USB devices in the sandbox), and
  more than one browser at a time (one profile; a second launch or a `browser login` while one
  runs is refused with a message).
* Signing in on the phone is possible in principle (it is a web page), but do not: what you
  type on the phone passes through the game.
* Sites can tell that the in-game browser is remote-controlled (`navigator.webdriver` is true
  while Firefox's remote control runs; Firefox 159 offers no way to hide it). Sites that
  distrust automation may ask for more captchas there; the sign-in window has no remote control.

**What the game can do with it.** The phone is code in the game, so the game can do what you
can do on the phone: open the listed sites, see what they show, click, scroll and type in them
— in a page where you are signed in, that is acting as you on that site, within the site's
budgets. It cannot open anything else, cannot read the browser's profile, its cookies or the
site's token (nothing the browser sends the phone contains them: pictures, the host name, the
page title and short notices), and cannot reach your files or your desktop through it. A site
that can be made to show its own secrets on screen (an XSS hole in it, a page that runs code
you type) is beyond what the launcher can prevent.

## 2. How it works

```
 phone (game) ─companion channel─▶ dispatcher ─▶ BrowserApp ─▶ driver ──WebDriver BiDi──┐
                                                                                         │
 launcher (confined after step 9)                       bridge: socket pair, held from   │
   relay: DNS (names of the OPEN site only)             before step 9; the stage copies  │
          HTTPS by SNI, per-site budgets  ◀─127.0.0.1:53, 0.0.0.0:443 in the browser's   │
                                            network namespace (sockets made by the       │
                                            namespace helper, handed over)               │
 browser sandbox (bubblewrap): stage ─fork─▶ Firefox --headless --remote-debugging-port ─┘
   /usr ro, synthetic /etc (+ enterprise policies), /profile = <data>/browser/profile (rw),
   /tmp, /dev/shm private; no home, no display, no D-Bus, no sound, nothing of the game's
```

* **Started before step 9** (`browser::prepare`, called in `launch.rs` after the log is open
  and the resource-limit decision is made, before the game's sandbox starts): the network
  helper (`__netns-exec`, as for the game's filtered network) creates the namespace and the two
  sockets, bubblewrap builds the sandbox, `bastion-mc __browser-stage` hardens itself and starts
  Firefox. Not for `--burner` sessions (the game there runs untrusted content; a signed-in
  browser is not put within its reach).
* **The stage** (`stage.rs`): `no_new_privs`; a fresh, empty session keyring (the inherited one
  leads to the user's keyring, where the refresh token can be); no core dumps; Landlock (read
  `/usr`, `/etc`, `/proc`; execute only Firefox's directory and the loader; write the profile,
  `/tmp`, `/dev/shm`, the runtime directory; TCP connect 443 and the remote-control port, bind
  only that port; signals and abstract sockets scoped); a seccomp **denylist** with `EPERM`
  (keyring calls, `ptrace`, `process_vm_*`, `pidfd_getfd`, `bpf`, `perf_event_open`,
  `userfaultfd`, `io_uring`, every mount call, `chroot`, `setns`, `unshare` and `clone` with a
  namespace flag, modules, `kexec`, …, the terminal-injection ioctls; `clone3` and all x32
  numbers `ENOSYS`). It is a denylist because Firefox installs **its own** seccomp sandbox on
  its content processes (it needs `seccomp` and `prctl`), which keeps working: it runs without
  user namespaces, as under Flatpak. Then it forks Firefox and copies bytes between its standard
  input (the launcher's socket pair) and Firefox's remote-control port on the sandbox's
  loopback, which the launcher outside the namespace could not reach.
* **The relay** (`relay.rs`), from step 10 on: answers name lookups only for hosts of the site
  that is **open** (synthetic addresses from 127.64.0.0/10, `egress::FakeIps`; AAAA/HTTPS
  records empty, so no ECH and the SNI stays readable); takes a TCP connection only for a
  synthetic address it handed out, whose name is still the open site's, within the site's
  connection budget, and whose TLS ClientHello names exactly that host (`egress::sni`); dials
  the host (public addresses only, or through `network.launcher_proxy`); copies bytes, taking
  each chunk from the site's byte budget; and cuts every connection of a site when the phone
  closes it or opens another. TLS stays end to end between Firefox and the site.
* **The driver** (`driver.rs`, `bidi.rs`), from step 10 on: WebSocket handshake over the
  bridge, `session.new` (prompts dismissed), the page focused, events subscribed,
  `network.addIntercept` on every request. Each paused request is decided by
  `bidi::decide`: `data:`/`blob:`/`about:` go on; anything else must be `https:`/`wss:` to a
  host of the open site, within the site's request budget (and, for API calls — destination
  empty or initiator fetch/xmlhttprequest/websocket/beacon/eventsource — the API-call budget);
  the rest is failed (`network.failRequest`), and a refused navigation of the page the phone
  shows is said on the phone. New windows are closed (links open in the same page), dialogs
  dismissed. Frames: `browsingContext.captureScreenshot` as JPEG, at most `frame_rate` a second
  (≤ 15), only while the phone asked (`frames on`) and a site is open; identical captures are
  not sent and captures thin out to one a second while nothing changes; input or navigation
  brings the full rate back. The title is read from an isolated realm (`sandbox: "bastion"`)
  after loads and input and every 2 s.
* **The app** (`app.rs`): the contract's vocabulary exactly (`open close back forward reload
  size mouse scroll key text frames`), each word checked (`BrowserCommand::parse`), with per-verb
  limits (`LIMITS`); it never blocks (a full queue drops the command and says so).
* **The profile** is the browser's only state: `<data>/browser/profile`, held under an
  exclusive lock while a browser uses it; `user.js` (written at every start: background
  services off, DoH off, HTTP/3 off, ORIGIN-frame and hostname coalescing off, HTTPS-only,
  WebRTC/camera/microphone/geolocation/notifications/push off, password manager and form
  history off, links in the same tab, dark colour scheme) and
  enterprise policies in the sandbox's `/etc/firefox/policies/policies.json` (extensions
  blocked, updates/telemetry/studies off, password manager locked off, DoH locked off).
* **At the end** of the launch the driver asks Firefox to close (`browser.close`, it writes its
  profile), then the sandbox is made to end; the relay's per-site summary goes into the log.
* **Resource limits**: where the game runs in a systemd scope (`sandbox.cgroup`), the browser
  gets one of its own (`memory_max_mb`, no swap, 1024 tasks), read back from the kernel like the
  game's and said in the launch log.

The confined launcher keeps nothing for the browser except TCP port 443 (or the proxy's port)
to reach the sites; the profile is not among the paths it may read after step 9.

## 3. Threat model (new channels; who can do what)

Actors as in docs/THREAT_MODEL.md: G = code in the game (mods, a server's content), W = a web
page of a listed site (or whoever controls one), F = a compromised Firefox (W plus a browser
exploit), U = other programs running as you.

| channel | who | can | cannot / bounded by | residual |
|---|---|---|---|---|
| phone → browser app (companion channel, protocol 2) | G | open listed sites; see frames, host, title, notices; click/scroll/type in the open page (as you, if signed in); spend the site's budgets; change the page size | name an address, a URL, a path or code (fixed vocabulary, every argument checked); open unlisted sites or hosts; read profile/cookies/tokens; open devtools or extensions; exceed per-verb rates and per-site budgets | everything a person at that page could do (send messages, join servers, change settings that need no password); a site with an XSS or code-running feature could be made to display its secrets in a frame |
| browser → network (relay) | W, F | reach the open site's hosts over HTTPS, within the connection and byte budgets | reach any other host (DNS answers only those names; no upstream lookups — no DNS exfiltration; no UDP; no route); reach private/loopback addresses (public only); present another name in the TLS SNI than the resolved one | F can put any bytes inside TLS to an allowed host, including an HTTP `Host` other than the SNI on CDNs that accept that ("domain fronting"; the driver refuses such requests, a compromised browser need not ask it); the hostnames a site lists with wildcards are trusted as a whole |
| browser → launcher (bridge: WebSocket + BiDi JSON + JPEG; relay: DNS queries, ClientHellos) | W, F | send arbitrary bytes on these channels | the launcher reads them only after confining itself (step 9: no program execution, no files beyond persist/ and cache, TCP 53/443/proxy); every parser is bounded and fuzzed (`websocket`, `bidi`, `dns`, `sni` harnesses); a message is at most 16 MiB, a frame at most 4 MiB, titles 128 characters, notices 256 bytes | a parser bug would be in the trusted base (as for the game's relay), inside a confined launcher |
| browser sandbox → host | F | read `/usr`, write its profile and private tmpfs; run code as you inside the sandbox | home directory, other files, the game's sandbox, display, D-Bus, sound, the kernel keyring (fresh session keyring + seccomp), new namespaces, ptrace, mounts, program execution outside Firefox's directory (Landlock); memory beyond its scope | it can change its own profile (site data, service workers), which persists into later sessions (`user.js` is rewritten at every start; extensions are blocked); kernel bugs reachable from a seccomp-filtered process |
| the profile at rest | U | read the site sessions in `<data>/browser/profile` (like any browser profile) | — (G cannot: it is not in the game's sandbox — checked in a real launch; the confined launcher cannot after step 9) | `bastion-mc scram` does not wipe it (see open items) |
| `browser login` window | W, F, U | a normal Firefox window on your display, reaching only the site's hosts | the game is not involved | on **X11** the window is an X client like any other (X has no isolation between clients); on Wayland the compositor mediates |
| remote-control port inside the browser sandbox | W | — | pages cannot use it: requests to `127.0.0.1` are refused by the driver (not `https:`/listed), Firefox's remote agent refuses requests with an `Origin`, and only one BiDi session exists (the launcher's) | F, already inside, gains nothing from it |

## 4. Verified by running (2026-11-14, this machine: Linux 7.2.14, Firefox 159.0.1)

* `cargo fmt --all` clean; `cargo clippy --workspace --all-targets` no warnings;
  `cargo test --workspace` (DISPLAY/WAYLAND_DISPLAY unset, XDG into `target/xdg`): 622 passed,
  0 failed, on the final tree (log `target/browser-logs/workspace-test.log`). New unit tests: 28 in `src/browser`
  (config 6, bidi 6, relay 4, sandbox 3, ws 3, budget 2, app 2, stage 2), robustness 4.
* **Relay against stand-in sockets** (unit tests, host loopback): names resolve only for the
  open site; wildcard below-only; AAAA/HTTPS empty; SNI mismatch, non-TLS and unknown synthetic
  addresses refused; connection budget refuses the fourth of three; a 1 MiB/min byte budget
  slowed 64 KiB over the burst to about 4 s; closing the site cut an open connection.
* **Real Firefox in its sandbox against stand-in HTTPS sites** (`tests/browser.rs`,
  `BASTION_BROWSER_TESTS=1`; test certificate authority added to the profile with `certutil`,
  sites served by the test with TLS 1.3): 6/6 passed (log `target/browser-logs/browser-optin.log`):
  - open → host and title in the state; frames on → 800×600 JPEG; typing `héllo ☕` reached a
    tapped input; a click at (400,350) hit a button; `ctrl-a`, text, `Enter` submitted a form
    (the stand-in received `/sent?msg=ok`); back; `size 400 700` → 400×700 frames; a page's
    own report: `prefers-color-scheme: dark` true, `navigator.webdriver` true (see §1); close.
  - a full-page link to an unlisted host: the phone was told `refused: evil.example is not part
    of Chat`, the page stayed; an image from an unlisted host refused, from a host below the
    site's wildcard loaded; the stand-in never saw a request for an unlisted name.
  - budgets: with 6 requests/min the stand-in saw ≤ 6 requests of a 12-image page and the phone
    was told; with 5 API calls/min exactly 5 of 12 `fetch` calls arrived (the page itself is not
    an API call).
  - frames: an animated page gave 12.0–13.5 frames/s at `frame_rate = 15` over four runs (never
    two closer than 50 ms); a static page gave 0 frames in 4 s after its first; a phone that
    connected again got a picture at once although nothing had changed; `frames off` gave none.
  - **sign-in window on the private display :83** (headless sway + Xwayland,
    tests/support/private-x11.sh): the window rendered (picture checked), the stand-in's
    sign-in page set a session cookie, Ctrl+Q (XTEST, `x11-chord.py`) closed it, the CLI path
    returned 0 and recorded the sign-in; the headless browser then started with the same profile
    and the stand-in received the cookie (title `signed in as tester`, `signed_in_hint` true).
  - **internet, no sign-in** (`BASTION_NET_TESTS=1`): `https://example.com/` rendered
    ("Example Domain"); `https://discord.com/app` rendered Discord's login page with its QR code
    in the sandbox (1000×700 frame, ~41 KB; looked at) through the relay; it used exactly
    `discord.com` and `remote-auth-gateway.discord.gg`.
* **The real CLI** on :83: `bastion-mc browser login example` (XDG in `target/xdg-cli`) showed
  example.com in a dark-themed window (picture checked); Ctrl+Q → exit 0, sign-in recorded.
  Also on the same compositor's **Wayland** socket: the window was a native `xdg_shell` client
  (`app_id` firefox, per swaymsg), closed by the compositor → exit 0. Error paths: unknown site,
  no display, unknown subcommand → exit 2 with a message.
* **Real game, demo mode** (`tests/browser_game.rs`; data root `target/e2e`, instance
  `vanilla-latest` = Minecraft 26.3, `memory_mb = 2048`, display :83 through the nested
  gamescope; log `target/browser-logs/browser-game.log`): the launch log shows Firefox started
  before `launcher: confined itself … TCP only to port(s) 53, 443, <test port>`, the browser's
  own limits in force (2048 MiB, no swap, 1024 tasks; read back), `browser: ready` after the
  confinement, the self-test (`[browser] self_test`, through the real `PhoneApp` interface)
  opening the site through the relay and getting an 800×600 frame, and `browser: closed` at the
  end. From outside, through `/proc/<java pid>/root` (inspected by a helper started before the
  confinement), the whole game root except `/proc`, `/sys`, `/dev` and the host's `/usr` was
  walked (5560 paths): the profile path does not exist there and no profile file
  (`cookies.sqlite`, `user.js`, `cert9.db`, `key4.db`, a marker file) was found.
* **Firefox's own sandbox stays on**: during a test, Firefox's main and fork-server processes
  had our 2 seccomp filters, its content, socket, RDD and utility processes 3 (ours and
  Firefox's own); Firefox logs `CanCreateUserNamespace() clone() failure: EPERM` (expected: it
  runs without user namespaces).
* Fuzzing: `cargo test --test robustness` (every run), and 200 000 mutations per new harness
  (`BASTION_FUZZ_ITERATIONS=200000 BASTION_FUZZ_SEED=7`, log
  `target/browser-logs/robustness-200k.log`); the fuzz crate checks (`cargo check --offline` in
  `fuzz/`, all targets including the three new ones).

## 5. Verified only against stand-ins, or not at all

* **Not with a signed-in Discord**: no account was used (the brief forbids it). Discord's
  signed-in client (gateway, CDN, media proxies) was not run; its hosts come from Discord's own
  page configuration (`GLOBAL_ENV`: API, gateway, CDN, media proxy, image proxies, remote-auth)
  and content security policy, read from `https://discord.com/app` without signing in
  (`curl -A curl`). Whether 300 API calls/min and 60 MB/min suit a busy Discord session is a
  guess from the defaults, not a measurement.
* **The phone itself**: the dispatcher and protocol 2 belong to mod-platform; in this tree the
  launch holds the app (`_browser_app`) without routing commands to it. The app was driven
  through its `PhoneApp` interface (tests, and the self-test inside a real launch), not from the
  game.
* Captcha (hCaptcha) on sign-in, two-factor prompts, passkeys: not exercised.
* Firefox versions other than 159.0.1; `firefox-esr`/developer edition paths (listed, not
  tested); a SOCKS5 `launcher_proxy` for the browser's relay (code path shared in shape with the
  game's, not tested here).
* The libFuzzer targets were not run (no nightly toolchain on this machine).
* Landlock's effect on Firefox was not probed separately: the stage refuses to start Firefox
  unless the kernel enforces the ruleset, and Firefox ran; best effort per right (rights the
  kernel lacks are dropped, never below ABI 6).

## 6. Contract differences (for the integrator)

* `BrowserService` has two methods besides `Service`: `stopper()` (a graceful end at the end of
  a launch: `browser.close`, then the sandbox; dropping the service kills it instead) and
  `is_running()` (the launch warns when the browser was enabled but could not start; the app then
  answers the phone with `"error": "the browser is not available: …"`).
* State fields beyond the contract's minimum: `ready`, `frames`, `width`, `height`, `refused`
  (requests refused so far) and per-site `signed_in_hint` in `sites`. `signed_in_hint` at the top
  level is that of the open site (a `browser login <site>` was completed for it at some point).
* `key` also accepts the lower-case names (`enter`, `pageup`, …); `mouse`'s button defaults to
  `left`; `text` with empty text is accepted and does nothing.
* Per-site limits: `api_calls_per_minute` in addition to `requests_per_minute` (one budget for
  everything broke Discord, which loads ~700 files at start).
* `src/sandbox/mod.rs`: `Args` and `mount_args` are `pub(crate)` (the browser builds its own
  bubblewrap argument list; nothing else changed there).
* The browser's log lines (relay, driver, Firefox's own output, at most 400 lines) go to the
  launch log under `Source::Egress`.

## 7. Open items

* `bastion-mc scram` does not wipe `<data>/browser/profile`, which holds site sessions (a
  Discord token is a credential). Suggest: scram deletes it (or offers to).
* `bastion-mc doctor` does not report whether Firefox is installed or the browser's sandbox
  starts.
* The phone's `phone` app should list `browser` whenever `prepare` returned a service (also one
  that is off, so the phone can show why).
* A visible sign-in window on X11 is a client of the user's X server like any other window;
  running it in the nested compositor (as the game) would isolate it, at the cost of a second
  compositor for a short task.
