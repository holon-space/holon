# Handoff: Investigate ConcurrentMutations PBT Failure

## Problem

The general E2E PBT (`just pbt general 1`) fails during `ConcurrentMutations` transitions — specifically same-block concurrent content updates where UI appends a suffix and External prepends a prefix.

### Observed failure

```
UI=Update { id: "block-0", fields: {"content": "pCFUz UR 3PX  U Lufqyptdn"} }
External=Update { id: "block-0", fields: {"content": "Znqsjdcj pCFUz UR 3PX  U"} }

Backend has:   "pCFUz UR 3PX  U Lufqyptdn"     (only UI suffix, external prefix lost)
Expected:      "Znqsjdcj pCFUz UR 3PX  U Lufqyptdn"  (both prefix and suffix merged)
```

The Loro CRDT merge drops the external mutation's prefix entirely.

### Secondary symptom

`org_sync_controller.rs:256` panics because `render_file_by_doc_id` returns empty after creating blocks — likely a consequence of the same concurrent mutation timing issue.

## Architecture context

The ConcurrentMutations flow in the SUT (`apply_concurrent_mutations` at `general_e2e_pbt.rs:3116`):
1. External mutation fires FIRST — writes to the org file on disk
2. UI mutation fires immediately after — calls `execute_op` through the engine
3. Single sync barrier waits for expected block count
4. Waits for external_processing + write windows to expire

The reference model (`general_e2e_pbt.rs:1694`) uses `loro_merge_text()` to simulate CRDT merge:
- Creates a common Loro ancestor with original content
- Peer A (UI) applies `LoroText::update(ui_content)`
- Peer B (External) applies `LoroText::update(ext_content)`
- Merges peers and reads result

### Known FIXME

`general_e2e_pbt.rs:3119-3121`:
```
// FIXME: external mutation should be applied from pre-merge state for true concurrency testing.
// Currently, the external mutation is applied from the post-both-mutations reference state,
// which means CRDT conflict resolution is never actually tested.
```

The external mutation uses `ref_state.blocks` which already has the **merged** content from the reference model, not the pre-merge content. So the org file written to disk contains the expected merged result, not the raw external content.

## Hypotheses (ordered by probability)

### H1: External mutation writes merged content instead of raw external content (HIGH)
The `apply_concurrent_mutations` passes `ref_state.blocks` to `apply_external_mutation`. But at this point, the reference model has ALREADY applied the CRDT merge (reference `transition` runs before SUT `apply`). So the org file gets the merged content, not the raw external content. The real system would have two independent writes: UI through Loro, External through the org file with only the external content.

**Validate**: Add logging in `apply_concurrent_mutations` to print the content being written to the org file. Compare with the raw `ext_event.mutation` fields.

### H2: Loro CRDT merge in production differs from `loro_merge_text` simulation (MEDIUM)
The test's `loro_merge_text` creates fresh peers and merges them. But in production, the UI mutation goes through `LoroBlockOperations::update()` which calls `LoroText::update()` on an existing document, and the external mutation goes through `OrgSyncController` which parses the org file and updates Loro. The merge semantics might differ because the production path doesn't create independent peers.

**Validate**: Add `tracing` or eprintln in `OrgSyncController::apply_external_changes` to log the content it writes into Loro for the updated block. Compare with what `loro_merge_text` predicts.

### H3: Timing issue — external write not visible when UI mutation triggers sync (LOW-MEDIUM)
Even though external fires first, the OrgSyncController's file watcher might not have picked up the change before the UI mutation's `on_block_changed` fires and re-renders the org file, overwriting the external content.

**Validate**: Check timestamps in log output. The `[ConcurrentMutations] Firing External mutation first` and the subsequent UI mutation should show whether the external file was processed before the UI mutation.

## Key files

- **PBT test**: `crates/holon-integration-tests/tests/general_e2e_pbt.rs`
  - Strategy generation: lines 1261-1319 (same-block concurrent edits)
  - Reference model: lines 1663-1739 (`ConcurrentMutations` transition)
  - SUT apply: lines 3114-3200 (`apply_concurrent_mutations`)
  - `loro_merge_text`: lines 78-110 (CRDT merge simulation)
- **Assertions**: `crates/holon-integration-tests/src/assertions.rs` (normalize_block, assert_blocks_equivalent)
- **OrgSyncController**: `crates/holon-orgmode/src/org_sync_controller.rs:256` (render empty assertion)
- **Test infrastructure**: `crates/holon-integration-tests/src/test_environment.rs` (apply_external_mutation)

## How to reproduce

```bash
# Single case is enough — ConcurrentMutations triggers reliably
PROPTEST_CASES=1 cargo test -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt -- --nocapture 2>&1 | tee /tmp/pbt-general.log

# Or use justfile
just pbt general 1
```

The test takes several minutes because after the failure, proptest shrinks (up to 200 iterations, each starting the full app). To skip shrinking during investigation, temporarily set `max_shrink_iters: 0` in the proptest config at line 3236.

## Recent fix applied

`INTERNAL_PROPS` in `crates/holon-integration-tests/src/org_utils.rs` was updated to include `"created_at"` and `"updated_at"`. Before this fix, every test case failed immediately because these properties leaked from the DB into the properties map but weren't in the reference model. This masked the ConcurrentMutations bug.

## Profiling

If you need to profile a stuck/slow PBT:
```bash
just sample-pbt general    # Auto-finds child process, captures stack traces
just profile general       # Full samply profile with Firefox Profiler UI
```
