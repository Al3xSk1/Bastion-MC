#!/bin/bash
# A throw-away X11 display for tests that must not touch the real session: headless sway
# (GPU renderer) with Xwayland, confined by bubblewrap. The X socket appears as
# <state-dir>/x11/X0; link it into /tmp/.X11-unix under a free number to use it as a display:
#   ln -s <state-dir>/x11/X0 /tmp/.X11-unix/X77  &&  DISPLAY=:77 XAUTHORITY=/dev/null …
# usage: private-x11.sh <state-dir>
set -eu
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_pkgs "${SWAY_PKGS[@]}"
RT="$1"
mkdir -p "$RT/run" "$RT/x11"; chmod 700 "$RT" "$RT/run"
: > "$RT/run/sway.conf"
printf 'compositor:x:%s:%s::/tmp:/usr/bin/nologin\n' "$(id -u)" "$(id -g)" > "$RT/passwd"
LAYERS=(--overlay-src /usr)
for p in "${SWAY_PKGS[@]}"; do
  LAYERS+=(--overlay-src "$ROOT/pkgs/$p/usr")
done
exec /usr/bin/bwrap \
  --unshare-all --die-with-parent --new-session --clearenv \
  "${LAYERS[@]}" --ro-overlay /usr \
  --symlink usr/lib /lib --symlink usr/lib /lib64 --symlink usr/bin /bin \
  --ro-bind /etc/fonts /etc/fonts --ro-bind "$RT/passwd" /etc/passwd \
  --proc /proc --dev /dev --dev-bind /dev/dri /dev/dri \
  --ro-bind /sys/dev/char /sys/dev/char --ro-bind /sys/devices /sys/devices \
  --tmpfs /tmp --bind "$RT/x11" /tmp/.X11-unix \
  --bind "$RT/run" /run/user/1000 \
  --setenv XDG_RUNTIME_DIR /run/user/1000 --setenv HOME /tmp --setenv PATH /usr/bin \
  --setenv WLR_BACKENDS headless --setenv WLR_RENDERER gles2 --setenv WLR_LIBINPUT_NO_DEVICES 1 \
  -- /usr/bin/sway -c /run/user/1000/sway.conf --unsupported-gpu
