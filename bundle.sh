#!/bin/bash
set -e

APP="Screamer.app"
CONTENTS="$APP/Contents"
MODELS_DIR="models"

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

# Step 4: Ad-hoc sign the executable
echo "Signing executable..."
codesign --force --sign - "$CONTENTS/MacOS/Screamer"

echo "=== Done: $APP ==="
echo "To run: open $APP"
