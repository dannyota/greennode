# SDK request policies

The SDK has three senders: private credential exchange, product API, and signed
S3. They share a private HTTP implementation, not request types, auth state,
redirect handlers, or hooks. [SDK design](sdk.md) owns the public interface. The
baseline's [auth][go-auth], [write][go-write], [single-send][go-once], and
[object][go-object] contracts apply unchanged except as stated here.

## Approaches

- Put policy switches on one request builder. This saves a few types but would
  let any operation turn credentials, redirects, or capture on.
- Build three unrelated HTTP clients. This isolates policy but duplicates
  cancellation, bounds, and TLS setup.
- Use three typed senders over private `hyper-util` clients and a common TLS
  connector. Take this approach: policy stays isolated without owning DNS, TCP,
  or TLS assembly.

## Contract

| Rule                    | Private credential exchange                                                                                           | Product API                                                                                                                                                                                | Signed S3                                                                                       |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| Who builds a request    | Only the private login state machines. Switch on built-in auth kind before construction.                              | A service or common discovery supplies a product route, operation ID, expected statuses, limits, and request policy.                                                                       | Core's S3 request builder accepts typed object operations from `storage::ObjectClient`.         |
| Destination             | Fixed service-account token URL. IAM-user sign-in and token origins come from trusted configuration, never discovery. | One validated product base per route. Public reads and CDN-key routes are distinct from bearer routes.                                                                                     | Fixed HTTPS endpoint for the explicit S3 region; no overrides or discovery.                     |
| Caller HTTP and capture | No raw client, hooks, capture, or product options. The caller may supply a TOTP provider, never a sender.             | No raw client or raw transport hook. Safe connect timeout, user agent, logger, and opt-in capture of response metadata.                                                                    | No management HTTP client, options, logger, hooks, or capture.                                  |
| Redirects               | None at HTTP level, even same-origin. IAM-user protocol transitions are checked separately.                           | Only declared GET/HEAD reads; resolve and require the same scheme, host, and effective port. Refuse for writes, `Once`, signed or opaque requests, and operations with a no-redirect rule. | None, even same-origin. Do not re-sign a peer-supplied destination.                             |
| Retry                   | One send per protocol step, including exchange. No retries on errors, statuses, or redirects.                         | Baseline per-operation retry and 401 rules; override with `Once`. No transport-layer replay.                                                                                               | One send, including on 401, 429 or failed dial. Never invalidate or refresh a management token. |
| Bounds                  | Service account 64 KiB decoded; IAM-user page and token response 1 MiB decoded. Bound headers separately.             | Default 8 MiB decoded, with the baseline's operation-specific limits and precedence. Bound headers separately.                                                                             | Keep operation-specific streaming, response byte, XML and metadata limits. No decompression.    |
| Debug                   | Login/exchange start and finish only.                                                                                 | One typed attempt event; no raw HTTP tracing.                                                                                                                                              | None.                                                                                           |

`wire/connection.rs` wraps `hyper_util::client::legacy::Client` and
`hyper_rustls::HttpsConnector`. Its raw request and connection types stay in
`wire`. No external or service caller can construct a raw request or choose a
sender by flag. `wire/credential/` receives only auth-specific input, a clock
and deadline, randomness, and a secret-free event sink. Credential request
constructors are private to that subtree. Neither product capture nor product
HTTP options are arguments to its constructor. `wire/product/` receives a
session token source and returns bounded responses to operation decoders;
services reach it through an operation client, not the socket layer. `wire/s3/`
accepts an S3 key pair, S3 region, timeout, and clock. It cannot accept
`Config`, `Session`, a token provider, a cache handle, or a product sender. Only
the storage object client calls it. Do not add outside imports of the HTTP stack
in SDK service modules.

Use verified TLS with WebPKI roots and SNI. No ambient proxy, cookies, or
root-store settings. The only cookie jar is per IAM-user login. Endpoint
validation rejects userinfo, fragments, and non-HTTPS URLs. Unit tests can
inject fixed loopback origins and a test root below the public interface,
including for the otherwise fixed IAM exchange and S3 hosts. Never accept a
plain HTTP or insecure TLS override in production. A public product endpoint
override is a deliberate trust decision: its origin may see the configured
product credential for that route. Provider errors do not change destinations.

### Private HTTP implementation

Build each `HttpsConnector` with WebPKI roots, an explicit rustls `ring`
provider, no client certificate and `ClientConfig.enable_early_data = false`.
Use `with_tls_config`, `https_only`, `enable_http1`, and `wrap_connector` over
`HttpConnector::enforce_http(false)`. The inner connector accepts an HTTPS URI;
the outer connector still refuses plain HTTP. Do not enable HTTP/2, upgrade,
CONNECT, proxy settings, or TLS 0-RTT. The [connector source][tls-builder]
confirms the HTTP/1-only path does not advertise HTTP/2 in ALPN.

Set `Client::builder(...).retry_canceled_requests(false)` on every client. The
[client source][legacy-client] shows this disables both the send retry on a
closed reused connection and the retry loop after a canceled pool checkout. The
Hyper client does not follow redirects; only product code can issue a new
request after validating a redirect. Pool only repeatable product reads. Scope
the pool to the session. Set `pool_max_idle_per_host(2)`, a 30-second
`pool_idle_timeout`, and an injected `pool_timer`. All other attempts use
`pool_max_idle_per_host(0)`; the [pool source][legacy-pool] shows zero removes
the pool, not just idle retention. Credential steps, `Once` and S3 always use a
fresh HTTP/1.1 connection. Reuse only after Hyper marks the connection ready for
another request at a valid HTTP boundary. An idle connection driver may live
with the pool, but no request continues detached from its operation or body
owner.

Every request times out even with no caller deadline. Product and login requests
default to 120 seconds; `with_timeout` sets a positive replacement. Include the
bounded body read and never extend the caller's deadline. Apply a shorter
product connect timeout to the connector future, including DNS and TLS. Keep a
separate typed indication of a DNS/TCP dial failure from the inner connector.
Hyper's `is_connect()` can also cover a closed checkout or TLS failure; it alone
does not prove a failed dial for `NonIdempotent` retry. Track dispatch
separately. Before encoding, auth, or connection establishment finishes, a
failed attempt cannot have sent HTTP bytes. After handing the request to HTTP
framing, report a possible send unless there is positive evidence otherwise.
Drop the connection or body on cancellation.

Set `http1_max_buf_size` to 64 KiB for login and product and 32 KiB for S3. This
bounds header buffering, not the exact accepted head length. The
[parser][hyper-head-buffer] grows the buffer within a bounded allocation and can
accept a head over the cap if the last read completes it before the buffer limit
check. Do not add a raw HTTP reader to tighten the cutoff.

Reject oversized bodies with an overflow-byte probe and retain observed status
only where the operation contract requires it. Close or drop the connection on
early refusal. For login and product allow identity or gzip; apply the decoded
limit before JSON or other parsers, and limit encoded bytes too (decoded cap
plus 64 KiB). S3 accepts no automatic decoding or request URL normalization. Do
not use a JSON walker that rewrites exact integers to floats. The cap bounds the
payload the SDK retains, not wire bytes: hyper reads into spare buffer capacity
that can grow beyond `http1_max_buf_size`, handles chunk framing separately, and
may drain a short body when a response is dropped. Refuse before reading when
hyper's validated body size exceeds the cap; when `Transfer-Encoding` is present
hyper ignores `Content-Length`. HEAD, 204 and 304 responses are bodyless
whatever length they declare. A refused connection is poisoned so the pool never
checks it out again; one already idle stays open until eviction, and hyper may
return a connection at a clean message boundary to the pool before the refusal
runs.

`hyper-util`'s `client-legacy` always emits `tracing` events with destinations
and socket addresses, and logs a whole URI on requests without scheme or
authority and the path and query of `CONNECT`; the shared send step refuses
those requests first. hyper's `tracing` and rustls's `logging` features are off;
an embedding build that enables them can see header names, sizes and formatted
errors. The SDK puts no credential in a URI and the CLI installs no subscriber.
An embedding application should filter the `hyper`, `hyper_util`, `hyper_rustls`
and `rustls` targets.

### Private exchange

Use the baseline's [fixed request and response contract][go-token] for service
accounts unchanged. There is no token endpoint option, alternate encoding, scope
negotiation, cookie jar, capture, or fallback identity. Token responses pass a
strict schema check: duplicate keys and fold aliases of known fields are refused
at every level, and nesting stops at 128 containers. If the fixed endpoint
redirects, fail with a fixed typed reason, not its destination. The cache sees
only a successful validated token.

For IAM users, maintain the baseline's PKCE, password, TOTP and token steps. Use
the injected CSPRNG for PKCE. Pass the secret only to the configured sign-in
origin; retain only its cookies for the life of the flow, respecting path,
secure and expiry attributes with the injected clock. The dashboard is an auth
parameter, not a request destination. Accept a 2FA location only if it matches
the sign-in origin and the recognized 2FA route. An expected callback resolves
to exactly the authorization request's redirect URI (scheme, host, effective
port and path) with one non-empty `code`; unlike Go, any other location fails.
Read an expected callback's one-time code as data without visiting its URL. Send
that code and verifier only to the preselected token URL. Unlike Go's login page
GET handler, no page fetch follows a redirect. Treat a moved sign-in page as an
auth failure. Do not surface HTML, cookies, parameters or upstream error
objects.

### Product API

Every operation supplies a fixed ID, a route made of encoded segments, a
read/write classification, and one of `Repeatable`, `NonIdempotent`, or `Once`.
Default to `Once` for writes until their baseline policy is proven. Do not infer
write safety from an HTTP method: a POST quote is a repeatable read, and a
toggle or paid order may be `Once` even if it uses PUT or DELETE. `Once` permits
one attempt, no redirect, and no resend. On 401 invalidate the exact bearer
sent, subject to the baseline age rule, without fetching another.

Keep the baseline's normal retry budget (three retries, one-second initial
interval), backoff, and 30-second cap on delays including `Retry-After`. Its
dates follow Go's HTTP-date layouts. Every zone abbreviation keeps the instant
as written, as Go does for `GMT±n` and unknown names; Go shifts a name the
machine's local zone recognizes. `Repeatable` can retry a transient network
error or 429, 502, 503, or 504. `NonIdempotent` can retry only a proven failed
dial or 429. Preserve the baseline's separate single 401 recovery when the
operation allows it; neither key-based nor unauthenticated requests try
management credentials on 401. `with_retry` may change the budget and interval
but not the eligible cases. A failed send can enter the network retry path. A
body read, decompression, or close failure after response headers does not
retry, even for `Repeatable` and even if the status would qualify. A 401 that
remains visible after body-error mapping may invalidate the exact bearer, but
cannot trigger a resend. Preserve observed status, decode strictness, empty-body
handling, and overflow classification per operation. A retry hint is not a
guarantee that rerunning a multi-step operation is safe.

Product redirects also reject userinfo, fragments, cycles and a chain longer
than nine hops. Check before attaching credentials to a newly resolved URI.
Unlike Go, refuse every write redirect and every POST read redirect, even at the
same origin. This includes the VPC rename and monitor log-project quote. Discard
`Location` in public errors. A refused redirect reports its 3xx status only for
operations whose Go request sets `Once` or `NoRedirect` and returns the redirect
itself; elsewhere it reports no status, as Go does for the cross-host redirects
it refuses. Routes own trusted templates; validate caller path IDs before
escaping and never interpolate them in log events.

Decoders may receive bounded response bytes and typed status internally. Accept
peer error codes and message fields only through the operation's safe
classifier. On credential-bearing or secret-bearing paths withhold fields that
are not known safe; use status fallback and fixed diagnostics. Even a field that
looks like an identifier can be a reflected credential. When any credential was
sent, keep only `code` and `message`, and withhold a message with non-ASCII
text, a credential field marker or a sent credential in any encoding; this is
stricter than Go, which case-folds Unicode. "Any encoding" means the literal,
percent, JSON-escape, Base64, Base64URL and hex forms, whole or embedded. An
embedded fragment of seven or more stable characters matches at any alignment; a
shorter encoded form matches only between token boundaries, so a six-byte
one-time code still matches and short noise does not. Withhold any peer `code`
or `message` containing `/`, `\`, `@`, `www.` in any case, or a colon followed
by anything but whitespace or the end; this closed rule stands in for URL
detection. Never expose raw headers, response bodies, unbounded peer text, URLs,
or decode errors via an error source, capture, or debug event. Keep per-attempt
credentials available only within the logical request for reflection checks,
including across retries.

Success bodies are checked against the operation's output schema before
decoding: object or array shape at every node, duplicate keys, and keys that
fold to the same field. Unlike Go, which matches keys to fields
case-insensitively, decoding is case-sensitive, so on output reads a lone key
that differs only in case is ignored. Guard and safety reads, whose values
decide whether a write is sent or a refusal is reported, refuse a lone case
variant of a known key as ambiguous, because ignoring it could hide a value Go
would act on. Where Go itself refuses any lone alias, as the network NAT
family's field-name check does, refuse it too.

### Signed S3

`ObjectConfig` is independent of `Config`: explicit `ObjectCredentials` (two
`Secret` values), `HCM04` or `HAN02`, operation timeout, optional injected
clock. Keep the five-minute default, one-hour timeout ceiling and fixed regional
path-style origin. Keep the baseline's SHA-256 payload hash, SigV4 header,
escaped path, and canonical query rules. Build a raw `http::Uri` with the exact
signed `PathAndQuery`. The [Hyper client][legacy-client] copies that value
verbatim when it emits the HTTP/1 origin form. Do not pass it through `Url`
parsing or normalize dot segments, slashes, or escapes. Send the S3 signature
and key only to the selected region's origin.

Keep the operation deadline. Set `HttpConnector::set_connect_timeout` to 30
seconds for TCP dialing. Put a 40-second deadline around the entire connector
future, including DNS, TCP and TLS. The [TLS connector][tls-connect] has no
separate handshake timer. For a request with no streamed body, bound the entire
`Client::request` future to 30 seconds through response headers. That includes
any connection setup. For a streamed upload, the operation deadline bounds both
the upload and the response wait; there is no separate 30-second post-flush
timer. All timers respect earlier caller deadlines.

`GetObjectOutput` carries metadata and an owned `ObjectBody`. Its async chunk
reader retains the deadline and cancellation signal and enforces both the
operation's byte limit and declared length; dropping it closes the response.
`PutObjectInput` borrows a `Send + Unpin` source implementing Tokio `AsyncRead`
and `AsyncSeek`, validates and hashes the declared bytes, rewinds, then streams
one length-limited upload without taking source ownership. Drive an owned HTTP
body through a bounded channel while the upload future owns the borrow; stop the
reader and connection together on failure or cancellation. Keep the baseline's
5,000,000,000-byte single PUT and default GET limits, header bounds, and bounded
XML rules for list and copy. Never serialize, debug-print, or capture an object
body. Multipart and presigned URLs are not added.

## Event allowlist

| Source                   | Event and fields                                                                                                                                            |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Product transport        | `ProductAttempt`: static operation ID, method, optional static route template, optional observed status, elapsed duration. No expanded path or resource ID. |
| Product response capture | `ResponseCapture`: static operation ID, method and observed status on a complete, bounded, non-sensitive response. No URL, raw headers, or body.            |
| IAM user                 | `LoginStarted`; `LoginFinished` with `ok` boolean.                                                                                                          |
| Service account          | `ExchangeStarted`; `ExchangeFinished` with `ok` boolean.                                                                                                    |
| Signed S3                | None.                                                                                                                                                       |

No other data enters SDK events: no header, body, query, origin, credentials,
cache key, or error text. Configuration provenance and CLI write lifecycle
events are separate allowlists in their respective designs. Do not enable hyper
connection verbosity, request dumps, or TLS key logging.

## Dependencies

Use only needed features. The table gives the latest stable releases checked on
2026-11-25 against crates.io, with release history and first-party repository
links in registry metadata. The HTTP/TLS, time, parsing and crypto families have
2026 releases. `httpdate` is an older, dependency-free leaf with no newer
release. `dual` means MIT OR Apache-2.0. Rerun advisory, license, age, and
feature-graph checks when adding each crate.

| Crate and version checked                                       | License                                                                 | Use and dependency cost                                                                                                                                                                                                                                                                                    |
| --------------------------------------------------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [tokio] 1.54.1                                                  | MIT                                                                     | `rt`, `net`, `time`, `sync`, `macros`, `io-util`; mio/socket2 and macros. No multithread runtime requirement for the SDK.                                                                                                                                                                                  |
| [hyper] 1.13.0, [hyper-util] 0.1.21                             | MIT                                                                     | Low-level HTTP/1 framing plus `client-legacy`, `http1`, and `tokio` in hyper-util. The latter enables the private client, pool, and TCP connector; no `http2` or proxy features. Server features are dev-only.                                                                                             |
| [hyper-rustls] 0.27.10, [rustls] 0.23.46, [webpki-roots] 1.0.12 | Apache-2.0 OR ISC OR MIT; Apache-2.0 OR ISC OR MIT; CDLA-Permissive-2.0 | `hyper-rustls` released 2026-09-20 from the rustls project. Disable defaults; enable `http1`, `ring`, and `tls12`. No native roots, platform verifier, logging, or AWS-LC. Adds `tokio-rustls` and `tower-service`; ring needs C/assembly build tooling. Enable only `std`, `ring`, and `tls12` in rustls. |
| [tower-service] 0.3.3                                           | MIT                                                                     | Dependency-free connector interface already used by both HTTP crates; use it to apply deadlines to connector futures.                                                                                                                                                                                      |
| [http] 1.5.0, [http-body-util] 0.1.5, [bytes] 1.12.2            | dual; MIT; MIT                                                          | Share Hyper's HTTP value and body types; no additional transport.                                                                                                                                                                                                                                          |
| [serde] 1.0.229, [serde_json] 1.0.154                           | dual                                                                    | Derive adds proc macros. Enable `raw_value` and `arbitrary_precision` for bounded exact payloads; no insertion-ordered map dependency in the SDK.                                                                                                                                                          |
| [time] 0.3.55, [httpdate] 1.0.3                                 | dual                                                                    | `formatting` and `parsing` for fixed time formats; no local time feature. Small time helpers; `httpdate` is a stable dependency-free HTTP date parser.                                                                                                                                                     |
| [hmac] 0.13.0, [sha1] 0.11.0, [sha2] 0.11.1                     | dual                                                                    | Shared RustCrypto digest family. HMAC-SHA1 only for TOTP; SHA-256 for PKCE, cache keys and SigV4.                                                                                                                                                                                                          |
| [getrandom] 0.4.3, [data-encoding] 2.11.1, [zeroize] 1.9.2      | dual; MIT; dual                                                         | OS entropy, no RNG framework. Leaf Base32/Base64/hex and zeroizing owned secrets.                                                                                                                                                                                                                          |
| [url] 2.5.9, [cookie] 0.18.3, [regex-lite] 0.1.9                | dual                                                                    | Parse origin and redirect/form data; URL adds IDNA/ICU. Parse login cookies only, no signed/private cookie features. Leaf matcher for bounded sign-in fields.                                                                                                                                              |
| [flate2] 1.1.11, [quick-xml] 0.42.0                             | dual; MIT                                                               | Rust `miniz_oxide` gzip backend. XML reader with no encoding or serde features; reject DTD/entities outside the baseline rules.                                                                                                                                                                            |
| [rustix] 1.1.6                                                  | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT                     | Unix `fs` flags for no-follow, exclusive creation and nonblocking open; select the allowlisted license branch. Use `std::fs::File::try_lock` for cancelable lock polling without a lock crate.                                                                                                             |
| [rcgen] 0.14.10, [tempfile] 3.27.0                              | dual                                                                    | Dev only: test roots with `ring` and isolated dirs, no PEM or X.509 parser features.                                                                                                                                                                                                                       |

The pool adds connector and concurrency helpers. URL's ICU and TLS remain the
largest dependency groups. Enable only the features each slice needs. Add S3 XML
and signing dependencies only when used. Do not add reqwest, an AWS SDK, OAuth
or TOTP clients, native TLS, ambient tracing, argument or JMESPath parsers,
shell, or terminal libraries. Offer no blocking SDK and do not create an
internal runtime. Callers run async methods on Tokio. Run bounded filesystem
work on blocking workers; lock attempts never block a Tokio worker or hold up a
canceled waiter.

## Policy tests

`tests/support/` runs a scripted loopback HTTP/TLS peer with a per-test root,
request counts, explicit barriers, and arbitrary malformed/oversized responses.
Return an owned test handle and join the peer on cleanup. Compare the raw HTTP/1
request target byte for byte with the signed path and query, using SigV4 vectors
with `/../`, `/./`, repeated slashes, and escaped keys. Count HTTP sends and
accepted connections for each policy. Close a pooled read's idle connection just
as a new request checks it out; with SDK retry disabled, the HTTP client must
not resend. Test that credential steps, `Once` and S3 never reuse a connection
or retry 401, 429, or transport failures. For ordinary product requests, verify
the separate 401 recovery, 429 and transient status retry rules, and no retry
for a truncated body even on retryable status. Test redirect send counts for
same-origin GET, POST quote and writes, other origins, changed schemes and
ports. Test cancellation, fragmented cap+1 response heads, and clearly oversized
heads. Assert refusal at or shortly above the head cap and bounded buffering,
not an exact accepted byte count. Check exact decoded body limits with cap+1
probes and compressed input. Test the S3 connector, bodyless request, and
overall deadlines separately. A streamed upload with a stalled response fails at
the operation deadline, not 30 seconds after the last byte. Test S3 metadata and
streaming errors without constructing a management config.

Inject a distinct toy canary for each password, TOTP seed and code, client
secret, login cookie, auth code, bearer (including rejected and new tokens), CDN
key, and S3 key/signature. Echo them in headers, redirect destinations, JSON/XML
bodies, errors, URL parameters and reflected fields. Assert absence in SDK debug
output, full errors including causes, capture and projection, and in CLI
stderr/stdout after projection, query, and rendering. Do not print canary values
when assertions fail. Run the product hook while forcing both built-in login
paths and prove the hook only sees product response metadata. Use a fake clock
and injectable entropy for contention, retries and expiry; never use real
account data or egress outside loopback.

[go-auth]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/design/service-account-auth.md
[go-token]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/design/service-account-auth-api.md#token-exchange
[go-write]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/adr/0002-write-api-conventions.md
[go-once]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/adr/0003-toggle-writes.md
[go-object]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/design/storage-objects.md
[tokio]: https://crates.io/crates/tokio
[hyper]: https://crates.io/crates/hyper
[hyper-util]: https://crates.io/crates/hyper-util
[hyper-rustls]: https://crates.io/crates/hyper-rustls
[tower-service]: https://crates.io/crates/tower-service
[legacy-client]:
  https://docs.rs/hyper-util/0.1.21/src/hyper_util/client/legacy/client.rs.html
[legacy-pool]:
  https://docs.rs/hyper-util/0.1.21/src/hyper_util/client/legacy/pool.rs.html
[tls-builder]:
  https://docs.rs/hyper-rustls/0.27.10/src/hyper_rustls/connector/builder.rs.html
[tls-connect]:
  https://docs.rs/hyper-rustls/0.27.10/src/hyper_rustls/connector.rs.html
[hyper-head-buffer]:
  https://docs.rs/hyper/1.13.0/src/hyper/proto/h1/io.rs.html#174-244
[rustls]: https://crates.io/crates/rustls
[webpki-roots]: https://crates.io/crates/webpki-roots
[http]: https://crates.io/crates/http
[http-body-util]: https://crates.io/crates/http-body-util
[bytes]: https://crates.io/crates/bytes
[serde]: https://crates.io/crates/serde
[serde_json]: https://crates.io/crates/serde_json
[time]: https://crates.io/crates/time
[httpdate]: https://crates.io/crates/httpdate
[hmac]: https://crates.io/crates/hmac
[sha1]: https://crates.io/crates/sha1
[sha2]: https://crates.io/crates/sha2
[getrandom]: https://crates.io/crates/getrandom
[data-encoding]: https://crates.io/crates/data-encoding
[zeroize]: https://crates.io/crates/zeroize
[url]: https://crates.io/crates/url
[cookie]: https://crates.io/crates/cookie
[regex-lite]: https://crates.io/crates/regex-lite
[flate2]: https://crates.io/crates/flate2
[quick-xml]: https://crates.io/crates/quick-xml
[rustix]: https://crates.io/crates/rustix
[rcgen]: https://crates.io/crates/rcgen
[tempfile]: https://crates.io/crates/tempfile
