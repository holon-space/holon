---
name: holon (core crate)
description: Main holon backend: storage, API, type registry, sync, Petri net wiring, and DI
type: reference
source_type: component
source_id: crates/holon/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon (crates/holon)

**Purpose**: Root backend crate integrating all subsystems: Turso storage, Loro CRDT sync, type registry, navigation, Petri net engine, render DSL, and DI wiring.

### Key Modules

| Module | Role |
|--------|------|
| `api` | `BackendEngine` trait + `backend_engine.rs` with PRQL stdlib |
| `core` | Core data operations and datasource orchestration |
| `computed` | Computed field evaluation |
| `di` | DI module wiring (FluxDI) |
| `entity_profile` | Entity profile configuration and resolution |
| `navigation` | Navigation cursor, history, `current_focus` materialized view |
| `petri` | Petri net engine integration |
| `render_dsl` | Rhai-based render expression evaluation |
| `storage` | Turso + Loro + in-memory storage backends |
| `sync` | CDC-based synchronization and event routing |
| `type_registry` | Pluggable type system with entity profiles |
| `util` | Shared utilities |
| `testing` | Test utilities (native-only, feature-gated) |

### Key Types

| Type | Role |
|------|------|
| `BackendEngine` | Central service: query execution, operation dispatch, CDC subscription |
| `BlockDomain` | Domain-level block operations |
| `BlockHierarchyView` | Hierarchical block view for rendering |
| `CacheEventSubscriber` | Subscribes to CDC events and updates `QueryableCache<Block>` |
| `EditOp` | Edit operation types (create, update, move, delete) |
| `StorageError` | Unified storage error type |
| `SyncStatus` | Sync state tracking |

### Architecture Notes

- PRQL stdlib defined in `backend_engine.rs`; `from children/siblings/descendants` virtual tables require `$context_id` / `$context_parent_id`
- IVM views preloaded before file watching starts to avoid "database is locked"
- Loro outbound reconcile uses `WHERE _expected_content` guard to prevent stale full-row UPDATEs
- `rows_to_blocks` uses `.expect()` (not `.ok()`) — panics on parse failure (fail-loud philosophy)

### Related

- **holon-api**: type definitions consumed here
- **holon-engine**: Petri net engine integrated via `petri` module
- **holon-orgmode**: sync controller wired through `sync` module
- **frontends/mcp**: exposes `BackendEngine` tools externally
