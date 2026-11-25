# CLI query engine

Port [go-jmespath v0.4.0][go-jmespath], the baseline's query engine, into
`crates/greennode/src/cli/query/`. Match what Go accepts before dispatch and
what it evaluates after dispatch. No SDK dependency on the query engine and no
JMESPath crate dependency.

## Approaches

| Approach                                  | Trade-off                                                                                                                                                 |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Use another Rust JMESPath engine directly | Small integration, but nominal compliance does not preserve which syntax can reach a write or how Go evaluates it.                                        |
| Adapt another engine                      | AST rewrites can fix number equality but cannot recover a missing comma that the lexer/parser already accepted. Function differences need separate fixes. |
| Port the pinned baseline (chosen)         | Own the lexer, parser, evaluator and function semantics. Test both stages against the exact Go oracle without shipping Go.                                |

## Module boundary

`query/mod.rs` exposes
`compile(expression) -> Result<CompiledQuery, SyntaxError>` and
`evaluate(compiled, value) -> Result<Value, EvaluationError>` to the CLI frame.
`lexer.rs` tokenizes with byte offsets; `parser.rs` owns precedence, the Go
parser's separator rules (including its quirks, which the differential fixtures
pin) and the private AST; `interpreter.rs` evaluates that AST; `functions.rs`
owns the fixed Go function names, arity, type checks and evaluation. There is no
user-defined function registry, no internal function addressable by user syntax,
and no reflection over SDK objects. This interface is not a public SDK API.

The frame treats an omitted or empty query as no query, except where an
operation rejects the flag itself. Compile a permitted nonempty query once in
the [local checks phase](cli.md#dispatch-and-guards), including for native S3.
Require end of input. Keep compile-time syntax checks separate from runtime
errors: an unknown function, invalid argument type or arity, and zero slice step
remain evaluation failures where Go defers the check. `InvalidUsage`/2 for
invalid syntax goes to stderr before any request. An evaluation failure is
`QueryFailed`/1; after a confirmed write the error says the write succeeded and
not to retry. The classifier never echoes the query expression, source values,
or evaluator error text.

## Values and limits

Evaluate only the safe [output projection](cli-output.md), including an
operation's declared presentation projection for table or text. Use null,
boolean, string, array and object values, and IEEE 754 `f64` numbers for both
input JSON and expression literals. Compare numbers exactly in equality and
within recursive array/object equality; ordering comparisons are numeric only.
Preserve Go's false values, short-circuit return values, expression references,
projection null handling, pipes, Unicode strings and slices. The unqueried JSON
path never passes through the query value tree. An expression reference is a
value only as a direct function argument; anywhere else Go would store or return
one (a multiselect, an index, a pipe, a projection or the result) is an
evaluation error instead.

Port the functions as Go implements them, not as a newer specification might.
`to_number('true')` returns null, and string parsing follows Go's `ParseFloat`,
not JSON parsing. For arrays, `contains` uses the Go scalar equality rule, not
approximate equality or deep comparison. `to_string` passes through strings and
otherwise uses compact Go JSON with sorted keys, Go float spelling and HTML
escaping; it does not insert terminal escapes inside the returned string. The
renderer escapes that string on output.

Go's lexer and parser have no expression size or syntax-tree depth policy.
Accept up to 16 KiB of expression bytes and 64 levels of parser nesting, AST
depth and embedded JSON nesting. Check before recursive descent or constructing
a deeper node; long left-associated chains count too. This is a syntax refusal,
not a remote guard. Go compiles an unterminated index such as `[1` to an empty
node that fails only at evaluation, after a write may have run; refuse it at
compile time instead. Evaluation uses checked indexing and slice arithmetic, and
checks structural depth before recursing into data or result trees (128 levels,
excluding the root). Panics in Go, such as a `contains` comparison of two
uncomparable arrays or objects, become safe evaluation errors. A value that
cannot be encoded as JSON, including a nonfinite result, fails evaluation rather
than replacing the value silently. Evaluation also has a work budget, because
depth alone does not bound it: a few `[@,@]` steps joined by pipes double the
result each time. Count one unit for every AST node visited, every value element
produced, and every element a built-in function or a recursive comparison
examines, and stop at 10,000,000 units. Track the expanded size of the result as
it is built, counting shared subtrees once per use, and stop at 64 MiB, the
table limit. Both checks run before the allocation they guard. Exceeding either
is `QueryFailed`, with the after-write wording when a write succeeded. A second
test runs a filter that calls `contains` on a long array for each row and
produces almost nothing, and asserts the budget stops it. A test runs one
mutation, then a doubling query, and asserts exactly one mutation request and
the no-retry message. For `*` on an object and for `keys` and `values`, traverse
by UTF-8 key bytes in ascending order. Go's map iteration does not promise an
order. Keep array ordering and all other specified ordering unchanged.

## Oracle and tests

Store verbatim `v0.4.0` `compliance/*.json` under
`crates/greennode/tests/fixtures/query/compliance/`. Commit `differential.json`
and `provenance.md` beside that directory. Record the upstream module, tag and
checksum, suite file or synthetic case source, generator command, Go version and
any explicit exception. Generate the differential corpus from the pinned Go
library once, locally, without calling GreenNode. Record `Compile` success or
failure independently of `Search` result, error or recovered panic. Store
expected successful JSON and the stage of each failure; no Go evaluation runs in
Rust CI.

Run the whole vendored suite using the upstream test harness's expected results,
not a newer corpus. Its `error` cases only assert that Go `Search` fails; the
differential cases pin whether parsing or evaluation fails. Test rejection and
acceptance of separators, trailing tokens, Unicode and raw/escaped literals,
filters, functions, projections, wildcard, null, negative indices/slices and
nested expression references. Pin exact and near-equal floats, large integers
rounded to `f64`, `contains`, `to_number('true')`, and compact `to_string`.
Compare ordered values exactly; only tests of Go's unspecified object traversal
order compare membership. Separately assert the Rust engine's stable order,
panic-free boundary cases, and length/depth refusals immediately over the
bounds.

Dispatch a paid write on loopback with `--query '[a b]'`. It must return
`InvalidUsage`/2 with zero requests, including sign-in and quote. Test an
accepted expression that fails at evaluation, with a read and a successful
write: each returns `QueryFailed`/1, and the write runs only once with the
no-retry diagnostic. The [CLI output goldens](cli-output.md#goldens) test the
bytes the query renderer produces, not only the engine's value.

## Attribution

The [upstream license][go-license] is Apache-2.0 and names
`Copyright 2015 James Saryerwinnie`. No upstream `NOTICE` is present at
`v0.4.0`. Keep the upstream copyright and license attribution in the repository
and in the published CLI package for the translated source and copied fixtures.
Mark translated sources as changed under [Apache-2.0 section 4][apache-license];
keep the pinned source and fixture provenance with the port. The existing
project license does not replace the upstream copyright notice.

Concretely: copy the upstream license unchanged to
`crates/greennode/licenses/go-jmespath-LICENSE`, which the published package and
every release archive include. Each translated source file starts with one line,
`// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.`,
which the [engineering standard](../standards/engineering.md#code-comments)
allows as a license notice. Copied fixtures carry a `PROVENANCE.md` naming the
same source and license.

[go-jmespath]: https://github.com/jmespath/go-jmespath/tree/v0.4.0
[go-license]: https://github.com/jmespath/go-jmespath/blob/v0.4.0/LICENSE
[apache-license]: https://www.apache.org/licenses/LICENSE-2.0#redistribution
