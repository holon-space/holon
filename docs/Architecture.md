# Architecture: Holon

## Overview

Holon is a Personal Knowledge & Task Management system that treats external systems (Todoist, org-mode, etc.) as first-class data sources. Unlike traditional PKM tools that import/export data, Holon maintains live bidirectional sync with external systems while enabling unified queries across all sources.

## Core Principles

### External Systems as First-Class Citizens

Data from external systems is stored in a format as close to the source as possible:
- All operations available in the external system can be performed locally
- All data can be displayed without loss
- Round-trip fidelity when syncing back

### Reactive Data Flow

Operations flow without blocking the UI:

```
User Action → Operation Dispatch → External/Internal System
                                                          ↓
UI <- futures-signals ← CDC Stream ← QueryableCache ← Sync Provider
```

- Operations are fire-and-forget
- Effects are observed through sync
- Changes propagate as streams
- Internal and external modifications are treated identically

#### Streaming-first render state

`ReactiveQueryResults` stores a non-optional `Mutable<RenderExpr>` initialized to `loading()` — a regular `FunctionCall { name: "loading" }`. When the first `Structure` event arrives from `watch_ui`, the real render expression replaces it. Consumers (GPUI signals, MCP snapshots) never see `Option<RenderExpr>` — `loading()` flows through the same interpret → build → render pipeline as any other widget. The `loading` builder (in `shadow_builders/loading.rs`) produces an `Empty` reactive view model, so frontends render nothing until real data arrives.

### Multi-Language Query Support

Users specify data needs using PRQL, GQL (ISO/IEC 39075 graph queries), or raw SQL. Rendering is specified in a sibling render block using Rhai syntax.

**PRQL** + **render sibling**:
```org
#+BEGIN_SRC holon_prql
from children
select {id, content, content_type, source_language}
#+END_SRC
#+BEGIN_SRC render
list(#{item_template: render_entity()})
#+END_SRC
```

**GQL** (graph queries, compiled to SQL using tables and FK relations):
```
MATCH (p:Person)-[:KNOWS]->(f:Person)
RETURN p.name, f.name
```


All query languages can be paired with a sibling render block (`source_language: render`) using Rhai map syntax (`#{key: value}`).

### Structural Primacy

Intelligence resides in the data structure, not in the AI model. This is a design commitment verified by the **substitution test**:

- **Swap the AI model** (replace one LLM with another) → the system continues to function with the same knowledge base
- **Remove the data structure** (delete Turso cache, Loro documents, entity graph) → no AI model can reconstruct it

The structure is irreplaceable; the model is not. When evaluating new features, prefer structural investments (schemas, typed relationships, materialized views, query surfaces) over model investments (prompts, fine-tunes, embeddings). Both are valuable, but the ratio should stay heavily structural. The WSJF ranking engine, the task syntax parser, the Petri Net materialization, and the entity type system are all structural intelligence. See [Vision/AI.md](Vision/AI.md) §Structural Primacy and [Vision/PetriNet.md](Vision/PetriNet.md) §Design Decisions.

## Crate Structure

```
crates/
├── holon/                # Main orchestration crate
├── holon-api/            # Shared types for all frontends
├── holon-core/           # Core trait definitions
├── holon-engine/         # Standalone Petri-net engine CLI (YAML nets, WSJF ranking)
├── holon-frontend/       # Platform-agnostic ViewModel layer (MVVM)
├── holon-macros/         # Procedural macros for code generation
├── holon-macros-test/    # Macro expansion tests
├── holon-mcp-client/     # Reusable MCP client → OperationProvider bridge
├── holon-todoist/        # Todoist API integration
├── holon-orgmode/        # Org-mode file integration
├── holon-filesystem/     # File system directory integration
└── holon-integration-tests/ # Cross-crate integration & PBT tests

frontends/
├── gpui/            # GPUI frontend (primary, runs on Android via Dioxus)
├── flutter/         # Flutter frontend with FFI bridge
├── blinc/           # Native Rust GUI frontend (blinc-app)
├── mcp/             # MCP server frontend (stdio + HTTP)
├── dioxus/          # Dioxus frontend
├── ply/             # Ply frontend
├── tui/             # Terminal UI frontend
└── waterui/         # WaterUI frontend
```

### Crate Responsibilities

| Crate | Purpose |
|-------|---------|
| `holon-api` | Value types, Operation descriptors, Change/CDC types, `TypeDefinition`, `IntoEntity`/`TryFromEntity` traits. No frontend-specific deps. |
| `holon-core` | Core traits: DataSource, CrudOperations, BlockOperations, OperationRegistry. Also provides default implementations for BlockOperations (indent, outdent, move, split) |
| `holon-engine` | Standalone Petri-net engine CLI. YAML-defined nets with Rhai guards, WSJF-based ranking, what-if analysis. No dependency on `holon` crate. |
| `holon-frontend` | Platform-agnostic MVVM layer: `ReactiveViewModel`, `ReactiveView`, `ReactiveEngine`, shadow builders, input triggers. Shared by all frontends. |
| `holon-macros` | `#[operations_trait]`, `#[affects(...)]`, entity derives |
| `holon-mcp-client` | Reusable MCP client: connects to any MCP server, converts tool schemas to `OperationDescriptor`s, executes tools via `OperationProvider`. YAML sidecar for UI annotations. |
| `holon-todoist` | Todoist sync provider, operation provider, API client |
| `holon-orgmode` | Org file parsing, DataSource, sync via file watching |
| `holon-integration-tests` | Cross-crate integration tests and property-based tests (PBTs) |
| `gql-parser` (external) | GQL (ISO/IEC 39075) parsing to AST |
| `gql-transform` (external) | GQL AST → SQL compilation via EAV schema |

## Core Traits

### Data Access

```rust
pub trait DataSource<T>: MaybeSendSync {
    async fn get_all(&self) -> Result<Vec<T>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<T>>;
    async fn get_children(&self, parent_id: &str) -> Result<Vec<T>>; // BlockEntity
}

pub trait CrudOperations<T>: MaybeSendSync {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult>;
    async fn create(&self, fields: HashMap<String, Value>) -> Result<(String, OperationResult)>;
    async fn delete(&self, id: &str) -> Result<OperationResult>;
}
```

### Entity Behavior

```rust
pub trait BlockEntity: MaybeSendSync {
    fn id(&self) -> &str;
    fn parent_id(&self) -> Option<&str>;
    fn sort_key(&self) -> &str;     // Fractional index for ordering
    fn depth(&self) -> i64;
    fn content(&self) -> &str;
}

pub trait TaskEntity: MaybeSendSync {
    fn completed(&self) -> bool;
    fn priority(&self) -> Option<i64>;
    fn due_date(&self) -> Option<DateTime<Utc>>;
}
```

These compile-time traits define built-in entity types. User-defined types (Person, Book, Organization) will be defined at runtime via YAML type definitions with a `FieldLifetime` enum governing storage and reconstruction. See [Entity Type System](docs/Architecture/Schema.md#entity-type-system-partially-implemented).

### Domain Operations

```rust
pub trait BlockOperations<T>: BlockDataSourceHelpers<T> {
    async fn indent(&self, id: &str, parent_id: &str) -> Result<OperationResult>;
    async fn outdent(&self, id: &str) -> Result<OperationResult>;
    async fn move_block(&self, id: &str, parent_id: &str, after_block_id: Option<&str>) -> Result<OperationResult>;
    async fn split_block(&self, id: &str, position: i64) -> Result<OperationResult>;
    async fn move_up(&self, id: &str) -> Result<OperationResult>;
    async fn move_down(&self, id: &str) -> Result<OperationResult>;
}

pub trait TaskOperations<T>: CrudOperations<T> {
    async fn set_state(&self, id: &str, task_state: String) -> Result<OperationResult>;
    async fn set_priority(&self, id: &str, priority: i64) -> Result<OperationResult>;
    async fn set_due_date(&self, id: &str, due_date: Option<DateTime<Utc>>) -> Result<OperationResult>;
}
```

### Operation Discovery

```rust
pub trait OperationRegistry: MaybeSendSync {
    fn all_operations() -> Vec<OperationDescriptor>;
    fn entity_name() -> &'static str;
    fn short_name() -> Option<&'static str> { None }
}

pub struct OperationDescriptor {
    pub name: String,
    pub description: String,
    pub params: Vec<ParamDescriptor>,
    pub affected_fields: Vec<String>,
}
```

Operations return `OperationResult` which includes `Vec<FieldDelta>` for CDC-level change tracking and an `UndoAction` for reversible operations. `FieldDelta` captures individual field changes at the operation level, while CDC captures row-level changes at the database level — both exist because operations may affect multiple rows (e.g., `indent` updates depth on descendants).


## Architecture Details

Detailed documentation lives in `docs/arch/`:

| File | Covers |
|------|--------|
| [storage.md](docs/Architecture/Storage.md) | QueryableCache, TursoBackend, CDC, DbHandle, Command Sourcing |
| [render-pipeline.md](docs/Architecture/RenderPipeline.md) | Query compilation (PRQL/GQL/SQL), EntityProfile, ReactiveViewModel, Three-Tier Events |
| [operations.md](docs/Architecture/Operations.md) | Operation System, Action Watcher, Undo/Redo, Procedural Macros |
| [integrations.md](docs/Architecture/Integrations.md) | External System Pattern, MCP Client, Dependency Injection, Frontend Architecture |
| [schema.md](docs/Architecture/Schema.md) | SchemaModule System, Entity Type System, FieldLifetime, Value Types |
| [engine.md](docs/Architecture/Engine.md) | Standalone Petri-Net Engine, Fractional Indexing, Platform Support |
| [sync.md](docs/Architecture/Sync.md) | Loro CRDT, CollaborativeDoc, LoroBackend, EventBus, P2P, Consistency Model |

See also [wiki/overview.md](wiki/overview.md) for the navigational layer and [wiki/index.md](wiki/index.md) for per-crate / per-concept pages.

## Key Files

| Path | Description |
|------|-------------|
| `crates/holon-core/src/traits.rs` | Core trait definitions (DataSource, CrudOperations, BlockOperations) |
| `crates/holon-core/src/undo.rs` | In-memory UndoStack for session-level undo/redo |
| `crates/holon-core/src/operation_log.rs` | OperationLogEntry entity and OperationStatus enum |
| `crates/holon/src/core/operation_log.rs` | OperationLogStore for persistent undo/redo |
| `crates/holon-macros/src/lib.rs` | Procedural macros (#[derive(Entity)], #[operations_trait]) |
| `crates/holon-api/src/entity.rs` | Entity types (DynamicEntity, TypeDefinition, IntoEntity, TryFromEntity) |
| `crates/holon-api/src/reactive.rs` | Reactive stream operators (scan_state, switch_map, combine_latest, coalesce), MapDiff, CdcAccumulator |
| `crates/holon/src/sync/live_data.rs` | CDC-driven collection with watch-based version notification |
| `crates/holon/src/api/ui_watcher.rs` | watch_ui: merge_triggers → switch_map → UiEvent stream |
| `crates/holon/src/storage/turso.rs` | Turso backend + CDC |
| `crates/holon/src/sync/collaborative_doc.rs` | Loro CRDT + Iroh P2P sync |
| `crates/holon/src/sync/loro_module.rs` | Standalone Loro DI module (independent of OrgMode) |
| `crates/holon/src/sync/loro_block_operations.rs` | OperationProvider routing writes through Loro CRDT |
| `crates/holon/src/sync/loro_event_adapter.rs` | Bridges Loro changes → EventBus |
| `crates/holon/src/core/sql_operation_provider.rs` | Direct SQL block operations (fallback when Loro disabled) |
| `crates/holon/src/api/loro_backend.rs` | LoroBackend: CoreOperations implementation for block documents |
| `crates/holon/src/api/repository.rs` | Repository trait definitions (CoreOperations, Lifecycle, P2POperations) |
| `crates/holon/src/petri.rs` | Petri-net materialization from blocks for WSJF ranking |
| `crates/holon-engine/src/` | Standalone Petri-net engine: `engine.rs` (firing/ranking), `guard.rs` (Rhai evaluation), `yaml/` (YAML net/state/history) |
| `crates/holon/src/storage/dynamic_schema_module.rs` | Runtime-generated SchemaModule from TypeDefinition |
| `crates/holon-mcp-client/src/mcp_provider.rs` | MCP connection + McpOperationProvider (OperationProvider impl) |
| `crates/holon-mcp-client/src/mcp_sidecar.rs` | YAML sidecar types, RhaiPrecondition (parse-don't-validate) |
| `crates/holon-mcp-client/src/mcp_schema_mapping.rs` | JSON Schema → TypeHint/OperationParam conversion |
| `crates/holon-todoist/todoist_mcp_operations.yaml` | Todoist MCP sidecar (entity mappings + tool annotations) |
| `crates/holon-todoist/src/` | Todoist integration |
| `frontends/gpui/src/` | GPUI frontend (primary) |
| `frontends/flutter/rust/src/` | Flutter FFI bridge |
| `frontends/mcp/src/tools.rs` | MCP tool implementations (unified `execute_query` for PRQL/GQL/SQL) |
