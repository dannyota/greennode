# Architecture

Status: proposed. An independent design review and the owner's answer to
[ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md) come before the core
work in the plan starts.

`greennode` is one binary that inspects and changes GreenNode resources from a
terminal or a script, in the style of the AWS CLI. It is the Rust port of the Go
CLI in vngcloud. Users install it with Cargo or from a release archive, with no
Go toolchain.

## Baseline

The baseline is [vngcloud](https://github.com/dannyota/vngcloud) at tag
`v1.0.1`, commit `d784efc9241adcc4ed31bcab21e80dfd4552aab5`. Its
`docs/design/cli.md` and `docs/design/sdk-and-cli.md` define the contract this
port keeps, and its per-domain designs define each operation. Only the owner
moves the baseline to a newer tag, and that change updates this section.

The baseline has about 370 operations in 19 services:

| Service              | Operations | Service             | Operations |
| -------------------- | ---------- | ------------------- | ---------- |
| `network`            | 71         | `dns`               | 10         |
| `monitor`            | 48         | `containerregistry` | 9          |
| `iam`                | 38         | `database`          | 9          |
| `storage`            | 35         | `portal`            | 5          |
| `loadbalancer`       | 33         | `vks`               | 5          |
| `compute`            | 31         | `tagging`           | 3          |
| `volume`             | 21         | `backup`            | 2          |
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
  `--query`, `--yes`, `--read-only`, and `--debug`.
- JSON on stdout by default. Keys are the baseline's Go field names, such as
  `Items[].Name`, not the raw API names. A list prints an object with its rows
  under `Items`.
- `--query` with JMESPath on that JSON, and `--output table` and `text`
  rendering the query result.
- Errors on stderr as one JSON line with `code`, `message`, and for an API error
  `status` and `operation`.
- Exit codes: 0 success, 1 API or network error or a cancelled command, 2 usage
  or configuration error, 3 missing or refused credentials, 4 not found.
- Guards that stop before any request: a write under read-only, a destructive
  command without `--yes`, and a quoted price above `--max-price`.
- The config and credentials file format, the profile rules, and the precedence
  of flags, environment, and files.
- The request each operation sends and the safety rules around it, which the
  [security design](security.md) lists.

## Differences from the baseline

| Topic                      | Baseline                                | greennode                                               |
| -------------------------- | --------------------------------------- | ------------------------------------------------------- |
| Command name               | `vngcloud`                              | `greennode`                                             |
| Environment prefix         | `VNGCLOUD_`                             | `GREENNODE_`                                            |
| Config, credentials, cache | `~/.vngcloud/`                          | `~/.greennode/`                                         |
| Version line               | `vngcloud <version> go<go> <os>/<arch>` | `greennode <version> <os>/<arch>`                       |
| Library                    | Public Go SDK                           | No supported library interface                          |
| Install                    | `go install`                            | `cargo install greennode --locked` or a release archive |
| Unported commands          | Not applicable                          | Absent from help until ported                           |

Error and help text names `greennode` and `GREENNODE_` wherever the baseline
names `vngcloud` and `VNGCLOUD_`. Any other difference is a defect.
[ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md) records why.

## Layout

| Path                        | Owns                                                                                                                                    |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `src/main.rs`               | The only code that reads the process: arguments, environment, home directory, clock, and streams                                        |
| `src/cli/`                  | Root command, global flags, guards, output formats, `--query`, the error document, exit codes, `configure`, and generated documentation |
| `src/core/`                 | Profiles and precedence, credentials, sign-in methods, the token cache, project discovery, and the shared error type                    |
| `src/transport/`            | The HTTP client: TLS, redirect refusal, size limits, retries, and debug logging                                                         |
| `src/services/<service>/`   | One service: its operations, flags, request building, models, and output                                                                |
| `tests/<service>.rs`        | Behavior tests for one service against a loopback server                                                                                |
| `tests/fixtures/<service>/` | Sanitized responses copied from the baseline's `testdata/`                                                                              |
| `tests/parity/`             | The baseline's command list and the test that compares against it                                                                       |

Dependencies point one way: `cli` uses `services`, `core`, and `transport`;
`services` use `core` and `transport`; `core` uses `transport`. A service never
reaches into another service's internals. A model two services share belongs to
the service that owns the resource.

A service directory is the unit of parallel work. One implementer owns one
service's source, tests, and fixtures, and no shared file needs to change to add
an operation to a service that is already registered.

## Request flow

1. `main` collects the arguments, environment, home directory, clock, and
   streams and passes them to `cli`.
2. `cli` parses the command. A usage error exits 2 before anything else runs.
3. Guards refuse a write under read-only and a destructive command without
   `--yes`.
4. `core` resolves the profile, region, project, and one credential source.
5. The operation asks `core` for a token. `core` reuses a cached token or signs
   in, under the cache lock.
6. The operation sends its requests through `transport` and decodes the response
   into its output type.
7. `cli` applies `--query`, renders the output format, and writes stdout.
8. An error becomes the stderr document and one exit code.

## Invariants

- **One source of process state.** Only `src/main.rs` reads the environment, the
  home directory, the clock, or the standard streams. Everything else receives
  them, so tests run every path without touching the machine.
- **One declaration per operation.** Each operation declares its name, its kind
  (read, write, or destructive), and its flags once. Parsing, guards, help,
  generated documentation, and the parity test all read that declaration.
- **Guards run before requests.** A refused command sends nothing, including no
  sign-in.
- **One HTTP path.** Every request goes through `transport`, which enforces the
  [secure defaults](security.md#secure-defaults).
- **Secrets stay out of output.** No password, one-time-code secret, token,
  authorization code, cookie, or `Authorization` header reaches stdout, stderr,
  an error, or a debug log.
- **Results on stdout, everything else on stderr.** A script can parse stdout
  without filtering.
- **Never prompt without a terminal.** A command that needs confirmation fails
  with a message naming the flag.

## Parity

`tests/parity/` holds the baseline's command list: every service, operation,
kind, and flag with its type and required state, generated from the baseline's
`docs/wiki/CLI-*.md` pages. One test compares each command greennode registers
with its entry and fails on any difference. The same list shows what is left to
port.

Parity of names and flags is checked by that test. Parity of behavior is checked
by porting the baseline's tests for each operation, as
[porting](../../instructions/porting.md) describes. Neither proves the GreenNode
API still behaves as the baseline recorded; only an approved live run does.

## Dependencies

| Crate  | Reason                           |
| ------ | -------------------------------- |
| `clap` | Nested commands, flags, and help |

Each further crate is added by the work that needs it and recorded here with its
reason, after the checks in
[security](../../instructions/security.md#dependencies). The expected needs are
JSON, an async runtime with an HTTP client built on rustls, JMESPath, HMAC and
SHA for one-time codes and S3 signing, and terminal input without echo. The
choice of each crate is part of the design review.

## Versions

Versions before `1.0.0` may change the command line. `1.0.0` is the release in
which every baseline operation is ported; from then on a change to a command
name, flag, query key, or exit code needs a new major version.
