---
title: Reactive View System
type: concept
tags: [reactive, viewmodel, futures-signals, mvvm]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-frontend/src/reactive.rs
  - crates/holon-frontend/src/reactive_view.rs
  - crates/holon-frontend/src/reactive_view_model.rs
  - crates/holon-frontend/src/render_interpreter.rs
  - frontends/gpui/src/views/reactive_shell.rs
---

# Reactive View System

The reactive view system translates `UiEvent` streams from `watch_ui` into a live `ReactiveViewModel` tree that frontends subscribe to.

## Design Philosophy

The old architecture had separate `ReactiveCollection`, `BlockWatchRegistry`, and `AppState` that required external wiring. The current design unifies these into self-managing components:

```
Turso IVM → UiEvent → ReactiveQueryResults → Signal<ViewModel> → Stream → Frontend
              (IS the cache)      (IS the join)       (IS the API)
```

## ReactiveQueryResults

`crates/holon-frontend/src/reactive.rs` — the reactive cache for a single query/block.

```rust
pub struct ReactiveQueryResults {
    pub render_expr: Mutable<RenderExpr>,    // initialized to loading()
    pub rows: MutableBTreeMap<String, Arc<DataRow>>,
}
```

- `render_expr` starts as `loading()` — a `FunctionCall { name: "loading" }` — so frontends render nothing until real data arrives
- `rows` is a `MutableBTreeMap` from futures-signals — sorted by key for deterministic rendering
- `apply_event(ui_event)` applies both `Structure` and `Data` events

## ReactiveEngine

`crates/holon-frontend/src/reactive.rs` — the top-level coordinator.

Implements `BuilderServices`. Manages:
- `watchers: HashMap<EntityUri, ReactiveQueryResults>` — one per watched block
- `session: Arc<FrontendSession>` — access to `BackendEngine` and Tokio runtime
- `rt_handle` — for spawning async tasks from sync builder code

Key method: `watch(block_id)` — starts `watch_ui(block_id)` if not already running, returns a reference to the `ReactiveQueryResults`.

The single `interpret()` method is the only entry point for DSL evaluation. Builders call `ctx.services.interpret(expr, ctx)` — never `RenderInterpreter` directly.

## ReactiveView

`crates/holon-frontend/src/reactive_view.rs` — self-managing reactive view.

```rust
pub struct ReactiveView {
    inner: ReactiveViewInner,
    pub items: MutableVec<Arc<ReactiveViewModel>>,
    driver_handle: Mutex<Option<AbortHandle>>,
}
```

`start()` spawns the collection driver task. `stop()` aborts it. `Drop` also stops it.

Two variants:
- `Block` — subscribes to a block's `ReactiveQueryResults`, manages child block_refs as sub-`ReactiveView`s
- `Collection` — renders rows from a `ReactiveQueryResults` using an `item_template`

The `items` `MutableVec` is the signal output. Frontends (GPUI, Flutter) subscribe to `items.signal_vec()` for incremental `VecDiff` updates.

## ReactiveViewModel (live node)

`crates/holon-frontend/src/reactive_view_model.rs` — a live node. Wraps `ViewKind` + `items` + optional `operations`.

```rust
pub struct ReactiveViewModel {
    pub kind: ViewKind,
    pub items: MutableVec<Arc<ReactiveViewModel>>,
    pub operations: Vec<OperationWiring>,
}
```

`ViewKind` is the rich enum: `Text { content }`, `EditableText { content, field, ... }`, `List { gap }`, `Table { columns }`, `TableRow { data }`, `BlockRef { id }`, `Columns { gap }`, `StateToggle { state, ... }`, `Loading`, `Empty`, `Error { message }`, etc.

## Row Render Context

When interpreting a collection item, `row_render_context(row, services)` builds a `RenderContext` with:
1. The row's `DataRow` attached
2. Operations resolved from `EntityProfile` for that row

This lets `state_toggle(col("task_state"))` get wired to the right `set_state` operation for each row without the builder knowing the entity type.

## ReactiveShell (GPUI)

`frontends/gpui/src/views/reactive_shell.rs` — GPUI entity that subscribes to a `WatchHandle` and drives state mutations.

On `UiEvent::Structure`: re-interprets the `RenderExpr` via `RenderInterpreter`, produces new `ReactiveViewModel`, calls `cx.notify()`.
On `UiEvent::Data`: applies `BatchMapChange` to the row cache, updates child `ReactiveViewModel` items, calls `cx.notify()`.

## Streaming-First Render State

`render_expr` is always `Mutable<RenderExpr>`, never `Option<RenderExpr>`. The initial value `loading()` flows through the same interpreter → builder → render pipeline as any real widget. The `loading` builder produces `ViewKind::Empty` so frontends render nothing without any null-checks.

## futures-signals

All reactive state uses `futures-signals`:
- `Mutable<T>` — single value signal (like an `Arc<RwLock<T>>` with change notification)
- `MutableVec<T>` — ordered list signal, emits `VecDiff` (Insert, Remove, Replace, etc.)
- `MutableBTreeMap<K, V>` — sorted map signal, emits `MapDiff`
- `signal_vec()`, `signal_map()` — convert to stream of diffs

The `map_ref!` macro creates a derived signal from multiple inputs.

## Related Pages

- [[concepts/cdc-and-streaming]] — the event sources
- [[entities/holon-frontend]] — builder system
- [[entities/gpui-frontend]] — GPUI consumer
- [[concepts/entity-profile]] — profile resolution per row
