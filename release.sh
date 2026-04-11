#!/usr/bin/env bash
# release.sh — bump the workspace version and publish all crates in dependency order.
#
# Usage:
#   ./release.sh 0.2.0          # bump to 0.2.0, publish everything
#   ./release.sh 0.2.0 --dry-run  # bump only, skip cargo publish
#
# Crates are published in dependency order so crates.io never sees a crate
# that references an as-yet-unpublished dependency.
#
# Publish order:
#   1. rustydemon-blp2   (no intra-workspace deps)
#   2. rustydemon-lib    (depends on blp2)          [uncomment when added]
#   3. rustydemon        (depends on blp2 + lib)    [uncomment when added]

set -euo pipefail

# ── Arguments ─────────────────────────────────────────────────────────────────
NEW_VERSION="${1:?Usage: ./release.sh <version> [--dry-run]}"
DRY_RUN=false
[[ "${2-}" == "--dry-run" ]] && DRY_RUN=true

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# ── Helpers ───────────────────────────────────────────────────────────────────
info()  { echo "[release] $*"; }
die()   { echo "[release] ERROR: $*" >&2; exit 1; }

publish() {
    local crate="$1"
    if [[ "$DRY_RUN" == true ]]; then
        info "DRY RUN: would publish $crate"
    else
        info "Publishing $crate …"
        cargo publish -p "$crate"
        # crates.io needs a moment to index the new version before dependents can use it.
        info "Waiting 20 s for crates.io to index $crate …"
        sleep 20
    fi
}

# ── Validate version format ───────────────────────────────────────────────────
if ! [[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    die "Version must be in MAJOR.MINOR.PATCH format (got: $NEW_VERSION)"
fi

# ── Check working tree is clean ───────────────────────────────────────────────
if [[ -n "$(git status --porcelain)" ]]; then
    die "Working tree is not clean. Commit or stash changes before releasing."
fi

# ── Bump version in workspace Cargo.toml ──────────────────────────────────────
info "Bumping workspace version to $NEW_VERSION …"
sed -i "s/^version\s*=\s*\".*\"/version     = \"$NEW_VERSION\"/" Cargo.toml

# ── Verify everything still builds and tests pass ─────────────────────────────
info "Running full test suite …"
cargo test --workspace

# ── Commit and tag ────────────────────────────────────────────────────────────
info "Committing version bump …"
git add Cargo.toml Cargo.lock
git commit -m "chore: release v${NEW_VERSION}"
git tag "v${NEW_VERSION}"

# ── Publish ───────────────────────────────────────────────────────────────────
publish rustydemon-blp2
# publish rustydemon-lib     # uncomment when the crate exists
# publish rustydemon         # uncomment when the crate exists

info "Done. Push the commit and tag with:"
info "  git push && git push origin v${NEW_VERSION}"
