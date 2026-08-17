---
id: 2026-07-22-long-block-content-does-reflow-martin
date: 2026-07-22
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Long block content does not reflow (Martin dogfooding): a long line runs off
  the right edge of the main panel and is clipped instead of wrapping onto new
  lines. Confirmed live on all three of a long spaced sentence, a long
  unbreakable token, and a long URL (all clipped pre-fix). Root cause: an
  outline row is `tree_item` → `div().flex_1()` wrapping the block's `w_full`
  content widget (`rendered_text`); a flex item defaults to `min-width: auto`
  (its content's intrinsic minimum), so without `min_w(0)` the wrapper refuses
  to shrink below the natural width of a long line and the `w_full` text
  inside never gets a bounded width to wrap against.
source_line: 1104
---

## Bug

Long block content does not reflow (Martin dogfooding): a long line runs off
the right edge of the main panel and is clipped instead of wrapping onto new
lines. Confirmed live on all three of a long spaced sentence, a long
unbreakable token, and a long URL (all clipped pre-fix). Root cause: an
outline row is `tree_item` → `div().flex_1()` wrapping the block's `w_full`
content widget (`rendered_text`); a flex item defaults to `min-width: auto`
(its content's intrinsic minimum), so without `min_w(0)` the wrapper refuses
to shrink below the natural width of a long line and the `w_full` text
inside never gets a bounded width to wrap against.

## Missing piece

Visual reflow/wrap property. A windowed height-based wrap assertion is NOT
expressible in the current harness: the headless gpui TestAppContext text
platform does NOT soft-wrap text at all — an empirical check showed a 5-char
and a 180-char block both commit an identical single-line height regardless
of container width (neither plain-string nor `StyledText`/`InteractiveText`
line-breaks headless). So the wrapping-capable text system is absent from
the test env; only the live window (CoreText/Metal) wraps.
`layout_matrix.rs` records the sibling non-wrapping `text` widget finding
but likewise cannot assert wrap.

## Remedy

FIXED 2026-07-22 (this session): `tree_item.rs` content wrapper is now
`div().flex_1().min_w(px(0.0))`, letting it shrink to the row's available
width so the `w_full` text wraps. Verified live before/after on a seeded
LongPage — the long sentence now wraps across 6 lines, the unbreakable token
across 2, the URL across 4 (all previously clipped off the right edge). No
windowed regression rung possible until the headless text platform performs
soft-wrapping (test-env gap noted above); a follow-up could add a
real-window snapshot rung or a text-measure that line-breaks.
