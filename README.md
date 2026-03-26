<div align="center">

<img src="resources/logo.png" width="180" alt="Screamer">

# Screamer

**A fast, fully local push-to-talk transcription app for macOS.**

Hold a key. Speak. Release. Text appears instantly.

[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Metal GPU](https://img.shields.io/badge/Metal-GPU_Accelerated-0071E3?style=for-the-badge&logo=apple&logoColor=white)](#speed)
[![License: MIT](https://img.shields.io/badge/License-MIT-22C55E?style=for-the-badge)](LICENSE)
[![100% Offline](https://img.shields.io/badge/100%25-Offline-8B5CF6?style=for-the-badge&logo=shieldsdotio&logoColor=white)](#)

---

<br>

> **~100ms** median Whisper inference on Apple M2 Max with `base.en`. No cloud. No subscription. No data leaves your machine.

<br>

</div>

## How it works

```
Hold Left Control  ──>  Speak  ──>  Release  ──>  Text pasted instantly
```

A frosted-glass waveform overlay appears while you speak. When you release, whisper.cpp transcribes on your GPU and the text is pasted into whatever app has focus.

<br>

## Speed

<div align="center">

Measured on **Apple M2 Max** with the `base.en` model using `GGML_NATIVE=OFF ITERATIONS=20 WARMUP=3 ./verify_latency.sh`:

| Sample | Sample duration | Median inference | Mean inference |
|---|---|---|---|
| Short phrase | `1.9s` | `~98ms` | `~98ms` |
| Sentence | `3.2s` | `~116ms` | `~117ms` |
| Long paragraph | `5.9s` | `~143ms` | `~142ms` |

</div>

> [!NOTE]
> These are local Whisper inference timings from the benchmark harness, not full key-release-to-paste end-to-end timings.

<br>

### vs. published claims

<div align="center">

| App | Public latency figure | Source |
|---|---|---|
| **Screamer** | **~98-143ms median inference (`base.en`, M2 Max)** | Local benchmark: [`./verify_latency.sh`](./verify_latency.sh) |
| Dictato | `80ms` real-time transcription latency claim | [Dictato](https://dicta.to/) |
| SuperWhisper | No public `ms` figure found | [Superwhisper](https://superwhisper.com/) |
| Wispr Flow | No public `ms` figure found | [Wispr Flow](https://wisprflow.ai/), [Microsoft Store](https://apps.microsoft.com/detail/9n1b9jwb3m35) |
| Otter.ai | No public `ms` figure found | [Otter](https://otter.ai/) |

</div>

> As of **March 26, 2026**, Dictato was the only other app above with a public numeric latency claim we could cite. Superwhisper, Wispr Flow, and Otter market speed or real-time transcription, but no public `ms` figures were found to link here, so the old unsourced numbers were removed.

<br>

## Accuracy

Screamer uses **whisper.cpp** — the same engine that powers SuperWhisper, MacWhisper, and most other local transcription apps. Same engine, same model weights = **identical accuracy**.

<div align="center">

Word Error Rate (WER) on [LibriSpeech test-clean](https://huggingface.co/datasets/librispeech_asr) benchmark:

| Model | WER | Tradeoff |
|---|---|---|
| `tiny.en` | ~7.7% | Fastest, lowest accuracy |
| `base.en` | **~5.0%** | **Best default for most people** |
| `small.en` | ~3.4% | Better for harder vocabulary |
| `medium.en` | ~2.9% | High accuracy, slower |
| `large-v3` | ~2.5% | Highest accuracy, slowest |

</div>

> [!TIP]
> **Same model = same accuracy.** The main difference between apps is packaging and performance, not the underlying Whisper transcription quality.

Pick your tradeoff:
- **`base.en`** (default) — 5% WER, with `~100-145ms` measured inference on this Apple M2 Max benchmark. Best balance for everyday use.
- **`small.en`** — 3.4% WER. Slower, but noticeably more accurate for complex vocabulary.
- **`large-v3`** — 2.5% WER. Slowest, but best when precision matters.

All models are free to download. Just run `./download_model.sh` and pick one.

<br>

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌───────────────┐     ┌──────────┐
│  Left Ctrl  │────>│  CoreAudio   │────>│  whisper.cpp   │────>│  Cmd+V   │
│  (hotkey)   │     │  (capture)   │     │  (Metal GPU)   │     │  (paste) │
└─────────────┘     └──────────────┘     └───────────────┘     └──────────┘
                           │
                    ┌──────────────┐
                    │   Waveform   │
                    │  (overlay)   │
                    └──────────────┘
```

- **whisper.cpp** via whisper-rs — model stays loaded in memory, zero cold-start
- **Metal GPU** with flash attention for fast inference on Apple Silicon
- **CoreAudio** capture at native sample rate, resampled to 16kHz
- **NSEvent** global monitor for modifier key detection
- **Spring-physics** waveform with idle breathing animation
- **Single binary** — no Electron, no Python, no runtime dependencies

<br>

## Install

### Prerequisites

- macOS 12+ on Apple Silicon (M1/M2/M3/M4)
- [Rust toolchain](https://rustup.rs/)
- cmake — `brew install cmake`

### Build from source

```bash
git clone https://github.com/user/screamer.git
cd screamer

# Download the whisper model (~142MB)
./download_model.sh

# Build with Metal GPU support and bundle into .app
GGML_NATIVE=OFF cargo build --release
./bundle.sh

# Launch
open Screamer.app
```

### Permissions

After first launch, grant **Accessibility** permission:

**System Settings → Privacy & Security → Accessibility → Screamer**

This is required for the global hotkey and paste simulation.

> [!NOTE]
> You'll need to re-toggle Accessibility permission after each rebuild — the ad-hoc code signature changes, so macOS treats it as a new app.

<br>

## Configuration

Config lives at `~/Library/Application Support/Screamer/config.json`:

```json
{
  "model": "base"
}
```

| Model | Size | Speed | Accuracy |
|---|---|---|---|
| `tiny` | 75 MB | Fastest | Good for simple phrases |
| `base` | 142 MB | **Fast (default)** | **Great for most use cases** |
| `small` | 466 MB | Moderate | Better for complex speech |
| `medium` | 1.5 GB | Slower | High accuracy |
| `large` | 3.1 GB | Slowest | Highest accuracy |

Download additional models with `./download_model.sh`.

<br>

## Why Screamer?

| | Screamer | SuperWhisper | Wispr Flow | Otter.ai |
|---|---|---|---|---|
| Accuracy (base) | **~5.0% WER** | ~5.0% WER | Proprietary | Proprietary |
| Latency evidence | **~98-143ms measured locally** | No public `ms` figure found | No public `ms` figure found | No public `ms` figure found |
| Price | **Free** | Paid | Paid | Paid |
| All model sizes | **Yes (tiny → large)** | Yes | N/A | N/A |
| Offline | **Yes** | Yes | No | No |
| Open source | **Yes** | No | No | No |
| GPU accelerated | **Yes (Metal)** | No | N/A (cloud) | N/A (cloud) |
| Data privacy | **100% local** | Local | Cloud | Cloud |

<br>

<div align="center">

## License

MIT — do whatever you want with it.

<br>

---

*Built with Rust, whisper.cpp, and Apple Metal.*

</div>
