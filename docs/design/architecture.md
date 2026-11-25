# Architecture

Status: proposed. An independent design review comes before the core work in the
plan starts. [ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md) holds the
owner's decisions this design builds on.

greennode is the Rust port of vngcloud. It has two parts: `greennode-sdk`, a
library that calls the GreenNode API, and `greennode`, a command-line tool in
the style of the AWS CLI that inspects and changes resources from a terminal or
a script. Users install the tool with Cargo or from a release archive, with no
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

| Topic                  | Baseline                                | greennode                                                                                      |
| ---------------------- | --------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Command name           | `vngcloud`                              | `greennode`                                                                                    |
| Environment prefix     | `VNGCLOUD_`                             | `GREENNODE_`, then `VNGCLOUD_` as a [fallback](#fallback-to-vngcloud-settings)                 |
| Config and credentials | `~/.vngcloud/`                          | `~/.greennode/`, then `~/.vngcloud/` as a read-only [fallback](#fallback-to-vngcloud-settings) |
| Token cache            | `~/.vngcloud/cache`                     | `~/.greennode/cache` only                                                                      |
| Version line           | `vngcloud <version> go<go> <os>/<arch>` | `greennode <version> <os>/<arch>`                                                              |
| Library                | Go module `danny.vn/vngcloud`           | Rust crate `greennode-sdk`                                                                     |
| Install                | `go install`                            | `cargo install greennode --locked` or a release archive                                        |
| Unported commands      | Not applicable                          | Absent from help until ported                                                                  |

Error and help text names `greennode` and `GREENNODE_` wherever the baseline
names `vngcloud` and `VNGCLOUD_`. Any other difference is a defect.
[ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md) records why.

## Fallback to vngcloud settings

A vngcloud user can run greennode with no setup. The baseline's precedence stays
as it is: flags, then environment, then files. Inside the environment step and
inside the files step, greennode's own name comes first and vngcloud's name
fills what is missing. The SDK applies these rules when it loads a
configuration, so a program that uses the SDK behaves like the tool.

Environment:

- A setting falls back one variable at a time. When `GREENNODE_PROFILE`,
  `GREENNODE_REGION`, `GREENNODE_PROJECT_ID`, `GREENNODE_CONFIG_FILE`,
  `GREENNODE_SHARED_CREDENTIALS_FILE`, or `GREENNODE_VCDN_API_KEY` is unset or
  empty, the `VNGCLOUD_` variable of the same name is used.
- Read-only is on when either `GREENNODE_READ_ONLY` or `VNGCLOUD_READ_ONLY`
  turns it on. Neither can turn it off for the other.
- Credential variables fall back as a set, never one at a time. When any
  `GREENNODE_` credential variable is non-empty, the environment's credentials
  are the `GREENNODE_` set and no `VNGCLOUD_` credential variable is read.
  Otherwise they are the `VNGCLOUD_` set. The credential variables are
  `ROOT_EMAIL`, `USERNAME`, `PASSWORD`, `TOTP_SECRET`, `ACCESS_TOKEN`,
  `CLIENT_ID`, and `CLIENT_SECRET`.

Files:

- The config and credentials files are read as a pair from one directory. When
  `~/.greennode/config` or `~/.greennode/credentials` exists, both come from
  `~/.greennode` and nothing under `~/.vngcloud` is read. Otherwise both come
  from `~/.vngcloud`.
- A path named by an option or by a `CONFIG_FILE` or `SHARED_CREDENTIALS_FILE`
  variable replaces that one file.
- The baseline's file safety rules apply to whichever files are read.
- greennode never writes under `~/.vngcloud`. `configure` writes `~/.greennode`.
  When the pair in use is vngcloud's, `configure` first copies both files into
  `~/.greennode` with the baseline's file modes and says so on stderr, so the
  profiles in use do not disappear after the first write.
- The token cache is always `~/.greennode/cache`. greennode never reads or
  writes vngcloud's cache.

With `--debug`, the tool logs which prefix and which directory supplied the
settings, and never a value.

## Layout

| Path                                         | Owns                                                                                                            |
| -------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `crates/greennode-sdk/src/`                  | Configuration loading, credentials, sign-in, the token cache, project discovery, the HTTP transport, and errors |
| `crates/greennode-sdk/src/<service>/`        | One service: its client, inputs, outputs, and requests                                                          |
| `crates/greennode-sdk/tests/<service>.rs`    | Behavior tests for one service against a loopback server                                                        |
| `crates/greennode-sdk/tests/fixtures/`       | Sanitized responses copied from the baseline's `testdata/`, one directory per service                           |
| `crates/greennode/src/main.rs`               | The tool's only code that reads the process: arguments, environment, home directory, clock, and streams         |
| `crates/greennode/src/cli/`                  | Root command, global flags, guards, output formats, `--query`, the error document, exit codes, and `configure`  |
| `crates/greennode/src/services/<service>.rs` | One service's commands: flags, the SDK call, and the output                                                     |
| `crates/greennode/tests/<service>.rs`        | Command tests for one service                                                                                   |
| `crates/greennode/tests/parity/`             | The baseline's command list and the test that compares against it                                               |

The tool calls the API only through the SDK's public interface, as the baseline
does. When a command needs something the SDK lacks, the SDK gains it first.

A service is the unit of parallel work. One implementer owns one service's
directory and tests in the SDK and its command file and tests in the tool, and
no shared file changes to add an operation to a registered service.

## The SDK

The SDK mirrors the baseline's Go SDK in Rust form:

- A configuration value holds the region, the project, the credential source,
  endpoint overrides, and retry settings. One function loads it from options,
  the environment, and the profile files; another builds it from options only
  and reads nothing.
- Each service has a client built from a configuration. Clients built from one
  configuration share one sign-in and one project discovery.
- Each operation is one method that takes one input value and returns one output
  value or the shared error type. Output types serialize to the baseline's JSON
  field names, and the tool prints that form.
- The SDK never prints, prompts, reads stdin, or exits. It never writes the
  config or credentials file; only `configure` in the tool does.
- The SDK depends on nothing a caller of the API does not need: no argument
  parser, no query language, and no terminal handling.

## Request flow

1. `main` collects the arguments, environment, home directory, clock, and
   streams and passes them to `cli`.
2. `cli` parses the command. A usage error exits 2 before anything else runs.
3. Guards refuse a write under read-only and a destructive command without
   `--yes`.
4. The SDK resolves the profile, region, project, and one credential source.
5. The operation asks for a token. The SDK reuses a cached token or signs in,
   under the cache lock.
6. The SDK sends the requests through its transport and decodes the response
   into the output type.
7. `cli` applies `--query`, renders the output format, and writes stdout.
8. An error becomes the stderr document and one exit code.

## Invariants

- **One source of process state.** In the tool, only `main.rs` reads the
  environment, the home directory, the clock, or the standard streams. In the
  SDK, only the default source behind configuration loading reads the
  environment and the home directory. Everything else receives them, so tests
  run every path without touching the machine.
- **One declaration per operation.** Each command declares its name, its kind
  (read, write, or destructive), and its flags once. Parsing, guards, help,
  generated documentation, and the parity test all read that declaration.
- **Guards run before requests.** A refused command sends nothing, including no
  sign-in.
- **One HTTP path.** Every request goes through the SDK's transport, which
  enforces the [secure defaults](security.md#secure-defaults).
- **Secrets stay out of output.** No password, one-time-code secret, token,
  authorization code, cookie, or `Authorization` header reaches stdout, stderr,
  an error, or a debug log.
- **Results on stdout, everything else on stderr.** A script can parse stdout
  without filtering.
- **Never prompt without a terminal.** A command that needs confirmation fails
  with a message naming the flag.

## Parity

`crates/greennode/tests/parity/` holds the baseline's command list: every
service, operation, kind, and flag with its type and required state, generated
from the baseline's `docs/wiki/CLI-*.md` pages. One test compares each command
greennode registers with its entry and fails on any difference. The same list
shows what is left to port.

Parity of names and flags is checked by that test. Parity of behavior is checked
by porting the baseline's tests for each operation, as
[porting](../../instructions/porting.md) describes. Neither proves the GreenNode
API still behaves as the baseline recorded; only an approved live run does.

## Dependencies

| Crate  | Used by     | Reason                           |
| ------ | ----------- | -------------------------------- |
| `clap` | `greennode` | Nested commands, flags, and help |

Each further crate is added by the work that needs it and recorded here with its
reason, after the checks in
[security](../../instructions/security.md#dependencies). The expected needs are
JSON, an async runtime with an HTTP client built on rustls, and HMAC and SHA for
one-time codes and S3 signing in the SDK; JMESPath and terminal input without
echo in the tool. The choice of each crate is part of the design review.

## Versions

Both crates carry one version and release together under one tag. The tool
depends on that exact SDK version.

Versions before `1.0.0` may change the command line and the SDK's interface.
`1.0.0` is the release in which every baseline operation is ported. From then
on, a change to a command name, flag, query key, or exit code, or a breaking
change to a public SDK item, needs a new major version.
