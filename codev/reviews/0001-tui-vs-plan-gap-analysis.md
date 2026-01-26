# TUI Implementation Gap Analysis

**Date**: 2025-01-11
**Plan**: `codev/plans/0001-reactive-prql-rendering.md`
**TUI Location**: `frontends/tui/`

---

## Executive Summary

The TUI implementation has **core functionality working** (PRQL parsing, CDC streaming, basic rendering) but is **missing several planned widgets/primitives** and some architectural features. This document identifies gaps between the plan and current implementation.

---

## ✅ What's Implemented (Matches Plan)

### Core Infrastructure
- ✅ **PRQL Parser & AST**: Generic `RenderExpr` parsing working
- ✅ **SQL Compilation**: PRQL → SQL via `prqlc` working
- ✅ **CDC Streaming**: Row change notifications via materialized views
- ✅ **Operation Execution**: `execute_operation()` integrated with async handling
- ✅ **Operation Inference**: Operations attached to `FunctionCall` nodes (Phase 2.4)

### TUI Interpreter (Phase 2.3)
- ✅ **Basic Widget Mappings**:
  - `"list"` → List rendering ✅
  - `"row"` → Horizontal layout ✅
  - `"text"` → Text rendering ✅
  - `"checkbox"` → `[✓]`/`[ ]` ✅
  - `"badge"` → Colored span ✅
  - `"icon"` → Icon character ✅
  - `"editable_text"` → Inline text editing with cursor ✅

- ✅ **Expression Evaluation**:
  - Column references (`ColumnRef`) ✅
  - Literals ✅
  - Binary operators (arithmetic, comparison, logical) ✅

- ✅ **CDC Integration**:
  - CDC stream forwarding to UI thread ✅
  - `poll_cdc_changes()` for non-blocking updates ✅
  - Hierarchical re-sorting after CDC updates ✅

---

## ❌ Missing Features (Gaps from Plan)

### 1. Missing Widget Primitives

According to **Phase 2.3** (lines 379-386), the TUI interpreter should support:

| Widget | Plan Mapping | Status | Notes |
|--------|-------------|--------|-------|
| `editable_text` | styled `Paragraph` or `Input` | ✅ **IMPLEMENTED** | Inline editing with cursor movement, arrow keys, Home/End navigation |
| `block` | `Paragraph` | ❌ **MISSING** | Plan mentions `block` primitive for structured content |
| `drop_zone` | Not specified | ❌ **MISSING** | Drag-drop zones (3 per block: before/after/as_child) |
| `collapse_button` | Not specified | ❌ **MISSING** | Expand/collapse indicator button |
| `block_operations` | Not specified | ❌ **MISSING** | Context menu for block operations |
| `flexible` | Ignore (TUI doesn't need) | ⚠️ **NOT NEEDED** | Plan says TUI can ignore this wrapper |

**Impact**: Cannot render full outliner UI as specified in plan. Missing drag-drop, collapse/expand. Inline editing now works.

**Files Affected**:
- `frontends/tui/src/render_interpreter.rs` - Missing widget handlers (editable_text ✅ implemented)
- `frontends/tui/src/ui_element.rs` - Missing UIElement variants (EditableText ✅ implemented)

### 2. Nested Reactive Queries

**Plan Reference**: Phase 2.3, line 90-92:
> "Each `RenderExpr` can have optional `query` field"

**Status**: ❌ **NOT IMPLEMENTED**

**Expected Behavior**:
- Widgets can have nested queries (e.g., block containing live table)
- Each nested query gets its own CDC stream
- Lazy loading: Query only when scrolled into view
- Auto-disposal when widget removed

**Current State**: TUI only handles single top-level query. No support for nested queries.

**Impact**: Cannot render blocks containing live sub-queries (e.g., "tasks in this project").

**Files Affected**:
- `frontends/tui/src/render_interpreter.rs` - No nested query handling
- `frontends/tui/src/components/block_list.rs` - Single query only
- `frontends/tui/src/state.rs` - Single CDC stream only

### 3. Keyed Widget Caching

**Plan Reference**: Phase 1.3, line 86-88:
> "Keyed Widget Caching: `HashMap<BlockId, RowData>` + sorted view
> - Minimal UI rebuilds via stable keys
> - Works for Flutter (`ValueKey`) and TUI (line indices)"

**Status**: ⚠️ **PARTIALLY IMPLEMENTED**

**Current State**:
- ✅ Selection tracking by block ID (`selected_block_id_cache`)
- ✅ Hierarchical re-sorting after CDC updates
- ❌ **No widget-level caching** - Rebuilds entire element tree on each render
- ❌ **No stable keys** - Uses array indices, not block IDs

**Impact**: Performance degradation with large datasets (1000+ blocks). Every CDC update rebuilds entire UI tree.

**Files Affected**:
- `frontends/tui/src/components/block_list.rs` - `rebuild_element_tree()` called every render
- `frontends/tui/src/render_interpreter.rs` - No caching layer

**Recommendation**: Implement `HashMap<String, UIElement>` cache keyed by block ID, only rebuild changed elements.

### 4. CDC Event Coalescing

**Plan Reference**: Phase 1.3, line 210-221:
> "CDC Event Coalescing (prevents materialized view flickering):
> - Batch process CDC events per callback invocation
> - DELETE then INSERT → UPDATE (existing logic)
> - INSERT then DELETE → no-op (drop both events)"

**Status**: ✅ **IMPLEMENTED IN BACKEND** (but need to verify TUI handles it correctly)

**Current State**:
- ✅ Backend coalesces DELETE+INSERT → UPDATE (in `turso.rs`)
- ✅ Backend coalesces INSERT+DELETE → no-op
- ⚠️ **TUI applies changes directly** - May see intermediate states if backend doesn't coalesce properly

**Impact**: Potential UI flicker if backend coalescing fails. Should be fine if backend works correctly.

**Verification Needed**: Test with rapid materialized view updates to ensure no flicker.

### 5. Operation Auto-Wiring

**Plan Reference**: Phase 2.4, line 408-522:
> "Automatic Operation Inference: Automatically wire operations based on column references"

**Status**: ✅ **BACKEND IMPLEMENTS** but ⚠️ **TUI USES MANUAL EXTRACTION**

**Current State**:
- ✅ Backend attaches `operations: Vec<OperationWiring>` to `FunctionCall` nodes
- ✅ TUI reads operations from `UIElement::Checkbox { operations }`
- ⚠️ **Manual extraction** - TUI manually builds operation signals in `block_list.rs:71-109`

**Impact**: Works but not as elegant as plan. Plan suggests operations should be automatically wired, but TUI still needs manual signal construction.

**Files Affected**:
- `frontends/tui/src/components/block_list.rs` - Manual operation signal building
- `frontends/tui/src/ui_element.rs` - `get_operation()` helper exists but limited

**Recommendation**: Create higher-level abstraction that automatically wires operations from `OperationWiring` to `AppSignal`.

### 6. Missing Helper Functions

**Plan Reference**: Phase 1.1, line 134-136:
> "Helper functions now work:
> - `drop_zones(invalid_targets)` → expands to 3 drop_zone primitives
> - `standard_block_ops(params)` → expands to block_operations"

**Status**: ✅ **BACKEND SUPPORTS** but ❌ **TUI CAN'T RENDER**

**Current State**:
- ✅ Backend expands helper functions during PRQL parsing
- ❌ TUI doesn't support `drop_zone` or `block_operations` widgets

**Impact**: Helper functions compile but render as unknown widgets (`[drop_zone]` fallback).

---

## 🔄 Architectural Differences

### 1. State Management

**Plan**: Mentions "StreamBuilder" pattern (Flutter) but doesn't specify TUI pattern.

**Current**: TUI uses:
- `State` struct with `Vec<HashMap<String, Value>>` data
- `poll_cdc_changes()` for non-blocking updates
- Manual re-sorting after CDC updates

**Status**: ✅ **WORKS** but different from Flutter's reactive pattern.

### 2. Operation Execution

**Plan**: Phase 3.1 mentions `execute_operation()` with `RowView` validation.

**Current**: TUI uses:
- Direct `execute_operation()` calls with `HashMap<String, Value>`
- No `RowView` validation layer (operations handle validation internally)

**Status**: ✅ **WORKS** - Operations validate internally, `RowView` not needed for TUI.

### 3. Error Handling

**Plan**: Phase 6 mentions error propagation strategy.

**Current**: TUI uses:
- `AppSignal::OperationResult` for async operation results
- Status message display in status bar
- No error dialogs or detailed error UI

**Status**: ⚠️ **BASIC** - Errors shown in status bar, no detailed error UI.

---

## 📊 Implementation Completeness

| Phase | Component | Status | Completeness |
|-------|-----------|--------|--------------|
| Phase 1.1 | PRQL Parser | ✅ Complete | 100% |
| Phase 1.2 | SQL Compilation | ✅ Complete | 100% |
| Phase 1.3 | CDC Streaming | ✅ Complete | 100% |
| Phase 1.4 | Fractional Indexing | ✅ Complete | 100% |
| Phase 2.1 | Generic AST | ✅ Complete | 100% |
| Phase 2.2 | FRB Types | ✅ Complete | 100% |
| Phase 2.3 | TUI Interpreter | ⚠️ Partial | **70%** - Missing 4 widgets (editable_text ✅) |
| Phase 2.4 | Operation Inference | ✅ Complete | 100% |
| Phase 3.1 | Operation Registry | ✅ Complete | 100% |
| Phase 3.2 | Block Operations | ⚠️ Partial | **40%** - Only UpdateField implemented |
| Phase 4.1 | FFI Bridge | ✅ Complete | 100% |
| Phase 4.2 | Widget Mappings | ⚠️ Partial | **70%** - Missing 4 widgets (editable_text ✅) |
| Phase 4.3 | State Management | ✅ Complete | 100% |

**Overall TUI Completeness**: **~80%**

---

## 🎯 Priority Recommendations

### High Priority (Blocks Core Features)

1. **Implement `block` primitive** (Phase 2.3)
   - Structured content container
   - Map to `Paragraph` or custom layout widget

2. **Implement `collapse_button`** (Phase 2.3)
   - Expand/collapse functionality
   - Wire to `UpdateField(collapsed)` operation

### Medium Priority (Enhances UX)

3. **Implement keyed widget caching** (Performance)
   - Cache `UIElement` tree by block ID
   - Only rebuild changed elements on CDC updates
   - Improves performance with 1000+ blocks

4. **Implement `drop_zone` and drag-drop** (Phase 2.3)
   - 3 drop zones per block (before/after/as_child)
   - Wire to `MoveBlock` operation
   - Client-side validation via `invalid_targets`

5. **Implement `block_operations`** (Phase 2.3)
   - Context menu for block actions
   - Wire to operations (indent, outdent, delete, etc.)

### Low Priority (Nice to Have)

6. **Nested reactive queries** (Phase 2.3)
   - Blocks containing live sub-queries
   - Own CDC stream per nested query
   - Lazy loading

7. **Enhanced error handling** (Phase 6)
   - Error dialogs
   - Detailed error messages
   - Retry mechanisms

---

## 📝 Files Requiring Updates

### High Priority
- `frontends/tui/src/render_interpreter.rs` - Add widget handlers
- `frontends/tui/src/ui_element.rs` - Add UIElement variants

### Medium Priority
- `frontends/tui/src/components/block_list.rs` - Add caching, drag-drop
- `frontends/tui/src/state.rs` - Optimize CDC handling

### Low Priority
- `frontends/tui/src/components/` - New components for nested queries

---

## ✅ What Works Well

1. **Core Architecture**: UI-agnostic backend working perfectly
2. **CDC Streaming**: Non-blocking updates working smoothly
3. **Operation Execution**: Async operations with signal-based results
4. **Expression Evaluation**: Full binary operator support
5. **Hierarchical Sorting**: Depth-first tree rendering working
6. **Inline Text Editing**: `editable_text` widget fully functional with cursor movement, arrow keys, Home/End navigation, and proper viewport handling

---

## 🔍 Testing Gaps

Based on plan's Phase 6.1 (Integration Tests):

- ❌ End-to-end test: PRQL → SQL → CDC → TUI update
- ❌ Test all operations (indent, outdent, split, delete, move)
- ❌ Test drag-drop with cycle prevention
- ❌ Performance benchmarks (1000+ blocks, rapid CDC polling)

**Recommendation**: Add integration tests matching Flutter test suite.

---

## Summary

The TUI implementation has **solid foundations** (80% complete) but is **missing 4 key widgets** that prevent full outliner functionality:
- ✅ `editable_text` - Inline editing **IMPLEMENTED** (with cursor movement, arrow keys, Home/End)
- `block` - Structured content
- `collapse_button` - Expand/collapse
- `drop_zone` - Drag-drop
- `block_operations` - Context menu

**Next Steps**: Implement missing widgets in priority order, starting with `collapse_button` and `block` for basic outliner functionality.

