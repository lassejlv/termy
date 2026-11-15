#!/usr/bin/env bash
# Prints the release notes for a Termy Next version, as Markdown.
#
#   scripts/next-release-notes.sh VERSION [CHANGELOG]
#
# The notes are the `## VERSION` section of next/CHANGELOG.md, else its
# `## Unreleased` section. Without either, GitHub writes them from the pull
# requests merged since the previous next-v* release (needs GH_TOKEN and
# GITHUB_REPOSITORY, as in the release workflow). Prints nothing when there
# are no notes at all.
set -euo pipefail

version="${1:?usage: next-release-notes.sh VERSION [CHANGELOG]}"
changelog="${2:-next/CHANGELOG.md}"

# The lines under the first `## <heading>` that matches, up to the next `## `.
section() {
  awk -v want="$1" '
    /^## / {
      if (found) exit
      heading = substr($0, 4)
      sub(/[[:space:]]+$/, "", heading)
      sub(/^\[/, "", heading); sub(/\].*$/, "", heading)
      sub(/^v/, "", heading)
      if (heading == want) { found = 1; next }
    }
    found { print }
  ' "$changelog" | sed -e '/./,$!d' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
}

if [[ -f "$changelog" ]]; then
  for heading in "$version" "Unreleased"; do
    notes="$(section "$heading")"
    if [[ -n "${notes//[[:space:]]/}" ]]; then
      printf '%s\n' "$notes"
      exit 0
    fi
  done
fi

# GitHub's generated notes, since the last Termy Next release.
if [[ -n "${GITHUB_REPOSITORY:-}" ]] && command -v gh >/dev/null; then
  previous="$(gh api "repos/$GITHUB_REPOSITORY/releases?per_page=100" \
    --jq '[.[] | select(.tag_name | startswith("next-v")) | select(.tag_name != "next-v'"$version"'") | .tag_name][0] // empty')"
  args=(-f tag_name="next-v$version")
  [[ -n "${GITHUB_SHA:-}" ]] && args+=(-f target_commitish="$GITHUB_SHA")
  [[ -n "$previous" ]] && args+=(-f previous_tag_name="$previous")
  gh api "repos/$GITHUB_REPOSITORY/releases/generate-notes" "${args[@]}" --jq .body 2>/dev/null || true
fi
