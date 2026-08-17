---
id: 2026-07-20-mark-bounds-render-clamp-warn-logged
date: 2026-07-20
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Mark-bounds render-clamp warn logged at ERROR every render frame (1847× for
  one corrupt block) — should throttle once-per-block
source_line: 1050
---

## Bug

Mark-bounds render-clamp warn logged at ERROR every render frame (1847× for
one corrupt block) — should throttle once-per-block

## Missing piece

log-throttle on the degraded-render warn

## Remedy

FIXED+WOVEN 2026-07-21 — once-per-key-per-boot throttle (warn_throttle.rs):
first occurrence ERROR (loudness preserved), repeats debug!; namespaced keys
(block id / corrupt span). No global mute. Unit-tested
