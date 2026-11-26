# Command-line design

One typed declaration drives each operation's parser, guards, help, generated
reference and parity check. An operation in a registered service changes only
that service's files. The CLI calls GreenNode only through `greennode-sdk`.

The baseline's [CLI design][baseline], [read conventions][reads], [usability
design][usability], and [write][writes] and [toggle][toggles] ADRs are unchanged
except where this design or [Configuration](config.md) says otherwise.
[CLI output](cli-output.md) owns projection and rendering. The
[query engine](cli-query.md) owns query semantics.

## Approaches

| Approach                                          | Trade-off                                                                                                                                                                                                                                              |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Hand-written clap `Args` and `Subcommand` derives | Typed parsing, but a second declaration for JSON-only fields, guards and parity. A root enum grows with services.                                                                                                                                      |
| Custom declarative or derive macro                | A declarative macro can emit argument types and metadata; a derive can inspect a struct. Both add a field language and expansion diagnostics. A CLI derive on SDK inputs also crosses the crate boundary; a separate derived mirror duplicates fields. |
| Typed table and clap builders (chosen)            | Like the Go operation table, without reflection. Field access and SDK calls are type-checked; the table explicitly records compatibility metadata Rust cannot infer. No new proc-macro crate.                                                          |

## Operation declaration

Build each declaration over a typed CLI staging value, SDK input and output.
Bind one typed CLI adapter, normally backed by one SDK method. Erase the
executor only after the builder checks those types. Start each invocation with
fresh staged defaults and source presence, not an SDK instance whose constructor
needs required arguments. Merge and validate before creating the SDK input. Each
service exports its descriptor and operations.

| Part          | Contract                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Identity      | CLI name, SDK operation, short help, optional notes and example. Own these in the service file, not in a documentation table.                                                                                                                                                                                                                                                                                                                   |
| Effects       | `Read`, `Write`, `Write, destructive`, `Read, local replacement requires --yes`, or `Write, replacement requires --yes`. The two replacement kinds are `storage get-object` and `put-object`; `--replace` triggers confirmation, while `--yes` alone grants no replacement. `copy-object` stays destructive and also requires `--replace`.                                                                                                      |
| Input fields  | Each descriptor binds a typed accessor to the staging value and declares its exact Go JSON name and wiki type. Generate the parser and JSON setter from the accessor's type where possible. The default public flag is the baseline's kebab-case name (including exceptions such as `Query` → `--search`); allow an explicit name. Use typed codecs for nested records, raw JSON and service-specific encodings.                                |
| Input sources | Derive ordinary flags for strings, `int`, `int64`, `float64`, booleans and string arrays, including their pointer forms. Mark complex or suppressed fields JSON-only. An Input can also use a custom extra flag codec (such as `--allowed-cidrs`), a global `--project-id` binding, or a secret source bound to a dedicated file flag (with stdin only where the baseline allows it). No literal argv value or JSON setter exists for a secret. |
| Extras        | Typed flags with no input accessor supply CLI settings: output paths, waits, sorting, scopes and other local options. Record defaults, exclusive groups, parser shape, and required state regardless of how a flag is implemented. An op can disable `--cli-input-json` or `--query`; S3 object commands disable input JSON.                                                                                                                    |
| Requirements  | Record ordinary required (nonzero in the Go sense), optional overrides for reused inputs such as quotes, and presence-required (explicit non-null assignment, including zero, false or empty when valid). Required pointers must be set; required slices distinguish absent from explicit `[]`. For a required file/CLI-only flag, require that source. Record errors in declaration order.                                                     |
| Hooks         | Named guard phase, typed CLI adapter and explicit [output policy](cli-output.md#operation-policies) for partials, queries and rendering. No hook registers arguments, sends HTTP directly, decides exit status or writes error documents.                                                                                                                                                                                                       |

Use clap's `Command` and `Arg` builders, `try_get_matches_from`, global args,
value parsers, `ValueSource::CommandLine`, `ArgAction::Append` and hidden
subcommands. No parser-level requirement or conflict for a constraint that must
inspect the merged input. Model pure argv conflicts in clap only when that
preserves the baseline's precedence. Repeated scalar flags take the last value.
Repeat a string-array flag once per element without splitting on commas; an
explicit array replaces the JSON array. A boolean accepts bare `--flag` for true
or `--flag=false` with `=`; `--flag false` leaves `false` as an argument. Test
Go numeric and boolean spellings, including signed values; use custom value
parsers where clap's built-ins differ. File paths use native OS strings.
Validate unique names and IDs, flag/global collisions, compatible codecs, and
valid source and requirement combinations at registry build in tests. Help marks
required input, not a required occurrence of a flag that JSON can satisfy.

## Input JSON

The baseline's input merge is unchanged. An absent or empty argument is a no-op.
Otherwise `--cli-input-json` takes an inline JSON object or a literal `file://`
prefix followed by a local path, not a URL. No decoding, remote fetch, or stdin
expansion. Follow input symlinks but check the opened handle is a regular file;
on Unix open nonblocking so a FIFO cannot hang the CLI. Read at most 1 MiB plus
one byte before rejecting oversized files. Ordinary inline input has the process
argument limit, not the file limit. The baseline `pricing estimate` codec caps
both at 1 MiB and requires its own exact keys, no duplicate keys and its
16-level container bound.

Start with operation defaults, overlay JSON, then overlay only explicit flags.
Apply the nonempty global `--project-id` last when the operation binds it to an
input field; JSON can still supply that field. Ordinary JSON matches
case-sensitive top-level Go names and lets the last duplicate top-level key win.
Reject trailing data and unknown nested struct fields; nested codecs keep the
baseline's JSON tags and Go case fallback where applicable. Map keys are data.
Top-level `null` is a no-op for the ordinary decoder; field `null` clears
nullable fields and does not replace nonnullable scalar defaults. It does not
count for presence. Reject secret fields even if null, inline or from a file.
Nested input codecs must not admit a typed secret either. Parse integers at
their declared width, without a float round trip.

Invalid type, unknown field, malformed or trailing JSON, inaccessible or
oversized file, missing required input, and wrong flag form are `InvalidUsage`.
Name the flag and safe field or reason without including supplied values,
document excerpts, raw clap diagnostics or untrusted file contents. Dedicated
secret-input file flags keep the operation's bounds and safe opened-file checks.
For secret output, check the path before dispatch, create exclusively without
following a symlink, and keep the baseline's per-operation cleanup and recovery
on failure. The secret file uses Unix mode 0600; only the safe output reaches
stdout or `--query`.

## Root and process

Keep the baseline's eight persistent flags, accepted before or after the
subcommand: `--profile`, `--region`, `--project-id`, `--output`, `--query`,
`--yes`, `--read-only`, `--debug`. Pass nonempty profile, region and project
overrides to configuration. An environment-selected profile is not explicit. Use
nonempty `--output`, then the selected profile's `output`, then `json`; allow
only `json`, `table`, `text`. No output environment variable. Version, help and
doc generation never load a profile or a client.

No arguments at the root or a service prints help on stdout with exit 0, as does
explicit help. Unknown commands, flags or extra arguments emit a usage error
document; only declared preflight outcomes also reach stdout. Do not add a
completion subcommand. Use a custom root version flag rather than clap's `-V`.
`version`, `--version` and `-v` print the same line
`greennode <version> <os>/<arch>` using the package version and target platform,
with baseline platform names such as `darwin/arm64` and `windows/amd64`. Only
the root accepts the version flags. Service leaf operations take no positionals.
Hidden `gen-docs <dir>` takes one directory and writes deterministic `CLI.md`
and `CLI-<Service>.md` from the registered declarations, sorted by service and
operation. Remove stale `CLI-*.md` only when marked as generated by this tool.
The generator writes documentation, not profile files or resources, and ignores
read-only.

`main.rs` constructs `Invocation`: native argv, an immutable injected
configuration source (environment view and optional home), stdin/stdout/stderr,
a terminal adapter and terminal detection per stream, and a running clock,
cancellable timer and request context. Only `main` obtains these from the real
process. Test callers supply their own values, including the SDK source; never
mutate global environment or use a global test override. The CLI opts into the
SDK cache at `~/.greennode/cache`; the SDK owns validation and no-home handling.

Install the Ctrl-C handler before running the command and handle SIGTERM on
Unix, as the baseline does. Cancel the request context, including lock waits,
terminal input and SDK retries or polling. Await dispatch classification and
cleanup before emitting the result; do not drop a write future or call exit from
the handler. Cancellation exits 1, including during login. Settlement still wins
when a mutation may have been sent and the SDK marks it uncertain. Only `main`
returns the resulting process exit status.

## Dispatch and guards

Use the [SDK](sdk.md) offline `Config::load(ctx, options, source)` for a
management operation. It resolves the profile, reads both selected files and
selects credentials once without authenticating. The
[configuration design](config.md) owns profile rules and the read-only latch.

The ordinary runner uses these phases, in order:

1. **Before input.** Parse argv and run argv/source guards such as a refusal of
   literal passwords before decoding JSON. No files open here.
2. **Merge and input guards.** Read only ordinary `--cli-input-json file://` if
   given, then merge flags and the global project binding. Run input guards,
   including a Synthetic quota mutation's query prohibition. `network add-route`
   and `remove-route` need `--yes` here even though not destructive. With
   `--read-only` and no `--yes`, `add-route` returns `InvalidUsage`, not
   `ReadOnly`. Check dedicated file flag shape here without opening its path.
3. **Local checks.** Refuse a write on flag or environment read-only. Then check
   required input, compile permitted `--query` once, validate explicit output,
   and enforce destructive `--yes`, in that order. No files open here. False
   never clears another latch.
4. **Offline settings.** Open only the selected configuration and management
   credentials files via `Config::load`. A load error wins over the profile
   latch. Resolve the output format and then enforce the latch from the loaded
   configuration's profile settings before building the client.
5. **Prepare and execute.** Read dedicated secret or transfer inputs and
   validate local file prerequisites before the first request. Construct the SDK
   input and call the CLI adapter once. The adapter invokes SDK methods; only
   those methods can access the network. The CLI never retries a write.
6. **Finish.** Apply the operation's output policy. Preserve typed failure,
   partial output and dispatch state; render before printing the error line.

Settings-only operations use `Config::resolve_profile(ctx, &options, source)`,
never a management client built from those settings. Native S3 checks
flag/environment read-only before accessing profile files on writes; it reads
only the config settings, never management credentials. Follow the
[policy-only profile rules](config.md#offline-resolution-and-file-safety): a
non-default profile must have a config section and malformed `read_only` fails
even on reads. Check both before opening S3 credentials or transfer input.
Explicit management region/project flags are refused. `get-object` is a read;
replacing a local file still needs `--replace --yes`.

All local refusals send zero requests, including sign-in. A paid operation's SDK
method may then authenticate and get a quote, even by POST. An over-cap or
unpriced quote sends no mutation. The cap is not a server guarantee.

A multi-step adapter declares each SDK call, local step, allowed compensation,
and cancellation path. `storage create-s3-key` creates, optionally attaches, and
only then persists the secret. On a missing secret, failed attachment or failed
save it compensates with a delete of that proven key. `compute create-ssh-key`
and `iam create-service-account` also have failure deletes; the IAM adapter must
prove ownership first. Use the same account and product policy and a fresh
10-second context detached from caller cancellation for those deletes only.
Never compensate on a guessed ID, retry the original mutation or expose an
unpublished secret. If cleanup cannot be confirmed, return the safe
reconciliation guidance. Each adapter tests cancellation, failed cleanup and no
delete without a proven target.

## Errors and exit codes

One classifier returns the stderr envelope and status from typed facts, not
message matching. Stderr has one compact JSON error line, with keys in order:
`{"error":{"code":"…","message":"…","status":404,"operation":"…"}}`. Keep the
SDK's safe public message, including outer reconciliation advice; omit status
unless positive and operation unless known for that class. Service-level
exceptions retain their baseline approved fields. Never print raw errors,
secrets, argv or discarded response data. Debug, prompts and specified hints are
separate from the error document.

The error name and exit choice have separate ranks, as in the baseline. For the
name, take the first matching row and the first matching identity within it; not
all causes contribute metadata. `QuotaChanged` applies only when `NotSettled` is
absent. The reset-password file failure outranks the nested write outcome.
Unconfirmed object copy/delete use a fixed operation and can carry the nested
status; unconfirmed object write can also carry API operation.
`HistoryIncomplete` can carry both. Real or mapped not-found wins over the
nested API code. Other API codes use SDK status fallback when the service
supplies no code, then `RequestFailed` if neither is available.

| Name precedence, highest first | Codes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Service and outcome identities | `HistoryIncomplete`, `QuotaChanged`, `QuotaStatusIncomplete`, reset `SecretFileFailed`, uncertain IAM `RequestFailed`, `ObjectCopyUnconfirmed`, `ObjectDeleteUnconfirmed`, `ObjectWriteUnconfirmed`, `DiscoveryIncomplete`, `EstimateLimit`, `EstimateOverflow`, `BucketEncryptionIncomplete`, `PurgeCooldown`, CDN `StatusUnconfirmed`, CDN `UnexpectedStatus`, CDN `ResourceBusy`, monitor `StatusUnconfirmed`, `UnexpectedStatus`, `OTPRejected`, `ZoneBusy`, `NotSettled`, `WriteFailed`, `OpenLogin`, `VolumeInUse`, `BootVolume`, `ServerRunning`, `SystemTag`, `RepositoryNotEmpty`, `PaymentRequired`, `ProjectNotEmpty`, `BucketNotEmpty`, `UserNotFound`, `SystemSecurityGroup`, `SecurityGroupInUse`, `ServerGroupInUse`, network/LB `ResourceInUse`, `DefaultResource`, network/LB `ResourceBusy`, `PriceAboveMax`, `Unpriced`, `SelfChange`, `PrivilegedChange`, `ManagedPolicy`, IAM `ResourceInUse` |
| Not-found, then API            | `NotFound`, then the API code or `RequestFailed`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Local and shared               | `InvalidUsage` (CLI), `ReadOnly` (CLI), `SecretFileFailed` (ordinary), `QueryFailed`, `NoCredentials` (including vCDN and credential files), `TokenCacheFailed`, `ServiceAccountLoginFailed`, `LoginFailed`, `InvalidUsage` (SDK invalid input), `InvalidConfig` (including ambiguous project), `PageFormat`, `RequestFailed`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |

For exit status, first match wins:

| Exit | Condition                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ---- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0    | Success                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 1    | `NotSettled`, `WriteFailed`, `StatusUnconfirmed`, `UnexpectedStatus`, `OpenLogin`, `QuotaStatusIncomplete`, reset-secret file failure, IAM uncertain/create-unconfirmed/origin-not-IAM, object copy/delete/write unconfirmed or exists, `DiscoveryIncomplete`, `BucketEncryptionIncomplete`, storage attach failure, `PurgeCooldown`, CDN busy, `ZoneBusy`, registry create `UserNotFound`, `OTPRejected`, `VolumeInUse`, storage `PaymentRequired` or `ProjectNotEmpty`. These identities beat nested cancellation, auth and not-found. |
| 1    | Cancellation or deadline, anywhere in the cause chain                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 3    | Missing credentials, credentials file error, generic login failure, auth identity, or HTTP 401. Unmatched HTTP 403 is not an auth exit.                                                                                                                                                                                                                                                                                                                                                                                                  |
| 4    | SDK not-found identity or first API detail status 404                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 2    | Usage, local read-only, invalid input, invalid config or ambiguous project. Keep the API code when a server-side error carries invalid-input identity.                                                                                                                                                                                                                                                                                                                                                                                   |
| 1    | Everything else, including token cache, noncancelled query execution and ordinary render failures                                                                                                                                                                                                                                                                                                                                                                                                                                        |

Use SDK `api_error()` for the first API detail in preorder (outer error, then
children in cause order). Use `has_identity` for typed facts anywhere in the
tree, not classes derived only from HTTP status. A status-only `500` then `404`
is exit 1, not 4; with a nested not-found identity it is 4, absent an outranking
fact. HTTP 401 checks only the first API detail too.

`errors.rs` owns this algorithm. Service descriptors supply typed rules; match
by identity, not message, across all causes. A common identity and its aliases
have one rule. Reject conflicting identities or duplicate name ranks at registry
construction. Exit tiers can share one outcome. Registry order never resolves a
tie. Test first-detail versus nested-identity cases and reject ambiguous rule
registration.

Render safe partial output under the operation's
[output policy](cli-output.md#operation-policies). A render failure cannot
replace the operation error or trigger another SDK call. On a clap parse error,
use a diagnostic-only pass of the same tree with `ignore_errors(true)` to select
a declared preflight outcome. Never execute those matches or change the usage
error.

## Configure

Keep the baseline's four command forms, key file/section mapping, prompt order,
blank-to-keep behavior, stdin limit and masks. On Unix `configure` requires
terminal stdin, prompts on stdout, and reads secrets without echo. On Windows
every `configure` write refuses with `InvalidUsage`/2 before reading stdin or
touching a file, and tells the user to edit the files by hand; `get` and `list`
still work. `configure set <key> -` requires nonterminal stdin, reads at most
4096 bytes plus one, strips one final LF or CRLF, and rejects an empty value or
embedded newline. Never accept literal `password`, `totp_secret`, `vcdn_api_key`
or `client_secret`. None of the commands authenticate. `get <key>` prints the
stored value and `list` prints the stored keys in fixed order, with `****` for
nonempty secrets. Both ignore output/query selection and remain available under
read-only and in legacy mode. Keep the missing-region hint on stderr, using the
selected paths and prefixes. Escape terminal controls in displayed settings.

Resolve initial paths with the configuration design's profile-only reader. A
write checks the effective read-only latch and refuses legacy mode (exit 2, with
manual copy instructions owned by [Configuration](config.md)) before reading
stdin, prompting or creating files. There is no migration or write under the
legacy tree, including through an alias or path override. Before the recheck,
take persistent sibling lock files for the selected config, which holds the
latch, and for every file the command edits, deduplicated by file identity and
acquired in canonical path order. Hold them through publication. Never lock only
the file that the rename replaces. Then apply the configuration design's
[under-lock recheck](config.md#configure) and cancellation checks. No command
can clear a live profile latch.

Preserve unrelated INI lines, comments, BOM and sections. Replace the effective
key, not an earlier duplicate. Use same-directory exclusive temporary files,
sync, then replace atomically; remove an unpublished temp on failure. For
interactive edits, validate all input first and publish credentials before
config; the two renames are not a single transaction. Unix creates default
directories with 0700 and both edited files with 0600. Explicit path overrides
need existing parents. Windows uses handle-based locking, safe replacement
without unlinking first, and bounded retries on sharing failures with the same
guard on every attempt. Do not claim POSIX mode enforcement on Windows.
Configuration owns alias/path refusal and platform file safety details.

## Differences from the baseline

These add to the names and paths in the
[architecture](architecture.md#differences-from-the-baseline):

- Help and usage text come from clap, so their wording and layout differ.
  Command names, flag names, and the stream and exit code of each help or usage
  path do not.
- A usage error names the flag and a safe reason. It never repeats a supplied
  value or a raw parser diagnostic. Dedicated file contents are checked after
  read-only; ordinary `--cli-input-json file://` retains its earlier phase.
- `configure get` and `list` escape terminal control characters in displayed
  settings.
- Any output failure after a confirmed write, not only a query failure or a
  table limit, says the write succeeded and must not be retried.
- When rendering a partial output fails, the operation's own failure stays on
  stderr and decides the exit code.
- On Windows `configure` has no writes in the first releases; edit the files by
  hand.
- [Output key normalization](cli-output.md#redaction) also redacts sensitive
  dynamic-map keys surrounded by non-ASCII letters or digits.
- The [query engine](cli-query.md) bounds query length and depth, reports
  evaluation errors rather than panicking, and fixes object traversal order.

## Debug and dependencies

`--debug` alone enables a typed stderr event sink. Allow only a request's
method, safe route path without query (use a template for sensitive segments),
status and elapsed time; fixed login and write start/end events with public
operation identity; config provenance as known variable names or default
directory labels. No arbitrary paths, user/profile values, headers, bodies,
query strings, URLs, error chains or credential-exchange internals. No logger or
capture hook sees the private exchange. Do not forward SDK or transport log
records verbatim.

| Choice                                                                       | Evidence and dependency scope                                                                                                                                                                                                            |
| ---------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [clap](https://crates.io/crates/clap) 4.6.10, MIT or Apache-2.0              | Recent release; use builder, `std`, `help`, `usage`, `error-context`, no derive or color feature needed for the frame. `clap_builder` uses `clap_lex` and `anstyle`; exclude optional terminal width, colors and suggestion stack.       |
| [Tokio](https://crates.io/crates/tokio) 1.54.1, MIT                          | Shared SDK runtime, released October 2026. Add `signal` for [Ctrl-C](https://docs.rs/tokio/latest/tokio/signal/fn.ctrl_c.html); shared timer and Unix signal support bring `mio` and `signal-hook-registry`, Windows uses `windows-sys`. |
| [rustix](https://crates.io/crates/rustix) 1.1.6, Apache-2.0 or MIT available | October 2026 release. Use safe Unix [termios](https://docs.rs/rustix/latest/rustix/termios/) and `poll`. Rustix uses `bitflags` and platform `errno`/`libc` or `linux-raw-sys`. No direct console FFI.                                   |

On Unix use the injected stdin handle, keep normal line editing, turn echo alone
off during secret input and restore the mode on every exit. Wait on stdin
readiness with a cancellation wakeup. `rpassword` has [blocking TTY
APIs][password], not the required cancellable terminal contract. On Windows a
plain process-owned reader thread sends at most 4097 bytes to the runner. It
never edits files. On cancellation, drop the receiver and exit without joining a
blocked read. Do not use a Tokio blocking task or console FFI. Use standard
[file locks][file-lock] where supported. Run dependency and crate-age checks.

## Layout, parity and tests

`crates/greennode/src/main.rs` owns OS adapters. One frame owner maintains
`src/cli/`: `root.rs` and `op.rs` for construction and execution, `input.rs`,
`configure.rs` and its file helpers, `render.rs`, `query/`, `errors.rs` and
`gendocs.rs`. A service owner owns `src/services/<service>.rs`,
`tests/<service>.rs` and service fixtures. Register a service once in
`src/services/mod.rs` with its module and descriptor; root, docs and parity walk
that same registry. No edits to the root for new operations.

`crates/greennode/tests/parity/commands.json` is an independent committed
extraction of all 373 operation headings in the 19 baseline service wiki pages.
Use a `services` array of named services with `operations` arrays. Each op has
`name`, `kind` (the exact page text, excluding the terminal period), `flags`
(`name` without `--`, exact Go `type`, boolean `required`) and `json_fields`
(exact field `name`, Go `type` and boolean `required`). A flag row is not a
JSON-only field. Compare each registered declaration and actual clap argument
inventory against the applicable baseline entries, handling persistent flags and
implicit help/JSON arguments separately. An absent unported op is fine; extra,
duplicate or mismatched entries are failures.

The wiki proves inventory and stated metadata, not parser defaults, exclusive
flags, side effects or output. Independently port root, version and hidden
`gen-docs` tests; verify positional counts, global flags before and after the
leaf, implicit flag presence/absence, empty input, real parser types, defaults
(such as quota `--max-days-left=7`), and mutually exclusive options (such as
`--zone-id` and `--volume-type-zone-id` on `volume list-volume-types`). Run
`gen-docs` twice and compare bytes to committed generated pages. Run dedicated
`configure` syntax and behavior tests, including all key positions and masking.
Generate `crates/greennode/tests/parity/shipped-commands.json` with the built
candidate's hidden `__command-inventory` command. It prints a JSON array of
command lines as typed, each starting with `greennode`, sorted by UTF-8 bytes,
with hidden commands excluded. The built-ins are exactly `greennode`,
`greennode --help`, `greennode --version`, `greennode -h`, `greennode -v`,
`greennode help`, and `greennode version`; each visible service adds
`greennode <service>`, and each operation adds
`greennode <service> <operation>`. The Rust `shipped_commands_match_registry`
test compares a fresh export byte for byte and fails on a stale file. The
[release gate](release.md) also checks the preceding published tag's inventory
is a subset of the candidate.

Use injected streams, sources, terminal and clock in frame tests. Use temp homes
and loopback HTTP in service tests. Assert exact stdout, stderr and exit
together, especially partial output and competing failure classes. Prove zero
requests for local refusals, quote reads but no mutation for price refusal, and
the declared mutation sequence after all checks. Test S3 named-profile absence
and malformed latch on reads with zero requests and no S3 credential opens.
Exercise both prefixes, file selection, guard priority, quote reads, EOF,
cancellation, Unix echo restoration, Windows prompt refusal and pipe input, lock
contention, permission checks and atomic replacement. Test projections against
the [baseline goldens][goldens].

[baseline]: https://github.com/dannyota/vngcloud/blob/v1.7.0/docs/design/cli.md
[reads]:
  https://github.com/dannyota/vngcloud/blob/v1.7.0/docs/design/cli-reads.md
[usability]:
  https://github.com/dannyota/vngcloud/blob/v1.7.0/docs/design/cli-usability.md
[writes]:
  https://github.com/dannyota/vngcloud/blob/v1.7.0/docs/adr/0002-write-api-conventions.md
[toggles]:
  https://github.com/dannyota/vngcloud/blob/v1.7.0/docs/adr/0003-toggle-writes.md
[password]: https://docs.rs/rpassword/7.5.5/rpassword/
[file-lock]: https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock
[goldens]: cli-output.md#goldens
