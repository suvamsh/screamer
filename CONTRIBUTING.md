# Contributing to Screamer

Thanks for helping improve Screamer.

## Development setup

Requirements:

- macOS 13+
- Rust 1.94+
- `cmake` via `brew install cmake`

First-time setup:

```bash
git clone https://github.com/suvamsh/screamer.git
cd screamer
./download_model.sh base
```

Run the usual checks before opening a PR:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
```

When a change could affect the existing macOS app shell, also run the bundle smoke test:

```bash
GGML_NATIVE=OFF cargo build --release
BIN_PATH=target/release/screamer ./scripts/macos_bundle_smoke.sh
```

To build the app bundle locally:

```bash
GGML_NATIVE=OFF cargo build --release
./bundle.sh
open Screamer.app
```

## Project guidelines

- Keep the app offline-first. New features should not require network access for transcription.
- Treat transcript content as sensitive user data. Do not add default logging that prints spoken text.
- Prefer small, reviewable patches over broad refactors unless the refactor clearly reduces risk or complexity.
- Do not commit downloaded model binaries from `models/`.
- Keep packaging and release changes documented in `docs/releases.md`.

## Pull requests

- Include a short summary of the user-visible change.
- Mention any macOS permissions affected by the change.
- Call out whether the macOS bundle smoke test still passed if you touched build, packaging, resources, permissions, hotkeys, paste behavior, or UI shell code.
- Call out any manual verification you performed, especially for audio capture, hotkeys, paste behavior, or signing/notarization flows.

## Security issues

Please do not open public issues for suspected security vulnerabilities. Follow the process in `SECURITY.md`.
