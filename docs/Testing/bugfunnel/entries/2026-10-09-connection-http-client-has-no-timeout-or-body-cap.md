---
id: 2026-10-09-connection-http-client-has-no-timeout-or-body-cap
date: 2026-10-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The connection HTTP client waited forever on a peer that never answered and read a response
  body of any size into memory.
---

## Bug
Found by a code audit of the jaq response mappers (vault item `5cbf9b32`, finding F5).

## Root cause
`secure_http_client` (`crates/holon-mcp-client/src/secure_client.rs`) set only a redirect
policy; `rest_transport.rs`, `rest_oauth2.rs` and `oauth_bootstrap.rs` read bodies with
`Response::text()`.

## Missing piece
No loopback peer in the tests served an oversized body or held a request open.

## Remedy
The client carries `REQUEST_TIMEOUT`; every body is read through `secure_client::read_text`,
which stops past `MAX_RESPONSE_BODY_BYTES` as the bytes stream in. Both errors name their
limit. Tests `crates/holon-mcp-client/tests/http_bounds.rs`; red log
`lane-logs/jaq-harden-red-http.log`; teeth `lane-logs/jaq-harden-teeth-bodycap.log`,
`lane-logs/jaq-harden-teeth-timeout.log`.
