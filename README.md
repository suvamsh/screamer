<div align="center">

<img src="resources/image.png" width="220" alt="Screamer">

# Screamer

**The fastest free push-to-talk transcription app.**

Hold a key. Speak. Watch live text appear. Release. Text appears instantly.

[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Metal GPU](https://img.shields.io/badge/Metal-GPU_Accelerated-0071E3?style=for-the-badge&logo=apple&logoColor=white)](#speed)
[![License: MIT](https://img.shields.io/badge/License-MIT-22C55E?style=for-the-badge)](LICENSE)
[![100% Offline](https://img.shields.io/badge/100%25-Offline-8B5CF6?style=for-the-badge&logo=shieldsdotio&logoColor=white)](#)

---

<br>

> **~32-68ms** measured app-path latency on an **Apple M2 Max** with `base.en`, including stop, resample, transcription, clipboard write, and `Cmd+V` dispatch. Hot local Whisper inference measures **~25-61ms** on the same machine. No cloud. No subscription. No data leaves your machine.

<br>

</div>

## How it works

```
Hold Left Control  ──>  Speak  ──>  See waveform + live text  ──>  Release  ──>  Text pasted instantly
```

A frosted-glass overlay tracks your mic input live while you speak, keeps the waveform visible, and shows rolling partial transcription from the same loaded Whisper model. When you release, Screamer runs the final full transcription and pastes it into whatever app has focus.

<br>

## Live Overlay Transcription

Live transcription is now part of the overlay, not the paste path:

- **Waveform + text together** while the hotkey is held
- **Same loaded Whisper model** for both live preview and final transcription
- **Final paste still happens on release** so we never type unstable partials into the target app
- **Enabled by default** with a `Live Transcription` toggle in the menu bar
- **Long dictation stays bounded** by previewing a rolling recent audio window and clipping the overlay to the most recent text

> [!NOTE]
> The benchmark tables below measure Screamer's release path, not the background live-preview loop. Live preview does add extra work while you hold the key, but the final transcription and paste flow is still the primary latency benchmark.

<br>

## Speed

Measured on an **Apple M2 Max** with the `base.en` model using Screamer's auto-selected runtime profile (`backend=gpu`, `flash_attn=yes`, `threads=4`, adaptive `audio_ctx`):

### Verified local inference latency

Using `GGML_NATIVE=OFF ITERATIONS=20 WARMUP=3 ./verify_latency.sh`:

| Sample | Sample duration | Median inference | Mean inference |
|---|---|---|---|
| Short phrase | `1.9s` | `~25ms` | `~25ms` |
| Sentence | `3.2s` | `~38ms` | `~38ms` |
| Long paragraph | `5.9s` | `~61ms` | `~61ms` |

### Verified app-path latency

Using `target/release/app_path_latency --device-rate 48000 --dispatch-paste` against the same phrase set, with real clipboard write and `Cmd+V` dispatch into a focused macOS app:

| Sample | Sample duration | Median app-path latency | Mean app-path latency |
|---|---|---|---|
| Short phrase | `1.9s` | `~32ms` | `~32ms` |
| Sentence | `3.2s` | `~52ms` | `~53ms` |
| Long paragraph | `5.9s` | `~68ms` | `~67ms` |

> [!NOTE]
> Hardware for these runs: **Apple M2 Max (arm64, 8 performance cores + 4 efficiency cores)**.
> The inference table measures hot-model, hot-state local Whisper decode time. The app-path table measures Screamer's synchronous release pipeline: stop, resample, transcribe, clipboard write, and `Cmd+V` dispatch.
> Live overlay transcription is a separate background decode while the key is held, and it uses a rolling recent-audio window to keep long holds responsive.

### Verification

We cross-checked the latency numbers four ways:

| Eval | What it validates | Result |
|---|---|---|
| `./verify_latency.sh` | Official hot-path benchmark harness | `25.2 / 38.2 / 60.8 ms` p50 |
| `latency_outer_wall` | Outer wall-clock vs internal timer drift | `26.2 / 39.1 / 61.6 ms` p50, `~0 ms` drift |
| `latency_direct_whisper` | Direct `whisper-rs` timing without Screamer wrapper | `25.1 / 38.9 / 60.8 ms` p50 |
| `app_path_latency --dispatch-paste` | Real release-path timing with paste dispatch | `32.0 / 51.6 / 68.2 ms` p50 |

Fresh-state sanity check: disabling state reuse moved the same inference benchmark to roughly `70 / 79 / 110 ms` p50, which confirms that the hot-path speedup comes from real engineering work rather than a misleading timer.

<br>

### vs. the competition

<div align="center">

| App | Latency | Source |
|---|---|---|
| **Screamer** | **`~25-61ms` inference p50, `~32-68ms` app-path p50 on Apple M2 Max** | Local benchmarks: [`./verify_latency.sh`](./verify_latency.sh), `latency_outer_wall`, `latency_direct_whisper`, `app_path_latency` |
| Dictato | `80ms` public latency claim | [Dictato](https://dicta.to/) |
| SuperWhisper | `~700ms` estimated | [Superwhisper](https://superwhisper.com/), [App Store](https://apps.apple.com/us/app/superwhisper/id6471464415?uo=4), [MacSources review](https://macsources.com/superwhisper-app-review/), [Declom review](https://declom.com/superwhisper/) |
| Wispr Flow | `~600ms` estimated | [Wispr Flow](https://wisprflow.ai/), [App Store](https://apps.apple.com/us/app/wispr-flow-ai-voice-keyboard/id6497229487?uo=4), [Microsoft Store](https://apps.microsoft.com/detail/9n1b9jwb3m35), [AI Productivity Coach review](https://aiproductivitycoach.com/wispr-flow-review/), [Letterly review](https://letterly.app/blog/wispr-flow-review/) |
| Otter.ai | `~1500ms` estimated | [Otter](https://otter.ai/), [App Store](https://apps.apple.com/us/app/otter-transcribe-voice-notes/id1276437113?uo=4) |

</div>

> As of **March 27, 2026**, Dictato was the only other app above with a public numeric latency claim we could cite directly. Their site advertises latency "as low as 80 milliseconds" and a prominent `80ms` latency stat. Our verified Apple M2 Max benchmarks land below `80ms` both for hot local inference and for Screamer's synchronous app-path benchmark on the phrase set above. The Superwhisper, Wispr Flow, and Otter numbers are still **rough estimates** inferred from public demos, App Store copy, and third-party reviews because no vendor-published `ms` benchmark was available.

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
- **`base.en`** (default) — 5% WER, with `~25-61ms` measured inference and `~32-68ms` measured app-path latency on the current Apple M2 Max benchmark. Best balance for everyday use.
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
                           │                     │
                    ┌──────────────┐      ┌──────────────┐
                    │   Waveform   │      │ Live Preview │
                    │  (overlay)   │      │  (overlay)   │
                    └──────────────┘      └──────────────┘
```

- **whisper.cpp** via whisper-rs — model stays loaded in memory, zero cold-start
- **Machine-aware GPU/CPU tuning** with Metal where available and flash attention on Apple Silicon
- **CoreAudio** capture at native sample rate, resampled to 16kHz
- **NSEvent** global monitor for modifier key detection
- **Live overlay transcription** that previews partial text while you hold the key
- **Audio-reactive** waveform that mirrors live mic input and goes flat on silence
- **Rolling live preview window** so long dictation remains stable and responsive
- **Single binary** — no Electron, no Python, no runtime dependencies

<br>

## Install

### Prerequisites

- Current release: macOS 12+ on Apple Silicon and Intel Macs
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
  "model": "base",
  "hotkey": "left_control",
  "overlay_position": "center",
  "live_transcription": true,
  "sound_effects": true
}
```

`live_transcription` controls whether the overlay shows live partial text while you hold the hotkey. It defaults to `true` and can also be toggled from the menu bar without editing the file by hand.
`sound_effects` controls the start/finish cue sounds. It defaults to `true` and can also be toggled from the menu bar.

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
| Latency | **`~25-61ms` inference / `~32-68ms` app-path on Apple M2 Max** | ~700ms est. | ~600ms est. | ~1500ms est. |
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
