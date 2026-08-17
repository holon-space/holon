---
id: 2026-07-20-mcp-dogfood-tooling-gap-main-panel
date: 2026-07-20
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  MCP dogfood-tooling gap: `click_entity` on main-panel blocks intermittently
  then persistently fails with "element bounds never committed within 5s;
  stale focus cleared to prevent silent mis-targeted typing", wedging
  agent-driven editing — once wedged, no in-page click (nor
  escape/double-click) re-focuses any block, and `describe_navigation`'s Focus
  Path is stale/unreliable (kept naming an old block while typing actually
  landed elsewhere). Main-panel block bounds are not committed to the
  BoundsRegistry (echoes the sidebar 0-height bounds-commit class fixed in
  651f1c8, but here blocks are visibly rendered). A real mouse bypasses
  BoundsRegistry, so this is primarily a harness/agent-dogfood limitation; the
  fail-loud focus-clear is correct behavior.
source_line: 1040
---

## Bug

MCP dogfood-tooling gap: `click_entity` on main-panel blocks intermittently
then persistently fails with "element bounds never committed within 5s;
stale focus cleared to prevent silent mis-targeted typing", wedging
agent-driven editing — once wedged, no in-page click (nor
escape/double-click) re-focuses any block, and `describe_navigation`'s Focus
Path is stale/unreliable (kept naming an old block while typing actually
landed elsewhere). Main-panel block bounds are not committed to the
BoundsRegistry (echoes the sidebar 0-height bounds-commit class fixed in
651f1c8, but here blocks are visibly rendered). A real mouse bypasses
BoundsRegistry, so this is primarily a harness/agent-dogfood limitation; the
fail-loud focus-clear is correct behavior.

## Missing piece

Main-panel `LiveBlock`/tree-item elements must commit bounds to the registry
so MCP `click_entity` can target them (and the keystone's own click driver
stays reliable); also make `describe_navigation` Focus Path reflect real
editor focus. Blocks agent exploratory coverage of main-panel editing.

## Remedy

OPEN
