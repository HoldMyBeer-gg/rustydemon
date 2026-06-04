#!/usr/bin/env bash
# macos-bundle.sh — build RustyDemon.app and package it into a DMG.
#
# Why this exists: cargo-dist ships the bare Unix binary in a tarball.
# Double-clicking that in Finder launches it through Terminal, which is
# the stray "command window" you see next to the app. A proper .app
# bundle is launched by LaunchServices with no terminal at all.
#
# Uses only tools that ship with macOS (sips, iconutil, hdiutil,
# codesign, lipo) plus cargo. No extra installs required.
#
# Usage:
#   ./scripts/macos-bundle.sh              # build for host arch, ad-hoc sign
#   ./scripts/macos-bundle.sh --universal  # build x86_64 + arm64 fat binary
#
# Real Dev ID signing + notarization hooks are marked TODO at the bottom.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

# ── Identity ──────────────────────────────────────────────────────────────────
APP_NAME="RustyDemon"
DISPLAY_NAME="Rusty Demon"
BIN_NAME="rustydemon"
BUNDLE_ID="gg.holdmybeer.rustydemon"
VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' Cargo.toml | head -1)"
ICON_SRC="$ROOT/assets/icons/icon_1024.png"

UNIVERSAL=false
[[ "${1-}" == "--universal" ]] && UNIVERSAL=true

OUT="$ROOT/target/macos"
APP="$OUT/$APP_NAME.app"
DMG="$OUT/$APP_NAME-$VERSION.dmg"

info() { echo "[bundle] $*"; }
die()  { echo "[bundle] ERROR: $*" >&2; exit 1; }

[[ -f "$ICON_SRC" ]] || die "icon source not found: $ICON_SRC"

# ── 1. Build the binary ───────────────────────────────────────────────────────
if $UNIVERSAL; then
    info "Building universal (x86_64 + arm64) release binary …"
    for t in x86_64-apple-darwin aarch64-apple-darwin; do
        rustup target list --installed | grep -q "^$t$" \
            || die "target $t not installed — run: rustup target add $t"
        cargo build --release -p "$BIN_NAME" --target "$t"
    done
    BIN="$OUT/$BIN_NAME-universal"
    mkdir -p "$OUT"
    lipo -create -output "$BIN" \
        "target/x86_64-apple-darwin/release/$BIN_NAME" \
        "target/aarch64-apple-darwin/release/$BIN_NAME"
else
    info "Building release binary for host arch …"
    cargo build --release -p "$BIN_NAME"
    BIN="target/release/$BIN_NAME"
fi

# ── 2. Generate the .icns from the 1024px PNG ─────────────────────────────────
info "Generating icon set …"
ICONSET="$OUT/$APP_NAME.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for sz in 16 32 128 256 512; do
    sips -z "$sz" "$sz"          "$ICON_SRC" --out "$ICONSET/icon_${sz}x${sz}.png"      >/dev/null
    sips -z $((sz*2)) $((sz*2))  "$ICON_SRC" --out "$ICONSET/icon_${sz}x${sz}@2x.png"   >/dev/null
done
iconutil -c icns "$ICONSET" -o "$OUT/$APP_NAME.icns"

# ── 3. Assemble the .app bundle ───────────────────────────────────────────────
info "Assembling $APP_NAME.app …"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$BIN_NAME"
chmod +x "$APP/Contents/MacOS/$BIN_NAME"
cp "$OUT/$APP_NAME.icns" "$APP/Contents/Resources/$APP_NAME.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>            <string>$DISPLAY_NAME</string>
    <key>CFBundleDisplayName</key>     <string>$DISPLAY_NAME</string>
    <key>CFBundleExecutable</key>      <string>$BIN_NAME</string>
    <key>CFBundleIdentifier</key>      <string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key>         <string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundlePackageType</key>     <string>APPL</string>
    <key>CFBundleIconFile</key>        <string>$APP_NAME</string>
    <key>LSMinimumSystemVersion</key>  <string>11.0</string>
    <key>NSHighResolutionCapable</key> <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

echo -n "APPL????" > "$APP/Contents/PkgInfo"

# ── 4. Sign ───────────────────────────────────────────────────────────────────
# Ad-hoc signature (-s -) so Gatekeeper lets it run on this machine. Replace
# with a Developer ID identity + notarization for public distribution (below).
info "Ad-hoc signing …"
codesign --force --deep --sign - --timestamp=none "$APP"

# ── 5. Build the DMG ──────────────────────────────────────────────────────────
info "Building DMG …"
rm -f "$DMG"
STAGE="$OUT/dmg-stage"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"   # drag-to-install target
hdiutil create \
    -volname "$DISPLAY_NAME" \
    -srcfolder "$STAGE" \
    -ov -format UDZO \
    "$DMG" >/dev/null
rm -rf "$STAGE"

info "Done:"
info "  app: $APP"
info "  dmg: $DMG"

# ── TODO: Developer ID signing + notarization (for public release) ────────────
# Once an Apple Developer ID Application cert is in the keychain:
#
#   codesign --force --deep --options runtime --timestamp \
#     --sign "Developer ID Application: YOUR NAME (TEAMID)" "$APP"
#   # rebuild the DMG from the signed .app, then:
#   xcrun notarytool submit "$DMG" --keychain-profile "NOTARY" --wait
#   xcrun stapler staple "$DMG"
#
# Without notarization, users must right-click → Open the first time.
