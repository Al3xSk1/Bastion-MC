#!/bin/bash
# Run the extracted gamescope nested on an X11 display, inside bubblewrap, for looking at
# its behaviour by hand. /usr inside is the host's /usr with the extracted packages layered
# on top (read-only), i.e. what the machine would look like with those packages installed.
# NOTE: this opens a window on $DISPLAY (default :0, the real session) and is given that
# display's cookie. The launcher's own path is src/sandbox/nested.rs; this is exploration.
# usage: gamescope-explore.sh <state-dir> <gamescope args...> -- <child...>
set -eu
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_pkgs "${GAMESCOPE_PKGS[@]}"
RT="$1"; shift
N=${DISPLAY:-:0}; N=${N#:}; N=${N%%.*}
mkdir -p "$RT/run" "$RT/x11"; chmod 700 "$RT" "$RT/run"
printf 'compositor:x:%s:%s::/tmp:/usr/bin/nologin\n' "$(id -u)" "$(id -g)" > "$RT/passwd"
LAYERS=(--overlay-src /usr)
for p in "${GAMESCOPE_PKGS[@]}"; do
  LAYERS+=(--overlay-src "$ROOT/pkgs/$p/usr")
done
exec /usr/bin/bwrap \
  --unshare-all --die-with-parent --new-session --clearenv \
  "${LAYERS[@]}" --ro-overlay /usr \
  --symlink usr/lib /lib --symlink usr/lib /lib64 --symlink usr/bin /bin \
  --ro-bind /etc/fonts /etc/fonts \
  --ro-bind "$RT/passwd" /etc/passwd \
  --proc /proc --dev /dev --dev-bind /dev/dri /dev/dri --ro-bind /sys /sys \
  --tmpfs /tmp --bind "$RT/x11" /tmp/.X11-unix --ro-bind "/tmp/.X11-unix/X$N" "/tmp/.X11-unix/X$N" \
  --ro-bind "${XAUTHORITY:-$HOME/.Xauthority}" /tmp/host-xauth \
  --bind "$RT/run" /run/user/1000 \
  --setenv DISPLAY ":$N" --setenv XAUTHORITY /tmp/host-xauth \
  --setenv XDG_RUNTIME_DIR /run/user/1000 --setenv HOME /tmp --setenv PATH /usr/bin \
  -- /usr/bin/gamescope "$@"
