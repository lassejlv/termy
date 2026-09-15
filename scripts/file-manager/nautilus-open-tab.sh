#!/usr/bin/env bash
set -euo pipefail
paths=()
if (($# > 0)); then
	paths=("$@")
elif [[ -n "${NAUTILUS_SCRIPT_SELECTED_FILE_PATHS:-}" ]]; then
	while IFS= read -r line; do
		[[ -n "$line" ]] && paths+=("$line")
	done <<< "$NAUTILUS_SCRIPT_SELECTED_FILE_PATHS"
elif [[ -n "${NEMO_SCRIPT_SELECTED_FILE_PATHS:-}" ]]; then
	while IFS= read -r line; do
		[[ -n "$line" ]] && paths+=("$line")
	done <<< "$NEMO_SCRIPT_SELECTED_FILE_PATHS"
fi
target="${paths[0]:-}"
if [[ -z "$target" ]]; then
	echo "No folder selected" >&2
	exit 1
fi
if [[ -f "$target" ]]; then
	target="$(dirname "$target")"
fi
exec termy --working-directory "$target"
