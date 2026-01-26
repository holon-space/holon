# Implementation Plan: Refactoring to `blocks` and `documents`

## Overview

Replace the org-mode-centric entities (`org_files`, `org_headlines`, `directories`) with format-agnostic entities (`blocks`, `documents`). Blocks already exist; this plan focuses on creating the `documents` entity and wiring everything together.

## Design Decisions (Confirmed)

1. **`id`** = UUID/ULID (unique identifier), **`name`** = slug/filename stem
2. **`parent_id`** is non-optional; root document has `parent_id = NO_PARENT_DOC_ID` (sentinel value `"__no_parent__"`)
3. **No `doc_id` on blocks** - derive document membership by traversing to root block
4. **Root document** created on first sync
5. **UNIQUE(parent_id, name)** constraint enforced
6. **Empty documents** can exist (Loro doc with no blocks, Org file with just ID property)
7. **Filesystem pattern**: `basename.org` + `basename/` folder for children
8. **No format/type fields** - serialization is adapter concern
9. **No file_hash** - FileWatcher handles change detection in-memory
10. **No _version/_dirty** - event-based CDC, not polling

## Existing Constants to Reuse

From `crates/holon-api/src/block.rs`:
```rust
pub const ROOT_PARENT_ID: &str = "__root_parent__";  // ID of root block
pub const NO_PARENT_ID: &str = "__no_parent__";      // Sentinel for root's parent
```

We will add document equivalents in the same file.

---

## Phase 1: Create Document Entity

### Task 1.1: Add Document constants to holon-api

**File**: `crates/holon-api/src/block.rs`

Add after line 12 (after `NO_PARENT_ID`):
```rust
/// ID of the root document in the document tree.
/// The root document represents the configured root directory (Org) or workspace (Loro).
pub const ROOT_DOC_ID: &str = "__root_doc__";

/// Sentinel value indicating a document has no parent (used for root document's parent_id).
/// This prevents the root document from forming a cycle with itself.
pub const NO_PARENT_DOC_ID: &str = "__no_parent__";  // Reuse same sentinel
```

**File**: `crates/holon-api/src/lib.rs`

Update the export (around line 12):
```rust
pub use block::{
    Block, BlockContent, BlockMetadata, BlockResult, BlockWithDepth, ResultOutput, SourceBlock,
    NO_PARENT_ID, ROOT_PARENT_ID, ROOT_DOC_ID, NO_PARENT_DOC_ID,
};
```

### Task 1.2: Create Document struct

**File**: `crates/holon/src/sync/document_entity.rs` (NEW FILE)

```rust
//! Document entity for the blocks/documents data model.
//!
//! Documents are containers for blocks. Each document maps to a file on disk
//! (e.g., `todo.org`) and can have child documents (stored in a folder with
//! the same name, e.g., `todo/`).

use holon_core::entity_derive::Entity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use holon_api::block::{NO_PARENT_DOC_ID, ROOT_DOC_ID};

/// A document in the hierarchical document tree.
///
/// Documents correspond to files on disk and contain blocks. The document
/// hierarchy mirrors the filesystem structure:
/// - Document "projects" → file `projects.org`, folder `projects/`
/// - Document "projects/todo" → file `projects/todo.org`
///
/// # Path Derivation
/// The filesystem path is derived from the document hierarchy:
/// - `name` provides the filename stem
/// - `parent_id` chain provides the directory path
///
/// # Root Document
/// The root document has:
/// - `id = ROOT_DOC_ID` ("__root_doc__")
/// - `parent_id = NO_PARENT_DOC_ID` ("__no_parent__")
/// - `name = ""` (empty, represents the configured root directory)
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "documents", short_name = "doc")]
pub struct Document {
    /// Unique identifier (UUID or ULID)
    #[primary_key]
    #[indexed]
    pub id: String,

    /// Parent document ID. Root document uses NO_PARENT_DOC_ID.
    #[indexed]
    pub parent_id: String,

    /// Display name / filename stem (e.g., "todo", "projects")
    /// Used to derive the filesystem path. Must be unique within parent.
    #[indexed]
    pub name: String,

    /// Fractional index for ordering within parent (lexicographic sort)
    /// Uses same algorithm as blocks (e.g., "a0", "a1", "a1V")
    pub sort_key: String,

    /// JSON-serialized metadata (title, todo_keywords, custom properties)
    /// Format: {"title": "My Document", "todo_keywords": "TODO DONE", ...}
    pub properties: String,

    /// Creation timestamp (Unix milliseconds)
    pub created_at: i64,

    /// Last update timestamp (Unix milliseconds)
    pub updated_at: i64,
}

impl Document {
    /// Create a new document with the given ID and name under the specified parent.
    pub fn new(id: String, parent_id: String, name: String) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id,
            parent_id,
            name,
            sort_key: "a0".to_string(),  // Default sort key
            properties: "{}".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Create the root document.
    pub fn root() -> Self {
        Self::new(
            ROOT_DOC_ID.to_string(),
            NO_PARENT_DOC_ID.to_string(),
            String::new(),  // Empty name for root
        )
    }

    /// Check if this is the root document.
    pub fn is_root(&self) -> bool {
        self.id == ROOT_DOC_ID
    }

    /// Get properties as a HashMap.
    pub fn properties_map(&self) -> HashMap<String, serde_json::Value> {
        serde_json::from_str(&self.properties).unwrap_or_default()
    }

    /// Set a property value.
    pub fn set_property(&mut self, key: &str, value: serde_json::Value) {
        let mut props = self.properties_map();
        props.insert(key.to_string(), value);
        self.properties = serde_json::to_string(&props).unwrap_or_default();
    }

    /// Get a property value.
    pub fn get_property(&self, key: &str) -> Option<serde_json::Value> {
        self.properties_map().get(key).cloned()
    }
}

/// Trait for stores that can resolve document paths.
pub trait DocumentPathResolver {
    /// Get a document by ID.
    fn get_document(&self, id: &str) -> Option<&Document>;
}

impl Document {
    /// Derive the filesystem path by walking up the parent chain.
    ///
    /// Returns path segments (e.g., ["projects", "todo"]) which can be
    /// joined with "/" and appended with ".org" for the file path.
    ///
    /// Returns None if any ancestor is missing (orphaned document).
    pub fn derive_path_segments<R: DocumentPathResolver>(&self, resolver: &R) -> Option<Vec<String>> {
        if self.is_root() {
            return Some(vec![]);
        }

        let mut segments = vec![self.name.clone()];
        let mut current_parent_id = &self.parent_id;

        while *current_parent_id != NO_PARENT_DOC_ID && *current_parent_id != ROOT_DOC_ID {
            let parent = resolver.get_document(current_parent_id)?;
            if !parent.name.is_empty() {
                segments.push(parent.name.clone());
            }
            current_parent_id = &parent.parent_id;
        }

        segments.reverse();
        Some(segments)
    }

    /// Derive the full path as a string (e.g., "projects/todo").
    pub fn derive_path<R: DocumentPathResolver>(&self, resolver: &R) -> Option<String> {
        self.derive_path_segments(resolver)
            .map(|segs| segs.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestResolver {
        docs: HashMap<String, Document>,
    }

    impl DocumentPathResolver for TestResolver {
        fn get_document(&self, id: &str) -> Option<&Document> {
            self.docs.get(id)
        }
    }

    #[test]
    fn test_root_document() {
        let root = Document::root();
        assert!(root.is_root());
        assert_eq!(root.parent_id, NO_PARENT_DOC_ID);
        assert_eq!(root.name, "");
    }

    #[test]
    fn test_derive_path() {
        let mut docs = HashMap::new();

        let root = Document::root();
        docs.insert(ROOT_DOC_ID.to_string(), root);

        let projects = Document::new(
            "doc-1".to_string(),
            ROOT_DOC_ID.to_string(),
            "projects".to_string(),
        );
        docs.insert("doc-1".to_string(), projects);

        let todo = Document::new(
            "doc-2".to_string(),
            "doc-1".to_string(),
            "todo".to_string(),
        );
        docs.insert("doc-2".to_string(), todo.clone());

        let resolver = TestResolver { docs };

        assert_eq!(todo.derive_path(&resolver), Some("projects/todo".to_string()));
    }

    #[test]
    fn test_properties() {
        let mut doc = Document::new(
            "doc-1".to_string(),
            ROOT_DOC_ID.to_string(),
            "test".to_string(),
        );

        doc.set_property("title", serde_json::json!("My Title"));
        doc.set_property("todo_keywords", serde_json::json!("TODO DONE"));

        assert_eq!(
            doc.get_property("title"),
            Some(serde_json::json!("My Title"))
        );
    }
}
```

### Task 1.3: Add Document to sync module exports

**File**: `crates/holon/src/sync/mod.rs`

Add:
```rust
mod document_entity;
pub use document_entity::{Document, DocumentPathResolver};
```

### Task 1.4: Create SQL schema for documents table

**File**: `crates/holon/src/sync/document_schema.sql` (NEW FILE - for reference/migrations)

```sql
-- Documents table schema
CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY,
    parent_id TEXT NOT NULL,
    name TEXT NOT NULL,
    sort_key TEXT NOT NULL,
    properties TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,

    -- Uniqueness constraint: no two documents with same name under same parent
    UNIQUE(parent_id, name)
);

-- Indexes for efficient queries
CREATE INDEX IF NOT EXISTS idx_documents_parent_id ON documents(parent_id);
CREATE INDEX IF NOT EXISTS idx_documents_name ON documents(name);

-- Insert root document if not exists
INSERT OR IGNORE INTO documents (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ('__root_doc__', '__no_parent__', '', 'a0', '{}', 0, 0);
```

---

## Phase 2: Create DocumentOperations

### Task 2.1: Create DocumentOperations struct

**File**: `crates/holon/src/sync/document_operations.rs` (NEW FILE)

```rust
//! Operations provider for the documents entity.
//!
//! Provides CRUD operations and document-specific operations like rename and move.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use holon_api::block::{NO_PARENT_DOC_ID, ROOT_DOC_ID};
use holon_api::Value;

use crate::api::TursoBackend;
use crate::core::datasource::{OperationDescriptor, OperationProvider};
use crate::core::queryable_cache::QueryableCache;
use crate::sync::document_entity::Document;

/// Operations provider for the `documents` entity.
pub struct DocumentOperations {
    backend: Arc<RwLock<TursoBackend>>,
    cache: Arc<QueryableCache<Document>>,
}

impl DocumentOperations {
    /// Create a new DocumentOperations instance.
    pub fn new(backend: Arc<RwLock<TursoBackend>>, cache: Arc<QueryableCache<Document>>) -> Self {
        Self { backend, cache }
    }

    /// Initialize the documents table schema.
    pub async fn init_schema(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let backend = self.backend.read().await;

        // Create table
        backend.execute_sql(
            "CREATE TABLE IF NOT EXISTS documents (
                id TEXT PRIMARY KEY,
                parent_id TEXT NOT NULL,
                name TEXT NOT NULL,
                sort_key TEXT NOT NULL,
                properties TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(parent_id, name)
            )",
            HashMap::new(),
        ).await?;

        // Create indexes
        backend.execute_sql(
            "CREATE INDEX IF NOT EXISTS idx_documents_parent_id ON documents(parent_id)",
            HashMap::new(),
        ).await?;

        backend.execute_sql(
            "CREATE INDEX IF NOT EXISTS idx_documents_name ON documents(name)",
            HashMap::new(),
        ).await?;

        // Insert root document if not exists
        let root = Document::root();
        backend.execute_sql(
            "INSERT OR IGNORE INTO documents (id, parent_id, name, sort_key, properties, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            [
                ("1".to_string(), Value::String(root.id)),
                ("2".to_string(), Value::String(root.parent_id)),
                ("3".to_string(), Value::String(root.name)),
                ("4".to_string(), Value::String(root.sort_key)),
                ("5".to_string(), Value::String(root.properties)),
                ("6".to_string(), Value::Integer(root.created_at)),
                ("7".to_string(), Value::Integer(root.updated_at)),
            ].into_iter().collect(),
        ).await?;

        Ok(())
    }

    /// Get a document by ID.
    pub async fn get_by_id(&self, id: &str) -> Result<Option<Document>, Box<dyn std::error::Error + Send + Sync>> {
        // Try cache first
        if let Some(doc) = self.cache.get(id).await {
            return Ok(Some(doc));
        }

        // Fall back to database
        let backend = self.backend.read().await;
        let rows = backend.execute_sql(
            "SELECT * FROM documents WHERE id = ?1",
            [("1".to_string(), Value::String(id.to_string()))].into_iter().collect(),
        ).await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let doc = Self::row_to_document(&rows[0])?;
        Ok(Some(doc))
    }

    /// Get all child documents of a parent.
    pub async fn get_children(&self, parent_id: &str) -> Result<Vec<Document>, Box<dyn std::error::Error + Send + Sync>> {
        let backend = self.backend.read().await;
        let rows = backend.execute_sql(
            "SELECT * FROM documents WHERE parent_id = ?1 ORDER BY sort_key",
            [("1".to_string(), Value::String(parent_id.to_string()))].into_iter().collect(),
        ).await?;

        rows.iter().map(Self::row_to_document).collect()
    }

    /// Create a new document.
    pub async fn create(&self, doc: Document) -> Result<Document, Box<dyn std::error::Error + Send + Sync>> {
        // Validate parent exists (unless it's root or root's parent)
        if doc.parent_id != NO_PARENT_DOC_ID && doc.parent_id != ROOT_DOC_ID {
            if self.get_by_id(&doc.parent_id).await?.is_none() {
                return Err(format!("Parent document '{}' not found", doc.parent_id).into());
            }
        }

        let backend = self.backend.read().await;
        backend.execute_sql(
            "INSERT INTO documents (id, parent_id, name, sort_key, properties, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            [
                ("1".to_string(), Value::String(doc.id.clone())),
                ("2".to_string(), Value::String(doc.parent_id.clone())),
                ("3".to_string(), Value::String(doc.name.clone())),
                ("4".to_string(), Value::String(doc.sort_key.clone())),
                ("5".to_string(), Value::String(doc.properties.clone())),
                ("6".to_string(), Value::Integer(doc.created_at)),
                ("7".to_string(), Value::Integer(doc.updated_at)),
            ].into_iter().collect(),
        ).await?;

        Ok(doc)
    }

    /// Update a document.
    pub async fn update(&self, doc: &Document) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let backend = self.backend.read().await;
        let now = chrono::Utc::now().timestamp_millis();

        backend.execute_sql(
            "UPDATE documents SET parent_id = ?2, name = ?3, sort_key = ?4, properties = ?5, updated_at = ?6
             WHERE id = ?1",
            [
                ("1".to_string(), Value::String(doc.id.clone())),
                ("2".to_string(), Value::String(doc.parent_id.clone())),
                ("3".to_string(), Value::String(doc.name.clone())),
                ("4".to_string(), Value::String(doc.sort_key.clone())),
                ("5".to_string(), Value::String(doc.properties.clone())),
                ("6".to_string(), Value::Integer(now)),
            ].into_iter().collect(),
        ).await?;

        Ok(())
    }

    /// Delete a document by ID.
    pub async fn delete(&self, id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if id == ROOT_DOC_ID {
            return Err("Cannot delete root document".into());
        }

        let backend = self.backend.read().await;
        backend.execute_sql(
            "DELETE FROM documents WHERE id = ?1",
            [("1".to_string(), Value::String(id.to_string()))].into_iter().collect(),
        ).await?;

        Ok(())
    }

    /// Find a document by parent_id and name.
    pub async fn find_by_parent_and_name(&self, parent_id: &str, name: &str) -> Result<Option<Document>, Box<dyn std::error::Error + Send + Sync>> {
        let backend = self.backend.read().await;
        let rows = backend.execute_sql(
            "SELECT * FROM documents WHERE parent_id = ?1 AND name = ?2",
            [
                ("1".to_string(), Value::String(parent_id.to_string())),
                ("2".to_string(), Value::String(name.to_string())),
            ].into_iter().collect(),
        ).await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let doc = Self::row_to_document(&rows[0])?;
        Ok(Some(doc))
    }

    /// Convert a database row to a Document.
    fn row_to_document(row: &HashMap<String, Value>) -> Result<Document, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Document {
            id: row.get("id").and_then(|v| v.as_string()).ok_or("Missing id")?.to_string(),
            parent_id: row.get("parent_id").and_then(|v| v.as_string()).ok_or("Missing parent_id")?.to_string(),
            name: row.get("name").and_then(|v| v.as_string()).ok_or("Missing name")?.to_string(),
            sort_key: row.get("sort_key").and_then(|v| v.as_string()).unwrap_or("a0").to_string(),
            properties: row.get("properties").and_then(|v| v.as_string()).unwrap_or("{}").to_string(),
            created_at: row.get("created_at").and_then(|v| v.as_integer()).unwrap_or(0),
            updated_at: row.get("updated_at").and_then(|v| v.as_integer()).unwrap_or(0),
        })
    }
}

#[async_trait]
impl OperationProvider for DocumentOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        Document::all_operations()
    }

    async fn execute(
        &self,
        operation: &str,
        params: HashMap<String, Value>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        match operation {
            "get_by_id" => {
                let id = params.get("id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'id' parameter")?;

                match self.get_by_id(id).await? {
                    Some(doc) => Ok(Value::String(serde_json::to_string(&doc)?)),
                    None => Ok(Value::Null),
                }
            }
            "create" => {
                let id = params.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let parent_id = params.get("parent_id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'parent_id' parameter")?
                    .to_string();
                let name = params.get("name")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'name' parameter")?
                    .to_string();

                let mut doc = Document::new(id, parent_id, name);

                if let Some(sort_key) = params.get("sort_key").and_then(|v| v.as_string()) {
                    doc.sort_key = sort_key.to_string();
                }
                if let Some(props) = params.get("properties").and_then(|v| v.as_string()) {
                    doc.properties = props.to_string();
                }

                let created = self.create(doc).await?;
                Ok(Value::String(serde_json::to_string(&created)?))
            }
            "update" => {
                let id = params.get("id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'id' parameter")?;

                let mut doc = self.get_by_id(id).await?
                    .ok_or_else(|| format!("Document '{}' not found", id))?;

                if let Some(name) = params.get("name").and_then(|v| v.as_string()) {
                    doc.name = name.to_string();
                }
                if let Some(parent_id) = params.get("parent_id").and_then(|v| v.as_string()) {
                    doc.parent_id = parent_id.to_string();
                }
                if let Some(sort_key) = params.get("sort_key").and_then(|v| v.as_string()) {
                    doc.sort_key = sort_key.to_string();
                }
                if let Some(props) = params.get("properties").and_then(|v| v.as_string()) {
                    doc.properties = props.to_string();
                }

                self.update(&doc).await?;
                Ok(Value::String(serde_json::to_string(&doc)?))
            }
            "delete" => {
                let id = params.get("id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'id' parameter")?;

                self.delete(id).await?;
                Ok(Value::Bool(true))
            }
            "get_children" => {
                let parent_id = params.get("parent_id")
                    .and_then(|v| v.as_string())
                    .ok_or("Missing 'parent_id' parameter")?;

                let children = self.get_children(parent_id).await?;
                Ok(Value::String(serde_json::to_string(&children)?))
            }
            _ => Err(format!("Unknown operation: {}", operation).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Add tests here using test infrastructure
}
```

### Task 2.2: Add DocumentOperations to sync module exports

**File**: `crates/holon/src/sync/mod.rs`

Add:
```rust
mod document_operations;
pub use document_operations::DocumentOperations;
```

---

## Phase 3: Wire into Dependency Injection

### Task 3.1: Update OrgModeModule to register Document services

**File**: `crates/holon-orgmode/src/di.rs`

Add import at top:
```rust
use holon::sync::{Document, DocumentOperations};
```

Inside `register_services`, add after `QueryableCache<LoroBlock>` registration (around line 90):

```rust
// Register QueryableCache for Documents
services.add_singleton_factory::<QueryableCache<Document>, _>(|r| {
    holon::di::create_queryable_cache(r)
});

// Register DocumentOperations
services.add_singleton_factory::<DocumentOperations, _>(|resolver| {
    let backend_provider = resolver.get_required_trait::<dyn holon::di::TursoBackendProvider>();
    let backend = backend_provider.backend();
    let cache = resolver.get_required::<QueryableCache<Document>>();

    let ops = DocumentOperations::new(backend, cache);

    // Initialize schema synchronously
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            ops.init_schema().await.expect("Failed to initialize documents schema");
        })
    });

    ops
});

// Register DocumentOperations as OperationProvider for "documents" entity
services.add_trait_factory::<dyn OperationProvider, _>(Lifetime::Singleton, |resolver| {
    let doc_ops = resolver.get_required::<DocumentOperations>();
    doc_ops as Arc<dyn OperationProvider>
});
```

---

## Phase 4: Update OrgAdapter to Use Documents

### Task 4.1: Add document creation to OrgAdapter

**File**: `crates/holon-orgmode/src/orgmode_adapter.rs`

The OrgAdapter needs to:
1. Create/find documents when processing org files
2. Associate blocks with documents (via root block convention)
3. Handle renames and moves

Add imports:
```rust
use holon::sync::{Document, DocumentOperations};
use holon_api::block::{ROOT_DOC_ID, NO_PARENT_DOC_ID};
```

Add field to `OrgAdapter`:
```rust
pub struct OrgAdapter {
    command_bus: Arc<dyn OperationProvider>,
    doc_ops: Arc<DocumentOperations>,  // NEW
    write_tracker: Arc<RwLock<WriteTracker>>,
}
```

Add method for document resolution:
```rust
impl OrgAdapter {
    /// Get or create a document for the given file path.
    ///
    /// Path is relative to root (e.g., "projects/todo.org").
    /// Creates parent documents as needed.
    async fn get_or_create_document(&self, rel_path: &Path) -> Result<Document> {
        // Strip .org extension to get the document path
        let doc_path = rel_path.with_extension("");
        let segments: Vec<&str> = doc_path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();

        if segments.is_empty() {
            // Root document
            return self.doc_ops.get_by_id(ROOT_DOC_ID).await?
                .ok_or_else(|| anyhow::anyhow!("Root document not found"));
        }

        let mut current_parent_id = ROOT_DOC_ID.to_string();
        let mut current_doc: Option<Document> = None;

        for segment in &segments {
            // Check if document exists under current parent
            if let Some(existing) = self.doc_ops.find_by_parent_and_name(&current_parent_id, segment).await? {
                current_parent_id = existing.id.clone();
                current_doc = Some(existing);
            } else {
                // Create new document
                let new_doc = Document::new(
                    uuid::Uuid::new_v4().to_string(),
                    current_parent_id.clone(),
                    segment.to_string(),
                );
                let created = self.doc_ops.create(new_doc).await?;
                current_parent_id = created.id.clone();
                current_doc = Some(created);
            }
        }

        current_doc.ok_or_else(|| anyhow::anyhow!("Failed to resolve document"))
    }
}
```

Update `on_file_changed` to use documents:
```rust
pub async fn on_file_changed(&self, file_path: &Path) -> Result<()> {
    // ... existing code ...

    // Get or create document for this file
    let rel_path = file_path.strip_prefix(&self.root_dir)?;
    let document = self.get_or_create_document(rel_path).await?;

    // When creating root blocks, use a convention to link to document
    // The root block for a document has id = "doc:{doc_id}:root"
    let doc_root_block_id = format!("doc:{}:root", document.id);

    // ... rest of block creation uses doc_root_block_id as parent for top-level blocks ...
}
```

### Task 4.2: Add helper to find document for a block

**File**: `crates/holon/src/sync/document_entity.rs`

Add function to derive document from block hierarchy:
```rust
/// Find the document ID for a block by traversing to its root.
///
/// Convention: Root blocks for a document have ID pattern "doc:{doc_id}:root"
pub fn document_id_from_block_root(root_block_id: &str) -> Option<String> {
    if root_block_id.starts_with("doc:") && root_block_id.ends_with(":root") {
        let inner = root_block_id.strip_prefix("doc:")?.strip_suffix(":root")?;
        Some(inner.to_string())
    } else {
        None
    }
}

/// Check if a block ID is a document root block.
pub fn is_document_root_block(block_id: &str) -> bool {
    block_id.starts_with("doc:") && block_id.ends_with(":root")
}
```

---

## Phase 5: Update FileWatcher for Hash-Based Change Detection

### Task 5.1: Add in-memory hash tracking to FileWatcher

**File**: `crates/holon-orgmode/src/file_watcher.rs`

Add field and methods:
```rust
use std::collections::HashMap;
use sha2::{Sha256, Digest};

pub struct OrgFileWatcher {
    watcher: RecommendedWatcher,
    sender: mpsc::Sender<PathBuf>,
    known_hashes: Arc<RwLock<HashMap<PathBuf, String>>>,  // NEW
}

impl OrgFileWatcher {
    /// Compute SHA256 hash of file contents.
    fn hash_file(path: &Path) -> std::io::Result<String> {
        let contents = std::fs::read(path)?;
        let mut hasher = Sha256::new();
        hasher.update(&contents);
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Check if file content actually changed (not just metadata/touch).
    pub async fn content_changed(&self, path: &Path) -> bool {
        let current_hash = match Self::hash_file(path) {
            Ok(h) => h,
            Err(_) => return true,  // Assume changed if we can't read
        };

        let known = self.known_hashes.read().await.get(path).cloned();

        if Some(&current_hash) != known.as_ref() {
            self.known_hashes.write().await.insert(path.to_path_buf(), current_hash);
            true
        } else {
            false
        }
    }

    /// Update known hash after writing a file (to prevent echo events).
    pub async fn update_hash(&self, path: &Path) {
        if let Ok(hash) = Self::hash_file(path) {
            self.known_hashes.write().await.insert(path.to_path_buf(), hash);
        }
    }
}
```

### Task 5.2: Update OrgFileWriter to notify FileWatcher after writes

**File**: `crates/holon-orgmode/src/orgmode_file_writer.rs`

After writing a file, call `file_watcher.update_hash(path)` to prevent the write from triggering a change event.

---

## Phase 6: Migration from Old Entities

### Task 6.1: Create migration helper

**File**: `crates/holon-orgmode/src/migration.rs` (NEW FILE)

```rust
//! Migration utilities for converting org_files/org_headlines to documents/blocks.

use std::collections::HashMap;
use std::sync::Arc;

use crate::models::{OrgFile, OrgHeadline};
use holon::sync::{Document, DocumentOperations};
use holon_api::block::ROOT_DOC_ID;

/// Migrate org_files to documents.
pub async fn migrate_org_files_to_documents(
    org_files: Vec<OrgFile>,
    doc_ops: Arc<DocumentOperations>,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut id_mapping: HashMap<String, String> = HashMap::new();

    // Sort by depth to ensure parents are created first
    let mut sorted_files = org_files;
    sorted_files.sort_by_key(|f| f.depth);

    for org_file in sorted_files {
        // Map parent_id
        let new_parent_id = if org_file.parent_id.is_empty() || org_file.depth == 0 {
            ROOT_DOC_ID.to_string()
        } else {
            id_mapping.get(&org_file.parent_id)
                .cloned()
                .unwrap_or_else(|| ROOT_DOC_ID.to_string())
        };

        // Create document
        let doc = Document::new(
            uuid::Uuid::new_v4().to_string(),
            new_parent_id,
            org_file.name.trim_end_matches(".org").to_string(),
        );

        // Copy properties
        let mut new_doc = doc;
        if let Some(title) = org_file.title {
            new_doc.set_property("title", serde_json::json!(title));
        }
        if let Some(keywords) = org_file.todo_keywords {
            new_doc.set_property("todo_keywords", serde_json::json!(keywords));
        }

        let created = doc_ops.create(new_doc).await?;
        id_mapping.insert(org_file.id, created.id);
    }

    Ok(id_mapping)
}
```

### Task 6.2: Deprecation markers for old entities

**File**: `crates/holon-orgmode/src/models.rs`

Add deprecation notice:
```rust
/// DEPRECATED: Use `holon::sync::Document` instead.
/// This struct is kept for migration compatibility only.
#[deprecated(since = "0.2.0", note = "Use holon::sync::Document instead")]
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "org_files", short_name = "file")]
pub struct OrgFile {
    // ... existing fields ...
}
```

---

## Phase 7: Update Tests

### Task 7.1: Add document entity tests

**File**: `crates/holon/tests/document_entity_test.rs` (NEW FILE)

```rust
//! Integration tests for the Document entity.

use holon::sync::{Document, DocumentOperations};
use holon_api::block::{ROOT_DOC_ID, NO_PARENT_DOC_ID};
// ... test infrastructure imports ...

#[tokio::test]
async fn test_document_crud() {
    // Setup test backend
    // ...

    // Create document
    let doc = Document::new(
        "test-doc-1".to_string(),
        ROOT_DOC_ID.to_string(),
        "my-document".to_string(),
    );

    // Verify create
    // Verify read
    // Verify update
    // Verify delete
}

#[tokio::test]
async fn test_document_hierarchy() {
    // Create parent
    // Create child
    // Verify derive_path returns correct path
}

#[tokio::test]
async fn test_unique_name_constraint() {
    // Try to create two documents with same parent+name
    // Verify error
}

#[tokio::test]
async fn test_cannot_delete_root() {
    // Try to delete ROOT_DOC_ID
    // Verify error
}
```

### Task 7.2: Update existing tests to use documents

Review and update tests in:
- `crates/holon-integration-tests/tests/general_e2e_pbt.rs`
- `crates/holon/tests/json_aggregation_test.rs`
- `crates/holon/tests/json_aggregation_e2e_test.rs`

---

## Phase 8: Cleanup (After Verification)

### Task 8.1: Remove deprecated code

After migration is verified:

1. Remove `OrgFile`, `OrgHeadline` from `crates/holon-orgmode/src/models.rs`
2. Remove `Directory` from `crates/holon-filesystem/src/directory.rs`
3. Remove `OrgFileOperations`, `OrgHeadlineOperations` from `crates/holon-orgmode/src/orgmode_datasource.rs`
4. Remove old QueryableCache registrations from DI
5. Clean up unused imports

### Task 8.2: Update documentation

- Update README files
- Update any architecture docs
- Update API documentation

---

## File Summary

### New Files to Create:
1. `crates/holon/src/sync/document_entity.rs` - Document struct
2. `crates/holon/src/sync/document_operations.rs` - CRUD operations
3. `crates/holon/src/sync/document_schema.sql` - Reference schema
4. `crates/holon-orgmode/src/migration.rs` - Migration helpers
5. `crates/holon/tests/document_entity_test.rs` - Tests

### Files to Modify:
1. `crates/holon-api/src/block.rs` - Add document constants
2. `crates/holon-api/src/lib.rs` - Export new constants
3. `crates/holon/src/sync/mod.rs` - Export Document, DocumentOperations
4. `crates/holon-orgmode/src/di.rs` - Register Document services
5. `crates/holon-orgmode/src/orgmode_adapter.rs` - Use documents
6. `crates/holon-orgmode/src/file_watcher.rs` - Add hash tracking
7. `crates/holon-orgmode/src/orgmode_file_writer.rs` - Notify hash updates
8. `crates/holon-orgmode/src/models.rs` - Deprecation markers

### Files to Eventually Remove (Phase 8):
1. `OrgFile`, `OrgHeadline` structs in `models.rs`
2. `Directory` struct in `directory.rs`
3. Old operation providers in `orgmode_datasource.rs`

---

## Execution Order

1. **Phase 1** - Create Document entity (can be done independently)
2. **Phase 2** - Create DocumentOperations (depends on Phase 1)
3. **Phase 3** - Wire into DI (depends on Phase 2)
4. **Phase 4** - Update OrgAdapter (depends on Phase 3)
5. **Phase 5** - Update FileWatcher (can be done in parallel with Phase 4)
6. **Phase 7** - Add tests (can be started after Phase 2, expanded as phases complete)
7. **Phase 6** - Migration helpers (after Phases 1-5 working)
8. **Phase 8** - Cleanup (after everything verified)

---

## Verification Checklist

After each phase, verify:

- [ ] Tests pass: `cargo test -p holon`
- [ ] Tests pass: `cargo test -p holon-orgmode`
- [ ] No compiler warnings for new code
- [ ] MCP interface still works: `list_operations` shows `documents` entity
- [ ] Org files sync correctly (create, update, delete)
- [ ] Documents table has correct schema in database
- [ ] Root document exists after first sync
