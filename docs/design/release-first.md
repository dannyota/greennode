# Releases from the owner's Mac

The Mac is the active crates.io publishing path until the owner approves the
[CI cutover](release.md#ci-cutover). Each upload uses `cargo publish` and the
owner's existing default Cargo login. The [release design](release.md) owns
versions, packaging and the registry gate.

The owner chose the existing login over minting a short-lived scoped token for
each release. The existing token has broader permissions. Only the owner may
narrow or revoke it. Agents never open, read, print or copy the token or its
credential file.

## 1. Verify the commit and tag

Use Bash, Python 3.11 or later and `gh` as `dannyota`, with tracing and `gh`
debug output off. Use release helpers from a clean, reviewed checkout of
protected `master` that never builds the tag. Keep that checkout separate from
the build, publish and receipt directories.

Before the first tag, remove the claim that nothing is on crates.io from the
root `README.md`. Replace the claim that nothing is implemented in
`crates/greennode-sdk/README.md` with the SDK capabilities. Cargo packages those
two READMEs with the tool and SDK respectively. Complete the
[Mac packaging proof](release.md#packaging-proof) and the
[GitHub Release setup](release.md#github-release-setup) before tagging.

1. Select the exact approved release commit after the
   [version commit](release.md#versions) lands. Fetch `origin/master`. Require
   branch protection, `git merge-base --is-ancestor "$COMMIT" origin/master`,
   and a successful `ci` job from `.github/workflows/ci.yml` on the `master`
   push for that exact SHA. Check the SHA on the run itself. The PR's head check
   alone is insufficient.
2. Obtain the owner's approval for the signed annotated tag `vX.Y.Z`, the
   resulting GitHub Release and the two crates.io uploads at that commit. Create
   the tag locally, verify its signature against the authorized signer with
   `git verify-tag`, and require `git rev-parse "refs/tags/$TAG^{commit}"` to
   equal `$COMMIT`. Only then push the tag. The push starts
   `.github/workflows/release.yml`, which publishes an immutable GitHub Release
   and its assets when its checks pass.
3. Verify that the remote tag object equals the signed local tag object. Stop on
   a failed identity, signature, ancestry or check result. The Mac upload uses
   the exact-SHA `ci` check and the signed tag. The GitHub Release and tag-push
   workflow artifacts are required for CI publishing.

## 2. Check the tree, packages and inventory

Before any credentialed Cargo command, save the NUL-delimited output of
`git ls-tree -r -z --full-tree "$COMMIT"` to `TREE` and run
`python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" tree "$TREE"`. Run
`check-crate-release.py manifest --release --tag "$TAG"` from the root of the
fresh checkout of that SHA, using the trusted copy of the script. Use only clean
checkouts with that exact HEAD. Refuse Cargo configuration in the checkout, in
any ancestor, or in the Cargo home used for release commands. The publish
checkout must never have built the tag.

Use two disposable macOS checkouts without credentials or write access to the
trusted checkout, publish checkout, or receipt directory. In the first, run
[workspace packaging](release.md#workspace-packaging) and inspect both package
file lists. Require the expected sources, LICENSE and READMEs and exclude any
private or untracked files. In the second, repeat the dry run with verification.
Transfer only the archives as data and require the two pairs to match by
SHA-256. Stage the `--no-verify` pair unchanged.

In a credential-free checkout, build the pinned tool with
`cargo +1.100.0 build --locked --release --package greennode` and export its
`__command-inventory` with a temporary home and cleared environment. Compare
that export to `crates/greennode/tests/parity/shipped-commands.json` at the tag
using `check-crate-release.py shipped CANDIDATE COMMITTED PREVIOUS`. On the
first release, pass `--first` in place of `PREVIOUS`. For a later release, use
the committed inventory at the verified signed tag for the most recent crates.io
`greennode` version, selected by
`python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" previous "$TAG"`. Verify
the previous tag's signature and protected `master` ancestry. Stop if the
registry lookup or the inventory check fails. The GitHub Release workflow runs a
separate comparison against the previous published GitHub Release.

Before any upload, keep the staged archives in an owner-only directory next to
the Mac receipt, outside all checkouts. Use a new directory for the tag, such as
`~/.local/state/greennode/releases/$TAG`, with an `archives` subdirectory
holding only the two approved `.crate` files. Use `umask 077` when creating
these directories and files. With `TRUSTED_SCRIPTS` pointing to the trusted
helpers and `RECEIPT_DIR` pointing to that directory, create the receipt from
the saved archives.

```bash
python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" receipt "$TAG" \
  "$RECEIPT_DIR/archives" "$RECEIPT_DIR/crate-checksums.json" \
  --commit "$COMMIT" --toolchain 1.100.0
```

Keep the receipt, both archives, the inventory check inputs, and the local
upload record for recovery. Before the first upload and on every resume,
recreate a receipt from the saved archives in a new scratch directory using the
verified tag and commit and the pinned toolchain. Require the recreated receipt
to match the saved receipt byte for byte. A missing receipt or archive, a failed
comparison, or a failed checkout or tree check stops before any credentialed
Cargo command. Keep the approved receipt and archives unchanged.

## 3. Check the registry and publish the SDK

Run `check-crate-release.py state RECEIPT` against the saved receipt and require
both versions to report `absent`. Pass `--first` only for the first release,
when the two crate names must also be unregistered. For later releases, omit
`--first` and check that both existing names list `dannyota` as a user owner
using
`python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" owners greennode-sdk greennode`.
For an expected unregistered name, insert `--unregistered` before its name and
check it separately. Require the same state and ownership checks immediately
before starting the SDK upload.

Use a plain Bash session in the fresh publish checkout. Refuse a set
`CARGO_REGISTRY_TOKEN` or any `CARGO_REGISTRIES_*_TOKEN`, even an empty one.
Check variable names with the following gate before each upload and stop on
failure.

```bash
(
  while IFS= read -r NAME; do
    case "$NAME" in
      CARGO_REGISTRY_TOKEN|CARGO_REGISTRIES_*_TOKEN) exit 1 ;;
    esac
  done < <(compgen -v)
)
```

Leave `CARGO_HOME` unset and use the owner's default Cargo home. Use the
built-in token provider with the credentials stored by [Cargo login]. Refuse
registry, source, provider, build or toolchain overrides. Leave the owner's
login in place on success, failure or interruption. No Mac upload starts before
all identity, ancestry, receipt, inventory and registry checks pass and the
owner has approved publication.

Only one publish attempt may run at a time. Immediately before starting Cargo,
record the crate, version and approved checksum as `unknown` in the durable Mac
upload record. Keep that entry until reconciliation confirms the result, even if
the shell exits. Run the SDK upload with the pinned toolchain.

```bash
cargo +1.100.0 publish --no-verify --locked --registry crates-io \
  --package greennode-sdk --config net.retry=0
```

Always run `check-crate-release.py state RECEIPT --wait greennode-sdk` after
Cargo returns, including on a Cargo error. Require `present-and-approved` for
the SDK in both registry readers and confirm its owner before proceeding. Cargo
repackages when it publishes. On this path the local receipt approves the
dry-run bytes and reconciliation detects any difference after an upload. The
[CI checksum provider](release.md#approve-the-bytes-cargo-uploads) checks actual
bytes before upload on the CI path.

## 4. Publish the tool and confirm the pair

Recheck the SDK owner and require `present-and-approved` for the SDK and
`absent` for the tool using `state` without `--first`. For the first release,
require the tool name to remain unregistered. For later releases, require
`dannyota` to remain a user owner of the tool name. Any unresolved earlier
attempt to upload the tool blocks a new attempt. Recheck the credential
environment, record the pending tool upload as in step 3, and run:

```bash
cargo +1.100.0 publish --no-verify --locked --registry crates-io \
  --package greennode --config net.retry=0
```

Always run `check-crate-release.py state RECEIPT --wait greennode` after Cargo
returns. Require `present-and-approved` for both crates and `dannyota` as a user
owner of both names. Record the confirmed results against the saved receipt.

## Resume a release

Get the owner's approval for the retry. Repeat the commit, signature, remote
tag, local tree and saved receipt checks in steps 1 and 2 using a new publish
checkout. Reuse the saved inventory inputs and recheck the registry and crate
ownership. Recovery uses the Mac record and archived packages, with no
requirement for a GitHub Release, its receipt, or its workflow artifacts.

Run `state` without `--first` to reconcile the pair. If both versions are
approved, check both owners and end the attempt. If the SDK is approved and only
the tool is confirmed absent, run step 4. This covers both the first release,
when the tool name is still unregistered, and later releases, when the owner
must already own the tool name. If both uploads are confirmed absent, repeat
steps 3 and 4 with the original first or later checks. A tool version present
without its approved SDK stops recovery.

If an upload may have started, a subsequent `absent` is insufficient. Require
both registry readers to confirm the approved bytes or get confirmation from
crates.io that the attempted upload did not land and is no longer in flight.
Preserve `unknown` until then. Stop on missing local records, invalid evidence,
yanks, mismatches, or unresolved uploads. Never overwrite the saved receipt.

## Standalone checks

The `tree` command checks the bounded tagged tree independently of release and
run evidence. `owners` checks each requested name against crates.io, and
`previous` prints the highest published stable tool tag, including yanked
versions, after checking the index against the version API. Registry lookups are
limited to 30 seconds and 1 MiB each.

[Cargo login]: https://doc.rust-lang.org/cargo/commands/cargo-login.html
