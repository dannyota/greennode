# SDK design

`greennode-sdk` exposes an async Rust client per service. Clients built from one
`Config` share a session. A failed operation can return a typed error and
partial output without losing evidence that a write may have reached the API.
The CLI uses only the public SDK. The SDK never prints, prompts, or exits.

The baseline's [SDK and CLI design][go-sdk], [service-account auth][go-auth],
[shared waits][go-waits], and [write conventions][go-writes] apply unchanged
except as specified here and in [request policies](sdk-requests.md).
[Configuration](config.md) owns setting precedence, file safety, and fallback.
[CLI](cli.md) owns guards, projection formatting, and exit codes.

## Approaches

- Translate Go signatures and HTTP hooks directly. Familiar to Go callers, but
  exposes credential paths and adds pointer patterns Rust does not need.
- Use one generic request client with generated models. Less repeated code, but
  loses operation-specific safety and output semantics without a schema.
- Use typed service clients on a small shared core. Take this approach so a
  service owns all its operations while the core owns credentials and sends.

## Public surface

`ConfigOptions::default()` starts an owned builder. Each `with_*` returns
`Self`. Repeated options of the same kind use the last value; different auth
kinds use the configuration design's winner rule, not call order. Constructors
consume options and snapshot built-in credentials. Caller providers and sinks
are shared via `Arc`, not copied. `Config` is cloneable, not defaultable, and
has no public mutable fields.

| Go item                                                         | Rust item or difference                                                                                                                                                                                                                                      |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `Config`, `LoadOption`                                          | `Config`, `ConfigOptions`; no externally implementable option trait.                                                                                                                                                                                         |
| `NewConfig`, `LoadConfig`                                       | `Config::new(options)` is synchronous and options-only. `Config::load(ctx, options, source).await` resolves local inputs then builds the same configuration. Neither logs in, probes a service, or creates cache files. Both return `Result<Config, Error>`. |
| `Config.Region`, `Config.ProfileSetting`                        | `region() -> &str`, `profile_setting(key) -> Option<&str>`. `None` means absent; an options-only configuration has no settings. Never expose credential-file values here.                                                                                    |
| `Config.Authenticate`                                           | `authenticate(ctx).await -> Result<(), Error>`. Runs the selected provider without exposing a token or discovering a project.                                                                                                                                |
| `WithRegion`, `WithProjectID`                                   | `with_region`, `with_project_id`. A supplied project ID bypasses selection by discovery, not reads that need its metadata.                                                                                                                                   |
| `WithProfile`, `WithConfigFile`, `WithSharedCredentialsFile`    | `with_profile`, `with_config_file`, `with_shared_credentials_file`; paths are `PathBuf`. `new` ignores file path options; the name still scopes an opt-in cache.                                                                                             |
| `IAMUserAuth`, `WithIAMUser`                                    | `IamUserAuth`, `with_iam_user`; move an owned credential snapshot into the builder.                                                                                                                                                                          |
| `TOTPProvider`, `TOTPFunc`, `SecretTOTP`                        | `TotpProvider`, `TotpFn`, `SecretTotp`; the adapter wraps an async closure. A code is a `Secret`, not a printable string.                                                                                                                                    |
| `ServiceAccountAuth`, `WithServiceAccount`                      | `ServiceAccountAuth`, `with_service_account`; client ID and `Secret` client secret. Private fixed exchange.                                                                                                                                                  |
| `CredentialsProvider`, `Token`, `WithCredentialsProvider`       | `CredentialsProvider`, `Token`, `with_credentials_provider`. See the contract below.                                                                                                                                                                         |
| `WithStaticToken`, `WithCDNAPIKey`                              | `with_static_token`, `with_cdn_api_key` each take a `Secret`. A CDN key does not select a management identity.                                                                                                                                               |
| `WithTokenCache`                                                | `with_token_cache(PathBuf)` opts the two built-in logins into disk cache, subject to the [cache directory check](config.md#token-cache-and-debug). Static tokens and custom providers never use it. No implicit disk cache.                                  |
| `EndpointOverrides`, `WithEndpointOverrides`                    | `EndpointOverrides`, `with_endpoint_overrides`; validate bases before use. Keep distinct product and IAM-user destinations. `GLB` and `VCR` synonyms go away; use `global_load_balancer` and `container_registry`.                                           |
| `WithHTTPClient`, `WithTransport`                               | No raw client or round-tripper option. `with_http_options(HttpOptions)` controls the product connect timeout. The SDK owns TLS, origins, and single-send enforcement.                                                                                        |
| `WithTimeout`, `WithRetry`, `WithUserAgent`                     | `with_timeout(Duration)`, `with_retry(RetryOptions)`, `with_user_agent`. Options never widen an operation policy.                                                                                                                                            |
| `WithLogger`                                                    | `with_logger` takes a `Send + Sync` sink for typed, safe `DebugEvent`s. Silence by default; no global logger.                                                                                                                                                |
| `ResponseCapture`, `ResponseCaptureFunc`, `WithResponseCapture` | `ResponseCapture`, `CaptureSink`, `with_response_capture`. Product response metadata only. Drop the Go body and URL fields; never give a hook upstream data or secrets.                                                                                      |
| `DefaultPage`, `DefaultPageSize`, `Ptr`                         | `DEFAULT_PAGE`, `DEFAULT_PAGE_SIZE`; use `Some(value)` instead of a pointer helper.                                                                                                                                                                          |
| `Secret`, `Secret.Reveal`                                       | `Secret::new`, `Secret::expose_secret`; deliberate exposure only. See [outputs](#outputs-and-projection).                                                                                                                                                    |
| `context.Context`                                               | `&RequestContext` on every operation, auth, and load. No arbitrary caller values.                                                                                                                                                                            |

`EndpointOverrides` keeps one slot for each canonical Go field: `vserver`,
`vlb`, `vnetwork`, `global_load_balancer`, `dns`, `container_registry`,
`portal`, `signin`, `dashboard`, `token`, `billing`, `cdn_docs`, `monitor`,
`iam`, `storage`, `cdn`, `vks`, `vserver_backup`, `backup_center`, `database`.
`signin`, `dashboard`, and `token` affect IAM-user login only. No override
selects the service-account or signed S3 host. `IamUserAuth` accepts the root
email, username, password `Secret`, and optional `Arc<dyn TotpProvider>`. Keep
its `SigninBaseURL`, `TokenURL`, and `DashboardURI` as `signin_base_url`,
`token_url`, and `dashboard_uri`, with precedence over the corresponding
`EndpointOverrides` fields. Drop `IAMUserAuth.HTTPClient` to confine login. Both
management constructors validate the region and only the winning auth source;
partial sets never trigger another provider.

### Providers and session

`CredentialsProvider: Send + Sync` has `token(&self, ctx)` returning
`Result<Token, Error>` and `invalidate(&self, ctx, rejected: &Secret)` returning
`Result<(), Error>`. Both return boxed `Send` futures tied to the borrows so
`Arc<dyn CredentialsProvider>` works without an adapter crate. `TotpProvider`
uses the same shape for `code(&self, ctx) -> Result<Secret, Error>`. Invoke
caller code only at the provider boundary, never on an HTTP request or with
built-in credentials. Providers return `Error::provider_failed()` for an
ordinary failure; never forward an underlying provider diagnostic. Providers
must cooperate with cancellation and may not retain references or be required to
complete after their future is dropped.

`Token` holds the access `Secret` and an optional UTC expiry. The equivalent of
Go's `NeedsRefresh` is
`Token::needs_refresh(&self, now: OffsetDateTime) -> bool`. Use `now` from the
injected clock. An empty token, absent expiry, or less than 30 seconds remaining
means refresh. With no expiry, ask the provider again on each logical request.
Reject empty or unsafe bearer values before sending. Invalidate only the exact,
non-empty token sent on a 401; the provider must compare before evicting. A
custom provider owns its own cache. Built-in auth keeps the baseline age,
pending invalidation, TOTP marker and token cache rules. Static tokens do not
refresh. Never refresh on 403.

`Config` holds `Arc<Session>`. A session owns auth kind, token state, projects,
validated product endpoints, and runtime settings. Cloning it shares refresh and
project discovery gates. Separate constructors create separate memory state even
with identical credentials. Keep the baseline's project selection and discovery
contracts; common discovery does not call a service module. The first holder
performs the work. A canceled waiter exits without canceling that holder. A
canceled holder releases the gate and leaves pending token invalidation intact;
a live waiter can take over. Recheck the caller and cache state after
acquisition and before publishing a result. Do not publish a token or project
from a failed or canceled request. Keep the baseline's short suppression of
rejected IAM-user logins. Do not hold a blocking mutex across an await.

The session exposes internal equivalents of `ClientOf`, `ProjectIDContext`,
`RequireProjectID`, `RequireProject`, and the auth-kind and endpoint queries.
Go's remaining internal client methods are not public Rust API. Keep the
selected auth kind and whether overrides were used so the service guards do not
mistake a service-account login for an IAM-user login.

### Local inputs and time

`ConfigSource: Send + Sync` supplies `var_os(name)` and `home_dir()` as OS
values. Only `ProcessConfigSource` reads the SDK process environment or home.
`Config::load` always takes an explicit source; the CLI passes its own snapshot
of allowed process inputs. `Config::new` does not consult a source, even for
home, TLS roots, or proxy settings. Use temporary directories, not a fake
filesystem, to exercise file checks and locking.

- `async fn load(ctx: &RequestContext, options: ConfigOptions, source: &dyn ConfigSource) -> Result<Config, Error>`
- `fn new(options: ConfigOptions) -> Result<Config, Error>`
- `async fn resolve_profile(ctx: &RequestContext, options: &ConfigOptions, source: &dyn ConfigSource) -> Result<ResolvedProfile, Error>`

All three are associated functions on `Config`. `load` resolves the profile,
checks and reads both selected files, and selects credentials in one offline
call with one options value and one source, opening each selected file once, as
the [configuration design](config.md) specifies. Never reuse a settings-only
result to construct a management config.

`resolve_profile` returns an opaque `ResolvedProfile` with `name()`,
`profile_setting(key)` (the same rules as on `Config`), `section_present()`
(whether the selected config file has the profile's section, even empty), and
`config_source()` and `credentials_source()`, each the fixed value-free label of
the selected file or `None`, without requiring a region or credentials. The CLI
reads the profile latch as `profile_setting("read_only")` and validates it;
`load` does not. It never signs in, opens the credentials file for content, or
reads credential values from the environment. `configure`, standalone discovery,
and S3 object commands may use this result without loading management
credentials.

For a management write, the CLI first checks the flag and environment read-only
guards without reading a configuration, credentials, secret, or transfer file; a
`file://` input argument keeps its baseline place. It then calls `load`. A load
error wins over a profile read-only refusal. If load succeeds, check the profile
latch on that `Config` before building a service client.

`RequestContext` carries a cloneable cancellation signal and optional monotonic
deadline. A separate handle cancels the request. A child deadline cannot extend
its parent's. Check it before local work and each send; race every network,
provider, gate, lock, backoff, settlement, or streaming wait against
cancellation and deadline. Dropping the operation future stops its work but
cannot return a failure; use the signal to receive `Cancelled` or
`DeadlineExceeded` and send state. Do not detach requests. Closing a connection
does not undo a write.

`Clock` supplies running UTC and monotonic time and an async sleep until an
instant. A `RequestContext` deadline and every timer the SDK itself creates use
the same clock as the request. Timers inside hyper-util's connector use Tokio
time; test them with paused Tokio time, not the injected clock.
`ConfigOptions::with_clock` and the S3 runtime input select the clock; the
default reads system time and uses Tokio timers. `Random` is internal:
production reads the OS CSPRNG, and tests inject bytes for PKCE, temporary
names, and retry jitter. Randomness failure during auth fails closed. Test time
drives cache expiry, TOTP rollover, HTTP deadlines, signing, and settlement
together.

## Services and models

Each public `<service>::Client::new(&Config) -> Client` is infallible,
cloneable, `Send + Sync`, and performs no I/O. Each operation is
`async fn get_server(&self, ctx: &RequestContext, input: GetServerInput) -> Result<GetServerOutput, Failure<GetServerOutput>>`.
Use one input value even for reads with no arguments. Required fields go in an
input constructor; offer `Default` only where all fields are optional. Perform
baseline required-field, shape, and write path checks before any request.
Operation names use lower snake case, and types use UpperCamelCase: `ListVPCs`
becomes `list_vpcs` and `ListVpcsInput`; `ID` in a field becomes `id`. Preserve
Go names separately for CLI serialization, never derive them from Rust names.

| Go shape                              | Rust shape                                                                                                                                                                                                                                                                            |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Optional pointer or nil update member | `Option<T>`; `Some(false)`, `Some(0)`, empty collections and explicit null retain their operation-specific meaning. Use a three-state field if the wire distinguishes omitted from null.                                                                                              |
| Slice or map                          | Use `Vec<T>` for an ordinary output slice, `Option<Vec<T>>` for a pointer to a slice, and `BTreeMap<K, V>` for a map. Keep map nullability explicitly. Use the operation's encoding for bytes.                                                                                        |
| Scalar                                | Exact `i64` or `u64` for Go integers, regardless of target pointer width; narrower type only where the contract says so. Keep exact numeric strings and decimals when the baseline does.                                                                                              |
| `time.Duration`, `time.Time`          | `Duration` for nonnegative durations, `time::Duration` when negative is meaningful. Time values use `time::OffsetDateTime` with explicit wire and projection formats. Preserve the baseline's precision, offsets, units and zero/absent distinction.                                  |
| `core.List[T]`, `core.PagedList[T]`   | `List<T>`, `PagedList<T>` with `items`, and for paged shapes `page`, `page_size`, `total_page`, `total_item`. Keep `Page`/`Size` inputs, defaults and operation caps. No implicit walk; preserve operations that already walk, and purpose-built outputs such as continuation tokens. |

Keep-or-differ examples:

| Go package                                                              | Rust item or difference                                                                                                                                                                                                 |
| ----------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `project.New`, `Project`, `ListProjects`                                | `project::Client::new`, re-export the shared `Project`, and expose `list_projects` with `ListProjectsInput` and `ListProjectsOutput = List<Project>`. Keep explicit listing separate from the session's chosen project. |
| `billing.New`, `ListBudgets`, `UpdateBudget`, `ErrHistoryIncomplete`    | `billing::Client::new`, `list_budgets`, `update_budget`, and `ErrorClass::HistoryIncomplete`. Keep named input/output/models, page metadata only where present, and billing envelope parsing private to billing.        |
| `network.New`, `ListVPCs`, `ListServersBySecurityGroup`, `ErrOpenLogin` | `network::Client::new`, `list_vpcs`, `list_servers_by_security_group`, and typed `network::ErrorDetail::OpenLogin`. Keep the paged VPC result and the unpaged server result.                                            |
| `storage.New`, `NewObjectClient`                                        | `storage::Client::new(&Config)` for management, `storage::ObjectClient::new(ObjectConfig) -> Result<ObjectClient, Error>` for S3. No conversion between them.                                                           |

In billing, also keep `GetBudget`, `GetCurrentPeriodCost`,
`ListBudgetThresholds`, `ListBudgetAlerts`, `CreateBudget`, `DeleteBudget`,
`CreateBudgetThreshold`, `UpdateBudgetThreshold`, `DeleteBudgetThreshold`,
`GetCostOverview`, `ListCostResources`, `GetBalances`, `ListResources`,
`ListTransactions`, and `ListPayments`. Apply the same naming and type rules to
each method, its named input and output, and billing's exported resource models.
An empty named output remains a struct that projects to `{}`, not Rust `()`.

Core never imports a service. Give models with multiple owners one definition in
private `model/` and re-export them at their natural service paths. For example,
`compute::Server` re-exports `model::server::Server`; network uses the same type
without importing compute. Apply the same rule to project and pricing/discovery
metadata. Share request building and quote/settlement logic only when two
services need it. Move that logic below both callers in a contract owned by
core; no service imports a sibling's implementation. Keep service-specific
validation, envelope decoding, and path templates local.

## Results and errors

`Failure<T>` owns an `Error`, `Option<T>` partial output, and `WriteDispatch`.
It offers `error()`, `partial()`, `write_dispatch()` and `into_parts()`; `Debug`
reports whether a partial exists, not its contents. `WriteDispatch` is
`NotAWrite`, `NotSent`, or `MayHaveBeenSent`. A write starts `NotSent`,
including when only a quote, authentication, or other preflight reads have run.
Once a mutation might have crossed the send boundary, retain `MayHaveBeenSent`
through every error, including a 401, cancellation, decode failure, or
settlement read. A response is evidence of a send, not necessarily of a state
change. Retryability is a separate fact. Never infer it from partial output or
dispatch alone.

`Error::class()` returns a non-exhaustive `ErrorClass`; `has_class` also tests
retained, typed causes. Accessors expose optional `ApiError`, `LoginError`, and
`ServiceAccountLoginError` data from the error or its causes, outermost first.
`ApiError` holds a static operation ID, optional HTTP status, safe code and
message, and the baseline's retryability hint. Login diagnostics hold a fixed
reason and optional status; only IAM-user login has a captcha hint. A
configuration file failure attaches a public `FileDiagnostic` with `slot()`
(`FileSlot::Config` or `FileSlot::Credentials`), `source()` (the fixed
value-free label), `reason()` (a fixed `&'static str`) and `line()`
(`Option<u32>`); it never holds a path or value. Preserve status when the
operation contract says it is observed. No raw HTTP, decode, filesystem, or
caller-provider error is reachable through `source()` or debug.

| Baseline identity                                                    | Rust class or diagnostic                                                                                                                                                         |
| -------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ErrNotSettled`, `ErrFailed`                                         | `NotSettled`, `Failed`; retain underlying API/cancellation cause without promoting it over settlement.                                                                           |
| `ErrAuth`, `*LoginError`, `*ServiceAccountLoginError`                | `Auth`, with the corresponding login detail when applicable. A caller cancellation or deadline during login retains detail but matches termination, not `Auth`.                  |
| `ErrTokenCache`                                                      | `TokenCache`; only a fixed safe message. An ended caller context at that boundary wins.                                                                                          |
| `*APIError`                                                          | `Api` or its semantic class plus `ApiError` detail. Use the baseline status fallback for empty, null or status-equal codes. A synthetic or transport failure can have no status. |
| `ErrNotFound`, `ErrPermission`, `ErrRateLimited` or HTTP 404/403/429 | `NotFound`, `PermissionDenied`, `RateLimited`; exhausted product 401 maps to `Auth`.                                                                                             |
| `ErrProjectNotFound`, `ErrProjectAmbiguous`, `ErrMissingProjectID`   | `ProjectNotFound`, `ProjectAmbiguous`, `MissingProjectId`; distinct from a resource 404.                                                                                         |
| `ErrInvalidConfig`, `ErrInvalidInput`                                | `InvalidConfig`, `InvalidInput`.                                                                                                                                                 |
| `ErrNoCredentials`, `ErrCredentialsFile`                             | `NoCredentials`, `CredentialsFile` with `InvalidConfig` cause. A missing winner in `load` differs from a malformed options-only winner; keep the config design's distinctions.   |
| `ErrHistoryIncomplete`, `ErrPriceAboveMax`, `ErrUnpriced`            | `HistoryIncomplete`, `PriceAboveMax`, `Unpriced`. A price limit or unpriced quote does not dispatch the write.                                                                   |
| `context.Canceled`, `context.DeadlineExceeded`                       | `Cancelled`, `DeadlineExceeded`. A per-attempt timeout is not the caller's deadline.                                                                                             |

`IsNotFound`, `IsPermissionDenied`, `IsRateLimited`, `IsProjectNotFound`, and
`IsProjectAmbiguous` become class checks. A class records its origin: a typed
identity the SDK attached (the Go sentinel's equivalent) or one derived only
from the status of the first `ApiError`, as Go derives it. `has_identity`
matches the first kind anywhere in the causes; `api_error()` returns the first
API detail. The CLI uses the two separately, so a status-only 500 followed by a
status-only 404 is not a not-found result. `IsRetryable` and `ErrorCode` use the
API detail accessor, not string matching. Match the outer class first for
workflow outcomes; checking a nested 404 alone does not classify an unconfirmed
write as not found. `detail::<service::ErrorDetail>()` returns typed local
reasons without adding a core variant for each service sentinel. Only SDK code
attaches safe details. Map each service's Go sentinels when porting that
service.

For example `volume.create_volume` may return an identified volume on
`NotSettled`; keep it with `MayHaveBeenSent`. Preserve synthetic quota outcome
objects on refusal even if the state is `NotSent`. The CLI maps the classes,
operation-specific reasons, and safe diagnostics to the baseline's precedence
and `{"error":{...}}` document. It owns when a partial is printed on stdout,
when it bypasses a query, and which call or render error wins. SDK error text
alone is not that CLI contract.

## Outputs and projection

One public output type per operation is the caller's typed result and a safe
`Serialize` projection. If the API shape matches, the type may also implement
`Deserialize` with independent wire names. Otherwise decode into a private wire
DTO and validate before constructing the result. Never infer wire encoding from
public `Serialize`: use private request DTOs for request bodies. Decoder errors
cannot quote raw input, even when decoding partially succeeds.

The [CLI output design](cli-output.md#projection-and-bytes) owns the exact
projection rules. Output types implement them before the CLI sees any value. For
ordinary collections a Go slice uses `Vec<T>`: nil and empty both project as
`[]`. A Go pointer to a slice uses `Option<Vec<T>>`: `None` projects as `null`,
and `Some(Vec::new())` as `[]`. Use a separate wire DTO when wire presence or
nullability matters. Golden tests cover the SDK value stream, including outputs
without `Items`; the CLI owns encoding and rendering.

`Secret` owns zeroizing storage. Its `Debug` and `Serialize` give the redacted
marker. It has no `Display`. `expose_secret` is intentional and must not appear
in formatting or generic serialization. `Config`, credential options, raw wire
DTOs, and S3 streams never derive printable forms that expose secret or
account-bearing internals. Do not promise that callers cannot copy an explicitly
exposed secret or that all transient copies can be erased.

Use `Secret` for known credential fields including one-time output secrets.
Provide explicit accessors for the baseline's secret-file workflows; never
serialize the file contents or include them in errors. Before serialization
apply the [projection policy](cli-output.md) for reads, writes, and partial
outputs alike: keep resource values as the baseline prints them, apply the
baseline's dynamic-map rule, and replace a value that contains a credential sent
during the same logical request, which wins over passthrough, as the
[projection policy](cli-output.md#redaction) states. Neither the projection nor
CLI query ever sees unredacted secrets. This applies equally to a partial
result. SDK callers can distinguish safe projection from deliberately inspecting
a typed field.

## Differences from the baseline

- A hexadecimal billing balance may round differently from Go in the last bit.
- Use async Rust clients, owned options and inputs, typed cancellation and
  failure with partial output and dispatch state. There is no Go-style nil
  input, zero `Config`, blocking API, pointer helper, or context value map.
- Load settings and credentials in one management call. Settings-only lookup
  cannot build a management `Config`. Configuration and cache location changes
  are in the [configuration design](config.md), including who checks an explicit
  cache directory when the SDK has no home.
- Remove `WithHTTPClient`, `WithTransport`, and `IAMUserAuth.HTTPClient`. Expose
  a product connect timeout, not a raw sender. Endpoint overrides require
  verified HTTPS, canonical dotted-decimal IPv4, and a base path with no dot
  segments, no malformed percent-escapes, and only bytes valid in a URI path; an
  override such as `/gateway/./` that Go accepted is refused. A dashboard
  override affects IAM-user sign-in only and never moves billing. Drop the `GLB`
  and `VCR` aliases and ignore ambient proxies and root-store settings.
- Capture only product response metadata, not URLs or bodies. Log static route
  templates rather than expanded paths. Withhold unapproved peer diagnostics.
- Refuse redirects on IAM sign-in page fetches, writes and POST reads, even at
  the same origin. Only declared GET/HEAD reads can follow same-origin
  redirects, within the [request policy](sdk-requests.md#product-api).
- Use private HTTP/1.1 clients. Pool only repeatable product reads. Keep
  credential steps, `Once` writes and S3 unpooled and disable automatic replay
  everywhere. The HTTP/1 buffer gives an approximate head limit, not an exact
  byte cutoff. Decoded body bounds remain exact.
- S3 uses one DNS/TCP/TLS deadline in addition to the TCP dial limit. For an
  upload, a stalled response waits until the operation deadline rather than a
  separate 30-second response-header timer. Bodyless requests have a 30-second
  response-header limit including connection setup. See the
  [request policy](sdk-requests.md#signed-s3).
- Use an injected time for `Token::needs_refresh` and async invalidation for
  providers. `Secret` has no `Display`; debug and serialization redact it. S3
  uses async upload sources and an owned bounded download body.

## Layout and ownership

Paths below are relative to `crates/greennode-sdk/`. The integration owner alone
edits shared registrations, the workspace manifests and lockfile, and gates.

| Path                                                                  | Owner and contract                                                                                                         |
| --------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml`, `README.md`, `src/lib.rs`                               | Core integration: package metadata, top-level docs, public re-exports and service registration.                            |
| `src/config/`, `src/context.rs`, `src/secret.rs`, `src/error.rs`      | Core: local resolution and options; cancellation and runtime seams; secret and error types.                                |
| `src/session/`, `src/cache/`, `src/discovery/`                        | Core: credential sources and gates; safe cache files; project selection and common discovery.                              |
| `src/wire/`, `src/endpoint/`                                          | Core: socket/TLS and policy-specific senders; trusted base endpoints, route assembly and origin checks.                    |
| `src/model/`                                                          | Core: public re-exports' shared definitions, paging, exact wire values and projection helpers.                             |
| `src/<service>/mod.rs`, `src/<service>/*.rs`                          | That service: client, exports, models, operations, local error mapping. Split by concern; one writer per overlapping path. |
| `src/storage/object/`                                                 | Storage: object methods, XML validation and public stream types. The S3 sender and signer remain in core `wire/s3/`.       |
| `tests/core_*.rs`, `tests/support/`, `tests/fixtures/core/`           | Core: shared harness and tests for config, session, wire and error invariants.                                             |
| `tests/<service>.rs`, `tests/<service>/`, `tests/fixtures/<service>/` | That service: loopback operations, sanitized API fixtures and projection goldens.                                          |

Register a service once in `src/lib.rs`; map a previously unknown product base
in `src/endpoint/` if needed. Register no operation in core. Before each wave
the integration owner registers services against their first real client files
on the shared base. No released method or CLI command is a stub. Adding an
operation changes only that service's paths after registration.

Dependency order: workspace and public context, options, error, model and policy
shapes first. Then the core owners can build configuration, socket/TLS and
policies, and shared session/cache as disjoint slices against those interfaces.
Core owns the first billing read through both crates to prove the interfaces.
Land shared model and quote/discovery prerequisites before releasing those
services to parallel work. Split large domains such as network by disjoint
concerns; their service owner integrates `mod.rs` and fixtures. A new common
contract goes through the integration owner before dependent ports.

## Public docs and checks

Give crate and public modules `//!` summaries. Use `///` on public items for
what types cannot express. An operation states what it changes, when it waits or
repeats, important limits, and when failure returns partial or uncertain output.
An input field states accepted units and ranges, default, and what omission,
null, empty, or zero mean when distinct. Do not copy the identifier into a
sentence or put design decisions in private code comments.

Port sanitized fixture and loopback tests with each operation. Include exact
method, path, query and request body, bounds, decode and status errors,
projection order and exact numbers, and write outcome on cancellation. Core adds
policy conformance, shared refresh/discovery contention, cache identity and
invalidation, two-process cache locking, fake time and entropy, and debug and
capture canaries. [Request policies](sdk-requests.md#policy-tests) specifies the
loopback and secret tests. No test calls GreenNode or uses the real home.

[go-sdk]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/design/sdk-and-cli.md
[go-auth]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/design/service-account-auth.md
[go-waits]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/design/pricing-discovery.md#shared-waits-and-cancellation
[go-writes]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/adr/0002-write-api-conventions.md
