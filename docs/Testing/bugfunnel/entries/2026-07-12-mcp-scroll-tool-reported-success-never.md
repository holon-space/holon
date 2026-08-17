---
id: 2026-07-12-mcp-scroll-tool-reported-success-never
date: 2026-07-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  MCP scroll tool reported success but never moved the list — synthetic
  off-cursor ScrollWheel no-ops gpui should_handle_scroll hover-gate;
  unhandled result swallowed
source_line: 965
---

## Bug

MCP scroll tool reported success but never moved the list — synthetic
off-cursor ScrollWheel no-ops gpui should_handle_scroll hover-gate;
unhandled result swallowed

## Missing piece

MCP scroll dispatched a synthetic wheel + ignored handled=false; fix =
direct ListState::scroll_by + fail-loud

## Remedy

FIXED
