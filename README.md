# greennode

[![CI](https://github.com/dannyota/greennode/actions/workflows/ci.yml/badge.svg)](https://github.com/dannyota/greennode/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/dannyota/greennode)](LICENSE)

A Rust SDK and an AWS-style command-line tool for
[GreenNode](https://greennode.ai), formerly VNG Cloud.

This is a personal, unofficial project, not affiliated with or endorsed by
GreenNode or VNG.

## Status

greennode is a Rust SDK and CLI for GreenNode, compatible with
[vngcloud](https://github.com/dannyota/vngcloud). The CLI ships
`greennode version`, `greennode configure`, and the service commands that
[CLI.md](docs/wiki/CLI.md) lists. Work continues service by service. No release
exists, and nothing is on crates.io.

Use [vngcloud](https://github.com/dannyota/vngcloud) for real work until the
commands you need ship. The [architecture](docs/design/architecture.md)
describes what greennode keeps from vngcloud and what changes.

## Build from source

Install the Rust version in `rust-toolchain.toml` with
[rustup](https://rustup.rs), then:

```bash
git clone https://github.com/dannyota/greennode
cd greennode
cargo run --locked -- version
```

## Documentation

- [CLI reference](docs/wiki/CLI.md)
- [Architecture](docs/design/architecture.md): the command-line contract and the
  code layout.
- [Security design](docs/design/security.md): how credentials and writes are
  protected.
- [Contributing](CONTRIBUTING.md): setup and checks.
- [Security policy](SECURITY.md): how to report a vulnerability.

## License

[Apache-2.0](LICENSE).
