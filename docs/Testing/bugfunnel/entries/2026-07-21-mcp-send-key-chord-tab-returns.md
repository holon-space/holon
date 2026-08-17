---
id: 2026-07-21-mcp-send-key-chord-tab-returns
date: 2026-07-21
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  MCP send_key_chord ["tab"] returns "No handler matched" (no indent); only
  type_text "tab" indents — MCP chord-dispatch diverges from the real
  keystroke path (real Tab works).
source_line: 1061
---

## Bug

MCP send_key_chord ["tab"] returns "No handler matched" (no indent); only
type_text "tab" indents — MCP chord-dispatch diverges from the real
keystroke path (real Tab works).

## Missing piece

none (MCP driver-surface artifact)

## Remedy

OPEN (minor; dogfood-round3 B4)
