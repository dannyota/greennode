# Testing

- Write the failing test first. A bug fix keeps a test that fails without the fix.
- Tests drive real code paths with only the network faked. Mock servers listen on loopback and answer with shapes copied from the baseline's sanitized fixtures or from documented responses.
- Tests never touch the owner's home directory or environment. In-process tests build the injected environment themselves. A process test clears the child's environment and sets each variable it needs, with a temporary directory as its home.
- A fallback test builds both the `GREENNODE_` and `VNGCLOUD_` sides, and both directories, inside its temporary home.
- Tests are deterministic: inject clocks and randomness, never sleep to wait for a condition, and never retry a flaky test into a pass.
- When you set `HOME` for a test run from a shell, set it for the test command only, never for Git commands, because Git reads the commit identity and signing key from it. Tests must not spawn Git.
- Name a test after the behavior it proves.
- Keep tests minimal: one test per rule, with a hostile and a legitimate case. No exhaustive variants.
- A ported operation has tests for its request, its success output, each documented error status, and each guard, as [porting](porting.md#how-to-port) describes.
- A decoding test reads a raw fixture, not output the code produced, because decoding drops unknown fields silently.
- Auth, credential, debug-output, and error tests plant canary values and assert they appear nowhere in stdout, stderr, or error text. Compare; never print the value in an assertion message.
- Security fixes include the hostile case and the legitimate case that must keep working.
- Live checks against the real GreenNode API run only with the owner's approval for that run, and only the primary runs them. Mocks do not prove API behavior, so state which claims stay unverified.
