---
id: 2026-07-19-top-toolbar-exposes-dev-only-affordances
date: 2026-07-19
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Top toolbar exposes DEV-only affordances to end users and there is NO
  user-facing content search (GPUI dogfood): the palette icon opens a "Widget
  Gallery" widget showcase and the magnifier icon opens a dev "INSPECTOR"
  hover-to-inspect panel — neither is a page/quick-open/full-text search, and
  no other search entry point exists. A Logseq power user expects the
  magnifier to be search; instead the app ships two developer tools in the
  primary toolbar and omits the single most-used PKM affordance.
source_line: 1014
---

## Bug

Top toolbar exposes DEV-only affordances to end users and there is NO
user-facing content search (GPUI dogfood): the palette icon opens a "Widget
Gallery" widget showcase and the magnifier icon opens a dev "INSPECTOR"
hover-to-inspect panel — neither is a page/quick-open/full-text search, and
no other search entry point exists. A Logseq power user expects the
magnifier to be search; instead the app ships two developer tools in the
primary toolbar and omits the single most-used PKM affordance.

## Missing piece

no headless invariant expresses "toolbar affordance is user-appropriate /
search exists"; pin via windowed toolbar snapshot + a product decision to
gate dev tools behind a debug flag and add a real search command

## Remedy

OPEN — found GPUI dogfood 2026-07-19
