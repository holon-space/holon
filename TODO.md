# TODO: Holon Implementation Roadmap

## Cleanup
- [ ] DRY code, especially tests
- [ ] Actually get available widget names from Registry in Flutter and Blinc
  - [ ] It should actually check if `will_widget_fit_on_screen`
- [ ] Improve error handling and logging
- [ ] Refactor code to use more idiomatic Rust practices

## Bugs Found (2026-07-05) — GPUI Dogfooding Triage

Full root-cause analysis with file:line citations: `devlog/2026-07-05-011500-gpui-dogfood-triage.md`.
Plan: lock each bug in as a failing PBT/test FIRST, then fix.

### PBT coverage gaps (do these first — they gate everything else)
- [ ] Register the seeded `left_sidebar::src::0` watch as a `RefWatch` in `transitions/start_app.rs` ref-apply so `inv-watch-rows-match-ref` covers the sidebar for free
- [ ] Add a page-delete transition (new `delete_document.rs` or lift `!is_page()` filters in `apply_mutation.rs:78,96,122`); ref model must cascade the subtree and drop the page from the expected Page set
- [ ] Keystone invariant: focused page's title row precedes the virtual "add block" child; title carries `role=page_title`
- [ ] Unit PBT: virtual-child sort key sorts after ANY FractionalIndex key (kills the `Float(f64::MAX)` → `"18442240…"` lexicographic bug class)

### Stale left sidebar after page deletion
- **Status**: Open; keystone blind to it (gaps above)
- **Causes (ranked)**: (H1) Turso chained-matview stale rows on delete — repro `crates/holon/examples/turso_ivm_chained_matview_stale_rows.rs`, needs upstream fix via /turso-fix; (H2) `block_tags` orphans — FK CASCADE declared but Turso FK enforcement off, `prepare_delete` skips junction tables; (H3) rowid-keyed CDC deletes no-op in `ReactiveRenderedRows::apply` (`reactive.rs:491-495`, no rowid fallback unlike `LiveData`)
- [ ] Fix H3 (rowid fallback) + H2 (cascade `block_tags` in `prepare_delete` or `PRAGMA foreign_keys=ON`); H1 upstream

### "Type here to add a new block" above page title; title not h1
- **Status**: Open, fully root-caused
- **Causes**: `collection_profile.yaml` `tree_view` lacks the `rules:` arg setting `role=page_title`; streaming tree driver evaluates rules with empty positional map and DISCARDS overrides (`reactive_view.rs:863-876`); virtual child parented to `block:default-main-panel` (never in rowset → root sibling) with `Float(f64::MAX)` sort key that string-sorts before FI hex keys
- [ ] `"\u{10FFFF}"` sentinel sort key + parent virtual row to focus-root page (cheap)
- [ ] Thread depth + rule overrides through the streaming tree driver (full)

### Multi-second interaction latency
- **Status**: Open; suspect identified, needs `snapshot_ms` (debug log) confirmation
- **Suspect**: `LoroProjection::project()` full-doc DFS snapshot + diff on EVERY Loro commit under global `project_lock` (`loro_sync_controller.rs:349-388`); amplified by 50ms-quiet/5s-ceiling settle (`turso_block_query_source.rs:72-73`)
- [ ] Confirm, then de-risk incremental projection (diff from Loro event deltas) with an experiment before refactoring
- [ ] Also: RSS grew +426MB in ~15min light use — leak check

### GitHub page shows no data
- **Status**: Root-caused; needs a design decision
- **Cause**: page queries read base `gh_*` tables; integration yaml is vtable-only (no sync) → base tables fill ONLY via lazy writeback when `gh_*_fdw` is queried, which nothing does. One manual FDW SELECT wrote 95 rows to `gh_repository`.
- [ ] Decide: page queries via `*_fdw` (≈5s network at render) vs declarative poll/materialization in yaml sidecar (generic-MCP phase 3)

### App freeze clicking some pages (e.g. Holon)
- **Status**: Open, unconfirmed suspect
- **Suspect**: page-open `CREATE MATERIALIZED VIEW watch_view_*` chained on `block` matview → known Turso matview-on-matview DDL hang; log shows same-family failure for `block:cc-conversation`
- [ ] Capture `sample Holon 3` during next freeze
- [ ] Guard: watch-view DDL timeout + loud error instead of indefinite block (integration test)

## Bugs Found (2026-02-27) — Custom Property Stripping

### Source Block Custom Properties Lost in Org Round-Trip
- **Status**: Open, PBT reproducer in `general_e2e_composed_pbt.rs`
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
* Implement OperationDispatcher
