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
Frontend ← Signal<ViewModel> ← ReactiveEngine ← UiEvent ← Turso IVM (CDC)
```

The read path is a persistent-node reactive cache: `ReactiveEngine`
(`crates/holon-frontend/src/reactive.rs`) replaced the earlier
`CdcAccumulator` + `BlockWatchRegistry` + `AppState` trio. Each watched block
or live query owns a `ReactiveRenderedRows` that IS the cache, accumulator, and
`Signal<ViewModel>` source; `ReactiveViewModel`
(`crates/holon-frontend/src/reactive_view_model.rs`) is the persistent-node
shared-VM boundary consumed by every frontend.

- Operations are fire-and-forget
- Effects are observed through sync
- Changes propagate as streams
- Internal and external modifications are treated identically

#### Streaming-first render state

`ReactiveRenderedRows` stores a non-optional `Mutable<RenderExpr>` initialized to `loading()` — a regular `FunctionCall { name: "loading" }`. When the first `Structure` event arrives from `watch_ui`, the real render expression replaces it. Consumers (GPUI signals, MCP snapshots) never see `Option<RenderExpr>` — `loading()` flows through the same interpret → build → render pipeline as any other widget. The `loading` builder (in `shadow_builders/loading.rs`) produces an `Empty` reactive view model, so frontends render nothing until real data arrives.

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
├── holon/                       # Main orchestration crate: sync, storage, API, DI wiring
├── holon-api/                   # Shared value types, CDC types, entity derives
├── holon-app/                   # DI assembly crate (composition root — names concrete backends)
├── holon-core/                  # Core traits, Cell<T>, block ordering, file-format seam
├── holon-engine/                # Standalone Petri-net engine CLI (YAML nets, WSJF ranking)
├── holon-expr/                  # Compiled Rhai expressions shared by holon-api + holon-engine
├── holon-frontend/              # Platform-agnostic ViewModel layer (MVVM)
├── holon-turso/                 # Turso (SQLite-IVM) storage adapter
├── holon-macros/                # Procedural macros for code generation
├── holon-macros-test/           # Macro expansion tests
├── holon-mcp-client/            # Reusable MCP client → OperationProvider bridge
├── holon-orgmode/               # Org-mode file watching, DI wiring, sync controller
├── holon-org-format/            # Pure org-mode parsing, rendering, diffing (no disk I/O)
├── holon-filesystem/            # FileSystem + FileChangeSource ports and adapters
├── holon-pbt-core/              # Cross-PBT transition traits shared between PBT crates
├── holon-layout-testing/        # Shared layout-testing primitives for UI PBTs
├── holon-block-roundtrip-testing/ # Block round-trip generators + NormalizedDocument comparison
├── holon-architecture-tests/    # `cargo test` wrapper that shells out to archlint
└── holon-integration-tests/     # Cross-crate integration & PBT tests

frontends/
├── gpui/        # GPUI frontend (primary — desktop; mobile via gpui-mobile feature)
├── mcp/         # MCP server frontend (stdio + HTTP)
├── tui/         # Terminal UI frontend
├── dioxus/      # Dioxus frontend (prototype)
├── dioxus-web/  # Dioxus web frontend (prototype)
├── holon-worker/ # Web worker support
└── waterui/     # WaterUI frontend (excluded from workspace — upstream compatibility issues)
```

> The tree above is the at-a-glance layout. The authoritative, machine-checked
> inventory (with each crate's purpose and C4 level) is generated to
> [CrateMap.md](Architecture/CrateMap.md) — see below.

### Crate Responsibilities

The per-crate inventory is **generated**, not hand-maintained here. Each crate's
purpose is the `@c4` annotation at the top of its `src/lib.rs` (the single source
of truth); [archidoc](https://github.com/GitSmart86/archidoc) projects those into
**[CrateMap.md](Architecture/CrateMap.md)** (plus C4 PlantUML diagrams under
`Architecture/c4/`) and `just arch-validate` fails CI if the crate/frontend structure
drifts from the committed baseline. Regenerate the map and diagrams with
`just arch-docs` after editing an annotation.

To change a crate's description, edit its `src/lib.rs` `@c4` block — not this file.

External (non-workspace) crates, for reference:

| Crate | Purpose |
|-------|---------|
| `gql-parser` (external) | GQL (ISO/IEC 39075) parsing to AST |
| `gql-transform` (external) | GQL AST → SQL compilation via EAV schema |

## Core Traits

### Data Access

```rust
pub trait DataSource<T>: MaybeSendSync {
    async fn get_all(&self) -> Result<Vec<T>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<T>>;
    async fn get_children(&self, parent_id: &EntityUri) -> Result<Vec<T>>; // where T: BlockEntity
}

pub trait CrudOperations<T>: MaybeSendSync {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult>;
    async fn create(&self, fields: StorageEntity) -> Result<(String, OperationResult)>;
    async fn delete(&self, id: &str) -> Result<OperationResult>;
}
```

### Entity Behavior

```rust
pub trait BlockEntity: MaybeSendSync {
    fn id(&self) -> &EntityUri;
    fn parent_id(&self) -> Option<&EntityUri>;
    fn depth(&self) -> i64;
    fn content(&self) -> &str;
    fn tags(&self) -> Tags;            // the `"Page"` tag marks an org-file root
    fn is_page(&self) -> bool;         // default impl: tags().contains("Page")
}

pub trait TaskEntity: MaybeSendSync {
    fn completed(&self) -> bool;
    fn priority(&self) -> Option<i64>;
    fn due_date(&self) -> Option<DateTime<Utc>>;
}
```

These compile-time traits define built-in entity types. User-defined types are defined at runtime via `DynamicSchemaModule`, which builds a schema module from a `TypeDefinition` — see [Module registry](Architecture/Schema.md#module-registry).

### Domain Operations

```rust
// crates/holon-core/src/traits.rs (abbreviated)
pub trait BlockOperations<T>: BlockDataSourceHelpers<T> {
    // spec-0008 seams — default None; production impls override
    fn cells(&self) -> Option<&dyn EntityCellRegistry>;
    fn ordering(&self) -> Option<&dyn BlockOrdering>;
    fn order_key_minter(&self) -> Option<&dyn OrderKeyMinting>;

    async fn indent(&self, id: &EntityUri) -> Result<OperationResult>; // derives the previous sibling itself
    async fn outdent(&self, id: &EntityUri) -> Result<OperationResult>;
    async fn move_block(&self, id: &EntityUri, parent_id: &EntityUri, after_block_id: Option<&EntityUri>) -> Result<OperationResult>;
    async fn move_to_position(&self, id: &EntityUri, parent_id: &EntityUri, after_block_id: Option<&EntityUri>) -> Result<Vec<FieldDelta>>;
    async fn split_block(&self, id: &EntityUri, position: i64) -> Result<OperationResult>;
    async fn move_up(&self, id: &EntityUri) -> Result<OperationResult>;
    async fn move_down(&self, id: &EntityUri) -> Result<OperationResult>;
}

pub trait TaskOperations<T>: MaybeSendSync {
    async fn set_title(&self, id: &str, title: &str) -> Result<OperationResult>;
    fn completion_states_with_progress(&self) -> Vec<CompletionStateInfo>;
    async fn set_state(&self, id: &str, task_state: String) -> Result<OperationResult>;
    async fn cycle_task_state(&self, id: &str) -> Result<OperationResult>;
    async fn set_priority(&self, id: &str, priority: i64) -> Result<OperationResult>;
    async fn set_due_date(&self, id: &str, due_date: Option<DateTime<Utc>>) -> Result<OperationResult>;
}
```

The ordering seam: `BlockOrdering` is the positional authority (it encapsulates the Loro `tree.mov` vs SqlOnly `gen_key_between` split), `children_ordered` on `BlockQueryHelpers` is the single ordered-read primitive (sibling order is a property of the parent→children relation, never of a per-block encoding), and `order_key_minter()` returns `Some` only when the store is the SqlOnly consolidator that mints order keys — in Loro mode the tree owns the fractional index and no key is minted on that path.

### Operation Discovery

```rust
pub trait OperationRegistry: MaybeSendSync {
    fn all_operations() -> Vec<OperationDescriptor>;
    fn entity_name() -> &'static str;
    fn short_name() -> Option<&'static str> { None }
}

// crates/holon-api/src/render_types.rs (abbreviated)
pub struct OperationDescriptor {
    pub entity_name: EntityName,
    pub entity_short_name: String,
    pub id_column: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub required_params: Vec<OperationParam>,
    pub affected_fields: Vec<String>,
    pub param_mappings: Vec<ParamMapping>,
    pub trigger: Option<Trigger>,
    pub bound_params: HashMap<String, Value>,
    pub precondition: Option<Arc<Box<PreconditionChecker>>>,
}
```

The `OperationRegistry` trait lives in `crates/holon-core/src/traits.rs`; the descriptor types (`OperationDescriptor`, `OperationParam`, `ParamMapping`, `Trigger`) live in `crates/holon-api/src/render_types.rs`.

Operations return `OperationResult` which includes `Vec<FieldDelta>` for CDC-level change tracking and an `UndoAction` for reversible operations. `FieldDelta` captures individual field changes at the operation level, while CDC captures row-level changes at the database level — both exist because operations may affect multiple rows (e.g., `indent` updates depth on descendants).

## Architecture Details

Detailed documentation lives in `docs/Architecture/`:

| File | Covers |
| ------ | -------- |
| [Model.md](Architecture/Model.md) | ★ **Read first** — the one-page mental model: five layers, mode axes, Loro's three capabilities, invariants 1–12 |
| [storage.md](Architecture/Storage.md) | QueryableCache, TursoBackend, CDC, DbHandle, Command Sourcing |
| [render-pipeline.md](Architecture/RenderPipeline.md) | Query compilation (PRQL/GQL/SQL), EntityProfile, ReactiveViewModel, Three-Tier Events |
| [operations.md](Architecture/Operations.md) | Operation System, Action Watcher, Undo/Redo, Procedural Macros |
| [integrations.md](Architecture/Integrations.md) | External System Pattern, MCP Client, Dependency Injection, Frontend Architecture |
| [schema.md](Architecture/Schema.md) | Table/view-level schema reference: module registry, block base table + junctions + hydration matview, hierarchy, navigation, sync/operations/links/identity, graph EAV |
| [engine.md](Architecture/Engine.md) | Standalone Petri-Net Engine, Fractional Indexing, Platform Support |
| [sync.md](Architecture/Sync.md) | Loro CRDT, CollaborativeDoc, LoroBackend, sync wiring (LiveData<Block> + direct cache feeds), P2P, Consistency Model |
| [replication.md](Architecture/Replication.md) | Target replication model: capability profiles, per-component base + 3-way merge, single-owner ordering, consolidator/sink roles, two transports |
| [archlint.md](Architecture/Archlint.md) | Architecture linter (ast-grep YAML + ripgrep smells + dylint cdylib), Claude Code PostToolUse hook, ALLOW protocol, cargo arch-test wrapper |

See also [wiki/overview.md](../wiki/overview.md) for the navigational layer and [wiki/index.md](../wiki/index.md) for per-crate / per-concept pages.

## Key Files

| Path | Description |
| ------ | ------------- |
| `crates/holon-core/src/traits.rs` | Core trait definitions (DataSource, CrudOperations, BlockOperations) |
| `crates/holon-core/src/undo.rs` | In-memory UndoStack for session-level undo/redo |
| `crates/holon-core/src/operation_log.rs` | OperationLogEntry entity and OperationStatus enum |
| `crates/holon/src/core/operation_log.rs` | OperationLogStore for persistent undo/redo |
| `crates/holon-macros/src/lib.rs` | Procedural macros (#[derive(Entity)], #[operations_trait]) |
| `crates/holon-api/src/entity.rs` | Entity types (DynamicEntity, TypeDefinition, IntoEntity, TryFromEntity) |
| `crates/holon-api/src/reactive.rs` | Reactive stream operators (scan_state, switch_map, combine_latest, coalesce), MapDiff, CdcAccumulator |
| `crates/holon-api/src/live_data.rs` | `LiveData<T>` CDC-driven collection + `BlockFeed` (CDC mirror of the block matview) for reactive consumers |
| `crates/holon/src/api/ui_watcher.rs` | watch_ui: merge_triggers → switch_map → UiEvent stream |
| `crates/holon-turso/src/turso.rs` | Turso backend + CDC |
| `crates/holon/src/sync/loro_module.rs` | Standalone Loro DI module (independent of OrgMode) |
| `crates/holon-loro/src/iroh_sync_adapter.rs` | Iroh P2P sync adapter (transport only) |
| `crates/holon-loro/src/loro_share_backend.rs` | Loro document sharing (P2P) |
| `crates/holon-loro/src/multi_peer.rs` | Shared multi-peer Loro sync infrastructure for property-based testing (PeerState/GroupState/GroupTransition) |
| `crates/holon-loro/src/loro_block_operations.rs` | OperationProvider routing writes through Loro CRDT |
| `crates/holon-loro/src/loro_sync_controller.rs` | LoroProjection: single writer projecting Loro → SQL block_raw |
| `crates/holon/src/core/sql_operation_provider.rs` | Direct SQL block operations (fallback when Loro disabled) |
| `crates/holon-loro/src/loro_backend.rs` | LoroBackend: CoreOperations implementation for block documents |
| `crates/holon-api/src/repository.rs` | Repository trait definitions (CoreOperations, Lifecycle, P2POperations) |
| `crates/holon-petri/src/lib.rs` | Petri-net materialization from blocks for WSJF ranking |
| `crates/holon-engine/src/` | Standalone Petri-net engine: `engine.rs` (firing/ranking), `guard.rs` (Rhai evaluation), `yaml/` (YAML net/state/history) |
| `crates/holon-turso/src/dynamic_schema_module.rs` | Runtime-generated SchemaModule from TypeDefinition |
| `crates/holon-mcp-client/src/mcp_provider.rs` | MCP connection + McpOperationProvider (OperationProvider impl) |
| `crates/holon-mcp-client/src/mcp_sidecar.rs` | YAML sidecar types (McpSidecar, SyncConfig, ToolConfig, RhaiPrecondition) |
| `crates/holon-mcp-client/src/mcp_schema_mapping.rs` | JSON Schema → TypeHint/OperationParam conversion |
| `crates/holon-mcp-client/src/mcp_sync_engine.rs` | McpSyncEngine: bulk + incremental cache sync, resource-notification re-sync |
| `crates/holon-mcp-client/src/integration_config.rs` | IntegrationFileConfig: top-level YAML schema, ${VAR} expansion |
| `crates/holon-app/src/mcp_integrations.rs` | McpIntegrationsModule + McpIntegrationRegistry + RegistryOperationProxy |
| `docs/integrations/todoist.yaml` | Todoist integration config (transport, auth, entities, tools, sync, undo) |
| `frontends/gpui/src/` | GPUI frontend (primary) |
| `frontends/mcp/src/tools.rs` | MCP tool implementations (unified `execute_query` for PRQL/GQL/SQL) |
