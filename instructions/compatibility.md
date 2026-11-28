# Compatibility

Rules for building a command or service that is compatible with the Go SDK and CLI. The [architecture](../docs/design/architecture.md) owns the contract; this file says how to work from the Go source.

## Baseline

The baseline is [vngcloud](https://github.com/dannyota/vngcloud) at tag `v1.8.0`, commit `85458dfb6b373116be410ec8be845257ad545f96`. `make reference` clones that tag into `.reference/vngcloud` in the checkout where it runs. The directory is git-ignored and read-only. A brief gives its absolute path, because a worktree does not contain it.

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

The [architecture](../docs/design/architecture.md#differences-from-the-baseline) lists every intended difference. Anything else that differs is a defect. When matching the baseline looks wrong or unsafe, stop and report it with the baseline `path:line`; the primary decides and records the answer in the design.

## How to add an operation

1. Read the baseline's design for the operation, then its Go source and tests.
2. Copy the sanitized fixtures the tests use from `testdata/<service>/` to `crates/greennode-sdk/tests/fixtures/<service>/` unchanged. Never copy from `examples/`, never copy a `.env`, and never take values from a live run.
3. Adopt the baseline's tests for the operation as the failing tests. SDK tests cover the request shape, decoding, and each documented error status. Tool tests cover flags, the printed output, each guard, and the exit code.
4. Write idiomatic Rust that passes them, the SDK method first and then the command that calls it. Match behavior, not structure: no reflection, no package-level state, no line-by-line translation of Go idioms.
5. Check the operation against the parity test, which compares its name, kind, and flags with the baseline.
6. List in the report anything the baseline does that the tests do not prove.

Billing is the template; copy its shape:

- SDK: `src/<service>/` with `Client::new(&Config)`, one file per group of operations, named inputs and outputs, models that deserialize from wire names and serialize with Go's output names, and the envelope private to the service. Each call is an operation declaration on the crate-private operation client with an output schema; reflection checks and test trust come from the shared code, never the service.
- CLI: `src/services/<service>.rs` or `<service>/`, one input descriptor per operation with `pre_guard` set, registered in `services/mod.rs`; output keys, table columns and text come from the SDK projection.
- Goldens: JSON, table and text for every command, generated in-process from the baseline with `scripts/billing-goldens/` (copy it per service), stored under `crates/greennode/tests/fixtures/<service>/`.
- Tests: SDK loopback tests per operation; CLI in-process tests that build `Config` through the SDK `test-support` feature and compare stdout with the goldens; built-binary tests only for offline paths.
- Finish with `shipped-commands.json` entries and the `docs/wiki/CLI-<Service>.md` page from `gen-docs`.

## Limits of fixture tests

Fixtures and mocks prove greennode matches the Go code. They do not prove the GreenNode API still behaves that way. State which claims rest only on the baseline, and leave live verification to the primary under the [security rules](security.md#live-calls).
