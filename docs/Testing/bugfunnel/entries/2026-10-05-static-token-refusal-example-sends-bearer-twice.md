---
id: 2026-10-05-static-token-refusal-example-sends-bearer-twice
date: 2026-10-05
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The refusal of a literal auth.static_token told the author to write
  "Bearer ${GITHUB_TOKEN}", which for static_token sends "Bearer Bearer <token>".
---

## Bug
Found by Martin in the live app (2026-10-05). A `github.yaml` with
`auth: { static_token: "ghp_..." }` was refused with an example that fits a
static header pair, not `static_token`. The refusal was followed by "no
schema_version could be established, and there is no bundled copy of 'github'",
which did not say the file must declare `schema_version: <current>`.

## Root cause
One `SecretRef` type and one `Display` message served `auth.static_token` and
`holon.auth.value`. The HTTP transport adds "Bearer " itself for a static token
(`AuthMode::StaticToken`, crates/holon-mcp-client/src/mcp_provider.rs), so a
prefix is wrong there and right for a header value
(crates/holon-mcp-client/src/secret_ref.rs).

## Missing piece
Tests pinned that a literal is refused and not quoted, but no test read the
message's example against the field. A prefixed `static_token` parsed
successfully, so the double "Bearer" only showed on the wire.

## Remedy
`static_token` is now a `TokenRef` (exactly one bare `${VAR}`; a prefix is
refused, naming that the transport adds "Bearer " itself). Every refusal gives
the example of its own field. The unbundled-provider refusals name
`schema_version: <SIDECAR_SCHEMA_VERSION>`. Tests:
`inline_secret_refused::a_static_token_with_a_literal_prefix_is_refused_naming_the_transport`,
`every_static_token_refusal_gives_the_static_token_example`,
`every_header_value_refusal_gives_the_header_value_example`,
`user_introduced_connection::an_unparseable_unbundled_file_says_what_schema_version_it_must_declare`
(red log: lane-logs/bearer-msg-red.log).
