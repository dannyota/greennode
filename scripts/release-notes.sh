#!/usr/bin/env bash
set -euo pipefail

tag="$1"
repo="${GITHUB_REPOSITORY:-dannyota/greennode}"
body="$(gh api "repos/$repo/releases/generate-notes" -f tag_name="$tag" --jq .body)"
compare="$(printf '%s\n' "$body" | grep -o "https://github.com/$repo/compare/[^ ]*" || true)"

echo "## What's Changed"
if [ -z "$compare" ]; then
  echo
  echo "First release."
  exit 0
fi
gh api "repos/$repo/compare/${compare##*/}" | jq -r '
  [.commits[] | select(.parents | length == 1)]
  | .[]
  | "* \(.commit.message | split("\n")[0]) (\(.sha[0:7]))"'
echo
echo "**Full Changelog**: $compare"
