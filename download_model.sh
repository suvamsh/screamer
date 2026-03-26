#!/bin/bash
set -e

MODELS_DIR="models"
mkdir -p "$MODELS_DIR"

MODEL="${1:-base}"

BASE_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"

case "$MODEL" in
    tiny)
        URL="$BASE_URL/ggml-tiny.en.bin"
        FILE="ggml-tiny.en.bin"
        SIZE="~75 MB"
        ;;
    base)
        URL="$BASE_URL/ggml-base.en.bin"
        FILE="ggml-base.en.bin"
        SIZE="~142 MB"
        ;;
    small)
        URL="$BASE_URL/ggml-small.en.bin"
        FILE="ggml-small.en.bin"
        SIZE="~466 MB"
        ;;
    large)
        URL="$BASE_URL/ggml-large-v3.bin"
        FILE="ggml-large.en.bin"
        SIZE="~3.1 GB"
        ;;
    all)
        echo "Downloading all models..."
        "$0" tiny
        "$0" base
        "$0" small
        "$0" large
        echo ""
        echo "All models downloaded!"
        ls -lh "$MODELS_DIR"/*.bin
        exit 0
        ;;
    *)
        echo "Usage: $0 [tiny|base|small|large|all]"
        echo ""
        echo "Models:"
        echo "  tiny   ~75 MB   Fastest, good for simple phrases"
        echo "  base   ~142 MB  Fast, great for most use cases (default)"
        echo "  small  ~466 MB  Moderate speed, better accuracy"
        echo "  large  ~3.1 GB  Slowest, highest accuracy"
        echo "  all    Download all models"
        exit 1
        ;;
esac

DEST="$MODELS_DIR/$FILE"

if [ -f "$DEST" ]; then
    echo "Model already exists: $DEST ($(du -h "$DEST" | cut -f1))"
    exit 0
fi

echo "Downloading $FILE ($SIZE)..."
curl -L -o "$DEST" "$URL" --progress-bar

echo "Done: $DEST ($(du -h "$DEST" | cut -f1))"
