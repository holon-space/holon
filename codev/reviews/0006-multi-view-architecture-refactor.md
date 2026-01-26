# Multi-View Architecture Refactor - Review Handoff

## Overview

This review covers the architectural refactoring to support multi-view rendering with a queryId-based provider system. The changes eliminate the singleton query pattern and introduce normalized PRQL hash-based query identification.

## Implementation Date

December 21, 2024

## Architectural Decisions Implemented

### Decision 1: Query Identification Strategy ✅
**Chosen**: Normalized PRQL hash as queryId

- **Implementation**: `frontends/flutter/lib/utils/query_id.dart`
- **Normalization**: Removes all whitespace (spaces, tabs, newlines)
- **Hashing**: SHA-256 hash of normalized PRQL (64-character hex string)
- **Future-ready**: Structure in place for comment removal extension
- **Failure mode**: Hard fail if queryId not found (via `validateQueryId()`)

**Key Functions**:
- `normalizePrql(String prql) -> String` - Normalizes PRQL for hashing
- `prqlToQueryId(String prql) -> String` - Generates stable queryId
- `validateQueryId(String queryId) -> void` - Validates and fails hard if invalid

### Decision 2: RenderSpec Provider Architecture ✅
**Chosen**: Convert singleton to family provider (Option A)

- **Removed**: Singleton `queryResultProvider` and `prqlQueryProvider`
- **Converted**: All query-related providers to family providers:
  - `renderSpecProvider(queryId)` - Family provider
  - `initialDataProvider(queryId)` - Family provider
  - `transformedInitialDataProvider(queryId)` - Family provider
  - `changeStreamProvider(queryId)` - Family provider
- **New**: `queryResultByQueryIdProvider(queryId)` - Lookup by queryId
- **Existing**: `queryResultByPrqlProvider(prql)` - Still used, auto-registers queryId

### Decision 3: Query ID Resolution ✅
**Chosen**: Derive from context (Option B)

- **Automatic**: `queryResultByPrqlProvider(prql)` automatically registers queryId
- **Lookup**: `queryResultByQueryIdProvider(queryId)` uses reverse mapping cache
- **Widgets**: Derive queryId from PRQL when needed (e.g., `prqlToQueryId(prql)`)

### Decision 4: View Selection State ✅
**Chosen**: Ephemeral state (Option A)

- **Implementation**: `ViewSelector` provider remains ephemeral
- **No persistence**: View selection lost on app restart (acceptable for MVP)
- **Future**: Can add persistence layer later if needed

## Files Changed

### New Files Created

1. **`frontends/flutter/lib/utils/query_id.dart`**
   - PRQL normalization and hashing utilities
   - QueryId validation

2. **`frontends/flutter/lib/utils/render_spec_extension.dart`**
   - `RenderSpecExtension` with `getViewStructure([viewName])` method
   - Provides clean API for accessing view structures
   - Falls back to `root` for backward compatibility

### Modified Files

1. **`frontends/flutter/lib/providers/query_providers.dart`**
   - Removed singleton `queryResultProvider`
   - Converted all providers to family providers
   - Added `queryResultByQueryIdProvider` for queryId-based lookup
   - Added reverse mapping cache (`_prqlSourceCache`) for queryId → PRQL lookup
   - Updated `queryResultByPrqlProvider` to auto-register queryIds

2. **`frontends/flutter/lib/providers/view_selector_provider.dart`**
   - Updated to use `renderSpecProvider(queryId)` family provider
   - Removed TODOs, now fully functional

3. **`frontends/flutter/lib/render/reactive_query_widget.dart`**
   - Added optional `viewName` parameter to `ReactiveQueryWidget`
   - Updated all three usages (lines 278, 437, 593) to use `renderSpec.getViewStructure(viewName)`
   - Updated `_ReactiveQueryWidgetContent` to accept and pass through `viewName`
   - Added import for `render_spec_extension.dart`

4. **`frontends/flutter/lib/providers/settings_provider.dart`**
   - Removed `prqlQueryProvider` (singleton)
   - Removed `setPrqlQuery()` function
   - Removed `_loadDefaultQuery()` helper
   - Removed `_defaultQueryAsset` constant
   - Removed `_prqlQueryKey` preference key

5. **`frontends/flutter/lib/ui/settings_screen.dart`**
   - Removed entire PRQL Query editing section
   - Removed `prqlQueryController` and related UI
   - Removed `savePrqlQuery()` function

6. **`frontends/flutter/lib/main.dart`**
   - Updated to use `queryResultByPrqlProvider(defaultMainQuery)` instead of singleton
   - Added `defaultMainQuery` constant (hardcoded default PRQL)
   - Updated to use `transformedInitialDataProvider(queryId)` family provider
   - Removed reference to `prqlQueryProvider`

7. **`frontends/flutter/pubspec.yaml`**
   - Added `crypto: ^3.0.5` dependency for SHA-256 hashing

8. **`frontends/flutter/lib/services/mock_backend_service.dart`**
   - Updated `RenderSpec` constructors to use `root` field (backward compatibility maintained)

### Rust Files Modified

1. **`crates/holon-api/src/render_types.rs`**
   - Kept `root` field for backward compatibility
   - Updated `root()` method to return `Option<&RenderExpr>`
   - Added `get_root_mut()` for mutable access
   - Method handles both single-view (uses `root`) and multi-view (uses `views[default_view].structure`)

2. **`crates/holon-prql-render/src/compiler.rs`**
   - Single-view queries: `views` empty, `root` populated
   - Multi-view queries: `views` populated, `root = views[default_view].structure`
   - All test cases updated to use `spec.root()` instead of `spec.root`

3. **`crates/holon-prql-render/src/lib.rs`**
   - Updated all `spec.root` to `spec.root()`
   - Updated all `&mut render_spec.root` to `render_spec.get_root_mut()`
   - All test cases updated

4. **`crates/holon/src/api/backend_engine.rs`**
   - Updated all `spec.root` to `spec.root()`
   - Updated `&mut render_spec.root` to `render_spec.get_root_mut()`
   - Test cases updated

5. **`crates/holon/tests/json_aggregation_e2e_test.rs`**
   - Updated to use `render_spec.root()`

6. **`crates/holon/tests/e2e_backend_engine_test.rs`**
   - Updated to use `render_spec.root()`

### Generated Files Updated

- `frontends/flutter/lib/providers/settings_provider.g.dart` - Regenerated (removed prqlQuery references)
- All other generated files regenerated via `build_runner`

## Breaking Changes

### ⚠️ Removed Providers

These providers no longer exist and must be migrated:

1. **`queryResultProvider`** (singleton)
   - **Replacement**: `queryResultByPrqlProvider(prql)` or `queryResultByQueryIdProvider(queryId)`
   - **Migration**: Pass PRQL string or queryId as parameter

2. **`prqlQueryProvider`** (singleton)
   - **Replacement**: None - global PRQL query setting removed
   - **Migration**: Each widget/component must provide its own PRQL query

3. **`renderSpecProvider`** (singleton)
   - **Replacement**: `renderSpecProvider(queryId)` (family provider)
   - **Migration**: Compute queryId from PRQL: `renderSpecProvider(prqlToQueryId(prql))`

4. **`initialDataProvider`** (singleton)
   - **Replacement**: `initialDataProvider(queryId)` (family provider)
   - **Migration**: Compute queryId from PRQL: `initialDataProvider(prqlToQueryId(prql))`

5. **`transformedInitialDataProvider`** (singleton)
   - **Replacement**: `transformedInitialDataProvider(queryId)` (family provider)
   - **Migration**: Compute queryId from PRQL: `transformedInitialDataProvider(prqlToQueryId(prql))`

6. **`changeStreamProvider`** (singleton)
   - **Replacement**: `changeStreamProvider(queryId)` (family provider)
   - **Migration**: Compute queryId from PRQL: `changeStreamProvider(prqlToQueryId(prql))`

### ⚠️ Removed Settings

- **PRQL Query setting**: No longer configurable via Settings UI
- **Impact**: Main UI uses hardcoded default query (see `main.dart`)
- **Future**: May need per-query configuration or query management UI

## Migration Guide

### For Widgets Using Queries

**Before**:
```dart
final queryResult = ref.watch(queryResultProvider);
final renderSpec = ref.watch(renderSpecProvider);
final initialData = ref.watch(transformedInitialDataProvider);
```

**After**:
```dart
const prql = 'from blocks render (list ...)';
final queryId = prqlToQueryId(prql);
final queryResult = ref.watch(queryResultByPrqlProvider(prql));
final renderSpec = ref.watch(renderSpecProvider(queryId));
final initialData = ref.watch(transformedInitialDataProvider(queryId));
```

### For Accessing Render Structure

**Before**:
```dart
final rootExpr = renderSpec.root;  // Direct field access
```

**After**:
```dart
import '../utils/render_spec_extension.dart';

// Single-view or default view
final rootExpr = renderSpec.getViewStructure();

// Specific view (multi-view queries)
final rootExpr = renderSpec.getViewStructure('sidebar');
```

### For ViewSelector Usage

**Before**:
```dart
// Didn't work - singleton provider
final selector = ref.watch(viewSelectorProvider('some-id'));
```

**After**:
```dart
final prql = 'from blocks render (views sidebar:(...) main:(...))';
final queryId = prqlToQueryId(prql);
final selector = ref.watch(viewSelectorProvider(queryId));
final currentView = ref.watch(viewSelectorProvider(queryId));
final availableViews = ref.read(viewSelectorProvider(queryId).notifier).availableViews;
```

### For ReactiveQueryWidget

**Before**:
```dart
ReactiveQueryWidget(
  sql: sql,
  params: params,
  renderSpec: renderSpec,
  // ...
)
```

**After**:
```dart
// Default view
ReactiveQueryWidget(
  sql: sql,
  params: params,
  renderSpec: renderSpec,
  // viewName: null (default) or omitted
)

// Specific view
ReactiveQueryWidget(
  sql: sql,
  params: params,
  renderSpec: renderSpec,
  viewName: 'sidebar',  // NEW: optional view name
)
```

## Testing Checklist

### ✅ Build Verification
- [x] `flutter build macos` succeeds
- [x] No compilation errors
- [x] Generated files regenerated correctly
- [x] Rust code compiles (`cargo check` passes)
- [x] Dart code analyzes cleanly

### 🔍 Functional Testing Needed

1. **Query Execution**
   - [ ] Main UI loads with default query
   - [ ] Embedded queries in blocks execute correctly
   - [ ] `LiveQueryWidget` works with PRQL strings
   - [ ] Multiple queries can run simultaneously

2. **QueryId Generation**
   - [ ] Same PRQL (with different whitespace) produces same queryId
   - [ ] Different PRQL produces different queryIds
   - [ ] QueryId lookup works after query execution

3. **Multi-View Rendering**
   - [ ] `ViewSelector` provider works correctly
   - [ ] View switching works
   - [ ] Filter expressions evaluate correctly
   - [ ] Multiple views render independently
   - [ ] `ReactiveQueryWidget` with `viewName` parameter works
   - [ ] Extension method `getViewStructure()` works for both single and multi-view

4. **Backward Compatibility**
   - [ ] Single-view queries still work (via `root` field)
   - [ ] `getViewStructure()` falls back to `root` correctly
   - [ ] Existing code using `renderSpec.root` still works (if any)

5. **Provider Behavior**
   - [ ] Family providers cache correctly per queryId
   - [ ] Query results are shared when same queryId used
   - [ ] Error handling works (queryId not found, etc.)

6. **Settings**
   - [ ] Settings screen loads without PRQL section
   - [ ] Other settings (API key, theme, etc.) still work

## Known Issues / Limitations

1. **Hardcoded Default Query**
   - Main UI uses hardcoded `defaultMainQuery` constant
   - No way to change main query without code changes
   - **Future**: Consider query management UI or configuration file

2. **No Query Persistence**
   - QueryId → PRQL mapping is in-memory only (`_prqlSourceCache`)
   - Lost on app restart
   - **Impact**: Queries must be re-executed to register queryIds
   - **Future**: Consider persisting mapping or using PRQL directly

3. **Settings Removal**
   - PRQL query editing removed from Settings
   - Users can't configure main query via UI
   - **Future**: May need per-query configuration or query library

4. **QueryId Collision Risk**
   - Very low (SHA-256), but theoretically possible
   - **Mitigation**: Normalization ensures deterministic hashing
   - **Future**: Could add collision detection if needed

5. **Root Field Dual Maintenance**
   - Both `root` and `views` fields maintained in `RenderSpec`
   - Compiler must populate both for multi-view queries
   - **Impact**: Minimal - compiler handles it automatically
   - **Rationale**: Backward compatibility worth the small overhead

## Future Enhancements

### Short-term
1. **Comment Removal**: Extend `normalizePrql()` to remove PRQL comments
2. **Query Management UI**: Allow users to save/manage queries
3. **Query Library**: Store frequently used queries

### Medium-term
1. **View Selection Persistence**: Persist view selection per queryId
2. **Query History**: Track recently used queries
3. **Query Templates**: Predefined query templates

### Long-term
1. **Query Sharing**: Export/import queries
2. **Query Versioning**: Track query changes over time
3. **Query Analytics**: Track query performance and usage

## Code Quality

### ✅ Strengths
- Clean separation of concerns
- Consistent provider pattern (all family providers)
- Fail-fast error handling
- Well-documented functions
- Type-safe throughout
- Extension method provides clean API without breaking changes
- Backward compatibility maintained through `root` field

### 🔍 Areas for Review
- Reverse mapping cache (`_prqlSourceCache`) is global mutable state
- Hardcoded default query in `main.dart`
- No tests for queryId normalization/hashing
- No tests for provider migration
- No tests for extension method behavior
- Dual maintenance of `root` and `views` fields (acceptable trade-off)

## Dependencies Added

- `crypto: ^3.0.5` - For SHA-256 hashing

## Build Status

✅ **Build Successful**
- macOS: `flutter build macos` ✓
- No compilation errors
- No linter errors
- Generated files up-to-date

## Review Response & Implementation Updates

### Issues Addressed

1. **✅ Duplicate Doc Comment Fixed**
   - Removed duplicate documentation for `queryResultByQueryIdProvider`
   - Single clear doc comment remains

2. **✅ Root Field - Extension Method Approach**
   - **Decision**: Keep `root` field for backward compatibility, use extension method for access
   - **Implementation**: 
     - `root` field remains in `RenderSpec` struct
     - Single-view queries: `views` empty, `root` populated
     - Multi-view queries: `views` populated, `root = views[default_view].structure`
   - **Dart Extension**: Created `RenderSpecExtension.getViewStructure([viewName])`
     - Falls back to `root` for single-view queries
     - Uses `views[viewName ?? defaultView].structure` for multi-view
   - **Rust Access**: Updated `root()` method to return `Option<&RenderExpr>`
     - Returns `Some(&root)` if views empty
     - Returns `Some(&views[default_view].structure)` if views populated
   - **Migration**: All Rust code uses `spec.root()` instead of `spec.root`
   - **Future**: `root` field can remain indefinitely as backward-compat layer

3. **✅ Race Condition in ViewSelector**
   - **Status**: Intentional fail-hard design
   - **Behavior**: `renderSpecProvider(queryId)` throws if query not loaded
   - **Rationale**: Matches project philosophy - fail fast and clearly
   - **Documentation**: This is expected behavior per design

4. **⚠️ Global Mutable State**
   - **Status**: Acknowledged, acceptable for MVP
   - **Current**: `_prqlSourceCache` is top-level mutable map
   - **Impact**: Works correctly, but could cause test isolation issues
   - **Future**: Consider provider-based cache for better testability
   - **Priority**: Low (can be addressed when adding tests)

## Review Questions

1. **Default Query Strategy**: Is hardcoded default query acceptable, or should we have a configuration mechanism?

2. **QueryId Persistence**: Should we persist the queryId → PRQL mapping, or is in-memory sufficient?

3. **Settings Removal**: Was removing PRQL editing from Settings the right call, or should we have a query management UI?

4. **Error Messages**: Are the error messages clear enough when queryId is not found?

5. **Testing**: Should we add unit tests for queryId normalization/hashing and extension method?

6. **Documentation**: Is the migration guide sufficient for other developers?

7. **Root Field Strategy**: ✅ **RESOLVED** - Keep `root` field indefinitely as backward-compat layer. Extension method provides clean API without requiring removal.

## Related Documentation

- `docs/HANDOFF_MULTI_VIEW_RENDER.md` - Original multi-view feature spec
- `docs/ARCHITECTURE_MULTI_VIEW_DECISIONS.md` - Architectural decision document

## Implementation Summary

### ✅ Completed Changes

1. **QueryId Architecture**
   - Normalized PRQL hash as queryId (whitespace removed, SHA-256)
   - All providers converted to family providers
   - Singleton pattern removed

2. **Root Field Strategy**
   - Kept `root` field for backward compatibility
   - Extension method `getViewStructure()` provides clean API
   - Rust `root()` method handles both single and multi-view cases

3. **Multi-View Support**
   - `ViewSelector` provider fully functional
   - `ReactiveQueryWidget` supports optional `viewName` parameter
   - Extension method enables view selection

4. **Code Migration**
   - All Rust code uses `spec.root()` instead of `spec.root`
   - All Dart code uses `renderSpec.getViewStructure(viewName)`
   - Backward compatibility maintained

## Sign-off

**Implementation**: ✅ Complete
**Build**: ✅ Passing (`flutter build macos` succeeds)
**Rust Compilation**: ✅ Passing (`cargo check` succeeds)
**Dart Analysis**: ✅ Clean (no errors)
**Ready for Review**: ✅ Yes

---

**Next Steps**:
1. ✅ Review architectural decisions - **COMPLETE**
2. Test multi-view rendering functionality
3. Verify provider migration in all widgets
4. Add unit tests for queryId normalization and extension method
5. Consider future enhancements based on usage
