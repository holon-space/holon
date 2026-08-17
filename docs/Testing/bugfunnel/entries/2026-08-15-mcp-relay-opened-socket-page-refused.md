---
id: 2026-08-15-mcp-relay-opened-socket-page-refused
date: 2026-08-15
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The MCP relay opened a `ws://` socket from an `https://` page — refused by
  the browser before any request leaves — and then retried it every second
  forever.
source_line: 697
---

## Bug

(task-#38/#41 web lane; found by console triage while driving live
holon.space) **The MCP relay opened a `ws://` socket from an `https://` page
— refused by the browser before any request leaves — and then retried it
every second forever.** `connect_mcp_relay` built the URL from
`location.host()` with no scheme guard (`frontends/dioxus-web/src/main.rs`),
so on holon.space every attempt died as `SecurityError: An insecure
WebSocket connection may not be initiated from a page loaded over HTTPS`,
and the failure arm recursed into itself after a 1s timeout with no attempt
cap — a permanent 1 Hz failure loop for the life of the page. This REVISES
defect D4 in `lane-logs/t32-web-report.md:257` ("`/mcp-hub?role=browser` 404
× 19"): over HTTPS it is neither a 404 nor bounded at 19; the earlier count
was a localhost-only artefact. Severity honest — it masked nothing, sidebar
navigation and focus were driven successfully with the loop running, which
is why it survived a public launch.

## Root cause

task-#38/#41 web lane, found by console triage while driving live
holon.space: **the MCP relay opens a `ws://` socket from an `https://` page,
which the browser refuses before any request leaves, and then retries it
every second forever.** `connect_mcp_relay` built the URL as
`format!("ws://{host}/mcp-hub?role=browser")` from `location.host()` with no
scheme guard (`frontends/dioxus-web/src/main.rs:504`), so on holon.space
every attempt dies as `SecurityError: An insecure WebSocket connection may
not be initiated from a page loaded over HTTPS`; the error arm then recursed
into itself after a 1s timeout with NO attempt cap (`main.rs:509-517`),
making it a permanent 1 Hz failure loop for the life of the page. This
REVISES defect D4 as recorded in `lane-logs/t32-web-report.md:257`
("`/mcp-hub?role=browser` 404 × 19") — on HTTPS it is neither a 404 nor
bounded at 19; the earlier count was a localhost-only artefact. Severity is
honest: it does NOT mask or break interaction — sidebar navigation and focus
were driven successfully with the loop running — it is console noise plus
wasted wakeups, which is exactly why it survived a public launch.
ENVIRONMENT: the failing path is scheme-dependent browser-platform code that
no headless test environment instantiates, and no gate loads the app over
HTTPS at all. FIXED in this lane as a separate labelled change: scheme now
follows `location.protocol` (`wss` when `https:`) and retries are capped at
`MCP_RELAY_MAX_ATTEMPTS = 5` with a disclosed give-up warning rather than a
silent stop; the on-close reconnect resets the counter to 0 because a socket
that HAD connected means the hub exists and a close is a restart, not an
unreachable host. `cargo check --target wasm32-unknown-unknown` green.)

## Missing piece

The failing path is scheme-dependent browser-platform code that no headless
test environment instantiates, and no gate loads the app over HTTPS at all.

## Remedy

FIXED as a separate labelled change. The scheme now follows
`location.protocol` (`wss` when `https:`). Retries are capped at
`MCP_RELAY_MAX_ATTEMPTS = 5` consecutive never-opened failures, after which
one disclosed `warn` names the relay URL and states that MCP tooling is
unavailable for the page, and retrying stops. The cap counts the right
thing: an `onclose` WITHOUT a preceding `onopen` is a construction-level
failure and increments, while an `onclose` AFTER a successful `onopen`
resets the counter to 0, because a socket that connected proves the hub
exists and its close is a restart (`trunk --watch`), not an unreachable
host. `cargo check --target wasm32-unknown-unknown` green; runtime proof of
exactly 5 attempts then the warn then silence in
`lane-logs/t38d-retrycap-proof.txt`.
