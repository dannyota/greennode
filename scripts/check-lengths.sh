#!/usr/bin/env bash
# Usage: scripts/check-lengths.sh [--base-ref <ref>]
set -Eeuo pipefail

RUST_MAX=700
MD_MAX=400

cd "${BASH_SOURCE[0]%/*}/.."
BASELINE=scripts/lengths-baseline.txt
violations=0
base_ref=''
if (($#)); then
  if [[ $# != 2 || $1 != --base-ref || -z $2 ]]; then
    printf 'usage: check-lengths.sh [--base-ref <ref>]\n' >&2
    exit 1
  fi
  base_ref=$2
fi

rows() { sed '/^#/d; /^$/d'; }

cap_for() {
  CAP_PATH="$1" awk '
    /^#/ || /^$/ { next }
    { n = $1; path = $0; sub(/^[0-9]+ /, "", path); if (path == ENVIRON["CAP_PATH"]) { print n; exit } }
  '
}

if ! awk -v name="$BASELINE" '
  /^#/ || /^$/ { next }
  !/^[1-9][0-9]* [^ ]+$/ { printf "%s: malformed baseline row: %s\n", name, $0; bad = 1; next }
  { path = $0; sub(/^[0-9]+ /, "", path); if (seen[path]++) { printf "%s: duplicate baseline path: %s\n", name, path; bad = 1 } }
  END { exit bad }
' <"$BASELINE"; then
  exit 1
fi

if [[ -n "$base_ref" ]]; then
  base_commit=$(git rev-parse --verify --end-of-options "$base_ref^{commit}" 2>/dev/null) || {
    printf 'check-lengths: invalid base ref: %s\n' "$base_ref" >&2
    exit 1
  }
  base_baseline=''
  base_has_baseline=0
  if [[ -n $(git ls-tree "$base_commit" -- "$BASELINE") ]]; then
    base_has_baseline=1
    base_baseline=$(git show "$base_commit:$BASELINE")
  else
    printf 'check-lengths: %s has no baseline; skipping base comparison\n' "$base_ref"
  fi
  ((base_has_baseline)) && while read -r cap file || [[ -n "$cap" ]]; do
    base_cap=$(cap_for "$file" <<<"$base_baseline")
    if [[ -z "$base_cap" ]]; then
      printf '%s: baseline entry absent from base ref\n' "$file"
      violations=$((violations + 1))
    elif ((cap > base_cap)); then
      printf '%s: cap %s exceeds base cap %s\n' "$file" "$cap" "$base_cap"
      violations=$((violations + 1))
    fi
  done < <(rows <"$BASELINE")
fi

exclude_tests=1
while IFS= read -r -d '' config; do
  if grep -Eq -e '^[[:space:]]*(ignore|disable_all_formatting|skip_children)[[:space:]]*=' -- "$config"; then
    exclude_tests=0
  fi
done < <(git ls-files -z -- '*rustfmt.toml')

count_lines() {
  if (($2)); then
    python3 -I scripts/count-rust-lines.py "$1"
  else
    awk 'END { print NR }' <"$1"
  fi
}

while IFS= read -r -d '' file; do
  exclude=0
  case "$file" in
    *.rs | *.inc)
      case "$file" in
        tests/support/*) continue ;;
        tests/*/*) ;;
        tests/*) continue ;;
      esac
      limit=$RUST_MAX
      if [[ "$file" == *.rs ]] && ((exclude_tests)) && [[ -f "$file" ]] &&
        grep -q -e '^#\[cfg(test)\]' -- "$file" &&
        ! grep -Eq -e 'rustfmt[[:space:]]*(::[[:space:]]*skip|_skip)' -- "$file"; then
        exclude=1
      fi
      ;;
    *.md | *.markdown)
      case "$file" in docs/plans/*) continue ;; esac
      limit=$MD_MAX
      ;;
    *) continue ;;
  esac
  [[ -f "$file" ]] || continue
  lines=$(count_lines "$file" "$exclude")
  cap=$(cap_for "$file" <"$BASELINE")
  if [[ -n "$cap" ]]; then
    if ((lines <= limit)); then
      printf '%s: %s lines (limit %s | cap %s), remove baseline entry\n' "$file" "$lines" "$limit" "$cap"
      violations=$((violations + 1))
    elif ((lines > cap)); then
      printf '%s: %s lines (cap %s)\n' "$file" "$lines" "$cap"
      violations=$((violations + 1))
    elif ((lines < cap)); then
      printf '%s: %s lines (cap %s), lower its cap to %s\n' "$file" "$lines" "$cap" "$lines"
      violations=$((violations + 1))
    fi
  elif ((lines > limit)); then
    printf '%s: %s lines (limit %s)\n' "$file" "$lines" "$limit"
    violations=$((violations + 1))
  fi
done < <(git ls-files -z -- '*.rs' '*.inc' '*.md' '*.markdown')

while read -r cap file || [[ -n "$cap" ]]; do
  if [[ ! -f "$file" ]] || ! git --literal-pathspecs ls-files --error-unmatch -- "$file" >/dev/null 2>&1; then
    printf '%s: 0 lines (cap %s), stale baseline path (missing or untracked)\n' "$file" "$cap"
    violations=$((violations + 1))
  fi
done < <(rows <"$BASELINE")

((violations == 0)) || exit 1
printf 'check-lengths: ok (Rust <= %s, Markdown <= %s lines)\n' "$RUST_MAX" "$MD_MAX"
