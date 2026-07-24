# Release Packaging

Release packaging is rooted in `scripts/`. GitHub release workflows should call these scripts directly and upload the artifact paths they produce.

## Source of Truth

- macOS DMG: `scripts/build-dmg.sh`
- signed macOS DMG wrapper: `scripts/build-dmg-signed.sh`
- Windows installer: `scripts/build-setup.ps1`
- Windows installer definition: `scripts/installer/termy.iss`
- Linux tarball and AppImage: `scripts/build-linux.sh`
- app icon generation: `scripts/generate-icon.sh`
- release CI: `.github/workflows/release.yml`

The app version used by packaging scripts comes from `crates/desktop_app/Cargo.toml` unless an explicit version is passed.

## Artifact Paths

- macOS DMG: `dist/Termy-<version>-macos-<arch>[-signed].dmg`
- Windows setup: `target/dist/Termy-<version>-windows-<arch>-Setup.exe`
- Linux tarball: `target/dist/Termy-<version>-linux-<arch>.tar.gz`
- Linux AppImage: `target/dist/Termy-<version>-linux-<arch>.AppImage`

Release workflows must upload these exact locations. If a packaging script changes its output path or file name, update this document, `justfile`, `.github/workflows/release.yml`, and `scripts/check-boundaries.sh` in the same change.

## Local Entrypoints

```sh
just build-dmg -- --version 0.3.0 --arch arm64
just build-setup -- -Version 0.3.0 -Arch x64 -Target x86_64-pc-windows-msvc
./scripts/build-linux.sh --version 0.3.0 --arch x86_64 --target x86_64-unknown-linux-gnu
```

Use `scripts/build-dmg-signed.sh` when a Developer ID signing identity is required. Unsigned DMGs should use `scripts/build-dmg.sh` directly.

## Browser Runtime Dependencies

Browser tabs use Wry native webviews:

- macOS packages use the system WebKit framework.
- Windows and Linux packages do not include a browser webview runtime because
  browser tabs are disabled for those desktop targets.

## Boundary Rules

- Keep packaging scripts in `scripts/`.
- Keep generated artifacts out of the repo and under `dist/` or `target/dist/`.
- Keep release CI aligned with the script outputs.
- Keep platform-specific installer definitions under `scripts/installer/` unless a platform needs a larger packaging tree.
- Do not restore obsolete `macos/` packaging paths without moving the scripts, docs, `justfile`, and release workflow together.

## Native macOS Host (Public Beta)

The Swift host in `macos/` is packaged on its own: `macos/scripts/build-dmg.sh` produces `macos/dist/Termy-<version>-macos-<arch>.dmg`, and `.github/workflows/macos-native-release.yml` publishes it as `Termy-native-<version>-macos-<arch>.dmg` on a `macos-native-v*` tag, in a prerelease of its own. See `macos/docs/native-release.md`.

That tree is not product packaging. `scripts/` remains the only source of truth for the GPUI desktop app, and `.github/workflows/release.yml` must never reference `macos/scripts` or `macos/dist` — `scripts/check-boundaries.sh` enforces it.

## Validation

Run these checks after packaging or release workflow changes:

```sh
bash -n scripts/build-dmg.sh scripts/build-dmg-signed.sh scripts/build-linux.sh
pwsh -NoProfile -Command '$null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/build-setup.ps1", [ref]$null, [ref]$null)' # when PowerShell is available
just check-boundaries
```
