#!/bin/bash
set -euo pipefail

APP="${APP:-Screamer.app}"
CONTENTS="$APP/Contents"
INFO_PLIST_TEMPLATE="${INFO_PLIST_TEMPLATE:-resources/Info.plist}"
INFO_PLIST="$CONTENTS/Info.plist"
MODELS_DIR="${MODELS_DIR:-models}"
BIN_PATH="${BIN_PATH:-target/release/screamer}"
PLIST_BUDDY="${PLIST_BUDDY:-/usr/libexec/PlistBuddy}"
SYSTEM_CODESIGN="${SYSTEM_CODESIGN:-/usr/bin/codesign}"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}"
CODESIGN_ENTITLEMENTS="${CODESIGN_ENTITLEMENTS:-}"
APP_VERSION="${APP_VERSION:-$(awk -F '\"' '/^version = / { print $2; exit }' Cargo.toml)}"
SKIP_BUILD="${SKIP_BUILD:-0}"

sign_target() {
    local target="$1"
    local sign_args=(--force --sign "$CODESIGN_IDENTITY")

    if [ "$CODESIGN_IDENTITY" != "-" ]; then
        sign_args+=(--timestamp --options runtime)
        if [ -n "$CODESIGN_ENTITLEMENTS" ]; then
            sign_args+=(--entitlements "$CODESIGN_ENTITLEMENTS")
        fi
    fi

    "$SYSTEM_CODESIGN" "${sign_args[@]}" "$target"
}

echo "=== Building Screamer ==="
echo "App version: $APP_VERSION"

# Step 1: Build release binary unless a prebuilt one was provided.
if [ "$SKIP_BUILD" = "1" ]; then
    echo "Skipping cargo build and using prebuilt binary at $BIN_PATH"
else
    echo "Compiling release binary..."
    source "$HOME/.cargo/env" 2>/dev/null || true
    cargo build --release
fi

if [ ! -f "$BIN_PATH" ]; then
    echo "Error: binary not found at $BIN_PATH"
    exit 1
fi

# Step 2: Assemble .app bundle.
echo "Assembling app bundle..."
rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS"
mkdir -p "$CONTENTS/Resources/models"

cp "$BIN_PATH" "$CONTENTS/MacOS/Screamer"
cp "$INFO_PLIST_TEMPLATE" "$INFO_PLIST"
cp resources/icon.icns "$CONTENTS/Resources/"
cp resources/image.png "$CONTENTS/Resources/"
cp resources/menubarTemplate.png "$CONTENTS/Resources/"
cp resources/menubarTemplate@2x.png "$CONTENTS/Resources/"

if [ -x "$PLIST_BUDDY" ]; then
    "$PLIST_BUDDY" -c "Set :CFBundleShortVersionString $APP_VERSION" "$INFO_PLIST"
    "$PLIST_BUDDY" -c "Set :CFBundleVersion $APP_VERSION" "$INFO_PLIST"
fi

# Step 3: Copy bundled models if present.
if [ -d "$MODELS_DIR" ]; then
    for model in "$MODELS_DIR"/*.bin; do
        if [ -f "$model" ]; then
            echo "Bundling model: $(basename "$model")"
            cp "$model" "$CONTENTS/Resources/models/"
        fi
    done
fi

if [ ! -x "$SYSTEM_CODESIGN" ]; then
    echo "Error: Apple codesign tool not found at $SYSTEM_CODESIGN"
    exit 1
fi

echo "Signing app bundle with Apple codesign..."
sign_target "$CONTENTS/MacOS/Screamer"
sign_target "$APP"
"$SYSTEM_CODESIGN" --verify --deep --strict --verbose=2 "$APP"

echo "=== Done: $APP ==="
echo "To run: open $APP"
