#!/bin/bash
# Fetch the Arch packages the display tests need into $ROOT/dl, verify each one, and only
# then extract it to $ROOT/pkgs/<name>. Nothing is installed and nothing is executed.
#
# Every package must pass both checks, or the script stops with a non-zero status:
#   1. SHA-256 equal to the one recorded in the local pacman sync database
#      (/var/lib/pacman/sync, itself fetched and signature-checked by pacman), and
#   2. a valid detached signature by a key in the pacman keyring.
# A package that fails is deleted and never extracted.
#
# usage: fetch.sh [package…]     (default: everything the tests use)
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

if [ $# -eq 0 ]; then
  set -- gamescope xorg-xwayland libei libliftoff libxres luajit seatd xcb-util-errors sway wlroots0.20
fi
db_dir=/var/lib/pacman/sync
keyring=/etc/pacman.d/gnupg/pubring.gpg
mirror=${BASTION_ARCH_MIRROR:-https://geo.mirror.pkgbuild.com}
mkdir -p "$ROOT/dl" "$ROOT/pkgs"

for name in "$@"; do
  case "$name" in *[!A-Za-z0-9._+-]*|"") echo "bad package name: $name" >&2; exit 1;; esac
  desc="" repo_found=""
  for repo in core extra; do
    entry=$(bsdtar -tf "$db_dir/$repo.db" | grep -E "^${name//./\\.}-[^-]+-[^-]+/desc$" | head -1 || true)
    if [ -n "$entry" ]; then desc=$(bsdtar -xOf "$db_dir/$repo.db" "$entry"); repo_found=$repo; break; fi
  done
  [ -n "$desc" ] || { echo "$name: not in the pacman sync database" >&2; exit 1; }
  file=$(awk '/^%FILENAME%$/{getline; print}' <<<"$desc")
  sha=$(awk '/^%SHA256SUM%$/{getline; print}' <<<"$desc")
  arch=$(awk '/^%ARCH%$/{getline; print}' <<<"$desc")
  [ -n "$file" ] && [ ${#sha} -eq 64 ] || { echo "$name: incomplete database entry" >&2; exit 1; }
  [ "$arch" = any ] && arch=x86_64
  # The mirror carries only the newest version; the archive keeps every version, including
  # the one the local database names when that database is a little behind.
  urls=("$mirror/$repo_found/os/$arch/$file" "https://archive.archlinux.org/packages/${name:0:1}/$name/$file")
  pkg="$ROOT/dl/$file"
  fetched=0
  for url in "${urls[@]}"; do
    if { [ -f "$pkg" ] || curl -sS --fail --proto '=https' --max-time 300 -A curl -o "$pkg" "$url" 2>/dev/null; } &&
       { [ -f "$pkg.sig" ] || curl -sS --fail --proto '=https' --max-time 60 -A curl -o "$pkg.sig" "$url.sig" 2>/dev/null; }; then
      fetched=1; break
    fi
    rm -f "$pkg" "$pkg.sig"
  done
  [ $fetched = 1 ] || { echo "$name: download failed (${urls[*]})" >&2; exit 1; }
  got=$(sha256sum "$pkg" | cut -d' ' -f1)
  if [ "$got" != "$sha" ]; then
    rm -f "$pkg" "$pkg.sig"
    echo "$name: SHA-256 MISMATCH (database $sha, downloaded $got); deleted" >&2; exit 1
  fi
  if ! gpgv --keyring "$keyring" "$pkg.sig" "$pkg" >/dev/null 2>"$pkg.gpgv"; then
    tail -1 "$pkg.gpgv" >&2
    rm -f "$pkg" "$pkg.sig"
    echo "$name: BAD SIGNATURE; deleted" >&2; exit 1
  fi
  rm -rf "$ROOT/pkgs/$name"
  mkdir -p "$ROOT/pkgs/$name"
  bsdtar -xf "$pkg" -C "$ROOT/pkgs/$name" --no-same-owner --no-same-permissions usr
  echo "$name: $file sha256=ok signature=good"
done

# What BASTION_NESTED_LAYERS wants: the gamescope set, as /usr layers.
layers=""
for p in "${GAMESCOPE_PKGS[@]}"; do
  [ -d "$ROOT/pkgs/$p/usr" ] && layers+="${layers:+:}$ROOT/pkgs/$p/usr"
done
printf '%s\n' "$layers" > "$ROOT/layers.txt"
echo "layers written to $ROOT/layers.txt"
