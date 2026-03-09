# Query & Render Pipeline

*Part of [Architecture](../Architecture.md)*



## Query Compilation by Language

```
PRQL string ──→ prqlc compile → SQL (pure data query, no render directives)
GQL string  ──→ gql_parser::parse → AST → gql_transform::transform_default → SQL
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

GQL also operates on ordinary tables with foreign key relations — not only the EAV schema. The schema is initialized idempotently (all `IF NOT EXISTS`) during database startup.

### EntityProfile System (Render Architecture)

Render specifications are resolved **at runtime per-row** via the EntityProfile system.

**Location**: `crates/holon/src/entity_profile.rs`

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

**Profile Resolution** (BackendEngine / entity_profile.rs):
```
For each row:
  - Look up EntityProfile by row's entity scheme in the `id` column
  - Evaluate Rhai variant conditions against row data
  - Attach matching RowProfile (render expr + operations)
```

**CDC stream forwarding** (`ui_watcher.rs`):
`watch_ui(block_id)` returns a `WatchHandle` carrying a `Stream<UiEvent>`. `merge_triggers` merges three event sources — structural CDC, `SetVariant` commands, and profile version changes — into a single `RenderTrigger` stream. This drives a `switch_map` that aborts the previous data forwarder and spawns a new one on each trigger. Each CDC Created/Updated event is enriched with profile-resolved computed fields before forwarding.

**Reactive layer** (`holon-frontend`):
Query results flow into `ReactiveView` (a self-managing reactive collection backed by futures-signals `MutableVec`). Each row is wrapped in a `ReactiveViewModel` — a persistent node that owns its `RenderExpr` and `DataRow` as `Mutable<_>` fields. When either changes the node re-interprets itself and pushes updates to child nodes without rebuilding the tree. `DataRowAccumulator` (`holon-api/src/widget_spec.rs`) is the single source of truth for `Change<DataRow>` → keyed collection conversion.

#### Core Types

```rust
// Location: crates/holon/src/entity_profile.rs
pub struct EntityProfile {
    pub entity_name: String,               // "blocks", "todoist_tasks"
    pub default: Arc<RowProfile>,          // Default rendering
    pub variants: Vec<RowVariant>,         // Conditional overrides (Rhai)
    pub computed_fields: Vec<ComputedField>,
}

pub struct RowVariant {
    pub name: String,
    pub condition_source: String,          // Rhai expression, e.g. "task_state == \"DONE\""
    pub profile: Arc<RowProfile>,
    pub specificity: usize,                // Higher = tried first
}

pub struct RowProfile {
    pub name: String,
    pub render: RenderExpr,                // e.g. tree(...), list(...), row(...)
    pub operations: Vec<OperationDescriptor>,
}
```

**Resolution algorithm** (`EntityProfile::resolve`):
1. If `ProfileContext.preferred_variant` is set, try that variant first
2. Evaluate variants in specificity order (descending)
3. First variant whose Rhai condition evaluates to `true` wins
4. Fall back to `default` profile if no variant matches
5. If no EntityProfile exists for this entity_name, return "fallback" (no profile attached)

#### ProfileResolving Trait

```rust
// Location: crates/holon/src/entity_profile.rs
pub trait ProfileResolving: Send + Sync {
    fn resolve(&self, row: &HashMap<String, Value>, context: &ProfileContext) -> Arc<RowProfile>;
    fn resolve_with_computed(&self, row, context) -> (Arc<RowProfile>, HashMap<String, Value>);
    fn resolve_batch(&self, rows: &[HashMap<String, Value>], context: &ProfileContext) -> Vec<Arc<RowProfile>>;
    fn subscribe_version(&self) -> watch::Receiver<u64>;  // push-based change notification
}

pub struct ProfileContext {
    pub preferred_variant: Option<String>,  // Hint from caller
    pub view_width: Option<f64>,            // Responsive breakpoints (future)
}
```

`ProfileResolver` loads profiles from org blocks with `entity_profile_for` property. Profiles are backed by CDC-driven `LiveData<EntityProfile>` — edits to profile blocks take effect immediately via `tokio::sync::watch` push notification (no polling).

#### MVVM Pattern: ReactiveViewModel Tree

The render pipeline follows Model-View-ViewModel (MVVM). The three layers are:

| Layer | Holon Component | Responsibility |
|-------|-----------------|----------------|
| **Model** | Turso/Loro (blocks, documents, queries) | Domain data, persistence, CDC streams |
| **ViewModel** | `ReactiveViewModel` tree (`holon-frontend`) | Platform-agnostic reactive presentation tree — persistent nodes that self-update via futures-signals |
| **View** | GPUI elements, Flutter widgets, Dioxus components, TUI cells | Platform-specific UI — mechanical mapping from `ReactiveViewModel` to native widgets |

`ReactiveViewModel` (`crates/holon-frontend/src/reactive_view_model.rs`) is the boundary between shared render logic and platform-specific frontends:

```rust
pub struct ReactiveViewModel {
    pub expr: Mutable<RenderExpr>,       // Render expression this node was built from
    pub data: Mutable<Arc<DataRow>>,     // The data row this node is interpreting
    pub children: Vec<Arc<ReactiveViewModel>>,  // Static layout children
    pub collection: Option<Arc<ReactiveView>>,  // Reactive collection (MutableVec)
    pub slot: Option<ReactiveSlot>,      // Deferred content (live_block, live_query)
    pub expanded: Option<Mutable<bool>>, // Expand/collapse state
    pub operations: Vec<OperationWiring>,
    pub triggers: Vec<InputTrigger>,
    pub layout_hint: LayoutHint,
}
```

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
| `crates/holon/src/entity_profile.rs` | `EntityProfile`, `RowProfile`, `RowVariant`, `ProfileResolver` |
| `crates/holon-api/src/widget_spec.rs` | `DataRow` type alias, `DataRowAccumulator` |
| `crates/holon-api/src/render_types.rs` | `RenderExpr`, `OperationDescriptor`, `OperationWiring` |
| `crates/holon/src/api/backend_engine.rs` | `get_root_block_id()`, `render_entity()`, `attach_row_profiles()` |
| `crates/holon/src/api/ui_watcher.rs` | `watch_ui()` — `Stream<UiEvent>` with `merge_triggers` + `switch_map` |

