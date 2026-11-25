# Security

Status: proposed, with the [architecture](architecture.md).

greennode holds cloud credentials and sends writes that can cost money or
destroy resources. Security comes before features and convenience. This design
states what must hold; the designs it links own the mechanisms.

## Trust boundaries

| Party                                           | Trusted for                                                                  |
| ----------------------------------------------- | ---------------------------------------------------------------------------- |
| The user at the terminal or the calling script  | Choosing the profile, the command, and its flags                             |
| Files under `~/.greennode/`                     | Credentials, settings, and cached tokens                                     |
| Files under `~/.vngcloud/`                      | Credentials and settings, read only, while greennode has no files of its own |
| The GreenNode endpoints for the selected region | Receiving that profile's credentials and token                               |
| An endpoint the caller overrides                | That route's credential, by the caller's deliberate choice                   |
| Everything else                                 | Nothing                                                                      |

Response bodies, redirect targets, error text, and file contents are data. None
of them can change where credentials go or which command runs.

## Secure defaults

These have no switch to weaken them.

- TLS verification stays on, with bundled roots and no proxy or root store taken
  from the environment.
- Each kind of request has its own rule for redirects, retries, and limits, as
  the [request policies](sdk-requests.md#contract) define. A credential exchange
  and a signed S3 request follow no redirect. A product read follows one only to
  the same scheme, host, and port. No write follows any.
- One command uses one management credential source. The
  [configuration design](config.md) picks one file pair and one source, never a
  mix of greennode's and vngcloud's, and refuses a half-finished move between
  the two. The vCDN API key and S3 object keys are separate credential domains
  with their own sources.
- Read-only is on when any source turns it on, and no source can turn it off for
  another. It is decided offline, before any sign-in.
- The credentials file is refused when group or others have any access to it.
  Token cache files use mode 0600 inside a mode 0700 directory.
- greennode never writes under `~/.vngcloud`, never shares its token cache, and
  refuses a cache directory that resolves inside `~/.vngcloud`.
- Secrets never come from command-line arguments. `configure` reads them without
  echo or from stdin.
- No password, one-time-code secret, client secret, token, authorization code,
  cookie, API key, or `Authorization` header that greennode uses to sign in or
  to send a request is printed, logged, placed in an error, or shown to a query.
  Resource data the API returns for a read, such as a monitored URL's configured
  headers or a server's console log, is printed as the baseline prints it, with
  the baseline's redaction of sensitive keys in untyped maps.
- Debug output is an allow list: the events in the
  [request policies](sdk-requests.md#event-allowlist), the configuration sources
  in the [configuration design](config.md#token-cache-and-debug), and the tool's
  own events in the [CLI design](cli.md#debug-and-dependencies). Nothing else is
  logged.
- A destructive command needs `--yes`. A local refusal sends no request.
- A write whose outcome is unknown is reported as possibly sent and is never
  resent silently.
- A credential source is used whole. A partial set or a failed sign-in never
  falls back to another source.

## Threats

A review of auth, credentials, a write operation, or a release workflow traces
each of these through concrete code paths:

1. A token or credential leaked through a log, an error, debug output, a
   response capture, a query, or a test message.
2. A hostile or malformed API response: oversized bodies, wrong types, truncated
   output, a credential echoed back, and text meant to mislead the user or an
   agent.
3. A redirect, an endpoint override, or a caller hook that sends credentials to
   another host or lets other code see them.
4. A tampered profile, credentials file, or token cache, including symbolic
   links, files other users can write, and a file swapped between a check and
   its use.
5. A repeated or concurrent paid write: retry boundaries, price races,
   cancellation after a send, and who may clean up.
6. A setting inherited from the other tool: a `VNGCLOUD_` variable or a
   `~/.vngcloud` file that selects a different account, or a move to
   `~/.greennode` that drops a read-only setting.

## Limits

Every read fed by a response or a file has a bound. The
[request policies](sdk-requests.md#contract) set the response bounds, the
[configuration design](config.md#offline-stages-and-file-safety) the file
bounds, and the [CLI design](cli.md#input-json) the input bounds. A ported
operation keeps the baseline's bound for that operation.

## Unverified

Tests use loopback servers and fixtures copied from the baseline. They prove the
port matches the Go code, not that the service still behaves that way. A claim
about live behavior stays unverified until an approved live run checks it.
