#!/usr/bin/env bash
# Fail CI when tracked Rust sources under crates/ exceed the line budget.
# Invoked from scripts/check-boundaries.sh (and via `just check-file-sizes`).
set -euo pipefail

MAX_LINES=1500

# Grandfathered files over MAX_LINES (warn only). Shrink this list per
# docs/engineering/terminal-view-decomposition.md — do not add entries casually.
ALLOWLIST=(
  crates/core/src/keyboard.rs
  crates/core/src/kitty_graphics.rs
  crates/core/src/runtime.rs
  crates/desktop_app/src/settings_view/sections.rs
  crates/desktop_app/src/terminal_view/command_palette/mod.rs
  crates/desktop_app/src/terminal_view/inline_input.rs
  crates/desktop_app/src/terminal_view/interaction/selection.rs
  crates/desktop_app/src/terminal_view/mod.rs
  crates/desktop_app/src/terminal_view/persistence.rs
  crates/desktop_app/src/terminal_view/render.rs
  crates/desktop_app/src/terminal_view/tabs/lifecycle.rs
  crates/core/src/ffi/mod.rs
  crates/core/src/plugin_runtime/mod.rs
  crates/core/src/plugin_runtime/tests.rs
  crates/desktop_app/src/terminal_ui/grid.rs
  crates/cli/src/xtask/benchmark.rs
)

is_allowlisted() {
  local file="$1"
  local allowed
  for allowed in "${ALLOWLIST[@]}"; do
    if [[ "$file" == "$allowed" ]]; then
      return 0
    fi
  done
  return 1
}

failed=0
warnings=0

while IFS= read -r file; do
  [[ -f "$file" ]] || continue
  lines=$(wc -l <"$file" | tr -d '[:space:]')
  if (( lines > MAX_LINES )); then
    if is_allowlisted "$file"; then
      echo "WARNING: allowlisted file exceeds ${MAX_LINES} lines (${lines}): ${file}" >&2
      warnings=$((warnings + 1))
    else
      echo "File size check failed: exceeds ${MAX_LINES} lines (${lines}): ${file}" >&2
      failed=1
    fi
  fi
done < <(rg --files crates -g '*.rs' | sort)

if (( failed != 0 )); then
  echo "File size checks failed" >&2
  exit 1
fi

if (( warnings > 0 )); then
  echo "File size checks passed with ${warnings} allowlisted warning(s)"
else
  echo "File size checks passed"
fi
