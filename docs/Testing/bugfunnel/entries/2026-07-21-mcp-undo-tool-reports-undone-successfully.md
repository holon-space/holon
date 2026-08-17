---
id: 2026-07-21-mcp-undo-tool-reports-undone-successfully
date: 2026-07-21
gap: ORACLE
secondary: PERCEPTION
status: OPEN
summary: >-
  MCP undo tool reports "undone successfully" when the agent-origin op never
  entered the user undo stack — misleading success on a no-op (agent-surface
  fail-loud gap from the N6/B2 analysis).
source_line: 1059
---

## Bug

MCP undo tool reports "undone successfully" when the agent-origin op never
entered the user undo stack — misleading success on a no-op (agent-surface
fail-loud gap from the N6/B2 analysis).

## Missing piece

agent undo surface must report the no-op honestly

## Remedy

OPEN (minor; W7 N6 + dogfood B2)
