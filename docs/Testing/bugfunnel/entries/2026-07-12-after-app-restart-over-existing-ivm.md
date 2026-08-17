---
id: 2026-07-12-after-app-restart-over-existing-ivm
date: 2026-07-12
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  After app restart over an existing DB, the `block` IVM matview returns 2-3
  DUPLICATE rows per id (9 ids incl. all user blocks + default panels;
  `block_raw` is clean) — re-ingest inserts un-consolidated deltas; UI masks
  it via entity-keyed dedupe but org writeback consumed a STALE duplicate
  (that's what re-serialized the old "]][[ " content variant → compounding
  disk corruption). Same class as the FIXED turso-ivm commit-consolidation
  bug, now on the reboot/re-ingest path
source_line: 904
---

## Bug

After app restart over an existing DB, the `block` IVM matview returns 2-3
DUPLICATE rows per id (9 ids incl. all user blocks + default panels;
`block_raw` is clean) — re-ingest inserts un-consolidated deltas; UI masks
it via entity-keyed dedupe but org writeback consumed a STALE duplicate
(that's what re-serialized the old "]][[ " content variant → compounding
disk corruption). Same class as the FIXED turso-ivm commit-consolidation
bug, now on the reboot/re-ingest path

## Missing piece

keystone never boots twice over the same DB (cold-restart re-ingest
unmodeled); no "matview row-count == base row-count per id" invariant

## Remedy

FIXED (verified 2026-07-16, no new code): root cause = Turso IVM reopen
consolidation — the reopen-triggered autocheckpoint rebuilt the persisted
`block` JOIN-matview's DBSP state with non-deterministic MergeOperator
rowids, so re-ingest retraction deltas couldn't match surviving output rows
→ 2-3 identical matview rows per id while `block_raw` stayed PK-unique.
Already fixed by the fork commits that shipped for the B1 browser
vault-brick (row 156): `ce1504b818` (deterministic MergeOperator rowids) +
`8517e30647` (antijoin TryAdvance ghost row), both ancestors of the holon
pin `turso 3dd5d689`; native was never independently confirmed until now.
HOLON-SIDE REPRO GREEN:
`matview_reboot_duplicate_repro.rs::block_matview_no_duplicates_after_reboot_over_existing_db`
(boot → `stop_app` → boot-2 over the same `test.db`, assert no dup id AND
per-id `block`==`block_raw`) + new faithful edge-field case
`block_matview_with_edge_fields_no_duplicates_after_reboot` (tags+requires
through prod `LoroBackend` before reboot, exercising the
`block_tags_agg`/`block_requires_agg` re-assert delta row 150 identified).
Also GREEN: `turso_ivm_negative_weight_restart_repro` + holon-turso nextest
110/110. ENV GAP stays open in the keystone: `SimulateRestart` only
touch-writes org files to re-parse — never shuts the Turso actor / reopens
the file-backed DB, so the reopen-autocheckpoint path is unreachable there;
only `stop_app()`+`start_app()` reaches it (same parity arm row 156
proposes)
