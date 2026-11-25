# Release publishing

Status: proposed.

One `vMAJOR.MINOR.PATCH` tag releases two crates at one version. Publish
`greennode-sdk` first, then `greennode`, whose packaged manifest must require
exactly `=VERSION` of the SDK. The owner publishes the first pair from the
owner's Mac. Later releases use GitHub Actions and crates.io Trusted Publishing.
A GitHub Release does not authorize a crates.io upload. An uploaded version
cannot be replaced or deleted ([Cargo publishing]).

## Approaches

| Approach                                                              | Trade-off                                                                                                                                |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Tag or release job also publishes                                     | Fewer steps, but no separate owner decision and one job could write to both GitHub and crates.io.                                        |
| One `cargo publish --workspace` upload                                | Cargo orders dependencies, but uploads are not atomic and a partial release needs a retry that omits the existing crate.                 |
| Separate release and manual publish workflows, with per-crate uploads | Keeps [duolane]'s approval and permissions gates and can resume the same tag without sending an existing crate again. Use this approach. |

## Versions

The virtual workspace's `Cargo.toml` owns `[workspace.package].version`. Both
members use `version.workspace = true`. The tool's `Cargo.toml` declares
`greennode-sdk = { path = "../greennode-sdk", version = "=VERSION" }`, with
`VERSION` replaced by the literal version. Cargo uses the path locally and the
version in the published package ([Cargo dependencies]).

A version-only release commit changes `Cargo.toml`,
`crates/greennode/Cargo.toml`, and `Cargo.lock`. Do not edit a member's
inherited package version. Run `cargo +1.100.0 check --workspace --all-targets`
to refresh the lockfile, then repeat with `--locked`. A manifest check in
`make check` and CI rejects a tool SDK requirement other than `=VERSION`, the
wrong local path, or either member not inheriting the workspace version. Check
this without a tag on every change and against the tag in release and preflight.

## First release from the owner's Mac

1. Get the owner's approval for the tag and GitHub Release. Run from the root of
   a clean, disposable checkout with no Cargo config or environment overrides.
   Use Bash and Python 3.11 or later. For the first tag:

   ```bash
   TAG=v0.1.0
   git fetch origin master --tags
   test -z "$(git status --porcelain)"
   git merge-base --is-ancestor "refs/tags/$TAG^{commit}" origin/master
   git switch --detach "refs/tags/$TAG"
   gh release view "$TAG" --repo dannyota/greennode --json isDraft,isImmutable \
     --jq '.isDraft == false and .isImmutable == true' | grep -qx true
   ```

   Run commands one at a time. Stop on a failed check. Always clean up in step 7
   after login. An agent may verify the tag and release assets against the exact
   successful run.

2. Before creating a token, check both packages together. Cargo 1.90 added
   verification against the workspace packages as if they were already
   published, including during [workspace dry runs]. Read both file lists. Check
   that each includes its LICENSE, README, and expected sources and nothing
   private or untracked. The `receipt` command checks the actual manifests and
   packaged lockfile before saving hashes outside the checkout.

   ```bash
   umask 077
   export CARGO_HOME="$(mktemp -d)"
   RECEIPT_DIR="$(mktemp -d)"
   export GREENNODE_RELEASE_RECEIPT="$RECEIPT_DIR/crate-checksums.json"
   cargo +1.100.0 publish --workspace --dry-run --locked --registry crates-io
   cargo +1.100.0 package --list --locked --registry crates-io -p greennode-sdk
   cargo +1.100.0 package --list --locked --registry crates-io -p greennode
   python3 -I scripts/check-crate-release.py receipt "$TAG" "$GREENNODE_RELEASE_RECEIPT"
   python3 -I scripts/check-crate-release.py state "$GREENNODE_RELEASE_RECEIPT" --first
   ```

   An agent may run credential-free checks. The owner reads both lists and
   approves the Mac receipt. Keep it and the two `.crate` files until both
   uploads are confirmed. The Mac receipt approves Mac bytes, not a Linux
   runner's output. `--first` requires both crate names to be unregistered.

3. **Owner only.** In crates.io Settings → API Tokens, create one short-lived
   token with endpoint scope `publish-new` only and two exact crate patterns:
   `greennode-sdk` and `greennode`. No wildcard, `publish-update`, owner, or
   yank scope. [Token scopes] distinguish creating a crate from releasing
   another version of an existing crate.

4. **Owner only.** Enter the token without echo; use Cargo's built-in provider
   for login. The token may exist in the crates.io form, the hidden prompt and
   its stdin pipe, and the isolated `$CARGO_HOME/credentials.toml` and Cargo
   processes. Never place it in command arguments, shell history, a checkout,
   GitHub secrets, logs, or chat. Keep tracing off.

   ```bash
   set +x
   IFS= read -r -s -p 'crates.io API token: ' release_token
   printf '\n'
   printf '%s\n' "$release_token" | cargo +1.100.0 login --registry crates-io \
     --config 'registry.credential-provider="cargo:token"'
   unset release_token
   ```

5. **Owner only.** Recheck both names with `state --first`, then publish the
   SDK. Run the wait check even if Cargo's upload errors or times out. Require
   the SDK to match the receipt, be indexed and not yanked, and list the owner.

   ```bash
   PROVIDER="registry.credential-provider=['python3','-I','$PWD/scripts/check-crate-publish-credential.py']"
   python3 -I scripts/check-crate-release.py state "$GREENNODE_RELEASE_RECEIPT" --first
   cargo +1.100.0 publish --no-verify --locked --registry crates-io \
     --package greennode-sdk --config "$PROVIDER" --config net.retry=0
   python3 -I scripts/check-crate-release.py state "$GREENNODE_RELEASE_RECEIPT" --wait greennode-sdk
   ```

6. **Owner only.** Recheck both with `state`. With the SDK approved and the tool
   still absent, publish the tool. Run the wait check even on a Cargo error.
   Confirm both versions and owners.

   ```bash
   python3 -I scripts/check-crate-release.py state "$GREENNODE_RELEASE_RECEIPT"
   cargo +1.100.0 publish --no-verify --locked --registry crates-io \
     --package greennode --config "$PROVIDER" --config net.retry=0
   python3 -I scripts/check-crate-release.py state "$GREENNODE_RELEASE_RECEIPT" --wait greennode
   ```

7. **Owner only.** Even after an error or interruption, remove the local token
   and revoke it in crates.io Settings → API Tokens. [Cargo logout] removes the
   local copy; it does not revoke the token on the server. Remove the isolated
   Cargo home, and keep the receipt and archives if resuming.

   ```bash
   cargo +1.100.0 logout --registry crates-io \
     --config 'registry.credential-provider="cargo:token"'
   rm -rf -- "$CARGO_HOME"
   unset CARGO_HOME
   ```

8. **Owner only.** Set up Trusted Publishing for both crates after the pair is
   confirmed. Follow [one-time setup](#one-time-setup).

### Resume the first release

Use the same tag and Mac receipt. Run the credential-free `state` command
without `--first`. If both match, do not upload. If the SDK matches and the
`greennode` name is still unregistered, create a new `publish-new` token scoped
only to `greennode`. Create a fresh isolated Cargo home, set `PROVIDER` from
step 5 without running its upload, and repeat steps 4, 6 and 7. Do not retry an
unknown upload or widen token scope. Without the saved receipt, stop.

## Release workflow

`.github/workflows/release.yml` runs on `v*` tag pushes. Validate the exact tag,
workspace manifests and lockfile, and run the dependency policy and workspace
tests with the pinned Rust and pinned CI tools. Build only the tool with
`cargo build --locked --release --package greennode --target TARGET`. Check the
binary's version against the tag and smoke-test the extracted Windows archive.

| Target                      | Archive                         |
| --------------------------- | ------------------------------- |
| `aarch64-apple-darwin`      | `greennode-darwin-arm64.tar.gz` |
| `x86_64-unknown-linux-gnu`  | `greennode-linux-amd64.tar.gz`  |
| `aarch64-unknown-linux-gnu` | `greennode-linux-arm64.tar.gz`  |
| `x86_64-pc-windows-msvc`    | `greennode-windows-amd64.zip`   |

Upload the four archives and their four `greennode-PLATFORM.sha256` files as
build artifacts and GitHub Release assets. A separate unprivileged job runs the
workspace dry run on Ubuntu 24.04 and uploads `crate-checksums.json` as a build
artifact. The Release job attaches that receipt as a ninth asset. It is the
reference for later crates.io releases and is not a substitute for the Mac
receipt on the first release. Stage all assets on a draft, then publish it to
lock the assets and tag ([GitHub immutable releases]). The Release job alone has
`contents: write`. Other jobs have `contents: read`, and no release job has OIDC
or crates.io credentials.

## Manual publish workflow

`.github/workflows/publish-crates.yml` uses `workflow_dispatch` in
`dannyota/greennode` on `refs/heads/master` only. The inputs are required string
`tag` (`vMAJOR.MINOR.PATCH` with no leading zeros) and boolean `publish`,
default `false`. Both modes use one repository-wide concurrency group with
`cancel-in-progress: false`. This serializes every publish for a tag and also
keeps two different versions from interleaving SDK and tool uploads. A publish
requires `publish: true`, protected `master`, and the first attempt of a fresh
dispatch, not GitHub's rerun button. Default permissions are `{}`.

### Preflight job

1. Give `verify` only `contents: read` and `actions: read`, no environment,
   OIDC, or token. Resolve the tag to a full commit SHA in the history of the
   dispatch commit. Check the tree and manifest data without executing tagged
   scripts. Reject tracked symlinks or Cargo config in the publish checkout.
2. Require a published GitHub Release and a successful `release.yml` tag-push
   run for that exact tag and commit. Check each expected uploaded asset's
   digest against its build artifact in that run. A publish additionally
   requires the Release to be immutable and `dannyota` to be listed as a user
   owner of each crate. In preflight-only mode, report missing ownership if a
   crate is not yet registered; otherwise enforce ownership. Reject other lookup
   failures in either mode.
3. Check the shared version and SDK pin. Require explicit README paths and a
   LICENSE copy per archive. README and license inputs must resolve inside the
   checkout to tracked regular files. The current tool README comes from the
   workspace root; the SDK README is local to the crate. Rebuild the receipt in
   a clean Ubuntu 24.04 checkout of the tag with the pinned numeric toolchain
   and no publish credentials:

   ```bash
   cargo +1.100.0 publish --workspace --dry-run --locked --registry crates-io
   ```

   Hash the `.crate` files Cargo created in `target/package`, not repacked
   copies. Each canonical `Cargo.toml` in the archives must have its exact crate
   name and tag version. The tool must require `greennode-sdk` as a normal
   crates.io dependency at `=VERSION`, without a path or alternate source. Its
   `Cargo.lock` must bind the SDK version and approved SDK SHA-256. Check both
   archive file lists. The JSON receipt records the tag, commit, toolchain, and
   exactly two crate records, each with name, version, filename and SHA-256.
   Require equality with the Release receipt. Cargo warns on an existing version
   in a dry run ([Cargo 1.100 publish source]); the registry gate decides if the
   existing version is acceptable.

4. Reconcile both crates. Put the tag, resolved commit, numeric toolchain,
   artifact ID, and receipt digest in job outputs. Upload the verified `.crate`
   files and receipt as one same-run artifact. Show both hashes and registry
   states in the owner approval summary. The Release receipt remains the
   reference on every dispatch; do not accept a user-supplied digest.

### Publish job

1. Give `publish` `contents: read`, `actions: read`, and `id-token: write`. Use
   the `crates-io` environment. Before exchanging OIDC, check that the owner is
   still a required reviewer and that GitHub recorded approval by that owner for
   this run and environment. Recheck protected `master`, the exact tag commit,
   immutable Release, absence of Cargo config and tracked symlinks, and the
   receipt and archive digests. Use the trusted helpers from the dispatch
   commit, not scripts from the tag or the artifact.
2. Reconcile both crates again before exchange. If both match, stop successfully
   without minting a token. Otherwise exchange once with the pinned
   [`rust-lang/crates-io-auth-action`]. The two identical publisher
   configurations match the same OIDC identity. crates.io [binds all matching
   crate IDs] to one token and [checks membership at upload]. No separate
   exchange is needed for each crate. The action revokes the token in its post
   step and crates.io expires it after 30 minutes ([crates.io guide source]).
3. Publish only missing versions, SDK before tool, in a fresh empty Cargo home,
   with pinned Rust, no restored build cache, and no compilation. Use the Mac
   `cargo publish --no-verify --locked` command with the checksum provider,
   `--registry crates-io`, and `--config net.retry=0`. Give only Cargo and its
   provider `CARGO_REGISTRY_TOKEN` for the upload. Never put the token in
   workflow-wide environment, files, or unmasked outputs.

### Approve the bytes Cargo uploads

`cargo publish` packages again rather than taking an existing `.crate` as an
argument. A preflight hash alone cannot prove which bytes Cargo will send. Cargo
1.100 computes SHA-256 on the actual tarball and asks its configured credential
provider for the `publish` operation with the name, version, and checksum before
upload ([Cargo credential protocol], [Cargo 1.100 publish source]).

Set `registry.credential-provider` explicitly to the checksum-checking helper.
It reads a validated receipt with both hashes before taking credentials. It
returns a token for `publish` only when the registry is crates.io and the name,
version, and actual checksum match the current upload and receipt. No other
write operation gets a token. If Cargo needs a `read` credential before
packaging, the provider may answer only for the same registry. Return
`cache: "never"` and `operation_independent: false` so a prior read cannot
bypass the publish check. Do not configure a fallback provider or pass
`--token`. Take the token from `CARGO_REGISTRY_TOKEN` in CI ([Cargo token
config]), or from Cargo's isolated login file on the Mac ([Cargo login]). The
provider sends it only to Cargo over the credential protocol, never to logs.
Refuse before upload if Cargo generates different bytes, including after the SDK
is indexed.

## Registry gate and recovery

Check each exact version using the [crates.io version API source] for
`https://crates.io/api/v1/crates/CRATE/VERSION` and the [Cargo sparse index] for
`https://index.crates.io/gr/ee/CRATE`. Use bounded, unauthenticated HTTPS reads
with fresh revalidation and a generic user agent. Match exact crate name and
version. Read SHA-256 (`cksum`) and the boolean `yanked` from the index; compare
them with the API's `version.checksum` and `version.yanked` and the approved
receipt. Do not use a Cargo cache or `cargo search` as evidence.

| Registry observation                                                                                 | Decision                                                    |
| ---------------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| API returns a valid not-found response and the index has no exact version, with no unresolved upload | Absent. Eligible for upload when its dependencies match.    |
| Both report the exact version, the approved SHA-256, and not yanked                                  | Present and approved. Skip upload.                          |
| Yanked, wrong checksum, invalid response, disagreement, or unresolved response or upload             | Stop. Do not publish either missing crate on that evidence. |

Check both before the first write and recheck before each write. The tool
requires a matching, non-yanked SDK in the sparse index, not just an upload
response. After each upload poll both endpoints for a bounded interval. A
network timeout, duplicate response, cancellation, or delayed visibility is not
a failed upload; it may already have landed. If the version cannot be confirmed,
record the exact crate, version, checksum and `unknown` outcome in the run
summary and stop. Do not retry the PUT or proceed to the tool. A job that
stopped after starting Cargo and has no confirmed outcome is also unknown.

For recovery, the owner checks the previous run before dispatching the same tag
again. If an upload may have landed, reconcile with fresh registry evidence
before asking for publish approval. If both readers still say absent after an
ambiguous write, get confirmation from crates.io that the write did not land and
is no longer in flight before retrying. A red job, an old 404, or elapsed time
alone does not clear an unknown upload. If both crates now match, nothing needs
publishing. If the SDK matches and the tool is confirmed absent, a fresh publish
dispatch uploads only the tool. If neither upload was attempted or both are
confirmed absent, upload both in order. Do not reuse an old approval.

A yanked version is not absent. A checksum mismatch means the tag would release
against unapproved bytes. The owner investigates the crate, account, and
workflow before any write. Do not automatically unyank or retag. An owner may
correct an accidental yank only after verifying the approved bytes; a version
with different bytes needs a new shared version and tag.

## One-time setup

- On GitHub, enable immutable Releases before the first tag. Protect `master`.
  Create the `crates-io` environment, restrict it to that branch, require the
  owner as a reviewer, and disallow bypass. Allow self-review while the owner is
  the only reviewer. Allow only the pinned actions required by these workflows,
  including `softprops/action-gh-release` and `rust-lang/crates-io-auth-action`
  at `3268eb413133e2960fa4ae0e963ca76380c54cd9`.
- On crates.io, open Settings → Trusted Publishing on each of `greennode-sdk`
  and `greennode`. Select GitHub and set repository owner `dannyota`, repository
  name `greennode`, workflow filename `publish-crates.yml`, and environment
  `crates-io` for both. Each crate needs its own configuration and must already
  exist ([RFC 3691], [crates.io guide source]). Only the owner does this.
- Never put a crates.io API token in GitHub repository, environment, or
  organization secrets. The bootstrap token stays local. Later jobs use OIDC.

## Implementation boundary

- Port duolane's release matrix, policy and test checks, exact binary version
  check, Windows smoke test, assets and notes, pinned actions and tools, and
  job-scoped permissions. Port its manual event, protected branch and owner
  approval gates, tag ancestry, release run and asset digest checks, registry
  owner check, safe checkout, pinned toolchain, and fresh-dispatch policy.
- Replace its standalone manifest check with workspace and archive checks;
  replace its single-package dry run with workspace verification. Add the
  immutable checksum receipt, per-crate registry gate, ordered recovery, and
  checksum-checking Cargo credential provider. The provider replaces plain
  `cargo:token` only for actual uploads; those still use `--no-verify`.
- Port the release checker and tests. Add hermetic tests for both package
  manifests, lockfile binding, wrong tool pin, missing or mixed receipts,
  provider calls and token redaction, registry disagreement, duplicate or
  ambiguous uploads, yank, mismatch, and SDK-only recovery.

## Unverified

- The rendered crates.io guide was unavailable to fetch. Its official source and
  the token form source confirm the fields; the owner checks the deployed UI
  while setting up the token and both Trusted Publishers.
- Validate with Rust 1.100.0 and two toy crates on a loopback registry that the
  workspace dry run and later individual uploads produce byte-identical archives
  before and after the first crate is indexed, and that a wrong hash causes no
  upload. Exercise both the login-file and environment token paths on macOS and
  Ubuntu. A failed match blocks real release; never weaken the checksum check to
  get past it.
- The actual GitHub environment and crates.io publisher settings have not been
  exercised. The owner runs credential-free preflight, then approves the first
  later release after verifying the two configurations.

[duolane]:
  https://github.com/dannyota/duolane/blob/master/docs/design/release.md
[Cargo publishing]: https://doc.rust-lang.org/cargo/reference/publishing.html
[Cargo dependencies]:
  https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations
[workspace dry runs]:
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
[Cargo sparse index]:
  https://doc.rust-lang.org/cargo/reference/registry-index.html
[GitHub immutable releases]:
  https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases
[RFC 3691]:
  https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html
[crates.io guide source]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/svelte/src/routes/docs/trusted-publishing/+page.svelte
[binds all matching crate IDs]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/src/controllers/trustpub/tokens/exchange/mod.rs#L152-L245
[checks membership at upload]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/src/controllers/krate/publish.rs#L211-L230
[crates.io version API source]:
  https://github.com/rust-lang/crates.io/blob/7866a4599199c2034bc631eb067d764486bf7a3a/src/controllers/version/metadata.rs#L22-L46
[`rust-lang/crates-io-auth-action`]:
  https://github.com/rust-lang/crates-io-auth-action/tree/3268eb413133e2960fa4ae0e963ca76380c54cd9
