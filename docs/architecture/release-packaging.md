# Release Packaging

Release packaging is rooted in `scripts/`. GitHub release workflows should call these scripts directly and upload the artifact paths they produce.

## Source of Truth

- macOS DMG: `scripts/build-dmg.sh`
- signed macOS DMG wrapper: `scripts/build-dmg-signed.sh`
- Windows installer: `scripts/build-setup.ps1`
- Windows installer definition: `scripts/installer/termy.iss`
- Linux tarball and AppImage: `scripts/build-linux.sh`
- app icon generation: `scripts/generate-icon.sh`
- release artifact CI: `.github/workflows/release.yml`
- stable release finalization and AUR dispatch: `.github/workflows/finalize-stable-release.yml`

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

## Boundary Rules

- Keep packaging scripts in `scripts/`.
- Keep generated artifacts out of the repo and under `dist/` or `target/dist/`.
- Keep release CI aligned with the script outputs.
- Keep platform-specific installer definitions under `scripts/installer/` unless a platform needs a larger packaging tree.
- File-manager context-menu payloads belong in `scripts/file-manager/` and must be copied by `scripts/build-linux.sh`, `scripts/build-dmg.sh`, and `scripts/installer/termy.iss`. Do not add a parallel `packaging/` tree for those files.

## Validation

Run these checks after packaging or release workflow changes:

```sh
bash -n scripts/build-dmg.sh scripts/build-dmg-signed.sh scripts/build-linux.sh
pwsh -NoProfile -Command '$null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/build-setup.ps1", [ref]$null, [ref]$null)' # when PowerShell is available
just check-boundaries
```

For a local GPUI release performance gate:

```sh
cargo build --release -p termy
./scripts/check-gpui-launch-idle.sh
```

The gate can seed `--plugins`, require `--expect-no-bun`, or load a fixture with
`--workspace-store` to cover plugin and persisted-session regressions.
