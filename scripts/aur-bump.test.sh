#!/usr/bin/env bash
# Checks scripts/aur-bump.sh against a scratch copy of the AUR recipe.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
recipe="$root/packaging/aur/rustydemon-bin"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fails=0

check() { # description, command...
  local what="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "ok    $what"; else echo "FAIL  $what"; fails=$((fails + 1)); fi
}
sums_of() { grep -oE "[0-9a-f]{64}" "$1"; }

# 1. A bump to a made-up version rewrites every place the version and the
#    tarball checksum appear, in both files.
cp -r "$recipe" "$tmp/new"
old="$(sed -n 's/^pkgver=//p' "$recipe/PKGBUILD")"
fake="$(printf 'a%.0s' {1..64})"
"$root/scripts/aur-bump.sh" 9.8.7 "$fake" --ref HEAD --dir "$tmp/new"

check "PKGBUILD has the new version"      grep -qx "pkgver=9.8.7" "$tmp/new/PKGBUILD"
check "PKGBUILD pkgrel is back to 1"      grep -qx "pkgrel=1" "$tmp/new/PKGBUILD"
check ".SRCINFO has the new version"      grep -qxP "\tpkgver = 9.8.7" "$tmp/new/.SRCINFO"
check ".SRCINFO sources name the new tag" grep -q "releases/download/v9.8.7/rustydemon-9.8.7-x86_64" "$tmp/new/.SRCINFO"
check "no trace of the old version"       bash -c "! grep -rqF '$old' '$tmp/new/PKGBUILD' '$tmp/new/.SRCINFO'"
check "PKGBUILD has seven checksums"      test "$(sums_of "$tmp/new/PKGBUILD" | wc -l)" -eq 7
check "tarball checksum comes first"      test "$(sums_of "$tmp/new/PKGBUILD" | head -1)" = "$fake"
check "both files list the same checksums" test "$(sums_of "$tmp/new/PKGBUILD")" = "$(sums_of "$tmp/new/.SRCINFO")"
check "desktop entry checksum is the file's" \
  test "$(sums_of "$tmp/new/PKGBUILD" | tail -1)" = "$(sha256sum "$recipe/rustydemon.desktop" | cut -d' ' -f1)"

# 2. Where makepkg exists, the .SRCINFO written by hand must be exactly what
#    makepkg would generate from the PKGBUILD.
if command -v makepkg >/dev/null; then
  check ".SRCINFO matches makepkg --printsrcinfo" \
    bash -c "cd '$tmp/new' && diff <(makepkg --printsrcinfo) .SRCINFO"
else
  echo "skip  .SRCINFO matches makepkg (no makepkg on this machine)"
fi

# 3. Bumping to the version already committed changes nothing.
if git -C "$root" rev-parse -q --verify "refs/tags/v$old" >/dev/null; then
  cp -r "$recipe" "$tmp/same"
  current="$(sums_of "$recipe/PKGBUILD" | head -1)"
  "$root/scripts/aur-bump.sh" "$old" "$current" --dir "$tmp/same"
  check "re-running for the committed version is a no-op" diff -r "$recipe" "$tmp/same"
else
  echo "FAIL  tag v$old is not available (fetch tags)"; fails=$((fails + 1))
fi

# 4. Bad input is refused, and nothing is written.
cp -r "$recipe" "$tmp/bad"
check "a checksum that is not 64 hex digits is refused" \
  bash -c "! '$root/scripts/aur-bump.sh' 9.8.7 nothex --ref HEAD --dir '$tmp/bad'"
check "a version with a leading v is refused" \
  bash -c "! '$root/scripts/aur-bump.sh' v9.8.7 '$fake' --ref HEAD --dir '$tmp/bad'"
check "refused input leaves the recipe untouched" diff -r "$recipe" "$tmp/bad"

[ "$fails" -eq 0 ] && echo "all checks passed" || { echo "$fails failed"; exit 1; }
