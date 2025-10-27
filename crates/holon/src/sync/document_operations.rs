//! Operations provider for the documents entity.
//!
//! Provides CRUD operations and document-specific operations like rename and move.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::document::{no_parent_doc_id, root_doc_id};

use crate::core::datasource::{
    DataSource, OperationDescriptor, OperationProvider, OperationRegistry, OperationResult,
};
use crate::core::queryable_cache::QueryableCache;
use crate::core::traits::Result;
use crate::storage::DbHandle;
use crate::storage::types::StorageEntity;
use crate::sync::document_entity::Document;

/// Operations provider for the `documents` entity.
pub struct DocumentOperations {
    db_handle: DbHandle,
    cache: Arc<QueryableCache<Document>>,
}

impl DocumentOperations {
    /// Create a new DocumentOperations instance.
    pub fn new(db_handle: DbHandle, cache: Arc<QueryableCache<Document>>) -> Self {
        Self { db_handle, cache }
    }

    /// Initialize the documents table data (schema is created by CoreSchemaModule).
    pub async fn init_schema(&self) -> Result<()> {
        // Insert root document if not exists
        let root = Document::root();
        let mut params = HashMap::new();
        params.insert("id".to_string(), root.id.clone().into());
        params.insert("parent_id".to_string(), root.parent_id.clone().into());
        params.insert("name".to_string(), Value::String(root.name.clone()));
        params.insert("sort_key".to_string(), Value::String(root.sort_key.clone()));
        params.insert(
            "properties".to_string(),
            Value::from(root.properties.clone()),
        );
        params.insert("created_at".to_string(), Value::Integer(root.created_at));
        params.insert("updated_at".to_string(), Value::Integer(root.updated_at));

        self.db_handle
            .query(
                include_str!("../../sql/documents/insert_or_ignore.sql"),
                params,
            )
            .await?;

        Ok(())
    }

    /// Get a document by ID.
    pub async fn get_by_id(&self, id: &EntityUri) -> Result<Option<Document>> {
        // Try cache first
        if let Ok(Some(doc)) = DataSource::get_by_id(&*self.cache, id.as_str()).await {
            return Ok(Some(doc));
        }

        // Fall back to database
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        let rows = self
            .db_handle
            .query(include_str!("../../sql/documents/get_by_id.sql"), params)
            .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let doc = Self::row_to_document(&rows[0])?;
        Ok(Some(doc))
    }

    /// Get all child documents of a parent.
    pub async fn get_children(&self, parent_id: &EntityUri) -> Result<Vec<Document>> {
        let mut params = HashMap::new();
        params.insert(
            "parent_id".to_string(),
            Value::String(parent_id.to_string()),
        );
        let rows = self
            .db_handle
            .query(include_str!("../../sql/documents/get_children.sql"), params)
            .await?;

        rows.iter().map(Self::row_to_document).collect()
    }

    /// Create a new document.
    pub async fn create(&self, doc: Document) -> Result<Document> {
        // Validate parent exists (unless it's root or root's parent)
        if doc.parent_id != no_parent_doc_id() && doc.parent_id != root_doc_id() {
            if self.get_by_id(&doc.parent_id).await?.is_none() {
                return Err(format!("Parent document '{}' not found", doc.parent_id).into());
            }
        }

        let mut params = HashMap::new();
        params.insert("id".to_string(), doc.id.clone().into());
        params.insert("parent_id".to_string(), doc.parent_id.clone().into());
        params.insert("name".to_string(), Value::String(doc.name.clone()));
        params.insert("sort_key".to_string(), Value::String(doc.sort_key.clone()));
        params.insert(
            "properties".to_string(),
            Value::from(doc.properties.clone()),
        );
        params.insert("created_at".to_string(), Value::Integer(doc.created_at));
        params.insert("updated_at".to_string(), Value::Integer(doc.updated_at));

        self.db_handle
            .query(include_str!("../../sql/documents/insert.sql"), params)
            .await?;

        Ok(doc)
    }

    /// Update a document.
    pub async fn update(&self, doc: &Document) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();

        let mut params = HashMap::new();
        params.insert("id".to_string(), doc.id.clone().into());
        params.insert("parent_id".to_string(), doc.parent_id.clone().into());
        params.insert("name".to_string(), Value::String(doc.name.clone()));
        params.insert("sort_key".to_string(), Value::String(doc.sort_key.clone()));
        params.insert(
            "properties".to_string(),
            Value::from(doc.properties.clone()),
        );
        params.insert("updated_at".to_string(), Value::Integer(now));

        self.db_handle
            .query(include_str!("../../sql/documents/update.sql"), params)
            .await?;

        Ok(())
    }

    /// Delete a document by ID.
    pub async fn delete(&self, id: &EntityUri) -> Result<()> {
        if *id == root_doc_id() {
            return Err("Cannot delete root document".into());
        }

        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        self.db_handle
            .query(include_str!("../../sql/documents/delete.sql"), params)
            .await?;

        Ok(())
    }

    /// Find a document by parent_id and name.
    pub async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        name: &str,
    ) -> Result<Option<Document>> {
        let mut params = HashMap::new();
        params.insert(
            "parent_id".to_string(),
            Value::String(parent_id.to_string()),
        );
        params.insert("name".to_string(), Value::String(name.to_string()));
        let rows = self
            .db_handle
            .query(include_str!("../../sql/documents/find_by_name.sql"), params)
            .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let doc = Self::row_to_document(&rows[0])?;
        Ok(Some(doc))
    }

    /// Convert a database row to a Document.
    pub fn row_to_document(row: &HashMap<String, Value>) -> Result<Document> {
        // Validate required fields explicitly
        let id_str = row.get("id").and_then(|v| v.as_string()).ok_or_else(|| {
            format!(
                "Document row missing required 'id' field. Row keys: {:?}",
                row.keys().collect::<Vec<_>>()
            )
        })?;
        let id = EntityUri::parse(id_str)
            .map_err(|e| format!("Document row has invalid 'id' URI '{}': {}", id_str, e))?;

        let parent_id_str = row
            .get("parent_id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| {
                format!(
                    "Document row missing required 'parent_id' field for id='{}'. Row keys: {:?}",
                    id,
                    row.keys().collect::<Vec<_>>()
                )
            })?;
        let parent_id = EntityUri::parse(parent_id_str).map_err(|e| {
            format!(
                "Document row has invalid 'parent_id' URI '{}' for id='{}': {}",
                parent_id_str, id, e
            )
        })?;

        let name = row
            .get("name")
            .and_then(|v| v.as_string())
            .ok_or_else(|| {
                format!(
                    "Document row missing required 'name' field for id='{}'. Row keys: {:?}",
                    id,
                    row.keys().collect::<Vec<_>>()
                )
            })?
            .to_string();

        // Optional fields with defaults
        let sort_key = row
            .get("sort_key")
            .and_then(|v| v.as_string())
            .unwrap_or("a0")
            .to_string();

        let properties: HashMap<String, Value> = match row.get("properties") {
            Some(Value::Object(obj)) => obj.clone(),
            Some(Value::String(s)) => {
                // Handle legacy JSON string format from database
                serde_json::from_str::<serde_json::Value>(s)
                    .ok()
                    .and_then(|json_val| {
                        if let serde_json::Value::Object(map) = json_val {
                            Some(
                                map.into_iter()
                                    .map(|(k, v)| (k, Value::from_json_value(v)))
                                    .collect(),
                            )
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default()
            }
            Some(Value::Null) | None => HashMap::new(),
            Some(other) => {
                panic!("Unexpected type for 'properties' field: {:?}", other);
            }
        };

        let created_at = row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);

        let updated_at = row.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0);

        Ok(Document {
            id,
            parent_id,
            name,
            sort_key,
            properties,
            created_at,
            updated_at,
        })
    }
}

#[async_trait]
impl OperationProvider for DocumentOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        Document::all_operations()
    }

    async fn execute_operation(
        &self,
        entity_name: &str,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        use crate::core::datasource::OperationRegistry;
        let expected_entity_name = Document::entity_name();
        if entity_name != expected_entity_name {
            return Err(format!(
                "Expected entity_name '{}', got '{}'",
                expected_entity_name, entity_name
            )
            .into());
        }

        match op_name {
            "get_by_id" => {
                let id_str = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'id' parameter")?;
                let id = EntityUri::parse(id_str)
                    .map_err(|e| format!("Invalid 'id' URI '{}': {}", id_str, e))?;

                self.get_by_id(&id).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            "get_children" => {
                let parent_id_str = params
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'parent_id' parameter")?;
                let parent_id = EntityUri::parse(parent_id_str)
                    .map_err(|e| format!("Invalid 'parent_id' URI '{}': {}", parent_id_str, e))?;

                self.get_children(&parent_id).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            "find_by_parent_and_name" => {
                let parent_id_str = params
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'parent_id' parameter")?;
                let parent_id = EntityUri::parse(parent_id_str)
                    .map_err(|e| format!("Invalid 'parent_id' URI '{}': {}", parent_id_str, e))?;
                let name = params
                    .get("name")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'name' parameter")?;

                self.find_by_parent_and_name(&parent_id, name).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            "create" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| {
                        EntityUri::parse(s)
                            .unwrap_or_else(|e| panic!("Invalid 'id' URI '{}': {}", s, e))
                    })
                    .unwrap_or_else(EntityUri::doc_random);
                let parent_id_str = params
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'parent_id' parameter")?;
                let parent_id = EntityUri::parse(parent_id_str)
                    .map_err(|e| format!("Invalid 'parent_id' URI '{}': {}", parent_id_str, e))?;
                let name = params
                    .get("name")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'name' parameter")?
                    .to_string();

                let mut doc = Document::new(id, parent_id, name);

                if let Some(sort_key) = params.get("sort_key").and_then(|v| v.as_string()) {
                    doc.sort_key = sort_key.to_string();
                }
                if let Some(props) = params.get("properties") {
                    let map: HashMap<String, Value> = props
                        .clone()
                        .try_into()
                        .expect("properties must be a JSON object");
                    doc.properties = map;
                }

                self.create(doc).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            "set_field" => {
                let id_str = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'id' parameter")?;
                let id = EntityUri::parse(id_str)
                    .map_err(|e| format!("Invalid 'id' URI '{}': {}", id_str, e))?;
                let field = params
                    .get("field")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'field' parameter")?;
                let value = params.get("value").ok_or("Missing 'value' parameter")?;

                let mut doc = self
                    .get_by_id(&id)
                    .await?
                    .ok_or_else(|| format!("Document '{}' not found", id))?;

                match &field[..] {
                    "name" => {
                        doc.name = value
                            .as_string()
                            .ok_or("name must be a string")?
                            .to_string();
                    }
                    "parent_id" => {
                        let pid_str = value.as_string().ok_or("parent_id must be a string")?;
                        doc.parent_id = EntityUri::parse(pid_str)
                            .map_err(|e| format!("Invalid parent_id URI '{}': {}", pid_str, e))?;
                    }
                    "sort_key" => {
                        doc.sort_key = value
                            .as_string()
                            .ok_or("sort_key must be a string")?
                            .to_string();
                    }
                    "properties" => {
                        doc.properties = value.clone().try_into().map_err(
                            |e: Box<dyn std::error::Error + Send + Sync>| {
                                format!("properties must be a JSON object: {}", e)
                            },
                        )?;
                    }
                    _ => return Err(format!("Unknown field: {}", field).into()),
                }

                self.update(&doc).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            "delete" => {
                let id_str = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'id' parameter")?;
                let id = EntityUri::parse(id_str)
                    .map_err(|e| format!("Invalid 'id' URI '{}': {}", id_str, e))?;

                self.delete(&id).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            _ => Err(format!("Unknown operation: {}", op_name).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    // Add tests here using test infrastructure
}
