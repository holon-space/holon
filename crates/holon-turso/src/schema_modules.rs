//! Concrete schema module implementations for core database objects.
//!
//! This module provides `SchemaModule` implementations for the core database
//! schema objects in Holon:
//!
//! - `CoreSchemaModule`: block_raw (the underlying block table), files, clock
//! - `BlockSchemaModule`: block_requires, block_tags junction tables (FK to
//!   block_raw)
//! - `BlockMatviewSchemaModule`: the `block` matview hydrating tags + requires
//! - `BlockRequirementEdgesSchemaModule`: the block_requirement_edges matview
//!   (chained on block)
//! - `BlockHierarchySchemaModule`: block_with_path materialized view
//! - `NavigationSchemaModule`: navigation_history, navigation_cursor,
//!   current_focus
//! - `SyncStateSchemaModule`: sync_states table
//! - `OperationsSchemaModule`: operations table for undo/redo
//! - `IdentitySchemaModule`: canonical_entity, entity_alias, proposal_queue
//!   tables

use std::collections::HashMap;

use async_trait::async_trait;
use holon_api::computation::PlantedColumn;
use holon_core::storage::resource::Resource;
use holon_core::storage::types::Result;
use holon_core::storage::types::StorageError;

use crate::matview_manager::reconcile_named_view;
use crate::schema_module::EdgeFieldDescriptor;
use crate::schema_module::SchemaModule;
use crate::sql_utils::sql_statements;
use crate::turso::DbHandle;

/// The canonical `block_raw` DDL (table + index).
///
/// Exposed so a test fixture standing up a PARTIAL schema binds the production
/// column set instead of hand-listing one. A hand-rolled narrow `block_raw` is
/// silently accepted by `CREATE MATERIALIZED VIEW` and only fails at query
/// time, with a misleading "incompatible DBSP version" parse error.
pub fn block_raw_schema_sql() -> &'static str {
    include_str!("../sql/schema/blocks.sql")
}

/// Core schema module providing the fundamental tables: block_raw, files, and
/// the `clock` relation.
///
/// This module has no dependencies and should be initialized first.
pub struct CoreSchemaModule;

#[async_trait]
impl SchemaModule for CoreSchemaModule {
    fn name(&self) -> &str {
        "core"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![
            Resource::schema("block_raw"),
            Resource::schema("file"),
            Resource::schema("clock"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![] // No dependencies - this is the root
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[CoreSchemaModule] Creating core tables");

        for stmt in sql_statements(block_raw_schema_sql()) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[CoreSchemaModule] block_raw table + index created");

        // Seed the self-parented `sentinel:no_parent` row so root blocks
        // (parent_id = 'sentinel:no_parent') satisfy the block_raw parent FK.
        // Self-reference is legal because the FK is DEFERRABLE INITIALLY
        // DEFERRED (checked at COMMIT, when the row itself is present).
        // Idempotent: re-running bootstrap must not duplicate or error.
        db_handle
            .execute(
                // Schema bootstrap seeding the FK anchor row, not a block
                // mutation — writes only the sentinel, never a real block.
                // ALLOW(sole_block_writer): sentinel-only FK anchor seed.
                "INSERT OR IGNORE INTO block_raw (id, parent_id) VALUES ('sentinel:no_parent', \
                 'sentinel:no_parent')",
                vec![],
            )
            .await?;
        tracing::debug!("[CoreSchemaModule] sentinel:no_parent row seeded");

        for stmt in sql_statements(include_str!("../sql/schema/files.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[CoreSchemaModule] files table + indexes created");

        // `clock` relation (ADR 0024 P5, time-as-data). Seed a deterministic
        // placeholder row so the boot guard always finds a `day` grain; the
        // `ClockScheduler`'s first tick replaces it with the real local date via
        // a CDC-emitting UPDATE before any temporal-guard matview is created.
        for stmt in sql_statements(include_str!("../sql/schema/clock.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        db_handle
            .execute(
                "INSERT OR IGNORE INTO clock (grain, today, epoch_day, updated_at) VALUES ('day', \
                 '1970-01-01', 0, '1970-01-01T00:00:00Z')",
                vec![],
            )
            .await?;
        tracing::debug!("[CoreSchemaModule] clock table created + day row seeded");

        tracing::info!("[CoreSchemaModule] Core tables created successfully");
        Ok(())
    }
}

/// Migrate one junction table off the pre-2026-07-22 shape that carried a
/// FOREIGN KEY on its TARGET column, PRESERVING its rows.
///
/// The target FK is the data-loss defect that change removed: a `:REQUIRES:` /
/// suppression edge legitimately points at a block parsed later in the same
/// scan, in another file, or never — and a FK on it fails the source block's
/// create transaction at COMMIT, aborting the WHOLE file ingest (dogfood
/// 2026-07-10). A database still carrying it keeps hitting that.
///
/// Detection sniffs the stored DDL, the same way `HistorySchemaModule` does;
/// there is no schema-version table in this crate to consult. `HistorySchema`
/// can simply drop what it finds because its relation is a disclosed ephemeral
/// cache — these junctions are durable projected state, so the old rows are
/// copied across instead.
///
/// A no-op on a current-shape database (one `sqlite_master` read).
async fn migrate_junction_dropping_target_fk(
    db_handle: &DbHandle,
    table: &str,
    target_fk_column: &str,
    index_name: &str,
    columns: &str,
) -> Result<()> {
    let stored = db_handle
        .query_positional(
            &format!("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = '{table}'"),
            vec![],
        )
        .await?;
    let old_shape = match stored.first() {
        None => false, // Absent: the CREATE below makes it in the current shape.
        Some(row) => {
            let ddl = match row.get("sql") {
                Some(holon_api::Value::String(s)) => s.clone(),
                other => {
                    return Err(StorageError::SchemaError(format!(
                        "sqlite_master.sql for {table}: expected TEXT, got {other:?}"
                    )));
                }
            };
            // The current shape names the target column only inside prose
            // comments explaining why it is NOT constrained; the
            // `FOREIGN KEY (<col>)` clause itself appears in the old shape alone.
            ddl.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains(&format!("FOREIGN KEY ({target_fk_column})"))
        }
    };

    // The copy below is not transactional, so a crash between the rename and
    // the copy leaves the staging table holding the ONLY surviving rows while
    // the main table already reads current-shape. Resuming on that state is the
    // difference between a retryable migration and one that silently strands
    // every row on the next boot.
    let staging = format!("{table}__pre_target_fk");
    let staging_present = !db_handle
        .query_positional(
            &format!("SELECT name FROM sqlite_master WHERE type = 'table' AND name = '{staging}'"),
            vec![],
        )
        .await?
        .is_empty();

    if !old_shape && !staging_present {
        return Ok(());
    }
    if old_shape && staging_present {
        return Err(StorageError::SchemaError(format!(
            "{table}: both the OLD-shape table and the staging table `{staging}` exist. That \
             combination is not reachable by this migration (the staging table only appears once \
             the old shape has been renamed away), so the rows may be split across the two and \
             this refuses to guess which is authoritative."
        )));
    }

    let create_sql = match table {
        "block_requires" => include_str!("../sql/schema/block_requires.sql"),
        "advice_suppressed" => include_str!("../sql/schema/advice_suppressed.sql"),
        other => {
            return Err(StorageError::SchemaError(format!(
                "migrate_junction_dropping_target_fk: no schema file bound for `{other}`"
            )));
        }
    };

    if old_shape {
        tracing::warn!(
            "[BlockSchemaModule] MIGRATING `{table}`: it carries the pre-2026-07-22 FOREIGN KEY \
             on `{target_fk_column}`, which aborts a whole file ingest when that target is a \
             forward or cross-file reference. Rebuilding the table without it and copying every \
             row across."
        );
        // A table carrying dependent matviews cannot be renamed — Turso rejects
        // the ALTER outright. Every deployed database has `block_requires_agg`
        // (and its chain) persisted, because `reconcile_named_view` early-returns
        // on an unchanged SELECT and so never recreates them. Clearing them here
        // is safe: their owning schema modules run AFTER this one and rebuild
        // them on the same boot, and dynamic watch views are recreated on watch
        // registration.
        crate::matview_manager::drop_dependent_views(db_handle, table)
            .await
            .map_err(|e| {
                StorageError::SchemaError(format!(
                    "{table}: clearing dependent matviews before the shape migration failed: {e:#}"
                ))
            })?;
        db_handle
            .execute_ddl(&format!("ALTER TABLE {table} RENAME TO {staging}"))
            .await?;
        // The index followed the table through the RENAME and still holds its
        // old name, so the current-shape `CREATE INDEX IF NOT EXISTS` would find
        // the name taken and silently leave the new table unindexed.
        db_handle
            .execute_ddl(&format!("DROP INDEX IF EXISTS {index_name}"))
            .await?;
    } else {
        tracing::warn!(
            "[BlockSchemaModule] RESUMING an interrupted `{table}` migration: staging table \
             `{staging}` is still present, so a previous attempt did not finish copying its rows."
        );
    }

    for stmt in sql_statements(create_sql) {
        db_handle.execute_ddl(stmt).await?;
    }

    // `OR IGNORE` so a resumed run tolerates the rows the interrupted attempt
    // already copied; the primary key makes the copy idempotent.
    db_handle
        .execute(
            &format!("INSERT OR IGNORE INTO {table} ({columns}) SELECT {columns} FROM {staging}"),
            vec![],
        )
        .await?;
    let count = db_handle
        .query_positional(&format!("SELECT COUNT(*) AS c FROM {table}"), vec![])
        .await?
        .first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            StorageError::SchemaError(format!("{table}: post-migration COUNT(*) returned no row"))
        })?;
    let staged = db_handle
        .query_positional(&format!("SELECT COUNT(*) AS c FROM {staging}"), vec![])
        .await?
        .first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| StorageError::SchemaError(format!("{staging}: COUNT(*) returned no row")))?;
    // Drop the only other copy of these rows solely on proof they all arrived.
    if count < staged {
        return Err(StorageError::SchemaError(format!(
            "{table}: migration copied {count} row(s) but staging `{staging}` holds {staged}; \
             refusing to drop staging while rows are unaccounted for"
        )));
    }
    db_handle
        .execute_ddl(&format!("DROP TABLE {staging}"))
        .await?;

    tracing::warn!("[BlockSchemaModule] `{table}` migrated; {count} row(s) preserved");
    Ok(())
}

/// Block junction-table schema module.
///
/// Owns `block_requires` and `block_tags` (the production junction tables).
/// FKs target `block_raw`. The chained matviews that depend on these
/// junctions (`block` matview, `block_requirement_edges`) are owned by
/// `BlockMatviewSchemaModule` and `BlockRequirementEdgesSchemaModule`
/// respectively to keep the dependency graph acyclic.
pub struct BlockSchemaModule;

#[async_trait]
impl SchemaModule for BlockSchemaModule {
    fn name(&self) -> &str {
        "block_junction"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![
            Resource::schema("block_requires"),
            Resource::schema("block_tags"),
            Resource::schema("advice_suppressed"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block_raw")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[BlockSchemaModule] Migrating junction tables");

        // `task_blockers` is the pre-rename name of `block_requires`; dropping
        // it (rather than ALTER RENAME) is correct because the matviews and
        // edge-field descriptors all reference `block_requires` now and
        // existing rows would no longer be reachable through the renamed
        // schema.
        //
        // Only the LEGACY name is dropped. The junctions below hold projected
        // state that persists across boots exactly as `block_raw` does, and
        // `ensure_schema` runs on every boot — dropping them here empties the
        // vault's tags and requirement edges on each start. A future shape
        // change to one of them is a versioned migration, not a standing wipe.
        db_handle
            .execute_ddl("DROP TABLE IF EXISTS task_blockers")
            .await?;

        // Databases created before 2026-07-22 still carry a FOREIGN KEY on the
        // junction TARGET column. Until now the unconditional drop above
        // reshaped them by destroying them; with the drop gone this is the only
        // path that reshapes them, so it must migrate rather than assume.
        migrate_junction_dropping_target_fk(
            db_handle,
            "block_requires",
            "required_id",
            "idx_block_requires_required",
            "block_id, required_id",
        )
        .await?;
        migrate_junction_dropping_target_fk(
            db_handle,
            "advice_suppressed",
            "lesson_id",
            "idx_advice_suppressed_lesson",
            "anchor_id, lesson_id",
        )
        .await?;

        for stmt in sql_statements(include_str!("../sql/schema/block_requires.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[BlockSchemaModule] block_requires table created");

        for stmt in sql_statements(include_str!("../sql/schema/block_tags.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[BlockSchemaModule] block_tags table created");

        for stmt in sql_statements(include_str!("../sql/schema/advice_suppressed.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[BlockSchemaModule] advice_suppressed table created");

        tracing::info!("[BlockSchemaModule] Junction tables ready");
        Ok(())
    }

    fn edge_fields(&self) -> Vec<EdgeFieldDescriptor> {
        vec![
            EdgeFieldDescriptor {
                entity: "block".to_string(),
                field: "requires".to_string(),
                join_table: "block_requires".to_string(),
                source_col: "block_id".to_string(),
                target_col: "required_id".to_string(),
            },
            EdgeFieldDescriptor {
                entity: "block".to_string(),
                field: "tags".to_string(),
                join_table: "block_tags".to_string(),
                source_col: "block_id".to_string(),
                target_col: "tag".to_string(),
            },
            EdgeFieldDescriptor {
                entity: "block".to_string(),
                field: "advice_suppressed".to_string(),
                join_table: "advice_suppressed".to_string(),
                source_col: "anchor_id".to_string(),
                target_col: "lesson_id".to_string(),
            },
        ]
    }
}

/// All columns of `block_raw`, projected verbatim into the `block` matview so
/// downstream readers (block_with_path, block_requirement_edges, GQL/PRQL
/// watch_view_*) see the same row shape they always did.
const BLOCK_RAW_COLUMNS: &[&str] = &[
    "id",
    "parent_id",
    "sort_key",
    "content",
    "content_type",
    "source_language",
    "source_name",
    "properties",
    "marks",
    "collapsed",
    "widget_only",
    "completed",
    "block_type",
    "created_at",
    "updated_at",
    "_change_origin",
    "write_seq",
];

/// The `block` entity's edge-field descriptors — the single registry both the
/// junction DDL (`BlockSchemaModule::edge_fields`) and the matview synthesis
/// below derive from.
fn block_edge_fields() -> Vec<EdgeFieldDescriptor> {
    BlockSchemaModule
        .edge_fields()
        .into_iter()
        .filter(|d| d.entity == "block")
        .collect()
}

/// Name of the per-junction aggregation matview for one edge field.
fn edge_agg_view_name(descriptor: &EdgeFieldDescriptor) -> String {
    format!("{}_agg", descriptor.join_table)
}

/// SELECT for the per-junction aggregation matview: one row per source id with
/// the target values as a JSON array. Aggregating each junction SEPARATELY is
/// what prevents the fan-out (see `block_matview_select`).
fn edge_agg_view_select(descriptor: &EdgeFieldDescriptor) -> String {
    format!(
        "SELECT {src} AS source_id, json_group_array({tgt}) AS vals FROM {jt} GROUP BY {src}",
        src = descriptor.source_col,
        tgt = descriptor.target_col,
        jt = descriptor.join_table,
    )
}

/// SELECT for the `block` matview: `block_raw` LEFT JOINed against the
/// per-junction agg matviews (at most ONE row each per block), `'[]'` when a
/// block has no junction rows.
///
/// This chained shape replaces the previous single view that GROUP BYed over
/// the LEFT-JOIN cross-product of all junctions at once — plain-SQL semantics
/// fan out there: a block with 3 tags and 1 requires row yielded
/// `requires = ["R","R","R"]` (masked for `tags` because sets dedup at parse,
/// corrupting `requires`/`advice_suppressed`). Both the bug and this fix are
/// pinned under IVM by
/// `holon-advice/tests/matview_build.
/// rs::probe_multi_junction_fanout_fix_shapes`.
fn block_matview_select(descriptors: &[EdgeFieldDescriptor]) -> String {
    block_matview_select_with_computed(descriptors, &[])
}

/// SELECT for the `block` matview, extended with C4 **SQL-planted derived-field
/// columns** (seat A). Each [`PlantedColumn`] is appended as `{sql} AS {name}`
/// — an inlined, parameter-free scalar expression over `b.*` columns — so
/// Turso's IVM maintains the derived value O(delta) alongside the block row.
///
/// `computed` is empty on the boot path (prototype-block declarations are user
/// data loaded *after* schema init). The remaining production wire is: when a
/// prototype block's derived-field set changes, re-`plan` it and re-reconcile
/// the `block` matview with the resulting columns (`reconcile_named_view`
/// already DROP+CREATEs only on a SELECT change). See
/// docs/Proposals/ComputationTrait.
fn block_matview_select_with_computed(
    descriptors: &[EdgeFieldDescriptor],
    computed: &[PlantedColumn],
) -> String {
    let mut columns: Vec<String> = BLOCK_RAW_COLUMNS.iter().map(|c| format!("b.{c}")).collect();
    let mut joins = Vec::new();
    for d in descriptors {
        let agg = edge_agg_view_name(d);
        columns.push(format!("COALESCE({agg}.vals, '[]') AS {}", d.field));
        joins.push(format!("LEFT OUTER JOIN {agg} ON {agg}.source_id = b.id"));
    }
    for c in computed {
        columns.push(format!("({}) AS {}", c.sql, c.name));
    }
    // Exclude the self-parented `sentinel:no_parent` FK-anchor row — it exists
    // only to satisfy the block_raw parent FK and must never surface as a real
    // block in any projection reading through the `block` matview.
    format!(
        "SELECT {} FROM block_raw b {} WHERE b.id != 'sentinel:no_parent'",
        columns.join(", "),
        joins.join(" ")
    )
}

/// `block` matview schema module.
///
/// Hydrates `block_raw` rows with the edge-typed fields (`tags`, `requires`,
/// `advice_suppressed`) as JSON arrays. Every consumer that wants a hydrated
/// block row reads from this matview; raw structural reads/writes target
/// `block_raw`. The DDL is synthesized from the `EdgeFieldDescriptor` registry:
/// one aggregation matview per junction, then `block` chained on top of them
/// (matview-on-matview, same pattern as `block_requirement_edges`).
pub struct BlockMatviewSchemaModule;

#[async_trait]
impl SchemaModule for BlockMatviewSchemaModule {
    fn name(&self) -> &str {
        "block_matview"
    }

    fn provides(&self) -> Vec<Resource> {
        let mut provides: Vec<Resource> = block_edge_fields()
            .iter()
            .map(|d| Resource::schema(edge_agg_view_name(d)))
            .collect();
        provides.push(Resource::schema("block"));
        provides
    }

    fn requires(&self) -> Vec<Resource> {
        let mut requires = vec![Resource::schema("block_raw")];
        requires.extend(
            block_edge_fields()
                .iter()
                .map(|d| Resource::schema(d.join_table.clone())),
        );
        requires
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[BlockMatviewSchemaModule] Reconciling block matview chain");
        let descriptors = block_edge_fields();
        assert!(
            !descriptors.is_empty(),
            "block edge-field registry must not be empty"
        );
        // Dependency order: the per-junction agg matviews first, then the
        // `block` matview that chains on them.
        for d in &descriptors {
            let name = edge_agg_view_name(d);
            reconcile_named_view(db_handle, &name, &edge_agg_view_select(d))
                .await
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        }
        let created = reconcile_named_view(db_handle, "block", &block_matview_select(&descriptors))
            .await
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        if created {
            tracing::info!("[BlockMatviewSchemaModule] block matview created/updated");
        } else {
            tracing::info!("[BlockMatviewSchemaModule] block matview unchanged");
        }
        Ok(())
    }
}

/// `block_requirement_edges` matview schema module.
///
/// Chained matview: `JOIN block ON ...` — block here is the matview, so
/// block_requirement_edges is matview-on-matview. Verified safe by the
/// chain_join shape in the chained-matview preflight.
pub struct BlockRequirementEdgesSchemaModule;

#[async_trait]
impl SchemaModule for BlockRequirementEdgesSchemaModule {
    fn name(&self) -> &str {
        "block_requirement_edges"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("block_requirement_edges")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![
            Resource::schema("block"),
            Resource::schema("block_requires"),
        ]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!(
            "[BlockRequirementEdgesSchemaModule] Reconciling block_requirement_edges matview"
        );
        reconcile_named_view(
            db_handle,
            "block_requirement_edges",
            include_str!("../sql/schema/block_requirement_edges_matview.sql"),
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        tracing::debug!(
            "[BlockRequirementEdgesSchemaModule] block_requirement_edges matview reconciled"
        );
        Ok(())
    }
}

/// Supervision view for the C5 trust gate: one row per proposal block
/// (coerced sub-threshold emission under `block:proposals`), keyed by the
/// proposer's provenance. IVM maintains it from `block_raw` CDC, so
/// "proposals by agent/rule with acceptance stats" is ONE query away — see
/// [`TRUST_PROPOSAL_STATS_SQL`] for the aggregation.
pub struct TrustProposalsSchemaModule;

/// Acceptance stats per proposer over the `trust_proposals` matview — the C2
/// "supervision = one query" payoff. Kept as a plain aggregate query (not a
/// second matview) so no aggregating-matview IVM constraint applies.
pub const TRUST_PROPOSAL_STATS_SQL: &str = "SELECT origin, transition_id, session_id, COUNT(*) AS proposals, SUM(CASE WHEN status = \
     'accepted' THEN 1 ELSE 0 END) AS accepted, SUM(CASE WHEN status = 'rejected' THEN 1 ELSE 0 \
     END) AS rejected, SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END) AS pending FROM \
     trust_proposals GROUP BY origin, transition_id, session_id";

#[async_trait]
impl SchemaModule for TrustProposalsSchemaModule {
    fn name(&self) -> &str {
        "trust_proposals"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("trust_proposals")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block_raw")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[TrustProposalsSchemaModule] Reconciling trust_proposals matview");
        reconcile_named_view(
            db_handle,
            "trust_proposals",
            include_str!("../sql/schema/trust_proposals_matview.sql"),
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}

/// Block hierarchy schema module providing the block_with_path materialized
/// view.
///
/// This view computes hierarchical paths using a recursive CTE, enabling
/// efficient ancestor/descendant queries via path prefix matching.
pub struct BlockHierarchySchemaModule;

#[async_trait]
impl SchemaModule for BlockHierarchySchemaModule {
    fn name(&self) -> &str {
        "block_hierarchy"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("block_with_path")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[BlockHierarchySchemaModule] Reconciling block_with_path view");
        let created = reconcile_named_view(
            db_handle,
            "block_with_path",
            include_str!("../sql/schema/blocks_with_paths.sql"),
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        if created {
            tracing::info!("[BlockHierarchySchemaModule] block_with_path view created/updated");
        } else {
            tracing::info!("[BlockHierarchySchemaModule] block_with_path view unchanged");
        }
        Ok(())
    }
}

/// Navigation schema module providing tables for navigation state persistence.
///
/// Provides:
/// - navigation_history: Back/forward history
/// - navigation_cursor: Current position in history per region
/// - current_focus: Materialized view for efficient focus lookups
pub struct NavigationSchemaModule;

#[async_trait]
impl SchemaModule for NavigationSchemaModule {
    fn name(&self) -> &str {
        "navigation"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![
            Resource::schema("navigation_history"),
            Resource::schema("navigation_cursor"),
            Resource::schema("current_focus"),
            Resource::schema("focus_roots"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        // focus_roots matview JOINs the block table
        vec![Resource::schema("block")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[NavigationSchemaModule] Creating navigation tables");

        for stmt in sql_statements(include_str!("../sql/schema/navigation.sql")) {
            match db_handle.execute_ddl(stmt).await {
                Ok(()) => {}
                Err(e) if e.to_string().contains("already exists") => {
                    tracing::debug!(
                        "[NavigationSchemaModule] Skipping (already exists): {}",
                        &stmt[..stmt.len().min(60)]
                    );
                }
                Err(e) => return Err(e),
            }
        }

        tracing::info!("[NavigationSchemaModule] Reconciling navigation matviews");
        let views: &[(&str, &str)] = &[
            (
                "current_focus",
                include_str!("../sql/schema/matview_current_focus.sql"),
            ),
            (
                "focus_roots",
                include_str!("../sql/schema/matview_focus_roots.sql"),
            ),
        ];
        for (name, select_sql) in views {
            reconcile_named_view(db_handle, name, select_sql)
                .await
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        }

        tracing::info!("[NavigationSchemaModule] Navigation schema ready");
        Ok(())
    }

    async fn initialize_data(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[NavigationSchemaModule] Initializing default regions");

        for region in holon_api::Region::ALL {
            let mut params = HashMap::new();
            params.insert("region".to_string(), holon_api::Value::from(*region));

            db_handle
                .query(
                    include_str!("../sql/navigation/init_default_region.sql"),
                    params,
                )
                .await?;
        }

        tracing::info!("[NavigationSchemaModule] Default regions initialized");
        Ok(())
    }

    fn graph_contributions(
        &self,
    ) -> (
        Vec<holon_api::entity::GraphNodeDef>,
        Vec<holon_api::entity::GraphEdgeDef>,
    ) {
        use holon_api::entity::GraphEdgeDef;
        use holon_api::entity::GraphNodeDef;

        let nodes = vec![
            GraphNodeDef {
                label: "current_focus".into(),
                table_name: "current_focus".into(),
                id_column: "region".into(),
                columns: vec![
                    ("region".into(), "region".into()),
                    ("block_id".into(), "block_id".into()),
                    ("timestamp".into(), "timestamp".into()),
                ],
            },
            GraphNodeDef {
                label: "focus_root".into(),
                table_name: "focus_roots".into(),
                id_column: "root_id".into(),
                // Every column the `focus_roots` matview projects
                // (see sql/schema/matview_focus_roots.sql) must be
                // declared here, or a GQL query referencing it (e.g. the
                // right-sidebar `ORDER BY fr.added_ts`) fails to compile
                // with `UnknownProperty`. The bundled-query smoke test in
                // holon::di::registration pins this correspondence.
                columns: vec![
                    ("region".into(), "region".into()),
                    ("root_id".into(), "root_id".into()),
                    ("added_ts".into(), "added_ts".into()),
                    ("history_id".into(), "history_id".into()),
                ],
            },
        ];

        let edges = vec![GraphEdgeDef {
            edge_name: "FOCUSES_ON".into(),
            source_label: Some("current_focus".into()),
            target_label: Some("block".into()),
            fk_table: "current_focus".into(),
            fk_column: "block_id".into(),
            target_table: "block".into(),
            target_id_column: "id".into(),
        }];

        (nodes, edges)
    }
}

/// Sync state schema module for tracking synchronization tokens.
pub struct SyncStateSchemaModule;

#[async_trait]
impl SchemaModule for SyncStateSchemaModule {
    fn name(&self) -> &str {
        "sync_state"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("sync_states")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[SyncStateSchemaModule] Creating sync_states table");
        for stmt in sql_statements(include_str!("../sql/schema/sync_states.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::info!("[SyncStateSchemaModule] sync_states table created");
        Ok(())
    }
}

/// Operations schema module for undo/redo persistence.
/// NOTE: This schema MUST match the OperationLogEntry entity in
/// holon-core/src/operation_log.rs
pub struct OperationsSchemaModule;

#[async_trait]
impl SchemaModule for OperationsSchemaModule {
    fn name(&self) -> &str {
        "operations"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("operation")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[OperationsSchemaModule] Creating operation table");
        for stmt in sql_statements(include_str!("../sql/schema/operations.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::info!("[OperationsSchemaModule] operation table created");
        Ok(())
    }
}

/// History schema module providing the `block_history` table — the C2b
/// op/effect history relation (ADR 0024 P8), a disclosed ephemeral cache.
///
/// SINGLE OWNER of this table's DDL (`sql/schema/history.sql`);
/// `TursoHistoryStore` is only the typed accessor and assumes the table
/// exists. Schema evolution is drop + recreate: a table whose stored DDL
/// lacks the current version's sentinel column is dropped and rebuilt —
/// contractually fine, the relation is rebuildable and never authoritative.
pub struct HistorySchemaModule;

/// Column unique to the current `block_history` shape; its absence from the
/// stored DDL marks a stale table (bump alongside `history.sql`'s
/// `schema-version` comment when the shape changes again).
const HISTORY_SENTINEL_COLUMN: &str = "op_group";

#[async_trait]
impl SchemaModule for HistorySchemaModule {
    fn name(&self) -> &str {
        "history"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("block_history")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        let stored = db_handle
            .query_positional(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'block_history'",
                vec![],
            )
            .await?;
        if let Some(row) = stored.first() {
            let ddl = match row.get("sql") {
                Some(holon_api::Value::String(s)) => s.clone(),
                other => {
                    return Err(StorageError::SchemaError(format!(
                        "sqlite_master.sql for block_history: expected TEXT, got {other:?}"
                    )));
                }
            };
            if !ddl.contains(HISTORY_SENTINEL_COLUMN) {
                tracing::warn!(
                    "[HistorySchemaModule] block_history has a stale shape (no \
                     `{HISTORY_SENTINEL_COLUMN}` column); dropping and recreating (the relation \
                     is a disclosed ephemeral cache)"
                );
                db_handle.execute_ddl("DROP TABLE block_history").await?;
            }
        }
        for stmt in sql_statements(include_str!("../sql/schema/history.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::info!("[HistorySchemaModule] block_history table ready");
        Ok(())
    }
}

/// C4 derived-field SIDECAR table (`block_derived`).
///
/// Narrow, normalized store of computed field values keyed by
/// `(block_id, field_name)` — deliberately NOT inline columns on the `block`
/// matview (the wide seat-A path), so a change to a prototype's derived-field
/// *declarations* never forces a DROP+CREATE of the `block` matview. Rows are
/// maintained reactively by [`crate::derived_reconciler`] (a CDC watcher over a
/// source view), which recomputes only the delta.
///
/// Each row carries a `provenance` string (a hash of the field's
/// [`holon_api::computation::Computation`]) so a value produced by an outdated
/// declaration is detectable (its provenance differs from the current
/// declaration's). The relation is a rebuildable cache — never authoritative —
/// so `CREATE TABLE IF NOT EXISTS` is the whole story; there is no FK to
/// `block_raw` (the watcher retracts rows on the block-Deleted CDC event, and
/// an FK would drag every sidecar write into the fork's deferred-FK
/// autocommit-no-rollback hazard).
pub struct BlockDerivedSchemaModule;

#[async_trait]
impl SchemaModule for BlockDerivedSchemaModule {
    fn name(&self) -> &str {
        "block_derived"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("block_derived")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        db_handle
            .execute_ddl(
                "CREATE TABLE IF NOT EXISTS block_derived (\
                 block_id TEXT NOT NULL, \
                 field_name TEXT NOT NULL, \
                 value_json TEXT NOT NULL, \
                 provenance TEXT NOT NULL, \
                 PRIMARY KEY (block_id, field_name))",
            )
            .await?;
        tracing::info!("[BlockDerivedSchemaModule] block_derived sidecar table ready");
        Ok(())
    }
}

/// The C2 automation-journal matview (ADR 0024 P8): `block_history` effects
/// grouped by `(origin, transition_id, day)` with a per-group count. IVM
/// maintains it O(delta) over the `block_history` base TABLE — a rule watching
/// this relation sees the 7th postponement the moment its history row lands
/// (F1(a)). Registered like `trust_proposals` via [`reconcile_named_view`].
pub struct AutomationsJournalSchemaModule;

#[async_trait]
impl SchemaModule for AutomationsJournalSchemaModule {
    fn name(&self) -> &str {
        "automations_journal"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("automations_journal")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block_history")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[AutomationsJournalSchemaModule] Reconciling automations_journal matview");
        reconcile_named_view(
            db_handle,
            "automations_journal",
            include_str!("../sql/schema/automations_journal_matview.sql"),
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}

/// The DAY-PAGE DETECTION layer of the journal-feed chain
/// (`docs/Plans/JournalFeed-2026-07-18.md`): `journal_day_pages` — one row per
/// journal day page (a block tagged `Page` under `block:journals`). Chained on
/// the `block` matview JOINed against the `block_tags` junction, the same
/// matview-JOIN-junction shape IVM maintains for `focus_roots`. IVM-maintained
/// O(delta) as day pages are created / edited / deleted.
pub struct JournalDayPagesSchemaModule;

#[async_trait]
impl SchemaModule for JournalDayPagesSchemaModule {
    fn name(&self) -> &str {
        "journal_day_pages"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("journal_day_pages")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block"), Resource::schema("block_tags")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[JournalDayPagesSchemaModule] Reconciling journal_day_pages matview");
        reconcile_named_view(
            db_handle,
            "journal_day_pages",
            include_str!("../sql/schema/journal_day_pages_matview.sql"),
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}

/// The FEED layer of the journal-feed chain: `journal_feed` — a matview chained
/// on the `journal_day_pages` detection matview (matview-on-matview, supported
/// on the pinned Turso rev). Adds `expand_default = 1` so `render_entity()`
/// shows each day's children inline; the seam where feed windowing/LIMIT will
/// live (increment 2). Ordering is the read query's job.
pub struct JournalFeedSchemaModule;

#[async_trait]
impl SchemaModule for JournalFeedSchemaModule {
    fn name(&self) -> &str {
        "journal_feed"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("journal_feed")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("journal_day_pages")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[JournalFeedSchemaModule] Reconciling journal_feed matview");
        reconcile_named_view(
            db_handle,
            "journal_feed",
            include_str!("../sql/schema/journal_feed_matview.sql"),
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}

/// SELECT for the `backlinks` matview: one row per resolved link, carrying
/// `target_id` plus the FULL source-block row.
///
/// Entity-shaped over base tables only (`block_links ⋈ block_raw` — no
/// matview-on-matview hazard). The full block row is not decoration: a backlink
/// row is rendered through the same `block` entity profile as any other block,
/// so a narrower projection silently unbinds that profile's computed fields
/// (`bullet_shape` needs `collapsed`; `is_rule_head`/`is_holon_source`/
/// `is_legacy_rule` need `source_language`).
pub fn backlinks_view_select() -> String {
    let block_cols = BLOCK_RAW_COLUMNS
        .iter()
        .map(|c| format!("b.{c} AS {c}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT bl.resolved_id AS target_id, {block_cols} FROM block_links bl \
         JOIN block_raw b ON b.id = bl.source_block_id \
         WHERE bl.resolved_id IS NOT NULL"
    )
}

/// Link schema module providing the block_link table.
///
/// Indexes wiki-style `[[...]]` links extracted from block content.
/// Backlink queries use the `target_id` index directly — no materialized view
/// needed.
pub struct LinkSchemaModule;

#[async_trait]
impl SchemaModule for LinkSchemaModule {
    fn name(&self) -> &str {
        "links"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![
            Resource::schema("block_links"),
            Resource::schema("block_redirects"),
            Resource::schema("backlinks"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block_raw")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[LinkSchemaModule] Creating block_links junction + backlinks matview");
        // The pre-increment-2 `block_link` table (LiveData-subscriber-fed,
        // content-regex extraction) is gone: links now derive from
        // `block.marks` at the SQL write boundary.
        db_handle
            .execute_ddl("DROP TABLE IF EXISTS block_link")
            .await?;
        for stmt in sql_statements(include_str!("../sql/schema/block_links.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        // Merge redirects live here rather than in their own module: they are
        // the other half of id resolution and are re-derived at the same SQL
        // write boundary (from the survivor's `merged_from` property, as
        // `block_links` is from `marks`). They are read on the block-lookup MISS
        // path, NOT by the `resolved_id` rewrite — `merge_blocks` re-points
        // inbound links eagerly, so a resolved link never needs the redirect.
        for stmt in sql_statements(include_str!("../sql/schema/block_redirects.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        reconcile_named_view(db_handle, "backlinks", &backlinks_view_select())
            .await
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        tracing::info!("[LinkSchemaModule] block_links + block_redirects + backlinks ready");
        Ok(())
    }

    fn graph_contributions(
        &self,
    ) -> (
        Vec<holon_api::entity::GraphNodeDef>,
        Vec<holon_api::entity::GraphEdgeDef>,
    ) {
        use holon_api::entity::GraphEdgeDef;

        let edges = vec![GraphEdgeDef {
            edge_name: "LINKS_TO".into(),
            source_label: Some("block".into()),
            target_label: None,
            fk_table: "block_links".into(),
            fk_column: "resolved_id".into(),
            target_table: "block_links".into(),
            target_id_column: "resolved_id".into(),
        }];

        (vec![], edges)
    }
}

/// Identity schema module providing canonical_entity, entity_alias, and
/// proposal_queue tables.
///
/// Tables are empty by default — they hold cross-system entity resolution state
/// once the merge / propose-merge / accept-proposal operations land. Adding the
/// schema seam now ensures every future integration plugs into the same
/// identity layer instead of growing ad-hoc identity columns. See
/// `docs/Architecture/Schema.md` §"Entity Identity".
pub struct IdentitySchemaModule;

#[async_trait]
impl SchemaModule for IdentitySchemaModule {
    fn name(&self) -> &str {
        "identity"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![
            Resource::schema("canonical_entity"),
            Resource::schema("entity_alias"),
            Resource::schema("proposal_queue"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[IdentitySchemaModule] Creating identity tables");
        for stmt in sql_statements(include_str!("../sql/schema/identity.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::info!("[IdentitySchemaModule] identity tables created");
        Ok(())
    }
}

/// Graph EAV (entity-attribute-value) schema for typed-entity edges.
///
/// Faithful transliteration of the former inline DDL loop in holon's
/// `di/schema_providers.rs` (`DbReady<GraphEavSchema>` provider): same
/// statements, same `Resource::schema("graph_eav")` availability marker,
/// same DDL-then-mark ordering (via `run_schema_module`).
pub struct GraphEavSchemaModule;

#[async_trait]
impl SchemaModule for GraphEavSchemaModule {
    fn name(&self) -> &str {
        "graph_eav"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("graph_eav")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        for stmt in sql_statements(include_str!("../sql/schema/graph_eav.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_schema_module_provides() {
        let module = CoreSchemaModule;
        let provides = module.provides();

        assert!(provides.contains(&Resource::schema("block_raw")));
        assert!(provides.contains(&Resource::schema("file")));
    }

    #[test]
    fn test_block_hierarchy_requires_blocks() {
        let module = BlockHierarchySchemaModule;
        let requires = module.requires();

        assert!(requires.contains(&Resource::schema("block")));
    }

    fn requires_descriptor() -> EdgeFieldDescriptor {
        EdgeFieldDescriptor {
            entity: "block".into(),
            field: "requires".into(),
            join_table: "block_requires".into(),
            source_col: "source_id".into(),
            target_col: "target_id".into(),
        }
    }

    // A backlink row is rendered through the `block` entity profile like any
    // other block row, so the matview must carry the whole block row. A
    // narrower projection unbinds the profile's computed fields — the shipped
    // `assets/default/types/block_profile.yaml` reads `collapsed`
    // (`bullet_shape`) and `source_language` (`is_rule_head`,
    // `is_holon_source`, `is_legacy_rule`).
    #[test]
    fn backlinks_view_projects_every_block_column() {
        let sql = backlinks_view_select();
        for column in BLOCK_RAW_COLUMNS {
            assert!(
                sql.contains(&format!("b.{column} AS {column}")),
                "backlinks matview drops block column '{column}': {sql}"
            );
        }
        assert!(sql.contains("bl.resolved_id AS target_id"));
    }

    #[test]
    fn edge_agg_view_name_appends_agg_suffix() {
        assert_eq!(
            edge_agg_view_name(&requires_descriptor()),
            "block_requires_agg"
        );
    }

    #[test]
    fn edge_agg_view_select_groups_targets_by_source() {
        assert_eq!(
            edge_agg_view_select(&requires_descriptor()),
            "SELECT source_id AS source_id, json_group_array(target_id) AS vals \
             FROM block_requires GROUP BY source_id"
        );
    }

    // The `block` matview SELECT is read by every projection: column order and
    // the sentinel-exclusion WHERE are load-bearing for correctness. Pin the
    // full string for a single-junction shape.
    #[test]
    fn block_matview_select_exact_shape() {
        assert_eq!(
            block_matview_select(&[requires_descriptor()]),
            "SELECT b.id, b.parent_id, b.sort_key, b.content, b.content_type, \
             b.source_language, b.source_name, b.properties, b.marks, b.collapsed, b.widget_only, \
             b.completed, \
             b.block_type, b.created_at, b.updated_at, b._change_origin, b.write_seq, \
             COALESCE(block_requires_agg.vals, '[]') AS requires FROM block_raw b \
             LEFT OUTER JOIN block_requires_agg ON block_requires_agg.source_id = b.id \
             WHERE b.id != 'sentinel:no_parent'"
        );
    }

    #[test]
    fn block_matview_select_with_computed_appends_planted_column() {
        let sql = block_matview_select_with_computed(
            &[requires_descriptor()],
            &[PlantedColumn {
                name: "is_done".into(),
                sql: "completed = 1".into(),
            }],
        );
        assert!(sql.contains("COALESCE(block_requires_agg.vals, '[]') AS requires"));
        assert!(sql.contains("(completed = 1) AS is_done"));
        assert!(sql.ends_with("WHERE b.id != 'sentinel:no_parent'"));
    }

    #[test]
    fn block_edge_fields_are_all_block_scoped_and_nonempty() {
        let fields = block_edge_fields();
        assert!(
            !fields.is_empty(),
            "block must declare at least one edge field"
        );
        assert!(
            fields.iter().all(|d| d.entity == "block"),
            "block_edge_fields must only return entity==block descriptors"
        );
    }

    /// The pre-2026-07-22 junction DDL, verbatim: a FOREIGN KEY on the TARGET
    /// column, which aborts a whole file ingest when that target is a forward
    /// or cross-file reference.
    const OLD_BLOCK_REQUIRES_DDL: &str = "CREATE TABLE block_requires (block_id TEXT NOT NULL, \
                                          required_id TEXT NOT NULL, PRIMARY KEY (block_id, \
                                          required_id), FOREIGN KEY (block_id) REFERENCES \
                                          block_raw(id) ON DELETE CASCADE, FOREIGN KEY \
                                          (required_id) REFERENCES block_raw(id) ON DELETE \
                                          CASCADE)";

    async fn block_requires_ddl(handle: &DbHandle) -> String {
        handle
            .query_positional(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'block_requires'",
                vec![],
            )
            .await
            .expect("read block_requires DDL")
            .first()
            .and_then(|r| r.get("sql"))
            .and_then(|v| v.as_string())
            .expect("block_requires must exist")
            .to_string()
    }

    /// THE DEPLOYED SHAPE, not just the old table: a real pre-2026-07-22
    /// database also carries the persisted `block_requires_agg` matview,
    /// because `reconcile_named_view` early-returns on an unchanged SELECT
    /// and therefore never recreates it across reboots. `BlockSchemaModule`
    /// runs BEFORE the matview module, so it meets that matview still
    /// standing — and Turso refuses to `ALTER TABLE ... RENAME` a table
    /// with dependent materialized views. Without clearing them first,
    /// `ensure_schema` Errs, the BOOT FAILS, and the ingest-aborting FK
    /// survives.
    #[tokio::test]
    async fn migration_survives_the_persisted_dependent_matview() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory().await.unwrap();
        CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("core schema");

        handle
            .execute_ddl(OLD_BLOCK_REQUIRES_DDL)
            .await
            .expect("old-shape block_requires");
        handle
            .execute(
                // ALLOW(sole_block_writer): schema-module unit test seeding the FK parent row.
                "INSERT INTO block_raw (id, parent_id, sort_key, content) VALUES \
                 ('block:src', 'sentinel:no_parent', 1.0, 'Source')",
                vec![],
            )
            .await
            .expect("seed block_raw");
        handle
            .execute(
                "INSERT INTO block_requires (block_id, required_id) VALUES ('block:src', \
                 'block:src')",
                vec![],
            )
            .await
            .expect("seed old junction row");

        // The dependent matview a deployed database carries.
        handle
            .execute_ddl(
                "CREATE MATERIALIZED VIEW block_requires_agg AS SELECT block_id AS source_id, \
                 json_group_array(required_id) AS vals FROM block_requires GROUP BY block_id",
            )
            .await
            .expect("persisted dependent matview");

        BlockSchemaModule.ensure_schema(&handle).await.expect(
            "boot must SUCCEED against a deployed pre-07-22 database; a rename blocked by the \
                 persisted dependent matview fails the whole boot and leaves the FK in place",
        );

        let ddl = block_requires_ddl(&handle).await;
        assert!(
            !ddl.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains("FOREIGN KEY (required_id)"),
            "the ingest-aborting target FK must be gone; DDL still reads: {ddl}"
        );
        let rows = handle
            .query(
                "SELECT block_id, required_id FROM block_requires",
                HashMap::new(),
            )
            .await
            .expect("query migrated rows");
        assert_eq!(rows.len(), 1, "the migration must PRESERVE rows");
    }

    /// The copy is not transactional, so a crash between the rename and the
    /// copy leaves the staging table holding the ONLY surviving rows. The
    /// next boot sees a current-shape main table and must RESUME rather
    /// than early-return — early-returning strands every row in an orphaned
    /// staging table.
    #[tokio::test]
    async fn interrupted_migration_resumes_and_recovers_staged_rows() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory().await.unwrap();
        CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("core schema");
        handle
            .execute(
                // ALLOW(sole_block_writer): schema-module unit test seeding the FK parent row.
                "INSERT INTO block_raw (id, parent_id, sort_key, content) VALUES \
                 ('block:src', 'sentinel:no_parent', 1.0, 'Source')",
                vec![],
            )
            .await
            .expect("seed block_raw");

        // Exactly the state a crash between RENAME and the copy leaves behind:
        // staging holds the rows, the main table does not exist yet.
        handle
            .execute_ddl(
                "CREATE TABLE block_requires__pre_target_fk (block_id TEXT NOT NULL, required_id \
                 TEXT NOT NULL, PRIMARY KEY (block_id, required_id))",
            )
            .await
            .expect("staging table");
        handle
            .execute(
                "INSERT INTO block_requires__pre_target_fk (block_id, required_id) VALUES \
                 ('block:src', 'block:orphaned')",
                vec![],
            )
            .await
            .expect("stranded row");

        BlockSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("boot must resume the interrupted migration");

        let rows = handle
            .query(
                "SELECT block_id, required_id FROM block_requires WHERE required_id = \
                 'block:orphaned'",
                HashMap::new(),
            )
            .await
            .expect("query recovered row");
        assert_eq!(
            rows.len(),
            1,
            "the row stranded in staging by the interrupted attempt must be recovered, not \
             abandoned"
        );

        let staging_left = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = \
                 'block_requires__pre_target_fk'",
                HashMap::new(),
            )
            .await
            .expect("query staging presence");
        assert!(
            staging_left.is_empty(),
            "staging must be dropped once its rows are accounted for"
        );
    }

    /// A database created before 2026-07-22 carries the old junction shape. The
    /// boot-time drop used to reshape it by destroying it; now that the drop is
    /// gone, `ensure_schema` must migrate it in place and keep every row —
    /// otherwise such a database silently keeps the ingest-aborting FK.
    #[tokio::test]
    async fn pre_target_fk_junction_is_migrated_preserving_rows() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory().await.unwrap();
        CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("core schema");

        // Stand up the OLD shape, as an old database would have it.
        handle
            .execute_ddl(OLD_BLOCK_REQUIRES_DDL)
            .await
            .expect("old-shape block_requires");
        handle
            .execute_ddl(
                "CREATE INDEX IF NOT EXISTS idx_block_requires_required ON \
                 block_requires(required_id)",
            )
            .await
            .expect("old-shape index");
        handle
            .execute(
                // ALLOW(sole_block_writer): schema-module unit test seeding the FK parent row.
                "INSERT INTO block_raw (id, parent_id, sort_key, content) VALUES \
                 ('block:src', 'sentinel:no_parent', 1.0, 'Source')",
                vec![],
            )
            .await
            .expect("seed block_raw");
        handle
            .execute(
                "INSERT INTO block_requires (block_id, required_id) VALUES ('block:src', \
                 'block:src')",
                vec![],
            )
            .await
            .expect("seed old junction row");

        BlockSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("boot over an old-shape database");

        let ddl = block_requires_ddl(&handle).await;
        assert!(
            !ddl.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains("FOREIGN KEY (required_id)"),
            "the ingest-aborting target FK must be gone after migration; DDL still reads: {ddl}"
        );

        let rows = handle
            .query(
                "SELECT block_id, required_id FROM block_requires",
                HashMap::new(),
            )
            .await
            .expect("query migrated rows");
        assert_eq!(
            rows.len(),
            1,
            "the migration must PRESERVE rows, not drop the table"
        );

        // The target FK is really gone: a dangling target now inserts, where the
        // old shape aborted the transaction.
        handle
            .execute(
                "INSERT INTO block_requires (block_id, required_id) VALUES ('block:src', \
                 'block:never-ingested')",
                vec![],
            )
            .await
            .expect("a dangling target must be insertable after the migration");

        // Second boot is a no-op: no re-migration, nothing lost.
        BlockSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("second boot");
        assert_eq!(
            block_requires_ddl(&handle).await,
            ddl,
            "a current-shape database must not be reshaped again"
        );
        let rows = handle
            .query(
                "SELECT block_id, required_id FROM block_requires",
                HashMap::new(),
            )
            .await
            .expect("query rows after second boot");
        assert_eq!(rows.len(), 2, "second boot must preserve every row");
    }

    #[test]
    fn test_identity_schema_module_provides() {
        let module = IdentitySchemaModule;
        let provides = module.provides();

        assert!(provides.contains(&Resource::schema("canonical_entity")));
        assert!(provides.contains(&Resource::schema("entity_alias")));
        assert!(provides.contains(&Resource::schema("proposal_queue")));
        assert!(module.requires().is_empty());
    }

    #[test]
    fn test_core_schema_module_provides_clock() {
        assert!(
            CoreSchemaModule
                .provides()
                .contains(&Resource::schema("clock"))
        );
    }

    /// The `clock` relation is created + seeded by `CoreSchemaModule`, and an
    /// `ensure_schema` runs on EVERY boot, so it must be non-destructive: the
    /// junction tables hold projected state that persists across restarts
    /// exactly like `block_raw` does. A `DROP TABLE` here silently empties them
    /// on the second boot, and the unchanged-file ingest fast path then never
    /// refills them — the vault's `Page` tags vanish while `block_raw`
    /// survives.
    #[tokio::test]
    async fn block_junction_schema_is_non_destructive_across_boots() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory().await.unwrap();

        CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("core schema");
        BlockSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("boot-1 junction schema");

        handle
            .execute(
                // ALLOW(sole_block_writer): schema-module unit test seeding the FK parent row.
                "INSERT INTO block_raw (id, parent_id, sort_key, content) VALUES \
                 ('block:p', 'sentinel:no_parent', 1.0, 'Page One')",
                vec![],
            )
            .await
            .expect("seed block_raw");
        handle
            .execute(
                "INSERT INTO block_tags (block_id, tag) VALUES ('block:p', 'Page')",
                vec![],
            )
            .await
            .expect("seed block_tags");
        handle
            .execute(
                "INSERT INTO block_requires (block_id, required_id) VALUES ('block:p', 'block:q')",
                vec![],
            )
            .await
            .expect("seed block_requires");

        // Boot 2: the same modules run again over the same database.
        CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("boot-2 core schema");
        BlockSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("boot-2 junction schema");

        let blocks = handle
            .query(
                "SELECT id FROM block_raw WHERE id != 'sentinel:no_parent'",
                HashMap::new(),
            )
            .await
            .expect("query block_raw");
        assert_eq!(
            blocks.len(),
            1,
            "premise: `block_raw` survives a second `ensure_schema`"
        );

        let tags = handle
            .query("SELECT block_id, tag FROM block_tags", HashMap::new())
            .await
            .expect("query block_tags");
        assert_eq!(
            tags.len(),
            1,
            "`block_tags` was EMPTIED by the second `ensure_schema` while `block_raw` survived — \
             every boot wipes the vault's Page tags"
        );

        let requires = handle
            .query(
                "SELECT block_id, required_id FROM block_requires",
                HashMap::new(),
            )
            .await
            .expect("query block_requires");
        assert_eq!(
            requires.len(),
            1,
            "`block_requires` was EMPTIED by the second `ensure_schema`"
        );
    }

    /// `UPDATE` of the day row emits CDC through a matview (base tables never
    /// emit directly — only matviews do; see `cdc_base_vs_matview_repro`).
    #[tokio::test]
    async fn clock_schema_creates_seeds_and_update_emits_cdc() {
        use holon_api::streaming::Change;

        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory().await.unwrap();

        CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("core schema (incl. clock) must initialize");

        // Seeded: exactly one `day` row at the deterministic placeholder.
        let rows = handle
            .query("SELECT grain, today, epoch_day FROM clock", HashMap::new())
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "clock must seed exactly one row");
        assert_eq!(rows[0].get("grain").unwrap().as_string(), Some("day"));
        assert_eq!(
            rows[0].get("today").unwrap().as_string(),
            Some("1970-01-01")
        );

        // A matview is the only relation that surfaces base-table changes as CDC.
        handle
            .execute_ddl(
                "CREATE MATERIALIZED VIEW clock_mirror AS SELECT grain, today, epoch_day FROM \
                 clock",
            )
            .await
            .unwrap();

        let mut cdc_rx = handle.subscribe_cdc("clock_mirror").await.unwrap();

        handle
            .execute(
                "UPDATE clock SET today = '2026-07-10', epoch_day = 20644, updated_at = \
                 '2026-07-10T00:00:00Z' WHERE grain = 'day'",
                vec![],
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let mut updates = 0usize;
        while let Ok(batch) = cdc_rx.try_recv() {
            for rc in batch.inner.items {
                if rc.relation_name == "clock_mirror"
                    && let Change::Updated { data, .. } = &rc.change
                {
                    assert_eq!(data.get("today").unwrap().as_string(), Some("2026-07-10"));
                    updates += 1;
                }
            }
        }
        assert!(
            updates >= 1,
            "UPDATE of the clock day row must emit an Updated CDC event via clock_mirror"
        );

        handle.shutdown().await.unwrap();
    }
}
