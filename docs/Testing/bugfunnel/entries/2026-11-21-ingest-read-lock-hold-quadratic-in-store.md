---
id: 2026-11-21-ingest-read-lock-hold-quadratic-in-store
date: 2026-11-21
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  The doc save timed out 77 times in one morning on a 16,025-block vault because org ingest held the
  Loro read lock for a time quadratic in the entity store, and the writer waits for every reader to drain.
---

## Bug

Found by reading the production log of 2026-11-21: `timed out after 30s waiting for the doc write lock` on doc
`holon_tree`, about once a minute from 05:43Z. A live stack sample parked the save in `DocLock::write` waiting for readers;
the reader was org ingest in `LoroEntityOperations::replace_scope`.

## Root cause

`replace_scope` and `read_all` read every key of a type's root map under one read lock and called `read_entity` per key.
`read_entity` resolved the whole root map through `LoroDoc::get_by_path` on every call, so n entities cost O(n^2) under one lock.
Measured on the `test` build profile with the `read_hold` spans: quadrupling the store from 500 to 2000 entities multiplied the hold
by 42.6x (`replace_scope`, 21 ms -> 906 ms) and 40.1x (`read_all`, 22 ms -> 867 ms); linear growth gives 4x.
The same per-key shape sat in `read_changed_rows` for the projector.

## Missing piece

No test measured read-lock hold against store size. The keystone `general_e2e_composed_pbt.rs` ingests vaults of a few dozen blocks;
the quadratic term is invisible below a few thousand entities of one type (21 ms at 500 on the test profile), so it can neither
generate nor feel the defect. ENVIRONMENT: the interaction is generatable, the vault size is not. Prod and test could be made closer by a
keystone seed that holds thousands of entities of one typed store, which costs CPU the keystone budget does not have today.

## Remedy

`read_entity_in` reads against an already-resolved root map; the three loops resolve the root once, so the hold is O(store keys + read fields).
Pinned by `typed_entity_read_scale` (hold ratio between two store sizes, from a `read_hold` span, red on the old loops, green after) and
`typed_entity_replace_scope_equivalence` (expected effect of `replace_scope` derived from a per-entity read).
Dogfood of at least 1 h with claude-history active remains to be run by the gate.
