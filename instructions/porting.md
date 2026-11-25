# Porting

Rules for moving a command or service from the Go SDK and CLI to Rust. The [architecture](../docs/design/architecture.md) owns the contract; this file says how to work from the Go source.

## Baseline

The baseline is [vngcloud](https://github.com/dannyota/vngcloud) at tag `v1.0.1`, commit `d784efc9241adcc4ed31bcab21e80dfd4552aab5`. `make reference` clones that tag into `.reference/vngcloud` in the checkout where it runs. The directory is git-ignored and read-only. A brief gives its absolute path, because a worktree does not contain it.

Read only the tag. A newer vngcloud commit is not the contract until the owner moves the baseline and the [architecture](../docs/design/architecture.md#baseline) records it.

|To learn|Read in the baseline|
|-|-|
|What an operation promises|`docs/design/<domain>.md` and its `-api.md` and `-checks.md` siblings|
|Command name, flags, kind, and help text|`docs/wiki/CLI-<Service>.md` and the operation table in `internal/cli/<service>*.go`|
|The SDK surface, request method, path, query, body, and decoding|The service package at the repository root, such as `billing/`|
|Config, credentials, login, token cache|`internal/core/`, `internal/ini/`, `internal/tokencache/`, `docs/design/sdk-and-cli.md`, `docs/design/service-account-auth.md`|
|HTTP rules, retries, redirects, limits, debug logs|`internal/transport/`|
|Output, query, errors, exit codes, guards|`internal/cli/` and `docs/design/cli.md`|
|Expected behavior, case by case|The `_test.go` files next to each source file|
|Real response shapes|`testdata/<service>/`|

## What must match

- Service and operation names, flag names, flag types, and which flags are required.
- JSON output: key names, nesting, value types, and key order where the baseline fixes it. `--query` paths that work in the baseline work unchanged.
- Table and text output for the same data.
- The stderr error document and the exit code for each failure class.
- Guards: `--read-only`, `--yes`, `--max-price`, and every refusal that happens before a request.
- The request each operation sends: method, path, query, headers the API needs, and body.
- Safety behavior: redirects refused, response size limits, the retry rule, and what happens on an unconfirmed write.
- The SDK surface: one client per service, one method per Go method with the same name in snake case, one input type and one output type per method with the Go type's fields, and the same error identities. The [SDK design](../docs/design/sdk.md) owns the shape.

## What differs

The [architecture](../docs/design/architecture.md#differences-from-the-baseline) lists every intended difference. Anything else that differs is a defect. When a faithful port looks wrong or unsafe, stop and report it with the baseline `path:line`; the primary decides and records the answer in the design.

## How to port

1. Read the baseline's design for the operation, then its Go source and tests.
2. Copy the sanitized fixtures the tests use from `testdata/<service>/` to `crates/greennode-sdk/tests/fixtures/<service>/` unchanged. Never copy from `examples/`, never copy a `.env`, and never take values from a live run.
3. Port the baseline's tests for the operation as the failing tests. SDK tests cover the request shape, decoding, and each documented error status. Tool tests cover flags, the printed output, each guard, and the exit code.
4. Write idiomatic Rust that passes them, the SDK method first and then the command that calls it. Port behavior, not structure: no reflection, no package-level state, no line-by-line translation of Go idioms.
5. Check the operation against the parity test, which compares its name, kind, and flags with the baseline.
6. List in the report anything the baseline does that the tests do not prove.

## Limits of a port

Fixtures and mocks prove the port matches the Go code. They do not prove the GreenNode API still behaves that way. State which claims rest only on the baseline, and leave live verification to the primary under the [security rules](security.md#live-calls).
