---
id: 2026-07-22-left-sidebar-edge-shows-col-resize
date: 2026-07-22
gap: PERCEPTION
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Left sidebar edge shows col-resize cursor (drawer.rs cursor_col_resize) but
  click only toggles open/closed (on_mouse_down → set_widget_open); no
  drag-to-resize handler or width state exists, so the resize affordance
  collapses the sidebar instead
source_line: 1092
---

## Bug

Left sidebar edge shows col-resize cursor (drawer.rs cursor_col_resize) but
click only toggles open/closed (on_mouse_down → set_widget_open); no
drag-to-resize handler or width state exists, so the resize affordance
collapses the sidebar instead

## Missing piece

no drag-resize gesture in harness; no assertion that a handle's hover cursor
matches its actual gesture

## Remedy

open
