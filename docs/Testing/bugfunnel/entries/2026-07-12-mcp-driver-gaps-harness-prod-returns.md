---
id: 2026-07-12-mcp-driver-gaps-harness-prod-returns
date: 2026-07-12
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  MCP driver gaps (harness, not prod): (a) `send_key_chord ["cmd","z"]`
  returns "No handler matched" though the binding exists
  (undo_redo_keybinding.rs) — chord router doesn't reach the window-level
  keymap, so real-keyboard undo is untestable over MCP; (b) `click{entity_id,
  region:"main"}` hit the SIDEBAR copy of the same entity id (coords 155,93) —
  region param doesn't constrain the hit-test
source_line: 906
---

## Bug

MCP driver gaps (harness, not prod): (a) `send_key_chord ["cmd","z"]`
returns "No handler matched" though the binding exists
(undo_redo_keybinding.rs) — chord router doesn't reach the window-level
keymap, so real-keyboard undo is untestable over MCP; (b) `click{entity_id,
region:"main"}` hit the SIDEBAR copy of the same entity id (coords 155,93) —
region param doesn't constrain the hit-test

## Missing piece

McpUserDriver chord path ≠ prod key dispatch; ambiguous-entity hit-test
ignores region

## Remedy

OPEN — blocks dogfooding of every keybinding-only surface
