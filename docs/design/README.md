# Design

These documents describe the intended product. Where the code differs, the
[plans](../plans/) track the gap. Behavior that greennode takes unchanged from
the baseline is documented in the baseline's own designs, not copied here.

| Design                                  | Owns                                                                        |
| --------------------------------------- | --------------------------------------------------------------------------- |
| [Architecture](architecture.md)         | The contract with the baseline, how the parts fit, and invariants           |
| [Configuration](config.md)              | Settings, profiles, precedence, the fallback to vngcloud, and file safety   |
| [SDK](sdk.md)                           | The public interface of `greennode-sdk`, results, errors, and outputs       |
| [SDK request policies](sdk-requests.md) | The three kinds of request, their limits, and the SDK's crates              |
| [Command line](cli.md)                  | Command declarations, guards, dispatch, errors, exit codes, and `configure` |
| [CLI output](cli-output.md)             | The JSON projection, table, and text                                        |
| [CLI query](cli-query.md)               | The `--query` engine, compatible with go-jmespath v0.4.0                    |
| [Security](security.md)                 | Trust boundaries, secure defaults, and threats                              |
| [Release publishing](release.md)        | GitHub Release and crates.io publishing gates                               |
