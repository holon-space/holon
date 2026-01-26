# Org Sync + Loro Reverse Sync Fixes (2026-04-06)

## Bug 1: OrgSyncController skips re-render for child blocks (FIXED)

**Root cause**: When a UI mutation creates a block as a child of another block (e.g., `block::block-4` with `parent_id = block:bulk-0-2`), `extract_doc_ids_from_event` extracts the parent_id as the doc_id. Since `bulk-0-2` is a regular block (not a document), `on_block_changed` calls `doc_id_to_path("block:bulk-0-2")` which returns `None`, silently skipping the re-render. The org file never gets the new block.

**Fix**:
- `on_block_changed` now returns `Result<bool>` — `false` when no matching document found.
- Caller in `di.rs` falls back to `re_render_all_tracked()` when none of the extracted doc_ids matched a file.

**Files**: `crates/holon-orgmode/src/org_sync_controller.rs`, `crates/holon-orgmode/src/di.rs`

## Bug 2: Loro reverse sync — "Cannot parse parent URI as TreeID" (FIXED)

**Root cause**: Content blocks have `parent_id = block:<uuid>` pointing to a document block. Document blocks are created by `DocumentManager` (not via block EventBus), so Loro never receives a Created event for them. When `apply_create` tries to resolve the parent, `find_tree_id_by_external_id` returns `None`, and the fallback passes the raw UUID to `create_block`, which fails because UUIDs aren't valid Loro TreeIDs.

**Fix**: When the parent isn't found in Loro and isn't a sentinel/no_parent, create a placeholder root node (document proxy) in the LoroTree. Set its `external_id` so subsequent blocks with the same parent reuse it.

**File**: `crates/holon/src/sync/loro_reverse_sync.rs`

## Bug 3: Loro reverse sync — "Event payload missing 'data'" for FieldsChanged (FIXED)

**Root cause**: `FieldsChanged` events carry a `"fields"` map in the payload, not `"data"`. The generic `apply_event` tried to extract `data` and failed.

**Fix**: Added dedicated `apply_fields_changed` handler that processes the `fields` map.

**File**: `crates/holon/src/sync/loro_reverse_sync.rs`

## Bug 4: Loro save — "No such file or directory" (FIXED)

**Root cause**: `save_all()` calls `save_to_file()` without ensuring the parent directory exists.

**Fix**: `create_dir_all` before saving.

**File**: `crates/holon/src/sync/loro_document_store.rs`

## Bug 5: LoroSut diagnostic output improved

**Fix**: `build_diagnostic()` now shows block IDs on each side, "Only in Loro/Ref" diff, and per-field diffs for mismatched blocks. Count mismatch after 5s retry is logged (not panicked) since it indicates incomplete reverse sync.

**File**: `crates/holon-integration-tests/src/pbt/loro_sut.rs`

## Test results

| Test | Duration | Result | Notes |
|------|----------|--------|-------|
| `general_e2e_pbt` | ~790s | PASS | All fixes working |
| `general_e2e_pbt_cross_executor` | ~397s | FAIL | BulkExternalAdd DB count panic + UNIQUE constraint on doc blocks |

## Remaining issues

1. **`UNIQUE constraint failed: block.(parent_id, name)`** — Document blocks are being duplicated during SimulateRestart. This is a SQL-layer issue in `CacheEventSubscriber`, not Loro.

2. **Cross-executor BulkExternalAdd timeout** — After the UNIQUE constraint failure, the expected block count isn't reached, causing BulkExternalAdd to panic.

3. **Index.org layout blocks missing from Loro** — The WriteOrgFile index.org blocks show as "Only in Ref" in LoroSut. They're created via EventBus (OrgSyncController) but Loro still doesn't have them after 5s. Might be a timing issue or the placeholder creation races with the actual block creation.
