#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import shutil
import sys
from pathlib import Path


def default_asset_root() -> Path:
    home = Path.home()
    return home / "Library" / "Application Support" / "Screamer" / "models" / "ambient-diarization"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def require_file(path: str) -> Path:
    file_path = Path(path).expanduser().resolve()
    if not file_path.is_file():
        raise SystemExit(f"Missing file: {file_path}")
    return file_path


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Install exported ambient diarization ONNX assets into Screamer's local cache."
    )
    parser.add_argument("--asset-version", required=True, help="Version label to store under the ambient-diarization cache root.")
    parser.add_argument("--segmentation-onnx", required=True, help="Path to the exported segmentation ONNX model.")
    parser.add_argument("--embedding-onnx", required=True, help="Path to the exported speaker embedding ONNX model.")
    parser.add_argument(
        "--output-root",
        default=str(default_asset_root()),
        help="Ambient diarization cache root. Defaults to ~/Library/Application Support/Screamer/models/ambient-diarization",
    )
    parser.add_argument(
        "--backend-kind",
        default="onnx_runtime_v1",
        help="Backend kind to record in the manifest. Defaults to onnx_runtime_v1.",
    )
    parser.add_argument("--segmentation-input-name", help="Optional ONNX input name for the segmentation model.")
    parser.add_argument("--segmentation-output-name", help="Optional ONNX output name for the segmentation model.")
    parser.add_argument(
        "--segmentation-input-layout",
        default="batch_samples",
        choices=["batch_samples", "batch_channel_samples"],
        help="Segmentation model waveform input layout. Defaults to batch_samples.",
    )
    parser.add_argument(
        "--segmentation-output-layout",
        default="batch_frames_speakers",
        choices=["frames_speakers", "batch_frames_speakers", "batch_speakers_frames"],
        help="Segmentation model activity output layout. Defaults to batch_frames_speakers.",
    )
    parser.add_argument(
        "--segmentation-window-ms",
        type=int,
        default=5000,
        help="Sliding-window length for segmentation inference. Defaults to 5000.",
    )
    parser.add_argument(
        "--segmentation-hop-ms",
        type=int,
        default=2500,
        help="Sliding-window hop for segmentation inference. Defaults to 2500.",
    )
    parser.add_argument(
        "--segmentation-frame-hop-ms",
        type=int,
        default=20,
        help="Frame hop represented by the segmentation output. Defaults to 20.",
    )
    parser.add_argument(
        "--segmentation-activation-threshold",
        type=float,
        default=0.4,
        help="Speech activity threshold applied to segmentation output. Defaults to 0.4.",
    )
    parser.add_argument(
        "--segmentation-min-speech-ms",
        type=int,
        default=200,
        help="Minimum retained speech span in milliseconds. Defaults to 200.",
    )
    parser.add_argument(
        "--segmentation-min-silence-ms",
        type=int,
        default=160,
        help="Maximum silence gap to fill between speech spans in milliseconds. Defaults to 160.",
    )
    parser.add_argument("--embedding-input-name", help="Optional ONNX input name for the embedding model.")
    parser.add_argument("--embedding-output-name", help="Optional ONNX output name for the embedding model.")
    parser.add_argument(
        "--embedding-input-layout",
        default="batch_samples",
        choices=["batch_samples", "batch_channel_samples"],
        help="Embedding model waveform input layout. Defaults to batch_samples.",
    )
    parser.add_argument(
        "--embedding-output-layout",
        default="batch_embedding_vector",
        choices=["embedding_vector", "batch_embedding_vector"],
        help="Embedding model output layout. Defaults to batch_embedding_vector.",
    )
    parser.add_argument(
        "--embedding-target-samples",
        type=int,
        help="Optional exact sample count to center-pad or crop embedding inputs to.",
    )
    parser.add_argument(
        "--segmentation-cache-subdir",
        default="segmentation-coreml-cache",
        help="CoreML cache subdirectory for the segmentation model.",
    )
    parser.add_argument(
        "--embedding-cache-subdir",
        default="embedding-coreml-cache",
        help="CoreML cache subdirectory for the embedding model.",
    )
    parser.add_argument(
        "--clustering-similarity-threshold",
        type=float,
        default=0.9,
        help="Agglomerative clustering similarity threshold. Defaults to 0.9.",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Overwrite an existing asset-version directory.",
    )
    args = parser.parse_args()

    segmentation = require_file(args.segmentation_onnx)
    embedding = require_file(args.embedding_onnx)

    target_root = Path(args.output_root).expanduser().resolve()
    target_dir = target_root / args.asset_version
    if target_dir.exists():
        if not args.force:
            raise SystemExit(
                f"Target asset directory already exists: {target_dir}\n"
                "Pass --force to overwrite it."
            )
        shutil.rmtree(target_dir)

    os.makedirs(target_dir, exist_ok=True)
    target_segmentation = target_dir / "segmentation.onnx"
    target_embedding = target_dir / "embedding.onnx"
    shutil.copy2(segmentation, target_segmentation)
    shutil.copy2(embedding, target_embedding)

    manifest = {
        "format_version": 1,
        "asset_version": args.asset_version,
        "backend_kind": args.backend_kind,
        "files": [
            {
                "relative_path": target_segmentation.name,
                "sha256": sha256_file(target_segmentation),
                "required": True,
            },
            {
                "relative_path": target_embedding.name,
                "sha256": sha256_file(target_embedding),
                "required": True,
            },
        ],
        "pipeline": {
            "segmentation": {
                "relative_path": target_segmentation.name,
                "input_name": args.segmentation_input_name,
                "output_name": args.segmentation_output_name,
                "sample_rate_hz": 16000,
                "input_layout": args.segmentation_input_layout,
                "output_layout": args.segmentation_output_layout,
                "window_ms": args.segmentation_window_ms,
                "hop_ms": args.segmentation_hop_ms,
                "frame_hop_ms": args.segmentation_frame_hop_ms,
                "activation_threshold": args.segmentation_activation_threshold,
                "min_speech_ms": args.segmentation_min_speech_ms,
                "min_silence_ms": args.segmentation_min_silence_ms,
                "model_cache_subdir": args.segmentation_cache_subdir,
            },
            "embedding": {
                "relative_path": target_embedding.name,
                "input_name": args.embedding_input_name,
                "output_name": args.embedding_output_name,
                "sample_rate_hz": 16000,
                "input_layout": args.embedding_input_layout,
                "output_layout": args.embedding_output_layout,
                "target_samples": args.embedding_target_samples,
                "window_ms": 3000,
                "hop_ms": 3000,
                "frame_hop_ms": 20,
                "activation_threshold": 0.4,
                "min_speech_ms": 200,
                "min_silence_ms": 160,
                "model_cache_subdir": args.embedding_cache_subdir,
            },
            "clustering_similarity_threshold": args.clustering_similarity_threshold,
        },
    }

    manifest_path = target_dir / "manifest.json"
    with manifest_path.open("w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2)
        handle.write("\n")

    print(f"Installed ambient diarization assets to {target_dir}")
    print(f"Manifest: {manifest_path}")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
