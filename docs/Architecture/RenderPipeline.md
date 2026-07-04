# Query & Render Pipeline

*Part of [Architecture](../Architecture.md)*



## Query Compilation by Language

```
PRQL string ──→ prqlc compile → SQL (pure data query, no render directives)
GQL string  ──→ gql_parser::parse → AST → gql_transform::transform(&query, &GraphSchema) → SQL → gql_params_to_dollar (`:param` → `$param`)
SQL string  ──→ (used directly)
```

All three paths produce pure SQL. Rendering is **decoupled** from query compilation — it is handled by the EntityProfile system at runtime (see [EntityProfile System](#entityprofile-system-render-architecture)).

### EAV Graph Schema

GQL queries operate on an Entity-Attribute-Value schema with 14 tables:
- `nodes`, `edges` — graph structure
- `node_labels` — label-based node classification
- `property_keys` — shared key dictionary
- `node_props_{int,text,real,bool,json}` — typed node properties
- `edge_props_{int,text,real,bool,json}` — typed edge properties

GQL also operates on ordinary tables with foreign key relations — not only the EAV schema. The unifying mechanism is the `GraphSchema` passed to `gql_transform::transform` (cached in `BackendEngine::graph_schema_cache`): it maps relational tables (`blocks`, `documents`) into graph nodes alongside the EAV tables, so both are queryable as one graph. After transform, `gql_params_to_dollar` normalizes GQL `:param` placeholders to `$param` — anyone debugging GQL parameters will hit this. The schema is initialized idempotently (all `IF NOT EXISTS`) during database startup.

### EntityProfile System (Render Architecture)

Render specifications are resolved **at runtime per-row** via the EntityProfile system.

**Location**: data model + `ProfileResolving` trait in
`crates/holon-api/src/entity_profile.rs` (storage de-leak Stage 10);
`ProfileResolver` and profile parsing live in the `holon-profiles` crate
(`crates/holon-profiles/src/lib.rs`, re-exported as `holon::entity_profile`).

#### Overview

```
PRQL / GQL / SQL → SQL → Turso → Vec<DataRow>
                                       ↓
                       EntityProfile.resolve(row, context)
                                       ↓
                       RowProfile { render, operations } per row
                                       ↓
                   ReactiveViewModel tree (holon-frontend)
                                       ↓
                         Frontend-specific View layer
```

**Render source blocks** use Rhai syntax in org blocks with `source_language: render`:
```org
#+BEGIN_SRC render :id my-block::render::0
list(#{item_template: render_entity()})
#+END_SRC
```

#### Widget registry seam

Adding a new widget (kanban, calendar, parametric-style, …) does **not** touch the `RenderExpr` enum, the Rhai parser, or the PBT `DisplayNode` assertions. The seam:

1. **Parse-time** (`crates/holon-api/src/render_dsl.rs`): every identifier-followed-by-`(` in the source is treated as a widget call. The parser scans the source for such names and registers them with the Rhai engine on the fly. `register_widget_names()` is an **optional** startup hint the frontend uses to seed the engine with the canonical builder list; backend tests / headless engines / `action_watcher` work without it because of source-driven discovery.
2. **Interpret-time** (`crates/holon-frontend/src/shadow_builders/`): the auto-generated `builder_registry!` macro picks up any new builder file and exposes it via `builder_names()`. The widget name resolves to the new builder at render time.

A new widget therefore lands as a single new file under `shadow_builders/`. No enum variant, no parser change, no PBT update. See `codev/specs/0006-pre-velocity-refactors.md` Phase 2.

**CDC stream forwarding** (`ui_watcher.rs`):
`watch_ui(block_id)` returns a `WatchHandle` carrying a `Stream<UiEvent>`. `merge_triggers` merges three event sources — structural CDC, `SetVariant` commands, and profile version changes — into a single `RenderTrigger` stream. This drives a `switch_map` that aborts the previous data forwarder and spawns a new one on each trigger. Each CDC Created/Updated event is enriched with profile-resolved computed fields before forwarding.

**Profile Resolution** happens per-row inside those data forwarders (`ui_watcher.rs`: `enrich_batch` / `enrich_row`, via the `Arc<dyn ProfileResolving>` obtained from `BackendEngine::profile_resolver()`):
```
For each row:
  - Look up EntityProfile by row's entity scheme in the `id` column
  - Evaluate Rhai variant conditions against row data
  - Attach matching RowProfile (render expr + operations)
```

**Reactive layer** (`holon-frontend`):
Query results flow into `ReactiveView` (a self-managing reactive collection backed by futures-signals `MutableVec`). Each row is wrapped in a `ReactiveViewModel` — a persistent node that owns its `RenderExpr` and `DataRow` as `Mutable<_>` fields. When either changes the node re-interprets itself and pushes updates to child nodes without rebuilding the tree. `DataRowAccumulator` (`holon-api/src/widget_spec.rs`) is the single source of truth for `Change<DataRow>` → keyed collection conversion.

#### Core Types

```rust
// Location: crates/holon-api/src/entity_profile.rs
pub struct EntityProfile {
    pub entity_name: EntityName,            // "block", "todoist-task"
    /// All variants, INCLUDING the conditionless "default". Sorted by
    /// priority descending at parse time — highest priority checked first.
    pub variants: Vec<StoredVariant>,
    /// Pre-compiled computed fields, topologically sorted.
    pub computed_fields: Vec<CompiledComputedField>,  // (name, CompiledExpr)
    /// Editable placeholder appended to collections of this entity type.
    pub virtual_child: Option<VirtualChildConfig>,
}

pub struct StoredVariant {                  // alias: RowVariant
    pub name: String,
    pub priority: i32,                      // Higher = checked first (seeded defaults: -1)
    pub condition_source: String,           // Full Rhai condition (empty = always matches)
    pub data_condition: Option<String>,     // Data-only part (Rhai, backend-evaluated)
    pub ui_condition: Predicate,            // UI-state part (frontend-evaluated, no round-trip)
    pub profile: Arc<StoredProfile>,
}

pub struct StoredProfile {                  // stored spec: render only, no operations
    pub name: String,
    pub render: RenderExpr,                 // e.g. tree(...), list(...), row(...)
}

// Location: crates/holon-api/src/render_types.rs — the RESOLVED output.
// Operations are injected at resolve time from the entity's registered
// operations, never stored in profile YAML.
pub struct RenderProfile {                  // deprecated alias: RowProfile
    pub name: String,
    pub render: RenderExpr,
    pub operations: Vec<OperationDescriptor>,
    pub variants: Vec<RenderVariant>,       // all matching candidates (multi-variant mode)
}
```

**Resolution algorithm**: conditions are split at parse time into a *data*
part (Rhai, evaluated against row + computed fields) and a *UI* part
(`Predicate` over `is_focused`/`view_mode`/viewport variables, evaluated
frontend-side with no backend round-trip).

1. Look the entity's profile up in the `ProfileCache` by URI scheme; if none
   is registered, return a default empty `RenderProfile` ("no profile attached")
2. Single-variant path (`EntityProfile::resolve`): walk variants in priority
   order (descending); first variant whose full `condition_source` is empty
   or Rhai-evaluates to `true` wins
3. Multi-variant path (`resolve_candidates`, used by
   `ProfileResolving::resolve_with_variants`): collect ALL variants whose
   `data_condition` matches; each carries its `ui_condition` so the frontend
   picks the active one from local UI state and can switch instantly
4. Computed fields are evaluated once per resolution, in topological order,
   with `properties` flattened into the Rhai scope

#### ProfileResolving Trait

```rust
// Location: crates/holon-api/src/entity_profile.rs
pub trait ProfileResolving: Send + Sync {
    fn resolve(&self, row: &HashMap<String, Value>) -> Arc<RenderProfile>;
    fn resolve_with_computed(&self, row) -> (Arc<RenderProfile>, HashMap<String, Value>);
    fn resolve_batch(&self, rows: &[HashMap<String, Value>]) -> Vec<Arc<RenderProfile>>;
    // Defaulted methods:
    fn resolve_with_variants(&self, row) -> (Arc<RenderProfile>, HashMap<String, Value>);
    fn virtual_child_config(&self, entity_name: &str) -> Option<VirtualChildConfig>;
    fn operations_for(&self, entity_name: &str) -> Vec<OperationDescriptor>;
    fn resolve_collection_variants(&self) -> Vec<RenderVariant>;  // tree/table/board view modes
    fn profile_signal(&self) -> Mutable<Arc<ProfileCache>>;       // push-based change signal
}
```

`ProfileResolver` (`crates/holon/src/entity_profile.rs`) loads profiles from
org blocks with the `entity_profile_for` property, layered over type-defined
profiles from the `TypeRegistry`. Profiles are backed by CDC-driven
`LiveData<EntityProfile>` — edits to profile blocks rebuild the cache in a
background task and swap a fresh `Arc<ProfileCache>` into `profile_signal()`,
so consumers `.signal_cloned()` it and react immediately (no polling).

#### MVVM Pattern: ReactiveViewModel Tree

The render pipeline follows Model-View-ViewModel (MVVM). The three layers are:

| Layer | Holon Component | Responsibility |
|-------|-----------------|----------------|
| **Model** | Loro (authority for blocks) + Turso (projection / matview / CDC) | Domain data, persistence, CDC streams; cells (`Cell<T>`) are the in-memory reactive read primitive |
| **ViewModel** | `ReactiveViewModel` tree (`holon-frontend`) | Platform-agnostic reactive presentation tree — persistent nodes that self-update via futures-signals; per-instance widget state lives here |
| **View** | GPUI elements, Flutter widgets, Dioxus components, TUI cells | Platform-specific UI — mechanical mapping from `ReactiveViewModel` to native widgets |

`ReactiveViewModel` (`crates/holon-frontend/src/reactive_view_model.rs`) is the boundary between shared render logic and platform-specific frontends. It holds **per-instance widget state** as `Mutable<T>` fields (the FU-1 pattern); **entity field state** is resolved through cells by `(uri, field)` and is not stored on the VM:

```rust
pub struct ReactiveViewModel {
    pub expr: Mutable<RenderExpr>,       // Render expression this node was built from
    pub data: Mutable<Arc<DataRow>>,     // The data row this node is interpreting (sourced from cells)
    pub children: Vec<Arc<ReactiveViewModel>>,  // Static layout children
    pub collection: Option<Arc<ReactiveView>>,  // Reactive collection (MutableVec)
    pub slot: Option<ReactiveSlot>,      // Deferred content (live_block, live_query)
    pub expanded: Option<Mutable<bool>>, // Per-instance expand/collapse state (NOT a cell — see UI.md FU-1)
    pub operations: Vec<OperationWiring>,
    pub triggers: Vec<InputTrigger>,
    pub layout_hint: LayoutHint,
}
```

See [UI](UI.md) for the cells-vs-`Mutable<T>` cut.

`ReactiveView` (`crates/holon-frontend/src/reactive_view.rs`) is a self-managing reactive collection that owns its data pipeline. The driver is spawned internally and stopped on Drop.

**Data flow:**

```
Vec<DataRow> + CDC stream (from watch_ui)
        │
        ▼
  ReactiveEngine (holon-frontend)
  interprets RenderExpr → ReactiveViewModel tree
  with ReactiveView collections (MutableVec)
        │
        ▼
  Frontend subscribes to Mutable/MutableVec signals
        │
        ▼
  Frontend-specific View (GPUI / Flutter / TUI)
```

Each frontend subscribes to `Mutable` and `MutableVec` signals on `ReactiveViewModel` nodes and re-renders only what changed. The frontend contains no layout or business logic.

#### Three-Tier Event Model (View → ViewModel Input)

The ReactiveViewModel also declares what input events it cares about via `InputTrigger`s. This keeps shared interaction logic (command menu, hotkeys, mode transitions) in the ViewModel layer without routing every keystroke through Rust.

**Tier 1 — Native (no round-trip):** Text input, cursor movement, selection, IME composition, scrolling. Handled entirely by the platform's text input stack. Fighting platform text editing causes IME bugs, latency, and accessibility issues — so we don't.

**Tier 2 — Trigger (local check, round-trip on match):** The ViewModel declares triggers on nodes. The View checks incoming input against triggers locally — O(number of triggers on that node), typically 1–3. Only when a trigger matches does the View send a semantic event to the ViewModel layer, which processes it and updates the reactive tree.

**Tier 3 — Sync (debounced, async):** Text content syncs to the backend on blur or after a debounce interval.

```rust
pub enum InputTrigger {
    PrefixAtCursor { prefix: String, cursor_pos: usize, action: String },
    KeyChord { chord: String, action: String },
    TextChanged { debounce_ms: u32, action: String },
}
```

**Example: `/` command menu flow:**

1. User types `/` at position 0 in an `EditableText` node
2. View checks triggers locally — `PrefixAtCursor{"/", 0, "command_menu"}` matches
3. View sends `ViewEvent { node_id, action: "command_menu", context: { text: "/", cursor: 1 } }`
4. ReactiveEngine produces a CommandMenu subtree and updates the reactive slot
5. View re-renders from the updated `Mutable` — no round-trip for subsequent keystrokes
6. On selection, ReactiveEngine replaces `/` with the command result

**Performance characteristics:**

| Event type | Frequency | Backend round-trip | Cost |
|---|---|---|---|
| Normal keystroke | ~5/sec | No | 0 |
| Trigger check | ~5/sec | No (local match) | ~100ns |
| Trigger fire | ~1/min | Yes | ~1ms |
| Text sync | ~3/sec (debounced) | Yes (async, non-blocking) | ~1ms |

**What stays in the View:** cursor position, text selection, IME composition, scroll position, focus rings, animations.

**What the ViewModel owns:** mode transitions, semantic actions (submit, delete, toggle), and any state that produces new UI (command menu items, autocomplete suggestions).

#### Key Files

| Path | Description |
|------|-------------|
| `crates/holon-frontend/src/reactive_view_model.rs` | `ReactiveViewModel`, `ReactiveSlot` — persistent reactive ViewModel nodes |
| `crates/holon-frontend/src/reactive_view.rs` | `ReactiveView` — self-managing reactive collection (MutableVec + driver) |
| `crates/holon-frontend/src/reactive.rs` | `ReactiveEngine`, shadow builders, render interpretation |
| `crates/holon-api/src/entity_profile.rs` | `EntityProfile`, `ProfileCache`, `StoredProfile`/`StoredVariant`, `ProfileResolving` trait |
| `crates/holon/src/entity_profile.rs` | `ProfileResolver` (LiveData-backed), profile parsing |
| `crates/holon-api/src/widget_spec.rs` | `DataRow` type alias, `DataRowAccumulator` |
| `crates/holon-api/src/render_types.rs` | `RenderExpr`, `OperationDescriptor`, `OperationWiring` |
| `crates/holon/src/api/backend_engine.rs` | `get_root_block_id()`, `render_entity()`, `attach_row_profiles()` |
| `crates/holon/src/api/ui_watcher.rs` | `watch_ui()` — `Stream<UiEvent>` with `merge_triggers` + `switch_map` |

