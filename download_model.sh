#!/bin/bash
set -e

MODELS_DIR="models"
mkdir -p "$MODELS_DIR"

MODEL="${1:-base}"

case "$MODEL" in
    base)
        URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
        FILE="ggml-base.en.bin"
        ;;
    small)
        URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin"
        FILE="ggml-small.en.bin"
        ;;
    *)
        echo "Usage: $0 [base|small]"
        exit 1
        ;;
esac

DEST="$MODELS_DIR/$FILE"

if [ -f "$DEST" ]; then
    echo "Model already exists: $DEST"
    exit 0
fi

echo "Downloading $FILE..."
curl -L -o "$DEST" "$URL" --progress-bar

echo "Done: $DEST ($(du -h "$DEST" | cut -f1))"
