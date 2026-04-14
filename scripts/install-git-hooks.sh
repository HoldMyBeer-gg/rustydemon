#!/usr/bin/env bash
# Install the pre-commit and pre-push git hooks that mirror GitHub CI.
# Run from anywhere inside the repo.  Idempotent.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

HOOK_DIR="$(git rev-parse --git-path hooks)"
mkdir -p "$HOOK_DIR"

# ── pre-commit: fast path (fmt + clippy, no tests) ─────────────────────────
cat >"$HOOK_DIR/pre-commit" <<'HOOK'
#!/usr/bin/env bash
# Auto-installed by scripts/install-git-hooks.sh
# Runs fmt + clippy with CI's exact flags before every commit.
# Skip tests here for speed; pre-push runs the full suite.
set -e
exec env FAST=1 bash "$(git rev-parse --show-toplevel)/scripts/ci-check.sh"
HOOK
chmod +x "$HOOK_DIR/pre-commit"

# ── pre-push: full CI suite ────────────────────────────────────────────────
cat >"$HOOK_DIR/pre-push" <<'HOOK'
#!/usr/bin/env bash
# Auto-installed by scripts/install-git-hooks.sh
# Runs the complete CI suite before every push so green-locally means
# green-on-GitHub.
set -e
exec bash "$(git rev-parse --show-toplevel)/scripts/ci-check.sh"
HOOK
chmod +x "$HOOK_DIR/pre-push"

printf 'installed pre-commit and pre-push hooks at %s\n' "$HOOK_DIR"
