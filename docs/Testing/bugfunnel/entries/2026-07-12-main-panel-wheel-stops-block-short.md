---
id: 2026-07-12-main-panel-wheel-stops-block-short
date: 2026-07-12
gap: ENVIRONMENT
secondary: PERCEPTION
status: MITIGATED
summary: >-
  Main-panel wheel stops ~1 block short: last block ~90% clipped + creation
  slot unreachable at max scroll (gpui list end-of-list summary().height
  undercount; content-independent)
source_line: 964
---

## Bug

Main-panel wheel stops ~1 block short: last block ~90% clipped + creation
slot unreachable at max scroll (gpui list end-of-list summary().height
undercount; content-independent)

## Missing piece

keystone drives synthetic scroll, never a real wheel at real geometry vs a
real vault; no oracle on "last row fully reachable"

## Remedy

MITIGATED via scroll-past-end padding (LIST_SCROLL_PAST_END_PX); gpui
undercount OPEN — real fix + real-geometry regression test belong in the
gpui fork
