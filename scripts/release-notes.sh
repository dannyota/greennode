#!/usr/bin/env bash
set -euo pipefail

body="$(< "$1")"
repo="${GITHUB_REPOSITORY:-dannyota/greennode}"
compare="$(printf '%s\n' "$body" | grep -o "https://github.com/$repo/compare/[^ ]*" || true)"

echo "## What's Changed"
if [ -z "$compare" ]; then
  echo
  echo "First release."
  exit 0
fi
jq -e 'type == "object" and (.commits | type == "array")' "$2" > /dev/null
jq -r '
  [.commits[] | select(.parents | length == 1)]
  | .[]
  | "* \(.commit.message | split("\n")[0]) (\(.sha[0:7]))"' "$2"
echo
echo "**Full Changelog**: $compare"
