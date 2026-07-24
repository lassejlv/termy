# Native macOS release

The `macOS Native Release` workflow
(`.github/workflows/macos-native-release.yml`) builds the native Swift/AppKit
host from `macos/` for Apple silicon and Intel and publishes both DMGs to their
own GitHub release.

It is deliberately separate from the public `Release` workflow. The native host
is a public beta: it ships on its own tag, with its own gates and its own
release page, so a native failure can never block or alter the GPUI desktop
artifacts, and the two DMGs never land in the same release.

Related workflows:

- [`macOS Native Candidates`](native-candidate-release.md) — unpublished,
  30-day candidate artifacts. Use it to hand a build around before tagging.
- `macOS Native Swift` — per-PR tests plus an unsigned build gate for both
  architectures. Nothing is published.

## Publishing

Tag the candidate commit and push:

```sh
version="$(awk '/^version = / { gsub(/"/, "", $3); print $3; exit }' crates/desktop_app/Cargo.toml)"
git tag "macos-native-v${version}"
git push origin "macos-native-v${version}"
```

Or dispatch `macOS Native Release` manually with the same version. Clear the
`publish` checkbox to run the full build and every gate without creating a
release — the DMGs are still uploaded as Actions artifacts.

## Versioning

`crates/desktop_app/Cargo.toml` stays the authoritative version. The workflow
refuses to build when the tag disagrees with it.

A tag may carry a prerelease suffix — `macos-native-v0.2.29-preview.2` — to
republish the same app version. The suffix travels into the release name and
the DMG file names; the version inside the bundle stays numeric.

## Artifacts

Published to a prerelease GitHub release at the tag, never marked latest:

- `Termy-native-<release version>-macos-arm64.dmg`
- `Termy-native-<release version>-macos-x86_64.dmg`
- a `.sha256` next to each DMG, plus a combined `checksums.txt`
- `native-release-<arch>.metadata` — source commit, versions, target, Rust,
  Swift, and Xcode versions

The `native` infix is deliberate. `macos/scripts/build-dmg.sh` produces
`Termy-<version>-macos-<arch>.dmg`, the same name the GPUI desktop app uses, so
the published copy is renamed to keep a downloaded beta distinguishable from the
product build once it leaves the release page. Both install as `Termy.app` with
the same bundle identifier, so they overwrite each other in `/Applications`.

The launch and soak JSON reports are kept as Actions artifacts for 30 days and
are not attached to the release.

## Gates

`resolve` validates the version, then `verify` builds the Swift host with
warnings as errors and runs the full `swift test` suite once. Each architecture
then runs:

| Gate | What it proves |
| --- | --- |
| `build-dmg.sh` (runs `check-release-readiness.sh --dmg`) | Bundle manifest, Mach-O architecture and load paths, read-only DMG contents, usable-window launch of the mounted app |
| `check-cli-install-smoke.sh` | "Install Command Line Tool…" works against an isolated HOME and shell profile |
| `check-launch-gate.sh` | Usable-window startup budget, settled CPU, one-tab and eight-pane RSS |
| `check-native-soak.sh` (30s, 30 cycles) | No window, pane, or RSS leak across PTY/resize/split/tab cycles |
| `check-release-readiness-regressions.sh` | The readiness gate actually rejects corrupted bundles and DMGs |

Render performance is intentionally not gated here — it runs in
`macOS Performance` where a threshold miss blocks a change instead of a
release.

## Signing

These builds are unsigned: no Developer ID signature, no notarization, no
stapling. Release notes tell downloaders to clear quarantine:

```sh
xattr -dr com.apple.quarantine /Applications/Termy.app
```

`macos/scripts/build-dmg.sh` already accepts `--sign-identity`, entitlements,
and notary credentials, so signing this pipeline is wiring a certificate into
the runner keychain and passing those flags — not new packaging work.

## Local reproduction

Same build and gates as one matrix leg:

```sh
version="$(awk '/^version = / { gsub(/"/, "", $3); print $3; exit }' crates/desktop_app/Cargo.toml)"
arch="$(uname -m)"
case "$arch" in
  arm64) target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac

./macos/scripts/build-dmg.sh --version "$version" --arch "$arch" --target "$target" --no-layout
./macos/scripts/check-cli-install-smoke.sh --app "macos/.build/dmg-${arch}/Termy.app"
./macos/scripts/check-launch-gate.sh --app "macos/.build/dmg-${arch}/Termy.app"
./macos/scripts/check-native-soak.sh --app "macos/.build/dmg-${arch}/Termy.app" \
  --duration-seconds 30 --minimum-cycles 30
./macos/scripts/check-release-readiness-regressions.sh \
  --app "macos/.build/dmg-${arch}/Termy.app" --arch "$arch" --version "$version"
```

Cross-building the other architecture works from either host; the launch and
soak gates run the foreign slice through Rosetta.
