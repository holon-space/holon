# Loro Block Operations - Corrected Architecture Handoff

## Goal

Create generic `LoroBlockOperations` that operate directly on Loro, with org-mode as just one persistence adapter.

## Architecture

```
                    ┌─────────────────────────┐
                    │   UI / API Consumer     │
                    └───────────┬─────────────┘
                                │
                                ▼
                    ┌─────────────────────────┐
                    │   LoroBlockOperations   │
                    │  (CrudOperations,       │
                    │   TaskOperations,       │
                    │   OperationProvider)    │
                    └───────────┬─────────────┘
                                │
                                ▼
                    ┌─────────────────────────┐
                    │      LoroBackend        │
                    │   (CoreOperations)      │
                    └───────────┬─────────────┘
                                │
                                ▼
                    ┌─────────────────────────┐
                    │   LoroDocumentStore     │
                    │   (multiple LoroDoc)    │
                    └───────────┬─────────────┘
                                │
              ┌─────────────────┼─────────────────┐
              ▼                 ▼                 ▼
    ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
    │  OrgRenderer    │ │  .loro files    │ │  Future: Cloud  │
    │  (org files)    │ │  (snapshots)    │ │  (P2P sync)     │
    └─────────────────┘ └─────────────────┘ └─────────────────┘
```

## Files to Create

### 1. `crates/holon/src/sync/loro_block_entity.rs` (NEW)

Flattened entity for QueryableCache:

```rust
//! Flattened Block entity for QueryableCache storage.

use holon_macros::Entity;
use serde::{Deserialize, Serialize};
use holon_api::block::{Block, BlockContent, BlockMetadata, SourceBlock};
use std::collections::HashMap;

/// Flattened block entity for database storage.
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "blocks", short_name = "block")]
pub struct LoroBlock {
    #[primary_key]
    #[indexed]
    pub id: String,

    #[indexed]
    pub parent_id: String,

    /// Text content (raw text or source code)
    pub content: String,

    /// Content type: "text" or "source"
    pub content_type: String,

    /// For source blocks: programming language
    pub source_language: Option<String>,

    /// Properties as JSON string (TODO, PRIORITY, TAGS, etc.)
    pub properties: String,

    /// Child IDs as JSON array string
    pub children: String,

    pub created_at: i64,
    pub updated_at: i64,
}

impl LoroBlock {
    pub fn from_block(block: &Block) -> Self {
        let (content, content_type, source_language) = match &block.content {
            BlockContent::Text { raw } => (raw.clone(), "text".to_string(), None),
            BlockContent::Source(source) => (
                source.source.clone(),
                "source".to_string(),
                source.language.clone(),
            ),
        };

        Self {
            id: block.id.clone(),
            parent_id: block.parent_id.clone(),
            content,
            content_type,
            source_language,
            properties: serde_json::to_string(&block.properties).unwrap_or_default(),
            children: serde_json::to_string(&block.children).unwrap_or_default(),
            created_at: block.metadata.created_at,
            updated_at: block.metadata.updated_at,
        }
    }

    pub fn to_block(&self) -> Block {
        let content = if self.content_type == "source" {
            BlockContent::Source(SourceBlock {
                language: self.source_language.clone(),
                source: self.content.clone(),
                name: None,
                header_args: None,
                results: None,
            })
        } else {
            BlockContent::Text { raw: self.content.clone() }
        };

        Block {
            id: self.id.clone(),
            parent_id: self.parent_id.clone(),
            content,
            properties: serde_json::from_str(&self.properties).unwrap_or_default(),
            children: serde_json::from_str(&self.children).unwrap_or_default(),
            metadata: BlockMetadata {
                created_at: self.created_at,
                updated_at: self.updated_at,
            },
        }
    }

    /// Get title (first line of content)
    pub fn title(&self) -> &str {
        self.content.lines().next().unwrap_or("")
    }

    /// Get property value
    pub fn get_property(&self, key: &str) -> Option<String> {
        let props: HashMap<String, serde_json::Value> =
            serde_json::from_str(&self.properties).ok()?;
        props.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
    }
}
```

### 2. `crates/holon/src/sync/loro_block_operations.rs` (NEW)

Generic operations on Loro blocks:

```rust
//! Generic operations on Loro blocks.
//!
//! This is the primary operations layer for Loro. It's independent of any
//! specific persistence format (org-mode, JSON, etc.).

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use holon_api::block::{Block, BlockContent, ROOT_PARENT_ID};
use holon_api::Value;

use crate::api::{CoreOperations, LoroBackend};
use crate::core::datasource::{
    CompletionStateInfo, CrudOperations, DataSource, HasCache, OperationProvider,
    OperationResult, Result, TaskOperations,
};
use crate::core::queryable_cache::QueryableCache;
use crate::sync::{LoroBlock, LoroDocumentStore};

/// Generic operations on Loro blocks.
///
/// Implements standard operation traits, delegating to LoroBackend.
/// Independent of persistence format (org-mode is just one adapter).
pub struct LoroBlockOperations {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    cache: Arc<QueryableCache<LoroBlock>>,
}

impl LoroBlockOperations {
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        cache: Arc<QueryableCache<LoroBlock>>,
    ) -> Self {
        Self { doc_store, cache }
    }

    /// Get backend for a specific document.
    async fn get_backend(&self, doc_id: &str) -> Result<LoroBackend> {
        let store = self.doc_store.read().await;
        // Find doc by ID or use default
        for (path, collab_doc) in store.iter().await {
            let id = path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            if id == doc_id || doc_id.is_empty() {
                return Ok(LoroBackend::from_collaborative_doc(collab_doc, id));
            }
        }
        Err(format!("Document not found: {}", doc_id).into())
    }

    /// Find which document contains a block by ID.
    async fn find_doc_for_block(&self, block_id: &str) -> Result<(String, LoroBackend)> {
        let store = self.doc_store.read().await;

        for (path, collab_doc) in store.iter().await {
            let doc_id = path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            let backend = LoroBackend::from_collaborative_doc(collab_doc, doc_id.clone());

            if let Ok(Some(_)) = backend.find_block_by_uuid(block_id).await {
                return Ok((doc_id, backend));
            }
            // Also check direct block ID
            if backend.get_block(block_id).await.is_ok() {
                return Ok((doc_id, backend));
            }
        }

        Err(format!("Block not found: {}", block_id).into())
    }

    /// Save a document after modification.
    async fn save_doc(&self, doc_path: &str) -> Result<()> {
        let store = self.doc_store.write().await;
        let path = std::path::Path::new(doc_path);
        store.save(path).await?;
        Ok(())
    }
}

#[async_trait]
impl DataSource<LoroBlock> for LoroBlockOperations {
    async fn get_all(&self) -> Result<Vec<LoroBlock>> {
        self.cache.get_all().await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<LoroBlock>> {
        self.cache.get_by_id(id).await
    }
}

#[async_trait]
impl HasCache<LoroBlock> for LoroBlockOperations {
    fn get_cache(&self) -> &QueryableCache<LoroBlock> {
        &self.cache
    }
}

#[async_trait]
impl CrudOperations<LoroBlock> for LoroBlockOperations {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
        let (doc_path, backend) = self.find_doc_for_block(id).await?;

        match field {
            "content" => {
                if let Value::String(s) = &value {
                    backend.update_block_text(id, s).await
                        .map_err(|e| format!("Failed to update content: {}", e))?;
                }
            }
            _ => {
                // Store in properties
                let mut props = HashMap::new();
                props.insert(field.to_string(), value);
                backend.update_block_properties(id, &props).await
                    .map_err(|e| format!("Failed to update property: {}", e))?;
            }
        }

        self.save_doc(&doc_path).await?;
        Ok(OperationResult::irreversible(vec![]))
    }

    async fn create(&self, fields: HashMap<String, Value>) -> Result<(String, OperationResult)> {
        let doc_id = fields.get("doc_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let parent_id = fields.get("parent_id")
            .and_then(|v| v.as_str())
            .unwrap_or(ROOT_PARENT_ID);

        let content = fields.get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let backend = self.get_backend(doc_id).await?;

        let block = backend.create_block(
            parent_id.to_string(),
            BlockContent::text(content),
            None,
        ).await.map_err(|e| format!("Failed to create block: {}", e))?;

        // Set additional properties
        let mut props = HashMap::new();
        for (key, value) in &fields {
            if key != "doc_id" && key != "parent_id" && key != "content" {
                props.insert(key.clone(), value.clone());
            }
        }
        if !props.is_empty() {
            backend.update_block_properties(&block.id, &props).await
                .map_err(|e| format!("Failed to set properties: {}", e))?;
        }

        // Save
        let store = self.doc_store.read().await;
        for (path, _) in store.iter().await {
            let id = path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            if id == doc_id || doc_id.is_empty() {
                drop(store);
                self.save_doc(&path.to_string_lossy()).await?;
                break;
            }
        }

        Ok((block.id, OperationResult::irreversible(vec![])))
    }

    async fn delete(&self, id: &str) -> Result<OperationResult> {
        let (doc_path, backend) = self.find_doc_for_block(id).await?;

        backend.delete_block(id).await
            .map_err(|e| format!("Failed to delete block: {}", e))?;

        self.save_doc(&doc_path).await?;
        Ok(OperationResult::irreversible(vec![]))
    }
}

#[async_trait]
impl TaskOperations<LoroBlock> for LoroBlockOperations {
    async fn set_title(&self, id: &str, title: &str) -> Result<OperationResult> {
        // Get current content, replace first line
        if let Some(block) = self.cache.get_by_id(id).await? {
            let body: String = block.content.lines().skip(1).collect::<Vec<_>>().join("\n");
            let new_content = if body.is_empty() {
                title.to_string()
            } else {
                format!("{}\n{}", title, body)
            };
            self.set_field(id, "content", Value::String(new_content)).await
        } else {
            Err(format!("Block not found: {}", id).into())
        }
    }

    fn completion_states_with_progress(&self) -> Vec<CompletionStateInfo> {
        vec![
            CompletionStateInfo { state: "TODO".into(), progress: 0.0, is_done: false, is_active: true },
            CompletionStateInfo { state: "DOING".into(), progress: 0.5, is_done: false, is_active: true },
            CompletionStateInfo { state: "DONE".into(), progress: 1.0, is_done: true, is_active: false },
        ]
    }

    async fn set_state(&self, id: &str, state: String) -> Result<OperationResult> {
        self.set_field(id, "TODO", Value::String(state)).await
    }

    async fn set_due_date(&self, id: &str, date: Option<chrono::DateTime<chrono::Utc>>) -> Result<OperationResult> {
        match date {
            Some(dt) => self.set_field(id, "DEADLINE", Value::String(dt.to_rfc3339())).await,
            None => self.set_field(id, "DEADLINE", Value::Null).await,
        }
    }

    async fn set_priority(&self, id: &str, priority: i64) -> Result<OperationResult> {
        self.set_field(id, "PRIORITY", Value::Integer(priority)).await
    }
}

// OperationProvider implementation would follow the same pattern as OrgHeadlineOperations
// Using the generated dispatch functions from #[operations_trait] macros
```

### 3. `crates/holon/src/sync/loro_blocks_datasource.rs` (NEW)

DataSource for populating QueryableCache:

```rust
//! DataSource that reads blocks from Loro and emits changes.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use futures::StreamExt;
use std::pin::Pin;

use holon_api::streaming::{Change, ChangeNotifications, ChangeOrigin, StreamPosition};
use holon_api::ApiError;
use crate::api::{CoreOperations, LoroBackend};
use crate::api::types::Traversal;
use crate::sync::{LoroBlock, LoroDocumentStore};
use crate::core::datasource::{CrudOperations, DataSource, Result, OperationResult};
use holon_api::Value;

pub struct LoroBlocksDataSource {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    change_tx: tokio::sync::broadcast::Sender<Vec<Change<LoroBlock>>>,
}

impl LoroBlocksDataSource {
    pub fn new(doc_store: Arc<RwLock<LoroDocumentStore>>) -> Self {
        let (change_tx, _) = tokio::sync::broadcast::channel(100);
        Self { doc_store, change_tx }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<Change<LoroBlock>>> {
        self.change_tx.subscribe()
    }

    /// Start polling for changes. Call once at startup.
    pub async fn start_polling(self: Arc<Self>) {
        let mut last_state: HashMap<String, LoroBlock> = HashMap::new();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                let store = self.doc_store.read().await;
                let mut current_state: HashMap<String, LoroBlock> = HashMap::new();

                for (path, collab_doc) in store.iter().await {
                    let doc_id = path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
                    let backend = LoroBackend::from_collaborative_doc(collab_doc, doc_id);

                    if let Ok(blocks) = backend.get_all_blocks(Traversal::ALL_BUT_ROOT).await {
                        for block in blocks {
                            let loro_block = LoroBlock::from_block(&block);
                            current_state.insert(loro_block.id.clone(), loro_block);
                        }
                    }
                }
                drop(store);

                // Compute diff
                let mut changes = Vec::new();

                // Created and Updated
                for (id, block) in &current_state {
                    match last_state.get(id) {
                        None => {
                            changes.push(Change::Created {
                                data: block.clone(),
                                origin: ChangeOrigin::Local { operation_id: None, trace_id: None },
                            });
                        }
                        Some(old) if old.content != block.content || old.properties != block.properties => {
                            changes.push(Change::Updated {
                                id: id.clone(),
                                data: block.clone(),
                                origin: ChangeOrigin::Local { operation_id: None, trace_id: None },
                            });
                        }
                        _ => {}
                    }
                }

                // Deleted
                for id in last_state.keys() {
                    if !current_state.contains_key(id) {
                        changes.push(Change::Deleted {
                            id: id.clone(),
                            origin: ChangeOrigin::Local { operation_id: None, trace_id: None },
                        });
                    }
                }

                if !changes.is_empty() {
                    let _ = self.change_tx.send(changes);
                }

                last_state = current_state;
            }
        });
    }
}

#[async_trait]
impl DataSource<LoroBlock> for LoroBlocksDataSource {
    async fn get_all(&self) -> Result<Vec<LoroBlock>> {
        let store = self.doc_store.read().await;
        let mut all = Vec::new();

        for (path, collab_doc) in store.iter().await {
            let doc_id = path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            let backend = LoroBackend::from_collaborative_doc(collab_doc, doc_id);

            if let Ok(blocks) = backend.get_all_blocks(Traversal::ALL_BUT_ROOT).await {
                all.extend(blocks.into_iter().map(|b| LoroBlock::from_block(&b)));
            }
        }

        Ok(all)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<LoroBlock>> {
        let store = self.doc_store.read().await;

        for (path, collab_doc) in store.iter().await {
            let doc_id = path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            let backend = LoroBackend::from_collaborative_doc(collab_doc, doc_id);

            if let Ok(block) = backend.get_block(id).await {
                return Ok(Some(LoroBlock::from_block(&block)));
            }
        }

        Ok(None)
    }
}

#[async_trait]
impl CrudOperations<LoroBlock> for LoroBlocksDataSource {
    async fn set_field(&self, _id: &str, _field: &str, _value: Value) -> Result<OperationResult> {
        Err("Use LoroBlockOperations for mutations".into())
    }

    async fn create(&self, _fields: HashMap<String, Value>) -> Result<(String, OperationResult)> {
        Err("Use LoroBlockOperations for mutations".into())
    }

    async fn delete(&self, _id: &str) -> Result<OperationResult> {
        Err("Use LoroBlockOperations for mutations".into())
    }
}

#[async_trait]
impl ChangeNotifications<LoroBlock> for LoroBlocksDataSource {
    async fn watch_changes_since(
        &self,
        _position: StreamPosition,
    ) -> Pin<Box<dyn futures::Stream<Item = std::result::Result<Vec<Change<LoroBlock>>, ApiError>> + Send>> {
        let rx = self.change_tx.subscribe();

        Box::pin(tokio_stream::wrappers::BroadcastStream::new(rx)
            .filter_map(|result| async move {
                result.ok().map(Ok)
            }))
    }
}
```

### 4. Update `crates/holon/src/sync/mod.rs`

```rust
pub mod loro_block_entity;
pub mod loro_block_operations;
pub mod loro_blocks_datasource;

pub use loro_block_entity::LoroBlock;
pub use loro_block_operations::LoroBlockOperations;
pub use loro_blocks_datasource::LoroBlocksDataSource;
```

### 5. Update DI - Two Options

#### Option A: In `crates/holon/src/di/mod.rs` (for core holon)

```rust
use crate::sync::{LoroBlock, LoroBlockOperations, LoroBlocksDataSource};

// Register QueryableCache<LoroBlock>
services.add_singleton_factory::<QueryableCache<LoroBlock>, _>(|r| create_queryable_cache(r));

// Register LoroBlocksDataSource
services.add_singleton_factory::<Arc<LoroBlocksDataSource>, _>(|resolver| {
    let doc_store = resolver.get_required::<Arc<RwLock<LoroDocumentStore>>>();
    Arc::new(LoroBlocksDataSource::new(doc_store))
});

// Register LoroBlockOperations
services.add_singleton_factory::<LoroBlockOperations, _>(|resolver| {
    let doc_store = resolver.get_required::<Arc<RwLock<LoroDocumentStore>>>();
    let cache = resolver.get_required::<QueryableCache<LoroBlock>>();
    LoroBlockOperations::new(doc_store, cache)
});
```

#### Option B: In `crates/holon-orgmode/src/di.rs` (if Loro is only used with org-mode for now)

Same registrations, but in the org-mode DI module.

### 6. Org-mode becomes a persistence adapter

Modify `crates/holon-orgmode/src/loro_renderer.rs` to be a pure persistence adapter:

```rust
/// OrgRenderer is now just a persistence adapter.
/// It subscribes to Loro changes and writes org files.
/// It does NOT define operations - that's LoroBlockOperations' job.
impl OrgRenderer {
    pub async fn start_persistence_subscription(
        self: Arc<Self>,
        blocks_datasource: Arc<LoroBlocksDataSource>,
        write_tracker: Arc<RwLock<WriteTracker>>,
    ) {
        let mut rx = blocks_datasource.subscribe();

        tokio::spawn(async move {
            while let Ok(changes) = rx.recv().await {
                // Group changes by document
                // Render affected documents to org files
                // Mark writes in WriteTracker
            }
        });
    }
}
```

### 7. Update PRQL query

The query remains the same since `LoroBlock` entity has `blocks` table:

```prql
from blocks
derive {
    title = s"CASE WHEN instr(content, char(10)) > 0 THEN substr(content, 1, instr(content, char(10)) - 1) ELSE content END",
    task_state = s"json_extract(properties, '$.TODO')",
    priority = s"json_extract(properties, '$.PRIORITY')",
    entity_name = "blocks",
    sort_key = created_at
}
derive { ui = (render (row (draggable (pie_menu (bullet) fields:this.*) on:'drag') (spacer 10) (state_toggle this.task_state) (editable_text content:this.title) (badge content:this.priority color:"cyan"))) }
select { id, parent_id, entity_name, sort_key, ui }
render (tree parent_id:parent_id sortkey:sort_key item_template:this.ui)
```

## Summary

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `LoroBlock` | `holon/sync` | Entity for QueryableCache |
| `LoroBlockOperations` | `holon/sync` | CRUD & Task operations on blocks |
| `LoroBlocksDataSource` | `holon/sync` | Read blocks, emit changes |
| `OrgRenderer` | `holon-orgmode` | Persistence adapter (Loro → org files) |
| `LoroOrgBridge` | `holon-orgmode` | Persistence adapter (org files → Loro) |

## Key Design Points

1. **Operations are generic** - `LoroBlockOperations` knows nothing about org-mode
2. **Persistence is pluggable** - OrgRenderer is just one way to persist
3. **QueryableCache enables PRQL** - Blocks are queryable via SQL
4. **Clean separation** - Core Loro in `holon`, org-mode adapter in `holon-orgmode`

## Implementation Deviations and Review Items

### Deviations from Original Plan

1. **TaskEntity Trait Implementation**
   - **Deviation**: The plan didn't specify that `LoroBlock` needed to implement `TaskEntity` trait
   - **Reason**: Required by `TaskOperations<LoroBlock>` trait bound
   - **Implementation**: Added `TaskEntity` impl with `completed()`, `priority()`, and `due_date()` methods
   - **Location**: `crates/holon/src/sync/loro_block_entity.rs`

2. **Type Conversions**
   - **Deviation**: Several type mismatches required fixes:
     - `Value::as_string()` returns `Option<&str>`, not `Option<String>`
     - `ROOT_PARENT_ID` is `&str`, needed `.to_string()` conversion
     - Stream type needed `tokio_stream::Stream` instead of `futures::Stream`
   - **Reason**: API differences between plan assumptions and actual codebase
   - **Location**: `crates/holon/src/sync/loro_block_operations.rs`, `loro_blocks_datasource.rs`

3. **ChangeNotifications Trait**
   - **Deviation**: Plan didn't mention `get_current_version()` method requirement
   - **Reason**: Required by `ChangeNotifications` trait
   - **Implementation**: Added stub implementation returning empty `Vec<u8>` (polling-based datasource doesn't track versions)
   - **Location**: `crates/holon/src/sync/loro_blocks_datasource.rs`

4. **DI Registration Location**
   - **Deviation**: Chose Option B (`holon-orgmode/src/di.rs`) instead of Option A
   - **Reason**: Loro-related services are already registered there, keeps related code together
   - **Additional**: Added subscription setup to connect `LoroBlocksDataSource` changes to `QueryableCache<LoroBlock>`
   - **Location**: `crates/holon-orgmode/src/di.rs`

5. **OrgRenderer Update**
   - **Deviation**: Plan mentioned updating `loro_renderer.rs` to use `LoroBlocksDataSource` subscription, but this wasn't implemented
   - **Reason**: Existing `start_loro_subscription()` already polls and renders, works independently
   - **Status**: Left as-is; may need refactoring later to use `LoroBlocksDataSource` instead of direct polling

### Review Items

1. **OperationProvider Implementation**
   - **Status**: Not implemented
   - **Plan Note**: "OperationProvider implementation would follow the same pattern as OrgHeadlineOperations"
   - **Action Needed**: Implement `OperationProvider` for `LoroBlockOperations` to enable operation dispatch
   - **Priority**: Medium - Required for full operation support

2. **OrgRenderer Integration**
   - **Status**: Uses its own polling mechanism
   - **Question**: Should `OrgRenderer` subscribe to `LoroBlocksDataSource` changes instead of polling directly?
   - **Benefit**: Would eliminate duplicate polling and ensure consistency
   - **Priority**: Low - Current implementation works, but could be optimized

3. **Cache Population Strategy**
   - **Status**: `LoroBlocksDataSource` polls every 500ms and applies changes to cache
   - **Question**: Is polling frequency appropriate? Should we use event-driven updates from `LoroBackend` instead?
   - **Current**: Polling-based with 500ms interval
   - **Priority**: Medium - Performance consideration

4. **Error Handling**
   - **Status**: Basic error handling implemented
   - **Review**: Check if error messages are sufficient for debugging
   - **Location**: All three new files

5. **Source Block Handling**
   - **Status**: `LoroBlock::from_block()` extracts source code but loses `name`, `header_args`, and `results`
   - **Impact**: Source block metadata not preserved in flattened entity
   - **Question**: Should we add fields to `LoroBlock` for source block metadata?
   - **Priority**: Low - May not be needed for current use cases

6. **Document ID Resolution**
   - **Status**: `get_backend()` and `find_doc_for_block()` iterate through all documents
   - **Performance**: O(n) lookup - may need optimization if many documents
   - **Priority**: Low - Only matters with many documents

7. **Save Strategy**
   - **Status**: Each operation saves the document immediately
   - **Question**: Should we batch saves or use a different persistence strategy?
   - **Priority**: Low - Current approach is simple and safe
