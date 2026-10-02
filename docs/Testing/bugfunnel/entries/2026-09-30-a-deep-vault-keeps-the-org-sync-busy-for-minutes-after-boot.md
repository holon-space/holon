---
id: 2026-09-30-a-deep-vault-keeps-the-org-sync-busy-for-minutes-after-boot
date: 2026-09-30
gap: ORACLE
secondary: null
status: PARTIAL
summary: >-
  On a 10,000-block vault (10 docs x 50 chains x depth 20) the session stays
  busy for more than 14 minutes after its boot is quiescent, and the first
  owning-page read of the test never returns. Root cause: the boot ingest is
  quadratic, because each org file ingest does work in proportion to the
  whole vault (four O(N)-per-file sites; one fixed).
---

## Bug
Found by an automated test, `owning_page_cost_on_a_deep_vault`
(`crates/holon-integration-tests/tests/authority_owning_page_latency.rs`), in
the dogfooding phase 1 lane (Increment 1c). It was read as "a slow test that
needs a longer nextest cap". That reading is wrong:

- The boot and `wait_for_loro_quiescence` finish (`OWNING_PAGE_BOOT loro`
  126 s and 210 s, test profile, host load 40-60).
- The first `measure("loro one-read", …)` then does not finish within 1200 s
  under nextest (TIMEOUT), or within 14 minutes under `cargo test`.
- A `sample` of the test process 14 minutes after the boot line
  (`lane-logs/dogfood-inc1c-owning-sample.txt` in that lane): the test thread
  waits in `block_on`. One worker is near 100 % CPU. Its hot frames are
  `holon_orgmode::di::run_file_sync_controller` →
  `holon_api::live_data::home_by::process_diff` (`BlockHomeAuthority`) →
  `CacheBlockReader::get_block_authoritative` / `BlockRowMemo::get` →
  `TursoBackend::query_rows`. They are mixed with Turso
  `incremental::join_operator … commit`, `persistence::WriteRow::write` and
  btree insert or balance. So the org sync does per-block authoritative reads
  and IVM maintenance without end, and the owning-page read waits behind it.

## Root cause
The boot ingest is quadratic: each org file ingest does work in proportion to
the whole vault. Measured on a synthetic vault with the shape of the real one
(`scripts/gen_deep_vault.py`, 400 pages, 17 404 headlines, release
`holon-mcp`): the per-file ingest time grows from 52 to 297 ms over 400 files.
The `sample` of the ingest shows these O(N)-per-file sites:

1. `find_foreign_blocks` loaded and hydrated every block of the store on every
   file ingest (trait default over `iter_documents_with_blocks`).
2. `LoroDocumentStore::save_all` exports the whole Loro doc and fsyncs it on
   every projection flush, that is once per file
   (`crates/holon-loro/src/loro_sync_controller.rs`, `emit_ops`).
3. `BlockCellRegistry::resolve_node_meta` scans the whole Loro tree on every
   field write.
4. `find_tree_id_by_stable_id_sync` scans the whole Loro tree on a cache miss,
   and every create of a new id misses.

The `home_by` fold (hypothesis 1 of the first triage) is not the dominant cost
on this shape: 28 of about 6000 samples.

## Fix status
- Site 1 is fixed: `holon_filesystem::find_foreign_blocks` does one point read
  per asked id and a memoized walk to the nearest page. Red:
  `crates/holon-filesystem/tests/find_foreign_blocks_cost.rs` (a rows-read
  bound and a differential check against the full scan). After the fix, the
  per-file ingest time grows from 78 to about 140 ms over 400 files.
- Site 4 is fixed by the complete stable-id index in holon-loro,
  maintained from Loro doc events (ruling D1.a), so a miss is authoritative
  and never scans (see Remedy).
- Site 3 is fixed: `resolve_node_meta` reads the per-doc stable-id index.
  Pinned by `crates/holon-loro/tests/field_write_cost.rs`. Red by sabotage
  (the old tree scan): 350 us per write at N=500 blocks, 6.97 ms at N=20000
  (`lane-logs/dvul-i4-red.log`); green 139 us and 141 us, 0 index nodes
  touched per write.
- Site 2 is fixed: `save_all` appends the changes since the last save to an
  update log beside the snapshot (`crates/holon-loro/src/update_log.rs`) and
  writes a snapshot only when the log would outgrow it. Pinned by
  `crates/holon-loro/tests/update_log_cost.rs`: red 19 371 bytes written per
  one-field save at N=500 and 292 680 at N=8000
  (`lane-logs/dvul-i6-red-cost.log`); green 96 and 97 bytes.

## Missing piece
The test has no latency oracle that fails with a message about latency. Its
only failure is the runner's timeout, and that reads as "the test needs more
time". No gate runs it.

## Remedy
PARTIAL. Sites 1-4 are fixed (see Fix status). The stable-id index
(D1.a) landed with increments 1-3; the probe
`crates/holon-loro/src/tree_event_delivery_probe.rs` shows that every event
source reaches a tree subscription, and `TreeWatch`
(`crates/holon-loro/src/tree_watch.rs`) is the shared watch that `MountIndex`
uses. The index (`crates/holon-loro/src/stable_id_index.rs`) is pinned by
`stable_id_index_pbt.rs` (differential against a full scan),
`stable_id_index_cost.rs` and the architecture rule
`stable_ids_are_written_only_through_the_index_chokepoint`.

Release dv400 (synthetic vault, 400 pages, profile `release`; the host was
loaded, load average 7-22 at the earlier runs and 5-10 at the last two):

| State | Initial scan | Per-file ingest, first 50 -> last 50 files |
|---|---|---|
| Before any fix | not recorded | 52 -> 297 ms |
| Fix 1 only | not recorded | 78 -> about 140 ms |
| Sites 1 and 4 | 36 s | 52 -> 96 ms |
| Sites 1, 3, 4 (increment 4) | 30.0 s | 42 -> 88 ms |
| Sites 1-4 (update log; two runs) | 27.4 s and 27.6 s | 46 -> 76 ms and 46 -> 74 ms |

The per-file time still grows about 1.6 times over the 400 files. A remaining
O(N) site is not yet found; shared docs (increment 5) are not done. The test
`owning_page_cost_on_a_deep_vault` gets no nextest override until it finishes
in budget; run it again after each fix.
