#!/usr/bin/env bash
# Usage: scripts/test/check-lengths-test.sh
set -Eeuo pipefail

ROOT=$(cd "${BASH_SOURCE[0]%/*}/../.." && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/greennode-check-lengths.XXXXXX")
trap 'rm -rf -- "$WORK"' EXIT
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
REPO=$WORK/repo
cases=0

fail() { printf 'check-lengths-test: %s\n' "$*" >&2; exit 1; }

fresh_repo() {
  rm -rf -- "$REPO"
  mkdir -p "$REPO/scripts"
  cp "$ROOT/scripts/check-lengths.sh" "$ROOT/scripts/count-rust-lines.py" "$ROOT/scripts/check-comments.py" "$REPO/scripts/"
  : >"$REPO/scripts/lengths-baseline.txt"
  git init -q "$REPO"
}

lines_file() {
  local path=$1 lines=$2 header_line=${3:-0} header=${4:-'// fixture header'}
  mkdir -p "$(dirname "$REPO/$path")"
  awk -v n="$lines" -v h="$header_line" -v text="$header" '
    BEGIN { for (i = 1; i <= n; i++) print (i == h ? text : "line " i) }
  ' >"$REPO/$path"
  git -C "$REPO" add -- "$path"
}

expect() {
  local label=$1 expected=$2 message=${3:-} status=0 output
  if (($# >= 3)); then shift 3; else shift "$#"; fi
  output=$(bash "$REPO/scripts/check-lengths.sh" "$@" 2>&1) || status=$?
  ((status == expected)) || fail "$label: expected exit $expected, got $status: $output"
  if [[ -n "$message" ]]; then
    [[ "$output" == "$message" ]] || fail "$label: unexpected output: $output"
  else
    [[ "$output" == 'check-lengths: ok '* ]] || fail "$label: missing summary: $output"
    [[ "$output" != *$'\n'* ]] || fail "$label: extra output: $output"
  fi
  cases=$((cases + 1))
}

fresh_repo
lines_file src/main.rs 700
expect 'Rust at limit' 0
lines_file src/main.rs 701
expect 'Rust over limit' 1 'src/main.rs: 701 lines (limit 700)'

fresh_repo
mkdir -p "$REPO/src"
awk 'BEGIN { for (i = 1; i <= 701; i++) print "line " i }' >"$REPO/src/new.rs"
expect 'untracked Rust over limit' 1 'src/new.rs: 701 lines (limit 700)'
printf '701 src/new.rs\n' >"$REPO/scripts/lengths-baseline.txt"
expect 'untracked capped Rust exists' 0

fresh_repo
lines_file docs/page.md 400
expect 'Markdown at limit' 0
lines_file docs/page.md 401
expect 'Markdown over limit' 1 'docs/page.md: 401 lines (limit 400)'

fresh_repo
lines_file src/main.rs 700
printf '#[cfg(test)]\nmod checks {\n    fn t() {}\n    fn u() {}\n}\n' >>"$REPO/src/main.rs"
expect 'test module excluded' 0
printf 'fn production() {}\n' >>"$REPO/src/main.rs"
expect 'production after the test module counts' 1 'src/main.rs: 701 lines (limit 700)'

fresh_repo
lines_file src/main.rs 750
printf '750 src/main.rs\n' >"$REPO/scripts/lengths-baseline.txt"
git -C "$REPO" add scripts/lengths-baseline.txt
git -C "$REPO" -c user.name=Test -c user.email=test@example.invalid -c commit.gpgsign=false commit -qm baseline
lines_file src/main.rs 751
printf '751 src/main.rs\n' >"$REPO/scripts/lengths-baseline.txt"
expect 'raised baseline cap' 1 'src/main.rs: cap 751 exceeds base cap 750' --base-ref HEAD
lines_file src/main.rs 749
printf '749 src/main.rs\n' >"$REPO/scripts/lengths-baseline.txt"
expect 'lower baseline cap' 0 '' --base-ref HEAD

fresh_repo
lines_file src/main.rs 701
printf '701  src/main.rs\n' >"$REPO/scripts/lengths-baseline.txt"
expect 'malformed baseline row' 1 'scripts/lengths-baseline.txt: malformed baseline row: 701  src/main.rs'

fresh_repo
lines_file 'src/a=b.rs' 701
printf '701 src/a=b.rs\n' >"$REPO/scripts/lengths-baseline.txt"
expect 'equals sign in a baseline path' 0

fresh_repo
lines_file tests/stray.rs 701
expect 'root test file counted' 1 'tests/stray.rs: 701 lines (limit 700)'

fresh_repo
lines_file crates/x/tests/cli.rs 701
lines_file crates/x/tests/support/helper.rs 701
expect 'crate test roots exempt' 0
lines_file crates/x/tests/deep/helper.rs 701
lines_file crates/x/src/lib.rs 701
expect 'crate production and nested tests counted' 1 $'crates/x/src/lib.rs: 701 lines (limit 700)\ncrates/x/tests/deep/helper.rs: 701 lines (limit 700)'

printf 'check-lengths-test: passed (%s cases)\n' "$cases"
