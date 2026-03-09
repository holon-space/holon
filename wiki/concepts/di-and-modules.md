---
title: Dependency Injection & Startup Modules
type: concept
tags: [di, dependency-injection, fluxdi, startup, modules]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon/src/di/mod.rs
  - crates/holon/src/di/lifecycle.rs
  - crates/holon/src/di/registration.rs
  - crates/holon/src/di/schema_providers.rs
  - crates/holon/src/storage/schema_modules.rs
---

# Dependency Injection & Startup Modules

Holon uses `fluxdi` for service registration and resolution. DI wiring is centralized to enable testing and configuration.

## fluxdi

`fluxdi` is Holon's DI container. Services are registered by type (via `TypeId`) or by string key. `Injector` resolves services. `Provider<T>` and `Shared<T>` are the registration wrappers.

String-key resolution is used in some places to avoid `TypeId` mismatches across crate boundaries.

## Core DI Traits

`crates/holon/src/di/mod.rs`:

```rust
pub trait TursoBackendProvider: Send + Sync {
    fn backend(&self) -> Arc<RwLock<TursoBackend>>;
}

pub trait DbHandleProvider: Send + Sync {
    fn handle(&self) -> DbHandle;
}
```

These traits abstract over the concrete backend types for cross-crate DI.

## Startup Flow

`crates/holon/src/di/lifecycle.rs::create_backend_engine()`:

1. Open Turso database (`TursoBackend::open(path)`)
2. Run `SchemaModule` initializations in dependency order:
   - `CoreSchemaModule` (no deps) → tables: `block`, `directory`, `file`
   - `BlockHierarchySchemaModule` (requires `block`) → `block_with_path` IVM
   - `NavigationSchemaModule` (requires `block_with_path`) → `navigation_cursor`, etc.
   - `SyncStateSchemaModule` → `sync_states`
   - `OperationsSchemaModule` → `operations`
3. Register core services via `register_core_services(injector)`
4. Start Loro sync controller (if enabled by config)
5. Start `OrgSyncController` (if `root_dir` configured)
6. Preload startup views via `preload_startup_views(engine)`
7. Return `Arc<BackendEngine>`

`create_backend_engine_with_extras(config, extras_fn)` — takes a closure for registering additional services (used by frontends to add frontend-specific services).

## SchemaModule Trait

`crates/holon/src/storage/schema_module.rs`:

```rust
#[async_trait]
pub trait SchemaModule: Send + Sync {
    fn name(&self) -> &str;
    fn provides(&self) -> Vec<Resource>;
    fn requires(&self) -> Vec<Resource>;
    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()>;
}
```

Modules declare `provides` and `requires` as `Resource` (table name or view name). `ModuleRunner` topological-sorts modules before running them.

`ensure_schema` is **idempotent** — all DDL uses `IF NOT EXISTS`. Safe to call on existing databases.

## DynamicSchemaModule

`crates/holon/src/storage/dynamic_schema_module.rs` — registers entity tables at runtime. Used by `QueryableCache<T>` to create the entity's table on demand.

## Registration

`crates/holon/src/di/registration.rs::register_core_services(injector)`:
- Registers `TursoBackend` via `TursoBackendProvider`
- Registers `DbHandle` via `DbHandleProvider`
- Registers `BackendEngine`
- Registers `HolonService`
- Registers query providers (OperationDispatcher)

## Test Helpers

`crates/holon/src/di/test_helpers.rs` — `create_test_backend_engine()` creates a fully initialized in-memory backend for tests. Uses a temp directory and skips org sync.

## DbReady / DbResource

`crates/holon/src/di/schema_providers.rs`:
- `DbReady` — a signal indicating the database is initialized and ready
- `DbResource` — wraps a lazily-initialized database resource

Used to delay service initialization until the database is ready.

## CoreInfraModule

`crates/holon/src/di/lifecycle.rs::CoreInfraModule` — GPUI entity module that holds the `BackendEngine` and provides services to the GPUI app model.

## Related Pages

- [[entities/holon-crate]] — DI is wired in `holon::di::lifecycle`
- [[entities/gpui-frontend]] — GPUI DI in `frontends/gpui/src/di.rs`
- [[entities/holon-orgmode]] — DI adapters in `holon-orgmode/src/di.rs`
