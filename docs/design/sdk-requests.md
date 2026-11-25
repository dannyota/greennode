# SDK request policies

Status: proposed.

The SDK has three senders: private credential exchange, product API, and signed
S3. They share socket and TLS code, not request types, auth state, redirect
handlers, or hooks. [SDK design](sdk.md) owns the public interface. The
baseline's [auth][go-auth], [write][go-write], [single-send][go-once], and
[object][go-object] contracts apply unchanged except as stated here.

## Approaches

- Put policy switches on one request builder. This saves a few types but would
  let any operation turn credentials, redirects, or capture on.
- Build three unrelated HTTP clients. This isolates policy but duplicates
  cancellation, bounds, and TLS setup.
- Use three typed senders over a private socket layer. Take this approach: each
  sender can construct only its own kind of request.

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

`wire/connection.rs` owns DNS, TCP, TLS, HTTP framing and cancellable body
reads. Its request/connection types are not exported from `wire`. No external or
service caller can construct a raw request or choose a sender by flag.
`wire/credential/` receives only auth-specific input, a clock and deadline,
randomness, and a secret-free event sink. Credential request constructors are
private to that subtree. Neither product capture nor product HTTP options are
arguments to its constructor. `wire/product/` receives a session token source
and returns bounded responses to operation decoders; services reach it through
an operation client, not the socket layer. `wire/s3/` accepts an S3 key pair, S3
region, timeout, and clock. It cannot accept `Config`, `Session`, a token
provider, a cache handle, or a product sender. Only the storage object client
calls it. Do not add outside imports of the HTTP stack in SDK service modules.

Use verified TLS with WebPKI roots and SNI. No ambient proxy, cookies, or
root-store settings. The only cookie jar is per IAM-user login. Endpoint
validation rejects userinfo, fragments, and non-HTTPS URLs. Unit tests can
inject fixed loopback origins and a test root below the public interface,
including for the otherwise fixed IAM exchange and S3 hosts. Never accept a
plain HTTP or insecure TLS override in production. A public product endpoint
override is a deliberate trust decision: its origin may see the configured
product credential for that route. Provider errors do not change destinations.

Every request times out even with no caller deadline. The product and login
request timeout defaults to 120 seconds and `with_timeout` sets a positive
replacement. It includes the bounded body read, and may not extend the caller's
deadline. The product connect timeout only narrows this limit. Use one fresh
HTTP/1.1 connection per attempt in the first implementation. The transport must
not reuse a connection, renegotiate to HTTP/2, or replay an attempt in any
class. Track the pre-send boundary: DNS, TCP, TLS, local encoding and auth
acquisition can fail before a write. Once handing the request to HTTP framing,
classify a failed attempt as possibly sent unless there is positive evidence
otherwise; cancel the connection on drop.

Cap response heads at 64 KiB for login and product and at the baseline's 32 KiB
for S3. Reject oversized bodies with an overflow-byte probe and retain observed
status only where the operation contract requires it. Close or drop the
connection on early refusal. For login and product allow identity or gzip; apply
the decoded limit before JSON or other parsers, and limit encoded bytes too
(decoded cap plus 64 KiB). S3 accepts no automatic decoding or request URL
normalization. Do not use a JSON walker that rewrites exact integers to floats.

### Private exchange

Use the baseline's [fixed request and response contract][go-token] for service
accounts unchanged. There is no token endpoint option, alternate encoding, scope
negotiation, cookie jar, capture, or fallback identity. If the fixed endpoint
redirects, fail with a fixed typed reason, not its destination. The cache sees
only a successful validated token.

For IAM users, maintain the baseline's PKCE, password, TOTP and token steps. Use
the injected CSPRNG for PKCE. Pass the secret only to the configured sign-in
origin; retain only its cookies for the life of the flow, respecting path,
secure and expiry attributes with the injected clock. The dashboard is an auth
parameter, not a request destination. Accept a 2FA location only if it matches
the sign-in origin and the recognized 2FA route. Read an expected callback's
one-time code as data without visiting its URL. Send that code and verifier only
to the preselected token URL. Unlike Go's login page GET handler, no page fetch
follows a redirect. Treat a moved sign-in page as an auth failure. Do not
surface HTML, cookies, parameters or upstream error objects.

### Product API

Every operation supplies a fixed ID, a route made of encoded segments, a
read/write classification, and one of `Repeatable`, `NonIdempotent`, or `Once`.
Default to `Once` for writes until their baseline policy is proven. Do not infer
write safety from an HTTP method: a POST quote is a repeatable read, and a
toggle or paid order may be `Once` even if it uses PUT or DELETE. `Once` permits
one attempt, no redirect, and no resend. On 401 invalidate the exact bearer
sent, subject to the baseline age rule, without fetching another.

Keep the baseline's normal retry budget (three retries, one-second initial
interval), backoff, and 30-second cap on delays including `Retry-After`.
`Repeatable` can retry a transient network error or 429, 502, 503, or 504.
`NonIdempotent` can retry only a proven failed dial or 429. Preserve the
baseline's separate single 401 recovery when the operation allows it; neither
key-based nor unauthenticated requests try management credentials on 401.
`with_retry` may change the budget and interval but not the eligible cases.
Preserve status, decode strictness, empty-body handling, and body-overflow
classification per operation. A retry hint is not a guarantee that rerunning a
multi-step operation is safe.

Product redirects also reject userinfo, fragments, cycles and a redirect chain
longer than nine hops. Check before attaching credentials to a newly resolved
URI. No write follows a redirect, even at the same origin. Discard `Location`
when building public errors. Routes own trusted templates; validate caller path
IDs before escaping and never interpolate them in a log event.

Decoders may receive bounded response bytes and typed status internally. Accept
peer error codes and message fields only through the operation's safe
classifier. On credential-bearing or secret-bearing paths withhold fields that
are not known safe; use status fallback and fixed diagnostics. Even a field that
looks like an identifier can be a reflected credential. Never expose raw
headers, response bodies, unbounded peer text, URLs, or decode errors via an
error source, capture, or debug event. Keep per-attempt credentials available
only within the logical request for reflection checks, including across retries.

### Signed S3

`ObjectConfig` is independent of `Config`: explicit `ObjectCredentials` (two
`Secret` values), `HCM04` or `HAN02`, operation timeout, optional injected
clock. Keep the five-minute default, one-hour timeout ceiling and fixed regional
path-style origin. Preserve the 30-second dial, 10-second TLS and 30-second
response header timeouts, all within the operation deadline. Keep the baseline's
SHA-256 payload hash, SigV4 header, escaped path, and canonical query rules
unchanged. Retain the exact signed bytes in the transmitted request target. In
particular, do not pass object paths through a URL parser that removes dot
segments, changes escaping, or collapses slashes. Serialize the S3 signature and
key only for that origin.

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

| Crate and version checked                                      | License                                             | Use and dependency cost                                                                                                                                                                        |
| -------------------------------------------------------------- | --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [tokio] 1.54.1                                                 | MIT                                                 | `rt`, `net`, `time`, `sync`, `macros`, `io-util`; mio/socket2 and macros. No multithread runtime requirement for the SDK.                                                                      |
| [hyper] 1.13.0, [hyper-util] 0.1.21                            | MIT                                                 | Low-level client HTTP/1 and Tokio I/O adapter only; httparse and small futures primitives, no legacy pooled client or h2. Test server only in dev features.                                    |
| [tokio-rustls] 0.26.6, [rustls] 0.23.46, [webpki-roots] 1.0.12 | dual; Apache-2.0 OR ISC OR MIT; CDLA-Permissive-2.0 | Explicit `ring`, `tls12` and `std` where supported, defaults off. Ring adds C/assembly build tooling; avoid AWS-LC, native cert loading and OpenSSL. TLS 1.2 and 1.3 with bundled roots.       |
| [http] 1.5.0, [http-body-util] 0.1.5, [bytes] 1.12.2           | dual; MIT; MIT                                      | Share Hyper's HTTP value and body types; no additional transport.                                                                                                                              |
| [serde] 1.0.229, [serde_json] 1.0.154                          | dual                                                | Derive adds proc macros. Enable `raw_value` and `arbitrary_precision` for bounded exact payloads; no insertion-ordered map dependency in the SDK.                                              |
| [time] 0.3.55, [httpdate] 1.0.3                                | dual                                                | `formatting` and `parsing` for fixed time formats; no local time feature. Small time helpers; `httpdate` is a stable dependency-free HTTP date parser.                                         |
| [hmac] 0.13.0, [sha1] 0.11.0, [sha2] 0.11.1                    | dual                                                | Shared RustCrypto digest family. HMAC-SHA1 only for TOTP; SHA-256 for PKCE, cache keys and SigV4.                                                                                              |
| [getrandom] 0.4.3, [data-encoding] 2.11.1, [zeroize] 1.9.2     | dual; MIT; dual                                     | OS entropy, no RNG framework. Leaf Base32/Base64/hex and zeroizing owned secrets.                                                                                                              |
| [url] 2.5.9, [cookie] 0.18.3, [regex-lite] 0.1.9               | dual                                                | Parse origin and redirect/form data; URL adds IDNA/ICU. Parse login cookies only, no signed/private cookie features. Leaf matcher for bounded sign-in fields.                                  |
| [flate2] 1.1.11, [quick-xml] 0.42.0                            | dual; MIT                                           | Rust `miniz_oxide` gzip backend. XML reader with no encoding or serde features; reject DTD/entities outside the baseline rules.                                                                |
| [rustix] 1.1.6                                                 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Unix `fs` flags for no-follow, exclusive creation and nonblocking open; select the allowlisted license branch. Use `std::fs::File::try_lock` for cancelable lock polling without a lock crate. |
| [rcgen] 0.14.10, [tempfile] 3.27.0                             | dual                                                | Dev only: test roots with `ring` and isolated dirs, no PEM or X.509 parser features.                                                                                                           |

A candidate feature graph resolves about 105 production packages, including
build dependencies. URL's ICU and TLS explain most of the weight; split new
features when they land instead of enabling default bundles. Add S3 XML and
signing dependencies only when used. Do not add reqwest, an AWS SDK, OAuth or
TOTP clients, native TLS, ambient tracing, argument or JMESPath parsers, shell,
or terminal libraries. Offer no blocking SDK and do not create an internal
runtime. Callers run async methods on Tokio. Run bounded filesystem work on
blocking workers; lock attempts never block a Tokio worker or hold up a canceled
waiter.

## Policy tests

`tests/support/` runs a scripted loopback HTTP/TLS peer with a per-test root,
request counts, explicit barriers, and arbitrary malformed/oversized responses.
Return an owned test handle and join the peer on cleanup. Test wire bytes and S3
signature vectors, including dot segments and escaped keys. Count sends on
same-origin and cross-origin redirects, scheme and port changes, fragmented
bodies, 401 recovery, `Once`, cancellation after a send, and signed writes.
Prove decoded bounds with a compressed body and cap+1 probes for each policy.
Test S3 metadata and streaming errors without constructing a management config.

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
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/design/service-account-auth.md
[go-token]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/design/service-account-auth-api.md#token-exchange
[go-write]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/adr/0002-write-api-conventions.md
[go-once]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/adr/0003-toggle-writes.md
[go-object]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/design/storage-objects.md
[tokio]: https://crates.io/crates/tokio
[hyper]: https://crates.io/crates/hyper
[hyper-util]: https://crates.io/crates/hyper-util
[tokio-rustls]: https://crates.io/crates/tokio-rustls
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
