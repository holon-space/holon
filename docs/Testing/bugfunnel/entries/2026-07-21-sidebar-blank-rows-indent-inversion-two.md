---
id: 2026-07-21-sidebar-blank-rows-indent-inversion-two
date: 2026-07-21
gap: PERCEPTION
secondary: COVERAGE
status: PARTIAL
summary: >-
  Sidebar blank rows + indent inversion: two ID-only org files whose folders
  hold child subdirectories ("Agentic DPL" — spaced folder — and "Resources")
  render as blank sidebar rows (`render: text(col("content"))` with NO
  empty-content fallback) and become depth-0 roots (computed-depth inversion)
  because they are orphaned to `sentinel:no_parent`. ROOT PRODUCER
  (explorer-confirmed 2026-07-21): these files were written by
  `convert_block_to_page` with only `#+ID:` and NO `#+TITLE:`, so reingest
  yields an empty-content Page — see the convert/writeback round-trip row
  below; NOT the initially-suspected Directory-entity-purge fallout (that
  theory is superseded).
source_line: 1075
---

## Bug

Sidebar blank rows + indent inversion: two ID-only org files whose folders
hold child subdirectories ("Agentic DPL" — spaced folder — and "Resources")
render as blank sidebar rows (`render: text(col("content"))` with NO
empty-content fallback) and become depth-0 roots (computed-depth inversion)
because they are orphaned to `sentinel:no_parent`. ROOT PRODUCER
(explorer-confirmed 2026-07-21): these files were written by
`convert_block_to_page` with only `#+ID:` and NO `#+TITLE:`, so reingest
yields an empty-content Page — see the convert/writeback round-trip row
below; NOT the initially-suspected Directory-entity-purge fallout (that
theory is superseded).

## Missing piece

No headless invariant expresses the visual blank row; the sidebar render has
no title fallback for an empty-content Page (it should show a
filename/DB-derived title rather than raw empty `content`). Secondary
COVERAGE: the round-trip that produces the title-less page (convert → write
org → reingest → render) is ungeneratable in the keystone — deferred to the
convert/writeback row below, its data root cause. Remedy: content-empty
title fallback in the tree render + fix the writeback (below) so titles
survive the round-trip.

## Remedy

PARTIALLY FIXED 2026-07-21 (ingest-heal LANDED on main: title heals from
filename, orphans reparented); render title-fallback lane in flight.
