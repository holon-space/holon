---
id: 2026-10-10-spent-connect-budget-panics-instead-of-refusing
date: 2026-10-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A peer that spent the whole CONNECT_BUDGET on the connect's first enumeration made the
  second one hit an `assert!` — a panic where every other bound of the same budget is a
  disclosed refusal.
---

## Bug
Found by the security verifier of the MCP peer hardening lane `jaq-harden`
(`lane-logs/jaq-verify5.md`, D3) — a code audit, reproduced mechanically through the public
production entry `McpOperationProvider::from_peer_shared` with a spent budget. The panic:
"[mcp_request] list_tools started after this connection's CONNECT_BUDGET (300s) was already
spent" (`lane-logs/jaq-r7-red-all.log`).

## Root cause
`crates/holon-mcp-client/src/mcp_request.rs:171` asserted that a connect's enumeration never
starts past the budget, on the premise that "a budget already spent means this is being called
from somewhere else". The premise is false: `finish_integration` runs TWO enumerations against
ONE budget — `list_all_resource_templates` (`mcp_integration.rs`), then cache/DDL/view work,
then `from_peer_shared` → `list_all_tools` (`mcp_provider.rs`) — and the PEER owns the elapsed
time of the first one. It can also widen the gap by declaring many templates, since each
becomes a cache table and a DDL. A state a peer can drive the process into is not an
invariant.

Softening (why it was a wrong shape, not a boot stop): the connect task runs under
`shutdown.spawn_disclosing_panic` (`crates/holon-app/src/mcp_integrations.rs`), so boot
survived and the panic was disclosed.

## Missing piece
`peer_budget_bounds.rs` covered each bound being HIT during one enumeration, never a SECOND
enumeration starting after the budget was gone — the suite had no test that ran two
enumerations against one budget, which is what every production connect does.

## Remedy
The assertion is a disclosed `ServiceError::Cancelled` naming CONNECT_BUDGET, like every other
refusal on this budget, so the connect fails loud and the integration is disclosed as
`IntegrationConnectFailed` instead of unwinding.

Pinned by
`crates/holon-mcp-client/tests/peer_budget_bounds.rs::an_enumeration_starting_past_the_budget_is_refused_naming_the_bound`
(connects over a mock peer, spends the budget, then runs the second enumeration through
`from_peer_shared`). Teeth: restoring the `assert!` turns it red
(`lane-logs/jaq-r7-teeth-assert.log`), with the file restored byte-for-byte
(`lane-logs/jaq-r7-sha-before.log` / `-sha-after.log`).
