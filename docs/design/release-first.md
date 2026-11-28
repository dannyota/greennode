# Releases from the owner's Mac

Until the CI publish workflow is proven, every `0.x.y` release of
`greennode-sdk` and `greennode` is published from the owner's Mac with
`cargo publish`; Trusted Publishing also needs each crate to exist first. The
owner decides each release and declares `1.0.0` once every baseline operation
ships and the owner judges it stable. The [release design](release.md) owns
versions, packaging and recovery.

1. The release commit is on protected `master` with the `ci` check green, and
   its version commit has landed as [versions](release.md#versions) describes.
   With the owner's approval for this release, the primary creates the signed
   annotated tag `vX.Y.Z` on that commit and pushes it. Use Bash, Python 3.11 or
   later, `jq`, and `gh` as `dannyota`, from a clean checkout of protected
   `master` used only for release helpers, never to compile the tag:

   ```bash
   set +x
   export TAG=vX.Y.Z TRUSTED_SCRIPTS="$PWD/scripts"
   git fetch origin master --tags
   COMMIT=$(git rev-parse "refs/tags/$TAG^{commit}")
   git merge-base --is-ancestor "$COMMIT" origin/master
   ```

   The tag push also runs the release workflow, which may build the GitHub
   Release; crates.io publication below does not depend on it. Later steps run
   commands one at a time without abort-on-error, so the wait checks and step 7
   cleanup always run. After login always clean up in step 7.

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
   `--first` requires both crate names to be unregistered; omit it after the
   first release, when both names must exist and both new versions be absent.

3. **Owner only.** Confirm the tag and receipt again after the dry run. In
   crates.io Settings → API Tokens, create a short-lived token with
   `publish-new` for the first release, or `publish-update` after it, and exact
   patterns `greennode-sdk` and `greennode`. No wildcard, update, owner or yank
   scope ([Token scopes]).

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

8. **Owner only.** After the first release, set up both Trusted Publishers once
   the CI publish workflow is proven. Follow
   [one-time setup](release.md#one-time-setup).

## Resume a release

Repeat the step 1 checks. Use the saved Mac receipt and another fresh checkout
that never built the tag. Run `state` without `--first`. If both match, do not
upload. If the SDK matches and the tool name is unregistered, create a new
`publish-new` token scoped only to `greennode`. Repeat steps 4, 6 and 7 and set
`PROVIDER` as in step 5 without uploading the SDK. Do not retry an unknown
upload. Stop without the original receipt or exact-run evidence. If the evidence
expired and only the SDK exists, cut a new shared tag. Omit `--first` for that
bootstrap, check SDK ownership and both new versions absent, and use separate
tokens: `publish-update` scoped to the SDK, then `publish-new` scoped to the
tool. Never widen one token or automatically yank the old SDK.

[Cargo logout]: https://doc.rust-lang.org/cargo/commands/cargo-logout.html
[Token scopes]:
  https://rust-lang.github.io/rfcs/2947-crates-io-token-scopes.html
