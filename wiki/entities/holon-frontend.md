---
title: holon-frontend crate (ViewModel / MVVM layer)
type: entity
tags: [crate, frontend, mvvm, reactive, viewmodel]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-frontend/src/lib.rs
  - crates/holon-frontend/src/reactive.rs
  - crates/holon-frontend/src/reactive_view.rs
  - crates/holon-frontend/src/reactive_view_model.rs
  - crates/holon-frontend/src/render_interpreter.rs
  - crates/holon-frontend/src/render_context.rs
  - crates/holon-frontend/src/view_model.rs
  - crates/holon-frontend/src/shadow_builders/
---

# holon-frontend crate

The **platform-agnostic ViewModel layer** shared by all frontends (GPUI, Flutter, MCP, TUI). Translates `UiEvent` streams from `watch_ui` into a `ViewModel` tree that frontends render. Implements the render DSL builder system via shadow builders.

## Key Abstractions

### ViewModel (static snapshot)

`crates/holon-frontend/src/view_model.rs` — an immutable snapshot of the UI tree, used by frontends for rendering. Can be serialized to JSON or pretty-printed. Used by MCP for the `get_display_tree` tool.

### ReactiveViewModel (live node)

`crates/holon-frontend/src/reactive_view_model.rs` — a live node in the reactive tree. Each node has:
- `kind: ViewKind` — enum describing the widget (Text, List, Table, BlockRef, etc.)
- `items: MutableVec<Arc<ReactiveViewModel>>` — reactive children
- `operations: Vec<OperationWiring>` — available operations for dispatch

`ViewKind` variants include: `Text`, `EditableText`, `Outline`, `List`, `Table`, `TableRow`, `BlockRef`, `Card`, `Columns`, `StateToggle`, `Spacer`, `Error`, `Loading`, `Empty`, etc.

### ReactiveView

`crates/holon-frontend/src/reactive_view.rs` — self-managing reactive view that owns its streaming pipeline. Replaces the old `ReactiveCollection + wire_collection_drivers` pattern.

Variants:
- `Block { data_source, item_template }` — watches a block's own watcher + manages child block_refs
- `Collection { layout, data_source, item_template }` — renders rows from a data source

```rust
pub struct ReactiveView {
    inner: ReactiveViewInner,
    pub items: MutableVec<Arc<ReactiveViewModel>>,
    driver_handle: Mutex<Option<AbortHandle>>,
}
```

`start()` spawns the driver task. `stop()` / `Drop` abort it.

### ReactiveQueryResults

`crates/holon-frontend/src/reactive.rs` — the reactive cache for a single query. Owns:
- `render_expr: Mutable<RenderExpr>` — initialized to `loading()`, replaced on first `Structure` event
- `rows: MutableBTreeMap<String, Arc<DataRow>>` — live CDC-maintained row map

Initialized from `UiEvent` stream via `apply_event()`.

### ReactiveEngine

`crates/holon-frontend/src/reactive.rs` — the top-level reactive coordinator. Implements `BuilderServices`. Manages `watch_ui` lifecycle per block ID, provides `interpret()` for render DSL evaluation.

### BuilderServices Trait

```rust
pub trait BuilderServices: Send + Sync {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel;
    fn get_block_data(&self, id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>);
    fn resolve_profile(&self, row: &DataRow) -> Option<RowProfile>;
    fn compile_to_sql(&self, query: &str, lang: QueryLanguage) -> Result<String>;
    fn start_query(&self, sql: String, ctx: Option<QueryContext>) -> Result<RowChangeStream>;
    fn widget_state(&self, id: &str) -> WidgetState;
    fn dispatch_intent(&self, intent: OperationIntent);
    fn get_ui_state(&self) -> UiState;
}
```

Builders never see `FrontendSession` or `ReactiveEngine` — they call these narrow methods through `ctx.services`. This is the single entry point for row interpretation in the reactive pipeline.

### RenderContext

`crates/holon-frontend/src/render_context.rs` — the context passed into builders during interpretation. Carries:
- Current `DataRow` (if inside a collection)
- `BuilderServices` reference
- Ancestry chain (parent block_ids)
- Current `operations: Vec<OperationWiring>`

### RenderInterpreter

`crates/holon-frontend/src/render_interpreter.rs` — dispatches `RenderExpr` to builders. Calls `shadow_builders/<name>.rs::render()` by matching the function name in `RenderExpr::FunctionCall`.

## Shadow Builders

`crates/holon-frontend/src/shadow_builders/` — one file per widget builder. Each `render()` function takes a `RenderContext` and relevant args, returns `ReactiveViewModel`.

Key builders:

| Builder | File | Description |
|---------|------|-------------|
| `text` | text.rs | Static/dynamic text from `col()` or literal |
| `list` | list.rs | Vertical list with `item_template` |
| `table` | table.rs | Data table with column definitions |
| `columns` | columns.rs | Side-by-side column layout |
| `block_ref` | block_ref.rs | Recursively renders a block by ID |
| `render_block` | render_block.rs | Renders a sibling render source block |
| `card` | card.rs | Card container widget |
| `outline` | outline.rs | Hierarchical outline with collapse |
| `editable_text` | editable_text.rs | Inline text editor wired to `set_field` |
| `state_toggle` | state_toggle.rs | Task state button wired to `set_state` |
| `checkbox` | checkbox.rs | Boolean toggle |
| `spacer` | spacer.rs | Fixed-size gap |
| `row` | row.rs | Horizontal row container |
| `loading` | loading.rs | Loading placeholder (produces Empty ViewModel) |
| `error` | error.rs | Error display widget |
| `source_block` | source_block.rs | Syntax-highlighted source display |
| `source_editor` | source_editor.rs | Editable source block |
| `tree` | tree.rs | Tree data structure display |
| `live_query` | live_query.rs | Inline live query widget |

The builder registry macro (`holon-macros::builder_registry`) generates the dispatch table at compile time by parsing `pub fn render(` signatures from each builder file.

## FrontendSession

`crates/holon-frontend/src/lib.rs` — holds the runtime-facing services: `BackendEngine`, Tokio runtime handle, `FrontendConfig`. All frontends create one `FrontendSession` at startup.

## WidgetState & UserDriver

`crates/holon-frontend/src/user_driver.rs` and `widget_gallery.rs` — abstract input handling. `UserDriver` trait for keyboard/mouse events. `WidgetState` caches per-block UI state (focus, collapse, edit mode).

## Related Pages

- [[entities/gpui-frontend]] — consumes this layer
- [[concepts/reactive-view]] — ReactiveView + ReactiveViewModel design
- [[concepts/entity-profile]] — how profiles are resolved per row
- [[entities/holon-api]] — RenderExpr, UiEvent, DataRow definitions
