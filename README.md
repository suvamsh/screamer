<div align="center">

<img src="resources/logo.png" width="180" alt="Screamer">

# Screamer

**The fastest push-to-talk transcription app for macOS.**

Hold a key. Speak. Release. Text appears instantly.

[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Metal GPU](https://img.shields.io/badge/Metal-GPU_Accelerated-0071E3?style=for-the-badge&logo=apple&logoColor=white)](#speed)
[![License: MIT](https://img.shields.io/badge/License-MIT-22C55E?style=for-the-badge)](LICENSE)
[![100% Offline](https://img.shields.io/badge/100%25-Offline-8B5CF6?style=for-the-badge&logo=shieldsdotio&logoColor=white)](#)

---

<br>

> **134ms** from key release to text pasted. No cloud. No subscription. No data leaves your machine.

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

Measured on **Apple M2 Max** with the `base.en` model:

| | Recording | Inference | Total |
|---|---|---|---|
| Short phrase | ~2s | `123ms` | **134ms** |
| Sentence | ~3s | `149ms` | **162ms** |
| Long paragraph | ~6s | `226ms` | **238ms** |

</div>

<br>

### vs. the competition

<div align="center">

| App | Type | Latency | Price | |
|---|---|---|---|---|
| **Screamer** | **Local, Metal GPU** | **123–238ms** | **Free & open source** | |
| Dictato (Parakeet) | Local, streaming | ~80ms | $15 | *Non-Whisper model* |
| Dictato (Whisper) | Local, streaming | ~120ms | $15 | |
| Apple Dictation | Local, Neural Engine | ~200ms | Free | *Limited customization* |
| Voibe | Local, quantized | <300ms | $8 | |
| SuperWhisper (tiny) | Local, CPU | ~500ms | $10/mo | *Least accurate* |
| SuperWhisper (base) | Local, CPU | ~500–800ms | $10/mo | *Same model as Screamer* |
| SuperWhisper (large-v3) | Local, CPU | 1–2s | $10/mo | |
| Wispr Flow | Cloud | 500–700ms | $10/mo | *Requires internet* |
| Otter.ai | Cloud | 400–600ms | $17/mo | *Requires internet* |

</div>

> **Screamer is 3–4x faster than SuperWhisper** using the same model size, and faster than every cloud service — while being fully offline and free.

<br>

## Accuracy

Screamer uses **whisper.cpp** — the same engine that powers SuperWhisper, MacWhisper, and most other local transcription apps. Same engine, same model weights = **identical accuracy**.

<div align="center">

Word Error Rate (WER) on [LibriSpeech test-clean](https://huggingface.co/datasets/librispeech_asr) benchmark:

| Model | WER | Screamer Latency | SuperWhisper Latency | Price |
|---|---|---|---|---|
| `tiny.en` | ~7.7% | ~80ms | ~500ms | **Free** vs $10/mo |
| `base.en` | **~5.0%** | **~134ms** | **~500–800ms** | **Free** vs $10/mo |
| `small.en` | ~3.4% | ~400ms | ~1–2s | **Free** vs $10/mo |
| `medium.en` | ~2.9% | ~800ms | ~3–5s | **Free** vs $10/mo |
| `large-v3` | ~2.5% | ~1.5s | ~5–8s | **Free** vs $10/mo |

</div>

> [!TIP]
> **Same model = same accuracy.** The difference is Screamer runs on your **Metal GPU** for free, while SuperWhisper runs on CPU for $10/mo. You get identical transcription quality at 3–4x the speed, at zero cost.

Pick your tradeoff:
- **`base.en`** (default) — 5% WER, 134ms latency. Best balance for everyday use.
- **`small.en`** — 3.4% WER, ~400ms. Noticeably more accurate for complex vocabulary.
- **`large-v3`** — 2.5% WER, ~1.5s. Maximum accuracy when precision matters.

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
| Latency | **~134ms** | ~500–800ms | ~500–700ms | ~400–600ms |
| Price | **Free** | $10/mo | $10/mo | $17/mo |
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
