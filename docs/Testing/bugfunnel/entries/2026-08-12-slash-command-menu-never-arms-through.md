---
id: 2026-08-12-slash-command-menu-never-arms-through
date: 2026-08-12
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The slash-command menu never arms through the `type_text` MCP driver, so the
  Compass template picker is unreachable and unrecordable.
source_line: 725
---

## Bug

(Compass dogfood lane; found by driving the live GPUI app over MCP; no
automated test produced it) **The slash-command menu never arms through the
`type_text` MCP driver, so the Compass template picker is unreachable and
unrecordable.** `type_text "/"` stores content `/` with no popup in
`describe_ui` (342-widget root dump) and none in a re-fronted,
md5-verified-live screenshot; the next typed char leaves `goal`, not
`/goal`. All keystrokes report `dropped:0`, so this is not the 2026-07-13
lost-keystroke row — the keys land, the `command_menu` trigger never fires.
Templates had to be instantiated via `execute_operation
instantiate_template`.

## Root cause

Compass dogfood lane, found by DRIVING the live GPUI app over MCP — no
automated test produced it: **the slash-command menu never arms through the
`type_text` MCP driver, so the template picker — the sanctioned entry path
for every Compass item — is unreachable and unrecordable.** Sequence: click
a block, `type_text {"text":"enter"}` (new sibling created, confirmed in
SQL), `type_text {"text":"/"}` → the block's stored content becomes `/`, and
NO popup appears in `describe_ui` (whole `block:root-layout` tree, 342
widgets) nor in a re-fronted screenshot whose md5 matched the previous
frame, proving the frame was live and not the stale-cache artifact §2 warns
about. Typing the filter next (`type_text {"text":"goal"}`) left content
`goal`, NOT `/goal` — the `/` was consumed without arming anything. Every
keystroke reported `keystrokes_handled:N, dropped:0`, so this is NOT the
2026-07-13 "no focused editor drops keystrokes" row: the keystrokes land,
the `command_menu` trigger check does not fire. Consequence for this lane:
all seven Compass templates had to be instantiated through
`execute_operation instantiate_template` instead, which is not the path a
user takes. ENVIRONMENT primary: the trigger check lives on the frontend's
editable-surface input path, which the MCP keystroke rung does not traverse
— the headless `TriggerSlashCommand` transition drives the menu through a
different seam entirely, so the composed keystone exercises a menu the live
driver cannot reach. Missing piece: an MCP rung that routes `type_text`
through the same trigger check the real editor uses, so the picker is
drivable — and, once drivable, a step that picks a NAMED menu entry (today
`I trigger the slash command on block {block_id}` hard-codes selecting
"delete"). REPORTED — not fixed by this lane.)

## Missing piece

no MCP rung routes `type_text` through the editor's trigger check, and no
step picks a NAMED menu entry (`I trigger the slash command on block {id}`
hard-codes "delete")

## Remedy

REPORTED — routed to the orchestrator, not fixed by this lane
