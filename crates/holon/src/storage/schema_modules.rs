//! Concrete schema module implementations for core database objects.
//!
//! This module provides `SchemaModule` implementations for the core database
//! schema objects in Holon:
//!
//! - `CoreSchemaModule`: blocks, documents, directories tables
//! - `BlockHierarchySchemaModule`: block_with_path materialized view
//! - `NavigationSchemaModule`: navigation_history, navigation_cursor, current_focus
//! - `SyncStateSchemaModule`: sync_states table
//! - `OperationsSchemaModule`: operations table for undo/redo

use std::collections::HashMap;

use async_trait::async_trait;

use super::resource::Resource;
use super::schema_module::SchemaModule;
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
}
