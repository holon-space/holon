//! Concrete schema module implementations for core database objects.
//!
//! This module provides `SchemaModule` implementations for the core database
//! schema objects in Holon:
//!
//! - `CoreSchemaModule`: blocks, documents, directories tables
//! - `BlockSchemaModule`: task_blockers, block_tags junction tables + task_blocking_edges matview
//! - `BlockHierarchySchemaModule`: block_with_path materialized view
//! - `NavigationSchemaModule`: navigation_history, navigation_cursor, current_focus
//! - `SyncStateSchemaModule`: sync_states table
//! - `OperationsSchemaModule`: operations table for undo/redo
//! - `IdentitySchemaModule`: canonical_entity, entity_alias, proposal_queue tables

use std::collections::HashMap;

use async_trait::async_trait;

use super::resource::Resource;
use super::schema_module::{EdgeFieldDescriptor, SchemaModule};
use super::sql_statements;
use super::turso::DbHandle;
use super::types::Result;
use super::types::StorageError;
use crate::sync::reconcile_named_view;

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
            Resource::schema("block"),
            Resource::schema("directory"),
            Resource::schema("file"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![] // No dependencies - this is the root
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[CoreSchemaModule] Creating core tables");

        for stmt in sql_statements(include_str!("../../sql/schema/blocks.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[CoreSchemaModule] blocks table + index created");

        for stmt in sql_statements(include_str!("../../sql/schema/directories.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[CoreSchemaModule] directories table + index created");

        for stmt in sql_statements(include_str!("../../sql/schema/files.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[CoreSchemaModule] files table + indexes created");

        tracing::info!("[CoreSchemaModule] Core tables created successfully");
        Ok(())
    }
}

/// Block junction-table schema module.
///
/// Owns `task_blockers` and `block_tags` (the production junction tables) plus
/// the `task_blocking_edges` H1 matview for CDC observability. Runs DROP TABLE
/// IF EXISTS on both tables before CREATE so any dev DB carrying the old Phase 0
/// scratch tables (which lacked PK/FK) picks up the correct schema.
pub struct BlockSchemaModule;

#[async_trait]
impl SchemaModule for BlockSchemaModule {
    fn name(&self) -> &str {
        "block_junction"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![
            Resource::schema("task_blockers"),
            Resource::schema("block_tags"),
            Resource::schema("task_blocking_edges"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[BlockSchemaModule] Migrating junction tables");

        // Drop Phase 0 scratch tables (lacked composite PK + FK CASCADE).
        db_handle
            .execute_ddl("DROP TABLE IF EXISTS task_blockers")
            .await?;
        db_handle
            .execute_ddl("DROP TABLE IF EXISTS block_tags")
            .await?;

        for stmt in sql_statements(include_str!("../../sql/schema/task_blockers.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[BlockSchemaModule] task_blockers table created");

        for stmt in sql_statements(include_str!("../../sql/schema/block_tags.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[BlockSchemaModule] block_tags table created");

        reconcile_named_view(
            db_handle,
            "task_blocking_edges",
            include_str!("../../sql/schema/task_blocking_edges_matview.sql"),
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        tracing::debug!("[BlockSchemaModule] task_blocking_edges matview reconciled");

        tracing::info!("[BlockSchemaModule] Junction tables ready");
        Ok(())
    }

    fn edge_fields(&self) -> Vec<EdgeFieldDescriptor> {
        vec![
            EdgeFieldDescriptor {
                entity: "block".to_string(),
                field: "blocked_by".to_string(),
                join_table: "task_blockers".to_string(),
                source_col: "blocked_id".to_string(),
                target_col: "blocker_id".to_string(),
            },
            EdgeFieldDescriptor {
                entity: "block".to_string(),
                field: "tags".to_string(),
                join_table: "block_tags".to_string(),
                source_col: "block_id".to_string(),
                target_col: "tag".to_string(),
            },
        ]
    }
}

/// Block hierarchy schema module providing the block_with_path materialized view.
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
            include_str!("../../sql/schema/blocks_with_paths.sql"),
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
            Resource::schema("editor_cursor"),
            Resource::schema("current_editor_focus"),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        // focus_roots matview JOINs the block table
        vec![Resource::schema("block")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[NavigationSchemaModule] Creating navigation tables");

        for stmt in sql_statements(include_str!("../../sql/schema/navigation.sql")) {
            match db_handle.execute_ddl(stmt).await {
                Ok(()) => {}
                Err(e) if e.to_string().contains("already exists") => {
                    tracing::debug!(
                        "[NavigationSchemaModule] Skipping (already exists): {}",
                        &stmt[..stmt.len().min(60)]
                    );
                }
                Err(e) => return Err(e.into()),
            }
        }

        tracing::info!("[NavigationSchemaModule] Reconciling navigation matviews");
        let views: &[(&str, &str)] = &[
            (
                "current_focus",
                include_str!("../../sql/schema/matview_current_focus.sql"),
            ),
            (
                "current_editor_focus",
                include_str!("../../sql/schema/matview_current_editor_focus.sql"),
            ),
            (
                "focus_roots",
                include_str!("../../sql/schema/matview_focus_roots.sql"),
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
                    include_str!("../../sql/navigation/init_default_region.sql"),
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
        use holon_api::entity::{GraphEdgeDef, GraphNodeDef};

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
        for stmt in sql_statements(include_str!("../../sql/schema/sync_states.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::info!("[SyncStateSchemaModule] sync_states table created");
        Ok(())
    }
}

/// Operations schema module for undo/redo persistence.
/// NOTE: This schema MUST match the OperationLogEntry entity in holon-core/src/operation_log.rs
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
        for stmt in sql_statements(include_str!("../../sql/schema/operations.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::info!("[OperationsSchemaModule] operation table created");
        Ok(())
    }
}

/// Link schema module providing the block_link table.
///
/// Indexes wiki-style `[[...]]` links extracted from block content.
/// Backlink queries use the `target_id` index directly — no materialized view needed.
pub struct LinkSchemaModule;

#[async_trait]
impl SchemaModule for LinkSchemaModule {
    fn name(&self) -> &str {
        "links"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("block_link")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[LinkSchemaModule] Creating block_link table");
        for stmt in sql_statements(include_str!("../../sql/schema/block_links.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::info!("[LinkSchemaModule] block_link table created");
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
            fk_table: "block_link".into(),
            fk_column: "target_id".into(),
            target_table: "block_link".into(),
            target_id_column: "target_id".into(),
        }];

        (vec![], edges)
    }
}

/// Identity schema module providing canonical_entity, entity_alias, and proposal_queue tables.
///
/// Tables are empty by default — they hold cross-system entity resolution state
/// once the merge / propose-merge / accept-proposal operations land. Adding the
/// schema seam now ensures every future integration plugs into the same identity
/// layer instead of growing ad-hoc identity columns. See
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
        for stmt in sql_statements(include_str!("../../sql/schema/identity.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::info!("[IdentitySchemaModule] identity tables created");
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

        assert!(provides.contains(&Resource::schema("block")));
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
}
