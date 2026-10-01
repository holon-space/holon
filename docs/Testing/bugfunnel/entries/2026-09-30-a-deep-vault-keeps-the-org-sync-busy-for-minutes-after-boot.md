---
id: 2026-09-30-a-deep-vault-keeps-the-org-sync-busy-for-minutes-after-boot
date: 2026-09-30
gap: ORACLE
secondary: null
status: OPEN
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
  and never scans (see Remedy). Sites 2-3 are open; site 3 moves to the same
  index next. Site 2 keeps its crash-safety contract and
  needs a save whose cost is in proportion to the change.

## Missing piece
The test has no latency oracle that fails with a message about latency. Its
only failure is the runner's timeout, and that reads as "the test needs more
time". No gate runs it.

## Remedy
PARTIAL. Site 1 is fixed (see Fix status). Site 4 uses the index; sites 2-3 are open. For the
complete stable-id index (D1.a), increments 1-2 landed: the probe
`crates/holon-loro/src/tree_event_delivery_probe.rs` shows that every event
source reaches a tree subscription, and `TreeWatch`
(`crates/holon-loro/src/tree_watch.rs`) is the shared watch that `MountIndex`
uses. Increment 3 landed: the stable-id index
(`crates/holon-loro/src/stable_id_index.rs`, one per doc in the doc-lock
registry, fed by tree events and the `write_stable_id` chokepoint) answers
`find_tree_id_by_stable_id_sync` and `resolve_parent_core`; a miss is
authoritative and the old cache and both scans are deleted. Pinned by
`crates/holon-loro/tests/stable_id_index_pbt.rs` (differential against a full
scan), `stable_id_index_cost.rs` (no rebuild, reads in proportion to the
change) and the architecture rule
`stable_ids_are_written_only_through_the_index_chokepoint`. Release dv400
(synthetic, host load 7-22): the per-file ingest time grows 52 -> 96 ms over
400 files (fix 1 alone: 78 -> ~150 ms), initial scan 36 s. Remaining: fix 3
(`resolve_node_meta` through the index, increment 4), shared docs (increment
5), fix 2 (increment 6). The test
`owning_page_cost_on_a_deep_vault` gets no nextest override until it finishes
in budget; run it again after each fix.
