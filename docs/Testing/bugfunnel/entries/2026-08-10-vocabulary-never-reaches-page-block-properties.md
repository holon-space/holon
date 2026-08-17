---
id: 2026-08-10-vocabulary-never-reaches-page-block-properties
date: 2026-08-10
gap: ENVIRONMENT
secondary: ORACLE
status: UNCLASSIFIED
summary: >-
  The `#+TODO:` vocabulary never reaches the page block's properties on a real
  org ingest, so `SqlTaskVocabularySource` returns the DEFAULT vocabulary for
  every genuine document — silently disabling BOTH task #68's promotion fix
  and task #79's cycle fix in production.
source_line: 1197
---

## Bug

(found by MEASUREMENT while building the task #79 cold-boot re-ingest leg —
probing a real ingested document, not by a failing test) **The `#+TODO:`
vocabulary never reaches the page block's properties on a real org ingest,
so `SqlTaskVocabularySource` returns the DEFAULT vocabulary for every
genuine document — silently disabling BOTH task #68's promotion fix and task
#79's cycle fix in production.** Probe: after a full ingest of a file whose
header declares `#+TODO: NEXT WAITING \ | DONE`, the page row reads
`block:page-errands, properties = Object({})`. Every test that exercises the
vocabulary — #68's S2/S3 and #79's whole suite — seeds `todo_keywords` by
hand via `set_field`, so the entire vocabulary feature is proven only
against hand-seeded state and has NO coverage of the path that would deliver
it in production. The wiring appears to exist
(`FileSyncController::ingest_file` → `sync_document_metadata` →
`doc_manager.update_metadata`,
`crates/holon-filesystem/src/file_sync_controller.rs:2666-2674`; persistence
seams at `turso_seams.rs:632-643` / `loro_seams.rs:278`), so something
downstream drops it. MECHANISM, verifier-proven end to end: **the write
FIRES and the break is DOWNSTREAM of it.**
`holon-org-format/src/models.rs:371-372` returns `None` when the property is
absent, so the `parsed_kws != persisted.todo_keywords()` guard at
`crates/holon-orgmode/src/file_format.rs:143-145` DOES fire and
`doc_manager.update_metadata(&doc)` IS called
(`file_sync_controller.rs:2666-2674`). An earlier version of this row blamed
a defaulting getter at that guard; that was REFUTED and is recorded here
only so the next reader does not re-derive it. ROOT CAUSE, now PROVEN and
fixed: **a MODE/AUTHORITY mismatch — the write landed correctly and was then
DELETED ~10ms later.** `LiveDocumentManager::update_metadata`
(`crates/holon-app/src/turso_seams.rs`) wrote doc-level metadata straight to
SQL through a hand-constructed `SqlOperationProvider`, but under the default
wiring Loro is the sole authority for block columns, so the next Loro→SQL
projection reverted it. Captured at `prepare_update`: one statement writes
`properties = '{"ID":…,"todo_keywords":"[{\"keyword\":\"NEXT\"…}]"}'` on
`block:page-errands`, and the next writes `properties = '{}'` on the same
row, arriving via `loro_sync_controller.rs:1077 on_loro_changed` ←
`consolidator.rs:139` ← `file_sync_controller.rs:2306 ingest_file`. The
projector's own removal-sentinel diff (`block_diff_params`,
`loro_sync_controller.rs:1821-1834`) logged `old_props={…todo_keywords…}
new_props={}` — `old` is the SQL-derived base, `new` is the Loro tree, and
since Loro never received the vocabulary the projector correctly deleted it.
The module doc states there is no SQL→Loro direction and no inbound EventBus
consumer; the `EventOrigin::Org` comment in `update_metadata` referred to an
inbound gate that no longer exists. EVERY candidate this row previously
listed was FALSIFIED by the dump and is recorded as such so the next reader
does not re-walk them: the page row had `page_tags = 1` with its parent
chain intact (no row mismatch, no cleared `Page` junction),
`partition_params` produced the correct merged JSON, read and write both
target `block_raw`, and `update_metadata`'s own null-sentinel loop behaved
correctly — a DIFFERENT null-sentinel loop, the projector's, did the
deleting. The orchestrator's ranking put "properties landed somewhere, the
read looks elsewhere" first; the truth was "properties landed, then were
deleted". Second-order consequence seen in the same probe: a write-back of
such a document would re-render WITHOUT its `#+TODO:` header.

## Missing piece

the vocabulary source is exercised only against hand-seeded `todo_keywords`;
no test ingests a real `#+TODO:` file and then asserts the vocabulary the
ENGINE resolves for a block in it — so test and prod differ at precisely the
seam the feature depends on. Secondary ORACLE: nothing asserts that a
document's declared vocabulary survives ingest at all

## Remedy

P1 — CONFIRMED by a fresh-context verifier end to end (real
`FileSyncController` ingest -> page row `properties = Object({})` ->
vocabulary source falls back to `::default()` -> cycle writes the
inadmissible `TODO`), no longer a hypothesis. SEVERITY MULTIPLIER, stated
plainly: this does not merely add a bug, it means two shipped "fixes" for
the #67 data-mutation class were INERT in the field, and both were green
only because every vocabulary test hand-seeded `todo_keywords` via
`set_field` — a hand-seeded green was itself the escape. FIXED 2026-08-10 as
the completing increment of the #79 lane: `update_metadata` now routes
through `BlockOrdering::update_in_tree` — the seam its own doc calls "the
single org→block write seam for mutations" — injected from DI, so it is
Loro-first under `Consolidator::Upstream` and direct SQL under `Store`; the
null removal sentinels still work (they reach `write_field` → Loro meta →
the projector's removal path), and `parent_id` is stripped from the params
so a `#+TODO:` change can never decode as a re-parent intent. RED-FIRST:
`crates/holon-integration-tests/tests/task_vocabulary_reaches_the_store.rs`
ingests through the REAL `FileSyncController` and asserts at
`SqlTaskVocabularySource::vocabulary_for_block` — the SOURCE's own seam, not
a hand-picked row, deliberately, so the test is immune to the row-identity
confusion that dominated the initial diagnosis — then drives the real
`cycle_task_state`. TWINNED ACROSS LORO AND SqlOnly, because the mode axis
is exactly where the bug lived and a single-mode test would let the fix
regress the other half (red log `lane-logs/f3-prod-path-red.txt`: `left:
["TODO","DOING","LATER","NOW"] / right: ["NEXT","WAITING"]`). DE-SEEDED: the
cold-boot suite's hand-seed is DELETED and it is green fully un-seeded; the
pure-engine suite keeps its seed, now labelled a unit-level stand-in that
names the prod-path test as the real proof. GAP NOT CLOSED, flagged:
`sync_document_metadata` syncs the doc-root BODY and `#+TITLE:` through the
same call, so those were being reverted identically — the fix repairs them
too, but no test pins them.
