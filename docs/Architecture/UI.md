Target Architecture: Persistent Reactive ViewModel (MVVM with FRP)

Core principle: A hierarchy of reactive ViewModel nodes, built once, updated in place via `Mutable<T>` (per-instance widget state) and `Cell<T>` (entity field state). Every node handles its own part of a reactive update and pushes everything else down to its children.

## Two layers of reactive state

The UI layer holds two distinct kinds of reactive state, with a clear cut between them:

| State kind | Tool | Examples | Authority |
|------------|------|----------|-----------|
| **Per-instance widget state** | `Mutable<T>` on the ViewModel node | tree-item `expanded`, view-mode-switcher selection, focused_block, scroll position, drag offset, hover, cursor blink | None — UI-local. Lost on reload. |
| **Entity field state** | `Cell<T>` from the per-entity cell registry | block `content`, block `completed`, block `parent_id`, todoist-task `priority`, jira-issue `description` | The entity's authority (Loro for blocks, Todoist API for todoist-task, etc.) — see [Sync](Sync.md). |

**Why not one or the other?** Per-instance widget state needs *per-render-slot* identity: two same-id rows in different trees / regions / panes legitimately need independent expansion state, view mode, etc. (FU-1 lesson — migrating these to a `(uri, field)` registry would collapse them and reintroduce a same-id-collision bug class.) Entity field state needs *cross-consumer* identity: every consumer of `block.completed` for the same `block_uri` must see the same value, which is exactly what a `(uri, field)`-keyed registry gives.

The bar for `Cell<T>` is "has identity (uri+field), could be queried/persisted/synced." The bar for raw `Mutable<T>` is "per-instance widget state, no entity identity." Fail loud when this is mistaken — the FU-1 entry in MEMORY.md is the canonical example.

## ViewModel principles

The ViewModel IS the per-instance state. Expand/collapse, view mode, and almost every other frontend state (except view sizes) is a ViewModel concern — a `Mutable<...>` field on the node that owns it. Not a rendering concern pushed to the platform, not a centralized HashMap. Entity field state is *not* on the ViewModel; it's resolved through the cell registry by `(uri, field)`.

Push-down updates: When an input changes (CDC data via cell signals, template change, UI interaction), the affected node receives it and decides locally what to do. It updates itself and pushes changes down to its children. No external tree walk, no reconciliation, no "old tree vs new tree."

One-way sync to frontends: The reactive ViewModel is shared by all UIs (GPUI, Dioxus, TUI, MCP, tests). Platform frontends subscribe to the Mutables and Cells and render accordingly. They don't own state — the ViewModel + cells do.

Minimal change propagation: Any change to one of the inputs triggers only the minimal changes throughout the computation DAG — computed columns, profile selection, per-row interpretation. Not a global re-interpretation of all blocks.

Change sources: Cell signals (entity field state, sourced from CDC + projector), per-VM Mutables (UI interactions), and possibly the event bus in the future. All flow through the same signal-based reactive graph.

Shared Mutables for broadcast: A collection's item template is a `Mutable<RenderExpr>` cloned into each child ItemNode. Setting it once propagates to all items — each self-reinterprets via `map_ref!` on its (data, template) signals.

Per-node self-interpretation: Each node owns `Mutable<RenderExpr>` (its template) + `Mutable<Arc<DataRow>>` (its data; data flows in from cells through the rendering pipeline). A `map_ref!` of both produces the rendered output. The node IS a live reactive processor, not just the output of one.

Structural changes: When the backend sends a new RenderExpr for a block, the root node receives it and handles the diff locally — keep matching children, create new ones, drop removed ones. Each child that's kept receives its updated sub-expression and handles it the same way, recursively.

Clean slate: This is not an evolution of the current ReactiveViewModel / ReactiveView / UiState architecture. It reuses useful components (futures-signals, RenderExpr, DataRow, the mini-interpreter concept) but is architecturally independent — no fallbacks to or remainders from centralized state, ephemeral trees, or ui_generation cascades.

## Editable text in the UI

Text-editing widgets (`editable_text` builder, `EditorView`, etc.) consume `Cell<String>` returned by `BuilderServices::editable_text(uri, field)`. The cell exposes:

- `current() -> String` — synchronous read of the live merged text
- `apply_text_op(TextOp)` — character-level insert/delete; Loro-backed cells preserve RGA history, LWW backings degrade to compute-then-replace
- `anchor_cursor` / `resolve_cursor` — cursor anchors that survive remote edits (Loro-backed); byte offsets (LWW)
- `remote_deltas() -> Stream<TextDelta>` — reactive stream of remote-origin changes for incremental editor updates

Chord ops (split, join, embed) read from the cell (`current()`) and write through the cell (`set(new_string)`); they never read the SQL projection directly for content. This dissolves the `BlockContentResolver` hatch from Phase 0+1 — cells ARE the live text source by construction.

See [Storage](Storage.md) for cell internals and [Operations](Operations.md) for the cells-vs-reflective-ops cut.