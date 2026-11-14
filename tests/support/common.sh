# Sourced by the scripts in this directory.
# Third-party programs the display tests need (sway, gamescope, Xwayland, …) are not
# installed on the machine: fetch.sh downloads the Arch packages, checks them, and extracts
# them under $ROOT, and everything is only ever run from there inside bubblewrap.
SUPPORT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$SUPPORT_DIR/../.." && pwd)
ROOT=${BASTION_THIRD_PARTY:-$REPO/target/third-party}
# What each consumer layers over /usr.
GAMESCOPE_PKGS=(gamescope xorg-xwayland libei libliftoff libxres luajit seatd xcb-util-errors)
SWAY_PKGS=(sway wlroots0.20 seatd libliftoff xcb-util-errors xorg-xwayland libei)

require_pkgs() {
  local p
  for p in "$@"; do
    [ -d "$ROOT/pkgs/$p/usr" ] || { echo "missing $ROOT/pkgs/$p — run tests/support/fetch.sh first" >&2; exit 1; }
  done
}
