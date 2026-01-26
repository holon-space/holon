//! DI providers for database schema initialization.
//!
//! Each core schema group is represented as a phantom-typed `DbReady<R>` marker
//! registered as an async DI provider. Dependencies between schemas (e.g.,
//! `block_with_path` depends on `block`) are expressed via `with_dependency`
//! hints, letting FluxDI's `resolve_all_eager()` determine the correct
//! topological order and maximize parallelism.
//!
//! ## Compile-time vs runtime schemas
//!
//! Core schemas known at compile time use typed markers (`DbReady<CoreTables>`).
//! User-defined schemas from YAML/MCP use FluxDI dynamic providers with string
//! keys and `depends_on_static::<DbReady<CoreTables>>()`.

use std::marker::PhantomData;

use fluxdi::{Injector, Provider, Shared};

use crate::storage::schema_modules::{
    BlockHierarchySchemaModule, BlockSchemaModule, CoreSchemaModule, IdentitySchemaModule,
    LinkSchemaModule, NavigationSchemaModule, OperationsSchemaModule, SyncStateSchemaModule,
};
use crate::storage::turso::DbHandle;

use super::DbHandleProvider;

// ---------------------------------------------------------------------------
// Phantom type infrastructure
// ---------------------------------------------------------------------------

/// Marker proving that a database resource group has been initialized.
///
/// `R` is a zero-sized type identifying which schema group is ready.
/// Services that need a particular table resolve `DbReady<R>` in their
/// DI factory, making the dependency compiler-checked and visible in
/// FluxDI's dependency graph.
pub struct DbReady<R: DbResource>(PhantomData<R>);

impl<R: DbResource> DbReady<R> {
    fn new() -> Self {
        Self(PhantomData)
    }
}

/// Marker trait for database resource groups.
pub trait DbResource: Send + Sync + 'static {}

// ---------------------------------------------------------------------------
// Marker types — one per schema group
// ---------------------------------------------------------------------------

/// `block`, `directory`, `file` tables.
pub struct CoreTables;
impl DbResource for CoreTables {}

/// `block_with_path` materialized view (depends on `block`).
pub struct BlockHierarchyView;
impl DbResource for BlockHierarchyView {}

/// `navigation_history`, `navigation_cursor`, `current_focus`, etc.
pub struct NavigationTables;
impl DbResource for NavigationTables {}

/// `sync_states` table.
pub struct SyncStateTables;
impl DbResource for SyncStateTables {}

/// `operation` table for undo/redo.
pub struct OperationTables;
impl DbResource for OperationTables {}

/// `task_blockers`, `block_tags`, `task_blocking_edges` (depend on `block`).
pub struct BlockTables;
impl DbResource for BlockTables {}

/// `block_link` table (depends on `block`).
pub struct LinkTables;
impl DbResource for LinkTables {}

/// `canonical_entity`, `entity_alias`, `proposal_queue` tables for cross-system identity.
pub struct IdentityTables;
impl DbResource for IdentityTables {}

/// `graph_eav` schema.
pub struct GraphEavSchema;
impl DbResource for GraphEavSchema {}

// ---------------------------------------------------------------------------
// Helper: run a SchemaModule's DDL via DbHandle
// ---------------------------------------------------------------------------

use crate::storage::schema_module::SchemaModule;

async fn run_schema_module(module: &dyn SchemaModule, db_handle: &DbHandle) -> anyhow::Result<()> {
    module
        .ensure_schema(db_handle)
        .await
        .map_err(|e| anyhow::anyhow!("[{}] ensure_schema failed: {e}", module.name()))?;
    module
        .initialize_data(db_handle)
        .await
        .map_err(|e| anyhow::anyhow!("[{}] initialize_data failed: {e}", module.name()))?;

    // Mark resources available in DbHandle for downstream DDL-dependency checks
    let provides = module.provides();
    if !provides.is_empty() {
        db_handle
            .mark_available(provides)
            .await
            .map_err(|e| anyhow::anyhow!("[{}] mark_available failed: {e}", module.name()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Provider registration
// ---------------------------------------------------------------------------

const GRAPH_EAV_SCHEMA_SQL: &str = include_str!("../../sql/schema/graph_eav.sql");

/// Register all core schema providers on the injector.
///
/// After calling this, `injector.resolve_all_eager().await` will create all
/// tables/views in the correct order with maximum parallelism.
pub fn register_schema_providers(injector: &Injector) {
    // -- CoreTables (no deps) --
    injector.provide::<DbReady<CoreTables>>(Provider::root_async(|inj| async move {
        let db = inj.resolve::<dyn DbHandleProvider>();
        run_schema_module(&CoreSchemaModule, &db.handle())
            .await
            .expect("CoreTables schema init failed");
        Shared::new(DbReady::<CoreTables>::new())
    }));

    // -- BlockHierarchyView (depends on CoreTables) --
    injector.provide::<DbReady<BlockHierarchyView>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            let db = inj.resolve::<dyn DbHandleProvider>();
            run_schema_module(&BlockHierarchySchemaModule, &db.handle())
                .await
                .expect("BlockHierarchyView schema init failed");
            Shared::new(DbReady::<BlockHierarchyView>::new())
        })
        .with_dependency::<DbReady<CoreTables>>(),
    );

    // -- NavigationTables (depends on CoreTables: focus_roots matview JOINs block table) --
    injector.provide::<DbReady<NavigationTables>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            let db = inj.resolve::<dyn DbHandleProvider>();
            run_schema_module(&NavigationSchemaModule, &db.handle())
                .await
                .expect("NavigationTables schema init failed");
            Shared::new(DbReady::<NavigationTables>::new())
        })
        .with_dependency::<DbReady<CoreTables>>(),
    );

    // -- SyncStateTables (no deps) --
    injector.provide::<DbReady<SyncStateTables>>(Provider::root_async(|inj| async move {
        let db = inj.resolve::<dyn DbHandleProvider>();
        run_schema_module(&SyncStateSchemaModule, &db.handle())
            .await
            .expect("SyncStateTables schema init failed");
        Shared::new(DbReady::<SyncStateTables>::new())
    }));

    // -- OperationTables (no deps) --
    injector.provide::<DbReady<OperationTables>>(Provider::root_async(|inj| async move {
        let db = inj.resolve::<dyn DbHandleProvider>();
        run_schema_module(&OperationsSchemaModule, &db.handle())
            .await
            .expect("OperationTables schema init failed");
        Shared::new(DbReady::<OperationTables>::new())
    }));

    // -- BlockTables (depends on CoreTables: FKs reference block.id) --
    injector.provide::<DbReady<BlockTables>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            let db = inj.resolve::<dyn DbHandleProvider>();
            run_schema_module(&BlockSchemaModule, &db.handle())
                .await
                .expect("BlockTables schema init failed");
            Shared::new(DbReady::<BlockTables>::new())
        })
        .with_dependency::<DbReady<CoreTables>>(),
    );

    // -- LinkTables (depends on CoreTables) --
    injector.provide::<DbReady<LinkTables>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            let db = inj.resolve::<dyn DbHandleProvider>();
            run_schema_module(&LinkSchemaModule, &db.handle())
                .await
                .expect("LinkTables schema init failed");
            Shared::new(DbReady::<LinkTables>::new())
        })
        .with_dependency::<DbReady<CoreTables>>(),
    );

    // -- IdentityTables (no deps; tables are independent of block) --
    injector.provide::<DbReady<IdentityTables>>(Provider::root_async(|inj| async move {
        let db = inj.resolve::<dyn DbHandleProvider>();
        run_schema_module(&IdentitySchemaModule, &db.handle())
            .await
            .expect("IdentityTables schema init failed");
        Shared::new(DbReady::<IdentityTables>::new())
    }));

    // -- GraphEavSchema (depends on CoreTables) --
    injector.provide::<DbReady<GraphEavSchema>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            let db = inj.resolve::<dyn DbHandleProvider>();
            let handle = db.handle();
            for stmt in crate::storage::sql_statements(GRAPH_EAV_SCHEMA_SQL) {
                handle.execute_ddl(stmt).await.expect("GraphEav DDL failed");
            }
            handle
                .mark_available(vec![crate::storage::resource::Resource::schema(
                    "graph_eav",
                )])
                .await
                .expect("GraphEav mark_available failed");
            Shared::new(DbReady::<GraphEavSchema>::new())
        })
        .with_dependency::<DbReady<CoreTables>>(),
    );
}

/// All core schema TypeIds. Single source of truth for `resolve_eager_roots`
/// and `with_dependency` declarations.
pub fn all_schema_roots() -> Vec<std::any::TypeId> {
    use std::any::TypeId;
    vec![
        TypeId::of::<DbReady<CoreTables>>(),
        TypeId::of::<DbReady<BlockTables>>(),
        TypeId::of::<DbReady<BlockHierarchyView>>(),
        TypeId::of::<DbReady<NavigationTables>>(),
        TypeId::of::<DbReady<SyncStateTables>>(),
        TypeId::of::<DbReady<OperationTables>>(),
        TypeId::of::<DbReady<LinkTables>>(),
        TypeId::of::<DbReady<GraphEavSchema>>(),
    ]
}
