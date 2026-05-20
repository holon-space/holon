---
title: holon crate (main orchestration)
type: entity
tags: [crate, backend, orchestration, storage, sync]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon/src/lib.rs
  - crates/holon/src/api/backend_engine.rs
  - crates/holon/src/api/holon_service.rs
  - crates/holon/src/api/ui_watcher.rs
  - crates/holon/src/storage/turso.rs
  - crates/holon/src/storage/schema_modules.rs
  - crates/holon/src/di/mod.rs
  - crates/holon/src/petri.rs
---

# holon crate

The main orchestration crate. Everything that touches real data flows through here.

## Module Structure

```
crates/holon/src/
├── api/
│   ├── backend_engine.rs    # BackendEngine — query execution, operation dispatch
│   ├── holon_service.rs     # HolonService — shared service layer (used by MCP + tests)
│   ├── ui_watcher.rs        # watch_ui() — streaming reactive UI events per block
│   ├── block_domain.rs      # Block domain operations (indent, outdent, move, split)
│   ├── operation_dispatcher.rs # Operation routing
│   └── repository.rs        # CoreOperations trait + LoroBackend impl
├── storage/
│   ├── turso.rs             # TursoBackend + DbHandle actor + CDC broadcast
│   ├── schema_modules.rs    # CoreSchemaModule, BlockHierarchySchemaModule, etc.
│   ├── schema_module.rs     # SchemaModule trait
│   ├── dynamic_schema_module.rs # Runtime-registered entity tables
│   ├── sql_utils.rs         # SQL helpers
│   └── graph_schema.rs      # EAV graph schema for GQL queries
├── sync/
│   ├── loro_document_store.rs # Global LoroTree document store
│   ├── loro_document.rs       # LoroDocument wrapper
│   ├── loro_blocks_datasource.rs # Reads blocks from Loro
│   ├── loro_block_operations.rs  # Write operations via Loro
│   ├── loro_sync_controller.rs   # Loro ↔ Turso reconciliation
│   ├── matview_manager.rs        # Materialized view lifecycle
│   ├── live_data.rs              # LiveData<Block> (BlockFeed): CDC mirror of the block matview
│   ├── consolidator.rs           # BlockConsolidator: single writer Loro → SQL block_raw
│   └── event_bus.rs              # Shared sync vocabulary (EventOrigin, PublishErrorTracker) — no bus
├── di/
│   ├── lifecycle.rs          # create_backend_engine(), startup flow
│   ├── registration.rs       # register_core_services()
│   └── schema_providers.rs   # DbReady, DbResource
├── petri.rs                  # Task → Petri Net materialization for WSJF ranking
├── entity_profile.rs         # EntityProfile system for runtime render resolution
├── navigation/               # Navigation cursor, history, current_focus matviews
├── render_dsl.rs             # Rhai-based render DSL parser
└── type_registry.rs          # Runtime type registry
```

## BackendEngine

`crates/holon/src/api/backend_engine.rs` — the central query and operation hub.

```rust
pub struct BackendEngine {
    db_handle: DbHandle,
    operation_dispatcher: OperationDispatcher,
    undo_stack: Arc<UndoStack>,
    // ...
}
```

Key methods:
- `execute_query(sql, params)` — runs compiled SQL against Turso
- `subscribe_sql(sql)` — returns a live `RowChangeStream` backed by Turso IVM
- `dispatch_operation(op, params)` — routes operation to correct provider
- `compile_query(query, lang, context)` — PRQL/GQL/SQL → SQL compilation
- `profile_resolver()` — access to the `EntityProfile` system

### QueryContext

Specifies `current_block_id` (for `from children`), `context_parent_id` (for `from siblings`), and `context_path_prefix` (for `from descendants`). Used by virtual PRQL tables defined in `PRQL_STDLIB`.

PRQL virtual tables: `children`, `siblings`, `descendants`, `roots`, `tasks`, `focus_roots`.

## HolonService

`crates/holon/src/api/holon_service.rs` — shared service adapter used by both MCP and integration tests. Wraps `BackendEngine` and adds:
- `execute_query()` with timing
- `list_tables()` returning `SchemaListing`
- `build_context()` — resolves block ID to `QueryContext`
- `undo()`, `redo()` — via `UndoStack`

## watch_ui

`crates/holon/src/api/ui_watcher.rs` — `watch_ui(engine, block_id)` is the main reactive UI primitive.

1. Creates a structural SQL matview on `block` table (`WHERE id = X OR parent_id = X`)
2. Subscribes to `RowChangeStream` from Turso IVM
3. Merges structural CDC + `WatcherCommand` channel + profile version changes into `RenderTrigger` stream
4. On each trigger, calls `BlockDomain::render_entity()` to re-render
5. Returns `WatchHandle` (output `UiEvent` stream + command sender)

The output stream emits `UiEvent::Structure { widget_spec, generation }` on re-render and `UiEvent::Data { batch, generation }` on data changes. See [[concepts/cdc-and-streaming]].

## Storage Layer

### TursoBackend

`crates/holon/src/storage/turso.rs` — actor-based database access.

```rust
pub struct TursoBackend {
    db: Arc<Database>,
    cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    tx: mpsc::Sender<DbCommand>,
}
```

- All access goes through `DbHandle` (a cheap clone of `mpsc::Sender<DbCommand>`)
- Single actor serializes all DB operations — no concurrent write contention
- CDC fires after each transaction via Turso's row change hooks
- `coalesce_row_changes()` merges DELETE+INSERT pairs into UPDATE events (prevents widget flicker from IVM updates)

### Schema Modules

`crates/holon/src/storage/schema_modules.rs` — dependency-ordered schema initialization.

| Module | Provides |
|--------|----------|
| `CoreSchemaModule` | `block`, `directory`, `file` tables |
| `BlockHierarchySchemaModule` | `block_with_path` materialized view |
| `NavigationSchemaModule` | `navigation_history`, `navigation_cursor`, `current_focus` |
| `SyncStateSchemaModule` | `sync_states` |
| `OperationsSchemaModule` | `operations` (undo/redo log) |

SQL DDL lives in `crates/holon/sql/schema/`.

### Block Table Schema

Key columns: `id TEXT PRIMARY KEY`, `content TEXT`, `content_type TEXT`, `source_language TEXT`, `parent_id TEXT`, `sort_key TEXT`, `depth INTEGER`, `task_state TEXT`, `priority INTEGER`, `tags TEXT`, `properties TEXT`, `document_id TEXT`, `created_at TEXT` (note: TEXT type but `Block` struct has `i64` — always provide explicit millis on create).

## Petri Net Materialization

`crates/holon/src/petri.rs` — materializes task blocks into a Petri Net for WSJF ranking.

- `TaskToken` — represents entities (Person, Document, etc.)
- `TaskTransition` — represents a task/action
- Content prefix parsing: `>` = sequential dep, `@[[Person]]:` = delegation, `?` = question/knowledge
- `resolve_prototype()` — merges prototype properties with instance properties, evaluates `=`-prefixed Rhai expressions
- `rank_tasks()` — scans DB for `prototype_for IS NOT NULL` and `is_self=true` blocks, materializes net, returns `Vec<RankedTransition>` sorted by `delta_per_minute`

See [[concepts/petri-net-wsjf]] for detailed design.

## DI / Startup

`crates/holon/src/di/lifecycle.rs` — `create_backend_engine()` is the main startup entry point.

1. Opens Turso database
2. Runs all `SchemaModule` initializations in dependency order
3. Registers services in `fluxdi` injector
4. Starts Loro sync controller (if enabled)
5. Starts org-mode sync controller (if configured)
6. Returns `Arc<BackendEngine>`

## Related Pages

- [[entities/holon-api]] — shared types
- [[entities/holon-orgmode]] — org sync controller
- [[concepts/cdc-and-streaming]] — CDC pipeline
- [[concepts/reactive-view]] — reactive ViewModel
- [[concepts/petri-net-wsjf]] — WSJF engine
- [[concepts/loro-crdt]] — Loro document store
