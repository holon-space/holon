---
id: 2026-09-08-badges-and-the-retry-button-are-invisible-to-the-mcp-driver
date: 2026-09-08
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The MCP driver cannot see a badge's bounds and cannot address the deferred
  reimport banner's Retry button — describe_ui's geometry section lists no badge
  rows, and click rejects the button's id because it is not an EntityUri.
---

## Bug

Found by the `dogfood-explorer` gate for D93.a / D94.a while trying to judge
those two features through the driver the gate is supposed to use.

**Badges have no geometry.** `describe_ui {"block_id":"block:default-main-panel"}`
prints `badge "kept from this device before pairing"` in the render tree for both
seeded conflict copies, and its geometry section
(`lane-logs/dogfood-w10/geom-main.txt`, 42 rows) contains no `badge` row for
either — while listing `tree_item`, `column`, `selectable`, `draggable`,
`state_toggle`, `rendered_text` and `drop_zone` for the same blocks. A badge
that is on screen and a badge that has been squeezed out of the row are
therefore indistinguishable to the driver; the second case had to be
established from a screenshot and from the neighbouring `rendered_text` width
(see `2026-09-08-long-conflict-copy-paints-no-badge-at-all`).

**The Retry button cannot be clicked by id.** D94.a exposes it as the tracked id
`deferred-reimport-retry` (`frontends/gpui/src/share_ui.rs:1349`), and:

    click {"entity_id":"deferred-reimport-retry"}
    -32602: entity_id is not a valid EntityUri:
            Invalid URI "deferred-reimport-retry": unexpected character at index 23

A coordinate click on the button's painted position returned
`{"clicked":[1426.0,46.0],"button":"left","handled":false}`. The retry could
only be exercised through `execute_operation device.pair_retry_reimport`, i.e.
around the button rather than through it, so the button's own wiring is
unverified by this pass.

## Root cause

`click` parses `entity_id` as an `EntityUri` before hit-testing, so the whole
class of overlay controls — which are bounds-registry ids, not entity uris —
is unaddressable. The geometry section of `describe_ui`
(`frontends/mcp/src/tools.rs:3366`) reports the tracked nodes it recognises per
block; badge nodes are tracked (`frontends/gpui/src/render/builders/badge.rs`
wraps itself in `geometry::tracked`) but do not reach that output.

The windowed GPUI tests read `BoundsRegistry` directly, which is why both
features' own tests are green on surfaces the MCP driver cannot reach.

## Missing piece

The dogfood channel is the declared final gate, and its two observation
primitives — `describe_ui` geometry and `click` — do not cover overlay chrome
or badge bounds. Anything a feature discloses through those surfaces is
gate-invisible.

## Remedy

Open. Emit badge rows in `describe_ui`'s geometry section, and let `click`
accept a bounds-registry id (a non-EntityUri string) so overlay controls are
drivable. Until then, overlay-only affordances need a screenshot step recorded
in the lane report rather than a driver assertion.
