# Releasing Screamer

This repo now supports two DMG release paths:

- Manual local DMG builds for testing or one-off releases.
- Automated GitHub releases that refresh the DMG on every push to `main`, on version tags, or by manual dispatch.

## 1. Manual local DMG release

Use this when you want to verify the packaging flow on your own Mac.

```bash
./download_model.sh base
GGML_NATIVE=OFF ./bundle.sh
./create_dmg.sh
```

That produces a versioned DMG in `dist/`, for example `dist/Screamer-1.0.0.dmg`.

For public distribution, sign and notarize it instead of using the default ad-hoc signature:

```bash
export CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="your-apple-id@example.com"
export APPLE_APP_SPECIFIC_PASSWORD="xxxx-xxxx-xxxx-xxxx"
export APPLE_TEAM_ID="TEAMID"

GGML_NATIVE=OFF ./bundle.sh
./create_dmg.sh
./notarize_dmg.sh dist/Screamer-1.0.0.dmg
```

`bundle.sh` uses the Cargo version as the source of truth and writes that into the built app bundle's `Info.plist`.

## 2. One-time GitHub setup

The workflow is in [`.github/workflows/release-macos.yml`](../.github/workflows/release-macos.yml).

Add these GitHub Actions secrets in the repo settings:

- `APPLE_CERTIFICATE_P12_BASE64`: Base64-encoded Developer ID Application certificate exported as `.p12`
- `APPLE_CERTIFICATE_PASSWORD`: Password used when exporting that `.p12`
- `APPLE_KEYCHAIN_PASSWORD`: Any strong temporary password for the runner keychain
- `APPLE_SIGNING_IDENTITY`: Exact signing identity name, for example `Developer ID Application: Your Name (TEAMID)`
- `APPLE_ID`: Apple ID email used for notarization
- `APPLE_APP_SPECIFIC_PASSWORD`: App-specific password from appleid.apple.com
- `APPLE_TEAM_ID`: Your Apple Developer team ID

### Export the signing certificate

On your Mac:

1. Open Keychain Access.
2. Find your `Developer ID Application` certificate.
3. Export it as a `.p12` file with a password.
4. Convert it to base64:

```bash
base64 -i developer-id-app.p12 | pbcopy
```

Paste that into the `APPLE_CERTIFICATE_P12_BASE64` GitHub secret.

## 3. What the workflow does

### On every push to `main` or manual workflow dispatch

- Builds a universal macOS binary (`arm64` + `x86_64`)
- Bundles the app and the default `base` Whisper model
- Signs and notarizes if the Apple secrets are present
- Updates a prerelease tagged `continuous`
- Uploads the DMG asset as `Screamer-latest.dmg`

Landing-page link for always-fresh builds:

[`https://github.com/suvamsh/screamer/releases/download/continuous/Screamer-latest.dmg`](https://github.com/suvamsh/screamer/releases/download/continuous/Screamer-latest.dmg)

### On every tag like `v1.0.2`

- Runs the same build
- Creates or updates a normal GitHub release for that tag
- Uploads:
  - `Screamer-1.0.2.dmg`
  - `Screamer.dmg`

Landing-page link for the latest official tagged release:

[`https://github.com/suvamsh/screamer/releases/latest/download/Screamer.dmg`](https://github.com/suvamsh/screamer/releases/latest/download/Screamer.dmg)

## 4. Cutting an official release

1. Bump the version in `Cargo.toml`.
2. Commit and push to `main`.
3. Create and push a tag:

```bash
git tag v1.0.2
git push origin v1.0.2
```

That tag will produce a notarized GitHub release DMG if the signing secrets are configured.

## 5. Creating an untagged DMG from GitHub

Every push to `main` refreshes the `continuous` prerelease automatically. If you want a fresh prerelease DMG without pushing a new commit, run the `Release macOS DMG` workflow manually from the Actions tab. That path also updates `continuous` and publishes `Screamer-latest.dmg`.
