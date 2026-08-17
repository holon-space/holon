---
id: 2026-08-12-linked-references-accordion-has-representation-backlink
date: 2026-08-12
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The "Linked references" accordion has no representation in `describe_ui`, so
  backlink rendering can be neither observed nor driven over MCP.
source_line: 726
---

## Bug

(Compass dogfood lane; found by driving the live GPUI app over MCP; no
automated test produced it) **The "Linked references" accordion has no
representation in `describe_ui`, so backlink rendering can be neither
observed nor driven over MCP.** Its own backing SQL returns 1 row for the
focused page, while the accordion appears in neither the widget tree (panel
ends at `divider` + `empty`) nor the geometry dump (last line is the
creation slot), and a coordinate click on the painted header returns
`handled:false`. Painted but unobservable. A second, UNPROVEN suspicion
rides on this and is not counted separately: the section showed no entries
with 1 row in its query, but "empty" could not be distinguished from
"collapsed" over MCP.

## Root cause

Compass dogfood lane, found by DRIVING the live GPUI app over MCP — no
automated test produced it: **the "Linked references" accordion has no
representation in `describe_ui`, so backlink rendering can be neither
observed nor driven over MCP.** After `convert_block_to_page` on a Compass
mission item the app's own accordion SQL (`SELECT bl.* FROM backlinks bl
JOIN focus_roots fr ON bl.target_id = fr.root_id JOIN navigation_cursor nc
ON … WHERE fr.region = 'main'`, copied verbatim out of
`default-main-panel::render::0`) returns exactly 1 row for the focused page,
while the accordion is absent from BOTH `describe_ui` surfaces: it never
appears as a widget in the tree dump (the main panel ends at `divider` +
`empty`) and it carries no geometry line in the text dump, whose last entry
is the creation slot at y=196. A coordinate `click` on the painted header
returned `handled:false` — no hit target. So the accordion IS painted
(screenshot `04-linked-refs-empty.png` shows its title) but is invisible to
every MCP observation and actuation primitive. ENVIRONMENT primary: the
widget is outside the described tree, so no MCP-driven check can reach it —
the same class as the 2026-07-12 "MCP scroll tool reported success but never
moved the list" row. NOTE — a SECOND, unproven suspicion rides on this and
is NOT counted separately: with 1 row in its backing query the section
rendered no visible entries, but because the accordion cannot be expanded or
enumerated over MCP this lane could not distinguish "empty" from
"collapsed", and reports it as a suspect only. Missing piece: accordion
contents in the `describe_ui` projection (widget + geometry + rows) so the
backlink surface becomes assertable at all. REPORTED — not fixed by this
lane.)

## Missing piece

accordion contents (widget + geometry + rows) absent from the `describe_ui`
projection, so the backlink surface is unassertable

## Remedy

REPORTED — routed to the orchestrator, not fixed by this lane
