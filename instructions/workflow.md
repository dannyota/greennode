# Workflow

The primary designs and decides. Implementers change code. Reviewers judge work they did not write. [AGENTS.md](../AGENTS.md#roles) owns who does what; ownership of paths comes from each brief.

## Flow

1. Read the relevant design, accepted ADRs, and the plan task. Create one worktree under `.worktrees/<task>` on branch `wip/<task>`.
2. A change to architecture, a contract, or a security boundary starts with a design or ADR update that an independent reviewer accepts. Porting an operation whose behavior the [baseline](porting.md#baseline) already defines skips this step.
3. Brief one implementer per bounded task, as [briefs](#briefs) describes.
4. Failing test first, then the smallest correct change. Hostile input, authorization, races, and bounds belong to the author.
5. The primary inspects the diff, confirms the changed files match the report and the owned paths, runs the gates, and commits.
6. Send the commit SHA to the reviewer the [roles](../AGENTS.md#roles) table selects, with the design, the baseline paths, known limits, and a regression checklist.
7. Paste findings verbatim into a new implementer brief. The same reviewer confirms the fixes.
8. Push, open a pull request, wait for the `ci` check on the exact head, and merge with a merge commit. Delete the branch and the worktree.

Two red CI runs from the same cause mean stop and repair the process. Fix a red `master` forward at once.

## Briefs

A brief stands alone. It names:

- the role, with "follow `instructions/roles.md` for it"
- the objective and the definition of done
- authorities to read: the design sections, ADRs, and exact baseline paths
- owned paths, stated generously, and forbidden paths
- numbered decisions already made, so the worker does not reopen them
- test requirements: one test per rule, with a hostile and a legitimate case, and no exhaustive variants
- gate commands and the report contract
- "You are not alone in this repository. Preserve others' edits. Do NOT commit or change Git state."

Give standing answers for routine ambiguity: pick the safer reading, list the choice in the report, and stop only for a contract change or an edit outside owned paths. Most stops come from a needed file outside the owned paths, so name shared files explicitly and say who edits them.

## Parallel work

- Each concurrent writer gets its own worktree and disjoint paths. A service task owns `src/services/<service>/`, `tests/<service>.rs`, `tests/fixtures/<service>/`, and its generated documentation page, and nothing else.
- A file two tasks need, such as `Cargo.toml` or a shared type, has one owner per round. The primary lands that change first or edits the file itself when merging.
- A worker's worktree never contains `.env` or other credentials.
- Run one `make check` at a time per worktree. Do not start workers to repeat verification.
- When two branches change the same file, the primary merges the first, rebases the second onto `master`, and sends a behavior-changing conflict resolution to review.

## Reviews

Match review depth to risk.

- Low risk: reads, decoding, help, tables, and documents. One review round. Fixes for low findings land without a second confirmation; medium or higher findings need one.
- Always adversarial: auth, token handling, credential storage, every write operation, release workflows, and dependency trust. Trace the threats in the [security design](../docs/design/security.md#threats) through concrete code paths.
- When review keeps finding variants of one edge-case class, close the class with one fail-closed rule instead of patching each variant.

Ask every reviewer for a verdict of MERGE, MERGE-WITH-FIXES, or BLOCK, then findings ordered by severity with `file:line`, a concrete failure scenario, and one fix sentence, then confirmed invariants, checks run, and what was not checked.

Give the reviewer the base and head commits, the diff, the local gate output, and the CI result for the exact head. A reviewer may build and run tests that write only under `target/` or a temporary directory.

## Reports

A report lists new, changed, and deleted files; checks run with results; checks not run with the reason; deviations from the brief; and open items. A worker's report supports verification and never replaces it.

## Codex CLI fallback

Use the Codex CLI only when direct subagent dispatch fails or its requests run on a model from another family.

`C=$(ls -d ~/.claude/plugins/cache/openai-codex/codex/*/ | sort -V | tail -1)scripts/codex-companion.mjs`

- Start from the worktree: `node "$C" task --background [--write] --fresh --model <model> --effort high "<prompt>"`. Point the prompt at a brief file in the scratchpad.
- Run `node "$C" status <id>` and `node "$C" result <id>` from the same directory. Poll no more often than every 90 seconds.
- Use `--write` only for implementers. Reviewers run read-only.
- The sandbox may have no network and cannot write outside the worktree. The primary reruns anything that needs either.
