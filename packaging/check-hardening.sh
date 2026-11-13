#!/usr/bin/env bash
# Verify that a linked binary really has the exploit mitigations the package promises.
# Run from PKGBUILD check(); exits non-zero if a *required* property is missing.
#
#   required:      PIE, full RELRO (GNU_RELRO + BIND_NOW), non-executable stack,
#                  no writable+executable segment, no RPATH/RUNPATH, no text relocations
#   informational: stack protector / FORTIFY references (present only where C code needs
#                  them), CET (IBT/SHSTK) note — see the PKGBUILD for why a stable-Rust
#                  binary cannot carry it.
set -euo pipefail

bin=${1:?usage: check-hardening.sh <elf-binary>}
fail=0
row() { printf '  %-10s %-34s %s\n' "$1" "$2" "$3"; }
need() { if "$1"; then row ok "$2" "${3:-}"; else row MISSING "$2" "${3:-}"; fail=1; fi; }
info() { row info "$1" "$2"; }

hdr=$(readelf -hW "$bin")
segs=$(readelf -lW "$bin")
dyn=$(readelf -dW "$bin")
syms=$(readelf -sW --dyn-syms "$bin" 2>/dev/null || true)
notes=$(readelf -nW "$bin" 2>/dev/null || true)

is_pie()      { grep -Eq 'Type:\s+DYN' <<<"$hdr" && grep -q 'PIE' <<<"$dyn"; }
has_relro()   { grep -q 'GNU_RELRO' <<<"$segs"; }
bind_now()    { grep -Eq 'BIND_NOW|FLAGS_1.*NOW' <<<"$dyn"; }
nx_stack()    { grep 'GNU_STACK' <<<"$segs" | grep -Eq '\sRW\s' ; }
no_wx()       { ! grep -E '^\s*LOAD' <<<"$segs" | grep -Eq '\sRWE\s'; }
no_rpath()    { ! grep -Eq 'RPATH|RUNPATH' <<<"$dyn"; }
no_textrel()  { ! grep -q 'TEXTREL' <<<"$dyn"; }

echo "hardening of $bin"
need is_pie     'position-independent executable'
need has_relro  'RELRO segment'
need bind_now   'immediate binding (full RELRO)'
need nx_stack   'non-executable stack'
need no_wx      'no writable+executable segment'
need no_rpath   'no RPATH/RUNPATH'
need no_textrel 'no text relocations'

if grep -q '__stack_chk_fail' <<<"$syms"; then info 'stack protector' 'referenced (C objects)'
else info 'stack protector' 'not referenced'; fi
n=$(grep -cE '__[a-z_]+_chk(@|$)' <<<"$syms" || true)
info '_FORTIFY_SOURCE' "$n fortified libc call(s) referenced"
if grep -q 'IBT' <<<"$notes" && grep -q 'SHSTK' <<<"$notes"; then info 'CET' 'IBT+SHSTK marked'
else info 'CET' 'not marked (expected with stable Rust; C objects are built with -fcf-protection)'; fi

exit "$fail"
