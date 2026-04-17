#!/usr/bin/env bash
# Regression smoke-test: open every known game install and extract one canonical
# file from each.  Runs on Windows (Git Bash / MSYS2) and Linux/SteamOS.
#
# Usage:
#   ./scripts/regress.sh              # uses dev build
#   ./scripts/regress.sh --release    # uses release build
#
# Exit code: 0 if all pass, 1 if any fail.

set -euo pipefail

RELEASE=0
for arg in "$@"; do
  [[ "$arg" == "--release" ]] && RELEASE=1
done

# ── Build ──────────────────────────────────────────────────────────────────────
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_FLAGS="--features rustydemon-lib/cdn"
[[ $RELEASE -eq 1 ]] && BUILD_FLAGS="$BUILD_FLAGS --release"

echo "Building rustydemon-cli..."
cargo build -p rustydemon-cli $BUILD_FLAGS -q 2>&1

BIN_DIR="$REPO/target/$([ $RELEASE -eq 1 ] && echo release || echo debug)"
CLI="$BIN_DIR/rustydemon-cli"
[[ -x "$CLI.exe" ]] && CLI="$CLI.exe"

# ── Game install candidates ────────────────────────────────────────────────────
# Add your own paths here.  Non-existent paths are silently skipped.
INSTALLS=(
  # Windows — Steam
  "C:/Program Files (x86)/Steam/steamapps/common/Diablo II Resurrected"
  "C:/Program Files (x86)/Steam/steamapps/common/Diablo IV"
  "C:/Program Files (x86)/Steam/steamapps/common/World of Warcraft"
  "C:/Program Files (x86)/Steam/steamapps/common/StarCraft"
  "C:/Program Files (x86)/Steam/steamapps/common/Heroes of the Storm"
  "C:/Program Files (x86)/Steam/steamapps/common/Overwatch"
  # Windows — Battle.net
  "C:/Program Files (x86)/Diablo IV"
  "C:/Program Files (x86)/Diablo IV Public Test"
  "C:/Program Files (x86)/World of Warcraft"
  "C:/Program Files (x86)/StarCraft"
  "C:/Program Files (x86)/Diablo III"
  "C:/Program Files (x86)/Heroes of the Storm"
  # Linux / SteamOS — Steam
  "$HOME/.local/share/Steam/steamapps/common/Diablo II Resurrected"
  "$HOME/.local/share/Steam/steamapps/common/Diablo IV"
  "$HOME/.local/share/Steam/steamapps/common/World of Warcraft"
  "$HOME/.local/share/Steam/steamapps/common/StarCraft"
  # Steam Deck SD card
  "/run/media/mmcblk0p1/steamapps/common/Diablo II Resurrected"
  "/run/media/mmcblk0p1/steamapps/common/Diablo IV"
  "/run/media/sdcard/steamapps/common/Diablo II Resurrected"
  "/run/media/sdcard/steamapps/common/Diablo IV"
)

# ── Filter to existing paths ───────────────────────────────────────────────────
FOUND=()
for p in "${INSTALLS[@]}"; do
  [[ -d "$p" ]] && FOUND+=("$p")
done

if [[ ${#FOUND[@]} -eq 0 ]]; then
  echo "No known game installs found. Add paths to INSTALLS in $0."
  exit 1
fi

echo "Probing ${#FOUND[@]} install(s)..."
echo

# ── Run probe ─────────────────────────────────────────────────────────────────
PROBE_ARGS=()
for p in "${FOUND[@]}"; do
  PROBE_ARGS+=("-a" "$p")
done

"$CLI" probe "${PROBE_ARGS[@]}"
