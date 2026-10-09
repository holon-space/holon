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
//! Core schemas known at compile time use typed markers
//! (`DbReady<CoreTables>`). User-defined schemas from YAML/MCP use FluxDI
//! dynamic providers with string
//! keys and `depends_on_static::<DbReady<CoreTables>>()`.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::Mutex;

use fluxdi::Injector;
use fluxdi::Provider;
use fluxdi::Shared;
use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_turso::schema_modules::AutomationsJournalSchemaModule;
use holon_turso::schema_modules::BlockDerivedSchemaModule;
use holon_turso::schema_modules::BlockHierarchySchemaModule;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockRequirementEdgesSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::HistorySchemaModule;
use holon_turso::schema_modules::IdentitySchemaModule;
use holon_turso::schema_modules::IntegrationStateSchemaModule;
use holon_turso::schema_modules::JournalDayPagesSchemaModule;
use holon_turso::schema_modules::JournalFeedSchemaModule;
use holon_turso::schema_modules::LinkSchemaModule;
use holon_turso::schema_modules::NavigationSchemaModule;
use holon_turso::schema_modules::OperationsSchemaModule;
use holon_turso::schema_modules::SyncStateSchemaModule;
use holon_turso::schema_modules::TrustProposalsSchemaModule;
use holon_turso::table_shape::TableAdapted;
use holon_turso::table_shape::TableChange;
use holon_turso::turso_adapter::TursoAdapter;
use holon_turso::turso_adapter::TypeRegistration;

use super::DbHandleProvider;
use crate::storage::turso::DbHandle;

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

/// `block_raw`, `directory`, `file` tables.
pub struct CoreTables;
impl DbResource for CoreTables {}

/// The `block` matview hydrating `tags` / `requires` from junction tables
/// (depends on `block_raw` + `block_tags` + `block_requires`).
pub struct BlockMatviewView;
impl DbResource for BlockMatviewView {}

/// `block_with_path` materialized view (depends on the `block` matview).
pub struct BlockHierarchyView;
impl DbResource for BlockHierarchyView {}

/// `block_requirement_edges` matview (chained on `block` matview).
pub struct BlockRequirementEdgesView;
impl DbResource for BlockRequirementEdgesView {}

/// `navigation_history`, `navigation_cursor`, `current_focus`, etc.
pub struct NavigationTables;
impl DbResource for NavigationTables {}

/// `sync_states` table.
pub struct SyncStateTables;
impl DbResource for SyncStateTables {}

/// `integration_state` table — the queryable mirror of the integration
/// enablement store.
pub struct IntegrationStateTables;
impl DbResource for IntegrationStateTables {}

/// `operation` table for undo/redo.
pub struct OperationTables;
impl DbResource for OperationTables {}

/// `block_history` — the C2b op/effect history relation (disclosed ephemeral
/// cache; ADR 0024 P8).
pub struct HistoryTables;
impl DbResource for HistoryTables {}

/// `automations_journal` matview — effects grouped by
/// `(origin, transition_id, day)` over `block_history` (ADR 0024 P8).
pub struct AutomationsJournalView;
impl DbResource for AutomationsJournalView {}

/// `block_requires`, `block_tags` junction tables (FK to `block_raw`).
pub struct BlockTables;
impl DbResource for BlockTables {}

/// `journal_day_pages` matview — journal day-page detection, chained on the
/// `block` matview + `block_tags` junction (journal-feed chain, stage 1).
pub struct JournalDayPagesView;
impl DbResource for JournalDayPagesView {}

/// `journal_feed` matview — the journal feed, chained on `journal_day_pages`
/// (journal-feed chain, stage 2).
pub struct JournalFeedView;
impl DbResource for JournalFeedView {}

/// `block_link` table (depends on `block`).
pub struct LinkTables;
impl DbResource for LinkTables {}

/// `canonical_entity`, `entity_alias`, `proposal_queue` tables for cross-system
/// identity.
pub struct IdentityTables;
impl DbResource for IdentityTables {}

/// `trust_proposals` supervision matview (C5 trust gate; FROM `block_raw`).
pub struct TrustProposalsView;
impl DbResource for TrustProposalsView {}

/// The Turso serialization (`<name>_raw` + `<name>` matview) of every
/// free-standing type in the `TypeRegistry` — derived generically by
/// `TursoAdapter`, with nothing hand-written per type.
pub struct FreeStandingTypeViews;
impl DbResource for FreeStandingTypeViews {}

/// `block_derived` — the C4 derived-field SIDECAR table (narrow
/// `(block_id, field_name)` cache; maintained reactively by the derived-field
/// CDC watcher, not by boot DDL beyond table creation).
pub struct BlockDerivedTable;
impl DbResource for BlockDerivedTable {}

// ---------------------------------------------------------------------------
// Helper: run a SchemaModule's DDL via DbHandle
// ---------------------------------------------------------------------------

use crate::storage::schema_module::SchemaModule;

#[tracing::instrument(skip(module, db_handle), name = "di.schema_module", fields(name = module.name()))]
async fn run_schema_module(
    module: &dyn SchemaModule,
    db_handle: &DbHandle,
) -> anyhow::Result<Vec<TableChange>> {
    let changes = module
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
    Ok(changes)
}

// ---------------------------------------------------------------------------
// Provider registration
// ---------------------------------------------------------------------------

/// Register all core schema providers on the injector.
///
/// After calling this, `injector.resolve_all_eager().await` will create all
/// tables/views in the correct order with maximum parallelism.
pub fn register_schema_providers(injector: &Injector) {
    // -- CoreTables (no deps) --
    injector.provide::<DbReady<CoreTables>>(Provider::root_async(|inj| async move {
        run_or_disclose(&inj, &CoreSchemaModule).await;
        Shared::new(DbReady::<CoreTables>::new())
    }));

    // -- BlockMatviewView (depends on CoreTables + BlockTables: matview JOINs
    // block_raw + junctions) --
    injector.provide::<DbReady<BlockMatviewView>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            let _bt = inj.resolve_async::<DbReady<BlockTables>>().await;
            run_or_disclose(&inj, &BlockMatviewSchemaModule).await;
            Shared::new(DbReady::<BlockMatviewView>::new())
        })
        .with_dependency::<DbReady<CoreTables>>()
        .with_dependency::<DbReady<BlockTables>>(),
    );

    // -- BlockHierarchyView (block_with_path: FROM block — chained on the matview)
    // --
    injector.provide::<DbReady<BlockHierarchyView>>(
        Provider::root_async(|inj| async move {
            let _bm = inj.resolve_async::<DbReady<BlockMatviewView>>().await;
            run_or_disclose(&inj, &BlockHierarchySchemaModule).await;
            Shared::new(DbReady::<BlockHierarchyView>::new())
        })
        .with_dependency::<DbReady<BlockMatviewView>>(),
    );

    // -- BlockRequirementEdgesView (chained on block matview + junctions) --
    injector.provide::<DbReady<BlockRequirementEdgesView>>(
        Provider::root_async(|inj| async move {
            let _bm = inj.resolve_async::<DbReady<BlockMatviewView>>().await;
            let _bt = inj.resolve_async::<DbReady<BlockTables>>().await;
            run_or_disclose(&inj, &BlockRequirementEdgesSchemaModule).await;
            Shared::new(DbReady::<BlockRequirementEdgesView>::new())
        })
        .with_dependency::<DbReady<BlockMatviewView>>()
        .with_dependency::<DbReady<BlockTables>>(),
    );

    // -- NavigationTables (focus_roots matview JOINs block — chained on the
    // matview) --
    injector.provide::<DbReady<NavigationTables>>(
        Provider::root_async(|inj| async move {
            let _bm = inj.resolve_async::<DbReady<BlockMatviewView>>().await;
            run_or_disclose(&inj, &NavigationSchemaModule).await;
            Shared::new(DbReady::<NavigationTables>::new())
        })
        .with_dependency::<DbReady<BlockMatviewView>>(),
    );

    // -- SyncStateTables (no deps) --
    injector.provide::<DbReady<SyncStateTables>>(Provider::root_async(|inj| async move {
        run_or_disclose(&inj, &SyncStateSchemaModule).await;
        Shared::new(DbReady::<SyncStateTables>::new())
    }));

    // -- IntegrationStateTables (no deps) --
    injector.provide::<DbReady<IntegrationStateTables>>(Provider::root_async(|inj| async move {
        run_or_disclose(&inj, &IntegrationStateSchemaModule).await;
        Shared::new(DbReady::<IntegrationStateTables>::new())
    }));

    // -- OperationTables (no deps) --
    injector.provide::<DbReady<OperationTables>>(Provider::root_async(|inj| async move {
        run_or_disclose(&inj, &OperationsSchemaModule).await;
        Shared::new(DbReady::<OperationTables>::new())
    }));

    // -- BlockDerivedTable (no DDL deps): the C4 derived-field sidecar table.
    // Table creation is dependency-free; the CDC watcher that populates it
    // binds to the block matview at runtime, not at DDL time. --
    injector.provide::<DbReady<BlockDerivedTable>>(Provider::root_async(|inj| async move {
        run_or_disclose(&inj, &BlockDerivedSchemaModule).await;
        Shared::new(DbReady::<BlockDerivedTable>::new())
    }));

    // -- HistoryTables (no deps): the C2b block_history relation, boot-owned
    // here so it is queryable (PRQL/raw SQL/list_tables) from session start —
    // never lazily created by its accessor --
    injector.provide::<DbReady<HistoryTables>>(Provider::root_async(|inj| async move {
        run_or_disclose(&inj, &HistorySchemaModule).await;
        Shared::new(DbReady::<HistoryTables>::new())
    }));

    // -- HistoryStore (C2b): the typed `dyn HistoryStore` port over the boot-
    // owned `block_history` table, so non-engine writers (the org-ingest
    // doc-page create in `FileSyncController`) can record provenance through the
    // same store the engine uses. Depends on HistoryTables so the relation
    // exists before any record. --
    injector.provide::<dyn holon_api::HistoryStore>(
        Provider::root_async(|inj| async move {
            let _hist = inj.resolve_async::<DbReady<HistoryTables>>().await;
            let db = inj.resolve::<dyn DbHandleProvider>();
            std::sync::Arc::new(crate::api::TursoHistoryStore::new(db.handle()))
                as std::sync::Arc<dyn holon_api::HistoryStore>
        })
        .with_dependency::<DbReady<HistoryTables>>(),
    );

    // -- AutomationsJournalView (matview grouped over block_history — depends on
    // HistoryTables only) --
    injector.provide::<DbReady<AutomationsJournalView>>(
        Provider::root_async(|inj| async move {
            let _hist = inj.resolve_async::<DbReady<HistoryTables>>().await;
            run_or_disclose(&inj, &AutomationsJournalSchemaModule).await;
            Shared::new(DbReady::<AutomationsJournalView>::new())
        })
        .with_dependency::<DbReady<HistoryTables>>(),
    );

    // -- JournalDayPagesView (journal-feed chain stage 1: `block` matview JOIN
    // `block_tags` — depends on BlockMatviewView + BlockTables) --
    injector.provide::<DbReady<JournalDayPagesView>>(
        Provider::root_async(|inj| async move {
            let _bm = inj.resolve_async::<DbReady<BlockMatviewView>>().await;
            let _bt = inj.resolve_async::<DbReady<BlockTables>>().await;
            run_or_disclose(&inj, &JournalDayPagesSchemaModule).await;
            Shared::new(DbReady::<JournalDayPagesView>::new())
        })
        .with_dependency::<DbReady<BlockMatviewView>>()
        .with_dependency::<DbReady<BlockTables>>(),
    );

    // -- JournalFeedView (journal-feed chain stage 2: matview chained on
    // `journal_day_pages` — depends on JournalDayPagesView) --
    injector.provide::<DbReady<JournalFeedView>>(
        Provider::root_async(|inj| async move {
            let _jdp = inj.resolve_async::<DbReady<JournalDayPagesView>>().await;
            run_or_disclose(&inj, &JournalFeedSchemaModule).await;
            Shared::new(DbReady::<JournalFeedView>::new())
        })
        .with_dependency::<DbReady<JournalDayPagesView>>(),
    );

    // -- BlockTables (depends on CoreTables: junction FKs reference block_raw.id)
    // --
    injector.provide::<DbReady<BlockTables>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            run_or_disclose(&inj, &BlockSchemaModule).await;
            Shared::new(DbReady::<BlockTables>::new())
        })
        .with_dependency::<DbReady<CoreTables>>(),
    );

    // -- LinkTables (depends on CoreTables) --
    injector.provide::<DbReady<LinkTables>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            run_or_disclose(&inj, &LinkSchemaModule).await;
            Shared::new(DbReady::<LinkTables>::new())
        })
        .with_dependency::<DbReady<CoreTables>>(),
    );

    // -- IdentityTables (no deps; tables are independent of block) --
    injector.provide::<DbReady<IdentityTables>>(Provider::root_async(|inj| async move {
        run_or_disclose(&inj, &IdentitySchemaModule).await;
        Shared::new(DbReady::<IdentityTables>::new())
    }));

    // -- TrustProposalsView (FROM block_raw — depends on CoreTables only) --
    injector.provide::<DbReady<TrustProposalsView>>(
        Provider::root_async(|inj| async move {
            let _core = inj.resolve_async::<DbReady<CoreTables>>().await;
            run_or_disclose(&inj, &TrustProposalsSchemaModule).await;
            Shared::new(DbReady::<TrustProposalsView>::new())
        })
        .with_dependency::<DbReady<CoreTables>>(),
    );

    injector.provide::<UnservedTypes>(Provider::root(|_| Shared::new(UnservedTypes::default())));

    // -- FreeStandingTypeViews: every free-standing type's own raw table +
    // read matview, derived from its TypeDefinition by TursoAdapter. No
    // dependency on the block chain — a free-standing type references nothing.
    injector.provide::<DbReady<FreeStandingTypeViews>>(Provider::root_async(|inj| async move {
        let type_registry = inj.resolve::<holon_profiles::TypeRegistry>();
        let db = inj.resolve::<dyn DbHandleProvider>();
        let bus = inj.resolve::<Arc<ConditionBus>>();
        let unserved = inj.resolve::<UnservedTypes>();
        for type_def in type_registry.all() {
            if !is_free_standing(&type_def) {
                continue;
            }
            let name = type_def.name.to_string();
            match TursoAdapter::reconcile(&type_def, &db.handle()).await {
                Ok(TypeRegistration::Registered { adapted, .. }) => {
                    if let Some(adapted) = adapted {
                        bus.emit(columns_added(name, adapted));
                    }
                }
                Ok(TypeRegistration::Refused(refusal)) => {
                    unserved.insert(
                        &name,
                        &refusal.diff.table,
                        format!("{}: {refusal}", ConditionKind::TYPE_TABLE_REFUSED),
                    );
                    // A view an older binary stored over the refused table
                    // would keep serving its undeclared shape.
                    if let Err(e) =
                        holon_turso::table_shape::drop_views_over(&db.handle(), &refusal.diff.table)
                            .await
                    {
                        tracing::error!("type '{name}': {e:#}");
                        bus.emit(Condition {
                            subject: format!("type {name}"),
                            reason: ConditionKind::SchemaModuleFailed {
                                error: format!("{e:#}"),
                            },
                        });
                    }
                    bus.emit(Condition {
                        subject: name,
                        reason: ConditionKind::TypeTableRefused {
                            table: refusal.diff.table.clone(),
                            diff: refusal.diff.to_string(),
                            rows: refusal.rows,
                        },
                    });
                }
                Err(e) => {
                    tracing::error!("registering type '{name}' failed: {e:#}");
                    unserved.insert(
                        &name,
                        &TursoAdapter::raw_table_name(&type_def),
                        format!("{}: {e:#}", ConditionKind::SCHEMA_MODULE_FAILED),
                    );
                    bus.emit(Condition {
                        subject: format!("type {name}"),
                        reason: ConditionKind::SchemaModuleFailed {
                            error: format!("{e:#}"),
                        },
                    });
                }
            }
        }
        Shared::new(DbReady::<FreeStandingTypeViews>::new())
    }));
}

/// The free-standing types this session does not serve, with why: their
/// stored table could not be brought to their declaration. Their reads and
/// writes fail with that reason until it is remedied.
#[derive(Clone, Default)]
pub struct UnservedTypes(Arc<Mutex<BTreeMap<String, Unserved>>>);

#[derive(Clone)]
struct Unserved {
    raw_table: String,
    reason: String,
}

impl UnservedTypes {
    pub fn insert(&self, type_name: &str, raw_table: &str, reason: String) {
        self.0.lock().expect("UnservedTypes lock").insert(
            type_name.to_string(),
            Unserved {
                raw_table: raw_table.to_string(),
                reason,
            },
        );
    }

    pub fn remove(&self, type_name: &str) {
        self.0.lock().expect("UnservedTypes lock").remove(type_name);
    }

    /// Each unserved type with its reason.
    pub fn entries(&self) -> BTreeMap<String, String> {
        self.0
            .lock()
            .expect("UnservedTypes lock")
            .iter()
            .map(|(name, unserved)| (name.clone(), unserved.reason.clone()))
            .collect()
    }

    /// Refuse `sql` when it reads an unserved type, by its name or its raw
    /// table.
    pub fn check_read(&self, sql: &str) -> anyhow::Result<()> {
        let unserved = self.0.lock().expect("UnservedTypes lock").clone();
        if unserved.is_empty() {
            return Ok(());
        }
        let statements = holon_turso::sql_parser::parse_sql(sql).map_err(|e| {
            anyhow::anyhow!(
                "cannot tell whether this read touches an unserved type ({}): {e}. SQL: {sql}",
                unserved.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;
        let read = holon_turso::sql_parser::extract_table_refs(&statements);
        for (name, Unserved { raw_table, reason }) in &unserved {
            if read.iter().any(|r| {
                r.name().eq_ignore_ascii_case(name) || r.name().eq_ignore_ascii_case(raw_table)
            }) {
                anyhow::bail!("type '{name}' is not served — {reason}");
            }
        }
        Ok(())
    }
}

fn columns_added(subject: String, adapted: TableAdapted) -> Condition {
    Condition {
        subject,
        reason: ConditionKind::TableColumnsAdded {
            table: adapted.table,
            added: adapted.added,
            undeclared: adapted.undeclared,
            rows: adapted.rows,
        },
    }
}

/// Run `module` and disclose on the condition bus what it changed in stored
/// tables, or why it failed. A failed module leaves only its own tables
/// unserved: boot goes on, and whatever needs them fails naming the error.
async fn run_or_disclose(inj: &Injector, module: &dyn SchemaModule) {
    let db = inj.resolve::<dyn DbHandleProvider>();
    let bus = inj.resolve::<Arc<ConditionBus>>();
    match run_schema_module(module, &db.handle()).await {
        Ok(changes) => {
            for change in changes {
                bus.emit(match change {
                    TableChange::ColumnsAdded(adapted) => {
                        columns_added(adapted.table.clone(), adapted)
                    }
                    TableChange::Rebuilt(rebuilt) => Condition {
                        subject: rebuilt.diff.table.clone(),
                        reason: ConditionKind::TableRebuilt {
                            diff: rebuilt.diff.to_string(),
                            rows: rebuilt.rows,
                        },
                    },
                });
            }
        }
        Err(e) => {
            tracing::error!("schema module {} failed: {e:#}", module.name());
            bus.emit(Condition {
                subject: module.name().to_string(),
                reason: ConditionKind::SchemaModuleFailed {
                    error: format!("{e:#}"),
                },
            });
        }
    }
}

/// A type is free-standing when its id references nothing and it has persisted
/// fields of its own. Persisted, not merely declared: a type carrying only
/// computed fields stores nothing, so it has no write table to derive.
///
/// `block` is excluded by name because its Turso serialization is still
/// hand-written (`CoreSchemaModule` + `BlockMatviewSchemaModule`); the literal
/// dissolves once block itself becomes an adapter instance.
pub(crate) fn is_free_standing(type_def: &holon_api::TypeDefinition) -> bool {
    type_def.id_references.is_none()
        && !type_def.persistent_fields().is_empty()
        && type_def.name != "block"
}

/// All core schema TypeIds. Single source of truth for `resolve_eager_roots`
/// and `with_dependency` declarations.
pub fn all_schema_roots() -> Vec<std::any::TypeId> {
    use std::any::TypeId;
    vec![
        TypeId::of::<DbReady<CoreTables>>(),
        TypeId::of::<DbReady<BlockTables>>(),
        TypeId::of::<DbReady<BlockMatviewView>>(),
        TypeId::of::<DbReady<BlockHierarchyView>>(),
        TypeId::of::<DbReady<BlockRequirementEdgesView>>(),
        TypeId::of::<DbReady<NavigationTables>>(),
        TypeId::of::<DbReady<SyncStateTables>>(),
        TypeId::of::<DbReady<IntegrationStateTables>>(),
        TypeId::of::<DbReady<OperationTables>>(),
        TypeId::of::<DbReady<LinkTables>>(),
        TypeId::of::<DbReady<TrustProposalsView>>(),
        TypeId::of::<DbReady<AutomationsJournalView>>(),
        TypeId::of::<DbReady<JournalDayPagesView>>(),
        TypeId::of::<DbReady<JournalFeedView>>(),
        TypeId::of::<DbReady<BlockDerivedTable>>(),
        TypeId::of::<DbReady<FreeStandingTypeViews>>(),
    ]
}
