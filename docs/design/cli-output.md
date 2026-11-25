# CLI output and queries

Status: proposed.

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

Redact typed secrets or withhold the affected field according to the operation's
contract. Keep the [baseline dynamic-map rule][map-redaction] before any
serialization or query: lowercase letters and digits only for comparison,
replace sensitive-key values with `<redacted>` and recurse through other values,
including nested arrays and objects. Do not conflate typed maps with untyped
maps: apply each typed field's explicit policy. A secret written to
`--secret-file` is never part of the projection, including on failure. Raw
fragments must already be approved as safe; raw is not a redaction bypass.

Ordinary list outputs keep their `Items` wrapper and any page metadata. Do not
wrap or flatten a result merely because the operation name starts with `list-`:
`iam list-policy-attachments` has `Groups`, `UserIDs` and `ServiceAccountIDs`.
Keep each service's baseline custom output where present, such as compact native
S3 metadata, pricing views and Synthetic quota outcomes. They share the safe
output path and error dispatcher.

## Query, table and text

When a query is present, or the output is table/text, take the same redacted
projection and normalize every JSON number to IEEE 754 `f64`, including query
JSON literals. Values above 2^53 can round. This never changes plain JSON. Use
the compiled query, if any. A queried JSON result uses sorted object keys at
every depth, two-space indentation and the same escaping. No match yields JSON
`null`, not an error. Float results use Go's generic JSON float spelling.

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

Syntax is checked before any request. Evaluation failure is `QueryFailed`, exit
1, without the raw engine error. After a successful write the message says
`query failed after the write succeeded` and warns against retrying. A table
limit after success is also `QueryFailed`, with the baseline guidance not to
rerun the write and to use `--output json`. On reads a table limit is
`InvalidUsage`, exit 2. Any other encoding or stdout failure after a confirmed
write exits 1 with `RequestFailed` and says the write succeeded and must not be
retried. Never turn a failed write into a claimed success; reconciliation and
partial-output precedence live in
[the dispatcher](cli.md#errors-and-exit-codes).

## Dependencies and compatibility

| Choice                                                                              | Evidence                                                                                                                                                                                                                                                                       |
| ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [jmespath](https://crates.io/crates/jmespath) 0.5.0, MIT (chosen)                   | Official project, January 2026 release and latest upstream commit checked. It has [generated compliance tests][rust-tests], a reusable expression and public AST/runtime. Uses `serde`, `serde_json`, and build-time `slug` (with `deunicode`). Use stable Rust features only. |
| [jmespath_community](https://crates.io/crates/jmespath_community) 0.1.2, Apache-2.0 | November 2025 release and upstream activity. Its [compliance suite][community-tests] targets the community spec, which adds syntax and functions not in the Go baseline. Adds `indexmap`, `lazy_static`, `regex` and `santiago` on top of Serde.                               |
| [serde_json](https://crates.io/crates/serde_json) 1.0.154, MIT or Apache-2.0        | October 2026 release. Use for JSON input and generic query values, not as the exact plain output formatter. Runtime dependencies include `serde_core`, `itoa`, `memchr` and `zmij`; no `preserve_order` feature required.                                                      |

The Go baseline uses [go-jmespath v0.4.0][go-tests]. Both projects run a
JMESPath compliance suite; that does not prove equivalence across versions. Use
an adapter in `query.rs` rather than equating the stock engines. In particular
the [Rust value equality][rust-equality] tolerates near-equal floats while [Go
compares exactly][go-equality]. Normalize number literals and operands to `f64`;
lower equality and inequality in the parsed AST to internal runtime functions
with exact recursive comparison, including comparisons in filters and expression
references. Adapt affected built-ins as differential tests require. Keep
ordering comparisons numeric only. Leave an unknown function or invalid runtime
argument to evaluation, as in Go. Do not reproduce a Go panic on a query input;
return `QueryFailed`. Object wildcard and object `keys`/`values` do not promise
Go's map iteration order; use the Rust engine's stable order. Resolve
differences on supported queries before shipping, using the release engine in
the compatibility tests.

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
partial outputs and failed stdout. Take a shared spec corpus plus locally
generated Go differential cases for numeric comparisons, filters, projections,
quoting, functions, runtime errors and null. For unspecified object traversal
order compare membership, not bytes. Snapshot the result or error of each
deterministic case before running the Rust comparison.

[baseline]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/design/cli.md#output
[lists]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/docs/design/cli-usability.md#list-output-shape
[encoder]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/internal/cli/encode.go
[renderer]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/internal/cli/render.go
[map-redaction]:
  https://github.com/dannyota/vngcloud/blob/v1.0.1/internal/cli/redact_maps.go
[rust-tests]: https://github.com/jmespath/jmespath.rs/tree/master/jmespath/tests
[community-tests]: https://github.com/jmespath-community/rust-jmespath
[go-tests]: https://github.com/jmespath/go-jmespath/tree/v0.4.0/compliance
[rust-equality]: https://docs.rs/jmespath/0.5.0/src/jmespath/variable.rs.html
[go-equality]: https://github.com/jmespath/go-jmespath/blob/v0.4.0/util.go
