#!/bin/bash
set -euo pipefail

APP="${APP:-Screamer.app}"
BIN_PATH="${BIN_PATH:-target/release/screamer}"
PLUTIL_BIN="${PLUTIL_BIN:-/usr/bin/plutil}"
CODESIGN_BIN="${CODESIGN_BIN:-/usr/bin/codesign}"

if [ ! -f "$BIN_PATH" ]; then
    echo "Error: binary not found at $BIN_PATH"
    exit 1
fi

TMP_MODELS_DIR="$(mktemp -d "${TMPDIR:-/tmp}/screamer-models.XXXXXX")"
cleanup() {
    rm -rf "$TMP_MODELS_DIR"
}
trap cleanup EXIT

for model in ggml-tiny.en.bin ggml-base.en.bin ggml-small.en.bin; do
    : > "$TMP_MODELS_DIR/$model"
done

MODELS_DIR="$TMP_MODELS_DIR" SKIP_BUILD=1 BIN_PATH="$BIN_PATH" ./bundle.sh

test -x "$APP/Contents/MacOS/Screamer"
test -f "$APP/Contents/Info.plist"
test -f "$APP/Contents/Resources/icon.icns"
test -f "$APP/Contents/Resources/image.png"
test -f "$APP/Contents/Resources/models/ggml-tiny.en.bin"
test -f "$APP/Contents/Resources/models/ggml-base.en.bin"
test -f "$APP/Contents/Resources/models/ggml-small.en.bin"

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
