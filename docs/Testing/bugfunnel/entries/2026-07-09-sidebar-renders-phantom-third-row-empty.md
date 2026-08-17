---
id: 2026-07-09-sidebar-renders-phantom-third-row-empty
date: 2026-07-09
gap: PERCEPTION
secondary: ORACLE
status: OPEN
summary: >-
  Sidebar renders a phantom third row `sentinel:__virtual:no_parent` (empty
  bullet, no text): 3 rendered rows vs the backing Page query returning 2
  (`block:a6249a34`, `block:journals`). The tree appends a `no_parent` virtual
  creation slot at the forest root and the sidebar renders it as an empty page
  row
source_line: 875
---

## Bug

Sidebar renders a phantom third row `sentinel:__virtual:no_parent` (empty
bullet, no text): 3 rendered rows vs the backing Page query returning 2
(`block:a6249a34`, `block:journals`). The tree appends a `no_parent` virtual
creation slot at the forest root and the sidebar renders it as an empty page
row

## Missing piece

sidebar tree appends a `__virtual:no_parent` creation slot rendered as an
empty row; no invariant reconciles rendered sidebar rows against backing
Page-query rows + declared virtual slots

## Remedy

open — INVESTIGATED (fix attempted + reverted after live verification):
gating the `tree` streaming-render `virtual_child` on the `creation_slot:
true` opt-in (`shadow_builders/tree.rs:95`, mirroring `build_trailing_slot`)
is NECESSARY but INSUFFICIENT — with it applied and rebuilt, the sidebar
STILL rendered `sentinel:__virtual:no_parent`. Both `tree` branches are then
gated, and the sidebar is a `tree` (not a `list`), so the forest-root slot
originates in a DEEPER layer (reactive-view
`AppendedRowsProvider::creation_slot` / the data-source injection at
`reactive_view.rs:156`, `:979-985`), not the widget builder. The tree.rs
gate was verified harmless to the wanted main-panel slot
(`block:__virtual:journals` preserved) but reverted this session because it
doesn't resolve the observed bug; the sibling
`shadow_builders/list.rs:9`/`:14` carry the same ungated pattern. Proper fix
= trace the sidebar's actual slot-injection path
(AppendedRowsProvider/data-source) and gate the forest-root (`no_parent`)
creation slot there / for non-editable page-list trees. ESCALATION (dogfood
#4, 2026-07-12): the phantom row now surfaces as `block:__virtual:journals`
(the MAIN panel's creation slot leaking into the sidebar page list), it is
CLICKABLE, and its selectable carries the page-list `navigation_focus`
action — clicking it navigates Main to the virtual block and renders a fully
EMPTY main panel (user dead-end; typed text + Enter then silently vanish).
No longer only perception: an interaction trap
