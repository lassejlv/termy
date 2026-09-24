# Scripts

This directory owns local and CI packaging entrypoints plus repository maintenance checks.

## Packaging

- `build-dmg.sh`: builds the macOS DMG into `dist/`.
- `build-dmg-signed.sh`: signed-DMG wrapper around `build-dmg.sh`.
- `setup-macos-signing.sh`: imports GitHub Actions Developer ID and notarization credentials into an ephemeral keychain.
- `build-setup.ps1`: builds the Windows installer into `target/dist/`.
- `build-linux.sh`: builds Linux tarballs and AppImages into `target/dist/`.
- `installer/termy.iss`: Inno Setup definition for the Windows installer.
- `file-manager/`: Explorer/Finder/Nautilus/Nemo/KDE "Open new Termy tab here" payloads copied into release packages.
- `install-linux.sh`: user-facing Linux install helper.

See `docs/architecture/release-packaging.md` for artifact names and release workflow ownership.

## Maintenance

- `check-boundaries.sh`: validates crate dependency boundaries, complexity allow tracking, ignored-test budget, dependency license policy, generated docs, required ownership docs, release packaging path alignment, and Rust file size limits (via `check-file-sizes.sh`).
- `check-file-sizes.sh`: fails if any tracked `crates/**/*.rs` exceeds 1,500 lines (allowlisted legacy files warn only).
- `generate-icon.sh`: regenerates app icon assets from the source image.
- `test_osc_sequences.sh`: exercises OSC behavior manually.

Keep platform packaging entrypoints here unless the architecture docs and release workflow move with them.
