#!/bin/bash
set -euo pipefail

APP="${APP:-Screamer.app}"
OUTPUT_DIR="${OUTPUT_DIR:-dist}"
DMG_TEMP="${DMG_TEMP:-dmg_temp}"
PLIST_BUDDY="${PLIST_BUDDY:-/usr/libexec/PlistBuddy}"
APP_NAME="${APP_NAME:-$(basename "$APP" .app)}"
VOLUME_NAME="${VOLUME_NAME:-$APP_NAME}"

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

echo "=== Done: $DMG_FILE ==="
echo ""
echo "Users can:"
echo "  1. Double-click $DMG_FILE"
echo "  2. Drag $APP_NAME to Applications"
echo "  3. Open from Applications"
echo ""
echo "For public distribution, sign in bundle.sh and notarize with ./notarize_dmg.sh $DMG_FILE"
