#!/usr/bin/env bash
# Checks where scripts/macos-bundle.sh reads the built binary from and writes
# its output to. Runs anywhere cargo does; it builds nothing.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fails=0

check() { # description, command...
  local what="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "ok    $what"; else echo "FAIL  $what"; fails=$((fails + 1)); fi
}
where() { "$root/scripts/macos-bundle.sh" --where "$@"; }

# 1. With cargo building somewhere else (CARGO_TARGET_DIR here; a
#    build.target-dir in ~/.cargo/config.toml behaves the same), the script
#    must look there, not in ./target.
export CARGO_TARGET_DIR="$tmp/elsewhere"
check "binary is read from cargo's target directory" \
  test "$(where | sed -n 's/^bin=//p')" = "$tmp/elsewhere/release/rustydemon"
check "output goes under cargo's target directory" \
  test "$(where | sed -n 's/^out=//p')" = "$tmp/elsewhere/macos"
check "universal slices are read from there too" \
  bash -c "'$root/scripts/macos-bundle.sh' --where --universal | grep -qx 'slice=$tmp/elsewhere/aarch64-apple-darwin/release/rustydemon'"

# 2. --where only reports; it must not build or create anything.
check "--where creates nothing" test ! -e "$tmp/elsewhere"

if [ "$fails" -ne 0 ]; then echo "$fails check(s) failed"; exit 1; fi
echo "all checks passed"
