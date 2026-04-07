#!/bin/bash
set -euo pipefail

APP="${APP:-Screamer.app}"
CONTENTS="$APP/Contents"
INFO_PLIST_TEMPLATE="${INFO_PLIST_TEMPLATE:-resources/Info.plist}"
INFO_PLIST="$CONTENTS/Info.plist"
MODELS_DIR="${MODELS_DIR:-models}"
SUMMARY_MODELS_DIR="${SUMMARY_MODELS_DIR:-models/summary}"
BIN_PATH="${BIN_PATH:-target/release/screamer}"
SUMMARY_HELPER_PATH="${SUMMARY_HELPER_PATH:-target/release/screamer_summary_helper}"
VISION_HELPER_PATH="${VISION_HELPER_PATH:-target/release/screamer_vision_helper}"
PLIST_BUDDY="${PLIST_BUDDY:-/usr/libexec/PlistBuddy}"
SYSTEM_CODESIGN="${SYSTEM_CODESIGN:-/usr/bin/codesign}"
SYSTEM_SECURITY="${SYSTEM_SECURITY:-/usr/bin/security}"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:-}"
DEFAULT_ENTITLEMENTS="resources/Screamer.entitlements"
CODESIGN_ENTITLEMENTS="${CODESIGN_ENTITLEMENTS:-$DEFAULT_ENTITLEMENTS}"
APP_VERSION="${APP_VERSION:-$(awk -F '\"' '/^version = / { print $2; exit }' Cargo.toml)}"
SKIP_BUILD="${SKIP_BUILD:-0}"
REQUIRED_MODELS=(
    "ggml-tiny.en.bin"
    "ggml-base.en.bin"
    "ggml-small.en.bin"
)
REQUIRED_SUMMARY_MODELS=(
    "gemma-3-1b-it-q4_k_m.gguf"
)
VISION_MODELS=(
    "gemma-3-4b-it-q4.gguf"
    "mmproj-gemma-3-4b-it-f16.gguf"
)

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

sign_target() {
    local target="$1"
    local sign_args=(--force --sign "$CODESIGN_IDENTITY")

    if [ "$CODESIGN_IDENTITY" != "-" ]; then
        sign_args+=(--timestamp --options runtime)
        if [ -n "$CODESIGN_ENTITLEMENTS" ] && [ -f "$CODESIGN_ENTITLEMENTS" ]; then
            sign_args+=(--entitlements "$CODESIGN_ENTITLEMENTS")
        fi
    fi

    if "$SYSTEM_CODESIGN" "${sign_args[@]}" "$target"; then
        return 0
    fi

    if [ "$CODESIGN_IDENTITY" = "-" ]; then
        return 1
    fi

    echo "Warning: signing with $CODESIGN_IDENTITY failed; falling back to ad-hoc signing." >&2
    CODESIGN_IDENTITY="-"
    "$SYSTEM_CODESIGN" --force --sign - "$target"
}

echo "=== Building Screamer ==="
echo "App version: $APP_VERSION"
if [ "$CODESIGN_IDENTITY" = "-" ]; then
    echo "Signing identity: ad-hoc"
else
    echo "Signing identity: $CODESIGN_IDENTITY"
fi
if [ -n "$CODESIGN_ENTITLEMENTS" ] && [ -f "$CODESIGN_ENTITLEMENTS" ]; then
    echo "Signing entitlements: $CODESIGN_ENTITLEMENTS"
fi

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

if [ ! -f "$SUMMARY_HELPER_PATH" ]; then
    echo "Error: summary helper binary not found at $SUMMARY_HELPER_PATH"
    echo "Run cargo build --release and ensure the helper target builds successfully."
    exit 1
fi

# Step 2: Assemble .app bundle.
echo "Assembling app bundle..."
rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS"
mkdir -p "$CONTENTS/Resources/models"
mkdir -p "$CONTENTS/Resources/models/summary"

cp "$BIN_PATH" "$CONTENTS/MacOS/Screamer"
cp "$SUMMARY_HELPER_PATH" "$CONTENTS/MacOS/screamer_summary_helper"
if [ -f "$VISION_HELPER_PATH" ]; then
    cp "$VISION_HELPER_PATH" "$CONTENTS/MacOS/screamer_vision_helper"
fi
cp "$INFO_PLIST_TEMPLATE" "$INFO_PLIST"
cp resources/icon.icns "$CONTENTS/Resources/"
cp resources/image.png "$CONTENTS/Resources/"
cp resources/menubarTemplate.png "$CONTENTS/Resources/"
cp resources/menubarTemplate@2x.png "$CONTENTS/Resources/"

if [ -x "$PLIST_BUDDY" ]; then
    "$PLIST_BUDDY" -c "Set :CFBundleShortVersionString $APP_VERSION" "$INFO_PLIST"
    "$PLIST_BUDDY" -c "Set :CFBundleVersion $APP_VERSION" "$INFO_PLIST"
fi

# Step 3: Copy required bundled models.
if [ ! -d "$MODELS_DIR" ]; then
    echo "Error: models directory not found at $MODELS_DIR"
    echo "Run ./download_model.sh bundled first."
    exit 1
fi

missing_models=()
for model_name in "${REQUIRED_MODELS[@]}"; do
    model_path="$MODELS_DIR/$model_name"
    if [ ! -f "$model_path" ]; then
        missing_models+=("$model_name")
        continue
    fi
    if [ ! -s "$model_path" ]; then
        echo "Error: bundled whisper model is empty: $model_path"
        exit 1
    fi

    echo "Bundling model: $model_name"
    cp "$model_path" "$CONTENTS/Resources/models/"
done

if [ "${#missing_models[@]}" -ne 0 ]; then
    echo "Error: missing required bundled models:"
    printf '  - %s\n' "${missing_models[@]}"
    echo "Run ./download_model.sh bundled and try again."
    exit 1
fi

if [ ! -d "$SUMMARY_MODELS_DIR" ]; then
    echo "Warning: summary models directory not found at $SUMMARY_MODELS_DIR"
    echo "Continuing without bundled summary GGUF; the app will fall back to the offline heuristic summarizer."
else
    missing_summary_models=()
    for model_name in "${REQUIRED_SUMMARY_MODELS[@]}"; do
        model_path="$SUMMARY_MODELS_DIR/$model_name"
        if [ ! -f "$model_path" ]; then
            missing_summary_models+=("$model_name")
            continue
        fi
        if [ ! -s "$model_path" ]; then
            echo "Error: bundled summary model is empty: $model_path"
            exit 1
        fi

        echo "Bundling summary model: $model_name"
        cp "$model_path" "$CONTENTS/Resources/models/summary/"
    done

    if [ "${#missing_summary_models[@]}" -ne 0 ]; then
        echo "Warning: missing bundled summary models:"
        printf '  - %s\n' "${missing_summary_models[@]}"
        echo "Continuing without bundled summary GGUF; the app will fall back to the offline heuristic summarizer."
    fi
fi

# Bundle vision models (optional — vision feature degrades gracefully without them).
for model_name in "${VISION_MODELS[@]}"; do
    model_path="$SUMMARY_MODELS_DIR/$model_name"
    if [ -f "$model_path" ] && [ -s "$model_path" ]; then
        echo "Bundling vision model: $model_name"
        cp "$model_path" "$CONTENTS/Resources/models/summary/"
    else
        echo "Skipping vision model (not found): $model_name"
    fi
done

if [ ! -x "$SYSTEM_CODESIGN" ]; then
    echo "Error: Apple codesign tool not found at $SYSTEM_CODESIGN"
    exit 1
fi

echo "Signing app bundle with Apple codesign..."
sign_target "$CONTENTS/MacOS/screamer_summary_helper"
if [ -f "$CONTENTS/MacOS/screamer_vision_helper" ]; then
    sign_target "$CONTENTS/MacOS/screamer_vision_helper"
fi
sign_target "$CONTENTS/MacOS/Screamer"
sign_target "$APP"
"$SYSTEM_CODESIGN" --verify --deep --strict --verbose=2 "$APP"

echo "=== Done: $APP ==="
echo "To run: open $APP"
