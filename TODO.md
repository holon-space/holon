# TODO: Holon Implementation Roadmap

## Cleanup
- [ ] DRY code, especially tests
- [ ] Actually get available widget names from Registry in Flutter and Blinc
  - [ ] It should actually check if `will_widget_fit_on_screen`
- [ ] Improve error handling and logging
- [ ] Refactor code to use more idiomatic Rust practices

## Bugs Found (2026-04-11) — GPUI Startup

### index.org Infinite Re-Processing Loop
- **Status**: Open
- **Symptom**: Every ~2s after startup: `[OrgSyncController] Duplicate block IDs across files! File index.org contains block IDs that already exist in other documents: 21 conflicts` followed by `[on_file_changed] CDC cache did not catch up within 2s for index.org (expected 23 blocks, cache has 2)`
- **Impact**: Infinite loop of re-processing, wasted CPU, never converges
- **Reproduction**: `rm -f ~/.config/holon/holon.db* && cargo run -p holon-gpui` — happens on every fresh DB startup
- **Key observations**:
  - First pass processes index.org fine (23 blocks inserted)
  - Subsequent passes detect 21 "duplicate block IDs" — blocks from index.org that already exist in other documents (likely the child documents that were processed first)
  - CDC cache reports only 2 blocks when 23 are expected — the QueryableCache subscription may be filtering to a subset
  - The error triggers a retry, which sees the same duplicates, triggering another retry, forever
- **Where to look**:
  - `crates/holon-orgmode/src/org_sync_controller.rs` — `Processing external change` path and duplicate detection logic
  - `crates/holon-orgmode/src/di.rs` — `on_file_changed` CDC wait logic, the "expected N blocks, cache has M" check
  - The root issue is likely that index.org shares block IDs with child documents (by design — document identity aliasing), but the duplicate check doesn't account for this

### shadow_index.rs Index Out of Bounds
- **Status**: Open
- **Symptom**: `index out of bounds: the len is 1377 but the index is 1379` at `crates/holon-frontend/src/shadow_index.rs:220`
- **Trigger**: Happens during `bubble_input` when navigating — the `nodes` vec is being indexed by a stale idx that was valid before a structural rebuild removed/reindexed nodes
- **Impact**: Panic on main thread, kills the app (double-panic because it's in a function that cannot unwind — likely a GPUI callback)
- **Where to look**:
  - `crates/holon-frontend/src/shadow_index.rs:220` — `let node = &self.nodes[idx]` in `bubble_input`
  - The `entity_index` HashMap maps entity_id → node index, but these indices become stale when the `nodes` Vec is rebuilt. Either the index needs to be rebuilt atomically, or lookups need bounds-checking

## Bugs Found (2026-02-27) — Custom Property Stripping

### Source Block Custom Properties Lost in Org Round-Trip
- **Status**: Open, PBT reproducer in `general_e2e_pbt.rs`
- **Issue**: Custom properties set on source blocks (content_type=Source) are lost during org sync round-trip
- **Root Cause**: `source_block_to_org()` in `models.rs` doesn't render custom properties — source blocks in org format (`#+BEGIN_SRC/#+END_SRC`) have no `:PROPERTIES:` drawer
- **Impact**: Properties like `column-priority` set programmatically on source blocks get stripped
- **Fix options**: (a) Don't overwrite SQL properties with org-parsed properties for source blocks, (b) Store custom props in source block header args

### SqlOperationProvider `update` Replaces Properties Instead of Merging
- **Status**: Open
- **Issue**: `sql_operation_provider.rs:430-433` — `update` operation does `SET "properties" = '{new_json}'` which replaces entire column
- **Impact**: If a block has `{"column-order": "1"}` and you update with `{"collapse-to": "2"}`, old `column-order` is lost
- **Fix**: Use `json_patch` or read-modify-write to merge new properties with existing

### Source Block Ordering Bug During Initial Sync
- **Status**: Open, workaround in `assertions.rs` (skip ordering check for source-only groups)
- **Issue**: OrgRenderer reorders source block siblings during initial file sync round-trip
- **Impact**: Source block order (e.g., `::src::1` before `::src::0`) can differ from expected after sync

### `_source_header_args` Leaks Into Drawer Properties
- **Status**: Fixed — added to `INTERNAL_KEYS` in `models.rs`
- **Issue**: `_source_header_args` property was not in `INTERNAL_KEYS`, so it appeared in `drawer_properties()` and `format_properties_drawer()`

## Bugs Found (2025-12-28)

### Schema Migration: Missing Columns in Blocks Table
- **Status**: Fixed manually, needs permanent solution
- **Issue**: The `blocks` table was missing columns added to the Block struct: `source_name`, `source_header_args`, `source_results`
- **Impact**: Silent insert failures during org sync - blocks weren't being created
- **Root Cause**: SQLite's `CREATE TABLE IF NOT EXISTS` doesn't add new columns to existing tables
- **Fix Applied**: Manual `ALTER TABLE ADD COLUMN` for each missing column
- **TODO**: Add schema migration logic to detect and add missing columns automatically

### Nested Headline Properties Not Syncing
- **Status**: Open
- **Issue**: Properties on nested org headlines (level 2+) are not being synced to the database
- **Example**: "All Documents" headline has `:REGION: left_sidebar` and `:VIEW: query` in org file, but `properties: {}` in database
- **Impact**: Regions can't be configured via org file properties
- **TODO**: Investigate parser or sync provider to fix property extraction for nested headlines
* Property-Based Tests for QueryableCache / Fake
  * I would say that QueryableCache is responsible for making sure offline-mode works as if you were online, so it needs to:
    1. Store all operations so they can be executed once one is online.
    2. Run the operations against a fake and the real system in parallel
    3. Take the result of the fake if the real system takes longer to respond
    4. Throw away the result of the fake once the real system responds
    5. Maintain a mapping
  * Take one fake as the Fake
  * Wrap an actual SourceSystem a mock
    * mock so we can easily simulate what happens if the source system
      * denies a change
      * is not available for a longer period
      * returns something conflicting
    * https://crates.io/crates/mry looks good as mock library
  * Test that the result of using Fake is equivalent to using SourceSystem after sync
  * Also allow wrapping another Fake in a mock as the SourceSystem
    * Allows running tests in case of rate limits
    * Does not test that the fake is implemented correctly, but that fake+cache behave the same way as fake alone
