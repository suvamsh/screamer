<div align="center">

<img src="resources/image.png" width="220" alt="Screamer">

# Screamer

**The fastest free speech to text AI in the world.**

Push-to-talk transcription. Hold a key, speak, release, and your text is pasted instantly.

[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Metal GPU](https://img.shields.io/badge/Metal-GPU_Accelerated-0071E3?style=for-the-badge&logo=apple&logoColor=white)](#performance)
[![License: MIT](https://img.shields.io/badge/License-MIT-22C55E?style=for-the-badge)](LICENSE)
[![100% Offline](https://img.shields.io/badge/100%25-Offline-8B5CF6?style=for-the-badge&logo=shieldsdotio&logoColor=white)](#)

</div>

## What is it?

Screamer is a free, open source, offline push-to-talk speech-to-text app for macOS.

- Hold a key, speak, release, and your text is pasted into the app you are using
- Runs locally with Whisper, so there is no cloud round-trip
- Shows a live overlay with waveform and rolling preview while you talk
- Built for low-latency dictation instead of full meeting transcription

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
| Median end-to-end app-path latency | **`~52ms`** |
| Verified phrase-set results | `32ms`, `52ms`, `68ms` |
| Benchmark path | Stop, resample, transcription, clipboard write, and `Cmd+V` dispatch |
| Test setup | Apple M2 Max with `base.en` via local `app_path_latency --dispatch-paste` |

### Screamer evals

Screamer uses `whisper.cpp` via `whisper-rs`, so accuracy mainly depends on the model you choose.

| Model | WER | Best for |
|---|---|---|
| `tiny.en` | ~7.7% | Maximum speed |
| `base.en` | **~5.0%** | **Best default for most people** |
| `small.en` | ~3.4% | Better accuracy for harder vocabulary |
| `medium.en` | ~2.9% | High accuracy |
| `large-v3` | ~2.5% | Highest accuracy |

All models are free to download with `./download_model.sh`.

### Speed vs. the competition

| App | Latency | Source |
|---|---|---|
| **Screamer** | **`~52ms`** | Local `app_path_latency --dispatch-paste` benchmark on Apple M2 Max with `base.en` |
| Dictato | `80ms` | [Dictato](https://dicta.to/) |
| SuperWhisper | `~700ms` estimated | [Superwhisper](https://superwhisper.com/), [App Store](https://apps.apple.com/us/app/superwhisper/id6471464415?uo=4), [MacSources review](https://macsources.com/superwhisper-app-review/), [Declom review](https://declom.com/superwhisper/) |
| Wispr Flow | `~600ms` estimated | [Wispr Flow](https://wisprflow.ai/), [App Store](https://apps.apple.com/us/app/wispr-flow-ai-voice-keyboard/id6497229487?uo=4), [Microsoft Store](https://apps.microsoft.com/detail/9n1b9jwb3m35), [AI Productivity Coach review](https://aiproductivitycoach.com/wispr-flow-review/), [Letterly review](https://letterly.app/blog/wispr-flow-review/) |
| Otter.ai | `~1500ms` estimated | [Otter](https://otter.ai/), [App Store](https://apps.apple.com/us/app/otter-transcribe-voice-notes/id1276437113?uo=4) |

> Screamer's number is the median of the verified end-to-end app-path benchmark across the current phrase set (`32ms`, `52ms`, `68ms`). Competitor numbers are public claims or rough public estimates as of March 27, 2026.

## Install

Requirements:

- macOS 12+ on Apple Silicon and Intel Macs
- [Rust toolchain](https://rustup.rs/)
- `cmake` via `brew install cmake`

Build from source:

```bash
git clone https://github.com/user/screamer.git
cd screamer
./download_model.sh
GGML_NATIVE=OFF cargo build --release
./bundle.sh
open Screamer.app
```

After first launch, grant **Accessibility** permission in:

`System Settings -> Privacy & Security -> Accessibility -> Screamer`

This is required for the global hotkey and paste simulation.

If you rebuild often, macOS may ask you to re-enable Accessibility because the app signature changes.

## Configuration

Config lives at `~/Library/Application Support/Screamer/config.json`:

```json
{
  "model": "base",
  "hotkey": "left_control",
  "overlay_position": "center",
  "live_transcription": true,
  "sound_effects": true
}
```

Key settings:

- `model`: Whisper model to use
- `hotkey`: push-to-talk key
- `overlay_position`: overlay placement
- `live_transcription`: live preview in the overlay
- `sound_effects`: start and finish cue sounds

## Stack

- Rust app with a single native binary
- `whisper.cpp` via `whisper-rs`
- CoreAudio capture
- Metal GPU acceleration on Apple Silicon

## License

MIT
