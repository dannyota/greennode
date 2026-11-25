#!/usr/bin/env bash
# Usage: GH_TOKEN=<token> scripts/tools-outdated.sh
set -Eeuo pipefail

cd "${BASH_SOURCE[0]%/*}/.."

stale=0
report() {
  printf 'outdated: %s\n' "$*"
  stale=$((stale + 1))
}

check() {
  if [[ "$2" != "$3" ]]; then
    report "$1 $2 -> $3"
  else
    printf 'current: %s %s\n' "$1" "$2"
  fi
}

pin() { awk -v n="$1" '{ sub(/\r$/, "") } $1 == n { print $2 }' .tool-versions; }

github() {
  local auth=()
  [[ -n "${GH_TOKEN:-}" ]] && auth=(-H "Authorization: Bearer $GH_TOKEN")
  curl -fsSL --proto '=https' ${auth[@]+"${auth[@]}"} -H 'Accept: application/vnd.github+json' "https://api.github.com/$1"
}

read -r -d '' PICK <<'PYTHON' || true
import datetime, json, re, sys
cutoff = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=24)
stable = re.compile(r"v?([0-9]+)\.([0-9]+)\.([0-9]+)")
data = json.load(sys.stdin)
if sys.argv[1] == "github":
    prefix = sys.argv[2] if len(sys.argv) > 2 else ""
    found = [(r["tag_name"].removeprefix(prefix), r["published_at"]) for r in data
             if not r["draft"] and not r["prerelease"] and r["published_at"]
             and r["tag_name"].startswith(prefix)]
elif sys.argv[1] == "node":
    found = [(r["version"], r["date"] + "T23:59:59Z") for r in data
             if r["lts"] and r["version"].startswith(f"v{sys.argv[2]}.")]
else:
    found = [(v, data["time"][v]) for v in data["versions"] if v in data["time"]]
ready = [(tuple(map(int, m.groups())), tag) for tag, when in found
         if (m := stable.fullmatch(tag))
         and datetime.datetime.fromisoformat(when) <= cutoff]
print(max(ready)[1].removeprefix("v"))
PYTHON

github_latest() { github "repos/$1/releases?per_page=100" | python3 -I -c "$PICK" github "${2:-}"; }
npm_latest() { curl -fsSL --proto '=https' "https://registry.npmjs.org/$1" | python3 -I -c "$PICK" npm; }
node_latest() { curl -fsSL --proto '=https' https://nodejs.org/dist/index.json | python3 -I -c "$PICK" node "$1"; }

latest=$(github_latest rust-lang/rust)
check rust "$(pin rust)" "$latest"
node=$(pin node)
latest=$(node_latest "${node%%.*}")
check node "$node" "$latest"
latest=$(github_latest EmbarkStudios/cargo-deny)
check cargo-deny "$(pin cargo-deny)" "$latest"
latest=$(github_latest nextest-rs/nextest cargo-nextest-)
check cargo-nextest "$(pin cargo-nextest)" "$latest"
latest=$(github_latest gitleaks/gitleaks)
check gitleaks "$(pin gitleaks)" "$latest"
latest=$(npm_latest prettier)
check prettier "$(pin prettier)" "$latest"
latest=$(npm_latest markdownlint-cli2)
check markdownlint-cli2 "$(pin markdownlint-cli2)" "$latest"

pins=$(python3 -I scripts/check-workflow-pins.py --list)
while read -r action version _; do
  latest=$(github_latest "$action")
  check "$action" "$version" "$latest"
done <<<"$pins"

if ((stale > 0)); then
  printf 'tools-outdated: %s pin(s) to update\n' "$stale" >&2
  exit 1
fi
printf 'tools-outdated: all pins current\n'
