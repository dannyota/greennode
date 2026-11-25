# Foundation plan

Remaining work from an empty SDK and a one-command tool to the first crates.io release and parallel service porting. The [architecture](../design/architecture.md) owns the contract and [workflow](../../instructions/workflow.md) owns how a task runs. Delete a task when it lands and this file when all of it has.

## Shared files

The primary is the integration owner. Only the primary edits the workspace `Cargo.toml`, `Cargo.lock`, each crate's `Cargo.toml`, `crates/greennode-sdk/src/lib.rs`, `crates/greennode/src/services/mod.rs`, the `Makefile`, the workflows, and the AGENTS.md code map. A brief names the manifest lines its worker may add in its own worktree; the primary regenerates the lockfile when merging. A new shared type or a change to one goes through the primary before the tasks that depend on it.

## Order

Task 1 gates every task that builds on a design. Tasks 2 and 3 run beside it because the baseline's own code fixes their behavior. Task 4 follows 1. Tasks 5, 6, and 7 follow 4 and run in parallel. Task 8 follows 5 and 6. Task 9 follows 7 and 8. Task 10 follows 9. Task 11 can start after 1 and must land before the first release. Task 12 starts when 9 lands.

## Tasks

- [ ] **1. Design review.** Reviewer (Astra) reviews the eight designs in the [design index](../design/README.md) and [ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md) against the baseline. Accept: findings resolved, the designs' status lines removed, and the owner's answers recorded for the baseline tag and the SDK doc comment exception.
- [ ] **2. Parity list.** In review. Lands `scripts/parity-list.py`, its test, and `crates/greennode/tests/parity/commands.json`.
- [ ] **3. Output formatter and goldens.** Implementer. The Go-compatible JSON writer, the table and text renderers, and their escaping, as [CLI output](../design/cli-output.md) specifies, with the baseline's golden files copied under `crates/greennode/tests/fixtures/`. Baseline: `internal/cli/encode.go`, `render.go`, `redact_maps.go`, their tests, and `internal/cli/testdata/golden`. Owns `crates/greennode/src/cli/render.rs`, `crates/greennode/src/cli/render/`, `crates/greennode/tests/render.rs`, `crates/greennode/tests/fixtures/render/`. Accept: every copied golden matches byte for byte; no query engine yet.
- [ ] **4. Shared contracts.** Implementer. The SDK's public shapes with no I/O behind them: the request context and cancellation, the clock, `Secret`, the error class, error, failure, and write-dispatch types, the list and paged-list models, the configuration source trait, and the debug event types, as the [SDK design](../design/sdk.md) specifies. Owns `crates/greennode-sdk/src/context.rs`, `secret.rs`, `error.rs`, `model/`, `tests/core_contracts.rs`. Review (Sol) with canary tests for `Secret`. Everything after this task compiles against these types.
- [ ] **5. Configuration.** Implementer. The INI reader, profile and mode selection, the incomplete-pair refusal, file safety, the offline `resolve_profile` step, and the `load_resolved` step up to the point of choosing a credential source, as the [configuration design](../design/config.md) specifies. Baseline: `internal/ini/`, `internal/core/load.go`, `internal/core/cdnkey.go`. Owns `crates/greennode-sdk/src/config/`, `tests/core_config.rs`, `tests/fixtures/core/config/`. Adversarial review (Sol) against the design's test table.
- [ ] **6. Connection and product sender.** Implementer. The private socket and TLS layer, endpoint validation, and the product sender with its redirect, retry, bound, and event rules, plus the loopback test server, as the [request policies](../design/sdk-requests.md) specify. Baseline: `internal/transport/`, `internal/endpoints/`. Owns `crates/greennode-sdk/src/wire/connection.rs`, `wire/product/`, `src/endpoint/`, `tests/support/`, `tests/core_wire.rs`. Adversarial review (Sol).
- [ ] **7. CLI frame.** Implementer. The root, global flags, the operation declaration and registry, `--cli-input-json`, dispatch and guards, the error classifier and exit codes, the query adapter, the documentation generator, the parity test, and process injection, as the [CLI design](../design/cli.md) specifies, with one toy service in tests only. Baseline: `internal/cli/root.go`, `op.go`, `flags.go`, `input.go`, `errors.go`, `errors_exit.go`, `gendocs.go`, `cmd/vngcloud/main.go`. Owns `crates/greennode/src/main.rs`, `src/cli/` except the render files, `tests/cli.rs`, `tests/frame_*.rs`, `tests/parity.rs`. Review (Sol).
- [ ] **8. Sign-in, cache, and discovery.** Four implementers on disjoint paths, then one adversarial review (Sol) of the whole with a canary for every secret. The token cache with its lock and one-time-code marker (`src/cache/`; baseline `internal/tokencache/`). The service-account exchange (`src/wire/credential/service_account.rs`; baseline `internal/core/service_account_*.go`). The IAM user sign-in with one-time codes (`src/wire/credential/iam_user.rs`; baseline `internal/iamuser/`). The session, provider contract, refresh gates, and project discovery (`src/session/`, `src/discovery/`; baseline `internal/core/`, `internal/discovery/`).
- [ ] **9. First service and configure.** Two implementers in parallel. One: billing reads in the SDK and their commands in the tool, as the template every service follows, with the generated documentation page. The other: `configure` with `set`, `get`, and `list`, including the refusal in legacy mode; baseline `internal/cli/configure*.go`, `configfile.go`; adversarial review (Sol). Afterwards the primary updates [porting](../../instructions/porting.md) with the pattern and asks the owner to approve one read-only live run.
- [ ] **10. First release.** The primary does the one-time GitHub setup. The owner approves the `v0.1.0` tag and publishes both crates by hand as the [release design](../design/release.md) describes, then adds the two Trusted Publishers.
- [ ] **11. Release tooling.** Implementer. `release.yml`, `publish-crates.yml`, the release checker with its receipt and registry-state commands, the checksum-checking credential helper, the manifest gate in `make check`, and their tests, ported from duolane and extended for two crates as the [release design](../design/release.md#implementation-boundary) lists. Adversarial review (Sol). Accept: the loopback-registry test in the design's unverified list passes on macOS and Ubuntu.
- [ ] **12. Services.** In waves. Before each wave the primary registers the wave's services in both crates and lands any shared model or quote and discovery code they need. One implementer and one worktree per service or slice, reads before writes, each write with adversarial review. A slice owns its service's paths in both crates as the [layout](../design/architecture.md#layout) lists them.

## Waves

Smallest first, so the pattern settles on cheap services. Lines are the baseline's Go source without tests. Large services are split by concern into slices of about 1,500 lines, each with its own brief.

|Wave|Services|Operations|Go lines|Needs first|
|-|-|-|-|-|
|1|`project`, `backup`, `portal`, `tagging`, `vks`|16|1,718|Nothing beyond task 9|
|2|`database`, `containerregistry`, `dns`, `billing` writes|35|5,300|The write conventions proven on billing|
|3|`pricing`, `volume`, `globalloadbalancer`, `cdn`|55|8,995|Shared quote and discovery code; the vCDN key path|
|4|`compute`, `iam`, `loadbalancer`|102|14,047|The shared server model|
|5|`storage`, `monitor`, `network`|154|25,856|The S3 sender for storage objects; compute and pricing for network|
