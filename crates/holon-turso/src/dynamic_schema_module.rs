//! DynamicSchemaModule: generates DDL from a TypeDefinition at runtime.
//!
//! Used for user-defined types (Person, Organization), pre-configured types,
//! and MCP entity types. All create extension tables that FK to the `block`
//! table.

use std::sync::Arc;

use async_trait::async_trait;
use holon_api::TypeDefinition;
use holon_core::storage::Resource;
use holon_core::storage::Result;

use super::schema_module::SchemaModule;
use super::turso::DbHandle;

/// A SchemaModule constructed from a TypeDefinition at runtime.
///
/// Creates an extension table that foreign-keys to `block(id)`.
/// The table name is the TypeDefinition's `name` field.
pub struct DynamicSchemaModule {
    type_def: TypeDefinition,
}

impl DynamicSchemaModule {
    pub fn new(type_def: TypeDefinition) -> Self {
        Self { type_def }
    }

    /// Create as `Arc<dyn SchemaModule>`.
    pub fn arc(type_def: TypeDefinition) -> Arc<dyn SchemaModule> {
        Arc::new(Self::new(type_def))
    }
}

#[async_trait]
impl SchemaModule for DynamicSchemaModule {
    fn name(&self) -> &str {
        &self.type_def.name
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema(&self.type_def.name)]
    }

    fn requires(&self) -> Vec<Resource> {
        let mut deps = Vec::new();
        // Extension tables require the block table
        if self.type_def.id_references.is_some() {
            deps.push(Resource::schema("block"));
        }
        deps
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        let create_sql = self.type_def.to_create_table_sql();
        tracing::info!(
            "[DynamicSchemaModule] Creating table '{}': {}",
            self.type_def.name,
            &create_sql[..create_sql.len().min(120)]
        );
        db_handle.execute_ddl(&create_sql).await.map_err(|e| {
            holon_core::storage::StorageError::DatabaseError(format!(
                "Failed to create table '{}': {e}",
                self.type_def.name
            ))
        })?;

        for index_sql in self.type_def.to_index_sql() {
            db_handle.execute_ddl(&index_sql).await.map_err(|e| {
                holon_core::storage::StorageError::DatabaseError(format!(
                    "Failed to create index for '{}': {e}",
                    self.type_def.name
                ))
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use holon_api::FieldSchema;

    use super::*;

    #[test]
    fn dynamic_module_provides_and_requires() {
        let td = TypeDefinition {
            name: "person".to_string(),
            id_references: Some("block".to_string()),
            ..TypeDefinition::new(
                "person",
                vec![
                    FieldSchema::new("id", "TEXT").primary_key(),
                    FieldSchema::new("email", "TEXT").indexed(),
                    FieldSchema::new("role", "TEXT").nullable(),
                ],
            )
        };

        let module = DynamicSchemaModule::new(td);
        assert_eq!(module.name(), "person");
        assert_eq!(module.provides(), vec![Resource::schema("person")]);
        assert_eq!(module.requires(), vec![Resource::schema("block")]);
    }

    #[test]
    fn standalone_module_no_requires() {
        let td = TypeDefinition::new(
            "standalone",
            vec![FieldSchema::new("id", "TEXT").primary_key()],
        );

        let module = DynamicSchemaModule::new(td);
        assert!(module.requires().is_empty());
    }

    // ensure_schema must actually CREATE the table (and its indexes), not just
    // return Ok(()). Drives a real in-memory backend and confirms the table +
    // its declared index land in sqlite_master.
    #[tokio::test]
    async fn ensure_schema_creates_the_table_and_index() {
        use std::collections::HashMap;

        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");

        let td = TypeDefinition::new(
            "widget",
            vec![
                FieldSchema::new("id", "TEXT").primary_key(),
                FieldSchema::new("label", "TEXT").indexed(),
            ],
        );
        DynamicSchemaModule::new(td)
            .ensure_schema(&handle)
            .await
            .expect("ensure_schema");

        let tables = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='widget'",
                HashMap::new(),
            )
            .await
            .expect("query table presence");
        assert_eq!(
            tables.len(),
            1,
            "ensure_schema must create the `widget` table"
        );

        let indexes = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='widget'",
                HashMap::new(),
            )
            .await
            .expect("query index presence");
        assert!(
            !indexes.is_empty(),
            "ensure_schema must create the declared index on `widget`"
        );

        handle.shutdown().await.expect("shutdown");
    }
}
