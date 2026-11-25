# Foundation plan

Remaining work from an empty crate to the first crates.io release and parallel service porting. The [architecture](../design/architecture.md) owns the contract and [workflow](../../instructions/workflow.md) owns how a task runs. Delete a task when it lands and this file when all of it has.

## Order

Tasks 1 and 2 start now and run in parallel. Task 3 follows task 1. Tasks 4 and 5 follow task 3 and run in parallel. Task 6 follows 4 and 5. Task 7 follows 6. Task 8 follows 7. Task 9 starts when task 7 lands and runs as many services at once as the laptop allows.

## Tasks

- [ ] **1. Design review and owner decisions.** Reviewer (Astra) reviews [architecture](../design/architecture.md), [security](../design/security.md), [release](../design/release.md), and [ADR 0001](../adr/0001-rust-port-of-the-vngcloud-cli.md). The owner confirms or changes ADR decisions 4 to 7. Accept: findings resolved, ADR status accepted, design status lines removed.
- [ ] **2. Parity list.** Implementer. A script reads the baseline's `docs/wiki/CLI-*.md` pages and writes `tests/parity/commands.json`: service, operation, kind, and each flag with type and required state. Owns `scripts/parity-list.py`, `scripts/test/parity-list-test.py`, `tests/parity/`, and one `Makefile` target. Accept: the list holds 19 services and every operation heading in those pages; the script's test covers a read, a write, and a page with no flags; rerunning the script changes nothing.
- [ ] **3. CLI frame.** Primary writes `docs/design/cli.md` from the baseline's `docs/design/cli.md` and `cli-usability.md`, recording only greennode's choices: how an operation is declared, the injected process value, the error type, and the JMESPath crate. Reviewer (Astra) accepts it. Then implementer: global flags, guards, JSON, table, and text output, `--query`, the stderr error document, exit codes, the hidden documentation generator, and the parity test. Owns `src/main.rs`, `src/cli/`, `src/lib.rs`, `tests/cli.rs`, `tests/parity/`, `Cargo.toml`. Accept: the baseline's `internal/cli` tests for these rules are ported and pass; the parity test fails when a registered command differs from the list.
- [ ] **4. Profiles and configure.** Implementer. The INI reader, profile and precedence rules, file safety, and `configure` with `set`, `get`, and `list`. Baseline: `internal/ini/`, `internal/core/load.go`, `internal/cli/configure*.go`, `docs/design/sdk-and-cli.md#configuration-and-credentials`, `docs/design/cli.md#configure`. Owns `src/core/` except sign-in and cache files, and `src/cli/configure/`. Adversarial review (Sol): file modes, symbolic links, secrets in arguments, and tampered files.
- [ ] **5. Transport.** Implementer. The HTTP client with TLS, redirect refusal, size limits, the retry rules, and debug logging. Baseline: `internal/transport/`. Owns `src/transport/`. Adversarial review (Sol): redirects, endpoint overrides, oversized and malformed bodies, and log content.
- [ ] **6. Sign-in and token cache.** Implementer. Static token, service account, and IAM user sign-in with one-time codes; the token cache with its lock and the one-time-code step marker; project discovery. Baseline: `internal/core/`, `internal/iamuser/`, `internal/tokencache/`, `internal/discovery/`, `docs/design/service-account-auth.md`. Owns the sign-in and cache files under `src/core/`. Adversarial review (Sol) with canary tests for every secret.
- [ ] **7. First service: billing reads.** Implementer. `billing list-budgets` and the other billing reads, as the template every service follows: module layout, fixtures, tests, and the generated documentation page. Owns `src/services/billing/`, `tests/billing.rs`, `tests/fixtures/billing/`. Afterwards the primary updates [porting](../../instructions/porting.md) with the pattern, registers the remaining 18 services as empty modules so later tasks touch no shared file, and asks the owner to approve one read-only live run.
- [ ] **8. Release workflows and first publish.** Implementer ports `release.yml`, `publish-crates.yml`, `scripts/check-crate-release.py`, and its test from duolane, as the [release design](../design/release.md) describes. Adversarial review (Sol). The primary does the one-time GitHub setup. The owner approves the `v0.1.0` tag and publishes the first version by hand, then adds the Trusted Publisher.
- [ ] **9. Services.** One implementer and one worktree per service, reads before writes, each write with adversarial review. Each task owns `src/services/<service>/`, `tests/<service>.rs`, and `tests/fixtures/<service>/`.

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
