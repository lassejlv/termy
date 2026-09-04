#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXPECTED_VERSION="cargo-audit-audit 0.22.2"
REVIEW_BEFORE="2026-10-01"

cd "$ROOT_DIR"

if ! command -v cargo-audit >/dev/null 2>&1; then
  echo "cargo-audit 0.22.2 is required; install it with:" >&2
  echo "  cargo install cargo-audit --version 0.22.2 --locked" >&2
  exit 1
fi
if [[ "$(cargo audit -V)" != "$EXPECTED_VERSION" ]]; then
  echo "release candidates require $EXPECTED_VERSION" >&2
  exit 1
fi

TODAY="$(date -u '+%Y-%m-%d')"
if [[ ! "$TODAY" < "$REVIEW_BEFORE" ]]; then
  echo "the temporary RustSec exceptions expired on $REVIEW_BEFORE" >&2
  echo "review DEPENDENCY_AUDIT.md and update or remove each exception" >&2
  exit 1
fi

# Both exceptions are transitive and narrowly reviewed in DEPENDENCY_AUDIT.md. All other
# vulnerabilities, unsoundness notices, yanks, and unmaintained warnings remain release-blocking.
cargo audit -D warnings \
  --ignore RUSTSEC-2026-0192 \
  --ignore RUSTSEC-2026-0253
