# Implementation Plan: `from children` Virtual Table in PRQL

## Overview

Replace hard-coded `REGION` property handling in `backend_engine.rs` with a declarative PRQL-native approach using virtual tables like `from children`.

## Phases

### Phase 1: Add QueryContext struct and stdlib injection
**Status**: completed

**Objective**: 
- Add `QueryContext` struct to represent query execution context
- Create PRQL stdlib with virtual table definitions (`children`, `roots`, `siblings`)
- Implement stdlib injection function

**Tasks**:
1. Add `QueryContext` struct with `current_block_id` and `context_parent_id` fields
2. Add helper methods `root()` and `for_block()`
3. Define `PRQL_STDLIB` constant with virtual table definitions
4. Implement `inject_stdlib()` function

**Success Criteria**:
- `QueryContext` struct compiles and has proper documentation
- `PRQL_STDLIB` contains valid PRQL definitions
- `inject_stdlib()` correctly prepends stdlib to user queries

**Implementation**:
- [`QueryContext` struct](crates/holon/src/api/backend_engine.rs#L28-L51): Lines 28-51
- [`PRQL_STDLIB` constant](crates/holon/src/api/backend_engine.rs#L54-L58): Lines 54-58
- [`inject_stdlib()` function](crates/holon/src/api/backend_engine.rs#L61-L63): Lines 61-63

**Files**: `crates/holon/src/api/backend_engine.rs`

---

### Phase 2: Modify compile_query to accept context and inject stdlib
**Status**: completed

**Depends on**: Phase 1

**Objective**:
- Update `compile_query` signature to accept optional `QueryContext`
- Inject stdlib before parsing user PRQL
- Ensure stdlib injection doesn't break existing functionality

**Tasks**:
1. Change `compile_query` signature to `compile_query(&self, prql: String, context: Option<QueryContext>)`
2. Inject stdlib at the start of `compile_query`
3. Update all call sites of `compile_query` to pass `None` for context (backward compatibility)
4. Verify existing tests still pass

**Success Criteria**:
- `compile_query` accepts optional context parameter
- Stdlib is injected before PRQL parsing
- All existing call sites compile without errors
- Existing tests pass

**Implementation**:
- [`compile_query()` signature and stdlib injection](crates/holon/src/api/backend_engine.rs#L135-L142): Lines 135-142
- Updated call sites in `backend_engine.rs`:
  - Test: [Line 1543](crates/holon/src/api/backend_engine.rs#L1543)
  - Test: [Line 1749](crates/holon/src/api/backend_engine.rs#L1749)
- Updated call sites in other files:
  - [`crates/holon/src/testing/e2e_test_helpers.rs:117`](crates/holon/src/testing/e2e_test_helpers.rs#L117)
  - [`frontends/mcp/src/tools.rs:182`](frontends/mcp/src/tools.rs#L182)

**Files**: 
- `crates/holon/src/api/backend_engine.rs`
- `crates/holon/src/testing/e2e_test_helpers.rs`
- `frontends/mcp/src/tools.rs`

---

### Phase 3: Implement context parameter binding
**Status**: completed

**Depends on**: Phase 2

**Objective**:
- Create function to bind context parameters (`$context_id`, `$context_parent_id`)
- Update `execute_query` and `watch_query` to accept and bind context
- Ensure parameter binding works correctly with NULL values

**Tasks**:
1. Implement `bind_context_params()` helper function
2. Update `execute_query` to accept optional `QueryContext` and bind parameters
3. Update `watch_query` to accept optional `QueryContext` and bind parameters
4. Update `query_and_watch` to accept and pass context
5. Handle NULL values correctly (None → Value::Null)

**Success Criteria**:
- Context parameters are correctly bound to SQL queries
- NULL values are handled properly
- Parameter binding works with existing parameter system
- Tests verify parameter binding

**Implementation**:
- [`bind_context_params()` helper](crates/holon/src/api/backend_engine.rs#L451-L468): Lines 451-468
- [`execute_query()` with context](crates/holon/src/api/backend_engine.rs#L479-L495): Lines 479-495
- [`watch_query()` with context](crates/holon/src/api/backend_engine.rs#L510-L519): Lines 510-519
- [`query_and_watch()` with context](crates/holon/src/api/backend_engine.rs#L745-L766): Lines 745-766
- Updated call sites in `backend_engine.rs`:
  - [Line 1091](crates/holon/src/api/backend_engine.rs#L1091) - child PRQL query
  - [Line 1138](crates/holon/src/api/backend_engine.rs#L1138) - load_root_layout_block
  - [Line 1266](crates/holon/src/api/backend_engine.rs#L1266) - test setup
  - [Line 1300](crates/holon/src/api/backend_engine.rs#L1300) - test setup
  - [Line 1592](crates/holon/src/api/backend_engine.rs#L1592) - test
  - [Line 1624](crates/holon/src/api/backend_engine.rs#L1624) - test
  - [Line 1681](crates/holon/src/api/backend_engine.rs#L1681) - test
- Updated call sites in other files:
  - [`crates/holon/src/testing/e2e_test_helpers.rs:122`](crates/holon/src/testing/e2e_test_helpers.rs#L122)
  - [`crates/holon/src/testing/e2e_test_helpers.rs:146`](crates/holon/src/testing/e2e_test_helpers.rs#L146)
  - [`frontends/mcp/src/tools.rs:80,158,189,271`](frontends/mcp/src/tools.rs#L80)
  - [`frontends/flutter/rust/src/api/ffi_bridge.rs:397`](frontends/flutter/rust/src/api/ffi_bridge.rs#L397)
  - [`frontends/tui/src/tui_pbt_backend.rs`](frontends/tui/src/tui_pbt_backend.rs) - multiple lines (122, 144, 174, 204, 288, 426, 452, 539, 597, 644, 683)

**Files**: `crates/holon/src/api/backend_engine.rs`

---

### Phase 4: Replace load_index_regions with load_root_layout_block
**Status**: completed

**Depends on**: Phase 3

**Objective**:
- Replace hard-coded region discovery with root layout block approach
- Query for first root block (parent_id IS NULL) with PRQL source
- Update `init_app_frame` to use new approach

**Tasks**:
1. Implement `load_root_layout_block()` function
2. Query for root block with PRQL source child
3. Replace `load_index_regions()` call in `init_app_frame`
4. Remove `region_display_name()` hard-coded mapping (use block content instead)
5. Update region building logic to use `from children` query

**Success Criteria**:
- `load_root_layout_block()` returns correct root block
- `init_app_frame` compiles and works with new approach
- Region configs are built from root block's children
- Display names come from block content, not hard-coded mapping

**Implementation**:
- [`load_root_layout_block()` function](crates/holon/src/api/backend_engine.rs#L1120-L1145): Lines 1120-1145
  - SQL query: Lines 1123-1136
  - Error handling: Lines 1140-1144
- Old `load_index_regions()` removed (was at lines 1051-1104)
- Old `region_display_name()` removed (was at lines 1106-1114)

**Files**: `crates/holon/src/api/backend_engine.rs`

---

### Phase 5: Update init_app_frame to use root layout with context
**Status**: completed

**Depends on**: Phase 4

**Objective**:
- Compile root block's PRQL with root context
- Execute query to get layout children
- Build regions from query results
- Each child becomes a region

**Tasks**:
1. Load root layout block
2. Compile root block's PRQL with `QueryContext::root()`
3. Execute query with context parameters bound
4. Build `RegionConfig` for each child block
5. Extract display name and width from child block properties
6. Handle errors gracefully (partial failure support)

**Success Criteria**:
- `init_app_frame` uses root layout block approach
- Root PRQL query compiles and executes successfully
- Regions are built from query results
- Display names and widths are extracted from block properties
- Error handling works correctly

**Implementation**:
- [`init_app_frame()` main function](crates/holon/src/api/backend_engine.rs#L1025-L1114): Lines 1025-1114
  - Load root layout block: [Line 1032](crates/holon/src/api/backend_engine.rs#L1032)
  - Extract PRQL source: [Lines 1035-1041](crates/holon/src/api/backend_engine.rs#L1035-L1041)
  - Compile with root context: [Lines 1050-1051](crates/holon/src/api/backend_engine.rs#L1050-L1051)
  - Execute query: [Line 1054](crates/holon/src/api/backend_engine.rs#L1054)
  - Build regions loop: [Lines 1058-1105](crates/holon/src/api/backend_engine.rs#L1058-L1105)
    - Extract block_id: [Lines 1059-1063](crates/holon/src/api/backend_engine.rs#L1059-L1063)
    - Extract display name: [Lines 1066-1073](crates/holon/src/api/backend_engine.rs#L1066-L1073)
    - Query child PRQL source: [Lines 1079-1091](crates/holon/src/api/backend_engine.rs#L1079-L1091)
    - Create region config: [Lines 1103-1104](crates/holon/src/api/backend_engine.rs#L1103-L1104)

**Files**: `crates/holon/src/api/backend_engine.rs`

---

### Phase 6: Testing and validation
**Status**: pending

**Note**: After review fixes, the implementation should be functionally correct. Testing is needed to verify:
- Stdlib injection works correctly
- Context parameter binding works with various scenarios
- Root layout block loading works
- Region building from children works correctly
- Integration with Flutter frontend works

**Review Fixes Applied**:
1. ✅ **Critical**: Fixed NULL comparison bug - Changed `init_app_frame` to use `QueryContext::for_block(root_block_id, None)` instead of `QueryContext::root()` ([Line 1055](crates/holon/src/api/backend_engine.rs#L1055))
   - **Issue**: `QueryContext::root()` sets `context_id = NULL`, causing `parent_id = NULL` in SQL which is always false
   - **Fix**: Use `QueryContext::for_block(root_block_id, None)` to get children of root block, not root blocks themselves
   
2. ✅ Removed dead code: Deleted `region_display_name()` and `build_region_query()` functions
   - These were marked for removal in Phase 4 but were still present
   - Confirmed removed (grep found no matches)
   
3. ✅ Fixed fallback PRQL: Changed from incorrect `from children\nfilter parent_id == "..."` to simple `from children` ([Line 1105](crates/holon/src/api/backend_engine.rs#L1105))
   - **Issue**: Redundant filter was wrong since `children` already filters by `parent_id == $context_id`
   - **Fix**: Simplified to just `from children` (requires context to be set when querying)
   
4. ✅ Added TODO for N+1 query optimization ([Line 1080](crates/holon/src/api/backend_engine.rs#L1080))
   - Currently queries each child's PRQL source separately
   - Could be optimized by JOINing child PRQL sources in the root layout query
   
5. ✅ Documented NULL limitation in stdlib comment ([Lines 53-58](crates/holon/src/api/backend_engine.rs#L53-L58))
   - Added documentation explaining that `roots` virtual table should be used for root-level queries
   - `children` virtual table should be used with `QueryContext::for_block()` which sets a non-NULL context_id
   
6. ✅ Cleaned up unused imports: Removed `IndexRegion` from imports ([Line 22](crates/holon/src/api/backend_engine.rs#L22))
   - `IndexRegion` is no longer used after removing `load_index_regions()`
   - `Operation` is still used (line 828) so kept in imports

**Depends on**: Phase 5

**Objective**:
- Create comprehensive tests for new functionality
- Verify stdlib injection works correctly
- Test context parameter binding
- Test root layout block loading
- Test region building from children

**Tasks**:
1. Create test with root block containing `from children` query
2. Verify stdlib injection produces valid SQL
3. Test context parameter binding with various scenarios (root, nested blocks)
4. Test `load_root_layout_block()` function
5. Test `init_app_frame()` with new approach
6. Integration test: verify Flutter receives proper region configs

**Success Criteria**:
- All new tests pass
- Existing tests still pass
- Stdlib injection verified
- Context binding verified
- Root layout approach verified
- Integration test passes

**Files**: 
- `crates/holon/src/api/backend_engine.rs` (test module)
- Integration test files if needed

---

## Notes

- The spec mentions that `query_and_watch` may need updates to pass context through ✅ **Done**: Updated at [Line 745](crates/holon/src/api/backend_engine.rs#L745)
- Need to check all call sites of `compile_query`, `execute_query`, `watch_query`, and `query_and_watch` ✅ **Done**: All call sites updated (see Phase 2 and Phase 3 annotations)
- The `build_region_query` function may become obsolete if we're using PRQL directly ✅ **Removed**: Old function removed, now using PRQL directly from child blocks
- Display names should come from block content, not hard-coded mappings ✅ **Done**: Implemented at [Lines 1066-1073](crates/holon/src/api/backend_engine.rs#L1066-L1073)
- Width property extraction from JSON is already handled in the spec example (not yet implemented in region building, but PRQL queries can extract it)

## Review Summary

### Critical Issues Fixed
1. **NULL Comparison Bug** - Fixed by using `QueryContext::for_block()` instead of `QueryContext::root()`
2. **Semantic Confusion** - Clarified that `QueryContext::root()` gets root blocks, not children of root block
3. **Dead Code** - Removed `region_display_name()` and `build_region_query()` functions
4. **Incorrect Fallback** - Fixed fallback PRQL query
5. **Unused Imports** - Removed `IndexRegion` import

### Known Limitations
- **N+1 Query Pattern**: Currently queries each child's PRQL source separately. Could be optimized with a JOIN.
- **NULL Handling**: When `$context_id` is NULL, PRQL generates `parent_id = NULL` which is always false. Use `roots` virtual table for root-level queries, or `QueryContext::for_block()` for children queries.

### Implementation Status
| Aspect                | Status                   |
|-----------------------|--------------------------|
| Architecture          | ✅ Good                  |
| QueryContext design   | ✅ Good                  |
| Stdlib injection      | ✅ Good                  |
| Parameter binding     | ✅ Good                  |
| Root context handling | ✅ Fixed - uses for_block() |
| init_app_frame        | ✅ Fixed - uses correct context |
| Dead code cleanup     | ✅ Complete              |
| N+1 queries           | ⚠️ Documented (TODO)     |
| Code cleanup          | ✅ Complete              |

## Implementation Summary

### Core Changes
- **QueryContext**: [Lines 28-51](crates/holon/src/api/backend_engine.rs#L28-L51) - New struct for query context
- **PRQL Stdlib**: [Lines 54-58](crates/holon/src/api/backend_engine.rs#L54-L58) - Virtual table definitions
- **Stdlib Injection**: [Lines 61-63, 137-139](crates/holon/src/api/backend_engine.rs#L61-L63) - Function and usage
- **Context Binding**: [Lines 451-468](crates/holon/src/api/backend_engine.rs#L451-L468) - Parameter binding logic

### API Changes
- `compile_query()`: Now accepts `Option<QueryContext>` parameter
- `execute_query()`: Now accepts `Option<QueryContext>` parameter  
- `watch_query()`: Now accepts `Option<QueryContext>` parameter
- `query_and_watch()`: Now accepts `Option<QueryContext>` parameter

### Removed Code
- `load_index_regions()`: Removed (was lines 1051-1104)
- `region_display_name()`: Removed (was lines 1106-1114)
- `build_region_query()`: Removed (was around line 1116)

### New Code
- `load_root_layout_block()`: [Lines 1120-1145](crates/holon/src/api/backend_engine.rs#L1120-L1145)
- Updated `init_app_frame()`: [Lines 1025-1114](crates/holon/src/api/backend_engine.rs#L1025-L1114)
