---
id: 2026-07-20-mcp-dogfood-drive-layer-gaps-three
date: 2026-07-20
gap: ENVIRONMENT
secondary: null
status: UNCLASSIFIED
summary: >-
  MCP dogfood DRIVE-LAYER gaps (three, blocking page-nav/link/breadcrumb
  dogfooding): (1) `click{entity_id, region:"left_sidebar"}` fails `element
  bounds never committed; stale focus cleared` for sidebar tree rows — sidebar
  rows never register bounds, so sidebar-click navigation is undrivable; (2)
  `click{x,y, region:"main"}` coordinate-click reports success but does NOT
  focus editors — subsequent `type_text` drops all keystrokes ("no focused
  editor"); (3) `send_navigation`/`execute_command navigation.focus` do not
  repaint the main panel (`focus_roots` never updated).
source_line: 1029
---

## Bug

MCP dogfood DRIVE-LAYER gaps (three, blocking page-nav/link/breadcrumb
dogfooding): (1) `click{entity_id, region:"left_sidebar"}` fails `element
bounds never committed; stale focus cleared` for sidebar tree rows — sidebar
rows never register bounds, so sidebar-click navigation is undrivable; (2)
`click{x,y, region:"main"}` coordinate-click reports success but does NOT
focus editors — subsequent `type_text` drops all keystrokes ("no focused
editor"); (3) `send_navigation`/`execute_command navigation.focus` do not
repaint the main panel (`focus_roots` never updated).

## Missing piece

the MCP/dogfood drive channel cannot exercise sidebar-click,
coordinate-click focus, or programmatic navigation repaint — so page↔page
navigation, wiki-link click-through, and breadcrumb/back-nav flows are not
drivable over MCP; the keystone's live-MCP twin has no rung for these.
Fixing sidebar-row bounds-commit would reopen the dogfood navigation
channel.

## Remedy

(1) FIXED 2026-07-20 — SAME root cause as row 230: the sidebar tree list
collapsed to 0 height inside the stacking `column`, so the virtualized
`gpui::list` prepainted no rows and committed no per-row bounds; MCP
`click{entity_id, region:"left_sidebar"}`'s retry-until-committed therefore
always timed out. The `column.rs` content-height fix (row 230) makes the
page rows actually paint, so `selectable`'s `tracked(...)` records each page
row's `entity_id` bounds (the sidebar rows were already wrapped in
`tracked()` via `selectable` + the `navigation_focus` click op — no separate
bounds-commit change was needed). LIVE-CONFIRMED: MCP
`click{entity_id:"block:journals", region:"left_sidebar"}` now returns
`{"clicked_entity":"block:journals"}` and navigates the main panel (was
"element bounds never committed"). Same detection test as row 230 asserts
the committed bounds. (2) coord-click-focus and (3) send_navigation-repaint
remain OPEN (out of scope, separate rows).
