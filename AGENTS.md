# greennode: guide for coding agents

Shared rules live here and in `instructions/`. [CLAUDE.md](CLAUDE.md) adds Claude Code details. Read the topic instructions for the work in hand.

## State

`greennode` is Danny's personal, unofficial Rust SDK and AWS-style command-line tool for GreenNode, formerly VNG Cloud. It is the Rust port of [vngcloud](https://github.com/dannyota/vngcloud), licensed under Apache-2.0, and not affiliated with GreenNode or VNG. Two crates publish to crates.io: `greennode-sdk` and the tool `greennode`. The repository and CI logs are public.

The tool builds one command, `greennode version`, and the SDK holds its shared contracts with no API calls yet. Remaining work is in the [foundation plan](docs/plans/foundation.md).

## Map

|Question|Entry point|
|-|-|
|What should the product become?|[Architecture](docs/design/architecture.md)|
|How are settings, profiles, and the vngcloud fallback resolved?|[Configuration design](docs/design/config.md)|
|What is the SDK's public interface, and how may it send requests?|[SDK design](docs/design/sdk.md) and [request policies](docs/design/sdk-requests.md)|
|How are commands declared, guarded, run, and printed?|[CLI design](docs/design/cli.md) and [CLI output](docs/design/cli-output.md)|
|What must hold for credentials, requests, and writes?|[Security design](docs/design/security.md)|
|How does a version reach crates.io?|[Release design](docs/design/release.md)|
|Why was a choice made?|[ADRs](docs/adr/README.md)|
|What work remains, in what order?|[Plans](docs/plans/)|
|How should documents and comments be written?|[Engineering standard](docs/standards/engineering.md)|
|What did the Go CLI do?|The [baseline](instructions/porting.md#baseline), `vngcloud` at tag `v1.2.0`|
|What does the system do now?|Code and tests|

|Doing|Read first|
|-|-|
|Planning, briefing, implementing, reviewing|[Workflow](instructions/workflow.md)|
|Working as a briefed implementer or reviewer|[Roles](instructions/roles.md)|
|Porting a command or service from the Go CLI|[Porting](instructions/porting.md)|
|Writing Rust|[Rust](instructions/rust.md)|
|Writing or running tests|[Testing](instructions/testing.md)|
|Touching auth, credentials, writes, dependencies, or workflows|[Security](instructions/security.md)|
|Writing documents|[Documentation](instructions/documentation.md)|

Direct owner instructions and platform safety rules come first. An accepted ADR wins over design text, and a greennode design wins over the baseline and over a plan. The baseline decides any behavior no greennode design covers. Code and tests describe current behavior; a disagreement with the design is a defect to repair or a gap the plan tracks.

## Code map

|Path|Owns|
|-|-|
|`crates/greennode/src/main.rs`|Process entry: passes arguments and streams to `cli::run`|
|`crates/greennode/src/cli.rs`|Root command and `version`|
|`crates/greennode/tests/cli.rs`|Process tests of the built binary|
|`crates/greennode-sdk/src/lib.rs`|SDK crate root and re-exports|
|`crates/greennode-sdk/src/{context,clock,secret,token,error,source,event}.rs`, `src/model/`|Shared contracts: request context, clock, secrets, tokens, errors, configuration source, debug events, list models|
|`scripts/`|Gates that `make check` and CI run, with their tests in `scripts/test/`|

The [architecture](docs/design/architecture.md#layout) owns the intended layout. Add a row here in the change that adds the path.

## Commands

`make check` before every commit. `make deny` and `make crate-age` when `Cargo.lock` changes. `make reference` clones the baseline. `make help` lists all targets.

## Roles

|Role|Who|Duty|
|-|-|-|
|Primary|The session the owner talks to|Decides, edits every design, writes documentation and plans, briefs, verifies, owns Git and shared files|
|Designer|GPT Sol|Drafts one design from a brief; never implements|
|Implementer|GPT Terra|One bounded task in named paths, with tests; never commits|
|Reviewer|GPT Sol for Terra's code, GPT Astra for designs and the primary's work|Independent review of designs, ADRs, and code; never edits files or changes Git|

|GPT model|Agent name|Claude stand-in when GPT is unavailable|
|-|-|-|
|Terra|`terra`|Sonnet|
|Sol|`sol`|Opus|
|Astra|`astra`|Fable|

Each name is a model family that resolves to its newest model, so plans, briefs, and notes name the family or role, never a versioned model ID. Astra and Fable design and review only; they never write code. A reviewer always runs on a different model from the author, never fixes the work it reviewed, and a worker's self-review never counts. A designer never implements the design it drafted. The owner chooses the primary session's model. Worker rules live in [roles](instructions/roles.md).

## Rules for every task

1. Start with `git status --short`. Existing changes belong to someone else unless the brief says otherwise.
2. Put security before features and convenience. Never read, print, or commit credential values, including `.env`, `~/.greennode`, and `~/.vngcloud`.
3. Keep the ported contract: command names, flags, output keys, query keys, and exit codes match the [baseline](instructions/porting.md#baseline) except where the [architecture](docs/design/architecture.md#differences-from-the-baseline) says otherwise. Report a needed difference instead of deciding it.
4. Write no code comments except the exceptions in the [engineering standard](docs/standards/engineering.md#code-comments). Put decisions in the design that owns them.
5. Tests stay hermetic: loopback servers, toy credentials, temporary directories, and no call to the real GreenNode API.
6. Live calls to GreenNode need the owner's approval for that run. Workers never make them.
7. Report exact files and checks, including failures and skipped checks. Never claim verification that did not run.

## Git and GitHub

- Run `gh` as `dannyota`: prefix it with `GH_TOKEN=$(gh auth token -u dannyota)` and pass `--repo dannyota/greennode`. Never run `gh auth switch` or change git config.
- Push with `GH_TOKEN=$(gh auth token -u dannyota) git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push`.
- Use one worktree per task under `.worktrees/`. Every change lands through a pull request against `master`. Only the primary edits the shared files the [plan](docs/plans/foundation.md#shared-files) lists, unless a brief hands a named one to a worker for that task.
- The primary commits, pushes, opens pull requests, and merges with a merge commit when the `ci` check is green on the exact head and an independent review passes. CodeQL is not awaited. Documentation-only changes and changes only to agent setup (`AGENTS.md`, `CLAUDE.md`, `instructions/`) need no independent review unless they set a design or ADR.
- Tags, releases, and crates.io publishing need the owner's approval each time.
- Commits are signed. Use Conventional Commits. Messages describe the change only and never mention agents, AI, or automated assistance. Stage named paths; never use `git add -A` or `git add .`. Never use `git stash`.

## Gotchas

- The shell `sed` is GNU, so `sed -i ''` fails. Edit files with the Edit tool or Python.
- The Bash tool runs zsh: `$var` does not word-split and a glob that matches nothing aborts the command. Write multi-step scripts to the scratchpad and run them with `bash`.
- The length gate fails when a capped file shrinks. Lower its cap in `scripts/lengths-baseline.txt`.
- Never print request bodies, tokens, or credential values in test assertion messages. CodeQL flags them as cleartext logging.
- `.reference/` is a git-ignored clone of another repository. Never edit it, and never copy from its `examples/` output or any `.env`.
- A new worktree has no `node_modules`. Run `npm ci --ignore-scripts` in it before `make check`.
- A worktree sits inside the main checkout, so Cargo there needs the worktree's own workspace `Cargo.toml`. Branch every worktree from a commit that has it.
- Once the fallback exists, the built tool can reach a real account through `~/.vngcloud`. Run it only with a cleared environment and a temporary home.
