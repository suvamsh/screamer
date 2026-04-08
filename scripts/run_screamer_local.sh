#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${SCREAMER_ENV_FILE:-$ROOT_DIR/.env.screamer.local}"

if [ -f "$ENV_FILE" ]; then
    # shellcheck disable=SC1090
    source "$ENV_FILE"
fi

if [ -n "${SCREAMER_AMBIENT_DIARIZATION_DIR:-}" ]; then
    echo "Using ambient diarization assets from: $SCREAMER_AMBIENT_DIARIZATION_DIR"
fi

cd "$ROOT_DIR"
exec cargo run --release
