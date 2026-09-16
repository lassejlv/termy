set shell := ["bash", "-cu"]

# Show available recipes
@default:
    just --list

# Checkout-local instance lock so this does not silently attach to Termy.app.
run:
    TERMY_INSTANCE_HOME="${TERMY_INSTANCE_HOME:-{{ justfile_directory() }}/target/termy-dev-instance}" cargo run -p termy --release

# Compare the terminal engines, enforce Tmon's snapshot baseline, and write a text report.
benchmark-tmon:
    #!/usr/bin/env bash
    set -euo pipefail
    report="${TMON_BENCH_OUTPUT:-tmon-alacritty-benchmark.txt}"
    {
      rustc --version
      echo
      TERMY_CORE_TEST_BACKEND=alacritty cargo run --locked --quiet -p xtask --release --example engine_compare
    } 2>&1 | tee "$report"
    gate_status=0
    set +e
    {
      echo
      cargo run --locked --quiet --release --manifest-path tools/tmon-revision-gate/Cargo.toml
    } 2>&1 | tee -a "$report"
    gate_status=$?
    set -e
    {
      echo
      TERMY_CORE_TEST_BACKEND=alacritty TMON_BENCH_ALLOCATIONS_ONLY=1 cargo run --locked --quiet -p xtask --release --example engine_compare --features benchmark-allocations
    } 2>&1 | tee -a "$report"
    exit "$gate_status"

# Compare Tmon and a pinned local libghostty-vt build for memory and feed throughput.
benchmark-tmon-ghostty-memory:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$PWD"
    ghostty_dir="$(cd "${GHOSTTY_DIR:?set GHOSTTY_DIR to a Ghostty source checkout}" && pwd)"
    prefix="${GHOSTTY_VT_PREFIX:-$root/target/ghostty-vt-benchmark}"
    report="${TMON_GHOSTTY_MEMORY_OUTPUT:-tmon-ghostty-memory-benchmark.txt}"
    expected_ghostty_revision="9e30f70f23418fecbdca1088673000417527c4e4"
    ghostty_revision="$(git -C "$ghostty_dir" rev-parse HEAD)"
    if [[ "$ghostty_revision" != "$expected_ghostty_revision" ]]; then
      echo "Ghostty checkout must be at $expected_ghostty_revision, got $ghostty_revision" >&2
      exit 1
    fi
    (
      cd "$ghostty_dir"
      zig build \
        -Dapp-runtime=none \
        -Demit-lib-vt=true \
        -Demit-xcframework=false \
        -Doptimize=ReleaseFast \
        -Dcpu=baseline \
        --prefix "$prefix"
    )
    {
      rustc --version
      zig version
      echo "Termy commit: $(git rev-parse HEAD)"
      echo "Ghostty commit: $ghostty_revision"
      echo
      GHOSTTY_VT_PREFIX="$prefix" cargo run --locked --quiet --release \
        --manifest-path tools/tmon-ghostty-memory/Cargo.toml
      echo
      GHOSTTY_VT_PREFIX="$prefix" cargo run --locked --quiet --release \
        --manifest-path tools/tmon-ghostty-memory/Cargo.toml -- --throughput
    } 2>&1 | tee "$report"

run-cli *args:
    cargo run --bin termy-cli --release {{ args }}

# Check a benchmark summary against regression gates
check-performance *args:
    ./scripts/check-performance-gates.sh {{ args }}

test:
    cargo test -p termy --release

# All workspace crate tests (release). Target for CI — see docs/engineering/roadmap.md E0.1
test-workspace:
    cargo test --workspace --release

# Format check (no writes). Target for CI — see docs/engineering/roadmap.md E0.2
fmt-check:
    cargo fmt --all -- --check

# Local gate closest to target CI (see docs/engineering/roadmap.md E0.3)
validate: check fmt-check test-workspace check-boundaries
    cargo clippy --workspace --all-targets -- -D warnings

dev:
    TERMY_INSTANCE_HOME="${TERMY_INSTANCE_HOME:-{{ justfile_directory() }}/target/termy-dev-instance}" cargo watch -x "run -p termy --release"

build:
    cargo build -p termy --release

check:
    cargo check --workspace

clean:
    cargo clean --workspace && rm -rf ./target

# Generate macOS .icns file from assets/termy_icon@1024px.png
generate-icon:
    ./scripts/generate-icon.sh

# Build the GPUI Termy app bundle and DMG (unsigned by default)
# Example:

# just build-dmg -- --version 0.1.0 --arch arm64 --sign-identity "Developer ID Application: ..."
build-dmg *args:
    set -- {{ args }}; \
    if [ "${1-}" = "--" ]; then shift; fi; \
    ./scripts/build-dmg.sh "$@"

# Build Windows Setup.exe via Inno Setup
# Example:

# just build-setup -- -Version 0.1.0 -Arch x64 -Target x86_64-pc-windows-msvc
build-setup *args:
    set -- {{ args }}; \
    if [ "${1-}" = "--" ]; then shift; fi; \
    if command -v powershell >/dev/null 2>&1; then \
      powershell -NoProfile -ExecutionPolicy Bypass -File ./scripts/build-setup.ps1 "$@"; \
    elif command -v powershell.exe >/dev/null 2>&1; then \
      powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/build-setup.ps1 "$@"; \
    elif [ -x /c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe ]; then \
      /c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/build-setup.ps1 "$@"; \
    else \
      echo "PowerShell not found. Install PowerShell or run scripts/build-setup.ps1 directly from PowerShell."; \
      exit 127; \
    fi

# Render the local AUR PKGBUILD. Optional version defaults to crates/desktop_app/Cargo.toml.
aur-pkgbuild version="":
    ./scripts/aur/build-local.sh render "{{ version }}"

# Build the local AUR package under target/aur. Optional version defaults to crates/desktop_app/Cargo.toml.
aur-build version="":
    ./scripts/aur/build-local.sh build "{{ version }}"

# Build and install the local AUR package. Optional version defaults to crates/desktop_app/Cargo.toml.
aur-install version="":
    ./scripts/aur/build-local.sh install "{{ version }}"

generate-keybindings-doc:
    cargo run -p xtask -- generate-keybindings-doc

generate-config-doc:
    cargo run -p xtask -- generate-config-doc

check-keybindings-doc:
    cargo run -p xtask -- generate-keybindings-doc --check

check-config-doc:
    cargo run -p xtask -- generate-config-doc --check

check-boundaries:
    ./scripts/check-boundaries.sh

check-file-sizes:
    ./scripts/check-file-sizes.sh

test-tmux-integration:
    #!/usr/bin/env bash
    set -euo pipefail
    tmux_tests=(
      "termy_terminal_ui|integration|tmux_split_integration|tmux_split_vertical_then_horizontal_refresh_snapshot_parses_nested_layout"
      "termy_terminal_ui|integration|tmux_split_integration|tmux_repeated_split_refresh_cycles_remain_parseable"
      "termy_terminal_ui|integration|tmux_split_integration|tmux_new_window_after_inserts_immediately_after_target_window"
      "termy_terminal_ui|integration|tmux_split_integration|tmux_working_directory_flags_apply_to_session_window_and_split"
      "termy_terminal_ui|integration|tmux_split_integration|tmux_capture_full_rejoins_wrapped_input_rows"
      "termy_terminal_ui|integration|tmux_split_integration|managed_nonpersistent_drop_kills_session"
      "termy_terminal_ui|integration|tmux_split_integration|managed_persistent_drop_keeps_session_but_removes_client"
      "termy_terminal_ui|integration|tmux_split_integration|repeated_reconnect_does_not_increase_client_count"
      "termy_tmux_control_core|lib||session::tests::launches_and_drives_control_mode"
      "termy_ffi|integration|tmux_control_ffi|ffi_control_open_poll_send_close"
    )
    if (( ${#tmux_tests[@]} != 10 )); then
      echo "Tmux integration configuration error: expected 10 tests, got ${#tmux_tests[@]}" >&2
      exit 1
    fi
    unique_count=$(
      printf '%s\n' "${tmux_tests[@]}" | cut -d '|' -f 4 | LC_ALL=C sort -u | wc -l | tr -d '[:space:]'
    )
    if (( unique_count != ${#tmux_tests[@]} )); then
      echo "Tmux integration configuration error: test names must be unique" >&2
      exit 1
    fi
    for test_spec in "${tmux_tests[@]}"; do
      just _validate-exact-tmux-test "$test_spec"
      IFS='|' read -r package target_kind target_name test_name <<<"$test_spec"
      cargo_args=(test --locked -p "$package")
      case "$target_kind" in
        lib) cargo_args+=(--lib) ;;
        integration) cargo_args+=(--test "$target_name") ;;
      esac
      cargo "${cargo_args[@]}" "$test_name" -- \
        --ignored --exact --nocapture --test-threads=1
    done

_validate-exact-tmux-test test_spec:
    #!/usr/bin/env bash
    set -euo pipefail
    test_spec={{ quote(test_spec) }}
    IFS='|' read -r package target_kind target_name test_name <<<"$test_spec"
    cargo_args=(test --locked -p "$package")
    case "$target_kind" in
      lib) cargo_args+=(--lib) ;;
      integration) cargo_args+=(--test "$target_name") ;;
      *)
        echo "Tmux integration configuration error: invalid target kind '$target_kind'" >&2
        exit 1
        ;;
    esac
    listed=$(cargo "${cargo_args[@]}" "$test_name" -- --ignored --exact --list)
    match_count=$(printf '%s\n' "$listed" | grep -Fxc -- "$test_name: test" || true)
    if (( match_count != 1 )); then
      echo "Tmux integration configuration error: '$test_name' matched $match_count tests" >&2
      printf '%s\n' "$listed" >&2
      exit 1
    fi

# Bump version in desktop app + cli Cargo.toml. Kind: major | minor | patch
bump kind:
    #!/usr/bin/env bash
    set -euo pipefail
    KIND="{{ kind }}"
    CURRENT=$(grep -m1 '^version = ' crates/desktop_app/Cargo.toml | sed -E 's/version = "(.*)"/\1/')
    IFS=. read -r MAJOR MINOR PATCH <<<"$CURRENT"
    case "$KIND" in
      major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
      minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
      patch) PATCH=$((PATCH + 1)) ;;
      *) echo "usage: just bump <major|minor|patch>"; exit 1 ;;
    esac
    NEW="$MAJOR.$MINOR.$PATCH"
    export NEW
    perl -i -pe '$done ||= s/^version = ".*"/version = "$ENV{NEW}"/' crates/desktop_app/Cargo.toml
    perl -i -pe '$done ||= s/^version = ".*"/version = "$ENV{NEW}"/' crates/cli/Cargo.toml
    cargo update --workspace --offline
    echo "Bumped $CURRENT -> $NEW"

# Set exact version in desktop app + cli Cargo.toml (e.g. just set-version 0.2.12)
set-version version:
    #!/usr/bin/env bash
    set -euo pipefail
    NEW="{{ version }}"
    if ! [[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "usage: just set-version <major.minor.patch> (got: $NEW)"; exit 1
    fi
    CURRENT=$(grep -m1 '^version = ' crates/desktop_app/Cargo.toml | sed -E 's/version = "(.*)"/\1/')
    export NEW
    perl -i -pe '$done ||= s/^version = ".*"/version = "$ENV{NEW}"/' crates/desktop_app/Cargo.toml
    perl -i -pe '$done ||= s/^version = ".*"/version = "$ENV{NEW}"/' crates/cli/Cargo.toml
    cargo update --workspace --offline
    echo "Set $CURRENT -> $NEW"
