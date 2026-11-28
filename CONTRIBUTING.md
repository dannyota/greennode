# Contributing

Bug reports, documentation improvements, and focused fixes are welcome. Open an
issue first to discuss a larger change. Report vulnerabilities privately as
explained in [SECURITY.md](SECURITY.md).

Fork the repository and create a branch from `master`. Before changing behavior,
read the [architecture](docs/design/architecture.md) and
[security design](docs/design/security.md).

## Setup

Install the tools pinned in `.tool-versions`. rustup reads the Rust version from
`rust-toolchain.toml`. Install Node, cargo-deny, cargo-nextest, and gitleaks at
their pinned versions. The scripts need Python 3.11 or later. Then install the
Markdown tools and the pre-commit hook:

```bash
npm ci --ignore-scripts
make hooks-install
```

`make reference` clones the compatibility baseline, the Go CLI greennode
matches, into `.reference/`.

## Making a change

1. Write a failing test. Tests use loopback servers and toy credentials, never a
   real account.
2. Make the smallest change that passes it.
3. Update the design or README if behavior changes.
4. Run `make check`, which runs every check that works offline. `make help`
   lists each target. A change to `Cargo.lock` also needs `make deny` and
   `make crate-age`.
5. Open a pull request against `master` with a short description and the checks
   you ran.

Code has no comments, as the
[engineering standard](docs/standards/engineering.md#code-comments) explains.
Contributions are licensed under the Apache License 2.0.
