# Architectural Decisions for Multi-View Rendering

## Current Architecture

The application currently has **two query execution patterns**:

### 1. Singleton Query Pattern
- **Provider**: `queryResultProvider` (singleton `FutureProvider`)
- **Source**: Uses `prqlQueryProvider` from settings (the "main" query)
- **Use Case**: Main application query displayed in the primary UI
- **RenderSpec Access**: Via `renderSpecProvider` (singleton `Provider`)

### 2. Family Query Pattern  
- **Provider**: `queryResultByPrqlProvider` (family `FutureProvider.family<String, QueryResult>`)
- **Source**: PRQL string passed as parameter
- **Use Case**: Embedded queries in blocks (e.g., `LiveQueryWidget`, query blocks in org-mode)
- **RenderSpec Access**: Currently extracted directly from `queryResultByPrqlProvider(prql).renderSpec`

## The Problem

`ViewSelector` is designed as a **family provider** that takes a `queryId` parameter:

```dart
@riverpod
class ViewSelector extends _$ViewSelector {
  @override
  String build(String queryId) {
    // Need: renderSpecProvider(queryId) to get views
    // But: renderSpecProvider is singleton, not family
  }
}
```

**Mismatch**: 
- `ViewSelector` expects per-query state management
- `renderSpecProvider` only works with the singleton query
- No clear mapping from `queryId` → query provider

## Architectural Decisions Needed

### Decision 1: Query Identification Strategy

**Question**: How should queries be uniquely identified?

**Options**:

#### Option A: PRQL String as Query ID
- **Pros**: 
  - Natural identifier (already used by `queryResultByPrqlProvider`)
  - Deterministic (same query = same ID)
  - Works for both singleton and family patterns
- **Cons**:
  - Long strings as keys (memory/performance)
  - No stable ID if query text changes
  - Hard to reference from UI (can't show full PRQL)

#### Option B: Stable Block ID
- **Pros**:
  - Stable identifier (block ID doesn't change)
  - Short, efficient keys
  - Natural for embedded queries (each block = one query)
- **Cons**:
  - Doesn't work for singleton query (no block)
  - Requires mapping block ID → PRQL source
  - More complex lookup

#### Option C: Hybrid Approach
- **Singleton query**: Use special ID like `"main"` or `"settings"`
- **Embedded queries**: Use block ID or PRQL hash
- **Pros**: Handles both patterns
- **Cons**: More complex, two ID systems

**Recommendation**: **Option A (PRQL String)** - simplest, already works with existing providers

---

### Decision 2: RenderSpec Provider Architecture

**Question**: How should `RenderSpec` be accessed per query?

**Options**:

#### Option A: Make `renderSpecProvider` a Family Provider
```dart
final renderSpecProvider = Provider.family<RenderSpec, String>((ref, queryId) {
  // If queryId is special value (e.g., "main"), use queryResultProvider
  // Otherwise, use queryResultByPrqlProvider(queryId)
});
```

**Pros**:
- Single provider for all queries
- Consistent API
- `ViewSelector` can use it directly

**Cons**:
- Breaks existing code using singleton `renderSpecProvider`
- Requires migration
- More complex logic (routing to different providers)

#### Option B: Create New Family Provider
```dart
final renderSpecByQueryIdProvider = Provider.family<RenderSpec, String>((ref, queryId) {
  // Route to appropriate provider based on queryId
});
```

**Pros**:
- Doesn't break existing code
- Clear separation of concerns
- Can deprecate old provider gradually

**Cons**:
- Two providers doing similar things
- More code to maintain

#### Option C: ViewSelector Accesses Query Providers Directly
```dart
@riverpod
class ViewSelector extends _$ViewSelector {
  @override
  String build(String queryId) {
    // Determine which provider to use based on queryId
    if (queryId == 'main') {
      final result = ref.watch(queryResultProvider);
      return result.when(...);
    } else {
      final result = ref.watch(queryResultByPrqlProvider(queryId));
      return result.when(...);
    }
  }
}
```

**Pros**:
- No new providers needed
- Direct access to data
- Flexible

**Cons**:
- ViewSelector becomes tightly coupled to query providers
- Logic duplication if other code needs RenderSpec
- Harder to test

**Recommendation**: **Option B (New Family Provider)** - cleanest separation, no breaking changes

---

### Decision 3: Query ID Resolution Strategy

**Question**: How should the app determine which `queryId` to use in different contexts?

**Contexts**:
1. **Main UI**: Uses singleton `queryResultProvider`
2. **LiveQueryWidget**: Uses `queryResultByPrqlProvider(prql)`
3. **Query blocks**: Uses `queryResultByPrqlProvider(block.content)`

**Options**:

#### Option A: Explicit Query ID Parameter
- Every widget that needs view selection passes `queryId`
- `LiveQueryWidget` would need: `LiveQueryWidget(prql: ..., queryId: ...)`
- **Pros**: Explicit, clear
- **Cons**: More parameters, easy to forget

#### Option B: Derive from Context
- `LiveQueryWidget` derives `queryId` from `prql` parameter
- Main UI uses special ID like `"main"`
- **Pros**: Automatic, less boilerplate
- **Cons**: Magic values, less explicit

#### Option C: Provider-Based Resolution
- Create a provider that maps context → queryId
- Widgets use context provider instead of explicit ID
- **Pros**: Centralized logic, testable
- **Cons**: More abstraction, harder to understand

**Recommendation**: **Option B (Derive from Context)** - simplest, matches current patterns

---

### Decision 4: View Selection State Management

**Question**: Where should view selection state live?

**Current**: `ViewSelector` is a family provider keyed by `queryId`

**Considerations**:
- View selection is **per-query** (different queries can have different selected views)
- View selection is **UI state** (user preference, not data)
- Should persist across rebuilds? (probably yes)
- Should persist across app restarts? (maybe, via settings)

**Options**:

#### Option A: Keep Current Design (Family Provider)
- `ViewSelector(queryId)` manages state per query
- State is ephemeral (lost on app restart)
- **Pros**: Simple, matches query structure
- **Cons**: No persistence

#### Option B: Add Persistence Layer
- `ViewSelector` reads/writes to settings
- Key: `"view_selection_$queryId"`
- **Pros**: Persists user preferences
- **Cons**: More complexity, settings management

#### Option C: Hybrid (Memory + Optional Persistence)
- Default: ephemeral state in provider
- Optional: persist to settings for "main" query
- **Pros**: Best of both worlds
- **Cons**: More complex

**Recommendation**: **Option A (Current Design)** - add persistence later if needed

---

## Recommended Implementation Plan

### Phase 1: Create Query ID Resolution
1. Define `queryId` convention:
   - Main query: `"main"` (or derive from `prqlQueryProvider`)
   - Embedded queries: Use PRQL string as-is
2. Create helper function:
   ```dart
   String getQueryId(String? prql) => prql ?? 'main';
   ```

### Phase 2: Create Family RenderSpec Provider
```dart
final renderSpecByQueryIdProvider = Provider.family<RenderSpec, String>(
  (ref, queryId) {
    if (queryId == 'main') {
      final result = ref.watch(queryResultProvider);
      return result.when(
        data: (r) => r.renderSpec,
        loading: () => throw UnimplementedError('...'),
        error: (_, __) => throw UnimplementedError('...'),
      );
    } else {
      final result = ref.watch(queryResultByPrqlProvider(queryId));
      return result.when(
        data: (r) => r.renderSpec,
        loading: () => throw UnimplementedError('...'),
        error: (_, __) => throw UnimplementedError('...'),
      );
    }
  },
);
```

### Phase 3: Update ViewSelector
```dart
@riverpod
class ViewSelector extends _$ViewSelector {
  @override
  String build(String queryId) {
    final spec = ref.watch(renderSpecByQueryIdProvider(queryId));
    return spec.defaultView;
  }

  List<String> get availableViews {
    final spec = ref.read(renderSpecByQueryIdProvider(queryId));
    return spec.views.keys.toList();
  }
}
```

### Phase 4: Update Widgets
- `LiveQueryWidget`: Pass `prql` as `queryId` to `ViewSelector`
- Main UI: Use `ViewSelector('main')`
- Query blocks: Use `ViewSelector(block.content)` or block ID

---

## Open Questions

1. **Should view selection persist across app restarts?**
   - Probably yes for main query
   - Probably no for embedded queries (they're transient)

2. **How to handle query updates?**
   - If PRQL changes, should view selection reset to default?
   - Or remember last selection per query text?

3. **Multiple instances of same query?**
   - If same PRQL appears in multiple blocks, share view selection?
   - Or separate state per block?

4. **View selection in nested queries?**
   - Can nested queries have their own views?
   - How to identify nested query IDs?

---

## Summary

The core architectural decision is: **How to map `queryId` → `RenderSpec`** when there are two different query provider patterns.

**Recommended approach**:
1. Use PRQL string as `queryId` (or special `"main"` for singleton)
2. Create `renderSpecByQueryIdProvider` family that routes to appropriate provider
3. Keep `ViewSelector` as family provider keyed by `queryId`
4. Derive `queryId` from context (PRQL string or `"main"`)

This maintains backward compatibility while enabling multi-view support for all query types.
