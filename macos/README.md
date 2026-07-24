# Termy

Public beta. The native host is feature-rich and open to everyone, but it is not
yet the default production macOS build.

Native macOS 14+ SwiftUI terminal host backed by the repo-local `libtermy`.

Beta builds are unsigned and ship from their own `macos-native-v*` tag, separate
from the GPUI desktop app — see [Release](#release).

See the [native production roadmap](road.md) for current blockers, exit gates,
and release order.

## Run

```sh
cargo macos run
```

This builds `crates/ffi` from the repository root first, then builds and launches the SwiftPM app as `macos/dist/Termy.app`.

The shell compatibility entrypoint is:

```sh
./scripts/build_and_run.sh
```

At startup the app reads Termy's local config, including `working_dir`, `window_width`, and `window_height`.

## Shortcuts

- `Cmd+T`: new native macOS window tab
- `Cmd+D`: split right
- `Cmd+Shift+D`: split down
- `Cmd+Shift+W`: close focused pane
- `Cmd+]`: focus next pane
- `Cmd+F`: search the focused pane
- `Ctrl+C`: send interrupt to the focused terminal
- `Shift+Tab`: backtab through Termy's keyboard encoder
- Mouse or trackpad scroll: move through the focused pane's scrollback
- The right-side scrollbar appears while scrolling; drag its thumb to move through scrollback
- Drag split dividers with the mouse to resize panes

Keyboard input is encoded through repo-local `termy_core`, including Kitty keyboard protocol modes when terminal applications negotiate them.

## Validate

```sh
./scripts/check-config-matrix.sh
./scripts/stress-native.sh
./scripts/check-release-readiness.sh
```

`check-config-matrix.sh` runs Swift regression tests for shared config/schema parity. `stress-native.sh` runs persistence, selection, and render-clamping stress tests; pass `--launch` for a local app launch smoke. `check-release-readiness.sh` validates unsigned/ad-hoc release candidates with `--app PATH` or `--dmg PATH`, including metadata, resources, every Mach-O architecture/load path, read-only DMG contents, and a clean-state usable-window launch. Developer ID trust and notarization are intentionally separate final-release gates.

To exercise “Install Command Line Tool…” without touching your real home or
shell profile, stage the app and run the isolated smoke:

```sh
cargo macos build
./macos/scripts/check-cli-install-smoke.sh --app macos/dist/Termy.app
```

For a staged app bundle, run a local startup/RSS/CPU gate with:

```sh
./scripts/check-launch-gate.sh --app .build/dmg-arm64/Termy.app
```

Native DMGs are built with `./scripts/build-dmg.sh`, include the target-specific `termy-cli`, and automatically pass the mounted unsigned-release gate before succeeding. Unsigned candidates receive ad-hoc signatures so their nested code and resources are internally valid. Pass `--sign-identity` plus notary credentials to produce the final trusted artifact, or use `./scripts/build-dmg-signed.sh` when a missing signing identity should fail loudly.

Performance benchmark summaries from `cargo run -p xtask -- benchmark-compare` can be gated with:

```sh
./scripts/check-performance-gates.sh --summary target/macos-performance-gate/summary.json
```

## Release

Push a `macos-native-v<version>` tag (or dispatch the `macOS Native Release`
workflow) to build arm64 and Intel DMGs, run the release gates, and publish them
as their own public-beta prerelease — separate from the GPUI desktop app's
`Release` workflow. See [native release](docs/native-release.md) for versioning
rules, gates, and local reproduction, and [native candidate
release](docs/native-candidate-release.md) for unpublished candidate builds.

Beta DMGs carry no Developer ID signature or notarization, so first launch needs
the quarantine flag cleared:

```sh
xattr -dr com.apple.quarantine /Applications/Termy.app
```
