#!/usr/bin/env python3

import json
import os
import re
import sys
import time
from typing import Any, Dict, List, Optional, Tuple


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    sys.exit(1)


def load_request() -> Dict[str, Any]:
    raw = sys.stdin.read()
    if not raw.strip():
        fail("WhisperX helper received an empty request.")
    try:
        request = json.loads(raw)
    except json.JSONDecodeError as exc:
        fail(f"WhisperX helper received invalid JSON: {exc}")
    if not isinstance(request, dict):
        fail("WhisperX helper expected a JSON object request.")
    return request


def require_string(request: Dict[str, Any], key: str) -> str:
    value = request.get(key)
    if not isinstance(value, str) or not value.strip():
        fail(f"WhisperX helper request is missing `{key}`.")
    return value


def normalize_text(text: str) -> str:
    text = re.sub(r"\s+", " ", text)
    text = re.sub(r"\s+([,.;:!?])", r"\1", text)
    text = re.sub(r"([(\[{])\s+", r"\1", text)
    return text.strip()


def ms(seconds: Optional[float]) -> Optional[int]:
    if seconds is None:
        return None
    return max(0, int(round(seconds * 1000.0)))


def get_hf_token() -> Optional[str]:
    for key in ("SCREAMER_HF_TOKEN", "HUGGINGFACE_HUB_TOKEN", "HF_TOKEN"):
        value = os.environ.get(key)
        if value:
            return value
    return None


def load_dependencies():
    try:
        import whisperx  # type: ignore
    except Exception as exc:
        fail(
            "WhisperX helper could not import `whisperx`. "
            "Install it in the Python environment used by Screamer. "
            f"Details: {exc}"
        )

    try:
        from whisperx.diarize import DiarizationPipeline  # type: ignore
    except Exception as exc:
        fail(
            "WhisperX helper could not import diarization support from `whisperx`. "
            f"Details: {exc}"
        )

    return whisperx, DiarizationPipeline


def create_diarization_pipeline(diarization_pipeline_cls: Any, token: str, device: str):
    preferred_model = "pyannote/speaker-diarization-community-1"
    signature_attempts = [
        {
            "model_name": preferred_model,
            "token": token,
            "device": device,
        },
        {
            "model_name": preferred_model,
            "use_auth_token": token,
            "device": device,
        },
        {
            "token": token,
            "device": device,
        },
        {
            "use_auth_token": token,
            "device": device,
        },
    ]
    type_errors: List[str] = []

    for kwargs in signature_attempts:
        try:
            return diarization_pipeline_cls(**kwargs)
        except TypeError as exc:
            type_errors.append(f"{kwargs.keys()}: {exc}")
            continue

    fail(
        "WhisperX diarization pipeline could not be constructed with the installed API. "
        "Tried model-preferring and fallback signatures. "
        f"Details: {' | '.join(type_errors)}"
    )


def diarize_audio(
    diarization_pipeline: Any, audio: Any
) -> Tuple[List[Tuple[float, float, str]], int]:
    diarize_df = diarization_pipeline(audio)
    intervals: List[Tuple[float, float, str]] = []
    for _, row in diarize_df.iterrows():
        intervals.append((float(row["start"]), float(row["end"]), str(row["speaker"])))
    speakers = len({speaker for _, _, speaker in intervals})
    return intervals, speakers


def dominant_speaker(
    start_s: float, end_s: float, intervals: List[Tuple[float, float, str]]
) -> Optional[str]:
    overlaps: Dict[str, float] = {}
    for seg_start, seg_end, speaker in intervals:
        overlap = min(seg_end, end_s) - max(seg_start, start_s)
        if overlap > 0:
            overlaps[speaker] = overlaps.get(speaker, 0.0) + overlap

    if overlaps:
        return max(overlaps.items(), key=lambda item: item[1])[0]

    if not intervals:
        return None

    midpoint = (start_s + end_s) / 2.0
    nearest = min(intervals, key=lambda item: abs(((item[0] + item[1]) / 2.0) - midpoint))
    return nearest[2]


def build_segments_from_aligned_words(segments: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    words: List[Dict[str, Any]] = []
    for segment in segments:
        segment_speaker = segment.get("speaker")
        for word in segment.get("words") or []:
            start_s = word.get("start")
            if start_s is None:
                continue
            end_s = word.get("end", start_s)
            text = word.get("word") or word.get("text") or ""
            if not isinstance(text, str) or not text.strip():
                continue
            words.append(
                {
                    "start_s": float(start_s),
                    "end_s": float(end_s),
                    "speaker": word.get("speaker") or segment_speaker,
                    "text": text,
                }
            )

    if not words:
        return build_segments_from_segment_speakers(segments)

    grouped: List[Dict[str, Any]] = []
    current: Optional[Dict[str, Any]] = None
    for word in words:
        gap_s = 0.0 if current is None else max(0.0, word["start_s"] - current["end_s"])
        should_split = (
            current is None
            or word["speaker"] != current["speaker"]
            or gap_s > 0.85
        )

        if should_split:
            if current is not None:
                grouped.append(
                    {
                        "start_ms": ms(current["start_s"]),
                        "end_ms": max(ms(current["end_s"]) or 0, (ms(current["start_s"]) or 0) + 1),
                        "speaker": current["speaker"],
                        "text": normalize_text("".join(current["pieces"])),
                        "words": current["words"],
                    }
                )
            current = {
                "start_s": word["start_s"],
                "end_s": word["end_s"],
                "speaker": word["speaker"],
                "pieces": [word["text"]],
                "words": [
                    {
                        "start_ms": ms(word["start_s"]),
                        "end_ms": ms(word["end_s"]),
                        "speaker": word["speaker"],
                        "text": normalize_text(word["text"]),
                    }
                ],
            }
            continue

        current["end_s"] = max(current["end_s"], word["end_s"])
        current["pieces"].append(word["text"])
        current["words"].append(
            {
                "start_ms": ms(word["start_s"]),
                "end_ms": ms(word["end_s"]),
                "speaker": word["speaker"],
                "text": normalize_text(word["text"]),
            }
        )

    if current is not None:
        grouped.append(
            {
                "start_ms": ms(current["start_s"]),
                "end_ms": max(ms(current["end_s"]) or 0, (ms(current["start_s"]) or 0) + 1),
                "speaker": current["speaker"],
                "text": normalize_text("".join(current["pieces"])),
                "words": current["words"],
            }
        )

    return [segment for segment in grouped if segment["text"]]


def build_segments_from_segment_speakers(segments: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    rebuilt: List[Dict[str, Any]] = []
    for segment in segments:
        text = normalize_text(segment.get("text", ""))
        if not text:
            continue
        rebuilt.append(
            {
                "start_ms": ms(segment.get("start")) or 0,
                "end_ms": max(ms(segment.get("end")) or 0, (ms(segment.get("start")) or 0) + 1),
                "speaker": segment.get("speaker"),
                "text": text,
                "words": [],
            }
        )
    return rebuilt


def run_whisperx_hybrid(request: Dict[str, Any], whisperx: Any, diarization_pipeline_cls: Any):
    audio_path = require_string(request, "audio_path")
    model_name = require_string(request, "model")
    device = require_string(request, "device")
    compute_type = require_string(request, "compute_type")
    language = request.get("language") or "en"
    token = get_hf_token()
    if not token:
        fail(
            "WhisperX hybrid diarization requires a Hugging Face token. "
            "Set SCREAMER_HF_TOKEN or HUGGINGFACE_HUB_TOKEN and accept the "
            "`pyannote/speaker-diarization-community-1` model terms."
        )

    total_t0 = time.perf_counter()
    audio = whisperx.load_audio(audio_path)

    transcription_t0 = time.perf_counter()
    model = whisperx.load_model(model_name, device, compute_type=compute_type)
    transcription = model.transcribe(audio, batch_size=1, language=language)
    transcription_ms = int((time.perf_counter() - transcription_t0) * 1000)

    alignment_t0 = time.perf_counter()
    model_a, metadata = whisperx.load_align_model(
        language_code=transcription.get("language") or language,
        device=device,
    )
    aligned = whisperx.align(
        transcription["segments"],
        model_a,
        metadata,
        audio,
        device,
        return_char_alignments=False,
    )
    alignment_ms = int((time.perf_counter() - alignment_t0) * 1000)

    diarization_t0 = time.perf_counter()
    diarization_pipeline = create_diarization_pipeline(
        diarization_pipeline_cls, token, device
    )
    diarization_segments, detected_speakers = diarize_audio(diarization_pipeline, audio)
    diarization_ms = int((time.perf_counter() - diarization_t0) * 1000)

    assignment_t0 = time.perf_counter()
    intervals_df = []
    for start_s, end_s, speaker in diarization_segments:
        intervals_df.append({"start": start_s, "end": end_s, "speaker": speaker})
    import pandas as pd  # type: ignore

    diarize_df = pd.DataFrame(intervals_df)
    assigned = whisperx.assign_word_speakers(diarize_df, aligned, fill_nearest=True)
    rebuilt_segments = build_segments_from_aligned_words(assigned.get("segments") or [])
    assignment_ms = int((time.perf_counter() - assignment_t0) * 1000)

    response = {
        "engine": "whisperx_hybrid_v1",
        "transcript_text": " ".join(segment["text"] for segment in rebuilt_segments).strip(),
        "segments": rebuilt_segments,
        "diagnostics": {
            "detected_speakers": detected_speakers,
            "transcription_ms": transcription_ms,
            "alignment_ms": alignment_ms,
            "diarization_ms": diarization_ms,
            "assignment_ms": assignment_ms,
            "total_ms": int((time.perf_counter() - total_t0) * 1000),
        },
    }
    return response


def run_pyannote_reassign(request: Dict[str, Any], whisperx: Any, diarization_pipeline_cls: Any):
    audio_path = require_string(request, "audio_path")
    device = require_string(request, "device")
    token = get_hf_token()
    if not token:
        fail(
            "Pyannote reassignment requires a Hugging Face token. "
            "Set SCREAMER_HF_TOKEN or HUGGINGFACE_HUB_TOKEN and accept the "
            "`pyannote/speaker-diarization-community-1` model terms."
        )

    audio = whisperx.load_audio(audio_path)
    diarization_t0 = time.perf_counter()
    diarization_pipeline = create_diarization_pipeline(
        diarization_pipeline_cls, token, device
    )
    intervals, detected_speakers = diarize_audio(diarization_pipeline, audio)
    diarization_ms = int((time.perf_counter() - diarization_t0) * 1000)

    assignment_t0 = time.perf_counter()
    rebuilt_segments: List[Dict[str, Any]] = []
    for segment in request.get("segments") or []:
        start_ms = int(segment.get("start_ms") or 0)
        end_ms = max(int(segment.get("end_ms") or start_ms + 1), start_ms + 1)
        speaker = dominant_speaker(start_ms / 1000.0, end_ms / 1000.0, intervals)
        text = normalize_text(str(segment.get("text") or ""))
        if not text:
            continue
        rebuilt_segments.append(
            {
                "start_ms": start_ms,
                "end_ms": end_ms,
                "speaker": speaker,
                "text": text,
                "words": [],
            }
        )
    assignment_ms = int((time.perf_counter() - assignment_t0) * 1000)

    return {
        "engine": "pyannote_reassign_v1",
        "transcript_text": " ".join(segment["text"] for segment in rebuilt_segments).strip(),
        "segments": rebuilt_segments,
        "diagnostics": {
            "detected_speakers": detected_speakers,
            "transcription_ms": 0,
            "alignment_ms": 0,
            "diarization_ms": diarization_ms,
            "assignment_ms": assignment_ms,
            "total_ms": diarization_ms + assignment_ms,
        },
    }


def main() -> None:
    request = load_request()
    mode = require_string(request, "mode")
    whisperx, diarization_pipeline_cls = load_dependencies()

    if mode == "whisperx_hybrid":
        response = run_whisperx_hybrid(request, whisperx, diarization_pipeline_cls)
    elif mode == "pyannote_reassign":
        response = run_pyannote_reassign(request, whisperx, diarization_pipeline_cls)
    else:
        fail(f"Unsupported WhisperX helper mode `{mode}`.")

    json.dump(response, sys.stdout)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        fail(f"WhisperX helper failed: {exc}")
