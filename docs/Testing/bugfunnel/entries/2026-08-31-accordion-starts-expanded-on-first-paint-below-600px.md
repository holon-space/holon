---
id: 2026-08-31-accordion-starts-expanded-on-first-paint-below-600px
date: 2026-08-31
gap: ENVIRONMENT
secondary: PERCEPTION
status: OPEN
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

`shadow_builders/accordion.rs:120-125` derives the default from measured space:

```rust
let narrow = ba.ctx.available_space
    .is_some_and(|s| s.width_px < ACCORDION_MIN_EXPANDED_WIDTH_PX);
let collapsed = ba.args.get_bool("collapsed").unwrap_or(narrow);
```

`available_space` comes from `BuilderServices::viewport_snapshot()`
(`crates/holon-frontend/src/reactive.rs:4156`), which maps
`ui_state.viewport()` — `None` until the window publishes its first viewport.
The comment at accordion.rs:121 states the intent plainly: "Unmeasured
available space is desktop-first … start expanded."

The root layout is re-interpreted once the viewport is known — the narrow
branch of `if_space(600, bottom_dock(…), …)`
(`crates/holon-api/src/perspective.rs:325`) IS active in the first
`describe_ui`, and the sidebars are already `mode:"overlay"`. The main panel's
own render source (`block:default-main-panel::render::0`), which is where the
accordion lives, is interpreted on the `live_block` path and evidently keeps
the pre-viewport node until something forces a rebuild.

## Missing piece

`frontends/gpui/tests/accordion_sizes_to_content_windowed.rs` builds its
`RenderContext` with `available_space: Some(space)` already populated
(line 156) before interpreting. R4 (`a_phone_width_panel_starts_collapsed`)
therefore tests the steady state and can never observe the first frame, when
`available_space` is still `None`. There is no assertion anywhere on the
accordion's state in the FIRST painted frame after boot.

## Remedy

Open. The gap closes with a windowed assertion on frame one — interpret with
`available_space: None`, publish the phone viewport, and require the accordion
to be collapsed before any navigation occurs.
