#!/bin/bash
set -e

APP="Screamer.app"
CONTENTS="$APP/Contents"
MODELS_DIR="models"
SYSTEM_CODESIGN="/usr/bin/codesign"

echo "=== Building Screamer ==="

# Step 1: Build release binary
echo "Compiling release binary..."
source "$HOME/.cargo/env" 2>/dev/null || true
cargo build --release

# Step 2: Assemble .app bundle
echo "Assembling app bundle..."
rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS"
mkdir -p "$CONTENTS/Resources/models"

cp target/release/screamer "$CONTENTS/MacOS/Screamer"
cp resources/Info.plist "$CONTENTS/"
cp resources/icon.icns "$CONTENTS/Resources/"
cp resources/image.png "$CONTENTS/Resources/"
cp resources/menubarTemplate.png "$CONTENTS/Resources/"
cp resources/menubarTemplate@2x.png "$CONTENTS/Resources/"

# Step 3: Copy models
if [ -d "$MODELS_DIR" ]; then
    for model in "$MODELS_DIR"/*.bin; do
        if [ -f "$model" ]; then
            echo "Bundling model: $(basename "$model")"
            cp "$model" "$CONTENTS/Resources/models/"
        fi
    done
fi

# Step 4: Ad-hoc sign the app bundle with Apple's codesign
if [ ! -x "$SYSTEM_CODESIGN" ]; then
    echo "Error: Apple codesign tool not found at $SYSTEM_CODESIGN"
    exit 1
fi

echo "Signing app bundle with Apple codesign..."
"$SYSTEM_CODESIGN" --force --sign - "$APP"
"$SYSTEM_CODESIGN" --verify --deep --strict --verbose=2 "$APP"

echo "=== Done: $APP ==="
echo "To run: open $APP"
