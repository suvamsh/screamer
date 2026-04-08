#!/bin/bash
set -euo pipefail

APP="${APP:-Screamer-smoke.app}"
BIN_PATH="${BIN_PATH:-target/release/screamer}"
PLUTIL_BIN="${PLUTIL_BIN:-/usr/bin/plutil}"
CODESIGN_BIN="${CODESIGN_BIN:-/usr/bin/codesign}"

if [ ! -f "$BIN_PATH" ]; then
    echo "Error: binary not found at $BIN_PATH"
    exit 1
fi

TMP_MODELS_DIR="$(mktemp -d "${TMPDIR:-/tmp}/screamer-models.XXXXXX")"
TMP_SUMMARY_MODELS_DIR="$(mktemp -d "${TMPDIR:-/tmp}/screamer-summary-models.XXXXXX")"
cleanup() {
    rm -rf "$TMP_MODELS_DIR"
    rm -rf "$TMP_SUMMARY_MODELS_DIR"
}
trap cleanup EXIT

for model in ggml-tiny.en.bin ggml-base.en.bin ggml-small.en.bin; do
    printf 'placeholder model\n' > "$TMP_MODELS_DIR/$model"
done
printf 'placeholder summary model\n' > "$TMP_SUMMARY_MODELS_DIR/gemma-3-1b-it-q4_k_m.gguf"
printf 'placeholder vision model\n' > "$TMP_SUMMARY_MODELS_DIR/gemma-3-4b-it-q4.gguf"

SUMMARY_HELPER_PATH="${SUMMARY_HELPER_PATH:-target/release/screamer_summary_helper}"
VISION_HELPER_PATH="${VISION_HELPER_PATH:-target/release/screamer_vision_helper}"

APP="$APP" \
  MODELS_DIR="$TMP_MODELS_DIR" \
  SUMMARY_MODELS_DIR="$TMP_SUMMARY_MODELS_DIR" \
  SKIP_BUILD=1 \
  BIN_PATH="$BIN_PATH" \
  SUMMARY_HELPER_PATH="$SUMMARY_HELPER_PATH" \
  VISION_HELPER_PATH="$VISION_HELPER_PATH" \
  ./bundle.sh

test -x "$APP/Contents/MacOS/Screamer"
test -f "$APP/Contents/Info.plist"
test -f "$APP/Contents/Resources/icon.icns"
test -f "$APP/Contents/Resources/image.png"
test -f "$APP/Contents/Resources/models/ggml-tiny.en.bin"
test -f "$APP/Contents/Resources/models/ggml-base.en.bin"
test -f "$APP/Contents/Resources/models/ggml-small.en.bin"
test -f "$APP/Contents/Resources/models/summary/gemma-3-1b-it-q4_k_m.gguf"
test -f "$APP/Contents/Resources/models/summary/gemma-3-4b-it-q4.gguf"

bundle_id="$("$PLUTIL_BIN" -extract CFBundleIdentifier raw "$APP/Contents/Info.plist")"
bundle_exec="$("$PLUTIL_BIN" -extract CFBundleExecutable raw "$APP/Contents/Info.plist")"

if [ "$bundle_id" != "com.screamer.app" ]; then
    echo "Error: unexpected bundle identifier: $bundle_id"
    exit 1
fi

if [ "$bundle_exec" != "Screamer" ]; then
    echo "Error: unexpected bundle executable: $bundle_exec"
    exit 1
fi

"$CODESIGN_BIN" --verify --deep --strict --verbose=2 "$APP"

echo "macOS bundle smoke test passed."
