//! Concrete schema module implementations for core database objects.
//!
//! This module provides `SchemaModule` implementations for the core database
//! schema objects in Holon:
//!
//! - `CoreSchemaModule`: block_raw (the underlying block table), directories,
//!   files
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
use holon_core::storage::resource::Resource;
use holon_core::storage::types::Result;
use holon_core::storage::types::StorageError;

use crate::matview_manager::reconcile_named_view;
use crate::schema_module::EdgeFieldDescriptor;
use crate::schema_module::SchemaModule;
use crate::sql_utils::sql_statements;
use crate::turso::DbHandle;

/// Core schema module providing the fundamental tables: blocks, directories.
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
            Resource::schema("directory"),
            Resource::schema("file"),
            Resource::schema("clock"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![] // No dependencies - this is the root
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[CoreSchemaModule] Creating core tables");

        for stmt in sql_statements(include_str!("../sql/schema/blocks.sql")) {
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

        for stmt in sql_statements(include_str!("../sql/schema/directories.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[CoreSchemaModule] directories table + index created");

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

        // Drop the legacy and the previous-name junction tables before
        // recreating. `task_blockers` is the pre-rename name; dropping it
        // (rather than ALTER RENAME) is correct because the matviews and
        // edge-field descriptors all reference `block_requires` now and
        // existing rows would no longer be reachable through the renamed
        // schema.
        db_handle
            .execute_ddl("DROP TABLE IF EXISTS task_blockers")
            .await?;
        db_handle
            .execute_ddl("DROP TABLE IF EXISTS block_requires")
            .await?;
        db_handle
            .execute_ddl("DROP TABLE IF EXISTS block_tags")
            .await?;
        db_handle
            .execute_ddl("DROP TABLE IF EXISTS advice_suppressed")
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
    "depth",
    "sort_key",
    "content",
    "content_type",
    "source_language",
    "source_name",
    "properties",
    "marks",
    "collapsed",
    "completed",
    "block_type",
    "created_at",
    "updated_at",
    "_change_origin",
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
    let mut columns: Vec<String> = BLOCK_RAW_COLUMNS.iter().map(|c| format!("b.{c}")).collect();
    let mut joins = Vec::new();
    for d in descriptors {
        let agg = edge_agg_view_name(d);
        columns.push(format!("COALESCE({agg}.vals, '[]') AS {}", d.field));
        joins.push(format!("LEFT OUTER JOIN {agg} ON {agg}.source_id = b.id"));
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
                columns: vec![
                    ("region".into(), "region".into()),
                    ("block_id".into(), "block_id".into()),
                    ("root_id".into(), "root_id".into()),
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
        // backlinks: entity-shaped (carries the source block's `id`) IVM
        // matview over base tables only (block_links ⋈ block_raw — no
        // matview-on-matview hazard). One row per resolved link: which block
        // references `target_id`, with enough of the source block to render a
        // backlink entry.
        reconcile_named_view(
            db_handle,
            "backlinks",
            "SELECT bl.resolved_id AS target_id, b.id AS id, b.parent_id AS parent_id, b.content \
             AS content, b.content_type AS content_type FROM block_links bl JOIN block_raw b ON \
             b.id = bl.source_block_id WHERE bl.resolved_id IS NOT NULL",
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        tracing::info!("[LinkSchemaModule] block_links + backlinks ready");
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
        assert!(provides.contains(&Resource::schema("directory")));
        assert!(provides.contains(&Resource::schema("file")));
    }

    #[test]
    fn test_block_hierarchy_requires_blocks() {
        let module = BlockHierarchySchemaModule;
        let requires = module.requires();

        assert!(requires.contains(&Resource::schema("block")));
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
                if rc.relation_name == "clock_mirror" {
                    if let Change::Updated { data, .. } = &rc.change {
                        assert_eq!(data.get("today").unwrap().as_string(), Some("2026-07-10"));
                        updates += 1;
                    }
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
