# Flutter Render Architecture Migration

## Context

RenderSpec was removed from the PRQL compilation pipeline. The old architecture compiled
PRQL queries into SQL + RenderSpec (a static tree describing the UI). The new architecture
uses EntityProfiles that resolve per-row at runtime, attaching RowProfile to each
ResolvedRow in the WidgetSpec.

The Rust side is done. The Flutter side has ~50 compilation errors because it still
references old types (RenderSpec, RowTemplate, ViewSpec, FilterExpr) that FRB no longer
generates.

## Key Insight: Two Levels of Rendering

The old code used RenderSpec for TWO purposes:
1. **Collection-level**: What type of view? (list/tree/columns/table) — was `RenderSpec.root`
2. **Item-level**: How to render each row? — was `RenderSpec.rowTemplates[i].expr`

The new architecture handles item-level via `ResolvedRow.profile.render`. Collection-level
rendering comes from the **sibling `render` block** — the same mechanism already used in
`load_root_layout_block()` (fetches `render_src.content AS render_source` alongside the
query source). This sibling block should use the same **variants** logic as row-level
profiles, enabling:
- Switching between collection renderings (tree, table, list) just like row-level variants
- Encoding collection-level operations (sort, filter, group-by, etc.)
- For the root layout, the convention is "columns" (but still expressed as a render block)
- For sub-queries via `live_query`, the render expression comes from the sibling render block

## Migration Plan

### Phase 2: Remove RenderSpec from WidgetSpec access

Replace `widgetSpec.renderSpec` with the **sibling `render` block** approach:

The Rust backend already loads the sibling render block in `load_root_layout_block()`
(`render_src.content AS render_source`), but `initial_widget()` currently ignores it.
The plan is to:
1. Parse the `render_source` into a `RenderExpr` (or collection-level profile with variants)
2. Pass it through `WidgetSpec` (new field) or alongside it to Flutter
3. Use the same variants mechanism as row-level profiles — the collection can have
   a default rendering (e.g., `table`) and conditional variants (e.g., `tree` when
   parent_id column exists), plus collection-level operations

**Rust changes needed:**
- `backend_engine.rs:initial_widget()` — parse `render_source` from root block into a
  `RenderExpr` and include it in the response (add field to `WidgetSpec` or return separately)

**Flutter changes:**
- `lib/main.dart:826` — use the collection-level render expr from the sibling render block
- `lib/providers/query_providers.dart` — change QueryResult typedef, remove RenderSpec
- `lib/render/live_query_widget.dart:74` — pass collection render expr, not renderSpec

### Phase 3: Replace RowTemplate usage with RowProfile

Each row now carries its own profile. Replace template-index-lookup pattern:
```dart
// OLD: lookup by ui column index
final template = rowTemplates.firstWhere((t) => t.index == uiIndex);
// NEW: use row's own profile
final profile = resolvedRow.profile;
final renderExpr = profile?.render ?? defaultExpr;
```

Files:
- `lib/render/render_context.dart` — replace `List<RowTemplate> rowTemplates` with profile access
- `lib/render/reactive_query_widget.dart` — replace `renderSpec` field with `rootExpr`
- `lib/render/tree_view_widget.dart` — use row profiles instead of rowTemplates
- `lib/render/tree_node_widget.dart` — use row profile instead of template
- `lib/render/builders/tree_builder.dart` — construct from profiles
- `lib/render/builders/draggable_builder.dart` — use row profile
- `lib/render/search_select_overlay.dart` — use row profile
- `lib/render/list_item_widget.dart` — use row profile

### Phase 4: Update RenderableItem

`lib/render/renderable_item_ext.dart` — replace `RowTemplate template` with profile-based access.
Used by draggable/drop-zone builders for DnD data payload.

### Phase 5: Update state providers

`lib/providers/ui_state_providers.dart` — remove `List<RowTemplate> rowTemplates` from
SearchSelectOverlayState. Replace with profile-based approach.

### Phase 6: Clean up

- Delete `lib/utils/render_spec_extension.dart` (extension on deleted RenderSpec)
- Update `lib/services/mock_backend_service.dart` — use WidgetSpec without renderSpec
- Remove `buildView()`, `_getViewSpec()`, `applyFilter()`, `evaluateFilter()` from
  `lib/render/render_interpreter.dart` (ViewSpec/FilterExpr dependencies)

## Available Dart Types (FRB-generated)

From `lib/src/rust/third_party/holon_api/widget_spec.dart`:
- `WidgetSpec` — `{data: List<ResolvedRow>, actions: List<ActionSpec>}`
- `ResolvedRow` — `{data: Map<String, Value>, profile: RowProfile?}`
- `ActionSpec` — `{id, displayName, icon?, operation: ActionOperation}`

From `lib/src/rust/third_party/holon_api/render_types.dart`:
- `RowProfile` — `{name: String, render: RenderExpr, operations: List<OperationDescriptor>}`
- `RenderExpr` — freezed sealed class (FunctionCall, ColumnRef, Literal, BinaryOp, Array, Object)
- `OperationDescriptor` — full operation metadata
- `OperationWiring` — `{widgetType, modifiedParam, descriptor}`
- `Arg` — `{name: String?, value: RenderExpr}`

NOT available (FRB ignored — exist in Rust but not generated):
- `RenderSpec`, `RowTemplate`, `ViewSpec`, `FilterExpr`, `Operation`, `RenderableItem`
