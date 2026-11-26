# Foundation plan

Remaining work from an empty SDK and a one-command tool to the first crates.io release and parallel service porting. The [architecture](../design/architecture.md) owns the contract and [workflow](../../instructions/workflow.md) owns how a task runs. Delete a task when it lands and this file when all of it has.

## Shared files

The primary is the integration owner of the workspace `Cargo.toml`, `Cargo.lock`, each crate's `Cargo.toml`, `crates/greennode-sdk/src/lib.rs`, `crates/greennode/src/cli.rs` and `src/services/mod.rs`, the `Makefile`, the workflows, and the AGENTS.md code map. A brief may hand a worker named lines or one named shared file for that task only; the brief says exactly which. The primary regenerates the lockfile when merging. A new shared type, or a change to one, lands before the tasks that depend on it.

## Order

Task 10 runs first; then 11, then 12. Task 13 lands before the first CI publish. Task 14 starts when 11 lands.

## Tasks

- [ ] **10. First reads end to end.** Billing reads in the SDK and their commands in the tool, as the template every service follows, with the generated documentation page. The first slice (`list-budgets`, `get-budget`) also finishes the frame: real dispatch handlers through the clap parser and registry, the `Config` adapter, input descriptors bound per operation with per-operation `--cli-input-json` and `--query` switches, the rest of the `gen-docs` root page, a process-level signal test, and an assertion that operation commands carry the implicit input flag. The other nine reads follow in parallel on that template. The primary then updates [porting](../../instructions/porting.md) with the pattern and asks the owner to approve one read-only live run.
- [ ] **11. First guarded write and configure.** Two implementers in parallel, each with adversarial review (Sol). One: `billing create-budget`, `update-budget`, and `delete-budget`, proving send once, write dispatch, `--yes`, and read-only on real operations. Partial output is proven in task 8 with a frame fixture, since these budget operations return none on error. The other: `configure` with `set`, `get`, and `list`, the legacy-mode refusal, and the recheck under lock; baseline `internal/cli/configure*.go`, `configfile.go`.
- [ ] **12. First release.** The primary does the one-time GitHub setup. The owner approves the `v0.1.0` tag and publishes both crates by hand as the [first release design](../design/release-first.md) describes, then adds the two Trusted Publishers.
- [ ] **13. Release proof on Ubuntu.** Run `scripts/test/loopback-registry-proof.py` on Ubuntu with Cargo 1.100.0 before the first CI publish, as the [release design](../design/release.md#unverified) requires.
- [ ] **14. Services.** In waves. Before each wave the primary registers the wave's services in both crates and lands any shared model or quote and discovery code they need. One implementer and one worktree per service or slice, reads before writes, each write with adversarial review. A slice owns its service's paths in both crates as the [layout](../design/architecture.md#layout) lists them. Baseline additions from `v1.2.0` to `v1.7.0` to port with their service: backup and volume list reads refuse reflected credentials and cover han-1; compute create-server adds disk-zone advice on HTTP 400; monitor adds dashboard list and get and HTTP check response-time reads; network adds NAT detail and inbound-rule reads, hcm-3 Public NAT create and delete, and clears stale partial IDs on an ambiguous create.

## Waves

Smallest first, so the pattern settles on cheap services. Lines are the baseline's Go source without tests. Large services are split by concern into slices of about 1,500 lines, each with its own brief.

|Wave|Services|Operations|Go lines|Needs first|
|-|-|-|-|-|
|1|`project`, `backup`, `portal`, `tagging`, `vks`|17|1,718|Task 11|
|2|`database`, `containerregistry`, `dns`, the rest of `billing`|32|5,000|Wave 1|
|3|`pricing`, `volume`, `globalloadbalancer`, `cdn`|55|8,995|Shared quote and discovery code; the vCDN key path|
|4|`compute`, `iam`, `loadbalancer`|102|14,047|The shared server model|
|5|`storage`, `monitor`, `network`|154|25,856|The S3 sender for storage objects; compute and pricing for network|
