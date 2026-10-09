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

MCP requests: the second verifier round (`lane-logs/jaq-verify.md`, R2-D2, R2-D3) found that
a peer sending one byte every <120 s held a tool call open forever, because the idle timeout
was the only bound, and that a timeout while reading a JSON reply named `REQUEST_TIMEOUT`
when `MCP_IDLE_TIMEOUT` fired. Now every MCP request waits at most `REQUEST_TIMEOUT` (120 s;
the slowest bundled call measured is 4.5 s, `lane-logs/jaq-harden-r3-mcp-timing.log`), names
the call in its error and tells the peer `notifications/cancelled`
(`crates/holon-mcp-client/src/mcp_request.rs`: `call_tool`, `read_resource`, `list_tools`,
`list_resource_templates`, the `initialize` handshake). A POST is bounded by the same
deadline, because rmcp sends a connection's messages one POST at a time and one reply that
never ends would stall every later request. A read error names the timeout of the client
that read. Tests: `mcp_call_surface::tests`, `mcp_request::tests`, `mcp_http_client::tests`
(`a_json_reply_that_*`) and `http_bounds.rs`
`an_mcp_peer_that_never_answers_ends_in_a_request_timeout_error`. Red log
`lane-logs/jaq-harden-r3-red.log`.

sse-stream: 0.2.4 copied an unfinished line on every chunk (quadratic CPU) and recursed once
per ready chunk, so 16 384 ready 4 KiB chunks of one event overflowed a 2 MiB stack. Updated
to 0.2.6 (which loops and extends the line in place) in `Cargo.lock`,
`frontends/holon-worker/Cargo.lock` and `frontends/dioxus-web/Cargo.lock`. Test
`one_event_of_the_largest_size_in_small_chunks_parses_in_linear_time`: SIGABRT on 0.2.4 with
a 2 MiB stack (`lane-logs/jaq-harden-r3-red.log`), 23.3 s on 0.2.4 with a 1 GiB stack
(`lane-logs/jaq-harden-r3-red-sse-quadratic.log`), 0.6 s on 0.2.6
(`lane-logs/jaq-harden-r3-green-sse-bigstack.log`).

Open:
- rmcp's OAuth code exchange (`AuthorizationManager::exchange_code_for_token`) builds its own
  client with no timeout.
- rmcp reads OAuth metadata and error bodies with `Response::text()`, with no cap.
- The SSE stream of a POST reply is not cut at the deadline. The caller gets its error, but the
  stream stays open while the peer keeps sending.
- `frontends/waterui/Cargo.lock` still has sse-stream 0.2.1. That lockfile is stale: updating
  it also adds `icu_casemap`, so it was left unchanged.
