---
id: 2026-07-08-per-edit-org-writeback-ran-recursive
date: 2026-07-08
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Per-edit org writeback ran the O(N) recursive-CTE `get_blocks` 2×/edit
  (render + `materialize_images`), ~585ms@2k / ~4s@5k — breaches p95<200ms on
  the CRDT interactive path
source_line: 868
---

## Bug

Per-edit org writeback ran the O(N) recursive-CTE `get_blocks` 2×/edit
(render + `materialize_images`), ~585ms@2k / ~4s@5k — breaches p95<200ms on
the CRDT interactive path

## Missing piece

no per-edit writeback SLO invariant / recursive-CTE-count assertion in
keystone; recursive CTE is cheap at keystone's small N so wall never
breaches

## Remedy

fixed (uncommitted): Tier-1 per-doc block cache + O(1) `block_raw`
point-read + image-gated `materialize_images`; regression test
`crates/holon-orgmode/tests/incremental_org_writeback_smoke.rs` asserts 0
recursive-CTE per content edit; keystone SLO invariant still open
