#!/bin/bash
# A headless Wayland compositor (the extracted sway), itself confined by bubblewrap:
# read-only /usr, no network, no home directory, one writable runtime directory.
# The socket appears as <runtime-dir>/wayland-1.
# usage: sway-headless.sh <runtime-dir> [pixman|gles2]
set -eu
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_pkgs sway wlroots0.20 seatd libliftoff xcb-util-errors
RT="$1"
RENDERER="${2:-pixman}"
mkdir -p "$RT"; chmod 700 "$RT"
: > "$RT/sway.conf"
LIBS=/pkgs/wlroots0.20/usr/lib:/pkgs/seatd/usr/lib:/pkgs/libliftoff/usr/lib:/pkgs/xcb-util-errors/usr/lib
GPU=()
if [ "$RENDERER" != pixman ]; then
  GPU=(--dev-bind /dev/dri /dev/dri --ro-bind /sys/dev/char /sys/dev/char --ro-bind /sys/devices /sys/devices)
fi
exec /usr/bin/bwrap \
  --unshare-all --die-with-parent --new-session --clearenv \
  --ro-bind /usr /usr --symlink usr/lib /lib --symlink usr/lib /lib64 --symlink usr/bin /bin \
  --ro-bind /etc/fonts /etc/fonts \
  --proc /proc --dev /dev --tmpfs /tmp "${GPU[@]}" \
  --ro-bind "$ROOT/pkgs" /pkgs \
  --bind "$RT" /run/user/1000 \
  --setenv XDG_RUNTIME_DIR /run/user/1000 \
  --setenv HOME /tmp \
  --setenv PATH /usr/bin \
  --setenv LD_LIBRARY_PATH "$LIBS" \
  --setenv WLR_BACKENDS headless \
  --setenv WLR_RENDERER "$RENDERER" \
  --setenv WLR_LIBINPUT_NO_DEVICES 1 \
  -- /pkgs/sway/usr/bin/sway -c /run/user/1000/sway.conf --unsupported-gpu
