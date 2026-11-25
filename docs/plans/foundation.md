# Foundation plan

Remaining work from an empty SDK and a one-command tool to the first crates.io release and parallel service porting. The [architecture](../design/architecture.md) owns the contract and [workflow](../../instructions/workflow.md) owns how a task runs. Delete a task when it lands and this file when all of it has.

## Shared files

The primary is the integration owner of the workspace `Cargo.toml`, `Cargo.lock`, each crate's `Cargo.toml`, `crates/greennode-sdk/src/lib.rs`, `crates/greennode/src/cli.rs` and `src/services/mod.rs`, the `Makefile`, the workflows, and the AGENTS.md code map. A brief may hand a worker named lines or one named shared file for that task only; the brief says exactly which. The primary regenerates the lockfile when merging. A new shared type, or a change to one, lands before the tasks that depend on it.

## Order

Task 1 can run at any time; from then on every SDK task writes its doc comments with its code. Task 2 started before it and is reworked if the review changes the projection rules. After 1: task 3, and task 4 once task 2 has landed. After 3: tasks 5, 6, and 7 in parallel. After 3, 4, and 7: task 8. After 5 and 6: task 9. After 8 and 9: task 10. After 10: task 11, then 12. Task 13 starts after 1 and lands before the first release. Task 14 starts when 11 lands.

## Tasks

- [ ] **3. Shared contracts.** Implementer. The SDK's public shapes with no I/O behind them: the request context and cancellation, the clock, `Secret`, `Token`, the error class, error, failure, and write-dispatch types, the list and paged-list models, the configuration source trait, and the debug event types, as the [SDK design](../design/sdk.md) specifies. Owns `crates/greennode-sdk/src/context.rs`, `secret.rs`, `error.rs`, `model/`, `tests/core_contracts.rs`. Review (Sol) with canary tests for `Secret`.
- [ ] **4. Query engine.** Implementer. The port of go-jmespath v0.4.0 as [CLI query](../design/cli-query.md) specifies, over the value tree from task 2, with the library's compliance suite and committed differential fixtures. Owns `crates/greennode/src/cli/query.rs`, `src/cli/query/`, `tests/query.rs`, `tests/fixtures/query/`, `crates/greennode/licenses/go-jmespath-LICENSE`, and the license-notice lines in `scripts/comments-allowlist.txt`. Review (Sol). Lexer, parser, `compile`, and evaluation of the core forms with the depth limit are done. Remaining: filters, comparators, logical operators, expression references, functions, the work budget and result size limit, and the differential corpus.
- [ ] **5. Configuration.** The INI readers, safe file opening with the alias check, mode and path selection, settings resolution, and credential selection are done. Remaining: `Config::load` and `Config::resolve_profile` as public entry points, calling the read-only preflight first, the cache-path check, the public credentials provider in place of the private marker, and removing the config module's dead-code allowance; adversarial review (Sol).
- [ ] **6. Connection and product sender.** Endpoint validation and route assembly are done. Remaining: the hyper-util client with the hyper-rustls connector, the product sender with its redirect, retry, bound, and event rules, and the loopback test server, as the [request policies](../design/sdk-requests.md) specify; adversarial review (Sol).
- [ ] **7. CLI frame: declarations and parsing.** Root, global flags, version, help, declarations, registry, parity, and the shipped-command inventory are done. Remaining: staged input with `--cli-input-json` and the hidden documentation generator, with operation flag parsing.
- [ ] **8. CLI frame: dispatch and errors.** Implementer. The dispatch phases and guards, the CLI adapter contract, output policies, the error classifier and exit codes with service-local rules, process injection and cancellation in `main.rs`, using the render and query modules. Baseline: `internal/cli/op.go`, `errors.go`, `errors_exit.go`, `cmd/vngcloud/main.go`. Owns `crates/greennode/src/main.rs`, `src/cli/dispatch.rs`, `errors.rs`, `tests/cli.rs`, `tests/frame_dispatch.rs`. Review (Sol).
- [ ] **9. Sign-in, cache, and discovery.** Four implementers on disjoint paths, then one adversarial review (Sol) of the whole with a canary for every secret. The token cache with its lock and one-time-code marker (`src/cache/`; baseline `internal/tokencache/`). The service-account exchange (`src/wire/credential/service_account.rs`; baseline `internal/core/service_account_*.go`). The IAM user sign-in with one-time codes (`src/wire/credential/iam_user.rs`; baseline `internal/iamuser/`). The session, provider contract, refresh gates, and project discovery (`src/session/`, `src/discovery/`; baseline `internal/core/`, `internal/discovery/`).
- [ ] **10. First reads end to end.** Implementer. Billing reads in the SDK and their commands in the tool, as the template every service follows, with the generated documentation page. The primary then updates [porting](../../instructions/porting.md) with the pattern and asks the owner to approve one read-only live run.
- [ ] **11. First guarded write and configure.** Two implementers in parallel, each with adversarial review (Sol). One: `billing create-budget`, `update-budget`, and `delete-budget`, proving send once, write dispatch, `--yes`, and read-only on real operations. Partial output is proven in task 8 with a frame fixture, since these budget operations return none on error. The other: `configure` with `set`, `get`, and `list`, the legacy-mode refusal, and the recheck under lock; baseline `internal/cli/configure*.go`, `configfile.go`.
- [ ] **12. First release.** The primary does the one-time GitHub setup. The owner approves the `v0.1.0` tag and publishes both crates by hand as the [first release design](../design/release-first.md) describes, then adds the two Trusted Publishers.
- [ ] **13. Release tooling.** The release checker with its `evidence` command, the checksum-checking credential provider, and `release.yml` are done. `publish-crates.yml` has its preflight and packaging jobs. Remaining: its `publish` job (approval gate, one token exchange, SDK-first uploads through the credential provider, registry gate), with adversarial review (Sol), and the loopback-registry proof in the design's unverified list on macOS and Ubuntu.
- [ ] **14. Services.** In waves. Before each wave the primary registers the wave's services in both crates and lands any shared model or quote and discovery code they need. One implementer and one worktree per service or slice, reads before writes, each write with adversarial review. A slice owns its service's paths in both crates as the [layout](../design/architecture.md#layout) lists them.

## Waves

Smallest first, so the pattern settles on cheap services. Lines are the baseline's Go source without tests. Large services are split by concern into slices of about 1,500 lines, each with its own brief.

|Wave|Services|Operations|Go lines|Needs first|
|-|-|-|-|-|
|1|`project`, `backup`, `portal`, `tagging`, `vks`|17|1,718|Task 11|
|2|`database`, `containerregistry`, `dns`, the rest of `billing`|32|5,000|Wave 1|
|3|`pricing`, `volume`, `globalloadbalancer`, `cdn`|55|8,995|Shared quote and discovery code; the vCDN key path|
|4|`compute`, `iam`, `loadbalancer`|102|14,047|The shared server model|
|5|`storage`, `monitor`, `network`|154|25,856|The S3 sender for storage objects; compute and pricing for network|
