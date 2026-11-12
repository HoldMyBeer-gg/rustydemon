#!/usr/bin/env bash
# Points the AUR recipe (packaging/aur/rustydemon-bin) at a release.
#
#   scripts/aur-bump.sh VERSION TARBALL_SHA256 [--ref GIT_REF] [--dir RECIPE_DIR]
#
# VERSION         release version without the leading "v", e.g. 0.5.1
# TARBALL_SHA256  checksum of rustydemon-VERSION-x86_64-unknown-linux-gnu.tar.gz,
#                 as published in the release's SHA256SUMS.txt
# --ref           commit the license and icons are read from (default: vVERSION,
#                 which is what the recipe downloads them from)
# --dir           recipe directory to rewrite (default: the one in this repo)
#
# Rewrites PKGBUILD and .SRCINFO together. .SRCINFO is written by hand so this
# runs on machines without makepkg; scripts/aur-bump.test.sh checks it against
# makepkg where makepkg exists.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dir="$root/packaging/aur/rustydemon-bin"
ref=""

die() { echo "aur-bump: $*" >&2; exit 1; }

[ $# -ge 2 ] || die "usage: aur-bump.sh VERSION TARBALL_SHA256 [--ref GIT_REF] [--dir RECIPE_DIR]"
version="$1"; tarball_sum="$2"; shift 2
while [ $# -gt 0 ]; do
  case "$1" in
    --ref) ref="${2:?--ref needs a value}"; shift 2 ;;
    --dir) dir="${2:?--dir needs a value}"; shift 2 ;;
    *) die "unknown argument: $1" ;;
  esac
done
ref="${ref:-v$version}"

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must look like 1.2.3, got '$version'"
[[ "$tarball_sum" =~ ^[0-9a-f]{64}$ ]] || die "checksum must be 64 lowercase hex digits"
[ -f "$dir/PKGBUILD" ] && [ -f "$dir/.SRCINFO" ] && [ -f "$dir/rustydemon.desktop" ] \
  || die "no recipe in $dir"
git -C "$root" rev-parse -q --verify "$ref^{commit}" >/dev/null || die "no such commit: $ref"

# Checksums in the order the recipe lists its sources.
sum_at_ref() {
  git -C "$root" cat-file -e "$ref:$1" 2>/dev/null || die "$1 does not exist at $ref"
  git -C "$root" show "$ref:$1" | sha256sum | cut -d' ' -f1
}
sums=(
  "$tarball_sum"
  "$(sum_at_ref LICENSE)"
  "$(sum_at_ref assets/icons/linux/icon_48.png)"
  "$(sum_at_ref assets/icons/linux/icon_128.png)"
  "$(sum_at_ref assets/icons/linux/icon_256.png)"
  "$(sum_at_ref assets/icons/linux/icon_512.png)"
  "$(sha256sum "$dir/rustydemon.desktop" | cut -d' ' -f1)"
)

old="$(sed -n 's/^pkgver=//p' "$dir/PKGBUILD")"
[ -n "$old" ] || die "PKGBUILD has no pkgver line"
[ "$(grep -cE "^  '[0-9a-f]{64}'$" "$dir/PKGBUILD")" -eq "${#sums[@]}" ] \
  || die "PKGBUILD does not list ${#sums[@]} checksums; the source list changed, update this script"
[ "$(grep -cE "^	sha256sums = [0-9a-f]{64}$" "$dir/.SRCINFO")" -eq "${#sums[@]}" ] \
  || die ".SRCINFO does not list ${#sums[@]} checksums; regenerate it with makepkg --printsrcinfo"

# Work on copies, and replace the real files only when both are done.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
joined="$(IFS=,; echo "${sums[*]}")"

# PKGBUILD: the version, the release counter, and each checksum line in turn.
awk -v ver="$version" -v sums="$joined" '
  BEGIN { split(sums, s, ","); n = 0 }
  /^pkgver=/ { print "pkgver=" ver; next }
  /^pkgrel=/ { print "pkgrel=1"; next }
  /^  \x27[0-9a-f]+\x27$/ { n++; print "  \x27" s[n] "\x27"; next }
  { print }
' "$dir/PKGBUILD" > "$work/PKGBUILD"

# .SRCINFO: the same, plus the version inside each source line. index() and
# substr() are used so the dots in the version are not treated as wildcards.
awk -v old="$old" -v ver="$version" -v sums="$joined" '
  function swap(line,   out, i) {
    out = ""
    while ((i = index(line, old)) > 0) {
      out = out substr(line, 1, i - 1) ver
      line = substr(line, i + length(old))
    }
    return out line
  }
  BEGIN { split(sums, s, ","); n = 0 }
  /^\tpkgver = / { print "\tpkgver = " ver; next }
  /^\tpkgrel = / { print "\tpkgrel = 1"; next }
  /^\tsha256sums = / { n++; print "\tsha256sums = " s[n]; next }
  /^\tsource = / { print swap($0); next }
  { print }
' "$dir/.SRCINFO" > "$work/.SRCINFO"

cp "$work/PKGBUILD" "$dir/PKGBUILD"
cp "$work/.SRCINFO" "$dir/.SRCINFO"
echo "aur-bump: recipe now at $version (was $old)"
