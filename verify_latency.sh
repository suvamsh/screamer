#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/screamer-latency.XXXXXX")"
ITERATIONS="${ITERATIONS:-15}"
WARMUP="${WARMUP:-2}"
MODEL="${MODEL:-base}"
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
  ffmpeg -hide_banner -loglevel error -y -i "$aiff" -ac 1 -ar 16000 -f f32le "$raw"

  printf "%-16s %6.2fs  %s\n" "$name" "$(duration_seconds "$aiff")" "$text"
}

require_cmd say
require_cmd ffmpeg
require_cmd ffprobe

echo "Screamer latency verification"
echo "  machine: $(sysctl -n machdep.cpu.brand_string)"
echo "  model: $MODEL"
echo "  temp dir: $TMP_DIR"
echo "  whisper log: $WHISPER_LOG"
echo
echo "Synthesizing benchmark audio:"
synthesize "short_phrase" "Schedule lunch with Maya tomorrow."
synthesize "sentence" "I shipped the recorder fix this morning, and it looks stable."
synthesize "long_paragraph" "I shipped the recorder fix this morning, and I want to test the release build again before we publish the update."
echo

cd "$ROOT"
cargo build --release --bin latency_bench
target/release/latency_bench \
  --model "$MODEL" \
  --warmup "$WARMUP" \
  --iterations "$ITERATIONS" \
  "$TMP_DIR/short_phrase.f32" \
  "$TMP_DIR/sentence.f32" \
  "$TMP_DIR/long_paragraph.f32" \
  2>"$WHISPER_LOG"
