---
id: 2026-07-21-via-live-mcp-block-returns-executed
date: 2026-07-21
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Via live MCP, `execute_operation` block-`create` returns "executed
  successfully" yet writes NO row to `block`/`block_raw`, while
  `insert_text`/`delete`/`add_tag` on the same surface DO persist — success
  reported without any write (fail-loud violation). Possibly the MCP
  `execute_operation` create wiring only.
source_line: 1077
---

## Bug

Via live MCP, `execute_operation` block-`create` returns "executed
successfully" yet writes NO row to `block`/`block_raw`, while
`insert_text`/`delete`/`add_tag` on the same surface DO persist — success
reported without any write (fail-loud violation). Possibly the MCP
`execute_operation` create wiring only.

## Missing piece

The keystone drives creates through its drivers, where
`inv-blocks-match-ref/{block_raw}` WOULD go red if a create didn't persist
(the invariant exists) — NOT through the MCP `execute_operation` create
path, so the failing wiring is absent from the test env (same MCP
drive-layer class as the `click_entity`/`send_key_chord`/screenshot rows).
Secondary ORACLE/fail-loud: the MCP surface reports success without checking
its own postcondition. Remedy: make the MCP `execute_operation` create
commit (or fail loud) + a live-MCP rung asserting a created block appears in
block/block_raw.

## Remedy

OPEN — 2026-07-21 live-MCP; unconfirmed whether the underlying op persists
off the driver path (possibly MCP-wiring-only).
