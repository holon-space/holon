---
id: 2026-08-31-accordion-starts-expanded-on-first-paint-below-600px
date: 2026-08-31
gap: ENVIRONMENT
secondary: PERCEPTION
status: FIXED
summary: >-
  Below 600px the "Linked references" accordion paints EXPANDED on the first
  frame after boot; the default-collapsed rule only takes effect once a
  navigation re-interprets the tree.
---

## Bug

Found by the `dogfood-explorer` gate driving a real GPUI window at
`HOLON_INITIAL_WINDOW_SIZE=560x850` over the embedded MCP server (lane
`dogfood-mobile`, port 8720).

Same window, same width, two moments:

- First paint after boot, window frontmost (`shots/00-baseline.png`): the
  accordion header reads `▾ link Linked references` — the `▾` glyph is the
  EXPANDED state, and a body row is painted below it.
- After one `navigation.focus` round trip (`shots/05-backlink.png`): the same
  header reads `▸ link Linked references` — collapsed, title row only, which is
  the intended behaviour below 600px.

Nothing about the window changed between the two; only a re-interpretation of
the tree happened. 560 < `ACCORDION_MIN_EXPANDED_WIDTH_PX` (600), so the first
frame is wrong and the later frame is right.

## Root cause

`shadow_builders/accordion.rs` derives the default from measured space:

```rust
let narrow = ba.ctx.available_space
    .is_some_and(|s| s.width_px < ACCORDION_MIN_EXPANDED_WIDTH_PX);
let collapsed = ba.args.get_bool("collapsed").unwrap_or(narrow);
```

`available_space` never arrived on this path at all — not merely at boot.
`ReactiveEngine::watch_live` and `watch_query_live` build their `RenderContext`
with `..Default::default()`, which leaves `available_space` at `None`, while
`snapshot_resolved` fills it from `viewport_snapshot()`. Everything interpreted
through a `live_block` — the main panel, and so the accordion — therefore took
the unmeasured desktop-first branch at every width, on every frame.

## Missing piece

`frontends/gpui/tests/accordion_sizes_to_content_windowed.rs` builds its
`RenderContext` with `available_space: Some(space)` already populated
(line 156) before interpreting. R4 (`a_phone_width_panel_starts_collapsed`)
therefore tests the steady state and can never observe the first frame, when
`available_space` is still `None`. There is no assertion anywhere on the
accordion's state in the FIRST painted frame after boot.

## Remedy

FIXED. Both live-block interpret paths (`ReactiveEngine::watch_live` and
`watch_query_live`, initial tree and structural re-interpretation alike) now
take `available_space` from `services.viewport_snapshot()`, the same seam
`snapshot_resolved` uses. The structural signal already re-fires on
`viewport_generation`, so a resize re-interprets against the new space and
`with_update` keeps the reader's own expand.

Locked by `a_narrow_first_frame_starts_collapsed`
(`frontends/gpui/tests/accordion_real_mount_windowed.rs`): a real window booted
at `HOLON_INITIAL_WINDOW_SIZE=560x850` with backlinks already in the store must
paint no section rows on the first settled frame, with no navigation performed
against the window.
