---
id: 2026-07-21-flat-single-color-background-window-lib
date: 2026-07-21
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Flat single-color background — window bg lib.rs:757 flat theme.background;
  linear_gradient API exists but only used in examples/design_gallery.rs.
source_line: 1055
---

## Bug

Flat single-color background — window bg lib.rs:757 flat theme.background;
linear_gradient API exists but only used in examples/design_gallery.rs.

## Missing piece

none (aesthetic; Martin requested subtle theme-derived fade)

## Remedy

FIXED+WOVEN 2026-07-21 (cycle 2) — theme-derived 160° linear_gradient,
per-frame from live theme.background, tunable spread (0.015 judged too faint
in round-4 → 0.035). Dark-mode confirmed by code (theme-tracking), not yet
by screenshot
