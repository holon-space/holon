# Review 0005: Implement `render_block` Primitive with Live Query Support

## Outcomes

- **Created `LiveQueryWidget`** (`lib/render/live_query_widget.dart`)
  - Executes PRQL queries using `queryResultByPrqlProvider` family provider
  - Renders results with `ReactiveQueryWidget` and handles loading/error states
  - Each instance creates its own independent CDC subscription for live updates

- **Added `render_block` primitive to RenderInterpreter**
  - Polymorphic dispatch based on block `content_type` and `source_language`
  - Three dispatch paths:
    1. `content_type: "source"` + `source_language: "prql"` → `LiveQueryWidget` (executes embedded query)
    2. `content_type: "source"` + other language → `source_editor` (syntax highlighting)
    3. Default → `editable_text` (standard text editing)
  - Enables nested live queries: journal pages can contain query blocks that show live-updating results

- **Cleaned up deprecated code**
  - Removed `NestedQueryWidget` trivial wrapper from `reactive_query_widget.dart` (lines 650-686)
  - No usages found in codebase, safe to delete

## Implementation Details

### Files Created
- `frontends/flutter/lib/render/live_query_widget.dart` - New widget for PRQL query execution

### Files Modified
- `frontends/flutter/lib/render/render_interpreter.dart`
  - Added import for `LiveQueryWidget`
  - Added `case 'render_block':` to switch statement (line ~256)
  - Added `_buildRenderBlock()` method (lines 2753-2801)

### Files Deleted
- `frontends/flutter/lib/render/reactive_query_widget.dart` - Removed `NestedQueryWidget` class

## Validation

- ✅ **Compilation**: `flutter build macos` succeeds
  - Fixed initial compilation errors:
    - `RenderExpr.literal()` requires named `value:` parameter
    - Added null assertion operator for `prqlSource` after assert check
  - Build output: `build/macos/Build/Products/Release/holon.app` (168.5MB)

- ✅ **Linting**: No linter errors in modified files

- ✅ **Bug Fix During Testing**: Fixed column name mismatch
  - Original implementation used `rowData['source_code']` but DB schema uses `content` column
  - Fixed in `_buildRenderBlock()` to use `context.rowData['content']`
  - Also fixed `index_layout_provider.dart` which selected non-existent `source_code` column

- ⚠️ **Runtime Testing**: Partial - data layer verified, UI rendering needs Flutter app
  - ✅ Created test PRQL block with `content_type: "source"`, `source_language: "prql"`, `content: "<prql>"`
  - ✅ Query with `render_block` in render spec compiles and returns correct data
  - ⚠️ LiveQueryWidget execution and CDC streaming require Flutter app to verify

## Architecture Notes

### Key Insight
`queryResultByPrqlProvider` already handles all the heavy lifting:
1. Takes PRQL string as family parameter (automatic keying)
2. Compiles PRQL to SQL + RenderSpec
3. Executes query and gets initial data
4. Sets up CDC stream

`LiveQueryWidget` is a thin wrapper that just connects the provider to `ReactiveQueryWidget`. No new compilation or streaming logic needed.

### Dispatch Logic
The `render_block` primitive reads block metadata from `context.rowData`:
- Checks `content_type` and `source_language` fields
- Falls back to `content` column for text blocks
- Uses assertions for error handling (fail hard on missing required fields)

## Follow-ups / Risks

### Known Limitations
- **Cycle detection**: Not implemented (deferred until needed)
- **Query result caching**: Identical `source_code` blocks create separate provider instances (by design for independent CDC streams)
- **Lazy loading**: All query blocks execute immediately when rendered (no off-screen optimization)
- **Multi-view support**: Nested queries with multiple views use `renderSpec.defaultView` only (future enhancement: view switcher for nested queries)

### Future Enhancements
- Optionally show view switcher for nested queries with multiple views
- Consider renaming `ReactiveQueryWidget.sql` parameter to `queryKey` for clarity
- Add cycle detection if nested query patterns become complex

### Testing Recommendations
1. Create a test block with:
   - `content_type: "source"`
   - `source_language: "prql"`
   - `content: "from blocks select { id, content } render (list item_template:(text content))"`
2. Render it via a parent query using `render_block(this)`
3. Verify:
   - The nested query executes
   - Results appear inline
   - CDC updates flow through (modify a block, see it update in nested query)

## Related Documents
- Implementation handoff: `frontends/flutter/HANDOFF_RENDER_BLOCK.md`
- Multi-view render context: `docs/HANDOFF_MULTI_VIEW_RENDER.md`
- Layout architecture: `frontends/flutter/HANDOFF_LAYOUT_ARCHITECTURE.md`
