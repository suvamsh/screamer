#!/bin/bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 path/to/Screamer.dmg"
    exit 1
fi

DMG_FILE="$1"

if [ ! -f "$DMG_FILE" ]; then
    echo "Error: DMG not found at $DMG_FILE"
    exit 1
fi

: "${APPLE_ID:?Set APPLE_ID before running notarize_dmg.sh}"
: "${APPLE_APP_SPECIFIC_PASSWORD:?Set APPLE_APP_SPECIFIC_PASSWORD before running notarize_dmg.sh}"
: "${APPLE_TEAM_ID:?Set APPLE_TEAM_ID before running notarize_dmg.sh}"

echo "=== Notarizing $DMG_FILE ==="
xcrun notarytool submit "$DMG_FILE" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait

echo "Stapling notarization ticket..."
xcrun stapler staple "$DMG_FILE"

echo "Validating Gatekeeper acceptance..."
spctl -a -t open --context context:primary-signature -vv "$DMG_FILE"

echo "=== Done: notarized $DMG_FILE ==="
