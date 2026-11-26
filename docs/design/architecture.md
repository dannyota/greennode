# Architecture

[ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md) holds the owner's
decisions this design builds on.

greennode is the Rust port of vngcloud. It has two parts: `greennode-sdk`, a
library that calls the GreenNode API, and `greennode`, a command-line tool in
the style of the AWS CLI that inspects and changes resources from a terminal or
a script. Users install the tool with Cargo or from a release archive, with no
Go toolchain.

This document owns the contract with the baseline and how the parts fit. Each
part has its own design:

| Design                                  | Owns                                                                        |
| --------------------------------------- | --------------------------------------------------------------------------- |
| [Configuration](config.md)              | Settings, profiles, precedence, the fallback to vngcloud, and file safety   |
| [SDK](sdk.md)                           | The public interface of `greennode-sdk`, results, errors, and outputs       |
| [SDK request policies](sdk-requests.md) | The three kinds of request, their limits, and the SDK's crates              |
| [Command line](cli.md)                  | Command declarations, guards, dispatch, errors, exit codes, and `configure` |
| [CLI output](cli-output.md)             | The JSON projection, table, and text                                        |
| [CLI query](cli-query.md)               | The `--query` engine, a port of go-jmespath v0.4.0                          |
| [Security](security.md)                 | Trust boundaries, secure defaults, and threats                              |
| [Release publishing](release.md)        | GitHub Releases and crates.io publishing                                    |

## Baseline

The baseline is [vngcloud](https://github.com/dannyota/vngcloud) at tag
`v1.2.0`, commit `f99c71490b1006a95a6dac7b0eac7f182b34e917`. Its
`docs/design/cli.md` and `docs/design/sdk-and-cli.md` define the contract this
port keeps, and its per-domain designs define each operation. Only the owner
moves the baseline to a newer tag. A move updates this section, the baseline
values in the `Makefile`, and the parity list, and the plan gains a task to port
what changed between the two tags.

The baseline has 373 operations in 19 services:

| Service              | Operations | Service             | Operations |
| -------------------- | ---------- | ------------------- | ---------- |
| `network`            | 71         | `dns`               | 10         |
| `monitor`            | 48         | `containerregistry` | 9          |
| `iam`                | 38         | `database`          | 9          |
| `storage`            | 35         | `portal`            | 5          |
| `loadbalancer`       | 33         | `vks`               | 5          |
| `compute`            | 31         | `tagging`           | 3          |
| `volume`             | 21         | `backup`            | 3          |
| `billing`            | 17         | `pricing`           | 2          |
| `cdn`                | 16         | `project`           | 1          |
| `globalloadbalancer` | 16         |                     |            |

The counts are the operation headings in the baseline's generated
`docs/wiki/CLI-<Service>.md` pages.

## The contract greennode keeps

For every ported command, these match the baseline:

- The shape `greennode <service> <operation> [flags]`, the service and operation
  names, and each flag's name, type, and required state.
- The global flags `--profile`, `--region`, `--project-id`, `--output`,
  `--query`, `--yes`, `--read-only`, and `--debug`, and `--cli-input-json` on
  every operation that accepts it, including the input fields that only it can
  set.
- JSON on stdout by default. Keys are the baseline's Go field names, such as
  `Items[].Name`, not the raw API names. A list prints an object with its rows
  under `Items`; an operation whose baseline output has another shape keeps that
  shape.
- `--query` with JMESPath on that JSON, and `--output table` and `text`
  rendering the query result.
- Errors on stderr as one JSON line with `code`, `message`, and for an API error
  `status` and `operation`. Where the baseline prints an output together with an
  error, greennode prints both.
- Exit codes: 0 success, 1 API or network error or a cancelled command, 2 usage
  or configuration error, 3 missing or refused credentials, 4 not found, with
  the baseline's precedence among them.
- Local guards that send nothing: a write under read-only, a destructive command
  without `--yes`, and missing required input. A paid write compares a quote
  with `--max-price` before ordering; that check can sign in and read but sends
  no change.
- The config and credentials file format, the profile rules, and the precedence
  of flags, environment, and files.
- The request each operation sends and the safety rules around it.

## Differences from the baseline

| Topic                      | Baseline                                | greennode                                                                                                                                                                    |
| -------------------------- | --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Command name               | `vngcloud`                              | `greennode`                                                                                                                                                                  |
| Environment prefix         | `VNGCLOUD_`                             | `GREENNODE_`, then `VNGCLOUD_`, as [configuration](config.md) defines                                                                                                        |
| Config and credentials     | `~/.vngcloud/`                          | `~/.greennode/`, or `~/.vngcloud/` read-only while greennode has no files of its own                                                                                         |
| Editing vngcloud's files   | `configure` writes them                 | Never; `configure` refuses and gives the copy commands                                                                                                                       |
| Token cache                | `~/.vngcloud/cache`                     | `~/.greennode/cache` only                                                                                                                                                    |
| Platforms                  | macOS, Linux and Windows                | Full support on macOS and Linux. Windows builds and runs, but keeps tokens in memory only and gets no platform-specific work or tests                                        |
| Version line               | `vngcloud <version> go<go> <os>/<arch>` | `greennode <version> <os>/<arch>`                                                                                                                                            |
| Help and usage text        | cobra's wording                         | clap's wording; names, streams, and exit codes unchanged                                                                                                                     |
| Library                    | Go module `danny.vn/vngcloud`           | Rust crate `greennode-sdk`                                                                                                                                                   |
| Install                    | `go install`                            | `cargo install greennode --locked` or a release archive                                                                                                                      |
| Unported commands          | Not applicable                          | Absent from help until ported                                                                                                                                                |
| Other intended differences |                                         | Listed in the [configuration](config.md#differences-from-the-baseline), [SDK](sdk.md#differences-from-the-baseline), and [CLI](cli.md#differences-from-the-baseline) designs |

Error and help text names `greennode` and `GREENNODE_` wherever the baseline
names `vngcloud` and `VNGCLOUD_`. Any other difference is a defect.
[ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md) records why.

## Layout

| Path                                         | Owns                                                                                            |
| -------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `crates/greennode-sdk/src/`                  | Shared core: configuration, session, token cache, discovery, senders, errors, and shared models |
| `crates/greennode-sdk/src/<service>/`        | One service: its client, inputs, outputs, and requests                                          |
| `crates/greennode-sdk/tests/`                | Core tests, one test file and fixture directory per service, and the loopback test server       |
| `crates/greennode/src/main.rs`               | The tool's only code that reads the process                                                     |
| `crates/greennode/src/cli/`                  | The frame: root, declarations, input, dispatch, output, query, errors, and `configure`          |
| `crates/greennode/src/services/<service>.rs` | One service's commands                                                                          |
| `crates/greennode/tests/`                    | Frame tests, one test file per service, output goldens, and the parity list                     |

The [SDK](sdk.md#layout-and-ownership) and [CLI](cli.md#layout-parity-and-tests)
designs list every path and its owner. The tool calls the API only through the
SDK's public interface. When a command needs something the SDK lacks, the SDK
gains it first.

A service is the unit of parallel work. Once a service is registered, adding an
operation changes only that service's files in both crates. Shared files have
one integration owner: the workspace manifests, the lockfile, each crate's
service registration, the `Makefile`, and the workflows.

## Request flow

1. `main` collects the arguments, an environment view, the home directory,
   streams, a terminal handle, and a running clock, and passes them to `cli`.
2. `cli` parses the command and merges `--cli-input-json` with flags. A usage
   error exits 2 before anything else runs.
3. Local guards run: read-only from the flag and environment, required input,
   the `--query` syntax, and `--yes`.
4. The SDK loads the configuration offline in one call: the profile, both
   selected files, and one credential source. A load error stops here. Then the
   profile's read-only setting is applied. Nothing has signed in or sent a
   request yet.
5. The command calls its CLI adapter, usually one SDK operation and sometimes
   several with stated cleanup, which signs in or reuses a cached token and
   sends the requests.
6. `cli` projects the output, applies `--query`, renders the format, and writes
   stdout.
7. A failure becomes the stderr document and one exit code. The SDK keeps any
   partial output that came with it, and the
   [CLI design](cli.md#errors-and-exit-codes) decides which are printed.

## Invariants

- **One source of process state.** In the tool, only `main.rs` reads the
  environment, the home directory, the clock, or the standard streams. In the
  SDK, only the default configuration source reads the environment and the home
  directory. Everything else receives them, so tests run every path without
  touching the machine.
- **One declaration per operation.** Each command declares its name, its kind,
  and its inputs once. Parsing, guards, help, generated documentation, and the
  parity test all read that declaration.
- **Local guards send nothing.** A command refused by read-only, a missing
  `--yes`, or missing input sends no request, including no sign-in.
- **Three kinds of request, never mixed.** Credential exchange, product
  requests, and signed S3 object requests have separate senders with separate
  rules. No caller hook sees a credential exchange, and S3 signing never sees
  management credentials.
- **A write's outcome is never guessed.** Once a change may have been sent, the
  result says so through every later error, and nothing resends it silently.
- **greennode's own credentials stay out of output.** No password, one-time-code
  secret, client secret, token, authorization code, cookie, API key, or
  `Authorization` header that greennode uses to sign in or to send a request
  reaches stdout, stderr, an error, a debug event, or a query. Resource data the
  API returns for a read or a write is printed as the baseline prints it, unless
  it contains a credential greennode sent in that request; the
  [CLI output design](cli-output.md) lists the fields and the baseline's own
  redaction rules.
- **Results on stdout, everything else on stderr.** A script can parse stdout
  without filtering.
- **Never prompt without a terminal.** A command that needs confirmation fails
  with a message naming the flag.

## Parity

`crates/greennode/tests/parity/commands.json` holds the baseline's command list:
every service, operation, kind, flag, and input field that only
`--cli-input-json` can set, generated from the baseline's `docs/wiki/CLI-*.md`
pages at the pinned commit. One test compares each command greennode registers
with its entry and fails on any difference. The same list shows what is left to
port.

The list proves names, kinds, and stated flags. It does not state defaults,
mutually exclusive flags, global flags, `configure`, or `version`; the
[CLI design](cli.md#layout-parity-and-tests) covers those with their own tests.
Behavior is checked by porting the baseline's tests for each operation, as
[porting](../../instructions/porting.md) describes, and output bytes by
[goldens](cli-output.md#goldens) taken from the baseline. None of this proves
the GreenNode API still behaves as the baseline recorded; only an approved live
run does.

## Dependencies

Each crate is added by the work that needs it, after the checks in
[security](../../instructions/security.md#dependencies). The vetted choices and
their reasons are in the [SDK request policies](sdk-requests.md#dependencies),
the [CLI design](cli.md#debug-and-dependencies), and the
[CLI query design](cli-query.md). The SDK depends on nothing a caller of the API
does not need: no argument parser, no query language, and no terminal handling.

## Versions

Both crates carry one version and release together under one tag. The tool
depends on that exact SDK version.

Versions before `1.0.0` may change the command line and the SDK's interface, but
a command that shipped in one release does not disappear in the next. `1.0.0` is
the release in which every baseline operation is ported. From then on, a change
to a command name, flag, query key, or exit code, or a breaking change to a
public SDK item, needs a new major version.
