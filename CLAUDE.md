@AGENTS.md

## Claude Code specifics

- Dispatch each worker as a fresh `Agent` subagent with `subagent_type` `terra`, `sol`, or `astra` from the user-level `~/.claude/agents`, with no `model` override. `Agent.model` accepts only Claude aliases, so GPT routing comes from the agent definition.
- Those agents have every tool, so the brief is the boundary: every brief names the role and points at [roles](instructions/roles.md), and a reviewer brief forbids edits, writing commands, and Git changes. Never use `fork` for an independent review.
- The worker's first report line is its runtime model ID, which must belong to the family in the [roles](AGENTS.md#roles) table. When the duolane monitor is running, confirm the family there; a worker's own report is weaker evidence.
- Run independent workers in the background in one message. Answer a worker's question, or confirm fixes with the same reviewer, through `SendMessage` to that agent instead of starting a new one. Every first review is a fresh agent.
- When a user-level agent cannot be dispatched, use the Codex plugin (`codex:codex-rescue`). When GPT is unavailable, start the Claude stand-in from the roles table with the Agent tool model alias `sonnet`, `opus`, or `fable`.
- Keep at most three workers that run `make check` at once on this laptop, and give building workers `CARGO_BUILD_JOBS=4`.
- Put briefs, review output, and temporary files in the session scratchpad, never under `docs/`.
- Tell reviewers and designers not to invoke review skills. A generic review skill can start a side agent the worker cannot stop; stop it with `TaskStop` and confirm the worktrees are unchanged.
