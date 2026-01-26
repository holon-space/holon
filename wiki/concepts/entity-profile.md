---
title: EntityProfile System (Runtime Render Resolution)
type: concept
tags: [entity-profile, rendering, runtime, rhai, variants]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon/src/entity_profile.rs
  - crates/holon-api/src/render_types.rs
  - crates/holon-frontend/src/reactive.rs
---

# EntityProfile System

The EntityProfile system resolves **per-row rendering at runtime** based on row data and Rhai conditions. This replaced the old compile-time approach where `RenderSpec` was extracted statically from PRQL queries.

## Why Runtime?

Old approach: `PRQL + render()` → compile time → static `RenderSpec` describing the entire UI.

Problem: Can't mix entity types in one query (Todoist task vs org headline need different renders). Can't vary rendering based on runtime UI state (e.g., focused block gets edit mode).

New approach: Each row carries enough data to resolve which `EntityProfile` it matches. The profile defines `RenderVariant`s with Rhai conditions; the frontend picks the active variant based on local UI state.

## EntityProfile

`crates/holon/src/entity_profile.rs` — defines a set of `RenderVariant`s for an entity type.

Each `EntityProfile` matches blocks by:
- `content_type` (Headline, Source, etc.)
- `source_language` (for source blocks)
- Entity-specific properties

## RenderVariant

`crates/holon-api/src/render_types.rs`:
```rust
pub struct RenderVariant {
    pub name: String,           // e.g., "default", "focused", "editing"
    pub render: RenderExpr,     // the render expression for this variant
    pub operations: Vec<OperationDescriptor>, // available operations
    pub condition: Predicate,   // when to use this variant
}
```

## RenderProfile (resolved per row)

```rust
pub struct RenderProfile {
    pub name: String,
    pub render: RenderExpr,
    pub operations: Vec<OperationDescriptor>,
    pub variants: Vec<RenderVariant>,  // all candidates
}
```

`RenderProfile` is sent to the frontend. The frontend evaluates `condition` against local `UiState` (focus, view mode) to pick the active variant.

## ProfileResolving Trait

```rust
pub trait ProfileResolving: Send + Sync {
    fn resolve_profile(&self, row: &DataRow) -> Option<RowProfile>;
    fn subscribe_version(&self) -> broadcast::Receiver<u64>;
}
```

`subscribe_version()` returns a channel that fires when entity profiles change (e.g., a profile block is edited). This triggers `RenderTrigger::ProfileChange` in `watch_ui`, causing a re-render without incrementing generation.

## RenderInterpreter + Builders

When interpreting a collection row, `row_render_context(row, services)` in `reactive_view.rs`:
1. Calls `services.resolve_profile(row)`
2. Attaches the profile's operations to the `RenderContext`
3. Passes `ctx` to the builder

This means builders like `state_toggle(col("task_state"))` automatically get the right `set_state` operation wired for that entity type.

## SetVariant Command

`WatcherCommand::SetVariant(name)` sent to `WatchHandle::command_tx` switches the active variant. Same `generation` (no structural re-render). The data forwarder restarts with updated profile context.

## Predicate Evaluation

`Predicate` is evaluated by the frontend against `UiState`:
```rust
pub struct UiState {
    pub is_focused: bool,
    pub view_mode: String,  // "tree", "table", "kanban"
    pub collapsed: bool,
}
```

This allows `condition: Predicate::Eq(UiField::ViewMode, "table")` to activate a table-row variant when in table mode.

## Related Pages

- [[concepts/reactive-view]] — how profiles are consumed during row interpretation
- [[concepts/query-pipeline]] — rendering is decoupled from query compilation
- [[entities/holon-api]] — `RenderProfile`, `RenderVariant`, `Predicate`
