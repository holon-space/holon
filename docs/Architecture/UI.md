Target Architecture: Persistent Reactive ViewModel (MVVM with FRP)

Core principle: A hierarchy of reactive ViewModel nodes, built once, updated in place via `Mutable<T>` (UI-local state — per-instance widget state and app-level singletons) and `Cell<T>` (entity field state). Every node handles its own part of a reactive update and pushes everything else down to its children.

## Three kinds of reactive state

The UI layer holds three distinct kinds of reactive state, with a clear cut between them:

| State kind | Tool | Examples | Authority |
|------------|------|----------|-----------|
| **Per-instance widget state** | `Mutable<T>` on the ViewModel node | tree-item `expanded`, view-mode-switcher selection, scroll position, drag offset, hover, cursor blink | None — UI-local. Lost on reload. |
| **App-level UI singletons** | `Mutable<T>` on `UiState` (`crates/holon-frontend/src/reactive.rs`) | `focused_block`, `pending_caret_seed`, viewport | None — UI-local, but window-global: exactly one focused block per window, and focus must move atomically. Per-instance homes would let two editors both believe they hold focus. |
| **Entity field state** | `Cell<T>` from the per-entity cell registry | block `content`, block `completed`, block `parent_id`, todoist-task `priority`, jira-issue `description` | The entity's authority (Loro for blocks, Todoist API for todoist-task, etc.) — see [Sync](Sync.md). |

**Why not one or the other?** Per-instance widget state needs *per-render-slot* identity: two same-id rows in different trees / regions / panes legitimately need independent expansion state, view mode, etc. (FU-1 lesson — migrating these to a `(uri, field)` registry would collapse them and reintroduce a same-id-collision bug class.) Entity field state needs *cross-consumer* identity: every consumer of `block.completed` for the same `block_uri` must see the same value, which is exactly what a `(uri, field)`-keyed registry gives.

The bar for `Cell<T>` is "has identity (uri+field), could be queried/persisted/synced." The bar for raw `Mutable<T>` is "per-instance widget state, no entity identity." Fail loud when this is mistaken — the FU-1 entry in MEMORY.md is the canonical example.

## ViewModel principles

The ViewModel IS the per-instance state. Expand/collapse, view mode, and almost every other frontend state (except view sizes) is a ViewModel concern — a `Mutable<...>` field on the node that owns it. Not a rendering concern pushed to the platform, not a centralized HashMap. Entity field state is *not* on the ViewModel; it's resolved through the cell registry by `(uri, field)`.

Push-down updates: When an input changes (CDC data via cell signals, template change, UI interaction), the affected node receives it and decides locally what to do. It updates itself and pushes changes down to its children. No external tree walk, no reconciliation, no "old tree vs new tree."

One-way sync to frontends: The reactive ViewModel is shared by all UIs (GPUI, Dioxus, TUI, MCP, tests). Platform frontends subscribe to the Mutables and Cells and render accordingly. They don't own state — the ViewModel + cells do.

Minimal change propagation: Any change to one of the inputs triggers only the minimal changes throughout the computation DAG — computed columns, profile selection, per-row interpretation. Not a global re-interpretation of all blocks.

Change sources: Cell signals (entity field state, sourced from CDC + projector) and per-VM Mutables (UI interactions). Block changes reach the UI through the `LiveData<Block>` feed (CDC off the block matview). All flow through the same signal-based reactive graph.

Shared Mutables for broadcast: A collection's item template is a `Mutable<RenderExpr>` cloned into each child ItemNode. Setting it once propagates to all items — each self-reinterprets via `map_ref!` on its (data, template) signals.

Per-node self-interpretation: Each node owns `Mutable<RenderExpr>` (its template) + `ReadOnlyMutable<Arc<DataRow>>` (its data; data flows in from cells through the rendering pipeline). The read-only type is load-bearing: the sole writable handle to a row lives inside `ReactiveRowSet` and `apply_change` is the sole writer, so a node `.set()`-ing its own data is a compile error — data flows in only; the node's writable surface is its template and its per-instance widget Mutables. A `map_ref!` of both produces the rendered output. The node IS a live reactive processor, not just the output of one.

Structural changes: When the backend sends a new RenderExpr for a block, the root node receives it and handles the diff locally — keep matching children, create new ones, drop removed ones. Each child that's kept receives its updated sub-expression and handles it the same way, recursively.

Implementation status: This architecture is IMPLEMENTED — `ReactiveViewModel` (`crates/holon-frontend/src/reactive_view_model.rs`) is the target, reusing the old name; it replaced the earlier snapshot-based `ReactiveViewModel` + `ReactiveViewKind` enum, and [RenderPipeline](RenderPipeline.md) cites it as the canonical shared-VM boundary. `ReactiveView` (`crates/holon-frontend/src/reactive_view.rs`) is part of the design, not a leftover: it is the reactive collection backing a node's children (`ReactiveViewModel.collection`). Sanctioned remainders: `UiState` (`crates/holon-frontend/src/reactive.rs`) holds the app-level UI singletons from the table above, and the GPUI frontend still uses `ui_generation` bumps (`frontends/gpui/src/lib.rs`) for viewport changes — focus changes deliberately do NOT bump it, so no global cascade on focus moves.

## Editable text in the UI

Text-editing widgets (`editable_text` builder, `EditorView`, etc.) consume `Cell<String>` returned by `BuilderServices::editable_text(uri, field)`. The cell exposes:

- `current() -> String` — synchronous read of the live merged text
- `apply_text_op(TextOp)` — character-level insert/delete; Loro-backed cells preserve RGA history, LWW backings degrade to compute-then-replace
- `anchor_cursor` / `resolve_cursor` — cursor anchors that survive remote edits (Loro-backed); byte offsets (LWW)
- `remote_deltas() -> Stream<TextDelta>` — reactive stream of remote-origin changes for incremental editor updates

Chord ops (split, join, embed) read from the cell (`read_content_via_cells` → `cell.current()`) and write through the single `set_field` content-write seam, which routes through `BlockCellRegistry::write_field` (cell-registry-routed, though not a literal `cell.set` call today); they never read the SQL projection directly for content. This dissolved the `BlockContentResolver` hatch from Phase 0+1 — cells ARE the live text source by construction (`BlockContentResolver` no longer exists; archlint bans its return).

See [Storage](Storage.md) for cell internals and [Operations](Operations.md) for the cells-vs-reflective-ops cut.

## Field authority and intent capture

> **The UI is responsible for displaying fields and capturing intent on them — not for their values.**

Ownership of values sits with the entity's authority, because display is many-to-one but authority can't be: two views showing the same block would otherwise both "own" its content. The editor is structurally just another replica with uncommitted local changes — same as a peer, a webhook, or a file reload. Its only legitimate privilege is the **optimistic fast path**: its changes apply locally without a round-trip because the user is watching. The moment an op signature accepts the UI's view of a field as truth-by-assertion (e.g. "split, and here is the full content"), the UI becomes a special replica whose changes overwrite instead of merge — and the asymmetry bites as soon as a second change source exists. Under Loro it is strictly worse: whole-content writes convert a CRDT merge into last-writer-wins.

A structural op decomposes into three responsibilities with three owners (worked example: splitting a Todoist task into two):

| Responsibility | Owner | Todoist-split example |
|---|---|---|
| **Intent capture** — live text deltas, caret, "split here" | UI | only the editor knows the caret and the in-flight keystrokes |
| **Distribution policy** — what the op does to each field | domain op on the **local authority** | new task inherits priority, status resets to open, subtasks stay |
| **Remote materialization** — turning the result into API calls | connector | `POST /tasks` + `PATCH /tasks/:id`, rate limits, retries |

Two contracts follow:

1. **Structural ops are commit points.** Any pending editor state flushes through the normal merge path *before* the op executes, in one ordered dispatch (a single task awaiting commit then op — two fire-and-forget dispatches can reorder). The op then always computes against the authority's current state. Canonical failure from violating this: `Split position 8 exceeds content length 3` (2026-06-11) — `split_block` computed against backend content (`"797"`) using the editor's cursor byte into its pending text (`"ßñ😀中797"`); SqlOnly's commit-on-blur left the two permanently divergent until a blur happened to fire.

2. **Sync-boundary batching lives in connectors, not widgets.** Batching keystrokes inside the editor until blur (SqlOnly today) implements a sync-boundary transaction inside a UI widget — the cursor then indexes a revision the authority has never seen. The transaction belongs at the authority↔external boundary (the Todoist connector batches toward the API; Loro batches via the CRDT), while UI→local-authority stays per-keystroke (debounce is a write-path detail, not an ownership change). Blur becomes a flush *hint*, not the commit mechanism.

Intents that reference positions should carry an **anchor** (the revision the position was measured against, or a CRDT cursor via `anchor_cursor`), so the authority can transform the position if a merge landed in between — a bare byte offset is the same stale-pointer bug in miniature.