# Foundation plan

Remaining work from one crate with one command to the first crates.io release and parallel service porting. The [architecture](../design/architecture.md) owns the contract and [workflow](../../instructions/workflow.md) owns how a task runs. Delete a task when it lands and this file when all of it has.

## Order

Tasks 1, 2, and 3 start now and run in parallel. Tasks 4 and 5 follow 1 and 2 and run in parallel. Task 6 follows 5. Task 7 follows 4 and 6. Task 8 follows 7. Task 9 starts when task 7 lands and runs as many services at once as the laptop allows.

## Tasks

- [ ] **1. Design review.** Reviewer (Astra) reviews [architecture](../design/architecture.md), [security](../design/security.md), [release](../design/release.md), and [ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md), with the [fallback rules](../design/architecture.md#fallback-to-vngcloud-settings) traced against the baseline's precedence and file safety rules. Accept: findings resolved and the design status lines removed.
- [ ] **2. Workspace split.** Implementer. Turn the single crate into a workspace with `crates/greennode-sdk` (library, no code yet beyond a documented crate root) and `crates/greennode` (the current binary and tests), one shared version, and the tool depending on the exact SDK version. Make the length gate treat each crate's `tests/` as the standard says, and make the comment gate allow doc comments under `crates/greennode-sdk/src/` only. Owns `Cargo.toml`, `Cargo.lock`, `src/`, `tests/`, `crates/`, `scripts/check-lengths.sh`, `scripts/check-comments.py`, and their tests. Accept: `make check` and `make deny` pass; each gate change has a failing-then-passing script test; `cargo package --list --package greennode-sdk` shows only the files the crate needs, with the license and a README.
- [ ] **3. Parity list.** Implementer. A script reads the baseline's `docs/wiki/CLI-*.md` pages and writes `crates/greennode/tests/parity/commands.json`: service, operation, kind, and each flag with type and required state. Owns `scripts/parity-list.py`, `scripts/test/parity-list-test.py`, `crates/greennode/tests/parity/`, and one `Makefile` target. Accept: the list holds 19 services and every operation heading in those pages; the script's test covers a read, a write, and an operation with no flags; rerunning the script changes nothing.
- [ ] **4. CLI frame.** Primary writes `docs/design/cli.md` from the baseline's `docs/design/cli.md` and `cli-usability.md`, recording only greennode's choices: how a command is declared, the injected process value, the error document, and the JMESPath crate. Reviewer (Astra) accepts it. Then implementer: global flags, guards, JSON, table, and text output, `--query`, the stderr error document, exit codes, the hidden documentation generator, and the parity test. Owns `crates/greennode/`. Accept: the baseline's `internal/cli` tests for these rules are ported and pass; the parity test fails when a registered command differs from the list.
- [ ] **5. SDK core: configuration and transport.** Primary writes `docs/design/sdk.md`: the public shape of the configuration, clients, inputs, outputs, and errors, how outputs carry the baseline's JSON names, and the async runtime and HTTP crates. Reviewer (Astra) accepts it. Then two implementers in parallel. One: the INI reader, profiles, precedence, file safety, and the [fallback rules](../design/architecture.md#fallback-to-vngcloud-settings); baseline `internal/ini/`, `internal/core/load.go`, `docs/design/sdk-and-cli.md#configuration-and-credentials`. The other: the HTTP transport with TLS, redirect refusal, size limits, retry rules, and debug logging; baseline `internal/transport/`. Adversarial review (Sol) of both: file modes, symbolic links, tampered files, mixed credential sources, redirects, endpoint overrides, oversized bodies, and log content.
- [ ] **6. Sign-in and token cache.** Implementer. Static token, service account, and IAM user sign-in with one-time codes; the token cache with its lock and the one-time-code step marker; project discovery. Baseline: `internal/core/`, `internal/iamuser/`, `internal/tokencache/`, `internal/discovery/`, `docs/design/service-account-auth.md`. Owns the sign-in and cache files in the SDK. Adversarial review (Sol) with canary tests for every secret.
- [ ] **7. First service and configure.** Two implementers in parallel. One: billing reads in the SDK and `billing list-budgets` with the other billing reads in the tool, as the template every service follows. The other: `configure` with `set`, `get`, and `list`, including the copy from `~/.vngcloud` on first write; baseline `internal/cli/configure*.go` and `docs/design/cli.md#configure`; adversarial review (Sol). Afterwards the primary updates [porting](../../instructions/porting.md) with the pattern, registers the remaining 18 services as empty modules in both crates so later tasks touch no shared file, and asks the owner to approve one read-only live run.
- [ ] **8. Release workflows and first publish.** Implementer ports `release.yml`, `publish-crates.yml`, `scripts/check-crate-release.py`, and its test from duolane for two crates, as the [release design](../design/release.md) describes. Adversarial review (Sol). The primary does the one-time GitHub setup. The owner approves the `v0.1.0` tag and publishes the first version of both crates by hand, then adds the two Trusted Publishers.
- [ ] **9. Services.** One implementer and one worktree per service, reads before writes, each write with adversarial review. Each task owns `crates/greennode-sdk/src/<service>/`, `crates/greennode-sdk/tests/<service>.rs`, `crates/greennode-sdk/tests/fixtures/<service>/`, `crates/greennode/src/services/<service>.rs`, and `crates/greennode/tests/<service>.rs`.

## Service order

Smallest first, so the pattern settles on cheap services before the large ones. Lines are the baseline's Go source without tests.

|Service|Operations|Go lines|
|-|-|-|
|`project`|1|46|
|`backup`|2|275|
|`portal`|5|327|
|`tagging`|3|434|
|`vks`|5|636|
|`database`|9|1,236|
|`containerregistry`|9|1,329|
|`dns`|10|1,431|
|`pricing`|2|1,448|
|`globalloadbalancer`|16|2,281|
|`cdn`|16|2,591|
|`billing` writes|17 with reads|2,635|
|`volume`|21|2,675|
|`compute`|31|4,328|
|`iam`|38|4,564|
|`loadbalancer`|33|5,155|
|`storage`|35|6,523|
|`monitor`|48|9,135|
|`network`|71|10,198|

The shared Go code the core tasks port is about 3,300 lines in `internal/core`, 2,100 in `internal/transport`, and 14,600 in `internal/cli`, most of the last being per-service operation tables that move with each service.
