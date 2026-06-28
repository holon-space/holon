# Architectural Principles

This document describes the foundational architectural decisions that guide the design of Holon. These principles are stable and should not require frequent updates as the implementation evolves.

For detailed technical documentation, see `Architecture.md`.

## Foundational Goal: Trust Enables Flow

Every architectural decision ultimately serves one purpose: enabling users to achieve **flow states** through **trust**.

- Trust that nothing is forgotten
- Trust that the right thing is being worked on
- Trust that relevant context is accessible

This shapes our priorities: reliability over features, transparency over magic, user control over automation.

---

## Core Principle: External Systems as First-Class Citizens

Unlike traditional PKM tools that treat external systems as import/export targets, Holon treats them as primary data sources with full operational capability.

**Implications:**

1. **Lossless Storage**: Data from external systems is stored in a format as close to the source as possible. Deviations must be bijective (reversible), such as column renaming. This ensures:
   - All operations available in the external system can be performed locally
   - All data from the external system can be displayed without loss
   - Round-trip fidelity when syncing back

2. **Operations, Not Just Data**: We expose every useful operation that the external system's API provides, not just CRUD. Users can mark tasks complete, change priorities, move items between projects—all without leaving the app.

3. **Unified View, Diverse Sources**: Items from different systems can appear in the same query result and view. A project page can show Todoist tasks alongside JIRA issues alongside internal notes, each with its native capabilities intact.

---

## The Three Modes

Capture, Orient, and Flow ship as **default layout and profile configuration** (overridable by users), which is why no mode enum exists in the production codebase. The modes are a design vocabulary and a default set of layouts/profiles — not a built-in state machine.

The UI architecture is organized around three modes that match how humans actually work:

### Capture Mode
**Purpose**: Quick input, get it out of my head

**Architectural Requirements**:
- Sub-100ms input latency for block creation
- Works offline with instant local commit
- Keyboard-driven quick add (global hotkey, command palette)
- Capture to current context (project/task) or to inbox
- Inbox that processes to zero

**Out of Scope**: Mobile-optimized capture, voice notes, email forwarding. These are better handled by integrated tools (Todoist, etc.). Holon's strength is integration, not competing on every feature.

### Orient Mode
**Purpose**: Big picture, daily/weekly reviews
**Architectural Requirements**:
- Watcher Dashboard with cross-system synthesis
- Efficient aggregation queries across all data sources
- CDC-driven real-time updates
- Risk/deadline/dependency analysis
- "Nothing forgotten" completeness guarantees

### Flow Mode
**Purpose**: Deep focus on present task

**Architectural Requirements**:
- Context Bundle assembly (related items across systems)
- Selective loading (only relevant context)
- Distraction hiding (non-relevant items filtered)
- Single-task view with all needed context
- Minimal UI chrome

---

## Context Bundles

When a user focuses on a project or task, the system assembles a **Context Bundle**:

```
Context Bundle for "Project X"
├── Native Holon blocks about X
├── Todoist tasks in project X
├── JIRA issues linked to X
├── Calendar events tagged X
├── Gmail threads about X
└── Related items (via embeddings)
```

**Architectural Principles**:
1. Context Bundles are computed, not stored (derived from links + queries)
2. Links are explicit (user-created) or inferred (AI-generated with confidence scores)
3. Bundle assembly must be fast (<200ms for typical project)
4. Bundles update reactively as underlying data changes

---

## Data Flow Architecture

### Reactive Sync Pattern

Operations flow one-way without blocking the UI for responses:

```
User Action → Operation Dispatch → External/Internal System
                                          ↓
UI ← CDC Stream ← QueryableCache ← Sync Provider
```

**Key aspects:**
- Operations are "fire and forget"—the UI doesn't await a response
- Effects are observed through sync with the external system
- Changes propagate through the QueryableCache as a stream
- Internal and external modifications are treated identically

### Change Data Capture (CDC)

Changes propagate from storage to UI via CDC streams:

```
Database Write → Turso CDC → BatchWithMetadata<RowChange> → UI Stream
```

This architecture enables:
- Real-time UI updates without polling
- Distributed tracing through the entire pipeline
- Consistent handling of local and remote changes

### Trace Context Propagation

Every operation carries trace context through the entire system:

```
Frontend (trace_id) → Operation → Database (_change_origin column)
                                        ↓
                     CDC Callback reads trace context
                                        ↓
                     Change event includes origin trace
```

This enables debugging, audit trails, and understanding causation across async boundaries.

---

## AI Services Architecture

> **Status: target architecture — not yet implemented.**

The Watcher, Integrator, and Guide are not hard-coded Rust services. They are realized as **declarative Petri-Net configuration**: AI agents are tokens placed on the net, and transitions like "Research" consume them. The Watcher/Integrator/Guide roles become the *default configuration* of tokens and transitions shipped with Holon — user-overridable, not wired in code. See [Vision/PetriNet.md](../Vision/PetriNet.md) §AI for how trust levels attach to transitions.

The three roles and their responsibilities remain the same conceptually:

- **Watcher** — monitoring, alerts, synthesis, conflict detection
- **Integrator** — cross-system linking, context assembly, confirmation stream, search
- **Guide** — patterns, insights, growth tracking, shadow self

### AI Architectural Principles

1. **Async & Non-Blocking**: AI operations never block the UI thread
2. **Local-First**: Embeddings, search, and classification run on-device
3. **Progressive Trust**: AI earns autonomy through demonstrated accuracy
4. **Transparent Reasoning**: Every AI suggestion includes explanation
5. **Easy Override**: User can always undo or correct AI decisions
6. **Learning Loop**: Corrections feed back into training data

### Trust Ladder

AI autonomy is gated by demonstrated competence:

| Level | Behavior | Gate |
|-------|----------|------|
| Passive | Answers when asked | Default |
| Advisory | Suggests, user decides | >80% acceptance |
| Agentic | Acts with permission | Low correction rate |
| Autonomous | Acts within bounds | Extended track record + opt-in |

Trust is tracked **per-feature**, not globally. In the Petri-Net model, trust levels attach to individual transitions rather than to a global AI state.

---

## Privacy & Deployment Architecture

Three deployment models with different privacy/capability tradeoffs:

### Option 1: Fully Local (Maximum Privacy)

```
┌─────────────────────────────────────────┐
│              User Device                │
│  ┌─────────────────────────────────┐   │
│  │  Holon App                      │   │
│  │  • All data local               │   │
│  │  • GGUF models (llama.cpp)      │   │
│  │  • Zero cloud dependency        │   │
│  └─────────────────────────────────┘   │
└─────────────────────────────────────────┘
```

### Option 2: Hybrid (Recommended)

```
┌─────────────────────────────────────────┐
│              User Device                │
│  ┌─────────────────────────────────┐   │
│  │  Holon App                      │   │
│  │  • All data local               │   │
│  │  • Embeddings local             │   │
│  │  • Classification local         │   │
│  └──────────────┬──────────────────┘   │
└─────────────────┼───────────────────────┘
                  │ Opt-in, minimal context
                  ▼
┌─────────────────────────────────────────┐
│           Cloud LLM (GPT-4/Claude)      │
│  • Complex reasoning only               │
│  • User controls what is sent           │
└─────────────────────────────────────────┘
```

### Option 3: Self-Hosted

```
┌─────────────────────────────────────────┐
│              User Device                │
│  ┌─────────────────────────────────┐   │
│  │  Holon App                      │   │
│  └──────────────┬──────────────────┘   │
└─────────────────┼───────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────┐
│        User's LLM Server                │
│  (Ollama, vLLM, etc.)                   │
│  • Full control                         │
│  • Good model quality                   │
└─────────────────────────────────────────┘
```

### Privacy Architectural Principles

1. **Data Never Leaves Without Consent**: All data stays local by default
2. **Minimal Context**: When cloud AI is used, send minimum necessary context
3. **User Visibility**: Clear indication of what goes where
4. **Graceful Degradation**: App fully functional without cloud AI

---

## Query and Render Architecture

### Declarative Queries with PRQL

Users specify what data they want using PRQL, including how it should be rendered:

```prql
from todoist_tasks
filter completed == false
select {id, content, priority, completed}
render (list item_template:(row
  (checkbox checked:this.completed)
  (text content:this.content)))
```

The `render` clause is a declarative UI specification that gets compiled alongside the SQL.

### Automatic Operation Discovery

The system automatically determines which operations are available for each rendered item:

1. **Lineage Analysis**: Traces which database columns flow into which UI widgets
2. **Operation Matching**: Compares widget parameters against operation `required_params`
3. **UI Annotation**: Attaches available operations to the rendered tree

A checkbox bound to `completed` automatically gets `set_completion` wired up because:
- The widget type is "checkbox"
- Its `checked` parameter traces to the `completed` column
- An operation exists that modifies `completed` with the available parameters

For multi-widget operations (e.g., `move_block` requiring `parent_id` from a drop target), **Gesture-Scoped Parameter Providers** extend this system. Widgets declare what params they provide, gestures accumulate params into a context, and operations declare mappings from alternative sources (e.g., `selected_id` → `parent_id`). See `docs/Proposals/GESTURE_PARAM_PROVIDERS.md` for details.

### RenderSpec Tree

Query compilation produces a `RenderSpec`—a data structure describing what to render:

```
RenderSpec
├── RenderExpr::FunctionCall("list", ...)
│   └── RenderExpr::FunctionCall("row", ...)
│       ├── RenderExpr::FunctionCall("checkbox", checked: ColumnRef("completed"))
│       │   └── operations: [OperationWiring { set_completion... }]
│       └── RenderExpr::FunctionCall("text", content: ColumnRef("content"))
```

The frontend interprets this tree to create native UI components while preserving operation bindings.

---

## Storage Architecture

### Authority + Projection (Layered Model)

Holon's data flow is layered. Each layer has one job:

```
┌─────────────────────────────────────────────────────────────────┐
│  Layer 4 — Chord ops + UI                                       │
│    (multi-step ops, frontend interactions; reads cells, writes  │
│     via OperationProvider OR cells)                             │
└──────────────┬──────────────────────────────────┬───────────────┘
               │ read                             │ write
               ▼                                  ▼
┌─────────────────────────────────────────────────────────────────┐
│  Layer 2 — Cells (reactive read primitive, in-memory)           │
│    Cell<T> per (EntityUri, FieldPath); current() / signal() /   │
│    set(); rich-op handles for text (Cell<String>)               │
└──────────────┬──────────────────────────────────┬───────────────┘
               │ project from                     │ dispatch to
               ▼                                  ▼
┌──────────────────────────────┐   ┌─────────────────────────────┐
│  Layer 3 — Matviews / IVM    │   │  Authority (per entity type)│
│    (Turso, derived,          │   │    Loro for blocks          │
│     incrementally maintained)│   │    Todoist API for tasks    │
│                              │   │    JIRA API for issues      │
│    Query-shaped, durable     │   │    org files for docs       │
└──────────────────────────────┘   └─────────────────────────────┘
                  ▲                                │
                  │                                │
                  │  emits CDC / projection events │
                  └────────────────────────────────┘
                  │
┌─────────────────┴───────────────────────────────────────────────┐
│  Layer 1 — Change log (Loro oplog + Turso CDC + WAL)             │
│    Durable, ordered, identity-bearing record of every change.   │
└─────────────────────────────────────────────────────────────────┘
```

**Authority + Projection rule**: each entity type has exactly one *authority* (the system that can refuse a write). Turso is uniformly downstream — it never holds state the authority hasn't accepted. The UI never queries an authority directly; it always reads cells (Layer 2), which project from the authority through the event log (Layer 1) and matviews (Layer 3).

### Unified Query Cache (Turso, projection only)

```
┌─────────────────────────────────────────────────────────────────┐
│                    UNIFIED TURSO CACHE (projection)             │
│            (SQLite-compatible, single query surface)            │
│                                                                 │
│    PRQL/SQL queries run here against ALL data uniformly        │
│    Writes flow Authority → Event Log → Turso projection        │
└───────────────────┬─────────────────────────┬───────────────────┘
                    ▲                         ▲
            projects from               projects from
                    │                         │
            ┌───────┴───────┐         ┌───────┴───────────┐
            │  LORO CRDT    │         │  THIRD-PARTY      │
            │  (authority   │         │  APIs             │
            │   for blocks) │         │  (authorities for │
            │               │         │   their entities) │
            └───────────────┘         └───────────────────┘
```

**Key insight**: The UI never queries Loro or external APIs directly. Everything goes through cells (Layer 2) backed by the unified Turso cache (Layer 3). This enables:
- Single query language (PRQL/SQL) for all data
- Consistent CDC stream for all changes
- Uniform reactive read primitive (`Cell<T>`) regardless of authority

### Authority by Data Type (default wiring)

Per [ADR 0004](../adr/0004-domain-adapter-actor-split.md) the block **domain** (the
knowledge graph itself) is the only *canonical* projection; Loro / Org / Markdown / Turso
are peer serialization adapters. The table below names which adapter is **authoritative**
for each data type under the **default DI wiring** — authority is a wiring choice, not a
permanent property of any adapter.

| Data Type | Authority (default wiring) | Turso Role |
|-----------|-----------|------------|
| Owned blocks (content, structure, metadata) | Loro CRDT | Projection only — never written directly |
| External system data (Todoist, JIRA, etc.) | External API | Projection — populated by polling / webhooks |
| User metadata on external items | Loro CRDT | Projection |
| AI embeddings | Generated on-device | Projection (computed → Turso) |
| Pattern/conflict logs | Local only | Stored locally (no upstream authority) |

**Rationale**: CRDTs excel at collaborative editing of owned data. External systems are server-authoritative—we cache their data and queue operations, but don't pretend to own it. Either way, Turso is downstream of the authoritative system; the UI reads through cells, which read through Turso projections.

### Reactive Read: Cell Registry

Cells (`Cell<T>`) are the system's universal reactive read primitive. Each cell is identified by `(EntityUri, FieldPath)` and exposes:
- `current() -> T` — synchronous read of the latest value
- `signal() -> impl Signal<Item = T>` — reactive stream of updates
- `set(T)` — write that dispatches via the entity's typed `CrudOperations` methods (NOT through `OperationDispatcher` — that would re-enter dispatch and double-log undo/trace)

`MutableText` is `Cell<String>` with rich-op methods (`apply_text_op(TextOp)`, cursor anchors, `remote_deltas()`). Loro-backed text cells supply rich behaviour natively; LWW backings degrade gracefully to compute-then-replace.

Per-entity-type cell registries hold the cells. `BlockCellRegistry` knows how to construct each block field's backing (Loro-meta-backed for `completed`/`collapsed`/etc., Loro-text-backed for `content`, LWW-scalar-backed in SqlOnly mode). Cells are `Weak`-keyed: held alive while consumers reference them; reaped when the last consumer drops.

**Cells vs `Mutable<T>`**: `Cell<T>` is for entity field state (has identity, has authority, could be persisted/queried/synced). Per-VM `Mutable<T>` (FU-1 pattern) is for per-instance widget state (tree-item `expanded`, view-mode-switcher selection, focused_block) — same-id entities in different render slots need independent state. Genuinely-ephemeral state (cursor blink, hover, drag offset) stays raw `Mutable<T>` too.

### Plain-Text File Layer

Local files (Markdown or Org Mode) provide an additional interface to owned data:

```
┌─────────────────────────────────────────┐
│           External Editors              │
│     (Vim, Emacs, VS Code, etc.)         │
└────────────────┬────────────────────────┘
                 │ reads/writes
                 │
┌────────────────▼────────────────────────┐
│         Plain-Text Files                │
│    (Markdown/Org Mode on disk)          │
└────────────────┬────────────────────────┘
                 │ bidirectional sync
                 │
┌────────────────▼────────────────────────┐
│            Loro CRDT                    │
│     (default authority for owned)       │
└─────────────────────────────────────────┘
```

**Capabilities**:
- Files act as a bidirectional cache of CRDT content
- External edits to files are detected and merged into CRDTs
- Enables interop with other tools
- Provides human-readable backup and portability

**Open questions**: Exact reconciliation strategy between file edits and CRDT state is TBD. Goal: you can always edit your notes in any text editor, and Holon will incorporate those changes.

### Sync Token Management

Sync tokens are persisted atomically with data in a single transaction:

```
BEGIN TRANSACTION
  -- Apply all data changes
  INSERT/UPDATE/DELETE ...
  -- Save sync token
  INSERT INTO sync_states (provider_name, sync_token) VALUES (...)
COMMIT
```

This prevents inconsistency between cached data and sync position.

---

## Operation System Architecture

Two complementary write surfaces, with a clear cut between them:

| Path | Use case | Reflective? |
|------|----------|-------------|
| **Cells** (`cell.set(v)` / `cell.apply_text_op(op)`) | Single-field write where the caller already has the value (keychord toggles, edit-commit, optimistic UI). | No — typed call into `CrudOperations`. Lives at Layer 2. |
| **OperationProvider / OperationDispatcher** | Multi-param ops, runtime-discovered ops, schema-introspectable ops (`move_block`, `split_block`, `embed_entity`, third-party ops, MCP, GraphQL). | Yes — `find_operations` discovers shape, params gathered from drag&drop / search / clipboard / etc. |

Both paths share the same underlying typed methods on the entity's `CrudOperations` trait — cells are sugar at the chord-op layer, not a parallel write path. They emit the same events with the same origin/trace tagging.

### Trait-Based Operations

Operations are defined via traits, not string-based dispatch:

```rust
trait MutableTaskDataSource<T> {
    async fn set_completion(&self, id: &str, completed: bool);
    async fn set_priority(&self, id: &str, priority: i64);
}
```

Procedural macros generate `OperationDescriptor` metadata from these traits.

### Operation Descriptors

Each operation is described with metadata for UI generation:

```rust
OperationDescriptor {
    entity_name: "todoist-task",
    name: "set_completion",
    required_params: ["id", "completed"],
    affected_fields: ["completed"],
    precondition: Some(PreconditionChecker { ... }),
}
```

The UI uses this to:
- Show only applicable operations (based on available params)
- Wire operation callbacks to widgets
- Validate before dispatch
- Discover operations from runtime-loaded third-party sources (the macro-reified shape is the only thing the UI needs to know about an op)

### Composite Operation Dispatch

`OperationDispatcher` aggregates multiple `OperationProvider` implementations:

```
OperationDispatcher
├── QueryableCache<TodoistDataSource, TodoistTask>
├── QueryableCache<JiraDataSource, JiraIssue>
└── LoroBlockOperations (authority for blocks)
```

Operations are routed by `entity_name` to the appropriate provider. `LoroBlockOperations` (post Phase 2 of the Cells plan) is the authority for the `block` entity — block writes go through Loro first, Turso projects via `LoroSyncController` + `BlockConsolidator`.

### Cells bypass the dispatcher (typed-method path)

Cells dispatch through *typed* `CrudOperations` methods (`block_ops.set_field(id, field, v)`), NOT through `OperationDispatcher::execute_operation`. Inside a chord op, the chord op is already the dispatched operation; nesting another dispatch would double-log to `OperationLog`, duplicate the trace span, and fork the undo stack. The typed-method route gives cells the same `event_bus.emit` / origin tagging the dispatcher uses, without re-entering dispatch.

---

## UI Architecture

### Frontend Agnosticism

The backend exposes a minimal FFI surface that any frontend can implement:

```rust
// Core FFI functions
fn init_render_engine() -> RenderEngine;
fn compile_query(prql: &str) -> CompiledQuery;
fn execute_operation(entity: &str, op: &str, params: StorageEntity);
fn watch_changes() -> Stream<Change<StorageEntity>>;
```

This enables:
- GPUI frontend (current primary — desktop; mobile via gpui-mobile)
- TUI frontend (keyboard-driven + test harness)
- Dioxus / dioxus-web frontend (prototype)
- MCP server frontend

### Reactive Updates

Frontends subscribe to change streams and update reactively:

```rust
// GPUI signals (holon-frontend ReactiveViewModel)
let mut stream = watch_changes(block_id.clone());
while let Some(change) = stream.next().await {
    ui_signal.set(change.data);
}
```

No explicit refresh calls—UI state derives from the change stream.

---

## Trust & Flow Observability

The system exposes observable properties that support trust and flow:

### Sync Status (Trust)

Every external item has visible sync status:
- ✓ Synced (matches external system)
- ⏳ Pending (local changes queued)
- ⚠️ Conflict (requires resolution)
- ❌ Error (sync failed)

### Completeness Indicators (Trust)

Orient mode shows system completeness:
- All systems connected and synced
- No unprocessed inbox items
- All reviews completed
- No stuck/overdue items (or explicit count)

### Focus Metrics (Flow)

Flow mode tracks:
- Time in current focus session
- Context switches (should be zero)
- Interruption count

---

## Dependency Injection

The system uses **fluxdi** (`Injector`, `Module`, `Provider`, `Shared`) for service registration and resolution. Modules implement the `Module` trait and register providers; first-wins semantics apply (the first registration wins; `override_provider` is available for test overrides). The composition root is `crates/holon-app/src/wiring.rs` (`FrontendInjectorExt::add_frontend`), which conditionally registers Loro and OrgMode modules based on runtime config. The Turso-free stack is assembled in `crates/holon-app/src/no_turso.rs`.

```rust
// Registration via Module trait (fluxdi)
impl Module for OrgModeModule {
    fn configure(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        injector.provide::<FileSyncController>(Provider::root_async(|resolver| async move {
            let block_reader = resolver.resolve::<dyn BlockReader>();
            let doc_manager = resolver.resolve::<dyn DocumentManager>();
            Shared::new(FileSyncController::new(block_reader, doc_manager))
        }));
        Ok(())
    }
}

// Extension traits wired in holon-app
injector.add_orgmode(PathBuf::from("/path/to/org/files"))?;
injector.add_mcp_server(8520)?;

// Resolution
let session = injector.resolve_async::<FrontendSession>().await;
```

Conditional registration: when `HOLON_CRDT_ENABLED` is set, `LoroModule` registers `LoroBlockOperations` as the authority for blocks and `LoroTextCellBacking` for `content` cells. When unset, `SqlOperationProvider` + LWW cell backings are registered instead. The same `FrontendSession` resolution path works for both — the DI container is the switch, not runtime branches.

This enables:
- Testability (mock providers, in-memory filesystem)
- Modularity (add providers without changing core code)
- Configuration (different providers for different environments — Loro on/off, Turso on/off)
- Automatic dependency resolution (services declare what they need, DI wires it)

### Async DI Pattern (Spec 0007 Phase 3.5)

The converged pattern (June 2026):

1. **Async boundary at the top**: `open_and_register_core` acquires the `DbHandle`
   via async Turso initialization. Callers obtain handles before entering DI
   registration.
2. **Sync factories capture clones**: `Provider::root` factories receive owned
   clones of `DbHandle`, `Arc<...>`, etc. — they never call `block_in_place`.
3. **Async work uses `Provider::root_async`**: 44 factories use the async path
   directly. fluxdi rejects sync resolution of an async provider loudly
   (fail-loud, no silent deadlock).
4. **The mcp_vtable `block_in_place`** (`holon-mcp-client/src/mcp_vtable.rs`)
   is a deliberate Turso-FFI boundary on a dedicated runtime — it is NOT the
   DI-deadlock class.
5. **Test-side `block_in_place`** (~50 sites) bridges `multi_thread` tokio
   (proptest) into async — untouched; this is test infrastructure, not DI.

Deleted (June 2026): `register_core_services` (only production `block_in_place`),
`run_async_in_sync`, and sync `create_queryable_cache` — all were dead code with
zero callers. The 27 remaining pure-sync `Provider::root` factories capture
clones only (no blocking) and are harmless to leave.

---

## Extension Points

### Adding a New External System

1. Implement `DataSource<T>` for read-only cache access
2. Implement `CrudOperationProvider<T>` for write operations
3. Implement domain-specific traits (e.g., `MutableTaskDataSource`)
4. Create a `SyncProvider` for incremental sync
5. Register in DI container via a module

### Adding a New Operation

1. Add method to appropriate trait (or create new trait)
2. Annotate with `#[affects("field1", "field2")]`
3. Implement in relevant providers
4. Macros auto-generate operation descriptor

### Adding a New UI Widget Type

1. Add function stub in lineage preprocessor (if using auto-wiring)
2. Implement widget in each frontend
3. Widget receives `RenderExpr` with operation bindings

### Adding a New AI Capability

1. Model it as a Petri-Net transition (or a configuration of transitions) — AI agents are tokens consumed by transitions
2. Define the trust level for the transition (see Trust Ladder above)
3. Add it to the default configuration (user-overridable)
4. Ensure the transition's outputs are observable (fail loud if the agent cannot complete)
5. Include reasoning/explanation in the output token

---

## Consistency Guarantees

### Local Consistency

Within a single client:
- Database transactions ensure atomic updates
- CDC delivers changes in commit order
- UI reflects committed state

### External Consistency

With external systems:
- Eventually consistent (5-30 second typical delay)
- Last-write-wins for concurrent edits
- Sync tokens prevent duplicate processing
- AI-assisted conflict detection and resolution

### P2P Consistency (Future)

Between devices:
- Loro CRDTs ensure convergence
- No central server required
- Works offline, syncs when connected

---

## Related Documents

- [docs/Vision/LongTerm.md](../Vision/LongTerm.md) — Philosophical foundation and product vision
- [docs/Vision.md](../Vision.md) — Technical vision and roadmap
- [docs/Vision/AI.md](../Vision/AI.md) — AI feature specifications
- [docs/Vision/PetriNet.md](../Vision/PetriNet.md) — Petri-Net model and AI trust configuration
- [docs/Architecture.md](../Architecture.md) — Detailed technical architecture
- [docs/Architecture/RenderPipeline.md](RenderPipeline.md) — Query/render pipeline details
