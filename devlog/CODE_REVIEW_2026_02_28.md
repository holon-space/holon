# Code Review — 2026-02-28

Comprehensive review of the holon Rust workspace by a 5-agent team covering architecture, patterns, parse-don't-validate, code simplicity, and Rust best practices.

---

## What's Good

- **Clean dependency DAG**: frontends → `holon-frontend` → integration crates → `holon` → `holon-api`/`holon-core`. `holon` never depends on `holon-orgmode` or frontends.
- **Strong Parse, Don't Validate discipline**: 14 priorities already migrated (EntityUri, ContentType, TaskState, Priority, Tags, Timestamp, OrgProperties, EventKind, etc.).
- **Trait-based decoupling in holon-orgmode**: `BlockReader`, `DocumentManager` invert dependencies correctly.
- **holon-api as shared vocabulary**: Types like `Block`, `EntityUri`, `Value`, `WidgetSpec`, `RenderExpr` used consistently across all crates.
- **holon-frontend as composition root**: `FrontendSession` centralizes DI and lifecycle for all 3 frontends.
- **holon-engine fully independent**: Zero workspace dependencies, standalone Petri net CLI.

---

## Tier 2 — Completed (this session)

### 1. eprintln! → tracing (Task #7) ✅

Replaced ~150 `eprintln!` calls across 20 files in `crates/` with structured `tracing` macros (`error!`, `warn!`, `info!`, `debug!`). Files in `frontends/` and test code were left untouched.

Top files migrated:
- `sync/loro_block_operations.rs` (24), `di/lifecycle.rs` (23), `di/registration.rs` (18), `navigation/provider.rs` (13), `sync/turso_event_bus.rs` (12), `sync/cache_event_subscriber.rs` (8)

### 2. Extract duplicated utilities + fix topo sort (Task #8) ✅

Created `crates/holon/src/util.rs` with 3 extracted functions:
- **`strip_order_by()`** — was duplicated in `backend_engine.rs` + `watched_query.rs`. Optimized to avoid `to_uppercase()` allocation when no ORDER BY present.
- **`expr_references()`** — was duplicated in `petri.rs` + `entity_profile.rs`.
- **`topo_sort_kahn()`** — was duplicated in `petri.rs` + `entity_profile.rs`. **Fixed O(n²) → O(n)**: replaced `Vec::remove(0)` with `VecDeque::pop_front()`.

### 3. Collapse double channel relay in watch_query (Task #9) ✅

Refactored `watch_query` in `backend_engine.rs` from 2 relay hops to 1: the filter now runs inside the single relay task, matching the pattern already used by `WatchedQuery::new()`.

### 4. EntityUri migration into BlockDiff + QueryContext (Task #10) ✅

- **BlockDiff** (`block_diff.rs`): All `String`/`Option<String>` ID fields → `EntityUri`/`Option<EntityUri>`. `PropertiesChanged.changes` kept as string tuples (property values, not entity IDs).
- **QueryContext** (`backend_engine.rs`): `current_block_id` and `context_parent_id` → `Option<EntityUri>`. `context_path_prefix` kept as `Option<String>` (SQL LIKE prefix). All 4 downstream callers (MCP, Flutter FFI, Blinc, integration tests) updated.

### 5. Document.properties String → HashMap (Task #11) ✅

`Document.properties` was already `HashMap<String, Value>` with `#[jsonb]` (migrated in prior session). Simplified `set_org_title()` and `set_todo_keywords()` to direct `self.properties.remove()` instead of serialize/deserialize round-trips. Fixed import paths for `ROOT_DOC_ID`/`NO_PARENT_DOC_ID` constants (moved to `holon_api::document`).

---

## Tier 1 — Quick Wins (not yet done)

### Dead code to remove (~2,400 LOC)

| Module | Lines | Why dead |
|---|---|---|
| `references/` (BlockReference, ViewConfig) | ~190 | Superseded by EntityUri + WidgetSpec |
| `tasks.rs` + `task_datasource.rs` | ~421 | Old prototype; system uses `Block` now |
| `core/transform/` (AstTransformer pipeline) | ~222 | Always `TransformPipeline::empty()` |
| `core/unified_query.rs`, `core/updates.rs` | ~532 | Typed query DSL — superseded by PRQL/SQL/GQL |
| `storage/command_sourcing.rs` | ~170 | Offline-first via Loro CRDTs instead |
| `sync/external_system.rs` | ~85 | Zero implementations |
| `api/ui_types.rs`, `operations/row_view.rs` | ~205 | Never used outside own tests |
| `examples/`, `main.rs` | ~9 | Commented out / placeholder |
| `adapter/sync_stats.rs` | ~18 | Never instantiated |
| 3 deprecated Flutter PBT files | ~200 | Superseded by `flutter_mutation_driver.rs` |

### Other quick wins

- **`STARTUP_QUERIES`** in `di/mod.rs` uses removed `render()` PRQL syntax — broken if still compiled
- **`RenderSpec`, `ViewSpec`, `FilterExpr`, `RowTemplate`** in holon-api — vestigial types, Flutter FRB ignores them
- **Backward-compat aliases** in `datasource.rs` — old trait names (`MutableBlockDataSource` etc.)
- **`ParamDescriptor`** — legacy type, superseded by `OperationParam`
- **Deprecated functions**: `pbt_get_all_blocks()`, `blocks_to_parsed_map()`, `headlines_to_block_map()`, `db_handle()`, `to_string_legacy()`

---

## Tier 3 — Larger Refactors (not yet done)

### Architecture

- **Split `BackendEngine`** (2,263 lines, 30+ methods) into focused components: query compiler, operation executor, render coordinator.
- **Move shared traits out of `holon`** so `holon-filesystem`, `holon-todoist`, `holon-mcp-client` don't pull the full dependency tree.
- **Make frontends consistently use `holon-frontend` facade** — MCP and Flutter FFI currently reach into `holon::` internals.
- **Consolidate dual undo systems** — in-memory `UndoStack` + persistent `OperationLogStore`.
- **`FrontendSession` as thin passthrough** — nearly every method delegates to `self.engine`. Either expose `engine()` or limit to lifecycle.

### Rust best practices

- **Resolve `unsafe impl Send/Sync`** on `EntityProfile` (likely caused by `CompiledExpr`).
- **`std::sync::RwLock` in `LiveData`** — blocks tokio runtime; should use `tokio::sync::RwLock`.
- **`execute_query` retry** has identical branches (lines 719-726) — dead logic.
- **`_backend_keepalive: Arc<RwLock<TursoBackend>>`** — the RwLock wrapper is unnecessary for keepalive.
- **`Tags::from_iter`** shadows `FromIterator` — should implement the standard trait.
- **`EventStatus::from_str` / `EventOrigin::from_str`** are inherent methods, not `FromStr` trait.

### Parse, Don't Validate — remaining violations

| Priority | What | Where | Risk |
|---|---|---|---|
| 16 | `OperationLogEntry.status: String` | `operation_log.rs:76` | Undo silently breaks |
| 18 | `operation/inverse: String` with `.ok()` | `operation_log.rs:111,117` | Undo silently fails |
| 19 | `CommandEntry.entity_type: String` | `command_log.rs:25` | `AggregateType` exists |
| 20 | `EventId`/`CommandId` type aliases | `event_bus.rs:77,80` | No compiler protection |
| 22 | `todo_keywords: Option<String>` | `models.rs:299-373` | Parsed in 3 places |
| 26 | `BlockDiff` String IDs | `block_diff.rs` | **FIXED** this session |
| 27 | `QueryContext` String IDs | `backend_engine.rs` | **FIXED** this session |
| B | `EventStatus::from_str` → `Confirmed` | `turso_event_bus.rs:241` | Silent corruption |
| D | `sort_key: String` no newtype | `document.rs:51` | No invariant enforcement |

### Code duplication remaining

- PRQL compilation call duplicated in `backend_engine.rs` + `todoist/queries.rs`
- View name hashing computed in 3 places
- Test engine creation: 6+ variants across 5+ files (candidate for builder pattern)

### Naming inconsistencies

- `handle()` vs `db_handle()` — TursoBackend has both
- Adapter vs Provider vs Controller — no documented convention
- `holon-core` vs `holon::core` — separate crate and module, one re-exports the other
