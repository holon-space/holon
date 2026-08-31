---
id: 2026-08-31-accordion-paints-one-of-four-backlink-rows
date: 2026-08-31
gap: ENVIRONMENT
secondary: PERCEPTION
status: OPEN
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

Not root-caused from this channel — two mechanisms are consistent with the
evidence and the fix must discriminate between them:

1. A height clamp: `accordion::render_bounded`
   (`frontends/gpui/src/render/builders/accordion.rs:70-105`) gives the body
   `flex_1().min_h_0().overflow_y_scroll()` inside a `max_h(relative(fraction))`
   region. If the region resolves against an indefinite height the body settles
   at roughly one row and clips.
2. The body's collection never streams past its first row — the same class as
   `2026-08-31-root-layout-collections-never-stream`, where a collection in the
   root-layout tree had its reactive driver never started.

This is the mirror of `2026-08-30-accordion-fixed-at-cap-not-content-sized`
(the region was pinned AT the cap regardless of content); the shrink-to-content
branch introduced there now under-sizes instead.

`describe_ui` cannot arbitrate: it reports the whole accordion subtree as
`{"widget":"empty"}` — see
`2026-08-31-describe-ui-erases-accordion-subtree`.

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

Open. Closing the gap means driving the assertion through the production
`live_block` mount — the same window the dogfood pass used — rather than a
hand-built panel VM, and asserting the painted ROW COUNT against the query's
row count, not only `region_h`.
