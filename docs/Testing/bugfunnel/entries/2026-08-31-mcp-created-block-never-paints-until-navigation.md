---
id: 2026-08-31-mcp-created-block-never-paints-until-navigation
date: 2026-08-31
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A block created through `execute_operation block.create` reaches SQL and the
  view model but is never painted, so it stays unclickable until an unrelated
  navigation forces a repaint.
---

## Bug

Found by the `dogfood-explorer` gate driving a real GPUI window over the
embedded MCP server (lane `dogfood-mobile`, port 8720).

```
execute_operation {"entity_name":"block","operation":"create",
  "params":{"id":"block:dogfood-link-1",
            "parent_id":"block:3a2dbaf6-…","content":"child under journal day"}}
-> Operation 'create' on entity 'block' executed successfully
```

The write landed: `SELECT id, content FROM block WHERE id='block:dogfood-link-1'`
returns the row. After ~5s the node is also in the view model —
`grep -c dogfood-link-1` over `describe_ui` returns 8 hits. But it was never
painted, and stayed unpainted across two attempts several seconds apart:

```
click {"entity_id":"block:dogfood-link-1","region":"main"}
-> click_entity failed: element bounds never committed;
   stale focus cleared to prevent silent mis-targeted typing
```

A following `type_text` correctly refused rather than typing into nothing:
`type_text dropped all 19 keystroke(s): no focused editor … consumed them`.

One `navigation.focus` away and back to the same page fixed it — the identical
click then succeeded, and the geometry index went 14 → 37 recorded elements.
So the window paints fine; the create simply never invalidated it.

Both refusals are exemplary fail-loud behaviour and are NOT the bug. The bug is
that the repaint is never scheduled.

## Root cause

`GpuiUserDriver::resolve_click_center_until`
(frontends/gpui/src/user_driver.rs:607) — the bounds wait every
entity-addressed click runs first — polled PASSIVELY: it re-read
`require_click_center` and slept on `geometry.changed()`, waiting for a frame
it never asked anyone to draw.

Bounds are render-derived, and the frame that would carry the new row was
nobody's job. A gesture is itself a platform input, so the interaction pump
draws on the way out and the row is committed before the wait even starts. An
MCP `execute_operation` dispatches no platform event at all, and a window that
is not the frontmost application paints only when driven — so the wait burned
its full 5s `CLICK_BOUNDS_TIMEOUT` on a row that had been ready to paint the
whole time. The following `navigation.focus` went through the input router,
which drew, and the identical click then succeeded.

The same hazard was already fixed one stage LATER in the same method chain:
`await_editor_window_focus` drives `InteractionEvent::ForceFrame` before every
read for exactly this reason (dogfood 2026-08-07, DRIVER PARITY). The bounds
resolution ahead of it was never given the same treatment.

## Missing piece

Driver parity, and a windowed rung that can see a frame NOT being drawn. The
composed keystone drives `SimUserDriver`, which pumps the whole gpui app on
every read, and gpui's TestPlatform draws every dirty window inside
`flush_effects` — so in the test environment a repaint always happens
ambiently and this class of bug is structurally invisible. Nothing exercised
`GpuiUserDriver`, the driver the MCP tools actually inject into.

## Remedy

FIXED. `resolve_click_center_until` now dispatches
`InteractionEvent::ForceFrame` once a read has MISSED, so the wait paces on
frames it draws rather than frames it hopes for — the contract
`await_editor_window_focus` already held. The first read still costs no frame,
so an already-painted row resolves without drawing at all.

Three rungs in frontends/gpui/tests/mcp_write_repaints_windowed.rs, all driving
the REAL `GpuiUserDriver` over the real interaction pump against a real
launched window. `DrivenFrameGeometry` restores the missing environment fact:
the driver reads a snapshot of the `BoundsRegistry` republished only after an
event whose pump arm actually calls `window.draw()` — `ForceFrame` is the only
one. (The gate is keyed on the event, not on an observed draw, because the
harness must pump the gpui app for the driver's channel round-trip at all, and
pumping repaints every dirty window — so no frame counter could be attributed
to the driver's own command.)

* `an_mcp_created_block_is_clickable_without_a_navigation` — the bug. Phase 1
  is the inverse control (a keyboard write, no navigation, still paints);
  phase 2 creates a block through `FrontendSession::execute_operation` — the
  exact call the MCP `execute_operation` tool makes — and clicks it.
  Red: `lane-logs/rung-red-2.log` (production force suppressed) — phase 1
  passed, phase 2 failed with the verbatim dogfood message `click_entity(
  "block:mcp-paint-sentinel-row"): element bounds never committed; stale focus
  cleared to prevent silent mis-targeted typing`, while the assertion's own
  diagnostic reported the row WAS painted in the window. The earlier
  `lane-logs/rung-red-1.log` is the same red against the force-first shape.
* `only_a_drawing_event_reveals_an_mcp_created_row` — the negative twin.
  Reverting the fix is not the only way to reintroduce the bug: swapping
  `ForceFrame` for any other event reintroduces it just as completely, because
  no other pump arm draws. The rung pushes a non-drawing event (`ScrollList`
  at a nonexistent entity) through the driver's own channel and pins that it
  reveals nothing, then pins that one `ForceFrame` does.
  Red: `lane-logs/rung-negative-red.log` — against a harness that republishes
  on ANY forwarded command (the defect adversarial verification found), it
  fails with `a NON-DRAWING event revealed the row on attempt 0`.
* `a_never_painted_entity_still_fails_loud_at_the_bounds_deadline` — the
  forced draws must not soften the loud arm: a row that genuinely never
  renders still fails at the full 5s `CLICK_BOUNDS_TIMEOUT` (and not beyond
  it) with the rich diagnostic, and the engine's stale focus is still cleared.

Green (all three): `lane-logs/rung-green-2.log`.
