#!/bin/bash
set -euo pipefail

APP="${APP:-Screamer.app}"
OUTPUT_DIR="${OUTPUT_DIR:-dist}"
DMG_TEMP="${DMG_TEMP:-dmg_temp}"
PLIST_BUDDY="${PLIST_BUDDY:-/usr/libexec/PlistBuddy}"
SYSTEM_CODESIGN="${SYSTEM_CODESIGN:-/usr/bin/codesign}"
SYSTEM_SECURITY="${SYSTEM_SECURITY:-/usr/bin/security}"
APP_NAME="${APP_NAME:-$(basename "$APP" .app)}"
VOLUME_NAME="${VOLUME_NAME:-$APP_NAME}"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:-${APPLE_SIGNING_IDENTITY:-}}"

detect_codesign_identity() {
    if [ -n "$CODESIGN_IDENTITY" ]; then
        echo "$CODESIGN_IDENTITY"
        return
    fi

    if [ ! -x "$SYSTEM_SECURITY" ]; then
        echo "-"
        return
    fi

    local detected_identity
    detected_identity="$("$SYSTEM_SECURITY" find-identity -v -p codesigning 2>/dev/null \
        | awk -F '"' '/Developer ID Application:/ { print $2; exit }')"

    if [ -n "$detected_identity" ]; then
        echo "$detected_identity"
    else
        echo "-"
    fi
}

CODESIGN_IDENTITY="$(detect_codesign_identity)"

if [ ! -d "$APP" ]; then
    echo "Error: $APP not found. Run ./bundle.sh first."
    exit 1
fi

APP_VERSION="${APP_VERSION:-$("$PLIST_BUDDY" -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")}"
DMG_NAME="${DMG_NAME:-${APP_NAME}-${APP_VERSION}}"
DMG_FILE="$OUTPUT_DIR/${DMG_NAME}.dmg"

echo "=== Creating DMG ==="
echo "Output: $DMG_FILE"

mkdir -p "$OUTPUT_DIR"

# Clean up any previous temp dir or stale output.
rm -rf "$DMG_TEMP"
rm -f "$DMG_FILE"

# Create temp directory with app and Applications symlink.
mkdir -p "$DMG_TEMP"
cp -R "$APP" "$DMG_TEMP/"
ln -s /Applications "$DMG_TEMP/Applications"

hdiutil create \
    -volname "$VOLUME_NAME" \
    -srcfolder "$DMG_TEMP" \
    -ov \
    -format UDZO \
    "$DMG_FILE"

rm -rf "$DMG_TEMP"

if [ "$CODESIGN_IDENTITY" != "-" ]; then
    echo "Signing DMG with Apple codesign..."
    "$SYSTEM_CODESIGN" --force --sign "$CODESIGN_IDENTITY" --timestamp "$DMG_FILE"
    "$SYSTEM_CODESIGN" --verify --verbose=2 "$DMG_FILE"
else
    echo "Skipping DMG signing because no Developer ID identity is available."
fi

echo "=== Done: $DMG_FILE ==="
echo ""
echo "Users can:"
echo "  1. Double-click $DMG_FILE"
echo "  2. Drag $APP_NAME to Applications"
echo "  3. Open from Applications"
echo ""
echo "For public distribution, notarize with ./notarize_dmg.sh $DMG_FILE"
