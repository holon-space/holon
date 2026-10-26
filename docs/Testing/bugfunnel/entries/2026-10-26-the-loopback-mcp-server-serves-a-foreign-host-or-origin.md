---
id: 2026-10-26-the-loopback-mcp-server-serves-a-foreign-host-or-origin
date: 2026-10-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The holon MCP HTTP server that every frontend starts answered requests whose Host or Origin named a foreign site, so a DNS-rebinding web page could open a session and call its tools.
---

## Bug
Found by code audit: a verifier probed the net daemon's MCP server with `Host: evil.example` and `Origin: http://evil.example` and got a session; a static read of the holon MCP server found the same gap.
The server binds 127.0.0.1:8520 by default, has no auth, and its tools run raw SQL, writes and UI input.
A page that rebinds its own DNS name to 127.0.0.1 is same-origin to that server, so the browser sends its requests and reads the `Mcp-Session-Id` reply.
The MCP spec says servers MUST validate Origin.

## Root cause
`serve_http_on` (`frontends/mcp/src/di.rs`) built both routers (the engine one and the browser-relay one) with no Host or Origin check.
rmcp 0.12's `StreamableHttpServerConfig` has no host allow-list, so nothing below the router checked either.

## Missing piece
No test sent a request with a foreign Host or Origin to the real router.
Litmus, coverage: no test, keystone or otherwise, generates an HTTP request with chosen headers against the MCP router.
Litmus, oracle: had one been sent, a 200 with a session id would have shown the defect directly.

## Remedy
`holon-loopback-guard::refuse_foreign_host_or_origin` wraps both routers in `serve_http_on`: 403 unless Host is the loopback name (or the specific bound address) on the bound port and Origin is absent or `http://` plus such a name.
`frontends/mcp/tests/foreign_host_or_origin_is_refused.rs` sends `initialize` to the real router: foreign Host and foreign Origin are refused without a session; loopback Host with no or a loopback Origin is served.
The net daemon's server reuses the same layer once it lands.
