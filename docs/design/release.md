# Release publishing

Status: proposed. The workflows this design describes are not in the repository
yet.

A release has two parts: a GitHub Release with archives of the tool, and one
version of each crate on crates.io. `greennode-sdk` and `greennode` carry the
same version and publish together, the SDK first, because the tool depends on
that exact SDK version. Publishing to crates.io is a separate owner decision
because an uploaded version cannot be replaced or deleted
([Cargo: publishing](https://doc.rust-lang.org/cargo/reference/publishing.html)).

## Approaches

Use the two-workflow design from
[duolane](https://github.com/dannyota/duolane/blob/master/docs/design/release.md):
a tag-triggered release workflow and a separate manual publishing workflow with
a preflight-only mode. Publishing automatically from a tag would remove the
owner's publish decision. Publishing from the release job would give one job
both GitHub write access and crates.io access.

## First version

crates.io creates a Trusted Publisher only for a crate that already exists: "A
Trusted Publisher Configuration can only be created after an initial manual
publishing of a crate"
([RFC 3691](https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html)).
The owner publishes the first version of both crates by hand from the owner's
Mac.

1. The version is tagged on `master` and its GitHub Release exists.
2. On crates.io, create an API token with the `publish-new` scope, limited to
   the crate names `greennode` and `greennode-sdk`
   ([RFC 2947](https://rust-lang.github.io/rfcs/2947-crates-io-token-scopes.html)).
3. In a clean checkout of the tag, run
   `cargo publish --dry-run --locked --registry crates-io --package greennode-sdk`
   and read the file list from `cargo package --list --package greennode-sdk`.
   The tool's dry run cannot pass until the SDK version is on crates.io, so it
   runs after step 5.
4. Run `cargo login` and paste the token at its prompt. The token never goes on
   a command line, into a file in the repository, or into a conversation.
5. Run `cargo publish --locked --registry crates-io --package greennode-sdk`.
6. Run the dry run for `--package greennode`, then
   `cargo publish --locked --registry crates-io --package greennode`.
7. Run `cargo logout` and revoke the token on crates.io.
8. On crates.io, add a Trusted Publisher to each crate: repository owner
   `dannyota`, repository `greennode`, workflow `publish-crates.yml`,
   environment `crates-io`.

Steps 4 to 8 are the owner's. An agent prepares and checks the tag and runs
step 3.

## Later versions

Every later version publishes from GitHub Actions with no stored token.

- The tag-triggered `release.yml` tests the workspace, builds archives of the
  tool for macOS on Apple silicon, Linux on x86-64 and Arm64, and Windows on
  x86-64, and uploads each archive with its checksum.
- The owner dispatches `publish-crates.yml` from `master` with a
  `vMAJOR.MINOR.PATCH` tag. The default is preflight only.
- Preflight resolves the tag to a commit in the history of `master`, checks both
  manifest versions and the tool's SDK requirement against the tag, requires a
  successful `release.yml` run for that tag and commit, checks every release
  asset against that run's artifacts, and builds the SDK package with
  `cargo publish --dry-run --locked`, without a token.
- Publishing needs a fresh dispatch with the explicit publish input, an
  immutable GitHub Release, HTTP 404 from crates.io for that exact version of
  both crates, and the owner's approval on the `crates-io` environment.
- The publish job alone has `id-token: write`. It exchanges the GitHub OIDC
  token for a short-lived crates.io token with the pinned
  `rust-lang/crates-io-auth-action`, passes it as `CARGO_REGISTRY_TOKEN`
  ([Cargo: registry.token](https://doc.rust-lang.org/cargo/reference/config.html#registrytoken)),
  and publishes the SDK and then the tool in an empty Cargo home.
- A run that publishes the SDK and then fails leaves the SDK version without its
  tool. A new dispatch for the same tag publishes only the crate that is still
  missing.

## One-time setup

- GitHub: enable immutable releases. Create the `crates-io` environment,
  restrict it to `master`, and require the owner's review, with self-review
  allowed while the owner is the only reviewer.
- GitHub: add `softprops/action-gh-release` and the pinned
  `rust-lang/crates-io-auth-action` commit to the Actions allowlist.
- crates.io: the two Trusted Publishers from step 8 above.
- Add no crates.io token to GitHub secrets.

## Unverified

- The Trusted Publisher form and the token form on crates.io were not opened for
  this design. The field names come from RFC 3691 and RFC 2947, and the
  [crates.io guide](https://crates.io/docs/trusted-publishing) is the page to
  follow when they differ.
- Whether one token from the auth action may publish two crates in one job is
  not confirmed. If it may not, the job exchanges a token per crate.
