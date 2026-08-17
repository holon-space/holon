---
id: 2026-07-21-phantom-empty-virtual-journals-row-leaks
date: 2026-07-21
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  Phantom empty __virtual:journals row leaks into the SIDEBAR page tree
  (main-panel virtual creation slot included as child of the sidebar tree
  source), rendering an empty bullet. Mirrors the 2026-07-07
  phantom-__virtual: class.
source_line: 1056
---

## Bug

Phantom empty __virtual:journals row leaks into the SIDEBAR page tree
(main-panel virtual creation slot included as child of the sidebar tree
source), rendering an empty bullet. Mirrors the 2026-07-07
phantom-__virtual: class.

## Missing piece

exclude __virtual: ids from the sidebar tree source; invariant: sidebar-tree
rows == backing page-query rows

## Remedy

OPEN (dogfood-round3 V4; ui-polish addendum)
