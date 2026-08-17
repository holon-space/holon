---
id: 2026-07-13-boot-seed-churn-task-ivm-diagnosis
date: 2026-07-13
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Boot re-seed churn (Task #65, from IVM P1 diagnosis): every launch re-runs
  `seed_default_layout` → `create_in_tree` for the always-seeded journals
  blocks (page/src/render/auto-create/rule); `create_entity`'s idempotent
  existing-node branch skipped the CREATE but UNCONDITIONALLY re-asserted
  tags/requires/advice (`set_block_tags` = `meta.insert`+`doc.commit()` every
  boot), emitting a Loro op → DiffEvent → `block_tags`/`block_raw`
  DELETE+INSERT for UNCHANGED rows. On a restart (persisted `block` matview
  keeps a `Page` tag its emptied base `block_tags` no longer holds) this
  redundant re-assert is also a delta that DOUBLES the matview tag row — the
  trigger for the matview-reopen duplicate. Wasted boot time + latency; no
  data loss
source_line: 980
---

## Bug

Boot re-seed churn (Task #65, from IVM P1 diagnosis): every launch re-runs
`seed_default_layout` → `create_in_tree` for the always-seeded journals
blocks (page/src/render/auto-create/rule); `create_entity`'s idempotent
existing-node branch skipped the CREATE but UNCONDITIONALLY re-asserted
tags/requires/advice (`set_block_tags` = `meta.insert`+`doc.commit()` every
boot), emitting a Loro op → DiffEvent → `block_tags`/`block_raw`
DELETE+INSERT for UNCHANGED rows. On a restart (persisted `block` matview
keeps a `Page` tag its emptied base `block_tags` no longer holds) this
redundant re-assert is also a delta that DOUBLES the matview tag row — the
trigger for the matview-reopen duplicate. Wasted boot time + latency; no
data loss

## Missing piece

keystone boots FRESH per case (each on its own temp DB) and never re-seeds
over a persisted Loro doc / existing DB, so a boot re-seed against
already-identical rows is never exercised; the `inv-sql-budget` N+1 oracle
is per-transition, not a per-boot "re-seed of unchanged state emits zero
edge-field writes" idempotence invariant

## Remedy

FIXED (2026-07-13): `BlockCellRegistry::create_entity` existing-node branch
now reads the current block from the Loro tree (the authority,
deterministically loaded before the seed runs — unlike the boot-lagging SQL
matview) and only calls
`set_block_tags`/`set_block_requires`/`set_block_advice_suppressed` when the
requested value DIFFERS. Tagless-placeholder reconcile preserved
(current≠requested still writes); user content never touched (existing-node
branch never wrote content). Pinned by
`create_entity_is_idempotent_for_unchanged_edge_fields` (oplog-frontier
watermark; negative control asserts a changed tag set still writes) in
`crates/holon-loro/src/block_cell_registry.rs`. NOTE: the residual
matview-reopen DUPLICATE from the reboot re-ingest path (block_tags base
empty on restart while persisted matview retains the tag) is a SEPARATE
Turso-IVM consolidation bug (`matview_reboot_duplicate_repro.rs`), not
closed by this fix
