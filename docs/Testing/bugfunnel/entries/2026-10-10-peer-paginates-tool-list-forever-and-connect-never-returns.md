---
id: 2026-10-10-peer-paginates-tool-list-forever-and-connect-never-returns
date: 2026-10-10
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An MCP peer that answers every `tools/list` page with one more `nextCursor` made the
  connect enumerate forever — 457 MiB RSS in 20 s, still climbing, and the integration never
  reached a state the user could act on.
---

## Bug
Found by the security verifier of the MCP peer hardening lane `jaq-harden`
(`lane-logs/jaq-verify4.md`, Round 4, R4-D1) — a code audit plus a hostile mock peer, not a
test in the suite. A peer that answers `initialize`, then answers every `tools/list` with 50
tools (4 KiB of description each) and always a `nextCursor`, makes
`McpOperationProvider::from_peer_shared` never return. Measured over an in-memory duplex
(debug): 1913 pages / ~95 650 tools and 457 MiB RSS after 20 s and still climbing; a second
run reached 4027 pages / ~786 MiB of description text in 20 s. The per-request deadline stays
satisfied the whole time, because each individual page arrives promptly.

Reproduced through the production entry point at
`crates/holon-mcp-client/tests/peer_budget_bounds.rs` (log
`lane-logs/jaq-r5-red-pagination.log`): `McpOperationProvider::connect` was still enumerating
after 20 s. At boot the same peer left its integration reading `Connecting` forever
(`lane-logs/jaq-r5-teeth-boot.log`).

## Root cause
`crates/holon-mcp-client/src/mcp_request.rs` `all_pages` was `loop { page(cursor).await? }`
with no page cap, no item cap and no total deadline — only [`REQUEST_TIMEOUT`] per request.
Both callers run on every connect: `list_all_tools` from `mcp_provider.rs`
(`McpOperationProvider::from_peer_shared`) and `list_all_resource_templates` from
`mcp_integration.rs` (`finish_integration`). Rounds 1-4 of the lane had each bounded one
neighbouring path (imports, request deadlines, `fromjson` depth/digits, POST reply streams),
so the enumeration loop was the next unbounded resource beside them.

## Missing piece
No test drove a connect against a peer that keeps a legitimate-looking answer coming, so
nothing could generate the interaction: every mock peer in the suite answers one complete
page with `next_cursor: None` (`fake_mcp_module.rs:69`, `pbt_mcp_fake.rs:71`,
`frontends/mcp/src/resources.rs:58`), and Holon's own server lists its 50 tools unpaginated.
Pagination support existed only for third-party peers, which no test played. Secondary
ENVIRONMENT: the growth happens in a background connect task at boot, a wiring the headless
keystone does not stand up.

## Remedy
One per-connection `PeerBudget` (`crates/holon-mcp-client/src/peer_budget.rs`) now carries
every bound, and `BudgetedPeer` (`crates/holon-mcp-client/src/mcp_request.rs`) keeps the rmcp
peer in a private field so no call path can reach a peer without it. Enumeration is bounded by
`MAX_LIST_PAGES` (256), `MAX_LIST_ITEMS` (4096) and `CONNECT_BUDGET` (300 s, covering the
handshake and every page); each refusal names the bound it hit. A trip degrades only that
integration — it is disclosed as `IntegrationConnectFailed` and Holon keeps booting (D-jaq-bound.a,
plus the 10-08 boot-always directive).

Pinned by `crates/holon-mcp-client/tests/peer_budget_bounds.rs` (page bound, item bound) and
`crates/holon-integration-tests/tests/frontend_suite/integration_connect_stall_blocks_boot.rs::a_peer_that_paginates_forever_degrades_only_its_own_integration`
(boot resolves, the integration reaches `Unavailable`). Teeth proven by inverting each bound:
`lane-logs/jaq-r5-teeth-pagination.log`, `lane-logs/jaq-r5-teeth-boot.log`.

The sibling enumeration on the same loop, `list_all_resource_templates`, had its failure
swallowed into a `warn!` plus an empty template list; it now carries the reason out on
`McpIntegration::discovery_incomplete` and the app discloses
`IntegrationDiscoveryIncomplete`, so an unreadable template list no longer reads as a peer
that publishes none.
