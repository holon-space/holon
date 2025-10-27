use crate::storage::{Filter, Result, StorageEntity};
use async_trait::async_trait;
use holon_api::TypeDefinition;

#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Create a new entity table from its type definition.
    /// Uses interior mutability - implementations should be thread-safe.
    async fn create_entity(&self, type_def: &TypeDefinition) -> Result<()>;

    async fn get(&self, entity: &str, id: &str) -> Result<Option<StorageEntity>>;

    async fn query(&self, entity: &str, filter: Filter) -> Result<Vec<StorageEntity>>;

    /// Insert a new row. Uses interior mutability - implementations should be thread-safe.
    async fn insert(&self, schema: &TypeDefinition, data: StorageEntity) -> Result<()>;

    /// Update an existing row. Uses interior mutability - implementations should be thread-safe.
    async fn update(&self, schema: &TypeDefinition, id: &str, data: StorageEntity) -> Result<()>;

    /// Delete a row. Uses interior mutability - implementations should be thread-safe.
    async fn delete(&self, entity: &str, id: &str) -> Result<()>;

    async fn get_version(&self, entity: &str, id: &str) -> Result<Option<String>>;

    /// Set version for a row. Uses interior mutability - implementations should be thread-safe.
    async fn set_version(&self, entity: &str, id: &str, version: String) -> Result<()>;

    async fn get_children(
        &self,
        entity: &str,
        parent_field: &str,
        parent_id: &str,
    ) -> Result<Vec<StorageEntity>>;

    async fn get_related(
        &self,
        entity: &str,
        foreign_key: &str,
        related_id: &str,
    ) -> Result<Vec<StorageEntity>>;
}
