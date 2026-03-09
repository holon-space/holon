# Handoff: blocks_by_document BFS fix + keyword echo loop

## What was done

### Bug 1: `blocks_by_document` BFS swallows sibling documents (FIXED)
**File**: `crates/holon-api/src/block.rs`

The `__default__` document block has `id = sentinel:no_parent`. All root document blocks have `parent_id = sentinel:no_parent`. When `__default__` was processed first in BFS, it claimed all other document blocks and their content as descendants.

**Fix**: Skip blocks with `is_document() = true` during BFS traversal. Document blocks form their own groups and should never be claimed as children of another document.

### Bug 2: Org keyword echo loop (FIXED)
**Files**: `org_utils.rs`, `test_environment.rs`, `state_machine.rs`, `sut.rs`

`serialize_blocks_to_org` and `apply_external_mutation` wrote org files without `#+TODO:` headers. Non-default keywords like WAITING weren't recognized on re-parse, causing content corruption (`* WAITING content` → parser sees `WAITING content` as title → re-render adds another WAITING → exponential growth).

**Fixes**:
- New `serialize_blocks_to_org_with_doc()` renders document header when doc block provided
- `WriteOrgFile`, `CreateDocument`, `StartApp` set `todo_keywords` on document blocks from the keyword set
- `apply_external_mutation` passes doc block for header rendering
- `wait_for_org_file_sync` uses `render_document` (with header) instead of `render_entitys`
- `wait_for_org_file_sync` expected_blocks now resolve block IDs via `doc_uri_map` (so document block IDs match UUID-keyed documents)
- Added `todo_keywords` to `INTERNAL_PROPS` (stripped during block comparison)

## Current blocker: `wait_for_org_file_sync` block count off by 1

Every proptest case fails with:
```
[wait_for_org_file_sync] WARNING: Org file ".../index.org" block count mismatch: actual=4 expected=5
```

Always `index.org`, always exactly 1 extra in expected. The 5th block needs to be identified.

### Hypotheses (ordered by probability)

1. **The `__default__` seed doc block's descendants leak into the index.org group**.
   After `doc_uri_map` resolution, some seed block's `parent_id` might resolve to the same UUID as the WriteOrgFile document, causing `blocks_by_document` to count it under index.org. Check: print all blocks grouped under the index.org UUID and compare with what's in the org file.

2. **`expected_blocks` resolution creates a collision**.
   The `b.id = self.doc_uri_map.get(&b.id)` at sut.rs:3161 resolves block IDs. If a seed block and a WriteOrgFile block share the same synthetic ID (unlikely but possible), one overwrites the other in the Vec, changing the count.

3. **A mutation creates a block under index.org that hasn't synced to the file**.
   Check the transition sequence before the count mismatch to see if ApplyMutation::Create adds a block to the index.org document.

### Debug approach
Add logging to `wait_for_org_file_sync` to print the block IDs in the expected group vs what's parsed from the file. The 1-off is very specific — identifying the phantom block will immediately reveal the cause.

## Files changed (summary)

| File | Change |
|------|--------|
| `crates/holon-api/src/block.rs` | BFS skips document blocks |
| `crates/holon-integration-tests/src/org_utils.rs` | `serialize_blocks_to_org_with_doc()`, `todo_keywords` in INTERNAL_PROPS |
| `crates/holon-integration-tests/src/test_environment.rs` | Doc header in `apply_external_mutation` + `wait_for_org_file_sync`, debug mismatch logging |
| `crates/holon-integration-tests/src/pbt/state_machine.rs` | `todo_keywords` on doc blocks (WriteOrgFile, CreateDocument, StartApp) |
| `crates/holon-integration-tests/src/pbt/sut.rs` | BulkExternalAdd uses doc header; ID resolution in sync wait blocks |
| `crates/holon-integration-tests/src/pbt/transition_budgets.rs` | Budget increases (Create reads 40→70, single_query 100→250ms) |

## Test results

| Run | Duration | Result | Notes |
|-----|----------|--------|-------|
| Before fix | ~400s | FAIL | `blocks_by_document` returns 0 for target doc |
| After BFS fix only | ~600s | FAIL | `inv13` budget violation (Create reads) |
| + budget increase | 4300s (SIGKILL) | Running | No failures observed, killed manually |
| + keyword fix | 36s | FAIL | `todo_keywords` in block comparison |
| + INTERNAL_PROPS | 465s | FAIL | `inv13` budget (single_query 190ms) |
| + budget + ID resolve | 490s | FAIL | `inv13` baseline regression |
| HOLON_PERF_BUDGET=0 | 383s | FAIL | Block count off by 1 in org sync |
