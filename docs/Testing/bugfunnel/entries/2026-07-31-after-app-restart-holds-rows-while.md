---
id: 2026-07-31-after-app-restart-holds-rows-while
date: 2026-07-31
gap: ENVIRONMENT
secondary: ORACLE
status: PARTIAL
summary: >-
  After an app restart `block_tags` holds 0 rows while 348 blocks are
  Page-tagged, degrading the breadcrumb banner on every page. ROOT CAUSE FOUND
  AND FIXED for one half, NOT REPRODUCED end-to-end for the other. PROVEN
  half: `BlockSchemaModule::ensure_schema`
  (`crates/holon-turso/src/schema_modules.rs:155-166`) ran `DROP TABLE IF
  EXISTS` on `block_tags`, `block_requires` AND `advice_suppressed` on EVERY
  boot, then recreated them empty; `block_raw` is `CREATE TABLE IF NOT EXISTS`
  (`sql/schema/blocks.sql:5`) and survives, so the junctions and only the
  junctions start each boot empty. Pinned by a new deterministic unit red
  `block_junction_schema_is_non_destructive_across_boots` (block_tags 0 where
  1 was written, block_raw intact), green after removing the three drops.
  UNPROVEN half: the org parser sets the doc-root Page tag itself
  (`crates/holon-org-format/src/parser.rs:166` `document.set_page(true)` ->
  `block.rs:400,410-416` -> `block_params.rs:71-76` ->
  `sql_operation_provider.rs:680` edge_field_replace_sql), so any RE-INGESTED
  file recovers its tag silently; only files whose ingest is SKIPPED by the
  cold-boot fast path (`file_sync_controller.rs:1942-1966`, whose skip
  predicate `content_present_in_all_stores` at `:871-883` certifies Loro tree
  membership and says nothing about junction rows) would stay lost. That
  combination is what Martins unchanged 348-block vault should hit, but FOUR
  headless reboot cases could not reproduce it:
  `page_tags_survive_reboot_over_existing_db`,
  `loro_written_edge_fields_survive_reboot_over_existing_db` and even
  `wiped_junction_is_repaired_on_next_boot` (explicit `DELETE FROM block_tags`
  between boots) all PASSED on the unfixed tree — the harness repopulates on
  boot 2 despite persisted `file.content_hash` values being present. So the
  fix is proven necessary but NOT proven sufficient for the live symptom.
  Breadcrumb causal link confirmed read-only: `QueryEngine::breadcrumb_trail`
  joins `block_tags` (`crates/holon/src/api/query_engine.rs:162-166`) and
  bails at `:194` with the exact banner text.
source_line: 1128
---

## Bug

(task #71, dogfood on the real vault) After an app restart `block_tags`
holds 0 rows while 348 blocks are Page-tagged, degrading the breadcrumb
banner on every page. ROOT CAUSE FOUND AND FIXED for one half, NOT
REPRODUCED end-to-end for the other. PROVEN half:
`BlockSchemaModule::ensure_schema`
(`crates/holon-turso/src/schema_modules.rs:155-166`) ran `DROP TABLE IF
EXISTS` on `block_tags`, `block_requires` AND `advice_suppressed` on EVERY
boot, then recreated them empty; `block_raw` is `CREATE TABLE IF NOT EXISTS`
(`sql/schema/blocks.sql:5`) and survives, so the junctions and only the
junctions start each boot empty. Pinned by a new deterministic unit red
`block_junction_schema_is_non_destructive_across_boots` (block_tags 0 where
1 was written, block_raw intact), green after removing the three drops.
UNPROVEN half: the org parser sets the doc-root Page tag itself
(`crates/holon-org-format/src/parser.rs:166` `document.set_page(true)` ->
`block.rs:400,410-416` -> `block_params.rs:71-76` ->
`sql_operation_provider.rs:680` edge_field_replace_sql), so any RE-INGESTED
file recovers its tag silently; only files whose ingest is SKIPPED by the
cold-boot fast path (`file_sync_controller.rs:1942-1966`, whose skip
predicate `content_present_in_all_stores` at `:871-883` certifies Loro tree
membership and says nothing about junction rows) would stay lost. That
combination is what Martins unchanged 348-block vault should hit, but FOUR
headless reboot cases could not reproduce it:
`page_tags_survive_reboot_over_existing_db`,
`loro_written_edge_fields_survive_reboot_over_existing_db` and even
`wiped_junction_is_repaired_on_next_boot` (explicit `DELETE FROM block_tags`
between boots) all PASSED on the unfixed tree — the harness repopulates on
boot 2 despite persisted `file.content_hash` values being present. So the
fix is proven necessary but NOT proven sufficient for the live symptom.
Breadcrumb causal link confirmed read-only: `QueryEngine::breadcrumb_trail`
joins `block_tags` (`crates/holon/src/api/query_engine.rs:162-166`) and
bails at `:194` with the exact banner text.

## Missing piece

The keystone SimulateRestart is a file-touch re-ingest, not a storage reboot
(`crates/holon-integration-tests/src/pbt/transitions/simulate_restart.rs`,
self-documented F9 fork) — it never drops+reopens the Turso handle, so a
boot-time DROP is structurally invisible to it. Deeper ENV gap: whether the
`stop_app`/`start_app` harness takes the cold-boot skip is UNRESOLVED — a
byte-identical `block_raw.updated_at` across the reboot cannot discriminate
(when store and disk agree, no write occurs whether or not ingest runs), and
an SQL-only content corruption was overwritten from the org file 2/2, so on
present evidence the harness more likely RE-INGESTS. The non-ingest repair
mechanism (Page-tag document-registry watch emptied →
`find_by_parent_and_name` miss → `create_forcing_id` re-creation) stands as
a verified code argument, not a harness measurement; in-harness it is
empirically inseparable from re-ingest because a wiped junction makes the
tagged root differ either way. Sound-discriminator recipe on task #87.
ORACLE secondary: no invariant asserted that junction rows survive a boot.

## Remedy

PARTIALLY FIXED 2026-07-31 — (a) the destructive DROP is removed (only the
legacy `task_blockers` name still drops), covered red-first; (b) a
CONDITIONAL SHAPE MIGRATION replaces what the drop was doing by accident:
the drop was this crate's ONLY migration mechanism (no `user_version` /
`schema_version` / migration table exists anywhere in holon-turso), and it
alone delivered the 2026-07-22 removal of the ingest-aborting FK on the
junction TARGET column, so removing it would have stranded pre-07-22
databases on that FK and regressed the P0 file-ingest-abort data loss.
`migrate_junction_dropping_target_fk` sniffs the stored DDL (the
`HistorySchemaModule` pattern) and rebuilds the table without the target FK,
COPYING every row, disclosed at WARN; pinned by
`pre_target_fk_junction_is_migrated_preserving_rows` (A/B-verified red: with
the migration disabled the old FK survives). BACKFILL DELIBERATELY NOT
BUILT: `wiped_junction_is_repaired_on_next_boot` shows the rows return
across a reboot; whether that reboot skipped ingest is unresolved (see the
ENV-gap note), so "self-repairs without re-ingest" rests on the code
argument. The backfill stays unbuilt because it would fire on the same
condition (missing derived state) the existing repair already triggers on —
racing it, not adding coverage. STILL OPEN and this is the real remainder:
the live 0-row state is therefore NOT explained by the DROP alone, since the
same wipe self-heals in the harness. Something about the real vault (scale,
Loro doc state, or the quarantined-file path) defeats that self-repair, and
it is unidentified. Do not treat #71 as closed.
