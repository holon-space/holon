---
id: 2026-07-19-empty-block-type-here-placeholder-text
date: 2026-07-19
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Empty-block "Type here" placeholder text renders OVERLAPPING typed content
  while editing the empty affordance block (GPUI dogfood): typing into the
  journal's empty new-block affordance shows the live text (e.g. "Buy milk",
  "Review PR", "/") drawn on top of the still-visible grey "Type here"
  placeholder — the placeholder is not hidden on first keystroke, producing
  overlapping glyphs until commit.
source_line: 1015
---

## Bug

Empty-block "Type here" placeholder text renders OVERLAPPING typed content
while editing the empty affordance block (GPUI dogfood): typing into the
journal's empty new-block affordance shows the live text (e.g. "Buy milk",
"Review PR", "/") drawn on top of the still-visible grey "Type here"
placeholder — the placeholder is not hidden on first keystroke, producing
overlapping glyphs until commit.

## Missing piece

no headless invariant for placeholder-vs-content occlusion; windowed layout
snapshot of an empty block mid-edit asserting placeholder hidden when buffer
non-empty

## Remedy

OPEN — found GPUI dogfood 2026-07-19; cosmetic
