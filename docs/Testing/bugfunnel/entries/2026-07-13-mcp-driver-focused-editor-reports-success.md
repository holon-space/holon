---
id: 2026-07-13-mcp-driver-focused-editor-reports-success
date: 2026-07-13
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  MCP driver: `type_text` with NO focused editor reports success
  (`keystrokes_sent:N`) while dropping every keystroke (after a failed click
  cleared focus, 22 keystrokes vanished silently — masked further by the CLI's
  stderr-only error reporting); `click` correctly fails loud, `type_text` must
  too
source_line: 977
---

## Bug

MCP driver: `type_text` with NO focused editor reports success
(`keystrokes_sent:N`) while dropping every keystroke (after a failed click
cleared focus, 22 keystrokes vanished silently — masked further by the CLI's
stderr-only error reporting); `click` correctly fails loud, `type_text` must
too

## Missing piece

MCP driver has no fail-loud contract on type_text without focus target

## Remedy

OPEN — dogfood #5; fix = error (or `dropped:true`) when no editor focused
