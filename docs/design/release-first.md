# Releases from the owner's Mac

The Mac is the active crates.io publishing path until the owner approves the
[CI cutover](release.md#ci-cutover). The owner chose plain `cargo publish` with
an existing default Cargo login. Only the owner may narrow or revoke that token.
Agents never open, read, print or copy the token or its credential file.

The Mac checks the actual uploaded bytes immediately after each upload. Plain
Cargo publishing trades away pre-upload approval of those bytes. Cargo may cache
a credential from the read before packaging and reuse it for publish, so a
provider in front of `cargo:token` cannot enforce this check just by falling
through on reads. Trusted Publishing restores [pre-upload checksum approval]
when CI takes over. [Cargo providers] and [Cargo auth source] describe the
provider order and credential cache.

## 1. Verify the commit and tag

Use Bash, Python 3.11 or later and `gh` as `dannyota`, with tracing and `gh`
debug output off. Use release helpers from a clean, reviewed checkout of
protected `master` that never builds the tag. Keep that checkout separate from
the build, publish and receipt directories. Complete the
[Mac packaging proof](release.md#packaging-proof) and the
[GitHub Release setup](release.md#github-release-setup) before tagging.

1. Select the exact approved release commit after the
   [version commit](release.md#versions) lands. Fetch `origin/master`. Require
   branch protection, `git merge-base --is-ancestor "$COMMIT" origin/master`,
   and a successful `ci` job from `.github/workflows/ci.yml` on the `master`
   push for that exact SHA. Check the SHA on the run itself.
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
fresh publish checkout of that SHA, using the trusted copy of the script.
Require exact HEAD and a clean checkout, including no untracked or ignored
files. Refuse Cargo configuration in the checkout and its ancestors, and
`config` or `config.toml` in the default Cargo home. Check filenames without
opening the credential files. The publish checkout never compiles the tag.

First run the [workspace dry run](release.md#workspace-packaging) with
verification in a disposable macOS checkout without credentials or write access
to the trusted checkout, publish checkout or receipt directory. Transfer its
archives as data. Build the pinned tool in a credential-free checkout with
`cargo +1.100.0 build --locked --release --package greennode` and export
`__command-inventory` with a temporary home and cleared environment.

Now use the clean publish checkout for the `--locked --no-verify` workspace dry
run and both package file lists. Give only these dry-run commands a new empty
Cargo home, and use new build and staging directories outside the checkout. Use
the same tagged source and Cargo 1.100.0 for staging and the real uploads.
Require the expected sources, LICENSE and READMEs in each list. Compare both
pairs of archives by SHA-256 and stage the identical `--no-verify` pair
unchanged. Start publication from that same checkout immediately after staging,
receipt validation and the registry checks. Make no changes to the checkout,
toolchain or packaging settings between the dry run and either upload.

Compare the inventory export to the tag's committed
`crates/greennode/tests/parity/shipped-commands.json` with
`check-crate-release.py shipped CANDIDATE COMMITTED PREVIOUS`. When the tool
name is unregistered, pass `--first` in place of `PREVIOUS`. When it exists, run
`python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" previous "$TAG"`, verify
the returned tag's signature and protected `master` ancestry, then use its
committed inventory. Stop if any check fails. The GitHub Release workflow
compares against the previous published GitHub Release independently.

Keep the two staged `.crate` files in an `archives` subdirectory next to the Mac
receipt, outside all checkouts. Use a new owner-only directory per tag, such as
`~/.local/state/greennode/releases/$TAG`, and `umask 077`. With
`TRUSTED_SCRIPTS` pointing to the trusted helpers and `RECEIPT_DIR` pointing to
the release directory, create the receipt from the saved archives.

```bash
python3 -I "$TRUSTED_SCRIPTS/check-crate-release.py" receipt "$TAG" \
  "$RECEIPT_DIR/archives" "$RECEIPT_DIR/crate-checksums.json" \
  --commit "$COMMIT" --toolchain 1.100.0
```

Keep the receipt, both archives, the inventory check inputs, and the upload
record for recovery. Before any upload, create a receipt in a scratch directory
from the saved archives, verified tag and commit, and pinned toolchain. Require
byte equality with the saved receipt. Check the clean publish checkout again.
Stop on any missing input or failed check. Keep the approved receipt and
archives unchanged.

## 3. Check the registry and publish the SDK

Require both versions to report `absent` in `state RECEIPT`. Use the checks for
the current registry state immediately before the SDK upload.

| Publication             | State mode      | SDK name     | Tool name    |
| ----------------------- | --------------- | ------------ | ------------ |
| Initial pair            | `state --first` | Unregistered | Unregistered |
| New tag after bootstrap | `state`         | Owned        | Unregistered |
| Later pair              | `state`         | Owned        | Owned        |

Use `check-crate-release.py owners NAME` to require `dannyota` as a user owner
and `owners --unregistered NAME` to require the crate-specific not-found
response. Verify both names in every case. For a new tag after an incomplete
first release, reconcile the old attempt as described in
[recovery](#resume-a-release) before starting the new tag. Use a fresh receipt
and approvals for the two versions on the new tag.

Use only the default Cargo home and the local credentials stored by [Cargo
login]. Before each upload, reject every set `CARGO` or `CARGO_*` name,
including `CARGO_HOME`, `CARGO_REGISTRY_*`, `CARGO_REGISTRIES_*` and
`CARGO_CREDENTIAL_ALIAS_*`, even if empty. Check names only. For the commands
below, set `PUBLISH_HOME` to the owner's macOS login home, `PUBLISH_PATH` to a
trusted executable search path, and `PUBLISH_CARGO` to the absolute path of the
trusted rustup-managed Cargo. Start with only `HOME` and `PATH` so Cargo
inherits no Rust, network or credential overrides. The only `--config` argument
is `net.retry=0`. The [Cargo config] reference describes the environment and
file settings.

```bash
(
  while IFS= read -r NAME; do
    case "$NAME" in
      CARGO|CARGO_*) exit 1 ;;
    esac
  done < <(compgen -v)
)
```

Stop on guard failure before invoking Cargo. Only one publish attempt may run at
a time. Immediately before the command, record the crate, version and approved
checksum as `unknown` in the durable Mac upload record. Retain that entry until
reconciliation confirms the outcome, even if the shell exits.

```bash
/usr/bin/env -i HOME="$PUBLISH_HOME" PATH="$PUBLISH_PATH" \
  "$PUBLISH_CARGO" +1.100.0 publish --no-verify --locked --registry crates-io \
  --package greennode-sdk --config net.retry=0
```

Always run `check-crate-release.py state RECEIPT --wait greennode-sdk` after
Cargo returns, including on a Cargo error. Require `present-and-approved` for
the SDK in the version API and sparse index and confirm its owner before
proceeding. If reconciliation fails, stop and follow the recovery rules.

## 4. Publish the tool and confirm the pair

Recheck the SDK owner and require `present-and-approved` for the SDK and
`absent` for the tool using `state` without `--first`. Repeat the applicable
ownership or unregistered check for the tool. Refresh the predecessor lookup and
inventory comparison from step 2 immediately before the tool upload, including
recovery. Verify the newly selected signed predecessor's ancestry and inventory
if the predecessor changed. If the tool is still unregistered, repeat its
unregistered check and run the first-tool inventory check.

Stop on an unresolved earlier attempt. Recheck the checkout, saved receipt and
environment. Record the pending tool upload as in step 3 and run:

```bash
/usr/bin/env -i HOME="$PUBLISH_HOME" PATH="$PUBLISH_PATH" \
  "$PUBLISH_CARGO" +1.100.0 publish --no-verify --locked --registry crates-io \
  --package greennode --config net.retry=0
```

Always run `check-crate-release.py state RECEIPT --wait greennode` after Cargo
returns. Require `present-and-approved` for both crates and `dannyota` as a user
owner of both names. Record the confirmed results against the receipt.

## Resume a release

Get the owner's approval for the retry. Repeat commit, signature, remote tag,
tree and manifest checks. Reuse the saved archives, receipt and upload record.
Recovery uses the Mac receipt without GitHub Release assets or tag-push workflow
artifacts.

Run `state` without `--first` to reconcile the pair. If both versions match,
confirm both owners and finish. If the SDK matches and only the tool is
confirmed absent, run step 4. If both uploads are confirmed absent, repeat steps
3 and 4 with the original state and ownership checks. A tool version present
without an approved SDK stops the attempt. Before any retry that uploads, repeat
the credential-free `--no-verify` dry run using Cargo 1.100.0 and `--locked` in
the fresh clean publish checkout of the same tag. Require its receipt to match
the original. Publish from that checkout.

If an upload may have started, an `absent` result by itself cannot clear it.
Require both readers to confirm the approved bytes or have crates.io confirm
that the write did not land and is no longer in flight. Preserve `unknown` until
the outcome is established. Stop on missing local records, malformed or
inconsistent registry responses, unresolved uploads, or a yank.

Run `check-crate-release.py checksum-mismatch RECEIPT CRATE` before classifying
a failed reconciliation as a checksum mismatch. When the version API and sparse
index agree on a different checksum for the exact uploaded version, record the
mismatch and stop. Never publish a tool against a mismatched SDK. After
verifying ownership and the exact affected version, the owner yanks that version
if it is still live. Keep the receipt and the remote checksum. Correct the cause
and get approval for a new shared patch tag. A registered SDK and unregistered
tool may use the mixed bootstrap path on that new tag after the old attempts are
reconciled. The old SDK version remains in the registry. Never overwrite an old
receipt or reuse a tag.

## Local checker contracts

- `tree` checks the bounded tagged tree without GitHub evidence. Reject
  symlinks, submodules and paths whose last two components are `.cargo/config`
  or `.cargo/config.toml`, case-insensitively. Allow `.cargo/sub/config`. The CI
  `evidence` tree policy remains in force for the workflow.
- `owners` checks the exact requested name and registration state against
  crates.io. Registry lookups use unauthenticated HTTPS, with no redirects and
  fresh revalidation, bounded to 30 seconds and 1 MiB per response.
- `previous TAG` selects the highest numeric `X.Y.Z` release in the crates.io
  [version list], including yanked releases. Require a complete response without
  pagination and a candidate tag above that version. Independently find the
  highest numeric version in the sparse index and require both sources to agree
  on version, checksum and yank status. Cross-check the exact-version API before
  printing the one verified tag. Missing or inconsistent data stops the release.
- `checksum-mismatch RECEIPT CRATE` is a read-only Mac diagnostic. It confirms a
  mismatch only when the version API and sparse index agree on the exact crate,
  version, checksum and yank status, and that checksum differs from the receipt.
  It reports those fields and the expected checksum. Any missing or disagreed
  field, or a checksum matching the receipt, produces no mismatch result. This
  diagnostic never clears an unresolved upload or yanks a version.

[Cargo login]: https://doc.rust-lang.org/cargo/commands/cargo-login.html
[Cargo config]: https://doc.rust-lang.org/cargo/reference/config.html
[Cargo providers]:
  https://doc.rust-lang.org/cargo/reference/registry-authentication.html
[Cargo auth source]:
  https://doc.rust-lang.org/stable/nightly-rustc/src/cargo/util/auth/mod.rs.html
[pre-upload checksum approval]: release.md#approve-the-bytes-cargo-uploads
[version list]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/src/controllers/krate/versions.rs
