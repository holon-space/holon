---
id: 2026-08-31-overlay-sidebar-not-drivable-over-mcp
date: 2026-08-31
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Neither the chrome sidebar toggle nor the overlay scrim is addressable over
  MCP, so the whole narrow-mode overlay-sidebar feature — open, dismiss on
  link, dismiss on outside click, scrim click-through — cannot be driven or
  verified by any MCP-driven check.
---

## Bug

Found by the `dogfood-explorer` gate attempting to exercise the overlay left
sidebar in a real GPUI window at 560x850 (lane `dogfood-mobile`, port 8720).
The feature is present and in narrow mode — `describe_ui` reports
`{"widget":"drawer","block_id":"block:default-left-sidebar","mode":"overlay","open":false}`.
It could not be opened, so none of its behaviour was tested.

Both addressing forms fail:

- By entity: the canonical element id is not an `EntityUri`, and the tool
  rejects it before dispatch —
  `click{"entity_id":"drawer_toggle::block:default-left-sidebar"}` →
  `entity_id is not a valid EntityUri: … unexpected character at index 6`.
- By coordinate: ten clicks across the toggle's painted area all returned
  `handled:false`, which is uninformative because that flag is false for every
  click including ones that land (see
  `2026-08-31-mcp-coordinate-click-always-reports-unhandled`), and no screenshot
  showed the drawer opening.

There is no operation surface either: `list_operations{"entity_name":"ui"}`
returns `[]`, and the `navigation` entity exposes focus/pin/close/back/forward/
home/activate/open_tab — nothing that toggles a drawer.

## Root cause

The chrome toggle is a bare GPUI div with a raw string id and an inline
handler (`frontends/gpui/src/lib.rs:1167-1176`):

```rust
div().id("sidebar-toggle").cursor_pointer()… .child("☰")
    .on_mouse_down(MouseButton::Left, move |_, _, cx| { … })
```

It is never registered in the `BoundsRegistry` and carries no entity URI, so
the hit-test path `click{entity_id}` resolves against cannot see it. The
dismiss scrim is in the same position: `columns.rs:355` gives it
`hashed_id(OVERLAY_SCRIM_ID)` and tracks it under the element name
`"overlay_scrim"` (columns.rs:369) — an element id, not an entity.

The drawer's own toggle widget IS registered
(`drawer_toggle_id_for` → `drawer_toggle::{block_id}`,
`crates/holon-frontend/src/geometry.rs:391`), which is what the layout PBT
clicks via `sut.click_at_element(&toggle_id)`
(`crates/holon-layout-testing/src/transitions/toggle_drawer.rs:67`). The MCP
`click` tool has no `click_at_element` equivalent — it accepts only an
`EntityUri` or raw coordinates.

## Missing piece

`frontends/gpui/tests/overlay_sidebar_dismisses_windowed.rs` reaches the drawer
through the in-process element-id path that the PBT drivers have. The MCP
surface — the one the live-MCP keystone (`just keystone-mcp`) and the
`dogfood-explorer` gate use — has no rung for element-id clicks at all. So the
final quality gate before Martin structurally cannot exercise the feature he is
most likely to hit first on a phone, and did not: open, dismiss-on-link,
dismiss-on-outside-click and scrim click-through are all UNVERIFIED by this
pass.

## Remedy

Open. The cheap fix is an element-id rung on the MCP `click` tool — accept the
`drawer_toggle::…` / `overlay_scrim` element ids the `BoundsRegistry` already
carries, alongside the `EntityUri` form. Registering the chrome `☰` in the
registry would additionally make the real user control (not just the drawer's
own handle) drivable.
