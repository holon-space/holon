---
id: 2026-07-22-less-external-edit-ingest-duplicates-churns
date: 2026-07-22
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  ID-less external-edit re-ingest duplicates / churns block identity (full
  triage in un-landed PR #76 / bookmark `bugfunnel-idless-dup`; this is the
  FIXED companion row). An external editor bulk-writes ID-less org headlines
  while the app runs → the app mints a fresh `Uuid::new_v4()` per headline on
  EVERY parse (`parser.rs::extract_or_generate_id`) and writes `:ID:` back; a
  SECOND external write of the stale pre-mint (still ID-less) text re-ingests,
  and because `FileSyncController::ingest_file` keyed UPDATE-vs-CREATE by
  block-id ONLY, each re-parsed headline got a NEW uuid, missed the by-id
  `old_blocks` lookup, and re-minted — the headline's identity CHURNED (all
  references break), and under a concurrent writeback (diff base desynced from
  the store via the TOCTOU write-skip) the old twin survived → the block
  DUPLICATED under two ids (~60 at live-vault scale; blocks that already
  carried `:ID:` never duplicated). Likely a mechanism behind Martin's
  recurring "duplicate content" reports.
source_line: 797
---

## Bug

ID-less external-edit re-ingest duplicates / churns block identity (full
triage in un-landed PR #76 / bookmark `bugfunnel-idless-dup`; this is the
FIXED companion row). An external editor bulk-writes ID-less org headlines
while the app runs → the app mints a fresh `Uuid::new_v4()` per headline on
EVERY parse (`parser.rs::extract_or_generate_id`) and writes `:ID:` back; a
SECOND external write of the stale pre-mint (still ID-less) text re-ingests,
and because `FileSyncController::ingest_file` keyed UPDATE-vs-CREATE by
block-id ONLY, each re-parsed headline got a NEW uuid, missed the by-id
`old_blocks` lookup, and re-minted — the headline's identity CHURNED (all
references break), and under a concurrent writeback (diff base desynced from
the store via the TOCTOU write-skip) the old twin survived → the block
DUPLICATED under two ids (~60 at live-vault scale; blocks that already
carried `:ID:` never duplicated). Likely a mechanism behind Martin's
recurring "duplicate content" reports.

## Missing piece

No keystone transition models EXTERNAL editor write → app writeback → stale
external re-write → re-ingest, so the triggering sequence is ungeneratable
(COVERAGE); the OS-filewatch re-ingest wiring +
app-running-concurrently-with-an-editor timing is prod-only (ENVIRONMENT).

## Remedy

FIXED 2026-07-22 (PR #76, this change). `ingest_file` now, when the parse
yields ID-less headlines (`FileFormatParseResult::blocks_needing_ids`),
reconciles each onto its already-minted twin by exact CONTENT + sibling
POSITION under the same parent BEFORE the by-id diff, remapping the fresh
uuid to the existing id so the stale re-write reconciles in place (id stays
stable, no duplicate). **Deviation from the literal triage remedy** (which
said "add a content/position fallback to the by-id `old_blocks` lookup"):
matching `old_blocks` does NOT fix the DUPLICATE, because in the duplicating
case the diff base is exactly what desynced — it holds throwaway ids that
match neither the real twin nor the new mint. The fix instead matches the
STORE's CURRENT children (`block_reader.get_blocks`, ground truth) via the
pure `compute_idless_remaps`. Positional 1:1 (two identical ID-less siblings
stay two blocks); a content match at a different position is disclosed via
WARN and left to mint. Files:
`crates/holon-filesystem/src/file_sync_controller.rs`. Red-first: pure-fn
unit tests (`mod idless_reconcile_tests`) + integration
id-stability/no-merge pins
(`crates/holon-integration-tests/tests/idless_external_reedit_dup.rs`).
Note: the single-threaded ingest path CHURNS identity but the delete pass
cleans up the orphan (no visible dup) — the observable DUPLICATE
additionally needs the concurrent-writeback base desync, so its keystone
rung (external-write → app-writeback-skip → stale-rewrite) is the OPEN
follow-up to make the keystone go red on the dup itself.
