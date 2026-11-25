# Security

Status: proposed, with the [architecture](architecture.md).

greennode holds cloud credentials and sends writes that can cost money or
destroy resources. Security comes before features and convenience.

## Trust boundaries

| Party                                              | Trusted for                                      |
| -------------------------------------------------- | ------------------------------------------------ |
| The user at the terminal or the calling script     | Choosing the profile, the command, and its flags |
| Files under `~/.greennode/` with owner-only access | Credentials, settings, and cached tokens         |
| The GreenNode endpoints for the selected region    | Receiving that profile's credentials and token   |
| Everything else                                    | Nothing                                          |

Response bodies, redirect targets, error text, and file contents are data. None
of them can change where credentials go or which command runs.

## Secure defaults

These match the baseline and have no switch to weaken them.

- TLS verification stays on.
- A redirect to another host is refused.
- The credentials file is refused when group or others can read it. Token cache
  files use mode 0600 inside a mode 0700 directory.
- Secrets never come from command-line arguments. `configure` reads them without
  echo or from stdin.
- No password, one-time-code secret, token, authorization code, cookie, or
  `Authorization` header is printed, logged, or placed in an error.
- Debug logs carry the method, the URL path without its query, the status, and
  the duration, and nothing else.
- A destructive command needs `--yes`. A profile key, an environment variable,
  or `--read-only` makes every write refuse before any request.
- Each operation keeps the baseline's retry rule. A write whose outcome is
  unknown is reported as unconfirmed and is never resent silently.
- A credential source is used whole. A partial set or a failed sign-in never
  falls back to another source.

## Threats

A review of auth, credentials, a write operation, or a release workflow traces
each of these through concrete code paths:

1. A token or credential leaked through a log, an error, debug output, or a test
   message.
2. A hostile or malformed API response: oversized bodies, wrong types, truncated
   output, and text meant to mislead the user or an agent.
3. A redirect or an endpoint override that sends credentials to another host.
4. A tampered profile, credentials file, or token cache, including symbolic
   links and files other users can write.
5. A repeated or concurrent paid write: retry boundaries, price races, and who
   may clean up.

## Limits

Every read fed by a response or a file has a bound. The baseline sets each
value, and a ported operation keeps the baseline's bound for that operation. A
new bound is recorded here in the change that adds it.

## Unverified

Tests use loopback servers and fixtures copied from the baseline. They prove the
port matches the Go code, not that the service still behaves that way. A claim
about live behavior stays unverified until an approved live run checks it.
