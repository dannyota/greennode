# Rust

## Checks

- `make check` runs every offline check and `make help` lists the targets. Run `make deny` and `make crate-age` when `Cargo.lock` changes.
- `make test` uses cargo-nextest, one process per test, with an empty home under `target/`, and fails if a test writes there.
- CI also proves each action SHA is its tag with `scripts/check-workflow-pins.py --tags`, which needs the network.

## Code rules

- Follow the surrounding module. Split files by concern, not by line count, and move code unchanged when splitting.
- Write no comments except the [standard's exceptions](../docs/standards/engineering.md#code-comments). `clap` help text uses `about` and `help` attributes, not doc comments.
- Only `src/main.rs` touches the process: arguments, environment variables, the home directory, the clock, and the standard streams enter through values that `main` builds and passes down. No other code calls `std::env::var`, reads the clock, or prints directly, so every behavior runs in a test without touching the machine.
- Keep no mutable global state. Constants, compiled patterns, and embedded assets are the only statics.
- Data from the network, a config file, a cache file, or the command line must not panic. Avoid `unwrap`, `expect`, indexing, and unchecked arithmetic outside tests and startup.
- Never slice text by a byte offset unless the offset is a checked character boundary.
- Write no `unsafe` code.
- Every HTTP request goes through `src/transport/`. No other module builds an HTTP client.
- A type that holds a secret has a `Debug` implementation that redacts it and no `Display`. An error never carries a token, a credential, or a response body.
- Every buffer, map, and file read fed by a response or a file has a bound in the [security design](../docs/design/security.md#limits).
- Blocking file or process calls do not run on async workers.
- Use a typed error that maps to one exit code. Decide the exit code in one place.
