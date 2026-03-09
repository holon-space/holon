---
title: GPUI Frontend (primary)
type: entity
tags: [frontend, gpui, ui, reactive]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - frontends/gpui/src/lib.rs
  - frontends/gpui/src/main.rs
  - frontends/gpui/src/di.rs
  - frontends/gpui/src/views/
  - frontends/gpui/src/render/
  - frontends/gpui/src/user_driver.rs
  - frontends/gpui/src/navigation_state.rs
---

# GPUI Frontend

The **primary frontend**, implemented in GPUI (GPU-accelerated native Rust UI framework by Zed). Priority #1 because it runs natively on all platforms including Android.

## Entry Point

`frontends/gpui/src/main.rs` — starts a GPUI app, creates the Tokio runtime, initializes `FrontendSession` via DI, and opens the main window with `AppModel`.

## AppModel

`frontends/gpui/src/lib.rs` — GPUI entity (reactive model) for the application.

```rust
struct AppModel {
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    focus: FocusRegistry,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    root_vm: ReactiveViewModel,
    view_model: ViewModel,
    shadow_ctx: RenderContext,
    root_block_refs: HashMap<String, Entity<views::ReactiveShell>>,
    // ...
}
```

`root_vm` is the live reactive tree for the root layout. `view_model` is a static snapshot produced from `root_vm` on each GPUI update cycle. `root_block_refs` holds `ReactiveShell` GPUI entities for each top-level block (left sidebar, main panel, right sidebar).

## View Architecture

```
frontends/gpui/src/views/
├── reactive_shell.rs    # Per-block reactive wrapper (subscribed to UiEvent stream)
├── editor_view.rs       # Editable text block (cursor, selection, input)
├── live_query_view.rs   # Live query result view
├── render_block_view.rs # Renders a resolved render-source block
└── mod.rs
```

### ReactiveShell

`frontends/gpui/src/views/reactive_shell.rs` — the core per-block GPUI entity. Subscribes to `watch_ui(block_id)` and translates `UiEvent` into GPUI state mutations.

- Receives `UiEvent::Structure` → rebuilds its widget tree using `RenderInterpreter`
- Receives `UiEvent::Data` → applies CDC diffs to child `ReactiveViewModel` nodes
- Emits `cx.notify()` to trigger GPUI re-renders when state changes

### EditorView

`frontends/gpui/src/views/editor_view.rs` — multi-line text editor with GPUI keyboard handling. Wired to `set_field` operations via `BuilderServices::dispatch_intent`.

## Render Builders

`frontends/gpui/src/render/builders/` — GPUI-specific render builder implementations. These convert `ReactiveViewModel` nodes into GPUI `AnyElement` outputs.

Each builder in `holon-frontend/src/shadow_builders/` has a corresponding GPUI renderer here.

`GpuiRenderContext` — carries the GPUI `WindowContext` alongside `RenderContext`. Passed through the builder chain.

## Navigation

`frontends/gpui/src/navigation_state.rs` — `NavigationState` tracks current focus block, navigation history, and region. Wired to keyboard bindings (arrow keys, Tab, Enter for navigation).

`Boundary` enum (from `holon-frontend`): `Block | Region | Document`. `NavDirection`: `Up | Down | Left | Right | Parent | FirstChild`.

## User Driver

`frontends/gpui/src/user_driver.rs` — `GpuiUserDriver` implements `UserDriver` for test automation. Used by PBT infrastructure to drive UI operations without a human.

## Inspector

`frontends/gpui/src/inspector.rs` — debug panel (debug builds only) showing the live `ViewModel` tree and entity profile details. Useful for diagnosing rendering issues.

## DI Wiring

`frontends/gpui/src/di.rs` — GPUI-specific DI setup. Creates `ReactiveEngine` with `FrontendSession`, registers `GpuiUserDriver`, wires org sync controller.

## Key Patterns

### GPUI Entity Pattern

GPUI uses an entity/component model. `AppModel`, `ReactiveShell`, `EditorView` are all GPUI `Entity<T>` types. State mutations go through `cx.update_entity()` and trigger re-renders via `cx.notify()`.

### Avoiding Cascade

The `.cached()` + `size_full()` layout chain on `EditorView` prevents idle render cascades. Without `.cached()`, GPUI would re-render `EditorView` on every tick. This reduced idle cascades from 6000/5s to near-zero.

### Tokio Bridge

The Tokio runtime lives in `AppModel::rt_handle`. Async operations from GPUI callbacks go through `rt_handle.spawn(...)`. CDC stream subscriptions are bridged via `mpsc::channel` with `cx.spawn()` on the GPUI side.

## Related Pages

- [[entities/holon-frontend]] — shared ViewModel layer consumed here
- [[concepts/reactive-view]] — ReactiveView and ReactiveShell design
- [[entities/mcp-frontend]] — parallel MCP frontend
- [[overview]] — architecture overview
