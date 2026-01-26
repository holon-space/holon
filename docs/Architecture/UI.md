Target Architecture: Persistent Reactive ViewModel (MVVM with FRP)

Core principle: A hierarchy of reactive ViewModel nodes, built once, updated in place via Mutable<T> fields. Every node handles its own part of a reactive update and pushes everything else down to its children.

The ViewModel IS the state. Expand/collapse, view mode, and almost every other frontend state (except view sizes) is a ViewModel concern — a Mutable<...> field on the node that owns it. Not a rendering concern pushed to the platform, not a centralized HashMap.

Push-down updates: When an input changes (CDC data, template change, UI interaction), the affected node receives it and decides locally what to do. It updates itself and pushes changes down to its children. No external tree walk, no reconciliation, no "old tree vs new tree."

One-way sync to frontends: The reactive ViewModel is shared by all UIs (GPUI, Dioxus, TUI, MCP, tests). Platform frontends subscribe to the Mutables and render accordingly. They don't own state — the ViewModel does.

Minimal change propagation: Any change to one of the inputs triggers only the minimal changes throughout the computation DAG — computed columns, profile selection, per-row interpretation. Not a global re-interpretation of all blocks.

Change sources: Turso CDC (data), UI interactions (expand, mode switch), and possibly the event bus in the future. All flow through the same Mutable-based signal graph.

Shared Mutables for broadcast: A collection's item template is a Mutable<RenderExpr> cloned into each child ItemNode. Setting it once propagates to all items — each self-reinterprets via map_ref! on its (data, template) signals.

Per-node self-interpretation: Each node owns Mutable<RenderExpr> (its template) + Mutable<Arc<DataRow>> (its data). A map_ref! of both produces the rendered output. The node IS a live reactive processor, not just the output of one.

Structural changes: When the backend sends a new RenderExpr for a block, the root node receives it and handles the diff locally — keep matching children, create new ones, drop removed ones. Each child that's kept receives its updated sub-expression and handles it the same way, recursively.

Clean slate: This is not an evolution of the current ReactiveViewModel / ReactiveView / UiState architecture. It reuses useful components (futures-signals, RenderExpr, DataRow, the mini-interpreter concept) but is architecturally independent — no fallbacks to or remainders from centralized state, ephemeral trees, or ui_generation cascades.