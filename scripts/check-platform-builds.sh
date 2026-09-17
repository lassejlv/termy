#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
  cat <<EOF
Usage: $0 --native|--local-cross

Modes:
  --native       Run the platform checks for the current OS runner.
  --local-cross  Run best-effort cross-platform checks available from macOS.

Environment:
  TERMY_CHECK_XWIN_MSVC=1  Also run a cargo-xwin desktop check for
                           x86_64-pc-windows-msvc when cargo-xwin is installed.
EOF
}

die() {
  echo "Error: $*" >&2
  exit 1
}

log() {
  echo "==> $*"
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "'$1' is required"
}

require_rust_target() {
  local target="$1"
  rustup target list --installed | grep -qx "$target" || die "Rust target is not installed: $target"
}

check_powershell_script() {
  require_cmd pwsh
  pwsh -NoProfile -Command '
    $tokens = $null
    $errors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/build-setup.ps1", [ref] $tokens, [ref] $errors)
    if ($errors.Count -gt 0) {
      $errors | ForEach-Object { Write-Error $_.Message }
      exit 1
    }
  '
}

run_desktop_checks() {
  log "Checking desktop and CLI crates"
  cargo check -p termy -p termy_cli

  log "Testing command availability"
  cargo test -p termy_core -p termy_cli
}

run_native() {
  case "$(uname -s)" in
    Linux)
      run_desktop_checks
      log "Checking Linux packaging script syntax"
      bash -n scripts/build-linux.sh scripts/install-linux.sh
      ;;
    Darwin)
      run_desktop_checks
      ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT)
      run_desktop_checks
      log "Checking Windows setup script syntax"
      check_powershell_script
      ;;
    *)
      die "Unsupported native platform: $(uname -s)"
      ;;
  esac
}

run_local_cross() {
  log "Checking shared command core for Windows MSVC"
  require_rust_target x86_64-pc-windows-msvc
  cargo check -p termy_core --target x86_64-pc-windows-msvc

  log "Checking shared command core for Linux GNU"
  require_rust_target x86_64-unknown-linux-gnu
  cargo check -p termy_core --target x86_64-unknown-linux-gnu

  if [[ "$(uname -s)" == "Darwin" ]]; then
    log "Checking macOS x86_64 desktop target"
    require_rust_target x86_64-apple-darwin
    cargo check -p termy --target x86_64-apple-darwin
  fi

  if rustup target list --installed | grep -qx x86_64-pc-windows-gnu; then
    log "Checking Windows GNU desktop target"
    cargo check -p termy --target x86_64-pc-windows-gnu
  else
    log "Skipping Windows GNU desktop target; Rust target is not installed"
  fi

  if [[ "${TERMY_CHECK_XWIN_MSVC:-0}" == "1" ]]; then
    require_cmd cargo-xwin
    log "Checking Windows MSVC desktop target with cargo-xwin"
    cargo xwin check --cross-compiler clang -p termy -p termy_cli --target x86_64-pc-windows-msvc
  fi

  if command -v zig >/dev/null 2>&1; then
    log "Checking Linux CLI target with Zig"
    (
      local tmp_dir
      tmp_dir="$(mktemp -d)"
      trap 'rm -rf "$tmp_dir"' EXIT
      cat > "$tmp_dir/zig-x86_64-linux-gnu" <<'SH'
#!/usr/bin/env bash
args=()
for arg in "$@"; do
  case "$arg" in
    --target=x86_64-unknown-linux-gnu|--target=x86_64-linux-gnu)
      ;;
    *)
      args+=("$arg")
      ;;
  esac
done
exec zig cc -target x86_64-linux-gnu "${args[@]}"
SH
      chmod +x "$tmp_dir/zig-x86_64-linux-gnu"
      CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="$tmp_dir/zig-x86_64-linux-gnu" \
        CC_x86_64_unknown_linux_gnu="$tmp_dir/zig-x86_64-linux-gnu" \
        cargo check -p termy_cli --target x86_64-unknown-linux-gnu
    )
  else
    log "Skipping Linux CLI cross-check; zig is not installed"
  fi
}

cd "$REPO_ROOT"

case "${1:-}" in
  --native)
    run_native
    ;;
  --local-cross)
    run_local_cross
    ;;
  --help|-h)
    usage
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac
