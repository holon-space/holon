---
id: 2026-07-17-pairing-selection-injection-env-default-keystone
date: 2026-07-17
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  F8 (pairing `inv-display-placement-canonical-inert` selection with its
  `HOLON_PBT_DISPLAY_PLACED` injection env → default keystone GREEN on that
  axis) UN-MASKED a deeper pre-existing keystone RED:
  `inv-history-records-all-creates/block_history` fails on
  `CreateBlockUnderFocus` — "block_history has 0 distinct op_group(s) but the
  oracle drove 1 UI create(s)". The production creation-slot commit path
  (`commit_creation_slot → handle_text_sync → block.create`) does NOT record a
  `block_history` op_group, while OTHER create paths DO (the invariant passes
  on split/ApplyMutation creates — engagement 24/24). Was invisible on base
  because proptest shrinks to the FIRST-reddening case: the display-placement
  (and before it, journals) baseline red aborted every full-frontend sequence
  at/near tick-0, so a `CreateBlockUnderFocus`-then-check sequence was never
  the minimal failure.
source_line: 997
---

## Bug

F8 (pairing `inv-display-placement-canonical-inert` selection with its
`HOLON_PBT_DISPLAY_PLACED` injection env → default keystone GREEN on that
axis) UN-MASKED a deeper pre-existing keystone RED:
`inv-history-records-all-creates/block_history` fails on
`CreateBlockUnderFocus` — "block_history has 0 distinct op_group(s) but the
oracle drove 1 UI create(s)". The production creation-slot commit path
(`commit_creation_slot → handle_text_sync → block.create`) does NOT record a
`block_history` op_group, while OTHER create paths DO (the invariant passes
on split/ApplyMutation creates — engagement 24/24). Was invisible on base
because proptest shrinks to the FIRST-reddening case: the display-placement
(and before it, journals) baseline red aborted every full-frontend sequence
at/near tick-0, so a `CreateBlockUnderFocus`-then-check sequence was never
the minimal failure.

## Missing piece

render/history invariants were unobservable in the standard gate while an
earlier baseline red short-circuited the shrink (the audit's P0
"baseline-RED masking")

## Remedy

RESOLVED 2026-07-23 (Martin ruling R3 = Option A: creation-slot must emit
the same `block_history` provenance as every other create). The
creation-slot path now records: `LoroBlockOperations::create` emits
`FieldDelta(id,"id",Null,minted-id)` (`loro_block_operations.rs:952`, landed
2026-07-22) exactly as `SqlOperationProvider::create` does
(`sql_operation_provider.rs:2577`), so the engine `record_history`
chokepoint (`operation_engine.rs:1268`) appends one op_group per slot
create. Regression lock: hand-authored keystone case
`slot-create-records-history-op-group` (a single `id:None
CreateBlockUnderFocus` = production slot gesture) —
`inv-history-records-all-creates` engaged + green; keystone-smoke green with
the invariant engaged 24/24+ across all sequences (it is a lower bound, so
mixed sequences always record enough). NOT introduced by F8 (F8 is correct:
display-placement now Skips when unset, engaged 4/4 when set). DISTINCT
RESIDUAL GAP (split out, backlog): watcher-ingested doc-page creates
(`CreateDocument` -> org file -> `FileSyncController` ingest -> block
materialized via the Loro->SQL consolidator `command_bus` =
`SqlOperationProvider` as `OriginTaggedWrites`, `loro_module.rs:159`) do NOT
pass through `DispatchingOperationEngine::execute_operation`, so
`record_history` never fires for them and they record no op_group.
`harness.rs:300` counts each `ref-doc-N->uuid` remap as a UI create
demanding >=1 op_group. Masked in mixed sequences by the lower bound;
reproducible ONLY in a degenerate all-doc-ingest sequence (2x
`CreateDocument`, no other recording create -> "block_history has 1 distinct
op_group(s) but the oracle drove 2 UI create(s) - 1 create(s) went
unrecorded"). Fix (deferred, not R3): route ingest doc creates through the
history-recording engine seam, or record the consolidator's projected
creates. Old repro (pre-fix): `HOLON_PBT_FORCE_FULL=1 PROPTEST_CASES=16
cargo test -p holon-integration-tests --features pbt --test
general_e2e_composed_pbt`. Separate finding same session: under
`HOLON_PBT_DISPLAY_PLACED=1` the injected display-placed node
(`block:parent` under `block:default-main-panel`) is picked up by
`inv-advice-rows-woven` as an unexpected woven row not in the ref
expectation — the Phase 1a injection seam is not inert w.r.t. the advice
oracle.
