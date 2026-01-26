# PBT TypeChars/DeleteBackward divergence — fixed (ref-model bug)

Date: 2026-05-09

## What landed

The carry-over `bulk-1-0 = "LM"` (prod) vs `"LM lX8G"` (ref) divergence
called out in the Phase 2 handoff
(`devlog/2026-05-09-175751-phase2-authority-flip-landed.md`) is fixed. It
was a reference-model bug, not a production bug.

## Root cause

The failing seed runs `BulkExternalAdd → FocusEditableText(bulk-1-0) →
PinBlock → DeleteBackward(count=4)`.

In **prod** (Phase 2 architecture):
- `bulk-1-0` ingested from the org file with `content = "LM lX8G"`.
- Each backspace dispatches `cell.apply_text_op(Delete)` → `LoroText`
  shrinks one char at a time.
- After 4 backspaces: `LoroText = "LM "` (3 chars, trailing space).
- `LoroSyncController.on_loro_changed` projects to SQL via
  `execute_batch_with_origin(Loro)` → `SqlOperationProvider::partition_params`
  applies `trimmed_content` → SQL `bulk-1-0.content = "LM"` (2 chars).

In **ref** (pre-fix):
- `DeleteBackward::apply_to_ref` mutated `active_editor.in_memory_content`
  in place but did NOT call `commit_active_editor_if_changed` — its
  comment "No commit during edit — see TypeChars apply_to_ref for
  rationale" was wrong: `TypeChars::apply_to_ref` DOES commit when
  `enable_loro` is true (Phase 1 contract).
- `block.content` for `bulk-1-0` stayed at `"LM lX8G"`.

Divergence: prod=`"LM"`, ref=`"LM lX8G"`.

## Fix

Two surgical changes:

1. **`crates/holon-integration-tests/src/pbt/transitions/delete_backward.rs`** —
   Added `state.commit_active_editor_if_changed()` after the
   `editor.delete_backward(self.count)` call when
   `state.variant.enable_loro`. Mirrors `TypeChars::apply_to_ref`.
   Updated the stale comment to explain the Phase 2 contract.

2. **`crates/holon-integration-tests/src/pbt/reference_state.rs`** —
   `commit_active_editor_if_changed` now passes `in_memory_content`
   through `super::types::normalize_content_for_org_roundtrip` (which
   mirrors `SqlOperationProvider::trimmed_content`) before writing to
   `block.content`. Without this, `"LM "` (3 chars) would commit verbatim
   while prod's SQL projection trims to `"LM"` (2 chars). Removed the
   no-op write-back of `in_memory` to `editor_mut.in_memory_content`
   that did nothing — the live editor buffer keeps trailing whitespace
   even after the persisted form trims it.

## Verification

| Check | Status |
|-------|--------|
| `cargo check --profile debugger -p holon-integration-tests --tests` | GREEN |
| `general_e2e_pbt` (Full mode, regression replay + 1 random) | ✅ PASS in 452s |
| `general_e2e_pbt_sql_only` (regression replay + 1 random) | ✅ PASS in 487s |

Pre-fix run (`/tmp/pbt-trace-run.log`) failed at the
`assert_blocks_equivalent` site at `sut.rs:3609` with the
`bulk-1-0 = "LM"` vs `"LM lX8G"` shape, exactly the carry-over from the
Phase 2 handoff.

## Notes for next time

The original handoff hypothesised production-side root causes
(`Weak`-keyed cache eviction races, async-vs-sync ordering, stale Loro
state). The actual bug was much simpler: the ref model's
`apply_to_ref` for `DeleteBackward` and the trim normalization in
`commit_active_editor_if_changed` were inconsistent with `TypeChars` and
with prod's SQL projection. The diagnosis path that worked: capture the
exact transitions from a trace-level run (the budget log lists every
`apply_to_*` call) rather than reasoning forward from architecture.

The same trim-divergence latently exists for `TypeChars` if a generator
ever produces a string ending in whitespace: pre-fix,
`commit_active_editor_if_changed` would store the untrimmed form. The
trim normalization landed today closes that latent class too.
