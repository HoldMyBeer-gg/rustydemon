#!/usr/bin/env bash
# Run the exact same checks CI runs, in the same order CI runs them,
# with the same flags CI uses.  Fail-fast on the first failure so you
# don't wait on a long test run when a trivial fmt issue is blocking.
#
# Keep this file in sync with .github/workflows/ci.yml — if CI gains
# a new check, add it here too.

set -euo pipefail

# CI sets RUSTFLAGS=-D warnings globally, which promotes every rustc
# warning to a hard error during compilation.  Match that here so a
# warning that would fail CI fails this script too.
export RUSTFLAGS="${RUSTFLAGS:--D warnings}"
export CARGO_TERM_COLOR=always

cd "$(git rev-parse --show-toplevel)"

step() {
    printf '\n\033[1;36m[ci-check]\033[0m %s\n' "$*"
}

# ── 1. Rustfmt ─────────────────────────────────────────────────────────────
step "cargo fmt --all --check"
cargo fmt --all --check

# ── 2. Clippy ──────────────────────────────────────────────────────────────
# These -A overrides MUST match .github/workflows/ci.yml's clippy step
# verbatim.  If you change either, change both.
step "cargo clippy --workspace --all-targets -- -D warnings (with CI's -A overrides)"
cargo clippy \
    --workspace \
    --all-targets \
    -- \
    -D warnings \
    -A clippy::manual_let_else \
    -A clippy::map_unwrap_or \
    -A clippy::redundant_closure_for_method_calls \
    -A clippy::cloned_instead_of_copied

# ── 3. Tests ───────────────────────────────────────────────────────────────
# Skip tests when FAST=1 — pre-commit uses that to keep fmt+clippy
# feedback under ~30 seconds.  pre-push and manual invocation run
# the full suite.
if [[ "${FAST:-0}" != "1" ]]; then
    step "cargo test --workspace"
    cargo test --workspace
fi

printf '\n\033[1;32m[ci-check] all checks passed\033[0m\n'
