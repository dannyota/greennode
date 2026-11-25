# greennode

[![CI](https://github.com/dannyota/greennode/actions/workflows/ci.yml/badge.svg)](https://github.com/dannyota/greennode/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/dannyota/greennode)](LICENSE)

An AWS-style command-line tool for [GreenNode](https://greennode.ai), formerly
VNG Cloud, written in Rust.

This is a personal, unofficial project, not affiliated with or endorsed by
GreenNode or VNG.

## Status

greennode is the Rust port of the Go CLI in
[vngcloud](https://github.com/dannyota/vngcloud), and the port has just started.
The only command today is `greennode version`. No release exists yet, and the
crate is not on crates.io.

Use [vngcloud](https://github.com/dannyota/vngcloud) for real work until the
commands you need are ported. The [architecture](docs/design/architecture.md)
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

- [Architecture](docs/design/architecture.md): the command-line contract and the
  code layout.
- [Security design](docs/design/security.md): how credentials and writes are
  protected.
- [Contributing](CONTRIBUTING.md): setup and checks.
- [Security policy](SECURITY.md): how to report a vulnerability.

## License

[Apache-2.0](LICENSE).
