---
id: 2026-08-31-accordion-paints-one-of-four-backlink-rows
date: 2026-08-31
gap: ENVIRONMENT
secondary: PERCEPTION
status: FIXED
summary: >-
  The expanded "Linked references" accordion paints one backlink row while its
  own query returns four, and never grows or scrolls to reveal the rest.
---

## Bug

Found by the `dogfood-explorer` gate driving a real GPUI window over the
embedded MCP server (lane `dogfood-mobile`, sandbox
`/tmp/dogfood-mobile-sandbox`, port 8720), while dogfooding the mobile-usability
wave that landed 2026-08-31.

Four blocks linking to the focused page were created, so the accordion's own
query returns four rows:

```
SELECT bl.id, bl.content FROM backlinks bl
  JOIN focus_roots fr ON bl.target_id = fr.root_id
  JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = fr.history_id
  WHERE fr.region = 'main' ORDER BY bl.content ASC
-> rows[4]
```

The expanded accordion paints exactly ONE of them.

- At 1200x850 (`shots/08-wide.png`): header at y≈798, a single backlink row at
  y≈824, window bottom at 850. The region is ~52px — header plus one row.
- At 560x850 (`shots/07-four-backlinks.png`): the same region shows the row's
  leading bullet glyph and no row text at all.
- A wheel scroll inside the body (`scroll{x:300,y:830,dy:3}`) changes nothing,
  so the missing rows are not merely scrolled out of a clipped viewport.

The region does not grow toward its `max_height_fraction: 0.33` cap (≈280px,
room for all four rows), and it does not shrink-wrap four rows either.

## Root cause

Neither candidate above. Measured against a real booted window
(`frontends/gpui/tests/accordion_real_mount_windowed.rs`): with four blocks
linking to the focused page, the section's own SQL returns all four straight
from the store, while the painted region holds none of them.

```
SELECT bl.id FROM backlinks bl JOIN focus_roots fr ... WHERE fr.region = 'main'
-> block:accordion-ref-1 .. block:accordion-ref-4
```

The section's `item_template` was a per-ROW template with no collection around
it:

```
live_query(#{sql: "...", item_template: selectable(row(icon("orgmode"), ...))})
```

A `live_query`'s `item_template` is interpreted as the WHOLE tree the shell
renders (`ReactiveEngine::watch_query_live`), so a bare row template binds the
first data row and nothing else — one row when the snapshot has rows, none when
it does not, and no per-row diffs afterwards because a non-collection tree has
no `ReactiveView` to stream into. The default template is `table()` and the left
sidebar wraps its own rows in `list(#{item_template: ...})`; this one section
did neither.

The height is a consequence, not a cause: the region shrink-wraps what it is
given, and it was given one row.

## Missing piece

`frontends/gpui/tests/accordion_sizes_to_content_windowed.rs` measures
`region_h` on a panel it interprets ITSELF: `interpret_panel` builds a
`RenderContext` by hand and calls `interp.interpret(&panel_expr, …)` directly
(lines 137-166). Production reaches the same accordion through
`live_block(block:default-main-panel)` → the flow-panel split in
`column::render_accordion_split`. R1/R2/R3 therefore assert content-sizing on a
tree that was never mounted the way production mounts it, so a clamp or a
non-streaming collection introduced at the mount seam cannot make them red.

## Remedy

FIXED. `assets/default/index.org` wraps the section's row template in a
collection (`item_template: list(#{item_template: selectable(...)})`), the form
the left sidebar already uses, so the shell renders a streaming collection and
every query row reaches the screen.

Locked by `every_backlink_the_query_returns_is_painted`
(`frontends/gpui/tests/accordion_real_mount_windowed.rs`), which boots a real
window over a `TestEnvironment`, seeds four linking blocks BEFORE the window
opens, and asserts the painted row count equals the query's row count — plus
that the region grows to hold them and stays under `max_height_fraction`.

Open point: a bare per-row `item_template` still degrades silently for the next
author. Making that loud needs the interpreter to classify which widgets produce
collections, which is a wider design call than this fix.
