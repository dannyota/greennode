# Engineering documentation and comments

Write each fact once, in the artifact that owns it, in short plain words. Plans
say what to do next and are deleted when the work lands. Designs and ADRs say
how the system should be and why. Code, tests, and living documents must stand
on their own after every plan is gone.

## Ownership

| Information                            | Owner                           |
| -------------------------------------- | ------------------------------- |
| Intended product or architecture       | `docs/design/`                  |
| One accepted choice and its trade-offs | `docs/adr/`                     |
| Current behavior                       | Code and tests                  |
| Work order and delivery state          | `docs/plans/`                   |
| Install and usage                      | `README.md`                     |
| Contributor setup                      | `CONTRIBUTING.md`               |
| Vulnerability reporting                | `SECURITY.md`                   |
| License and inherited copyright        | `LICENSE`                       |
| Shared agent work rules                | `AGENTS.md` and `instructions/` |

## Writing

- Lead with the result. Use one idea per sentence and active voice.
- Name the exact file, command, or value instead of "it" or "some".
- Use bullets for parallel items, numbers for ordered steps, tables for
  comparisons, and prose for connected reasoning.
- Do not use em dashes, filler, or closing recaps.
- The README, guides, and agent rules describe the current state without status
  history. Designs describe the intended state. Decision records and owner
  attributions may carry dates.
- Verify before asserting. Cite first-party documentation for the behavior of
  another product, and mark what stays unverified.
- Never describe designed but unbuilt behavior as available in the README.

## Code comments

Design documents, not code comments, hold decisions and explanations. Comments
drift from the code and mislead agents (owner, 2026-11-18).

- Code has no comments, with three exceptions: tool directives and
  generated-code markers; one line on why for a behavior of an external tool or
  service that no design document owns, naming its source; and the one-line
  license notice a third-party license requires in a file translated from its
  source, in the form the owning design states.
- A public item of the SDK crate has a `///` doc comment that tells a caller
  what the signature does not: units, limits, errors, and what a write changes.
  The SDK's crate root and public modules have a `//!` summary. These comments
  are the SDK's reference on docs.rs. Never restate the name.
- Write no other doc comments. Private items and the command-line crate have
  none.
- A rule the code must keep belongs in the design that owns it, or in a test
  that fails when it breaks.

`scripts/check-comments.py` fails on any comment in a tracked Rust file unless
it is a doc comment under `crates/greennode-sdk/src/` or
`scripts/comments-allowlist.txt` lists it. Each allowlist line is
`path:line: text`, exactly as the script prints the finding, and lines starting
with `#` are ignored.

## File size

Markdown stays at or under 400 lines. Rust stays at or under 700 lines, not
counting a top-level `#[cfg(test)]` module or files directly in a crate's
`tests/` and under its `tests/support/`. Split by concern and name each file
after its concern. Plans under `docs/plans/` are exempt because they are deleted
when their work lands.

`scripts/lengths-baseline.txt` caps files that were already over the limit, and
a cap may only fall. A baselined file cannot be renamed; split it instead.

Prettier formats human Markdown and markdownlint checks it, using the repository
configuration.

## ADR rules

A record has Status, Context, Decision, Consequences, Alternatives rejected, and
History. To change a decision in force, update its record with the owner's
approval and add a History line. Open a new record only for a new topic.

## Naming

| Item      | Convention                                  |
| --------- | ------------------------------------------- |
| Rust file | `<concern>.rs`                              |
| Rust test | `<behavior_it_proves>`                      |
| ADR       | `NNNN-<topic>.md`, sentence-case title      |
| Workflow  | lowercase kebab case `.yml`, stable job IDs |
