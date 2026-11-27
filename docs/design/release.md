# Release publishing

One `vMAJOR.MINOR.PATCH` tag releases two crates at one version. Publish
`greennode-sdk` first, then `greennode`, whose packaged manifest must require
exactly `=VERSION` of the SDK. The owner publishes the first pair from the Mac,
later ones through GitHub Actions. Uploads are permanent ([Cargo publishing]).

## Approaches

| Approach                                                              | Trade-off                                                                                      |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Publish from a tag or release job                                     | Fewer steps, but no separate owner decision and one job can write to GitHub and crates.io.     |
| Upload with `cargo publish --workspace`                               | Cargo orders dependencies, but uploads are not atomic and a retry must omit existing versions. |
| Separate release and manual publish workflows, with per-crate uploads | Use this approach. It keeps [duolane]'s gates and retries only missing uploads.                |

## Versions

`greennode-sdk` declares `license = "Apache-2.0 AND BSD-3-Clause"` and ships
`licenses/go-LICENSE` for the tables derived from Go's standard library.

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

The owner publishes the first pair by hand, as the
[first release](release-first.md) design specifies.

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
and `licenses/go-LICENSE` (the Go notice for the HTML entity and `strconv`
tables); the release job checks all three license files against the tag and the
executable against its build artifact. Upload the archives, their
`greennode-PLATFORM.sha256` files, and the `crate-checksums.json` an
unprivileged Ubuntu 24.04 job builds from the staged packages, each with
`retention-days: 90`. Stage all nine assets on a draft. Immediately before
publishing it to lock them and the tag ([GitHub immutable releases]), check that
the live tag names the verified commit and the preceding published tag is
unchanged. This workflow is the only Release writer; nobody creates or edits a
Release by hand. Only the Release job has `contents: write`. It runs no script
from the tag: it writes the notes with inline `gh api` calls to the [generate
notes] and compare endpoints, listing non-merge commit subjects. No release job
has OIDC or registry write credentials. The receipt is written on a fresh runner
that never built the tag, from the staged archives the packaging job uploads.

### Release evidence

Require a published immutable GitHub Release, tag commit in protected `master`'s
history, and a successful `release.yml` tag-push run for the exact tag and
commit. Compare each expected uploaded asset's SHA-256 with the corresponding
downloaded build artifact from that run. No missing artifacts, expired artifacts
or digest mismatches pass. Bootstrap, dispatch and recovery must finish within
[GitHub artifact retention]. After that, never rebuild for new evidence: cut and
publish a new shared version; a lone SDK version stays published. The immutable
Release receipt preserves the checksums but cannot replace the exact-run
evidence.

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
  `master`. Add a tag ruleset on `v*` that blocks updates and deletions and lets
  only the owner create tags. Create environment `crates-io`, restrict it to
  `master`, require the owner and disallow bypass. Allow self-review while the
  owner is the only reviewer. Allow the pinned workflow actions, including
  `softprops/action-gh-release` and `rust-lang/crates-io-auth-action` at
  `3268eb413133e2960fa4ae0e963ca76380c54cd9`.
- Only the owner adds both crates' Trusted Publishers after initial publication
  ([RFC 3691], [crates.io guide source]). In each crate's Settings → Trusted
  Publishing, select GitHub. Set owner `dannyota`, repository `greennode`,
  workflow `publish-crates.yml` and environment `crates-io`.
- Never put a crates.io API token in GitHub secrets. The bootstrap token stays
  local; later jobs use OIDC.

## Unverified

- The crates.io forms come from their source, not the rendered guide; the owner
  checks the deployed forms.
- Accepted limits of the registry check: crates.io's not-found bodies were
  observed on 2026-11-25, not documented; a reset during the response head and a
  TLS timeout behind a proxy are not retried precisely.
- Before any release, test two toy crates on a loopback registry with Cargo
  1.100.0 on macOS and Ubuntu. From empty paths, check dry-run staging, equality
  with the individual publish operation's checksums before and after the SDK is
  indexed, and no upload on mismatch. Test both secret sources. Cargo rebuilds
  the packaged lockfile, so byte equality is not guaranteed. A mismatch blocks
  release; do not weaken the gate. `scripts/test/loopback-registry-proof.py`
  runs it; it passed on macOS on 2026-11-26 for both secret sources. Ubuntu is
  still to run.
- Environment and Trusted Publishers stay untested until the first CI publish.
  The environment and run-approval reads rely on GitHub's documented repository
  read access, as duolane's do; the first dispatch confirms them.
- The `gh` field names the [first release](release-first.md) gathers are checked
  against live output before that release.

[duolane]:
  https://github.com/dannyota/duolane/blob/master/docs/design/release.md
[Cargo publishing]: https://doc.rust-lang.org/cargo/reference/publishing.html
[Cargo dependencies]:
  https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations
[dry runs]:
  https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/#cargo-adds-native-support-for-workspace-publishing
[Cargo login]: https://doc.rust-lang.org/cargo/commands/cargo-login.html
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
