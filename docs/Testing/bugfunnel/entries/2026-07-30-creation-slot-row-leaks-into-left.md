---
id: 2026-07-30-creation-slot-row-leaks-into-left
date: 2026-07-30
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A `block:__virtual:<parent>` creation-slot row leaks into the LEFT SIDEBAR's
  tree, with two faces. (i) It RENDERS as a phantom sidebar row — empty
  content, bullet marker, sitting inside an expanded parent's subtree:
  `block:__virtual:alpha-design`, `parent_id` `block:alpha-design`, `sort_key`
  f64::MAX, 27 tree_items rendered where the sidebar query returns 26 pages
  (visible in the screenshots as an unlabelled bulleted row between a page and
  its child). (ii) EVERY disclosure toggle on that subtree emits an
  ERROR-level `[tree-desync] after Pop: in provider but not row_map
  ["block:__virtual:alpha-design"]` — 6 of 6 toggles, against 0 events over an
  8s idle control, so the toggle causes it. The same family also raised a red
  user-facing banner "Breadcrumb unavailable: breadcrumb: block
  block:__virtual:beta-notebook has no path in block_with_path" after a
  main-panel creation-slot click. Pre-existing (the collapse write path
  predates the disclosure affordance), surfaced by exercising collapse.
source_line: 1123
---

## Bug

A `block:__virtual:<parent>` creation-slot row leaks into the LEFT SIDEBAR's
tree, with two faces. (i) It RENDERS as a phantom sidebar row — empty
content, bullet marker, sitting inside an expanded parent's subtree:
`block:__virtual:alpha-design`, `parent_id` `block:alpha-design`, `sort_key`
f64::MAX, 27 tree_items rendered where the sidebar query returns 26 pages
(visible in the screenshots as an unlabelled bulleted row between a page and
its child). (ii) EVERY disclosure toggle on that subtree emits an
ERROR-level `[tree-desync] after Pop: in provider but not row_map
["block:__virtual:alpha-design"]` — 6 of 6 toggles, against 0 events over an
8s idle control, so the toggle causes it. The same family also raised a red
user-facing banner "Breadcrumb unavailable: breadcrumb: block
block:__virtual:beta-notebook has no path in block_with_path" after a
main-panel creation-slot click. Pre-existing (the collapse write path
predates the disclosure affordance), surfaced by exercising collapse.

## Root cause

a `block:__virtual:<parent>` creation-slot row leaks into the LEFT SIDEBAR's
tree and shows two faces. (i) It renders as a real, phantom sidebar row —
empty content, bullet marker, nested inside an expanded parent's subtree
(`block:__virtual:alpha-design`, `parent_id` `block:alpha-design`,
`sort_key` f64::MAX); 27 tree_items rendered where the sidebar query returns
26 pages. (ii) EVERY disclosure toggle on that subtree emits an ERROR-level
`[tree-desync] after Pop: in provider but not row_map
["block:__virtual:alpha-design"]` — 6/6 toggles, versus 0 events over an 8s
idle control, so it is caused by the toggle, not by background churn. The
same family also produced a red user-facing banner "Breadcrumb unavailable:
breadcrumb: block block:__virtual:beta-notebook has no path in
block_with_path" after a main-panel creation-slot click. ENVIRONMENT: the
keystone's sidebar wiring has no creation-slot provider, so the virtual row
that desyncs provider from row_map does not exist in the test environment at
all — the missing piece is the slot seam in the sidebar's collection
provider, not a transition.)

## Missing piece

The keystone's sidebar wiring has no creation-slot provider, so the virtual
row that desyncs provider from row_map does not exist in the test
environment — the missing piece is the slot seam in the sidebar collection
provider, not a transition. Either the sidebar's page query should not get a
creation slot at all, or the slot must be present in BOTH the provider and
the row_map.

## Remedy

OPEN 2026-07-30 — found by the dogfood gate; distinct from the feature under
test and reported separately so the feature is not blocked on an inherited
defect.
