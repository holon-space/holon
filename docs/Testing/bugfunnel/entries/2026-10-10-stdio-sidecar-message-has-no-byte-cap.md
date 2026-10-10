---
id: 2026-10-10-stdio-sidecar-message-has-no-byte-cap
date: 2026-10-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  One stdio JSON-RPC message of 68 MiB was read and delivered whole — the response-body cap
  bounded the HTTP legs only, so a sidecar decided how much memory its framing used.
---

## Bug
Found by the security verifier of the MCP peer hardening lane `jaq-harden`
(`lane-logs/jaq-verify5.md`, D2) — a hostile mock sidecar, not a test in the suite. A sidecar
answers `initialize` and then writes one `notifications/resources/updated` whose `uri` is
68 MiB. Measured through the production stdio connect (`lane-logs/jaq-r7-red-all.log`):
**delivered whole, 71,303,176 bytes**, past `MAX_RESPONSE_BODY_BYTES` (64 MiB).

## Root cause
`crates/holon-mcp-client/src/mcp_provider.rs` used `TokioChildProcess::new` directly. rmcp
frames stdio messages with `JsonRpcMessageCodec`, whose `max_length` is `usize::MAX`
(`rmcp-v0.12.0`, `crates/rmcp/src/transport/async_rw.rs:157`), and `TokioChildProcess`
exposes no way to set it and no way to reach the child's stdout. The lane's caps lived in
`mcp_http_client.rs` / `secure_client.rs` and covered the HTTP legs: an unterminated line on
the stdio leg was memory the sidecar sized.

## Missing piece
Every bound this lane added was written against an HTTP peer, because that is the transport
the hostile-peer mocks speak (raw sockets, `peer_budget_bounds.rs`, `http_bounds.rs`). No test
drove a stdio sidecar that frames a message at all, so the transport with the WEAKER framing
was the one nothing exercised — the bundled sidecars are all stdio.

## Remedy
`crates/holon-mcp-client/src/child_transport.rs`: Holon spawns the sidecar itself
(`kill_on_drop`) and reads its stdout through
`BoundedChildStdout`, which charges the bytes of the line it has not finished to the
connection's `HeldEventBytes` — the same per-connection `MAX_RESPONSE_BODY_BYTES` allowance the
HTTP legs' partial SSE events charge, so there is one allowance per connection and not one per
transport. A finished line releases what it held, so a well-behaved sidecar streams any volume
in messages. Past the allowance the read fails naming the bound, rmcp logs it at `error!` and
the leg ends; the integration degrades, Holon keeps running.

The ended leg is disclosed rather than only logged: the refusal is published on
`PeerBudget.transport` (`BoundTrips`), carried out as `McpIntegration.transport_ended`, and
`holon-app`'s `spawn_disclose_transport_end` records `IntegrationStatus::Unavailable` on the
integration's row and raises `ConditionKind::IntegrationConnectionEnded` with the bound named in
the body, so the row never keeps claiming a transportless integration is connected.

The sidecar's stderr was the other half of the same hole: inherited, it goes wherever Holon's
own stderr goes, and with `HOLON_LOG=file://…` that is an append-only, unrotated file a peer
grows line by line. It is now piped and read to its end by `forward_stderr` (so the pipe
cannot fill and stall the sidecar) under `MAX_STDERR_BYTES` (1 MiB per connection,
`StderrAllowance` in `peer_budget.rs`): each line is cut at `MAX_DISCLOSED_PEER_TEXT_BYTES`
with the skipped tail counted, escaped through `bounded_peer_text`, and charged to the
allowance. Past it the first dropped line names the bound, the running count is repeated at
every power of ten, and the total is disclosed when stderr ends — dropped, never silently.

`bounded_peer_text` itself now escapes every control character, newline included: a sidecar's
stderr line and a peer's error text both reach a line-oriented log and a one-line Integrations
row, where a raw newline is a log line the peer wrote itself and a CSI sequence rewrites the
terminal. The escaped bytes are what the 4096-byte bound is charged, so escaping cannot widen
it.

Pinned by `crates/holon-mcp-client/src/child_transport.rs::tests` (the refusal names the
bound; 100 MiB in finished 1 MiB lines still streams;
`a_flooding_sidecars_stderr_is_bounded_and_what_it_cost_is_disclosed` drives a real sidecar
that writes escape sequences, one 8 MiB line and 4 MiB of flood),
`crates/holon-mcp-client/src/peer_budget.rs::tests` (control characters escaped and named),
`crates/holon-mcp-client/tests/inbound_bounds.rs::one_stdio_message_past_the_byte_allowance_ends_the_connection`
and `crates/holon-app/src/mcp_integrations.rs::tests::a_bound_that_ends_a_sidecars_leg_marks_the_integration_unavailable`
(a real sidecar floods one unterminated line past the allowance; the condition names the bound
and the mirror reads `Unavailable`).
Teeth: removing the charge turns both the unit test and the end-to-end test red
(`lane-logs/jaq-r7-teeth-stdio.log`), with the file restored byte-for-byte
(`lane-logs/jaq-r7-sha-before.log` / `-sha-after.log`).
