# browser — hand-over file

Package: the phone's web browser (round 4). Copy: `target/agents/r4-browser`, branch `work`.
Deliverable notes: `docs/notes/browser.md` (user text, design, threat model, what was verified
and how, contract differences, open items). This file is for a successor.

## State: complete as briefed (2026-11-14)

Everything in the brief is built and was run; see browser.md §4 for the runs and §5 for what
could only be checked against stand-ins. Nothing is in progress. Final tree: fmt clean, clippy
without warnings, `cargo test --workspace` 622 passed / 0 failed, the 6 opt-in browser tests
(stand-ins, internet, sign-in window on :83) and the real demo launch (`tests/browser_game.rs`)
passed on it; logs in `target/browser-logs/`. The private display was stopped at hand-in.

## Design in one paragraph

Firefox (installed, 159) runs headless in a bubblewrap sandbox of its own (`src/browser/sandbox.rs`):
own user/pid/ipc/uts/cgroup namespaces, a network namespace created by the existing
`__netns-exec` helper with exactly two sockets (DNS 127.0.0.1:53, TCP 0.0.0.0:443) handed to the
launcher, `/usr` read-only, synthetic `/etc` (incl. `/etc/firefox/policies/policies.json`),
profile `<data>/browser/profile` bound at `/profile`, no home. Inside, `bastion-mc __browser-stage`
(`stage.rs`): no_new_privs, fresh session keyring, Landlock (own ruleset; TCP connect 443+9222,
bind 9222), seccomp denylist (Firefox's own seccomp sandbox still works), then forks Firefox
(`--headless --remote-debugging-port 9222`) and bridges its stdin (a socketpair whose other end
the launcher holds) to 127.0.0.1:9222. After step 9 the launcher speaks WebSocket + BiDi over
that socketpair (`ws.rs`, `driver.rs`, `bidi.rs`) and serves the two sockets with `relay.rs`
(DNS fake IPs only for the OPEN site's hosts; TCP: SNI must equal the resolved name; per-site
connection and byte buckets; switching sites cuts connections). Requests are paused by
`network.addIntercept` and decided in `bidi::decide` (https/wss to the open site's hosts; request
and API-call buckets). `app.rs` = PhoneApp "browser", `mod.rs` = Service (`prepare`, `into_app`,
`Stopper`, limits read back), `login.rs` = `bastion-mc browser login <site>`.

## How to re-run the checks

    cd target/agents/r4-browser
    export XDG_CONFIG_HOME=$PWD/target/xdg/config XDG_DATA_HOME=$PWD/target/xdg/data \
           XDG_CACHE_HOME=$PWD/target/xdg/cache XDG_STATE_HOME=$PWD/target/xdg/state
    env -u DISPLAY -u WAYLAND_DISPLAY cargo test --workspace
    # real Firefox, stand-ins (+ internet, + login window on a PRIVATE display)
    BASTION_BROWSER_TESTS=1 BASTION_NET_TESTS=1 BASTION_BROWSER_LOGIN_DISPLAY=:83 \
      env -u DISPLAY -u WAYLAND_DISPLAY cargo test --test browser -- --test-threads=1
    # real game, demo mode (XDG_* into target/e2e; DISPLAY = the private display)
    BASTION_E2E_ROOT=$PWD/target/e2e BASTION_BROWSER_TESTS=1 \
      env -u WAYLAND_DISPLAY DISPLAY=:83 XAUTHORITY=/dev/null cargo test --test browser_game

## Machine notes

- Private display :83: `tests/support/private-x11.sh /run/user/1000/bmc-r3/x` (background), then
  `ln -sfn /run/user/1000/bmc-r3/x/x11/X0 /tmp/.X11-unix/X83`; `DISPLAY=:83 XAUTHORITY=/dev/null`.
  The same sway has a Wayland socket at `/run/user/1000/bmc-r3/x/run/wayland-1` and IPC at
  `…/sway-ipc.1000.2.sock` (run `swaymsg` from `target/third-party/pkgs/sway` inside bwrap).
- Screenshots of the private display: `tests/support/x11-shot.py` (XGetImage of the largest
  window; ffmpeg's x11grab fails there: MIT-SHM across IPC namespaces; the root window of a
  rootless Xwayland cannot be read). Key chords: `tests/support/x11-chord.py ffe3 71` (Ctrl+Q).
- `target/e2e/.../vanilla-latest/instance.toml` now has `memory_mb = 2048` (brief: ≤ 3072).
- A test server that closes with unread data sends RST and truncates what the relay had in
  flight: stand-ins must read everything before closing.
- An in-process launch confines the test process too: anything that inspects the game after
  step 9 must be a helper started before it (`tests/browser_game.rs`, `INSPECTOR`).
- Firefox 159 refuses the BiDi event `browsingContext.navigationAborted` (subscribed as optional);
  its first capture during the first navigation is the whole window (1366×682), so captures of
  another size than the page are dropped (up to 5 in a row) and the size is set again.
- Discord's client loads ~700 scripts/styles at start; a single request budget of 600/min broke
  it (webpack retries made it ~1200). Hence `requests_per_minute = 6000` and a separate
  `api_calls_per_minute = 300`.
- `certutil`, `openssl` are installed (nss); Firefox's roots come from p11-kit
  (`/usr/lib/libnssckbi.so -> pkcs11/p11-kit-trust.so`, anchors under `/usr/share/ca-certificates`).
- A Firefox started 2026-11-13 18:29 (pid 421449, parent a bash) is not this package's: leave it.
