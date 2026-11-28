# Foundation plan

Remaining work from an empty SDK and a one-command tool to the first crates.io release and parallel service work. The [architecture](../design/architecture.md) owns the contract and [workflow](../../instructions/workflow.md) owns how a task runs. Delete a task when it lands and this file when all of it has.

## Shared files

The primary is the integration owner of the workspace `Cargo.toml`, `Cargo.lock`, each crate's `Cargo.toml`, `crates/greennode-sdk/src/lib.rs`, `crates/greennode/src/cli.rs` and `src/services/mod.rs`, the `Makefile`, the workflows, and the AGENTS.md code map. A brief may hand a worker named lines or one named shared file for that task only; the brief says exactly which. The primary regenerates the lockfile when merging. A new shared type, or a change to one, lands before the tasks that depend on it.

## Order

Task 14 runs now. Task 10 waits for the owner. Task 12 runs alongside task 14. Task 13's Ubuntu proof may run alongside task 12. Trusted Publishing setup follows task 12, and task 13 includes the first CI publish and cutover.

## Tasks

- [ ] **10. Live read check.** The billing template, its reads and the frame have landed. The owner approves one read-only live billing run; until then live behavior stays unverified.
- [ ] **12. First Mac release.** Land the remaining local checker changes and confirm the macOS packaging proof. With the owner's approval, tag `v0.1.0` and publish both crates using the [Mac release procedure](../design/release-first.md).
- [ ] **13. CI publish proof and cutover.** Pass `scripts/test/loopback-registry-proof.py` on Ubuntu with Cargo 1.100.0. Configure Trusted Publishing after both names exist, then run a preflight-only dispatch and prove the uploads on the first owner-approved CI publish. The owner approves cutover after the proof, as the [release design](../design/release.md#ci-cutover) specifies.
- [ ] **14. Services.** In waves. Before each wave the primary registers the wave's services in both crates and lands any shared model or quote and discovery code they need. One implementer and one worktree per service or slice, reads before writes, each write with adversarial review. A slice owns its service's paths in both crates as the [layout](../design/architecture.md#layout) lists them. Baseline additions from `v1.2.0` to `v1.7.0` to build with their service: backup and volume list reads refuse reflected credentials and cover han-1; compute create-server adds disk-zone advice on HTTP 400; monitor adds dashboard list and get and HTTP check response-time reads; network adds NAT detail and inbound-rule reads, hcm-3 Public NAT create and delete, and clears stale partial IDs on an ambiguous create. Landed: waves 1 and 2, the shared quote code, and the globalloadbalancer and cdn reads. Baseline additions from `v1.7.0` to `v1.8.1` to build with their service:
  - Compute: flavor price discovery accepts a root disk type and quotes explicitly selected disabled zones.
  - Global Load Balancer: pool, listener, and member group writes resolve regions and package availability from the account catalog.
  - Monitor: synthetic package reads and quota status, quote, create, renew, and recover validate free offers against the catalog.
  - Network: list bandwidth packages; private endpoint quote and create accept catalog zone, category, service, and package selectors, with catalog and deletion inventory guards.
  - Network, `v1.8.1` and the pending `v1.8.2`: NAT create and quote take the availability zone and offer from the catalog only and check the VPC by identity, not its `zones` field; NAT inbound-rule IDs, which never enter a path, use the type-ID charset that allows `_`.
  - Pricing: estimates accept catalog storage regions and quote explicitly selected disabled zones.
  - Storage: project types, project quotes, and storage prices resolve region names or IDs against the complete account catalog.
  - Volume: price discovery quotes explicitly selected disabled zones and reports offering bounds.

## Waves

Smallest first, so the pattern settles on cheap services. Lines are the baseline's Go source without tests. Large services are split by concern into slices of about 1,500 lines, each with its own brief.

|Wave|Services|Operations|Go lines|Needs first|
|-|-|-|-|-|
|1|`project`, `backup`, `portal`, `tagging`, `vks`|17|1,718|Task 11|
|2|`database`, `containerregistry`, `dns`, the rest of `billing`|32|5,000|Wave 1|
|3|`pricing`, `volume`, `globalloadbalancer`, `cdn`|55|8,995|Shared quote and discovery code; the vCDN key path|
|4|`compute`, `iam`, `loadbalancer`|102|14,047|The shared server model|
|5|`storage`, `monitor`, `network`|154|25,856|The S3 sender for storage objects; compute and pricing for network|
