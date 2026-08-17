---
id: 2026-07-16-vault-scale-latency-slo-catastrophically-breached
date: 2026-07-16
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Vault-scale latency SLO catastrophically breached (SqlOnly desktop, ~50-file
  real vault): set_field e2e p50=119ms / p95=23,090ms / max=55,439ms (n=13);
  boot ingest 211.7s total, worst single file 71s; execute_raw_sql via
  DatabaseActor varies 0.6ms–9s for trivial queries under churn
source_line: 826
---

## Bug

Vault-scale latency SLO catastrophically breached (SqlOnly desktop, ~50-file
real vault): set_field e2e p50=119ms / p95=23,090ms / max=55,439ms (n=13);
boot ingest 211.7s total, worst single file 71s; execute_raw_sql via
DatabaseActor varies 0.6ms–9s for trivial queries under churn

## Missing piece

latency SLO oracle not wired at real-vault scale for SqlOnly (prior verdict
"SqlOnly meets SLO" measured 1.5k synthetic blocks, not real-vault file
count/shape)

## Remedy

OPEN — measured via scripts/measure_latency.py on
/tmp/dogfood-0716-logs/app3.log. RE-MEASURED 2026-07-17 (GPUI SqlOnly,
/tmp/holon.log, loro:false): WORSE — boot_ingest_total 347.9s (was 211.7s),
worst single file Projects/Holon.org boot_write 135.7s (was 71s). Confirmed
O(N²): per-block cost rises with ACCUMULATED VAULT SIZE, not file size —
Projects.org 30ms/block (ingested 1st, 907 blocks) vs Advice Dogfood.org
1,139ms/block (5 blocks, ingested late); inter-write matview-demux gap grows
11→120→258ms across 5,505 writes (max 4.6s). Mechanism localized: per-block
single-row writes in file_sync_controller::on_file_changed boot_write loop
(file_sync_controller.rs:1622-1651) each drive whole-block-table matview IVM
maintenance while live UI watch-views are active during boot
(matview_manager demux fires once per write, items=1, but delta PRODUCTION
cost scales with table size). Ruled out: new_child_anchor/sibling_keys
(per-parent, resets per file) and the 2026-07-17 row-33 links fix
(Loro-batch path only; this run is SqlOnly — CAVEAT: on Loro-default wiring
the new per-op block_links derivation sits in the same hot loop, measure
separately). Fix levers cheapest-first: (1) batch per-file op application so
maintenance runs per-file not per-block; (2) suspend/defer live watch-view
maintenance during initial scan + one end-of-scan convergence pass (mirrors
row-84 feed barrier); (3) true O(delta) matview maintenance for single-row
inserts (fork IVM work). Keystone gap: needs many-file cold-boot rung
(HOLON_SOAK_SEED_FILES harness exists) with an O(N²)-detecting
per-block-cost-vs-accumulated-count assertion. **FIX LANDED 2026-07-17
(lever 1, pending Martin's live re-measure — his GPUI binary rebuild over
the real vault is the real proof).** Root fix: the boot loop no longer
applies ops one block at a time. `file_sync_controller::on_file_changed`'s
per-op `for (op,params) { update_in_tree/delete_in_tree }`
(file_sync_controller.rs ~1622) is replaced by ONE
`BlockOrdering::apply_ingest_batch(operations)` call per file. New trait
method on `BlockOrdering` (block_ordering.rs) with a default per-op-loop
impl (Loro seam + test impls unchanged); `SqlBlockOperations`
(sql_block_operations.rs) overrides it: SqlOnly routes the whole file's
op-vector through `SqlOperationProvider::execute_batch_with_origin` — ONE
`db_handle.transaction()`, so the live-watch `block` matview IVM maintenance
runs ONCE per file instead of once per block. Semantics preserved: same ops,
EventOrigin::Org, create-vs-update re-derived from the SQL cache exactly as
`update_in_tree`; block_links + page-reresolve derived by the batch sink
(verified by the two `wiki_link_ingest_marks_junction` tests, SqlOnly+Loro
green). Creates are born carrying strictly-increasing per-parent doc-order
`sort_key`s (minted in-memory via `gen_key_between`, seeded once per parent
from the sibling set) so the downstream SqlOnly `place_all` totalizer finds
them ordered and rewrites ZERO rows — otherwise the per-block cost would
just migrate into place_all's single-op `set_field`s. FK ordering safe:
batch writes all `block_raw` rows then all edges in one transaction,
deferred parent FK settles at COMMIT. Loro mode falls back to the per-op
seam verbatim (the measured O(N²) was SqlOnly-only). Deterministic proof
(unit tests in sql_block_operations.rs, counting `block`-matview CDC batches
= IVM maintenance passes):
`batched_ingest_runs_matview_maintenance_once_per_file` = 16 blocks → **1**
pass; contrast `per_op_ingest_runs_one_matview_pass_per_block` = 8 blocks →
**8** passes. Gates green: nextest holon-filesystem+holon-turso (144), both
wiki_link tests, keystone standard PROPTEST_CASES=8. Keystone many-file
O(N²) rung still open (soak/diag guard remains the wall-clock non-vacuity
proof). **RE-MEASURED LIVE 2026-07-17 (Martin's rebuilt GPUI binary, real
vault): FIX CONFIRMED ACTIVE** — inter-write matview-demux gap collapsed
5,758→244 (the batched path fires per-file, not per-block); worst-file
per-block cost 30→2.7ms/block; boot_ingest_total 347.9s→219s. The remaining
~219s wall is now dominated by a SEPARATE cause — the
MCP-provider-sync-vs-boot-scan contention (new 2026-07-17 row above): the
claude-history integration's 79 concurrent full re-syncs interleaved with
per-file ingest on the serialized DatabaseActor; that row's DEFER+DEBOUNCE
fix removes the interleaving.
