# Release publishing

One `vMAJOR.MINOR.PATCH` tag releases two crates at one version. Publish
`greennode-sdk` first, then `greennode`, whose packaged manifest must require
exactly `=VERSION` of the SDK. Mac publishing is active until the owner approves
cutover to a proven CI publish workflow. Uploads are permanent ([Cargo
publishing]).

## Approaches

| Approach                                             | Trade-off                                                                                       |
| ---------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| Publish each crate from the Mac                      | Use now. The owner controls each release and can reconcile the two uploads independently.       |
| Publish from a tag or release job                    | One job can write to GitHub and crates.io, without a separate approval for the registry upload. |
| Dispatch a separate workflow with Trusted Publishing | Use after proof and approval. It keeps [duolane]'s separate approval and per-crate recovery.    |

## Versions

`greennode-sdk` declares `license = "Apache-2.0 AND BSD-3-Clause"` and ships
`licenses/go-LICENSE` and `licenses/go-NOTICE` (each Go-derived file with its
upstream copyright line) for code and tables derived from Go's standard library.

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

## Releases from the owner's Mac

The [Mac release procedure](release-first.md) uses the owner's existing Cargo
login. It stages archives in the publish checkout and checks the uploaded
checksums against the saved receipt before proceeding. The tag push runs the
GitHub Release workflow. CI publishing uses that workflow's packaging evidence
and checks the actual archive before upload.

## Release workflow

Before pushing a tag, the owner checks its commit against freshly fetched
protected `master`, because a tag push runs the workflow file from the tagged
commit. `.github/workflows/release.yml` runs on `v*` tag pushes, one run at a
time. As defense in depth, its first job fails unless the tag's commit is in
protected `master`'s history; later jobs check out that SHA, and no step that
runs tagged code holds a token. Validate the tag, manifests, lockfile,
dependency policy and tests with pinned tools. Build only the tool with
`cargo build --locked --release --package greennode --target TARGET`, check its
version against the tag, and smoke-test the extracted Windows archive.

Each matrix job exports its binary's list with the hidden `__command-inventory`
command; all four must match. Run
`check-crate-release.py shipped CANDIDATE COMMITTED PREVIOUS`, or `--first` for
the first release. It blocks the Release unless the export matches
`crates/greennode/tests/parity/shipped-commands.json` at the tag and contains
every entry of the preceding published tag's list, even if crates.io publication
was incomplete. The [CLI design](cli.md) owns the format.

| Target                      | Archive                         |
| --------------------------- | ------------------------------- |
| `aarch64-apple-darwin`      | `greennode-darwin-arm64.tar.gz` |
| `x86_64-unknown-linux-gnu`  | `greennode-linux-amd64.tar.gz`  |
| `aarch64-unknown-linux-gnu` | `greennode-linux-arm64.tar.gz`  |
| `x86_64-pc-windows-msvc`    | `greennode-windows-amd64.zip`   |

Each archive holds the executable, `LICENSE`, `licenses/go-jmespath-LICENSE`,
`licenses/go-LICENSE` and `licenses/go-NOTICE` (Go's licence and the copyright
lines of each Go-derived file); the release job checks all four license files
against the tag and the executable against its build artifact. Upload the
archives, their `greennode-PLATFORM.sha256` files, and the
`crate-checksums.json` an unprivileged Ubuntu 24.04 job builds from the staged
packages, each with `retention-days: 90`. Stage all nine assets on a draft.
Immediately before publishing it to lock them and the tag ([GitHub immutable
releases]), check that the live tag names the verified commit and the preceding
published tag is unchanged. This workflow is the only Release writer; nobody
creates or edits a Release by hand. Only the Release job has `contents: write`.
It runs no script from the tag: it writes the notes with inline `gh api` calls
to the [generate notes] and compare endpoints, listing non-merge commit
subjects. No release job has OIDC or registry write credentials. The receipt is
written on a fresh runner that never built the tag, from the staged archives the
packaging job uploads.

### Release evidence for CI publishing

The CI crates.io workflow requires a published immutable GitHub Release, the tag
commit in protected `master`'s history, and a successful `release.yml` tag-push
run for the exact tag and commit. Compare each expected uploaded asset's SHA-256
with the corresponding downloaded build artifact from that run. Dispatch and
recovery must finish within [GitHub artifact retention]. A missing artifact or
digest mismatch blocks CI publishing. If the artifacts expire, cut and publish a
new shared version. A lone SDK version stays published. The immutable Release
receipt preserves the checksums but cannot replace the exact-run evidence.

### Workspace packaging

Use a clean tagged checkout, no Cargo overrides or registry credentials, and new
empty directories for the target, build and staging paths. Stage both packages
with a workspace [dry run][dry runs] and `--no-verify`, so no build script or
proc macro runs before the archives are copied out of the build directory's
`package/tmp-crate/` ([Cargo package source]), never `target/package/`. A
separate job repeats the dry run with verification and fails unless its archives
hash the same.

```bash
BUILD_ROOT=$(mktemp -d)
STAGE=$(mktemp -d)
CARGO_TARGET_DIR="$BUILD_ROOT/target" CARGO_BUILD_BUILD_DIR="$BUILD_ROOT/build" \
  cargo +1.100.0 publish --workspace --dry-run --no-verify --locked \
  --registry crates-io
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
   home, pinned Rust and the checksum-checking provider. No compilation or
   restored build cache. Give `CARGO_REGISTRY_TOKEN` only to Cargo and its
   provider for uploads, never workflow-wide or in files.

### Approve the bytes Cargo uploads

`cargo publish` packages again. Cargo 1.100 computes SHA-256 of the actual
archive and asks its credential provider for the `publish` operation with the
name, version and checksum before uploading ([Cargo credential protocol], [Cargo
1.100 publish source]). In CI, pass `--config scripts/publish-cargo.toml`, the
one protected file that sets `registry.credential-provider` to the executable
checksum-checking helper. Cargo runs only a provider's program with
`--cargo-plugin` and sends its other configured arguments in the protocol
messages ([Cargo 1.100 credential process]), so the helper is the program
itself, with a shebang that runs isolated Python. Give a token for `publish`
only if the registry is crates.io and the name, version and checksum match the
current upload and validated receipt. Allow a `read` credential only for that
registry. Allow no other write. Set `cache: "never"` and
`operation_independent: false`. No fallback provider or `--token`. Read
`CARGO_REGISTRY_TOKEN` ([Cargo token config]) and send it only through the
provider protocol. Refuse different bytes before upload.

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

The owner checks the preceding attempt before another upload. If both readers
still say absent after an ambiguous write, get confirmation from crates.io that
the write did not land and is no longer in flight. A red job, old 404 or elapsed
time does not clear it. For Mac recovery, use the
[saved local receipt and archives](release-first.md#resume-a-release). For CI
recovery, dispatch the same tag for a new approval while the exact-run artifacts
still exist. Publish only confirmed missing versions. Investigate yanks and
mismatches before any write. Never automatically unyank or retag. Correct an
accidental yank only after verifying the approved bytes. A version with
different bytes needs a new shared tag.

## GitHub Release setup

Protect `master`, enable immutable GitHub Releases and allow 90-day artifact
retention. Add a tag ruleset on `v*` that blocks updates and deletions and lets
only the owner create tags. Allow the pinned actions for the Release workflow,
including `softprops/action-gh-release`.

## Packaging proof

Use `scripts/test/loopback-registry-proof.py` with Cargo 1.100.0 and two toy
crates. From empty paths, check dry-run staging and equality with the individual
publish archive checksums before and after the SDK is indexed. The proof also
checks that the CI checksum provider prevents a mismatched upload. It runs the
workflow's exact Cargo arguments, `--registry crates-io` and
`scripts/publish-cargo.toml`, against the loopback registry through Cargo
1.100's test-only crates.io URL override, because Cargo ignores source
replacement for an explicitly named registry ([Cargo 1.100 registry source]). A
gate compares the workflow's Cargo arguments with the proof's. Cargo rebuilds
the packaged lockfile, so byte equality must be established by the proof. A
mismatch blocks the affected publish path.

The Mac path requires a passing macOS proof with the pinned Cargo version. The
script passed on macOS on 2026-11-26 for both toy secret sources. The first live
CI upload, for `v0.1.1` on 2026-11-28, stopped before upload because the proof
had not run the workflow's provider configuration. The corrected proof passed on
macOS and on Ubuntu 24.04 (aarch64) on 2026-11-28 for both sources. The publish
job runs on x86_64, so the next CI upload is the proof on that architecture.

## CI cutover

1. Pass the Ubuntu packaging proof. Create the `crates-io` environment, restrict
   it to `master`, require the owner and disallow bypass. Allow self-review
   while the owner is the only reviewer. Allow the pinned
   `rust-lang/crates-io-auth-action` at
   `3268eb413133e2960fa4ae0e963ca76380c54cd9`.
2. After both names exist, only the owner adds their Trusted Publishers ([RFC
   3691], [crates.io guide source]). In each crate's Settings → Trusted
   Publishing, select GitHub. Set owner `dannyota`, repository `greennode`,
   workflow `publish-crates.yml` and environment `crates-io`. CI uses OIDC. Keep
   the Mac token out of GitHub secrets.
3. The first CI candidate must pass `check-crate-release.py previous` and
   `shipped` against the latest tool release on crates.io, not the latest GitHub
   tag, before its upload is authorized, so a Mac release whose GitHub Release
   failed is still compared.
4. For a release chosen by the owner, confirm the `gh` Release and exact-run
   fields against live output in a preflight-only dispatch. Pass that preflight
   and obtain the owner's approval for the first live CI dispatch. Prove both
   uploads, their approved checksums and ownership. Recover a partial CI upload
   through the CI path for that tag. After the proof succeeds, the owner
   approves moving subsequent releases to CI.

## Unverified

- The owner checks the deployed Trusted Publishing forms against their source
  and verifies the environment and run-approval reads on the first CI dispatch.
- The crates.io not-found bodies were observed on 2026-11-25. A reset during the
  response head or a TLS timeout behind a proxy may stop the registry check.

[duolane]:
  https://github.com/dannyota/duolane/blob/master/docs/design/release.md
[Cargo publishing]: https://doc.rust-lang.org/cargo/reference/publishing.html
[Cargo dependencies]:
  https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations
[dry runs]:
  https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/#cargo-adds-native-support-for-workspace-publishing
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
[generate notes]:
  https://docs.github.com/en/rest/releases/releases#generate-release-notes-content-for-a-release
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
[Cargo 1.100 credential process]:
  https://github.com/rust-lang/cargo/blob/rust-1.100.0/src/util/credential/process.rs#L92-L105
[Cargo 1.100 registry source]:
  https://github.com/rust-lang/cargo/blob/rust-1.100.0/src/ops/registry/mod.rs#L157-L184
