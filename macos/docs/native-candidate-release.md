# Native candidate release

The `macOS Native Candidates` workflow builds unpublished arm64 and x86_64
Swift/AppKit candidates. It runs independently of the public `Release` workflow,
so a native candidate failure cannot remove or block the GPUI macOS artifacts.

Dispatch the workflow with the numeric version already stored in
`crates/desktop_app/Cargo.toml`. A published GitHub release also triggers the
candidate workflow, but its DMGs remain Actions artifacts and are not attached
to the public release.

Each architecture artifact contains the exact DMG, its SHA-256 checksum, and a
metadata file recording the source commit, version, architecture, target, Rust,
Swift, and Xcode versions. The workflow retains candidates for 30 days.

## Local reproduction

Run the same packaging and verification steps from the candidate commit:

```bash
version="$(awk '/^version = / { gsub(/"/, "", $3); print $3; exit }' crates/desktop_app/Cargo.toml)"
arch="$(uname -m)"
case "$arch" in
  arm64) target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac

./macos/scripts/build-dmg.sh --version "$version" --arch "$arch" --target "$target" --no-layout
./macos/scripts/check-release-readiness.sh \
  --dmg "macos/dist/Termy-${version}-macos-${arch}.dmg" \
  --arch "$arch" \
  --version "$version"
./macos/scripts/check-cli-install-smoke.sh --app "macos/.build/dmg-${arch}/Termy.app"
shasum -a 256 "macos/dist/Termy-${version}-macos-${arch}.dmg"
```

The workflow is intentionally unsigned. Developer ID signing, notarization,
stapling, and Gatekeeper validation remain the final release task.

To publish a candidate, tag it and let the `macOS Native Release` workflow build
and attach both architectures — see [native release](native-release.md).
