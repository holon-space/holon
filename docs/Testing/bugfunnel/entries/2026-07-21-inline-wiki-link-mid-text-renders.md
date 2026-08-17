---
id: 2026-07-21-inline-wiki-link-mid-text-renders
date: 2026-07-21
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Inline wiki-link mid-text renders the block on 3 stacked lines (inline runs
  laid out as a flex-column instead of a flex-row-with-wrap); bullet
  vertically mis-centered over the tall column; GPUI outliner (reproduced on
  `Plan my Day` block `block:c99c763a…`, link mark over "Implementation
  Intention" splitting the block into ≥2 text runs). Panel is far wider than
  the text, so not word-wrap.
source_line: 1083
---

## Bug

Inline wiki-link mid-text renders the block on 3 stacked lines (inline runs
laid out as a flex-column instead of a flex-row-with-wrap); bullet
vertically mis-centered over the tall column; GPUI outliner (reproduced on
`Plan my Day` block `block:c99c763a…`, link mark over "Implementation
Intention" splitting the block into ≥2 text runs). Panel is far wider than
the text, so not word-wrap.

## Missing piece

no inline-run horizontal-flow layout assertion (windowed T3 layout snapshot
/ GPUI layout unit test on the inline-run container's flex direction)

## Remedy

open
