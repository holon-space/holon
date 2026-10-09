---
id: 2026-10-09-connection-http-client-has-no-timeout-or-body-cap
date: 2026-10-09
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  The connection HTTP clients (REST, OAuth token, MCP over HTTP) waited forever on a peer that
  never answered and read a response body or message of any size into memory.
---

## Bug
Found by a code audit of the jaq response mappers (vault item `5cbf9b32`, finding F5). The
security verifier of that fix (`lane-logs/jaq-verify.md`, D2) found that MCP-over-HTTP
connections were not covered: `connect_mcp_with_handler` used rmcp's default
`reqwest::Client` and `connect_mcp_oauth_with_handler` passed `reqwest::Client::default()`,
so they had no timeout, no body cap and reqwest's default redirect policy, which follows a hop
from https to http. Their errors also quoted the connection URL.

## Root cause
`secure_http_client` (`crates/holon-mcp-client/src/secure_client.rs`) set only a redirect
policy; `rest_transport.rs`, `rest_oauth2.rs` and `oauth_bootstrap.rs` read bodies with
`Response::text()`. `mcp_provider.rs` built its transports from rmcp's client, which reads a
JSON reply with `Response::json()` and an SSE event with no limit.

## Missing piece
No loopback peer in the tests served an oversized body, held a request open or redirected an
MCP connection.

## Remedy
REST and OAuth token calls: the client carries `REQUEST_TIMEOUT`, and every body is read
through `secure_client::read_text`, which stops past `MAX_RESPONSE_BODY_BYTES` as the bytes
arrive. Tests: `crates/holon-mcp-client/tests/http_bounds.rs`. Red log
`lane-logs/jaq-harden-red-http.log`. Teeth: `lane-logs/jaq-harden-teeth-bodycap.log` and
`lane-logs/jaq-harden-teeth-timeout.log`.

MCP over HTTP: `mcp_http_client::McpHttpClient` has the same redirect policy. It uses the
idle timeout `MCP_IDLE_TIMEOUT` because a total timeout would cut a healthy long-lived SSE
stream. It caps each JSON reply and each SSE event at `MAX_RESPONSE_BODY_BYTES`, and strips
URLs from its errors. The OAuth `AuthorizationManager` gets `secure_http_client`. Tests:
the `an_mcp_*` tests in `http_bounds.rs` and `mcp_http_client::tests`. Red logs:
`lane-logs/jaq-harden-r2-red-d2.log` and `lane-logs/jaq-harden-r2-red-d2-sse.log`. Teeth for
idle vs. total timeout: `lane-logs/jaq-harden-r2-teeth-idle.log`.

Open:
- rmcp's OAuth code exchange (`AuthorizationManager::exchange_code_for_token`) builds its own
  client with no timeout.
- rmcp reads OAuth metadata and error bodies with `Response::text()`, with no cap.
- sse-stream 0.2.4 copies an unfinished line on every chunk, so a 64 MiB single-line event
  costs quadratic CPU. sse-stream 0.2.6 extends the line in place.
