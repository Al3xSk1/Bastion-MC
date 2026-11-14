#!/usr/bin/env bash
# Refresh the website's screenshots from the front end's own test snapshots.
#
#   site/update-screenshots.sh [SNAPSHOT_DIR [NAME…]]
#   site/update-screenshots.sh --list
#
# The front end's tests draw every state of the window in software and leave the pictures
# in target/gui-snapshots/ (`cargo test -p bastion-mc-gui`; this script does not run them).
# They are drawn from fixtures: no game, no account and no network is involved, and the
# website has to say so under the pictures.
#
# SNAPSHOT_DIR (default: target/gui-snapshots of this checkout) may use either naming
# scheme. For each picture the redesigned interface's file is taken if it is there, and
# otherwise the file of the interface before the redesign:
#
#   launch-running-protected-1280x800.png    the redesigned interface (page-state-WIDTHxHEIGHT)
#   running-tiny-skia.png                    the interface before it (state-renderer)
#
# Which pictures: the ones the pages ask for. A page shows one with {{SHOT name "alt text"}}
# (site/build.py turns that into site/src/assets/shot-NAME.png with its real width and
# height), so this script reads site/src/pages/*.html and installs exactly those names from
# the table below. With NAMEs after the directory, exactly those are installed instead.
# shot-*.png files that are then not wanted are removed, because the generator's --check
# refuses a published file that no page uses.
#
# Nothing is converted: each PNG is copied byte for byte, after a check that it is a
# complete PNG file without chunks about its making (see png_check). Only bash and
# coreutils are needed. Either everything asked for is installed or nothing changes; a
# picture that cannot be had is an error, never a gap.
set -euo pipefail

# The redesigned interface is drawn at 880x720 and at 1280x800; the site uses the wide one.
SIZE=1280x800

# name | redesigned interface | interface before the redesign | what the picture shows
# ("-" = that interface has no such view). The stable NAME is all a page ever writes.
MENU=(
  "launch|launch-running-protected|running|the launch page while a game runs, with the protection readout: all five in force"
  "accounts|accounts|idle|the accounts: who is signed in and where each refresh token is kept"
  "security|security|emergency|the security page: what the five readouts mean, the emergency panel and the recovery pages"
  "console|console|running|the console: lines from the launcher, the network filter and the game"
  "refused|launch-refused|refused|a launch that was refused, with the reason and the ways out"
  "reduced|security-running|-|the security page while a game runs with one protection reduced and one off"
)

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/.." && pwd)
assets=$here/src/assets
pages=$here/src/pages

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}
warn() { printf 'warning: %s\n' "$*" >&2; }

usage() {
  cat <<'EOF'
usage: site/update-screenshots.sh [SNAPSHOT_DIR [NAME…]]
       site/update-screenshots.sh --list

Copies the front end's test snapshots to site/src/assets/shot-NAME.png.
SNAPSHOT_DIR defaults to target/gui-snapshots (written by `cargo test -p bastion-mc-gui`).
Without NAMEs, the pictures that site/src/pages/*.html use ({{SHOT name "alt text"}}).
EOF
}

# menu_entry NAME: sets entry_new, entry_old and entry_what; fails if NAME is not offered.
menu_entry() {
  local line name
  for line in "${MENU[@]}"; do
    IFS='|' read -r name entry_new entry_old entry_what <<<"$line"
    if [ "$name" = "$1" ]; then return 0; fi
  done
  return 1
}

list_menu() {
  local line name new old what
  printf '%-10s %-40s %-24s %s\n' "NAME" "REDESIGNED INTERFACE" "BEFORE THE REDESIGN" "SHOWS"
  for line in "${MENU[@]}"; do
    IFS='|' read -r name new old what <<<"$line"
    if [ "$old" = "-" ]; then old="(no such view)"; else old="$old-tiny-skia.png"; fi
    printf '%-10s %-40s %-24s %s\n' "$name" "$new-$SIZE.png" "$old" "$what"
  done
}

# png_check FILE: succeeds if FILE is a PNG picture the site can publish and sets png_width
# and png_height; otherwise png_why says what is wrong. The file is read chunk by chunk:
# the signature, the header chunk first, image data, and the end chunk as the very last
# thing in the file (a file that is still being written, or was cut short, does not end
# that way). A chunk that says how the file was made (text, a time, camera data) is
# refused, because the generator's --check refuses a picture that carries one. That check
# also verifies every chunk's checksum, which coreutils alone cannot do here.
png_check() {
  local file=$1 size head code octal letter kind length offset=8 count=0 data=0
  local -a bytes
  png_why="it is not a PNG file"
  size=$(wc -c <"$file")
  head=$(od -An -v -tx1 -N8 -- "$file" | tr -d ' \n')
  [ "$head" = "89504e470d0a1a0a" ] || return 1
  while :; do
    png_why="it is not a complete PNG file (is it being written right now?)"
    [ $((offset + 12)) -le "$size" ] || return 1
    read -r -a bytes < <(od -An -v -tu1 -j"$offset" -N8 -- "$file")
    [ "${#bytes[@]}" -eq 8 ] || return 1
    length=$(((bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3]))
    kind=
    for code in "${bytes[@]:4:4}"; do
      if { [ "$code" -lt 65 ] || [ "$code" -gt 90 ]; } && { [ "$code" -lt 97 ] || [ "$code" -gt 122 ]; }; then
        png_why="it is not a PNG file (after $offset bytes there is no chunk)"
        return 1
      fi
      printf -v octal '%03o' "$code"
      printf -v letter "\\$octal"
      kind+=$letter
    done
    [ $((offset + 12 + length)) -le "$size" ] || return 1
    count=$((count + 1))
    if [ "$count" -eq 1 ]; then
      png_why="it is not a PNG file (it does not begin with its header chunk)"
      [ "$kind" = IHDR ] && [ "$length" -eq 13 ] || return 1
      read -r -a bytes < <(od -An -v -tu1 -j16 -N8 -- "$file")
      [ "${#bytes[@]}" -eq 8 ] || return 1
      png_width=$(((bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3]))
      png_height=$(((bytes[4] << 24) | (bytes[5] << 16) | (bytes[6] << 8) | bytes[7]))
      png_why="it is not a PNG picture (its header gives no width or no height)"
      [ "$png_width" -gt 0 ] && [ "$png_height" -gt 0 ] || return 1
    fi
    case $kind in
      IDAT) data=1 ;;
      tEXt | zTXt | iTXt | eXIf | tIME)
        png_why="it carries a $kind chunk (text, a time or camera data about how the file was made); the generator refuses such a picture"
        return 1
        ;;
      IEND)
        png_why="it is not a complete PNG file (no image data, or an end chunk that is not empty)"
        [ "$length" -eq 0 ] && [ "$data" -eq 1 ] || return 1
        png_why="$((size - offset - 12)) byte(s) follow the end of the picture"
        [ $((offset + 12)) -eq "$size" ] || return 1
        png_why=
        return 0
        ;;
    esac
    offset=$((offset + 12 + length))
  done
}

checksum() {
  local sum
  read -r sum _ < <(sha256sum -- "$1")
  printf '%s' "$sum"
}

# The names the pages use, in the order of first use, one per line. The generator's own
# rule is followed: a directive is {{…}} with no brace inside, and its words are split like
# a shell's. A {{SHOT …}} this function cannot read is an error, never skipped.
shot_directive='\{\{[[:space:]]*SHOT[[:space:]]([^{}]*)\}\}'
names_used_by_pages() {
  local page text match arguments name
  local -A seen=()
  for page in "$pages"/*.html; do
    [ -f "$page" ] || continue
    text=$(<"$page")
    while [[ $text =~ $shot_directive ]]; do
      match=${BASH_REMATCH[0]}
      arguments=${BASH_REMATCH[1]}
      text=${text#*"$match"}
      arguments=${arguments//$'\n'/ }
      name=
      read -r name _ <<<"$arguments" || true
      if ! [[ $name =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
        die "${page#"$repo"/}: cannot read the picture's name in $match (write it bare: {{SHOT launch \"alt text\"}})"
      fi
      if [ -z "${seen[$name]:-}" ]; then
        seen[$name]=1
        printf '%s\n' "$name"
      fi
    done
  done
}

case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
  --list)
    list_menu
    exit 0
    ;;
  -*)
    usage >&2
    exit 2
    ;;
esac

snapshots=${1:-$repo/target/gui-snapshots}
[ -d "$snapshots" ] || die "$snapshots is not a directory (run \`cargo test -p bastion-mc-gui\` first, or name the directory that holds the snapshots)"
[ -d "$assets" ] || die "$assets does not exist"

wanted=()
if [ $# -gt 1 ]; then
  shift
  wanted=("$@")
  chosen_by="named on the command line"
else
  used=$(names_used_by_pages)
  while IFS= read -r name; do
    if [ -n "$name" ]; then wanted+=("$name"); fi
  done <<<"$used"
  chosen_by="used by site/src/pages/*.html"
  [ "${#wanted[@]}" -gt 0 ] || die "no page under site/src/pages/ uses a screenshot ({{SHOT name \"alt text\"}}); name the pictures to install, see --list"
fi

# Stage everything first: copy, then check the copy, so that what is installed is exactly
# what was checked even if the snapshot directory is being rewritten meanwhile. The staging
# directory is next to this script (same file system as the assets, and not a place the
# generator reads).
stage=$(mktemp -d "$here/.update-screenshots.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT

problems=0
names=()
old_scheme=()
declare -A from=() dimensions=()
for name in "${wanted[@]}"; do
  if [ -n "${from[$name]:-}" ]; then continue; fi
  if ! menu_entry "$name"; then
    printf 'error: no picture is called "%s" (see --list; a new one is one more line in the table of site/update-screenshots.sh)\n' "$name" >&2
    problems=$((problems + 1))
    continue
  fi
  if [ -f "$snapshots/$entry_new-$SIZE.png" ]; then
    file=$entry_new-$SIZE.png
    scheme=new
  elif [ "$entry_old" != "-" ] && [ -f "$snapshots/$entry_old-tiny-skia.png" ]; then
    file=$entry_old-tiny-skia.png
    scheme=old
  else
    looked="$entry_new-$SIZE.png"
    if [ "$entry_old" != "-" ]; then
      looked="$looked or $entry_old-tiny-skia.png"
    else
      looked="$looked (the interface before the redesign has no such view)"
    fi
    printf 'error: shot-%s.png: %s has no %s\n' "$name" "$snapshots" "$looked" >&2
    problems=$((problems + 1))
    continue
  fi
  cp -- "$snapshots/$file" "$stage/shot-$name.png"
  chmod 644 "$stage/shot-$name.png"
  if ! png_check "$stage/shot-$name.png"; then
    printf 'error: shot-%s.png: %s/%s: %s\n' "$name" "$snapshots" "$file" "$png_why" >&2
    problems=$((problems + 1))
    continue
  fi
  names+=("$name")
  from[$name]=$file
  dimensions[$name]="${png_width}x${png_height}"
  if [ "$scheme" = old ]; then old_scheme+=("$name"); fi
done
[ "$problems" -eq 0 ] || die "$problems picture(s) cannot be had; nothing was changed"

# Install: a rename within one file system, so that no build ever reads half a picture.
printf 'screenshots %s, from %s\n' "$chosen_by" "$snapshots"
for name in "${names[@]}"; do
  target=$assets/shot-$name.png
  if [ -f "$target" ] && [ "$(checksum "$target")" = "$(checksum "$stage/shot-$name.png")" ]; then
    state=unchanged
  elif [ -e "$target" ]; then
    state=updated
  else
    state=new
  fi
  size=$(wc -c <"$stage/shot-$name.png")
  if [ "$state" != unchanged ]; then
    mv -fT -- "$stage/shot-$name.png" "$target"
  fi
  printf '  %-9s %-20s %-10s %8d bytes  <- %s\n' "$state" "shot-$name.png" "${dimensions[$name]}" "$size" "${from[$name]}"
done

# What an earlier choice left behind.
for target in "$assets"/shot-*.png; do
  [ -e "$target" ] || continue
  name=${target##*/shot-}
  name=${name%.png}
  if [ -z "${from[$name]:-}" ]; then
    rm -f -- "$target"
    printf '  %-9s %s (no longer wanted)\n' "removed" "shot-$name.png"
  fi
done

# Things a person has to look at; none of them is a failure of this script.
declare -A first_use=()
for name in "${names[@]}"; do
  file=${from[$name]}
  if [ -n "${first_use[$file]:-}" ]; then
    warn "shot-${first_use[$file]}.png and shot-$name.png are the same picture ($file): the site would show it twice"
  else
    first_use[$file]=$name
  fi
done
if [ "${#old_scheme[@]}" -gt 0 ]; then
  warn "taken from the interface before the redesign: ${old_scheme[*]}"
  warn "the alt texts and captions in site/src/pages/ have to describe what these pictures show: read them again before publishing"
fi
printf 'next: python3 site/build.py --check\n'
