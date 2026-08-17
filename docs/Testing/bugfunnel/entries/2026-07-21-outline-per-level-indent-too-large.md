---
id: 2026-07-21-outline-per-level-indent-too-large
date: 2026-07-21
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Outline per-level indent too large (style.rs:49 tree_indent_px=28.0, applied
  tree_item.rs:203/212) + oversized marker→text gap.
source_line: 1054
---

## Bug

Outline per-level indent too large (style.rs:49 tree_indent_px=28.0, applied
tree_item.rs:203/212) + oversized marker→text gap.

## Missing piece

none (aesthetic)

## Remedy

FIXED+WOVEN 2026-07-21 (cycle 2) — tree_indent_px 28→20; round-4
live-confirmed
