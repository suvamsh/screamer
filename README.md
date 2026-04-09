<div align="center">

<img src="resources/image.png" width="220" alt="Screamer">

# Screamer

**Offline dictation and ambient notetaking for macOS.**

[www.screamer.app](https://www.screamer.app)

Push-to-talk transcription. Hold a key, speak, release, and your text is pasted instantly.

[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Metal GPU](https://img.shields.io/badge/Metal-GPU_Accelerated-0071E3?style=for-the-badge&logo=apple&logoColor=white)](#performance)
[![License: MIT](https://img.shields.io/badge/License-MIT-22C55E?style=for-the-badge)](LICENSE)
[![100% Offline](https://img.shields.io/badge/100%25-Offline-8B5CF6?style=for-the-badge&logo=shieldsdotio&logoColor=white)](#)
[![Download for Mac](https://img.shields.io/badge/Download_for_Mac-Screamer.dmg-111111?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/suvamsh/screamer/releases/download/stable/Screamer.dmg)

</div>

## What is it?

Screamer is a free, open source, offline speech app for macOS with two local-first modes:

- Push-to-talk dictation: hold a key, speak, release, and your text is pasted into the app you are using
- Ambient notetaker: run a live session, keep editable live notes, then generate structured notes locally when the session ends
- Runs locally with Whisper and a bundled summary backend, so there is no required cloud round-trip

Project docs:

- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Release guide](docs/releases.md)
- [SDK refactor plan](docs/sdk-refactor-plan.md)

## How it works

```text
Hold Left Control -> Speak -> See waveform + live text -> Release -> Text pastes instantly
```

- Local Whisper transcription with no cloud round-trip
- Live overlay with waveform and rolling preview while the hotkey is held
- Final transcription and paste on release, so unstable partials are never typed into the target app
- Free, offline, and open source

## Performance

### Screamer latency

| Metric | Result |
|---|---|
| Average release-to-paste-dispatch latency | **`~55ms`** |
| Benchmark path | Stop, resample, transcription, clipboard write, and `Cmd+V` dispatch |
| Verification harness | `./verify_latency.sh` running `app_path_latency --dispatch-paste` on the current synthetic phrase set |
| Test setup | Apple M2 Max with `base.en` |

### Under the hood

- Two pre-warmed Whisper states stay ready in memory: one for final transcription and one for live preview, so release-time transcription does not have to build a fresh state.
- Adaptive audio context keeps short dictation fast by shrinking Whisper's work to match the utterance instead of always using the model's full default window.
- The decode path is tuned for push-to-talk, not long recordings: no timestamps, single-segment output, no rolling context, and a fast greedy decode.
- Live preview is best-effort and never types into your app. Only the final transcript is pasted, which lets Screamer drop stale preview work instead of slowing down the critical path.
- Silence is trimmed before inference, and the recorder keeps latency low with preallocated buffers, small input buffers, and a lightweight paste dispatch.

### Model guidance

Screamer does not yet ship a Screamer-specific WER harness in this repo. Accuracy mainly follows the underlying Whisper model, so treat these as relative model tradeoffs rather than audited Screamer eval numbers.

| Model | Tradeoff | Best for |
|---|---|---|
| `tiny.en` | Fastest, lowest accuracy | Maximum speed |
| `base.en` | Balanced default | **Best default for most people** |
| `small.en` | Slower, more accurate | Better accuracy for harder vocabulary |
| `medium.en` | High accuracy, higher latency | Higher accuracy |
| `large-v3` | Highest accuracy, highest latency | Highest accuracy |

All models are free to download with `./download_model.sh`. The bundled ambient-summary path uses a local Gemma 3 1B GGUF (`models/summary/gemma-3-1b-it-q4_k_m.gguf`) and runs it through a bundled `llama.cpp` helper with Metal offload on Apple Silicon when that artifact is present.

### Speed vs. the competition

| App | Latency | Source |
|---|---|---|
| **Screamer** | **`~55ms`** | Local average from `./verify_latency.sh` (`app_path_latency --dispatch-paste`) on Apple M2 Max with `base.en` |
| Dictato | `80ms` | [Dictato](https://dicta.to/) |
| Handy | `~350ms` estimated | [Handy README](https://github.com/cjpais/Handy#system-requirementsrecommendations), [Handy model config](https://github.com/cjpais/Handy/blob/main/src-tauri/src/managers/model.rs), [Handy settings strings](https://github.com/cjpais/Handy/blob/main/src/i18n/locales/en/translation.json) |
| SuperWhisper | `~700ms` estimated | [Superwhisper](https://superwhisper.com/), [App Store](https://apps.apple.com/us/app/superwhisper/id6471464415?uo=4), [MacSources review](https://macsources.com/superwhisper-app-review/), [Declom review](https://declom.com/superwhisper/) |
| Wispr Flow | `~600ms` estimated | [Wispr Flow](https://wisprflow.ai/), [App Store](https://apps.apple.com/us/app/wispr-flow-ai-voice-keyboard/id6497229487?uo=4), [Microsoft Store](https://apps.microsoft.com/detail/9n1b9jwb3m35), [AI Productivity Coach review](https://aiproductivitycoach.com/wispr-flow-review/), [Letterly review](https://letterly.app/blog/wispr-flow-review/) |
| Otter.ai | `~1500ms` estimated | [Otter](https://otter.ai/), [App Store](https://apps.apple.com/us/app/otter-transcribe-voice-notes/id1276437113?uo=4) |

> Screamer's number is the approximate average from the current synthetic phrase set on Apple M2 Max. Competitor numbers are public claims or rough public estimates as of March 29, 2026.

## Install

Requirements:

- macOS 13+ on Apple Silicon and Intel Macs
- [Rust toolchain](https://rustup.rs/) 1.94+
- `cmake` via `brew install cmake`

Build from source:

```bash
git clone https://github.com/suvamsh/screamer.git
cd screamer
./download_model.sh bundled
GGML_NATIVE=OFF cargo build --release
./bundle.sh
open Screamer.app
```

After first launch, grant **Accessibility** permission in:

`System Settings -> Privacy & Security -> Accessibility -> Screamer`

This is required for the global hotkey and paste simulation. If it isn't enabled yet, Screamer will keep an in-app helper window visible and can open the exact Accessibility pane for you.

macOS will also prompt for **Microphone** permission the first time you record.

`bundle.sh` will automatically try the first installed `Developer ID Application` certificate if one is available, which helps macOS keep Accessibility approval across rebuilds. If no usable certificate is installed, it falls back to ad-hoc signing and macOS may ask you to re-enable Accessibility after rebuilds.

## Configuration

Config lives at `~/Library/Application Support/Screamer/config.json`:

```json
{
  "model": "base",
  "hotkey": "left_control",
  "overlay_position": "center",
  "appearance": "dark",
  "live_transcription": true,
  "sound_effects": true,
  "ambient_microphone": true,
  "ambient_system_audio": true,
  "ambient_final_backend": "native",
  "summary_backend": "bundled",
  "summary_ollama_model": "gemma4:latest",
  "show_accessibility_helper_on_launch": true,
  "accessibility_helper_dismissed": false
}
```

Key settings:

- `model`: Whisper model to use
- `hotkey`: push-to-talk key
- `overlay_position`: overlay placement
- `appearance`: app theme
- `live_transcription`: live preview in the overlay
- `sound_effects`: start and finish cue sounds
- `ambient_microphone`: enable the microphone lane for ambient notetaker sessions
- `ambient_system_audio`: request the system-output lane for ambient notetaker sessions
- `ambient_final_backend`: final ambient transcript backend, `native` or `native_diarization`
- `summary_backend`: `bundled` or `ollama`
- `summary_ollama_model`: local Ollama model to use when `summary_backend` is `ollama`
- `show_accessibility_helper_on_launch`: whether the helper window should appear on first launch
- `accessibility_helper_dismissed`: remembers whether the helper window was dismissed

## Native Ambient Final Pass

Screamer now supports an optional ambient-only native final pass called `native_diarization`.

- Live ambient notes stay on the native Screamer pipeline while the session is running
- When you stop the session, Screamer can regenerate the final transcript with a native diarization/resegmentation pass
- Push-to-talk dictation is unchanged

This backend reuses Screamer's native Whisper decode for the transcript backbone and applies a native speaker-attribution pass over the final session audio. Prepared ambient diarization assets are local-only for now and are not bundled with the app.

### Setup

1. Export your segmentation and embedding models to ONNX outside the app.

2. Install the exported ONNX files into Screamer's local asset cache:

```bash
python3 scripts/prepare_ambient_diarization_assets.py \
  --asset-version pyannote-community-1-v1 \
  --segmentation-onnx /absolute/path/to/segmentation.onnx \
  --embedding-onnx /absolute/path/to/embedding.onnx
```

3. In Screamer Settings, set `Ambient final pass` to `Native Diarization`.

Notes:

- Runtime inference stays local
- The ONNX Runtime CoreML backend currently builds behind `--features ambient-ort-coreml`
- On Apple Silicon, the downloaded ORT CoreML binary expects an Xcode/macOS SDK new enough to expose `MLComputePlan` and `MLOptimizationHints` (Xcode 15+ is the safe baseline)
- If the native final pass fails, Screamer falls back to the native ambient transcript for that session
- Exported assets are local development artifacts for this iteration and are not bundled into `Screamer.app`

### Environment Variables

- `SCREAMER_AMBIENT_DIARIZATION_DIR`: override the ambient diarization asset directory
- `SCREAMER_HF_TOKEN`: reserved for developer-side asset preparation workflows

## Ambient Eval Harness

Use the manifest-driven harness to compare the current native ambient path, the native final pass, and the optional legacy Python benchmark.

Example manifest:

```json
{
  "model": "base",
  "cases": [
    {
      "id": "two-speaker-meeting",
      "required": true,
      "audio_path": "/absolute/path/to/two-speaker-meeting.wav",
      "reference_transcript_path": "/absolute/path/to/two-speaker-meeting.txt",
      "reference_turns_path": "/absolute/path/to/two-speaker-meeting.turns.json"
    }
  ]
}
```

Run it with:

```bash
cargo run --bin ambient_eval -- --manifest /absolute/path/to/ambient-eval.json
```

Enable the legacy Python benchmark only when you explicitly want it:

```bash
cargo run --bin ambient_eval -- \
  --manifest /absolute/path/to/ambient-eval.json \
  --enable-legacy-python \
  --baseline-out "$PWD/.screamer-benchmarks/ambient-eval-baseline.json"
```

The report is emitted as JSON to stdout and includes per-case backend status, speaker/turn metrics, word error rate, real-time factor, peak RSS, and runtime metadata.

## Privacy and logging

- Transcription runs locally. Screamer does not send audio or text to a cloud service.
- Runtime logs are written to `~/Library/Logs/Screamer/screamer.log` by default.
- Ambient sessions now append a shareable report block to that same log file with the final transcript and summary, so beta testers can send one file after using the app.
- Dictation and vision transcript contents are still not logged by default.
- Set `SCREAMER_LOG_TRANSCRIPTS=1` only when you explicitly want transcript text in logs for debugging.
- Set `SCREAMER_LOG_FILE=/custom/path.log` to override the log file location.

## Stack

- Rust app with a single native binary
- `whisper.cpp` via `whisper-rs`
- CoreAudio capture
- Metal GPU acceleration on Apple Silicon

## License

MIT
