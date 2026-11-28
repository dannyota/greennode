# SDK credential screening

The SDK sends credentials, and a peer can reflect one back in a response. This
design owns how every response, retained value, decision, error and CLI byte is
checked for a reflected credential. [Request policies](sdk-requests.md) own the
senders and their bounds; [security](security.md) owns what must never be shown.

## Approaches

- Screen per service. Each review round found variants of the same gap, so this
  is not maintainable.
- Screen once at decode. Values can change after decoding: retained values meet
  later tokens, local errors are built later, and the CLI re-renders output.
- Detect centrally, require a declared outcome per operation, and recheck at
  every decision and exit. Take this approach: a service cannot skip screening
  by omission.

## Matching

Decoders may receive bounded response bytes and typed status internally. Accept
peer error codes and message fields only through the operation's safe
classifier. On credential-bearing or secret-bearing paths withhold fields that
are not known safe; use status fallback and fixed diagnostics. Even a field that
looks like an identifier can be a reflected credential. When any credential was
sent, keep only `code` and `message`, and withhold a message with non-ASCII
text, a credential field marker or a sent credential in any encoding; this is
stricter than Go, which case-folds Unicode. "Any encoding" means the literal,
percent and JSON-escape forms anywhere at any length, and Base64, Base64URL and
lowercase-compared hex forms by these rules: a whole encoding of eight or more
characters, a stable fragment of seven or more (the part of the encoding that
does not depend on neighbouring bytes, at each of the three alignments), and a
hex form of seven or more match anywhere; a shorter whole encoding matches only
when bounded, that is when neither neighbour is an ASCII letter or digit, nor
`+` or `/` for Base64, nor `-` or `_` for Base64URL. Identifier checks match
every form anywhere. Withhold any peer `code` or `message` containing `/`, `\`,
`@`, `www.` in any case, or a colon followed by anything but whitespace or the
end; this closed rule stands in for URL detection. Never expose raw headers,
response bodies, unbounded peer text, URLs, or decode errors via an error
source, capture, or debug event.

The sender records every credential it sends, including the exact header values
as sent, before the bytes leave, so a failure or a dropped future keeps them.
Per-attempt credentials stay available only within the logical request,
including across retries. Matching compiles the credentials once per set into
one multi-pattern pass with a body-wide work and memory budget. The set holds
the 256 distinct credentials Go allows in one operation, each with its wire
forms, and compiled storage grows with the count under a fixed ceiling; the
257th fails closed, as Go does.

## Screening a body

One shared screener checks a peer body: its bytes as received (spanning a key,
delimiter or raw escape), numbers as received, and each decoded string, with the
identifier check for everything under a key whose last word (split at `_`, `-`
or a lower-to-upper case change) is `id`, `ids`, `uuid` or `uuids` in any case;
typed models declare their identifier fields by wire name so a rename keeps the
role. Ordinary authenticated responses must be one JSON value, and opaque
responses get a byte scan. Project discovery screens an error body as a stream
of JSON values before any retry; one that starts as structured JSON and breaks
is indeterminate, while plain text and HTML are text. The result is clean,
reflected or indeterminate, and only clean permits a decision; budget or depth
exhaustion is indeterminate. A 404 proves absence only when its body screens
clean: before a write an unclean 404 refuses with nothing sent, after acceptance
it leaves the outcome `NotSettled`. A safe status stays visible as a diagnostic
(`HttpStatus`) but is never itself proof. A proof is bound to the request it
answered. Plain-text and HTML 404 bodies that screen clean are clean.

Credentials are of three kinds. Transport credentials (bearers, client IDs and
secrets, Basic and Cookie headers, CSRF values, one-time codes) make a guard
read or a write acknowledgement that reflects them untrusted; an ordinary output
read declared `Redact` returns its screened projection instead. Secrets an
operation submitted (a certificate passphrase) are redacted in its output, as Go
does. A newly issued secret is the only copy: from an otherwise unclean
acknowledgement only the declared secret field is taken, checked against the
transport credentials and delivered privately; every other field is withheld and
the outcome is unconfirmed. An ID from an acknowledgement stays bound to its
receipt for every later decision.

The sender screens every attempt before any response guard. Each operation
declares what a match does: refuse, redact (replace, then verify the
replacement), or deliver an issued secret privately after a final check against
the transport credentials.

## Decisions

A read whose value decides whether a write is sent, retried, classified or
settled is a guard read: it refuses any reflection and returns proofs. A proof
keeps its response receipt and its decoded operands with their roles, and is
rechecked against current credentials after the last read of a decision, at
every write attempt, and before a typed refusal or outcome is built. Settlement
loops share one guarded polling step that retires superseded receipts. Every
read after a write's dispatch is a guard read. A guard read answered from a
redirected URL proves nothing, unlike Go, which follows same-origin redirects:
its decision is refused or left unconfirmed.

## Exits

Output spellings apply only to kept values. A withheld status makes
`ApiError::status()` `None`; SDK code classifies from private evidence. Every
SDK result, partial and error, including its assembled text and `Debug`, is
rechecked at the SDK exit; public formatters render numbers the same under any
caller flags and screen both the compact and alternate `Debug` forms. The CLI
buffers each authenticated command's stdout and stderr and checks the exact
bytes before writing them.
