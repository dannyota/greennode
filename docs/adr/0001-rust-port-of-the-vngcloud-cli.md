# 0001: greennode is the Rust port of the vngcloud CLI

## Status

Decisions 1 to 3 are accepted (owner, 2026-11-25). Decisions 4 to 7 are proposed
and wait for the owner's confirmation.

## Context

[vngcloud](https://github.com/dannyota/vngcloud) is a Go SDK and an AWS-style
command-line tool for GreenNode, formerly VNG Cloud. Its `v1.0.1` tag covers
about 370 operations in 19 services, and installing it needs a Go toolchain. The
provider's current name is GreenNode. The owner wants the command-line tool in
Rust, published on crates.io under the current name.

## Decision

1. The product is a Rust command-line tool named `greennode`: the crate, the
   binary, and the repository `dannyota/greennode`, public, with `master` as its
   default branch.
2. It is a port of the vngcloud CLI. vngcloud at tag `v1.0.1` is the baseline
   that defines commands, flags, output, and exit codes.
3. Releases go to crates.io. The owner publishes the first version from the
   owner's Mac; later versions publish from GitHub Actions through Trusted
   Publishing, as the [release design](../design/release.md) describes.
4. Proposed: the port moves in small releases. Each release adds whole commands
   that match the baseline. A command that is not ported yet is absent, never a
   stub.
5. Proposed: one crate. Its library target exists so the binary and its tests
   share code and is not a supported Rust SDK. Semantic versioning covers the
   command-line contract only.
6. Proposed: greennode uses its own names for everything a user sets: the
   `GREENNODE_` environment prefix and the `~/.greennode` directory. It reads no
   `VNGCLOUD_` variable and no file under `~/.vngcloud`. The file formats and
   keys match the baseline, so a user can copy the two files.
7. Proposed: the license is Apache-2.0, the same as vngcloud.

## Consequences

- Scripts written for vngcloud run on greennode after the command name and the
  environment prefix change, once their commands are ported.
- The two tools keep separate credentials and token caches, so neither can read
  or corrupt the other's files. A user who signs in with a one-time code in both
  tools within 30 seconds can have the second sign-in refused.
- A fix in vngcloud after `v1.0.1` reaches greennode only when the owner moves
  the baseline.
- No Rust program can depend on greennode as a library with a stability promise.

## Alternatives rejected

- **A drop-in replacement in one release.** The baseline is about 84,000 lines
  of Go without tests. Nothing would ship until all of it is ported and
  reviewed.
- **A wrapper that calls the Go binary.** It keeps the Go toolchain requirement
  the port exists to remove.
- **Reading `~/.vngcloud` and `VNGCLOUD_` variables as a fallback.** A fallback
  makes it unclear which account a command uses, and two programs would write
  one token cache.
- **A workspace with a separate SDK crate.** It doubles the published surface
  before any Rust caller exists. The modules can move into a second crate later
  without changing the command line.

## History

- 2026-11-25: Recorded. Decisions 1 to 3 accepted; decisions 4 to 7 proposed.
