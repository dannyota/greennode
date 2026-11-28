# Documentation

The [engineering standard](../docs/standards/engineering.md) owns placement and writing rules. These rules add agent handling.

- Documents, code, comments, and commits are in English.
- Agent-only files (`AGENTS.md`, `CLAUDE.md`, `instructions/`, `docs/plans/`) use one line per paragraph or list item and unpadded tables.
- Human documents wrap at 80 columns. Tables and URLs may run longer. Run `node_modules/.bin/prettier --write <file>` on a human document before `make docs`.
- Create a directory with its first real file, never a placeholder.
- Update the design in the same change as the behavior it defines. Update the README in the same change as a public command, flag, or environment variable.
- A greennode design records only what greennode decides or changes. Behavior taken unchanged from the [baseline](compatibility.md#baseline) stays documented there; link to it instead of copying it.
- Check relative links and heading anchors after a rename or move.
- Never cite plans, tasks, or review findings in code, tests, or human documents. Cite the design or ADR, or state the rule.
