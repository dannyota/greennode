# CLI output

Project SDK outputs into the baseline's safe JSON shape before a query or
renderer can see them. The CLI owns the output bytes. The SDK owns wire decoding
and the ordered, safe `Serialize` representation, not its exact JSON encoding.
The [baseline CLI][baseline] and its [list rules][lists] are unchanged except
where noted below.

## Approaches

| Approach                                                           | Trade-off                                                                                                                                |
| ------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Serialize the wire record or convert to a generic JSON value first | Simple, but exposes wire names and loses struct order or type distinctions. Generic defaults also differ in escaping and float notation. |
| Duplicate every output as a CLI record                             | Allows exact formatting but doubles the model and creates two places to fix redaction.                                                   |
| Safe SDK projection and CLI formatter (chosen)                     | Reuse typed field order and redaction. One formatter handles Go byte compatibility without requiring CLI dependencies in the SDK.        |

## Projection and bytes

Use the SDK `Serialize` stream, or an explicitly safe service-local projection
when the CLI adds fields or must withhold output. The formatter consumes fields
in the order the serializer emits them; on the plain JSON path do not round trip
through `serde_json::Value`, even with a preserving map. Keep these [baseline
encoder][encoder] rules:

- Use exact Go field names, not wire names or Rust identifiers. Emit exported
  fields in declaration order, including zero values. Flatten embedded records
  in place, even when their type is private. An absent embedded pointer emits no
  keys. An ordinary nested record retains its wrapper. Reject flatten-key
  collisions in projection tests rather than relying on a winner.
- Emit a slice with zero rows as `[]`, including a nil Go slice. A nil pointer
  to a slice is `null`, while a present pointer to an empty slice is `[]`. A nil
  map, nil ordinary pointer or absent dynamic value is `null`; an empty non-nil
  map is `{}`. Emit dynamic values by their actual type. Sort a map by the UTF-8
  bytes of its stringified keys, not by encoded key text. Map keys are not
  renamed. Because Serde flatten can serialize via a map, the SDK must emit
  sorted data maps itself and preserve record order at flatten sites. Use sorted
  map keys even for maps inside arrays and nested records.
- Emit signed and unsigned integers as exact decimal digits, never via floats.
  Float leaves use the shortest round-trip Go spelling at their source width:
  decimal except for nonzero magnitude below `1e-6` or at least `1e21`,
  lowercase `e`, sign on positive exponents, no leading zeros in exponents, and
  negative zero as `-0`. Reject nonfinite float leaves rather than converting
  them to null. Use RFC 3339 with Go fractional seconds and offset for time
  leaves (including the baseline zero time). Validate and compact non-nil raw
  JSON in place, retaining its key order and number text; a nil raw slice yields
  `[]`. Output types need explicit safe adapters for time, raw fragments and
  nonfinite checks when a plain `Serialize` implementation cannot represent the
  distinction.
- Use two spaces per depth, a colon and space after each object key, and one LF
  after the document. Empty arrays and objects have no interior whitespace. Use
  Go `encoding/json` string escapes in keys and values: escape quote, backslash,
  C0 (short escapes for tab, LF, CR, backspace and form feed), `<`, `>`, `&`,
  and U+2028/U+2029. Escape DEL, C1 and bidi controls as lowercase `\u` escapes
  too: U+061C, U+200E/U+200F, U+202A–U+202E and U+2066–U+2069. A wire codec
  handles invalid UTF-8 replacement where the baseline decoder does.

## Redaction

Reflection wins over passthrough. The SDK keeps the credentials it sent during
one logical request, including retries and sign-in, and before it builds an
output or a partial output it replaces any string value that contains one of
them with `<redacted>`, in reads and writes alike. Map keys get the same check:
an entry whose decoded key contains a sent credential is dropped, including keys
inside raw JSON fragments, so no key is renamed and no two keys collide. Any
other value in a typed resource field passes through as the baseline prints it,
even when it is some other credential. Tests plant a resource canary and an
own-credential canary in the same field and check JSON, query, table, text, and
the console log: the first prints, the second never does, including as a map key
in plain JSON, a table header, and a `keys(@)` query. No test loads a real
credential.

Never print greennode's own credentials or session material: profile password,
one-time-code secret, client secret, access token, authorization code, cookies,
own request `Authorization` header, vCDN API key, S3 signing keys, and issued
one-time secrets the baseline writes to a secret file. Keep the baseline's
explicit typed redaction and withholding, including monitor channel reads and
writes. Run it before queries and every renderer, also on partial output. Raw
fragments and a `--secret-file` diagnostic are not redaction bypasses.

For the baseline's [dynamic-map rule][map-redaction], lowercase each key
character with `char::to_lowercase` and retain only ASCII letters and digits for
matching. Keep the original key. Replace sensitive-key values with `<redacted>`;
recurse through other values, arrays and objects. This drops non-ASCII
characters Go keeps and can mask additional keys. Do not add a Unicode data
dependency. Use the baseline's sensitive-name match list. Every
credential-marker check on a peer key or name, such as quote line names and
billing history fields, uses the same normalization.

Requested resource data is not the CLI's own credential material. Keep the
baseline behavior for the following typed fields; do not run the dynamic-map
mask over them. The caller may see credentials configured in those resources.
Only their selected output path may reveal the data; never copy it to errors or
debug logs.

| Resource fields                                                   | Baseline evidence                                                                                                                                                                                                                           |
| ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Monitor `Check.Config.Request`: `Headers`, `Query`, `Body`, `URL` | [internal/cli/redact_maps_test.go:177][check-test] explicitly tests typed header and query passthrough. [monitor/models.go:70][check-model] defines the request; list, get, create, update, pause and resume project the same `Check` type. |
| Global load balancer `Listener.Headers`                           | [globalloadbalancer/globalloadbalancer.go:528][glb-header] declares the optional string, used by listener list/get and create.                                                                                                              |
| Load balancer `Listener.InsertHeaders[].HeaderValue`              | [loadbalancer/loadbalancer.go:562][lb-header] declares the typed name and value. Listener list/get and create/update use it.                                                                                                                |
| S3 `HeadObject.Metadata`                                          | [storage/objects_head.go:24][head-metadata] defines already filtered metadata; [internal/cli/svc_storage_object_operations.go:191][head-cli] projects it directly. Keep SDK filtering, which includes the [signing pair][head-filter].      |
| Compute `GetServerConsoleLogOutput.Log`                           | [internal/cli/svc_compute_console_log.go:18][console-log] explicitly reveals the `Secret` only in the `compute get-server-console-log` output adapter. No other output adapter adds a secret reveal.                                        |

Use Rust canaries for greennode's own credentials, the dynamic-map matcher,
existing typed withholding and secret-file output. Keep a separate resource
canary that verifies the baseline passthrough, including queries on the typed
monitor headers and query fields. Synthetic test data never uses real secrets.

## Operation policies

Every declaration carries a policy for partial output, query and rendering.
Ordinary output is its safe projection with the selected format and query. A
safe SDK partial uses the selected format without query; any operation failure
still decides stderr and exit. No output means no stdout unless a policy
declares a safe fallback. Preserve each operation's documented privacy and
absence rules. An ordinary list keeps `Items` and page metadata; do not force
that wrapper on `iam list-policy-attachments`, which returns `Groups`, `UserIDs`
and `ServiceAccountIDs`.

| Operations                                                                                                                                                                                                                                             | Non-default policy                                                                                                                                                                                                                                                                                                                                     |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `containerregistry create-user`                                                                                                                                                                                                                        | On `UserNotFound` always render the safe partial with `SecretFile`, ignoring `--query`. The registry create failure still decides stderr and exit.                                                                                                                                                                                                     |
| `billing list-transactions`, `list-payments`                                                                                                                                                                                                           | Buffer the report. An unqueried table uses fixed columns in baseline order, not sorted generic keys. Codec, query or table refusal withholds output and returns `QueryFailed`/1; a stdout write failure is separate.                                                                                                                                   |
| `billing list-resources`; `storage get-project-auto-renew`, `put-project-auto-renew`                                                                                                                                                                   | Use the operation's table view only without an explicit query. JSON and queries use the SDK shape.                                                                                                                                                                                                                                                     |
| `compute list-flavor-prices`; `network list-network-prices`; `storage list-storage-prices`; `volume list-volume-prices`                                                                                                                                | Sort before rendering; keep the query on `DiscoveryIncomplete` partials. Unqueried table/text uses the pricing row view; include the baseline coverage notes on stderr.                                                                                                                                                                                |
| `pricing estimate`                                                                                                                                                                                                                                     | Keep queries on estimate results or approved partials. Unqueried table/text presents rows, breakdown and totals. Keep baseline coverage on stderr for non-JSON or any query.                                                                                                                                                                           |
| `monitor get-notification-quota`, `get-notification-quota-usage`, `create-notification-quota`, `set-notification-quota-auto-renew`, `delete-notification-quota`, `set-synthetic-quota-auto-renew`, `list-notification-packages`, `get-synthetic-quota` | On success, table/text queries see the presentation view with UTC timestamp strings where present; replace null with `unknown` after the query. Usage also has its explicit JSON view. Settlement partials use the safe SDK projection without query as in the ordinary write failure path.                                                            |
| `monitor quota-status`, `list-synthetic-packages`                                                                                                                                                                                                      | Use the unqueried table/text reports, or the safe SDK shape for queries and JSON, with `unknown` for null cells in table/text. `QuotaStatusIncomplete` renders the safe report without query.                                                                                                                                                          |
| `monitor create-synthetic-quota`, `ensure-synthetic-quota`, `recover-synthetic-quota`, `renew-synthetic-quota`, `delete-synthetic-quota`                                                                                                               | Reject `--query` even if empty. Keep safe outcomes even on preflight refusal, including the fixed mutation stderr operation/code/status where specified and the delete effect text. On a render failure before any stdout attempt, use the baseline safe fallback; do not claim the write was not sent. After any attempt never write another outcome. |
| `compute get-server-console-log`                                                                                                                                                                                                                       | Reveal `Log` explicitly in this output adapter. For plain JSON keep `Log`; queries see the same root. Text without a query, or a text query returning a string, writes raw bytes on a pipe/file and escapes terminal controls except LF and tab on a terminal. No added newline. Query errors withhold the log.                                        |
| `storage put-object`, `get-object`, `list-objects`, `head-object`, `delete-object`, `copy-object`                                                                                                                                                      | Keep the compact native metadata JSON, not transfer bytes, and query only that metadata. Preserve the baseline completed-transfer diagnostic, even for a download's local file change.                                                                                                                                                                 |
| `compute create-ssh-key`; `iam create-service-account`, `reset-service-account-secret`, `create-user`, `reset-user-password`; `storage create-s3-key`                                                                                                  | Publish the adapter's safe secret-file outcome and added fields rather than serializing a secret. Keep per-command recovery if file creation fails or the write is unconfirmed. Service-account create and IAM user create/reset can save a secret despite uncertainty; withhold the partial result and report recovery on stderr as in Go.            |

## Table and text

Use the [query engine](cli-query.md) on the chosen safe tree. Queried JSON uses
sorted object keys, two-space indentation and the same escaping as plain JSON.
No match yields JSON `null`. Table and text without query still use the engine's
generic `f64` value tree; this does not change plain JSON.

Table and text use a result array as rows. If the result is an object with an
array-valued `Items`, use that array instead; render every other object or
scalar as one row. Text prints each row's object values in sorted-key order, no
header, tab separators, one LF per row. Table takes the sorted union of object
keys, plus `Value` for scalar rows or an empty list, with blank cells for
missing keys. Match the [baseline renderer][renderer] for the `+`, `-` and `|`
borders, one padding space either side, and left alignment measured in Unicode
scalar values, not bytes or display columns. Keep the preflight limits of 100
columns, 1,000,000 cells and 64 MiB projected table bytes, including borders and
multibyte characters; check before writing stdout.

Null is an empty cell; booleans are `true` or `false`; numbers use shortest
round-trip fixed-point text, not an exponent. Nested arrays and objects use
compact sorted-key JSON. In string cells and table headers write visible `\t`,
`\n`, `\r`, `\xNN` for other C0/DEL/C1 controls, and lowercase `\uNNNN` for bidi
controls. JSON cells use JSON escaping, not double escaping.

The [query engine](cli-query.md#module-boundary) owns query failures. A table
limit after a confirmed write is `QueryFailed`/1 with the baseline guidance not
to rerun the write and to use `--output json`. An ordinary read table limit is
`InvalidUsage`/2 unless the operation policy says otherwise. Any other encoding
or stdout failure after a confirmed write exits 1 with `RequestFailed` and says
the write succeeded and must not be retried. Never turn a failed write into a
claimed success; reconciliation and partial-output precedence live in
[the dispatcher](cli.md#errors-and-exit-codes).

`serde_json` 1.0.154 (MIT or Apache-2.0, October 2026) handles generic JSON
values, not the exact plain output formatter. Its runtime dependencies include
`serde_core`, `itoa`, `memchr` and `zmij`; no `preserve_order` feature is
needed.

## Goldens

Copy the pinned baseline's sanitized `internal/cli/testdata/golden` cases
verbatim with tag, source path and test name; add missing cases by running the
baseline renderer or dispatcher with synthetic inputs in a disposable local
copy, never against GreenNode. Commit raw stdout/stderr bytes and the exit
status in `crates/greennode/tests/fixtures/` with sanitized input and
provenance. Rust CI consumes only those fixtures, compares bytes including the
final LF, and never runs Go or regenerates the expected side from Rust.

Cover ordered/flattened structs and maps, time and raw JSON, nil/empty/present
slices and maps, exact integer and float boundaries, hostile keys and cells,
redaction before queries, heterogeneous rows, non-`Items` shapes, table limits,
partial output, ignored recovery queries, billing render refusal, quota
presentation timestamps and failed stdout. The query engine's
[Go corpus](cli-query.md#oracle-and-tests) checks parser acceptance and query
semantics separately.

[baseline]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/design/cli.md#output
[lists]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/docs/design/cli-usability.md#list-output-shape
[encoder]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/internal/cli/encode.go
[renderer]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/internal/cli/render.go
[map-redaction]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/internal/cli/redact_maps.go
[check-test]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/internal/cli/redact_maps_test.go#L177
[check-model]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/monitor/models.go#L70
[glb-header]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/globalloadbalancer/globalloadbalancer.go#L528
[lb-header]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/loadbalancer/loadbalancer.go#L562
[head-metadata]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/storage/objects_head.go#L24
[head-cli]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/internal/cli/svc_storage_object_operations.go#L191
[head-filter]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/storage/object_errors.go#L40
[console-log]:
  https://github.com/dannyota/vngcloud/blob/v1.8.0/internal/cli/svc_compute_console_log.go#L18
