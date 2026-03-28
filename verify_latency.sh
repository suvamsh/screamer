#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/screamer-latency.XXXXXX")"
ITERATIONS="${ITERATIONS:-15}"
WARMUP="${WARMUP:-2}"
MODEL="${MODEL:-base}"
DEVICE_RATE="${DEVICE_RATE:-48000}"
DISPATCH_PASTE="${DISPATCH_PASTE:-1}"
WHISPER_LOG="${WHISPER_LOG:-/tmp/screamer-latency-whisper.log}"

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

duration_seconds() {
  ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$1"
}

synthesize() {
  local name="$1"
  local text="$2"
  local aiff="$TMP_DIR/$name.aiff"
  local raw="$TMP_DIR/$name.f32"

  say -o "$aiff" "$text"
  ffmpeg -hide_banner -loglevel error -y -i "$aiff" -ac 1 -ar "$DEVICE_RATE" -f f32le "$raw"

  printf "%-16s %6.2fs  %s\n" "$name" "$(duration_seconds "$aiff")" "$text"
}

prepare_paste_target() {
  osascript \
    -e 'tell application "TextEdit" to activate' \
    -e 'tell application "TextEdit" to if (count of documents) = 0 then make new document' \
    -e 'tell application "TextEdit" to set text of document 1 to ""' \
    -e 'delay 0.2' >/dev/null
}

require_cmd say
require_cmd ffmpeg
require_cmd ffprobe
if [[ "$DISPATCH_PASTE" == "1" ]]; then
  require_cmd osascript
fi

echo "Screamer app-path latency verification"
echo "  machine: $(sysctl -n machdep.cpu.brand_string)"
echo "  model: $MODEL"
echo "  device sample rate: $DEVICE_RATE Hz"
echo "  dispatch paste: $([[ "$DISPATCH_PASTE" == "1" ]] && echo yes || echo no)"
echo "  temp dir: $TMP_DIR"
echo "  whisper log: $WHISPER_LOG"
echo
echo "Synthesizing benchmark audio:"
synthesize "short_phrase" "Schedule lunch with Maya tomorrow."
synthesize "sentence" "I shipped the recorder fix this morning, and it looks stable."
synthesize "long_paragraph" "I shipped the recorder fix this morning, and I want to test the release build again before we publish the update."
echo

BENCH_ARGS=(
  --model "$MODEL"
  --warmup "$WARMUP"
  --iterations "$ITERATIONS"
  --device-rate "$DEVICE_RATE"
)

if [[ "$DISPATCH_PASTE" == "1" ]]; then
  echo "Preparing TextEdit as the paste target for dispatch timing..."
  prepare_paste_target
  BENCH_ARGS+=(--dispatch-paste)
  echo
fi

cd "$ROOT"
cargo build --release --bin app_path_latency
target/release/app_path_latency \
  "${BENCH_ARGS[@]}" \
  "$TMP_DIR/short_phrase.f32" \
  "$TMP_DIR/sentence.f32" \
  "$TMP_DIR/long_paragraph.f32" \
  2>"$WHISPER_LOG"
