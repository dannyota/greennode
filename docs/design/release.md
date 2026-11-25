# Release publishing

One `vMAJOR.MINOR.PATCH` tag releases two crates at one version. Publish
`greennode-sdk` first, then `greennode`, whose packaged manifest must require
exactly `=VERSION` of the SDK. The owner publishes the first pair from the
owner's Mac. Later releases use GitHub Actions and crates.io Trusted Publishing.
A GitHub Release does not authorize a crates.io upload. An uploaded version
cannot be replaced or deleted ([Cargo publishing]).

## Approaches

| Approach                                                              | Trade-off                                                                                      |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Publish from a tag or release job                                     | Fewer steps, but no separate owner decision and one job can write to GitHub and crates.io.     |
| Upload with `cargo publish --workspace`                               | Cargo orders dependencies, but uploads are not atomic and a retry must omit existing versions. |
| Separate release and manual publish workflows, with per-crate uploads | Use this approach. It keeps [duolane]'s gates and retries only missing uploads.                |

## Versions

The root `Cargo.toml` owns `[workspace.package].version`. Both members use
`version.workspace = true`. The tool has no SDK dependency until it first uses
SDK code. From that commit, `crates/greennode/Cargo.toml` declares
`greennode-sdk = { path = "../greennode-sdk", version = "=VERSION" }` with the
literal version. Cargo uses the path locally and the version when published
([Cargo dependencies]). The manifest gate in `make check` and CI checks version
inheritance and rejects any SDK dependency with a different path or pin. A
release also rejects a missing SDK dependency and checks the tag version.

A version-only release commit changes the root and tool `Cargo.toml` plus
`Cargo.lock`. Refresh with `cargo +1.100.0 check --workspace --all-targets`,
then repeat with `--locked`. Do not edit a member's inherited version.

## First release from the owner's Mac

1. The approved tag and immutable GitHub Release must already exist. Use Bash
   and Python 3.11 or later. Start in a clean, reviewed checkout of protected
   `master` used only for release helpers, never to compile the tag. For the
   first tag, run the full [release evidence](#release-evidence) check:

   ```bash
   TAG=v0.1.0
   git fetch origin master --tags
   TRUSTED_SCRIPTS="$PWD/scripts"
   COMMIT=$(python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" release "$TAG" origin/master)
   ```

   `release` prints the verified tag commit and fails without it. An agent may
   check the evidence, but only the owner authorizes the tag and Release. Run
   commands one at a time and stop on a failed check. After login always clean
   up in step 7.

2. In a disposable macOS checkout of `$COMMIT`, with no credentials or write
   access to the owner's trusted checkout or receipt directory, run
   [workspace packaging](#workspace-packaging). Read both file lists:

   ```bash
   cargo +1.100.0 package --list --locked --registry crates-io -p greennode-sdk
   cargo +1.100.0 package --list --locked --registry crates-io -p greennode
   ```

   Back on the owner's Mac, pass only the staged archives as data. In a fresh
   checkout of `$COMMIT` that never built the tag, with no Cargo config or
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
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" receipt "$TAG" "$COMMIT" "$STAGE" "$RECEIPT_DIR"
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
   token may exist in the crates.io form, the hidden prompt and stdin pipe,
   Cargo processes, and isolated `$CARGO_HOME/credentials.toml`. Never put it in
   command arguments, history, a checkout, GitHub secrets, logs or chat.

   ```bash
   set +x
   CARGO_HOME=$(mktemp -d)
   export CARGO_HOME
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
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" state "$GREENNODE_RELEASE_RECEIPT" --first
   cargo +1.100.0 publish --no-verify --locked --registry crates-io \
     --package greennode-sdk --config "$PROVIDER" --config net.retry=0
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" state "$GREENNODE_RELEASE_RECEIPT" --wait greennode-sdk
   ```

6. **Owner only.** With the SDK approved and the tool absent, publish the tool.
   Run the wait check even on a Cargo error. Confirm both versions and owners.

   ```bash
   python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" state "$GREENNODE_RELEASE_RECEIPT"
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
   unset CARGO_HOME
   ```

8. **Owner only.** Set up both Trusted Publishers after confirming the pair.
   Follow [one-time setup](#one-time-setup).

### Resume the first release

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

## Release workflow

`.github/workflows/release.yml` runs on `v*` tag pushes. Validate the tag,
workspace manifests, lockfile, dependency policy and workspace tests using
pinned Rust and CI tools. Build only the tool with
`cargo build --locked --release --package greennode --target TARGET`. Check its
version against the tag and smoke-test the extracted Windows archive.

Use the built candidate's hidden `gen-docs <dir> --command-inventory <path>`
export. Block the Release unless the export exactly matches
`crates/greennode/tests/parity/shipped-commands.json` at the tag. The
[CLI design](cli.md) owns the sorted paths, built-ins and version/help aliases.
Require the preceding published tag's list to be a subset, even if crates.io
publication was incomplete. On the first release, check equality only.

| Target                      | Archive                         |
| --------------------------- | ------------------------------- |
| `aarch64-apple-darwin`      | `greennode-darwin-arm64.tar.gz` |
| `x86_64-unknown-linux-gnu`  | `greennode-linux-amd64.tar.gz`  |
| `aarch64-unknown-linux-gnu` | `greennode-linux-arm64.tar.gz`  |
| `x86_64-pc-windows-msvc`    | `greennode-windows-amd64.zip`   |

Each archive holds the executable, `LICENSE`, and
`licenses/go-jmespath-LICENSE`; the release job extracts each archive and checks
the two license files match the tagged sources and the executable matches its
verified build artifact. Upload the archives, their `greennode-PLATFORM.sha256`
files, and the `crate-checksums.json` an unprivileged Ubuntu 24.04 job builds
from the staged packages as artifacts. Set `retention-days: 90` on every release
run artifact that backs those assets. Stage all nine assets on a draft, then
publish it to lock them and the tag ([GitHub immutable releases]). Only the
Release job has `contents: write`; no release job has OIDC or registry write
credentials.

### Release evidence

Require a published immutable GitHub Release, tag commit in protected `master`'s
history, and a successful `release.yml` tag-push run for the exact tag and
commit. Compare each expected uploaded asset's SHA-256 with the corresponding
downloaded build artifact from that run. No missing artifacts, expired artifacts
or digest mismatches pass. Bootstrap, dispatch and recovery must run while those
artifacts exist; [GitHub artifact retention] is bounded. After expiration this
tag cannot be published or completed. Do not rerun the build for new evidence.
Cut a new shared version and tag and publish both crates at the new version. A
lone SDK version stays published. The immutable Release receipt preserves the
checksums but cannot replace the exact-run evidence.

### Workspace packaging

Use a clean tagged checkout, no Cargo overrides or registry credentials, and new
empty directories for the target, build and staging paths. Cargo verifies both
packages in workspace [dry runs]. The dry run itself writes the archives to the
build directory's `package/tmp-crate/` ([Cargo package source]). Only
`cargo package` without `--list` copies archives to `target/package/`; never
read that path for this receipt.

```bash
BUILD_ROOT=$(mktemp -d)
STAGE=$(mktemp -d)
CARGO_TARGET_DIR="$BUILD_ROOT/target" CARGO_BUILD_BUILD_DIR="$BUILD_ROOT/build" \
  cargo +1.100.0 publish --workspace --dry-run --locked --registry crates-io
```

Only after success, copy the two regular files unchanged into `$STAGE`. Refuse
symlinks and unexpected `.crate` entries under `package/tmp-crate/`.

```bash
for NAME in greennode-sdk greennode; do
  FILE="$BUILD_ROOT/build/package/tmp-crate/$NAME-${TAG#v}.crate"
  test -f "$FILE" && test ! -L "$FILE" && cp "$FILE" "$STAGE/" || exit 1
done
```

Hash the staged bytes, not repacked archives. Both canonical `Cargo.toml` files
must carry the expected name and tag version. Require tracked, in-checkout
regular README and LICENSE inputs, including the root README for the tool and
local README for the SDK. Check both archive lists. The tool archive must
contain a normal crates.io dependency on `greennode-sdk` at `=VERSION`, with no
path or alternate source. Its packaged `Cargo.lock` must bind that SDK version
and approved SHA-256. The receipt records tag, commit, toolchain, and both
names, versions, filenames and hashes.

## Manual publish workflow

`.github/workflows/publish-crates.yml` accepts `workflow_dispatch` only in
`dannyota/greennode` from `refs/heads/master`. Require a string `tag` with no
leading zeros and a boolean `publish` defaulting to `false`. Both modes use a
repository-wide concurrency group with `cancel-in-progress: false`. Publishing
requires protected `master`, `publish: true` and a fresh dispatch's first
attempt, not a rerun. Default permissions are `{}`.

### Preflight

1. Give `verify` only `contents: read` and `actions: read`. No environment, OIDC
   or token. Use trusted helpers from the protected dispatch commit. Never run
   tagged code here. Check release evidence, resolving the tag in the dispatch
   commit's history. Inspect the tagged manifests and tree. Reject tracked
   symlinks or Cargo config. Require `dannyota` as a user owner of each crate;
   in preflight-only mode only, report an unregistered crate.
2. Validate the immutable Release receipt and reconcile both crates. Record the
   approved tag, commit, pinned numeric toolchain, release run and receipt
   digest as `verify` outputs. Show hashes and registry states in the approval
   summary before any tagged build runs.
3. On a separate unprivileged Ubuntu 24.04 runner, run workspace packaging as
   the final preflight step. Compare the candidate hashes to those already
   recorded. No later step on that runner produces trusted evidence. Pass only
   success, not build files or outputs, to `publish`. Cargo can warn for an
   existing version in a dry run; the registry gate decides whether it passes.

### Publish job

1. Give `publish` `contents: read`, `actions: read` and `id-token: write`.
   Require the `crates-io` environment, its owner reviewer to still be
   configured, and a recorded owner approval for this run and environment. Check
   protected `master`, the exact tag commit and safe checkout again.
   Independently reload the receipt from the immutable Release and compare it
   with `verify`'s expected identity and digest. Require the exact-run artifacts
   still exist. Use a fresh checkout that did not compile the tag, with helpers
   only from the protected dispatch commit.
2. Reconcile both crates before exchange. If both match, do not mint a token.
   Otherwise exchange once with the pinned [`rust-lang/crates-io-auth-action`].
   crates.io [binds all matching crate IDs] to one token and [checks membership
   at upload], so the identical configurations need one exchange, not two. The
   action revokes the token in its post step; crates.io expires it after 30
   minutes ([crates.io guide source]).
3. Publish only missing versions in SDK-first order. Use a fresh empty Cargo
   home, pinned Rust and the same guarded `cargo publish` commands as on the
   Mac. No compilation or restored build cache. Give `CARGO_REGISTRY_TOKEN` only
   to Cargo and its provider for uploads, never workflow-wide or in files.

### Approve the bytes Cargo uploads

`cargo publish` packages again. Cargo 1.100 computes SHA-256 of the actual
archive and asks its credential provider for the `publish` operation with the
name, version and checksum before uploading ([Cargo credential protocol], [Cargo
1.100 publish source]). Set `registry.credential-provider` explicitly to the
checksum-checking helper. Give a token for `publish` only if the registry is
crates.io and the name, version and checksum match the current upload and
validated receipt. Allow a `read` credential only for that registry. Allow no
other write. Set `cache: "never"` and `operation_independent: false`. No
fallback provider or `--token`. Read `CARGO_REGISTRY_TOKEN` in CI ([Cargo token
config]) or the isolated [Cargo login] file on the Mac. Send the token only
through the provider protocol. Refuse different bytes before upload.

## Registry gate and recovery

Read the [version API] at `https://crates.io/api/v1/crates/CRATE/VERSION` and
the [Cargo sparse index] at `https://index.crates.io/gr/ee/CRATE`. Use bounded,
unauthenticated HTTPS reads with fresh revalidation and a generic user agent.
Match exact names and versions. Compare API `version.checksum` and
`version.yanked` with index `cksum`, `yanked` and the approved receipt.

| Observation                                                              | Decision                                  |
| ------------------------------------------------------------------------ | ----------------------------------------- |
| Valid API not-found, no exact index version, no unresolved upload        | Absent. Upload if its dependencies match. |
| Both sources agree on the exact version, approved SHA-256 and not yanked | Present and approved. Skip upload.        |
| Yank, mismatch, malformed response, disagreement or unresolved upload    | Stop.                                     |

Recheck both before each write. The tool requires an approved SDK in the index,
not just an upload response. After each upload, poll both endpoints for a
bounded interval. Timeout, duplication, cancellation and index delay may mean
the upload landed. If still unconfirmed, record crate, version, checksum and
`unknown` in the summary. Stop without retrying or proceeding to the tool. A
stopped job that started Cargo without confirming the upload is also unknown.

The owner checks the preceding attempt before dispatching again. If both readers
still say absent after an ambiguous write, get confirmation from crates.io that
the write did not land and is no longer in flight. A red job, old 404 or elapsed
time does not clear it. While the release run artifacts still exist, dispatch
the same tag for a new approval and publish only missing crates. A yanked
version is not absent. Investigate yanks and mismatches before any write. Never
automatically unyank or retag. Correct an accidental yank only after verifying
the approved bytes. A version with different bytes needs a new shared tag.

## One-time setup

- Enable immutable GitHub Releases, allow 90-day artifact retention, and protect
  `master`. Create environment `crates-io`, restrict it to `master`, require the
  owner and disallow bypass. Allow self-review while the owner is the only
  reviewer. Allow the pinned workflow actions, including
  `softprops/action-gh-release` and `rust-lang/crates-io-auth-action` at
  `3268eb413133e2960fa4ae0e963ca76380c54cd9`.
- Only the owner adds both crates' Trusted Publishers after initial publication
  ([RFC 3691], [crates.io guide source]). In each crate's Settings → Trusted
  Publishing, select GitHub. Set owner `dannyota`, repository `greennode`,
  workflow `publish-crates.yml` and environment `crates-io`.
- Never put a crates.io API token in GitHub secrets. The bootstrap token stays
  local; later jobs use OIDC.

## Implementation boundary

- Port duolane's build matrix, checks, smoke test, release notes, pins, assets
  and safety gates, including exact-run checks and protected owner approval.
- Add workspace and archive checks, shipped-command comparison, staging and
  immutable receipt, checksum credential provider and per-crate recovery.
- Test clean targets, bad manifests, changed receipts, wrong hashes, token
  redaction, registry disagreement, ambiguous uploads and SDK-only recovery.

## Unverified

- The rendered crates.io guide was unavailable. Its official source and the
  token form source confirm the fields. The owner checks the deployed forms.
- Before any release, test two toy crates on a loopback registry with Cargo
  1.100.0 on macOS and Ubuntu. From empty paths, check dry-run staging, equality
  with the individual publish operation's checksums before and after the SDK is
  indexed, and no upload on mismatch. Test both secret sources. Cargo rebuilds
  the packaged lockfile, so byte equality is not guaranteed. A mismatch blocks
  release; do not weaken the gate.
- The GitHub environment and Trusted Publishers remain untested. The owner
  checks preflight and approval before the first Actions publish.

[duolane]:
  https://github.com/dannyota/duolane/blob/master/docs/design/release.md
[Cargo publishing]: https://doc.rust-lang.org/cargo/reference/publishing.html
[Cargo dependencies]:
  https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations
[dry runs]:
  https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/#cargo-adds-native-support-for-workspace-publishing
[Token scopes]:
  https://rust-lang.github.io/rfcs/2947-crates-io-token-scopes.html
[Cargo login]: https://doc.rust-lang.org/cargo/commands/cargo-login.html
[Cargo logout]: https://doc.rust-lang.org/cargo/commands/cargo-logout.html
[Cargo token config]:
  https://doc.rust-lang.org/cargo/reference/config.html#registrytoken
[Cargo credential protocol]:
  https://doc.rust-lang.org/cargo/reference/credential-provider-protocol.html#publish-request
[Cargo 1.100 publish source]:
  https://github.com/rust-lang/cargo/blob/rust-1.100.0/src/ops/registry/cargo_publish.rs
[Cargo package source]:
  https://github.com/rust-lang/cargo/blob/rust-1.100.0/src/ops/cargo_package/mod.rs#L167-L251
[Cargo sparse index]:
  https://doc.rust-lang.org/cargo/reference/registry-index.html
[GitHub immutable releases]:
  https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases
[GitHub artifact retention]:
  https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts
[RFC 3691]:
  https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html
[crates.io guide source]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/svelte/src/routes/docs/trusted-publishing/+page.svelte
[binds all matching crate IDs]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/src/controllers/trustpub/tokens/exchange/mod.rs#L152-L245
[checks membership at upload]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/src/controllers/krate/publish.rs#L211-L230
[version API]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/src/controllers/version/metadata.rs#L22-L46
[`rust-lang/crates-io-auth-action`]:
  https://github.com/rust-lang/crates-io-auth-action/tree/3268eb413133e2960fa4ae0e963ca76380c54cd9
