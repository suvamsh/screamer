#!/bin/bash
set -e

APP="Screamer.app"
DMG_NAME="Screamer"
DMG_TEMP="dmg_temp"
DMG_FILE="${DMG_NAME}.dmg"
VOLUME_NAME="Screamer"

if [ ! -d "$APP" ]; then
    echo "Error: $APP not found. Run ./bundle.sh first."
    exit 1
fi

echo "=== Creating DMG ==="

# Clean up any previous temp dir
rm -rf "$DMG_TEMP" "$DMG_FILE"

# Create temp directory with app and Applications symlink
mkdir -p "$DMG_TEMP"
cp -R "$APP" "$DMG_TEMP/"
ln -s /Applications "$DMG_TEMP/Applications"

# Create the DMG
hdiutil create \
    -volname "$VOLUME_NAME" \
    -srcfolder "$DMG_TEMP" \
    -ov \
    -format UDZO \
    "$DMG_FILE"

# Clean up
rm -rf "$DMG_TEMP"

echo "=== Done: $DMG_FILE ==="
echo ""
echo "Users can:"
echo "  1. Double-click $DMG_FILE"
echo "  2. Drag Screamer to Applications"
echo "  3. Open from Applications"
echo ""
echo "Note: For public distribution, you'll want to:"
echo "  - Sign with a Developer ID: codesign --force --sign 'Developer ID Application: Your Name' $APP"
echo "  - Notarize with Apple: xcrun notarytool submit $DMG_FILE --apple-id ... --team-id ... --password ..."
echo "  - Without notarization, users will see Gatekeeper warnings and need to right-click > Open"
