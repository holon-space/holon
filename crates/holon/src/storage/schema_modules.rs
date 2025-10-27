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
use std::sync::Arc;

use async_trait::async_trait;

use super::resource::Resource;
use super::schema_module::SchemaModule;
use super::sql_statements;
use super::turso::DbHandle;
use super::types::Result;

/// Core schema module providing the fundamental tables: blocks, documents, directories.
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
            Resource::schema("document"),
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

        for stmt in sql_statements(include_str!("../../sql/schema/documents.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }
        tracing::debug!("[CoreSchemaModule] documents table + indexes created");

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
        tracing::info!("[BlockHierarchySchemaModule] Creating block_with_path view");
        db_handle
            .execute_ddl(include_str!("../../sql/schema/blocks_with_paths.sql"))
            .await?;
        tracing::info!("[BlockHierarchySchemaModule] block_with_path view created");
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
        vec![] // No dependencies on other modules
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        tracing::info!("[NavigationSchemaModule] Creating navigation tables");

        for stmt in sql_statements(include_str!("../../sql/schema/navigation.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }

        tracing::info!("[NavigationSchemaModule] Navigation tables created");
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
                label: "CurrentFocus".into(),
                table_name: "current_focus".into(),
                id_column: "region".into(),
                columns: vec![
                    ("region".into(), "region".into()),
                    ("block_id".into(), "block_id".into()),
                    ("timestamp".into(), "timestamp".into()),
                ],
            },
            GraphNodeDef {
                label: "FocusRoot".into(),
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
            source_label: Some("CurrentFocus".into()),
            target_label: Some("Block".into()),
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

/// Creates a SchemaRegistry with all core schema modules registered.
///
/// This is the standard configuration for Holon applications.
/// Modules are registered (but not yet initialized) in this function.
///
/// # Example
///
/// ```rust,ignore
/// let registry = create_core_schema_registry();
/// registry.initialize_all(backend, scheduler_handle, vec![]).await?;
/// ```
pub fn create_core_schema_registry() -> super::schema_module::SchemaRegistry {
    let mut registry = super::schema_module::SchemaRegistry::new();

    // Register all core modules
    registry.register(Arc::new(CoreSchemaModule));
    registry.register(Arc::new(BlockHierarchySchemaModule));
    registry.register(Arc::new(NavigationSchemaModule));
    registry.register(Arc::new(SyncStateSchemaModule));
    registry.register(Arc::new(OperationsSchemaModule));

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_schema_module_provides() {
        let module = CoreSchemaModule;
        let provides = module.provides();

        assert!(provides.contains(&Resource::schema("block")));
        assert!(provides.contains(&Resource::schema("document")));
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
    fn test_core_registry_ordering() {
        let registry = create_core_schema_registry();

        // Should have 5 modules
        assert_eq!(registry.len(), 5);

        // Topological sort should succeed (no cycles)
        // Note: We can't test the actual order without accessing private methods,
        // but we can verify it doesn't panic
    }
}
