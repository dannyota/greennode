# First release from the owner's Mac

The first `greennode-sdk` and `greennode` versions are published from the
owner's Mac, because Trusted Publishing needs each crate to exist first. The
[release design](release.md) owns versions, the workflows, evidence, packaging
and recovery.

1. The approved tag and immutable GitHub Release must already exist. Use Bash,
   Python 3.11 or later, `jq`, and `gh` as `dannyota`. Start in a clean,
   reviewed checkout of protected `master` used only for release helpers, never
   to compile the tag. Gather the
   [release evidence](release.md#release-evidence) into a fresh owner-only
   directory per attempt and check it in a separate `bash -euo pipefail`
   process, so any failure stops it; `COMMIT` is set only when it succeeds.
   Tracing and `gh` debug output are off, and `gh` gets the owner's token per
   command. An owners file is `null` only for crates.io's exact not-found body.

   ```bash
   set +x
   export TAG=v0.1.0 REPO=dannyota/greennode TRUSTED_SCRIPTS="$PWD/scripts"
   mkdir -p "$HOME/.local/state/greennode/evidence"
   export EVIDENCE=$(mktemp -d "$HOME/.local/state/greennode/evidence/$TAG.XXXXXX")
   unset COMMIT
   if bash -euo pipefail <<'EOF'; then COMMIT=$(< "$EVIDENCE/commit"); fi
   set +x
   umask 077
   unset GH_DEBUG
   gh() {
     local token
     token=$(command gh auth token -u dannyota) && test -n "$token" || return 1
     GH_TOKEN="$token" command gh "$@"
   }
   git fetch origin master --tags
   COMMIT=$(git rev-parse "refs/tags/$TAG^{commit}")
   git merge-base --is-ancestor "$COMMIT" origin/master
   gh release view "$TAG" --repo "$REPO" \
     --json tagName,isDraft,publishedAt,isImmutable,assets > "$EVIDENCE/release.json"
   gh api --paginate --slurp \
     "repos/$REPO/actions/workflows/release.yml/runs?event=push&head_sha=$COMMIT" |
     jq --arg tag "$TAG" '[.[].workflow_runs[] | select(.head_branch == $tag) |
       {databaseId: .id, headBranch: .head_branch, headSha: .head_sha, path,
        conclusion, event}]' > "$EVIDENCE/runs.json"
   RUN=$(jq -er 'if length == 1 then .[0].databaseId else error("runs") end' \
     "$EVIDENCE/runs.json")
   for NAME in darwin-arm64 linux-amd64 linux-arm64 windows-amd64 crate-checksums; do
     gh run download "$RUN" --repo "$REPO" --name "$NAME" --dir "$EVIDENCE/artifacts/$NAME"
   done
   git ls-tree -r -z --full-tree "$COMMIT" > "$EVIDENCE/tree"
   for PAIR in greennode-sdk:sdk greennode:tool; do
     CRATE=${PAIR%%:*}
     OUT="$EVIDENCE/${PAIR##*:}-owners.json"
     CODE=$(curl -sS -A greennode-release -o "$OUT.body" -w '%{http_code}' \
       "https://crates.io/api/v1/crates/$CRATE/owners")
     case "$CODE" in
       200) jq -e '.users | type == "array"' "$OUT.body" > /dev/null
            mv "$OUT.body" "$OUT" ;;
       404) jq -e --arg d "crate \`$CRATE\` does not exist" \
              '. == {errors: [{detail: $d}]}' "$OUT.body" > /dev/null
            printf 'null\n' > "$OUT" ;;
       *) exit 1 ;;
     esac
   done
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" evidence "$TAG" "$COMMIT" \
     "$EVIDENCE/release.json" "$EVIDENCE/runs.json" "$EVIDENCE/sdk-owners.json" \
     "$EVIDENCE/tool-owners.json" "$EVIDENCE/artifacts" "$EVIDENCE/tree"
   printf '%s\n' "$COMMIT" > "$EVIDENCE/commit"
   EOF
   test -n "${COMMIT:-}"
   ```

   An agent may check the evidence, but only the owner authorizes the tag and
   Release. Later steps run commands one at a time without abort-on-error, so
   the wait checks and step 7 cleanup always run. After login always clean up in
   step 7.

2. In a disposable macOS checkout of `$COMMIT`, with no credentials or write
   access to the owner's trusted checkout or receipt directory, run
   [workspace packaging](release.md#workspace-packaging). Read both file lists:

   ```bash
   cargo +1.100.0 package --list --locked --registry crates-io -p greennode-sdk
   cargo +1.100.0 package --list --locked --registry crates-io -p greennode
   ```

   In a second disposable checkout, run the same dry run without `--no-verify`.
   Back on the owner's Mac, pass only both archive pairs as data and stop unless
   each pair hashes the same; the `--no-verify` pair is the staged one. In a
   fresh checkout of `$COMMIT` that never built the tag, with no Cargo config or
   overrides, use the helpers at `$TRUSTED_SCRIPTS`. Check LICENSE, README,
   sources, and no private or untracked files. Record the approved Mac hashes
   and archives before creating a token. `STAGE` is the transferred directory.

   ```bash
   test "$(git rev-parse HEAD)" = "$COMMIT"
   umask 077
   RECEIPT_DIR="$HOME/.local/state/greennode/releases/$TAG"
   mkdir -p "$(dirname "$RECEIPT_DIR")"
   mkdir "$RECEIPT_DIR"
   export GREENNODE_RELEASE_RECEIPT="$RECEIPT_DIR/crate-checksums.json"
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" receipt "$TAG" "$STAGE" \
     "$GREENNODE_RELEASE_RECEIPT" --commit "$COMMIT" --toolchain 1.100.0
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" state "$GREENNODE_RELEASE_RECEIPT" --first
   ```

   The owner keeps this receipt and its archives. Do not overwrite them on
   recovery. The Mac receipt approves Mac bytes, not the Linux Release bytes.
   `--first` requires both crate names to be unregistered.

3. **Owner only.** Confirm the release evidence again after the dry run. In
   crates.io Settings → API Tokens, create a short-lived token with
   `publish-new` only and exact patterns `greennode-sdk` and `greennode`. No
   wildcard, update, owner or yank scope ([Token scopes]).

4. **Owner only.** In the fresh checkout, log in with the built-in provider. The
   token lives only in the form, the hidden prompt, Cargo, and the isolated
   `$CARGO_HOME/credentials.toml`, never in arguments, history, a checkout,
   secrets, logs or chat. The helper reads that file only when
   `GREENNODE_RELEASE_CARGO_HOME` equals `CARGO_HOME`, it is not `~/.cargo`, and
   the file is an owner-only regular file.

   ```bash
   set +x
   CARGO_HOME=$(mktemp -d)
   export CARGO_HOME GREENNODE_RELEASE_CARGO_HOME="$CARGO_HOME"
   IFS= read -r -s -p 'crates.io API token: ' release_token
   printf '\n'
   printf '%s\n' "$release_token" | cargo +1.100.0 login --registry crates-io \
     --config 'registry.credential-provider="cargo:token"'
   unset release_token
   ```

5. **Owner only.** Recheck both names, then publish the SDK. Run the wait check
   even if Cargo errors. Require the SDK's approved checksum, non-yanked index
   record and owner before publishing the tool.

   ```bash
   PROVIDER="registry.credential-provider=['python3','-I','$TRUSTED_SCRIPTS/check-crate-publish-credential.py']"
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" state "$GREENNODE_RELEASE_RECEIPT" --first &&
     cargo +1.100.0 publish --no-verify --locked --registry crates-io \
       --package greennode-sdk --config "$PROVIDER" --config net.retry=0
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" state "$GREENNODE_RELEASE_RECEIPT" --wait greennode-sdk
   ```

6. **Owner only.** With the SDK approved and the tool absent, publish the tool.
   Cargo starts only after a fresh SDK owners fetch lists user `dannyota` and
   the state check passes. Run the wait check even on a Cargo error. Confirm
   both versions, then both crates' owners.

   ```bash
   OWNERS=$(curl -fsS -A greennode-release https://crates.io/api/v1/crates/greennode-sdk/owners) &&
     jq -e '.users | any(.kind == "user" and .login == "dannyota")' <<< "$OWNERS" &&
     STATE=$(python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" state "$GREENNODE_RELEASE_RECEIPT") &&
     test "$STATE" = $'greennode-sdk: present-and-approved\ngreennode: absent' &&
     cargo +1.100.0 publish --no-verify --locked --registry crates-io \
       --package greennode --config "$PROVIDER" --config net.retry=0
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" state "$GREENNODE_RELEASE_RECEIPT" --wait greennode
   ```

7. **Owner only.** Even after interruption, log out, remove the isolated Cargo
   home, and revoke the token in crates.io Settings → API Tokens. [Cargo logout]
   removes the local copy, not the server token.

   ```bash
   cargo +1.100.0 logout --registry crates-io \
     --config 'registry.credential-provider="cargo:token"'
   rm -rf -- "$CARGO_HOME"
   unset CARGO_HOME GREENNODE_RELEASE_CARGO_HOME
   ```

8. **Owner only.** Set up both Trusted Publishers after confirming the pair.
   Follow [one-time setup](release.md#one-time-setup).

## Resume the first release

Repeat the release evidence check. Use the saved Mac receipt and another fresh
checkout that never built the tag. Run `state` without `--first`. If both match,
do not upload. If the SDK matches and the tool name is unregistered, create a
new `publish-new` token scoped only to `greennode`. Repeat steps 4, 6 and 7 and
set `PROVIDER` as in step 5 without uploading the SDK. Do not retry an unknown
upload. Stop without the original receipt or exact-run evidence. If the evidence
expired and only the SDK exists, cut a new shared tag. Omit `--first` for that
bootstrap, check SDK ownership and both new versions absent, and use separate
tokens: `publish-update` scoped to the SDK, then `publish-new` scoped to the
tool. Never widen one token or automatically yank the old SDK.

[Cargo logout]: https://doc.rust-lang.org/cargo/commands/cargo-logout.html
[Token scopes]:
  https://rust-lang.github.io/rfcs/2947-crates-io-token-scopes.html
