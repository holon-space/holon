---
id: 2026-09-01-capability-token-in-url-path-survives-redaction
date: 2026-09-01
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `redact_url` stripped only the query string, so a credential sitting in a URL
  PATH segment — the capability-URL shape — printed verbatim into every `rest`
  transport error and log line, and again whenever the upstream body echoed the
  request URL back.
---

## Bug

Found by code audit (a planning lane reading the `rest` transport's error
paths), not by a test.

`redact_url` (crates/holon-mcp-client/src/rest_oauth2.rs:571) split on `?` and
replaced whatever followed. A URL whose credential is a path segment therefore
passed through unchanged, and all eight error/log sites in
`crates/holon-mcp-client/src/rest_transport.rs` printed it: the JSON-decode
failure (:252), the feed-decode failure (:267), the send and body-read failures
(:307, :315), the 401-retry `warn!` and its post-refresh failure (:336, :344),
the non-2xx failure (:353), and the paginated decode failure (:387).
`RestManual`'s and `RestCallSurface`'s `Debug` printed `base_url` raw as well,
so a panic carrying an `McpTransport::Rest` disclosed it too.

Two carriers, not one: the URL the message names, and the response-body preview
those messages quote — an upstream 404/500 that echoes the request target puts
the same credential in the message a second time, where no URL-shaped redaction
would ever reach it.

The `rest` transport already serves live integrations, and an upcoming one
authenticates with a capability URL whose path segment IS the credential. Its
first failing request would have written the token into the log file and the
user-visible toast.

Red log (4/4 red, each leaking a synthetic token in both carriers):
`lane-logs/red-02.txt`.

## Root cause

Redaction was decided by SHAPE (everything after `?`) when the property that
makes a value secret is its PROVENANCE. `integration_config.rs` knows exactly
which substrings are secret — a `${VAR}` reference exists in a sidecar
*because* the value must stay out of the YAML — but that knowledge was
discarded the moment `expand_vars` returned, leaving the transport to guess
from URL syntax.

## Missing piece

No test ever configured a `${VAR}`-sourced value into a URL path segment: every
`rest` fixture put its secret in a header or in the OAuth token-endpoint body,
so the sidecar alphabet the crate's tests exercise never contained the
capability-URL shape (COVERAGE).

Secondarily, the redaction assertion that did exist
(`oauth2_refresh_failure_redacts_all_secrets`) was scoped to the token
endpoint's response body, and `redact_url_strips_query` pinned only the query
half. Nothing asserted the general invariant — *no resolved secret appears in
any string this transport emits* — so even a generated case would have needed a
new oracle to go red (ORACLE).

The composed keystone cannot reproduce this: it wires no `rest` transport and
makes no outbound HTTP request, so no draw reaches this code at all. Prod/test
parity work here would mean giving the keystone an integration-with-a-mock-server
rung; the cheaper pin is the crate-level test below, which drives the real
transport over a real socket.

## Remedy

Redaction now follows provenance. `Redactor`
(crates/holon-mcp-client/src/redaction.rs) holds the registered secrets behind
an `RwLock` and travels on `RestManual` as a field; the OAuth2 provider behind
that transport holds the same one. `Redactor::redact` replaces each registered
value wherever it occurs — path segment, echoed body, or upstream error text —
and `Redactor::redact_url` adds the old query-strip on top, so an unknown query
parameter is still dropped even when it came from no `${VAR}`. The module doc
states the contract: what is registered, what is not, and the deliberate
over-redaction tradeoff.

Secrets register themselves at their source rather than at one construction
moment, which is what makes runtime credentials reachable: `expand_vars`
registers each `${VAR}` value, `OAuth2TokenProvider::from_config` registers the
client secret and refresh token, and `do_refresh` registers every access token
as it is minted. A configuration-time-only set would have missed the minted
token entirely — it is never configured anywhere, and a resource server that
echoes the `Authorization` header into an error body discloses it.

Matching canonicalizes both sides rather than enumerating encodings: the
registered secret and the candidate text are percent-decoded, `+` read as a
space and `\` as `/`, the match is found in canonical space, and the span is
mapped back onto the ORIGINAL text, which is what gets replaced. Enumerating
encodings was the first attempt and it is not sound — the HTTP stack escapes
some characters and not others, so a secret can arrive partially encoded (`|`
raw beside `%3C`) in a form no precomputed set contains, and a URL parser
rewrites `\` to `/`, which is not an encoding at all.

Runtime tokens live in a ring of the last `MINTED_RING` (3), so a long-lived
process refreshing on a timer does not accumulate dead credentials; secrets
resolved at configuration time are never evicted.

All eight sites route through one pair of helpers on `RestCallSurface`:
`safe_url` for the request URL and `safe` for the finished message, so a site
added later that forgets `safe` is the only way to reintroduce the leak. Both
`Debug` impls redact `base_url`.

Values shorter than 8 bytes are not registered: a short value collides with
ordinary message text, and mangling every occurrence of `true` protects
nothing. A credential that short is out of the redactor's reach — it is not a
credential worth the name, but it is a stated limit, not an oversight.

Covered by `crates/holon-mcp-client/tests/rest_transport_redaction.rs`, which
drives the real transport against a local mock whose bodies echo the request
target: an HTTP 500, a non-JSON 200, the OAuth2 401-refresh-and-retry path
(asserted against captured `tracing` output, not just the returned error), a
body echoing the `Authorization` header, a secret whose wire form is
percent-encoded, and the `Debug` impl. Each asserts both halves — the secret
absent AND the `<redacted>` marker present — so a redaction that worked by
going silent does not pass. `redaction.rs`'s unit tests pin the path-segment
case, the echoed-body case, both encoded forms, late registration, the length
guard, and longest-secret-first ordering; `redact_url_strips_query` still pins
the generic query layer.

The contract is also stated where a user configures these secrets
(`assets/integrations/README.md`, "auth"), including the 8-byte floor and the
greedy-redaction consequence of putting a whole URL in one variable.

Red logs from two adversarial-verifier rounds, each with the relevant fix
disabled:

- `lane-logs/red-d1d2-02.txt` — the minted bearer token printed in full from an
  echoed `Authorization` header; a secret's `%20` wire form surviving in an
  echoed body while its raw form was stripped from the URL beside it.
- `lane-logs/red-e1-02.txt` — with canonicalization off, three encoding shapes
  leak: `cap%204Qk3…` (fully escaped), `cap|4Qk3%3CvR7…` (partially escaped),
  and `cap/4Qk3…` (backslash rewritten by the URL parser).

### Known residual

A secret reaching a message under any transform other than URL encoding —
base64, a hash, a provider that echoes only a prefix — is not matched. That
needs the transform, not the value, and no such path is known in the current
transports. A minted token older than the last three is also no longer
redactable, by design.
