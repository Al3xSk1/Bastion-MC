#!/bin/bash
# The companion's phone, home screen and dark menus in the real game (package mod-ui):
# launches <instance> in demo mode on a PRIVATE X display, presses keys there with XTEST
# (tests/support/x11-key.py), and takes screenshots of that display with ffmpeg (x11grab).
#
# Only ever on a display that nothing else uses (tests/support/private-x11.sh): the keys go to
# whatever has the focus there. The launcher runs with all four XDG directories inside
# <data-root>, never the user's own; the game is stopped with SIGINT at the end.
#
# usage: phone-shots.sh <data-root> <instance> <display-number> <out-dir> <width> <height> [world]
#   <width> <height>  the game window (written into the instance's instance.toml)
#   world             also enter the demo world and pull the phone out there
#
# Writes <out-dir>/launch.log, <out-dir>/keys.log and one PNG per step; prints the steps.
set -u
ROOT=$1
ID=$2
DISP=$3
OUT=$4
W=$5
H=$6
WORLD=${7:-}
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BIN=${BASTION_BIN:-$REPO/target/debug/bastion-mc}
INST=$ROOT/data/bastion-mc/instances/$ID
mkdir -p "$OUT"
[ -d "$INST" ] || { echo "no instance $INST" >&2; exit 1; }
[ -S "/tmp/.X11-unix/X$DISP" ] || [ -L "/tmp/.X11-unix/X$DISP" ] || { echo "no display :$DISP" >&2; exit 1; }

# The window size, memory within the machine's rules, quiet, no onboarding screen.
python3 -I - "$INST/instance.toml" "$W" "$H" <<'EOF'
import re, sys
path, w, h = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(path).read()
head, sep, rest = text.partition("\n[")
lines = [l for l in head.split("\n") if not re.match(r"\s*(width|height|memory_mb)\s*=", l)]
lines += [f"width = {w}", f"height = {h}", "memory_mb = 2048"]
text = "\n".join(l for l in lines if l != "") + "\n" + (sep + rest if sep else "")
# The companion on Minecraft versions not yet on the launcher's tested list.
if "\n[companion]" not in text:
    text += "\n[companion]\nforce = true\n"
open(path, "w").write(text)
EOF
mkdir -p "$INST/persist"
OPTS=$INST/persist/options.txt
touch "$OPTS"
# GUI_SCALE=<n> asks for that GUI scale (0, the default, is the game's "auto").
for kv in onboardAccessibility:false soundCategory_master:0.0 pauseOnLostFocus:false skipMultiplayerWarning:true \
          tutorialStep:none joinedFirstServer:true "guiScale:${GUI_SCALE:-0}"; do
  k=${kv%%:*}
  grep -q "^$k:" "$OPTS" && sed -i "s/^$k:.*/$kv/" "$OPTS" || echo "$kv" >> "$OPTS"
done

export XDG_CONFIG_HOME=$ROOT/config XDG_DATA_HOME=$ROOT/data XDG_CACHE_HOME=$ROOT/cache XDG_STATE_HOME=$ROOT/state
DISPLAY=:$DISP XAUTHORITY=/dev/null WAYLAND_DISPLAY= BASTION_PHONE_DEMO=${BASTION_PHONE_DEMO:-1} "$BIN" launch "$ID" --demo > "$OUT/launch.log" 2>&1 &
LAUNCHER=$!
stop() {
  kill -INT "$LAUNCHER" 2>/dev/null
  for _ in $(seq 1 60); do kill -0 "$LAUNCHER" 2>/dev/null || break; sleep 0.5; done
  kill -0 "$LAUNCHER" 2>/dev/null && kill -TERM "$LAUNCHER"
}
trap stop EXIT

waitfor() { # <pattern> <seconds>
  for _ in $(seq 1 $(( $2 * 2 ))); do
    grep -q "$1" "$OUT/launch.log" && return 0
    kill -0 "$LAUNCHER" 2>/dev/null || return 1
    sleep 0.5
  done
  return 1
}
# The game's window (gamescope's, or the game's own as a direct client). A rootless Xwayland
# (headless sway) cannot read its root window, so the capture is of that window by id.
win() {
  DISPLAY=:$DISP XAUTHORITY=/dev/null xprop -root _NET_CLIENT_LIST 2>/dev/null | grep -o '0x[0-9a-f]*' | head -1
}
LAST_SUM=
grab() { # <file>: one capture of the game's window
  local id
  id=$(win)
  [ -n "$id" ] || { echo "no window to capture" >&2; return 1; }
  ffmpeg -loglevel error -f x11grab -window_id "$id" -i ":$DISP" -frames:v 1 -y "$1" 2>>"$OUT/ffmpeg.log"
}
# A screenshot of the step just done. The capture can lag the game on a busy machine, so wait
# (up to 6 s) until it differs from the previous step's, let it settle, and take it then; a
# step that changes nothing on screen just waits the 6 s.
shot() {
  local file=$OUT/$1.png sum i
  for i in $(seq 1 15); do
    grab "$file" || return 1
    sum=$(md5sum < "$file" | cut -c1-32)
    [ "$sum" != "$LAST_SUM" ] && break
    sleep 0.4
  done
  sleep 0.6
  grab "$file" || return 1
  LAST_SUM=$(md5sum < "$file" | cut -c1-32)
  echo "shot $1"
}

if ! waitfor "blocks.png-atlas" 240; then
  echo "the game did not get to its title screen; see $OUT/launch.log" >&2
  exit 1
fi
sleep 9

coproc KEYS { DISPLAY=:$DISP XAUTHORITY=/dev/null python3 -I "$REPO/tests/support/x11-input.py" 2>&1; }
read -r ready <&"${KEYS[0]}"
echo "$ready" > "$OUT/keys.log"
say() { # <line>: one request to x11-input.py, its answer in keys.log (and on stdout)
  echo "$1" >&"${KEYS[1]}"
  read -r answer <&"${KEYS[0]}"
  echo "$answer" >> "$OUT/keys.log"
  REPLY_LINE=$answer
}
key() { # <keysym> [times]
  for _ in $(seq 1 "${2:-1}"); do
    say "key $1 70"
    sleep 0.15
  done
}
typeit() { # <text>: lower-case letters, digits, space
  local i ch
  for (( i = 0; i < ${#1}; i++ )); do
    ch=${1:$i:1}
    if [ "$ch" = " " ]; then key 0x20; else key "$(printf '0x%x' "'$ch")"; fi
  done
}
R=0x72; ESC=0xff1b; TAB=0xff09; RET=0xff0d; DOWN=0xff54; UP=0xff52
open_app() { key $TAB $(( $1 + 1 )); key $RET; }
# Back to the phone's home screen from anywhere in it: Escape until it is away, then pull it
# out again (it comes back where it was put away: on its home screen). Title screen only (in a
# world one Escape too many opens the pause menu).
phone_home() { key $ESC 4; sleep 0.6; key $R; sleep 1; }

# Where a GUI point of the game is on the display: the game draws at the instance's size and
# auto GUI scale; gamescope shows that letterboxed in its window.
gui_to_root() { # <gui x> <gui y> -> "x y"
  say "geometry"
  python3 -I - "$REPLY_LINE" "$W" "$H" "$1" "$2" <<'PY'
import sys
gx, gy, gw, gh = (int(v) for v in sys.argv[1].split())
w, h, x, y = int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4]), float(sys.argv[5])
s = 1
while s < w and s < h and w // (s + 1) >= 320 and h // (s + 1) >= 240:
    s += 1
k = min(gw / w, gh / h)
ox, oy = gx + (gw - w * k) / 2, gy + (gh - h * k) / 2
print(int(ox + x * s * k), int(oy + y * s * k))
PY
}
# The middle of the browser page's area on the phone (Phone.body, then the app's rows).
page_point() {
  python3 -I - "$W" "$H" <<'PY'
import sys
w, h = int(sys.argv[1]), int(sys.argv[2])
s = 1
while s < w and s < h and w // (s + 1) >= 320 and h // (s + 1) >= 240:
    s += 1
sw, sh = -(-w // s), -(-h // s)
r = lambda v: int(v + 0.5)
m = min(8, max(0, min(sw, sh) // 20))
aw, ah = max(0, sw - 2 * m), max(0, sh - 2 * m)
ph = min(min(ah, 320), max(220, r(sh * 0.82))) if ah >= 220 else ah
pw = min(aw, max(120, min(196, r(ph * 0.66))))
right = min(sw - m, sw - max(m, sw // 40))
bx, by = max(m, right - pw), sh - m - ph
top, bottom = by + 4 + 11 + 14 + 2 + 12 + 2 + 10, by + ph - 4 - 8 - 2
print(bx + pw // 2, (top + bottom) // 2)
PY
}

shot 01-home
key $TAB
sleep 0.3
shot 02-home-focus
key $R
sleep 1
shot 03-phone-home

open_app 0                      # Spotify
sleep 1.5
shot 10-spotify
key $TAB 3
key $RET                        # the search tab; its field takes the keys (R is a letter there)
sleep 0.3
typeit "rock"
key $RET
sleep 1.5
shot 11-spotify-search
phone_home

open_app 1                      # Discord: the browser at the site named discord
sleep 3
shot 20-discord
read -r px py < <(page_point)
read -r rx ry < <(gui_to_root "$px" "$py")
say "click $rx $ry 1"
sleep 0.4
typeit "hi there"
sleep 1.5
shot 21-discord-clicked-typed
echo "page click at gui $px,$py root $rx,$ry" >> "$OUT/keys.log"
key $ESC                        # the page gives the keyboard back
key $TAB 6                      # back, page back, forward, reload, close, turn
key $RET                        # turned sideways, for reading
sleep 1.5
shot 22-discord-landscape
phone_home

open_app 2                      # Browser
sleep 1.2
shot 30-browser
phone_home

open_app 3                      # Clips
sleep 3
shot 40-clips
phone_home

open_app 4                      # Realms: the demo accepts 424242
sleep 1
typeit "424242"
key $RET
sleep 1.5
shot 50-realms
phone_home

open_app 5                      # Settings
sleep 1.2
shot 60-settings
key $ESC 4
sleep 1
shot 70-phone-away

key $UP 6
key $DOWN                       # Options (demo mode: Play Demo World, Options, Quit)
key $RET
sleep 1.5
shot 80-options-dark
key $ESC
sleep 1

if [ -n "$WORLD" ]; then
  key $UP 6
  key $RET                      # Play Demo World
  waitfor "joined the game" 150
  sleep 10
  key $ESC                      # the demo's greeting
  sleep 1.5
  shot 90-world
  key $R
  sleep 1
  shot 91-world-phone
  open_app 0                    # Spotify, over the running world
  sleep 1.5
  shot 92-world-spotify
  key $ESC                      # back to the phone's home screen
  sleep 0.4
  open_app 3                    # Clips: save the last seconds; the demo answers with a notice
  sleep 2
  key $TAB 2
  key $RET
  sleep 0.8
  shot 93-world-clips-banner
  key $ESC                      # the phone's home screen
  sleep 0.4
  key $ESC                      # away: the notice peeks in at the right
  sleep 0.8
  shot 94-world-toast
  key $ESC                      # the game's pause menu, with the dark veil
  sleep 1.5
  shot 95-pause-dark
  key $ESC
  sleep 0.5
fi
exec {KEYS[1]}>&-
echo "done"
