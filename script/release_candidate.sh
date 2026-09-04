#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ALLOW_DIRTY=0
ARCH_MODE="universal"

while (( $# > 0 )); do
  case "$1" in
    --allow-dirty)
      ALLOW_DIRTY=1
      ;;
    --native)
      ARCH_MODE="native"
      ;;
    --universal)
      ARCH_MODE="universal"
      ;;
    *)
      echo "usage: $0 [--allow-dirty] [--native|--universal]" >&2
      exit 2
      ;;
  esac
  shift
done

cd "$ROOT_DIR"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "release candidates must be built from a Git checkout" >&2
  exit 1
fi

REVISION="$(git rev-parse HEAD)"
BRANCH="$(git symbolic-ref --quiet --short HEAD || printf 'detached')"
TREE_STATUS="$(git status --porcelain=v1 --untracked-files=all)"
DIRTY=false
if [[ -n "$TREE_STATUS" ]]; then
  DIRTY=true
  if (( ALLOW_DIRTY == 0 )); then
    echo "refusing to build a release candidate from a dirty working tree" >&2
    echo "commit or stash all tracked and untracked changes first" >&2
    echo "use --allow-dirty only for local, non-publishable validation" >&2
    exit 1
  fi
fi

PACKAGE_ID="$(cargo pkgid --locked --manifest-path "$ROOT_DIR/Cargo.toml" -p tmon)"
VERSION="${PACKAGE_ID##*@}"
BUILD_NUMBER="${TMON_BUILD_NUMBER:-1}"
MINIMUM_MACOS="${TMON_MINIMUM_MACOS:-14.0}"
BUNDLE_IDENTIFIER="${TMON_BUNDLE_IDENTIFIER:-com.tmon.app}"
SIGN_IDENTITY="${TMON_SIGN_IDENTITY:--}"
export TMON_BUILD_NUMBER="$BUILD_NUMBER"
export TMON_MINIMUM_MACOS="$MINIMUM_MACOS"
export TMON_BUNDLE_IDENTIFIER="$BUNDLE_IDENTIFIER"
export TMON_SIGN_IDENTITY="$SIGN_IDENTITY"
export TMON_SOURCE_REVISION="$REVISION"
export TMON_SOURCE_DIRTY="$DIRTY"

case "$ARCH_MODE" in
  native)
    ARCHIVE_ARCH="$(uname -m)"
    ;;
  universal)
    ARCHIVE_ARCH="universal"
    ;;
esac

ARCHIVE_BASENAME="Tmon-$VERSION-$BUILD_NUMBER-macos-$ARCHIVE_ARCH.zip"
ARCHIVE_PATH="$ROOT_DIR/dist/$ARCHIVE_BASENAME"
CHECKSUM_PATH="$ARCHIVE_PATH.sha256"
EVIDENCE_BASENAME="Tmon-$VERSION-$BUILD_NUMBER-release-evidence.json"
EVIDENCE_PATH="$ROOT_DIR/dist/$EVIDENCE_BASENAME"
for output in "$ARCHIVE_PATH" "$CHECKSUM_PATH" "$EVIDENCE_PATH"; do
  if [[ -e "$output" ]]; then
    echo "refusing to overwrite release-candidate output: $output" >&2
    echo "choose a new TMON_BUILD_NUMBER so candidate evidence remains immutable" >&2
    exit 1
  fi
done

echo "release candidate: Tmon $VERSION ($BUILD_NUMBER)"
echo "revision: $REVISION"
echo "tree: $([[ "$DIRTY" == true ]] && printf 'dirty local validation' || printf 'clean')"

EXPECTED_CARGO_DENY_VERSION="cargo-deny 0.19.0"
if ! command -v cargo-deny >/dev/null 2>&1; then
  echo "cargo-deny 0.19.0 is required; install it with:" >&2
  echo "  cargo install cargo-deny --version 0.19.0 --locked" >&2
  exit 1
fi
if [[ "$(cargo-deny --version)" != "$EXPECTED_CARGO_DENY_VERSION" ]]; then
  echo "release candidates require $EXPECTED_CARGO_DENY_VERSION" >&2
  exit 1
fi

echo "gate: RustSec advisories"
bash "$ROOT_DIR/script/dependency_audit.sh"

echo "gate: unsafe boundary inventory"
bash "$ROOT_DIR/script/audit_unsafe.sh"

echo "gate: dependency licenses and sources"
cargo-deny check licenses sources

echo "gate: formatting"
cargo fmt --all -- --check

echo "gate: clippy"
cargo clippy --locked --workspace --all-targets -- -D warnings

echo "gate: release tests"
cargo test --locked --release --workspace

echo "gate: C ABI consumer"
bash "$ROOT_DIR/script/test_ffi_c.sh"

echo "gate: package and bundle validation"
bash "$ROOT_DIR/script/package_macos.sh" "--$ARCH_MODE"

if [[ ! -f "$ARCHIVE_PATH" || ! -f "$CHECKSUM_PATH" ]]; then
  echo "packaging did not produce the expected archive and checksum" >&2
  exit 1
fi
(
  cd "$ROOT_DIR/dist"
  shasum -a 256 -c "$ARCHIVE_BASENAME.sha256"
)
ARCHIVE_SHA256="$(awk 'NR == 1 { print $1 }' "$CHECKSUM_PATH")"

SIGNING_MODE="developer-id"
if [[ "$SIGN_IDENTITY" == "-" ]]; then
  SIGNING_MODE="ad-hoc"
fi

EVIDENCE_PLIST="$(mktemp /tmp/tmon-release-evidence.XXXXXX.plist)"
trap 'rm -f "$EVIDENCE_PLIST"' EXIT

plutil -create xml1 "$EVIDENCE_PLIST"
plutil -insert schema_version -integer 1 "$EVIDENCE_PLIST"
plutil -insert generated_at -string "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$EVIDENCE_PLIST"
plutil -insert source -dictionary "$EVIDENCE_PLIST"
plutil -insert source.revision -string "$REVISION" "$EVIDENCE_PLIST"
plutil -insert source.branch -string "$BRANCH" "$EVIDENCE_PLIST"
plutil -insert source.dirty -bool "$DIRTY" "$EVIDENCE_PLIST"
plutil -insert candidate -dictionary "$EVIDENCE_PLIST"
plutil -insert candidate.version -string "$VERSION" "$EVIDENCE_PLIST"
plutil -insert candidate.build_number -string "$BUILD_NUMBER" "$EVIDENCE_PLIST"
plutil -insert candidate.bundle_identifier -string "$BUNDLE_IDENTIFIER" "$EVIDENCE_PLIST"
plutil -insert candidate.minimum_macos -string "$MINIMUM_MACOS" "$EVIDENCE_PLIST"
plutil -insert candidate.architecture -string "$ARCH_MODE" "$EVIDENCE_PLIST"
plutil -insert toolchain -dictionary "$EVIDENCE_PLIST"
plutil -insert toolchain.rustc -string "$(rustc --version)" "$EVIDENCE_PLIST"
plutil -insert artifact -dictionary "$EVIDENCE_PLIST"
plutil -insert artifact.path -string "dist/$ARCHIVE_BASENAME" "$EVIDENCE_PLIST"
plutil -insert artifact.sha256 -string "$ARCHIVE_SHA256" "$EVIDENCE_PLIST"
plutil -insert gates -dictionary "$EVIDENCE_PLIST"
plutil -insert gates.dependency_policy -bool true "$EVIDENCE_PLIST"
plutil -insert gates.rustsec_advisories -bool true "$EVIDENCE_PLIST"
plutil -insert gates.unsafe_boundaries -bool true "$EVIDENCE_PLIST"
plutil -insert gates.format -bool true "$EVIDENCE_PLIST"
plutil -insert gates.clippy -bool true "$EVIDENCE_PLIST"
plutil -insert gates.release_tests -bool true "$EVIDENCE_PLIST"
plutil -insert gates.c_abi_smoke -bool true "$EVIDENCE_PLIST"
plutil -insert gates.bundle_validation -bool true "$EVIDENCE_PLIST"
plutil -insert distribution -dictionary "$EVIDENCE_PLIST"
plutil -insert distribution.signing -string "$SIGNING_MODE" "$EVIDENCE_PLIST"
plutil -insert distribution.publishable -bool false "$EVIDENCE_PLIST"
plutil -insert distribution.reason -string \
  "Internal candidate only; notarization, compatibility, performance, and clean-install evidence are separate release gates." \
  "$EVIDENCE_PLIST"
plutil -convert json -o "$EVIDENCE_PATH" "$EVIDENCE_PLIST"
plutil -p "$EVIDENCE_PATH" >/dev/null

echo "evidence: $EVIDENCE_PATH"
if [[ "$DIRTY" == true ]]; then
  echo "result: deterministic gates passed for a dirty, non-publishable local candidate"
else
  echo "result: deterministic gates passed for clean revision $REVISION"
fi
