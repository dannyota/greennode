# 0001: greennode is the Rust port of the vngcloud CLI

## Status

Accepted (owner, 2026-11-25).

## Context

[vngcloud](https://github.com/dannyota/vngcloud) is a Go SDK and an AWS-style
command-line tool for GreenNode, formerly VNG Cloud. Its `v1.0.1` tag covers
about 370 operations in 19 services, and installing it needs a Go toolchain. The
provider's current name is GreenNode. The owner wants the tool in Rust,
published on crates.io under the current name.

## Decision

1. The product is written in Rust and named `greennode`: the command, the
   command-line crate, and the repository `dannyota/greennode`, public, with
   `master` as its default branch.
2. It is a port of vngcloud. vngcloud at tag `v1.0.1` is the baseline that
   defines commands, flags, output, and exit codes.
3. Releases go to crates.io. The owner publishes the first version from the
   owner's Mac; later versions publish from GitHub Actions through Trusted
   Publishing, as the [release design](../design/release.md) describes.
4. The port moves in small releases. Each release adds whole commands that match
   the baseline. A command that is not ported yet is absent, never a stub.
5. Two crates. `greennode-sdk` is a supported Rust SDK, as vngcloud has a
   supported Go SDK. `greennode` is the command-line tool and calls the API only
   through the SDK's public interface.
6. greennode uses its own names for everything a user sets: the `GREENNODE_`
   environment prefix and the `~/.greennode` directory. Where a user has set
   nothing under those names, it falls back to the matching `VNGCLOUD_` variable
   and to the files under `~/.vngcloud`. The
   [configuration design](../design/config.md) owns the fallback rules.
7. The license is Apache-2.0, the same as vngcloud.

## Consequences

- A vngcloud user can run a ported greennode command with no setup, and a script
  written for vngcloud runs after its command name changes.
- Two crates publish for every release, the SDK first. Each needs its own first
  publish by hand and its own Trusted Publisher.
- The SDK's public types and functions are a second contract beside the command
  line, and both follow semantic versioning.
- greennode reads vngcloud's settings and credentials but never writes them, and
  each tool keeps its own token cache. A user who signs in with a one-time code
  in both tools within 30 seconds can have the second sign-in refused.
- A fix in vngcloud after `v1.0.1` reaches greennode only when the owner moves
  the baseline.

## Alternatives rejected

- **A drop-in replacement in one release.** The baseline is about 84,000 lines
  of Go without tests. Nothing would ship until all of it is ported and
  reviewed.
- **A wrapper that calls the Go binary.** It keeps the Go toolchain requirement
  the port exists to remove.
- **One crate with no supported library.** Proposed first. It publishes less,
  but a Rust program could not call GreenNode without running the command.
- **No fallback to vngcloud settings.** Proposed first. It never leaves the
  account in doubt, but every vngcloud user would configure greennode again
  before a first command works.
- **Sharing vngcloud's token cache.** Two programs would write one cache and
  depend on each other's file format and locking.

## History

- 2026-11-25: Recorded, with decisions 4 to 7 proposed as one crate, no
  fallback, and Apache-2.0.
- 2026-11-25: The owner accepted decisions 4 and 7, changed decision 5 to two
  crates with a supported SDK, and changed decision 6 to add the fallback.
