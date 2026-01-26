# Loro as Source of Truth - Implementation Handoff

## Goal

Flip the architecture so Loro is the source of truth, not org files. Operations go to Loro first, then sync to org files.

## Current Flow (WRONG)

```
UI → OrgHeadlineOperations → Org Files → LoroOrgBridge → Loro
```

## Target Flow (CORRECT)

```
UI → LoroOrgOperations → LoroBackend → Loro → OrgRenderer → Org Files
                                         ↑
External org edits → LoroOrgBridge ──────┘
```

## Files to Create

### 1. `crates/holon-orgmode/src/loro_org_operations.rs` (NEW)

This replaces `OrgHeadlineOperations` as the primary operations provider.

```rust
//! Loro-backed operations for OrgHeadline entities.
//!
//! Uses Loro as source of truth. Changes flow: Loro → OrgRenderer → org files.

use async_trait::async_trait;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use holon::api::LoroBackend;
use holon::sync::LoroDocumentStore;
use holon::core::datasource::{CrudOperations, TaskOperations, OperationProvider, CompletionStateInfo};
use holon::core::queryable_cache::QueryableCache;
use holon_api::{Value, OperationDescriptor, OperationResult};

use crate::models::OrgHeadline;
use crate::loro_renderer::OrgRenderer;

/// Operations layer using Loro as source of truth.
pub struct LoroOrgOperations {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    renderer: Arc<OrgRenderer>,
    /// Cache for fast reads (populated from Loro change events)
    cache: Arc<QueryableCache<OrgHeadline>>,
}

impl LoroOrgOperations {
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        renderer: Arc<OrgRenderer>,
        cache: Arc<QueryableCache<OrgHeadline>>,
    ) -> Self {
        Self { doc_store, renderer, cache }
    }

    /// Get LoroBackend for a specific org file path.
    async fn get_backend(&self, file_path: &str) -> Result<Arc<LoroBackend>> {
        let store = self.doc_store.read().await;
        let path = std::path::Path::new(file_path);
        let collab_doc = store.get(path)
            .ok_or_else(|| anyhow::anyhow!("No Loro doc for file: {}", file_path))?;

        let doc_id = file_path.replace(std::path::MAIN_SEPARATOR, "/");
        Ok(Arc::new(LoroBackend::from_collaborative_doc(collab_doc.clone(), doc_id)))
    }

    /// Find which file contains a block by UUID, then get its backend.
    async fn find_backend_for_uuid(&self, uuid: &str) -> Result<(String, Arc<LoroBackend>)> {
        let store = self.doc_store.read().await;

        for (path, collab_doc) in store.iter() {
            let doc_id = path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            let backend = LoroBackend::from_collaborative_doc(collab_doc.clone(), doc_id.clone());

            if let Ok(Some(_)) = backend.find_block_by_uuid(uuid).await {
                return Ok((path.to_string_lossy().to_string(), Arc::new(backend)));
            }
        }

        Err(anyhow::anyhow!("Block not found in any Loro doc: {}", uuid))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CrudOperations<OrgHeadline> for LoroOrgOperations {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
        let (file_path, backend) = self.find_backend_for_uuid(id).await?;

        let block_id = backend.find_block_by_uuid(id).await?
            .ok_or_else(|| anyhow::anyhow!("Block not found: {}", id))?;

        // Map OrgHeadline fields to Block properties
        let mut properties = HashMap::new();
        match field {
            "title" => {
                if let Value::String(s) = &value {
                    backend.update_block_text(&block_id, s).await
                        .map_err(|e| anyhow::anyhow!("Failed to update text: {}", e))?;
                }
            }
            "task_state" => {
                properties.insert("TODO".to_string(), value);
            }
            "priority" => {
                properties.insert("PRIORITY".to_string(), value);
            }
            "tags" => {
                properties.insert("TAGS".to_string(), value);
            }
            "scheduled" => {
                properties.insert("SCHEDULED".to_string(), value);
            }
            "deadline" => {
                properties.insert("DEADLINE".to_string(), value);
            }
            "body" => {
                // Body is part of content after title
                if let Value::String(s) = &value {
                    // Get current title, append body
                    let block = backend.get_block(&block_id).await
                        .map_err(|e| anyhow::anyhow!("Failed to get block: {}", e))?;
                    let title = block.content.text().lines().next().unwrap_or("");
                    let new_content = if s.is_empty() {
                        title.to_string()
                    } else {
                        format!("{}\n{}", title, s)
                    };
                    backend.update_block_text(&block_id, &new_content).await
                        .map_err(|e| anyhow::anyhow!("Failed to update text: {}", e))?;
                }
            }
            other => {
                // Store as generic property
                properties.insert(other.to_string(), value);
            }
        }

        if !properties.is_empty() {
            backend.update_block_properties(&block_id, &properties).await
                .map_err(|e| anyhow::anyhow!("Failed to update properties: {}", e))?;
        }

        // Save Loro doc (triggers OrgRenderer subscription to write org file)
        let store = self.doc_store.write().await;
        store.save(std::path::Path::new(&file_path)).await?;

        Ok(OperationResult::success())
    }

    async fn create(&self, fields: HashMap<String, Value>) -> Result<(String, OperationResult)> {
        let file_path = fields.get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("file_path required for create"))?;

        let parent_id = fields.get("parent_id")
            .and_then(|v| v.as_str())
            .unwrap_or(holon_api::block::ROOT_PARENT_ID);

        let title = fields.get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let backend = self.get_backend(file_path).await?;

        // Create block in Loro
        let content = holon_api::block::BlockContent::Text { raw: title.to_string() };
        let block = backend.create_block(parent_id.to_string(), content, None).await
            .map_err(|e| anyhow::anyhow!("Failed to create block: {}", e))?;

        // Set additional properties
        let mut properties = HashMap::new();
        if let Some(task_state) = fields.get("task_state") {
            properties.insert("TODO".to_string(), task_state.clone());
        }
        if let Some(tags) = fields.get("tags") {
            properties.insert("TAGS".to_string(), tags.clone());
        }
        // Add ID property
        properties.insert("ID".to_string(), Value::String(block.id.clone()));

        if !properties.is_empty() {
            backend.update_block_properties(&block.id, &properties).await
                .map_err(|e| anyhow::anyhow!("Failed to set properties: {}", e))?;
        }

        // Save Loro doc
        let store = self.doc_store.write().await;
        store.save(std::path::Path::new(file_path)).await?;

        Ok((block.id, OperationResult::success()))
    }

    async fn delete(&self, id: &str) -> Result<OperationResult> {
        let (file_path, backend) = self.find_backend_for_uuid(id).await?;

        let block_id = backend.find_block_by_uuid(id).await?
            .ok_or_else(|| anyhow::anyhow!("Block not found: {}", id))?;

        backend.delete_block(&block_id).await
            .map_err(|e| anyhow::anyhow!("Failed to delete block: {}", e))?;

        // Save Loro doc
        let store = self.doc_store.write().await;
        store.save(std::path::Path::new(&file_path)).await?;

        Ok(OperationResult::success())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl TaskOperations<OrgHeadline> for LoroOrgOperations {
    async fn set_title(&self, id: &str, title: &str) -> Result<OperationResult> {
        self.set_field(id, "title", Value::String(title.to_string())).await
    }

    fn completion_states_with_progress(&self) -> Vec<CompletionStateInfo> {
        vec![
            CompletionStateInfo { state: "TODO".into(), progress: 0.0, is_done: false },
            CompletionStateInfo { state: "DOING".into(), progress: 0.5, is_done: false },
            CompletionStateInfo { state: "DONE".into(), progress: 1.0, is_done: true },
        ]
    }

    async fn set_completion(&self, id: &str, state: &str) -> Result<OperationResult> {
        self.set_field(id, "task_state", Value::String(state.to_string())).await
    }

    async fn set_due_date(&self, id: &str, date: Option<&str>) -> Result<OperationResult> {
        match date {
            Some(d) => self.set_field(id, "deadline", Value::String(d.to_string())).await,
            None => self.set_field(id, "deadline", Value::Null).await,
        }
    }

    async fn set_scheduled_date(&self, id: &str, date: Option<&str>) -> Result<OperationResult> {
        match date {
            Some(d) => self.set_field(id, "scheduled", Value::String(d.to_string())).await,
            None => self.set_field(id, "scheduled", Value::Null).await,
        }
    }

    async fn set_priority(&self, id: &str, priority: Option<i32>) -> Result<OperationResult> {
        match priority {
            Some(p) => self.set_field(id, "priority", Value::Integer(p as i64)).await,
            None => self.set_field(id, "priority", Value::Null).await,
        }
    }
}

// TODO: Implement OperationProvider for dynamic dispatch
// Copy pattern from OrgHeadlineOperations in orgmode_datasource.rs
```

### 2. Modify `crates/holon-orgmode/src/loro_renderer.rs`

Add Loro change subscription that writes org files when Loro changes.

**Add this method to `OrgRenderer`:**

```rust
use tokio_stream::StreamExt;

impl OrgRenderer {
    /// Start watching Loro changes and rendering to org files.
    /// Call this once during app initialization.
    pub async fn start_loro_subscription(
        self: Arc<Self>,
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        write_tracker: Arc<RwLock<WriteTracker>>,
    ) {
        use tracing::{info, error, debug};

        info!("OrgRenderer: Starting Loro change subscription");

        // For each doc in store, subscribe to changes
        let store = doc_store.read().await;
        for (org_path, collab_doc) in store.iter() {
            let org_path = org_path.clone();
            let renderer = self.clone();
            let doc = collab_doc.clone();
            let tracker = write_tracker.clone();
            let doc_store_clone = doc_store.clone();

            tokio::spawn(async move {
                // LoroDoc subscription - check collaborative_doc.rs for actual API
                // This is pseudocode - adapt to actual Loro subscription API
                loop {
                    // Wait for changes (implement based on Loro's actual API)
                    // For now, poll periodically
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                    // Get current blocks from Loro
                    let store = doc_store_clone.read().await;
                    if let Some(doc) = store.get(&org_path) {
                        // Get file_id from path
                        let file_id = org_path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");

                        // Create backend to read blocks
                        let backend = LoroBackend::from_collaborative_doc(doc.clone(), file_id.clone());

                        match backend.get_children(holon_api::block::ROOT_PARENT_ID).await {
                            Ok(blocks) => {
                                // Render to org format
                                let org_content = Self::render_blocks(&blocks, &org_path, &file_id);

                                // Mark as our write so LoroOrgBridge ignores it
                                {
                                    let mut t = tracker.write().await;
                                    t.mark_our_write(&org_path.to_string_lossy());
                                }

                                // Write to file
                                if let Err(e) = tokio::fs::write(&org_path, &org_content).await {
                                    error!("Failed to write org file {}: {}", org_path.display(), e);
                                } else {
                                    debug!("Rendered org file: {}", org_path.display());
                                }
                            }
                            Err(e) => {
                                error!("Failed to get blocks for {}: {}", org_path.display(), e);
                            }
                        }
                    }
                }
            });
        }
    }
}
```

### 3. Modify `crates/holon-orgmode/src/lib.rs`

Add the new module:

```rust
pub mod loro_org_operations;
pub use loro_org_operations::LoroOrgOperations;
```

### 4. Modify `crates/holon-orgmode/src/di.rs`

Replace `OrgHeadlineOperations` with `LoroOrgOperations` in the DI registration.

**Find the `dyn OperationProvider` registration and update it:**

```rust
// Add import at top
use crate::loro_org_operations::LoroOrgOperations;
use crate::loro_renderer::OrgRenderer;

// In register_services, replace the OperationProvider registration:

// Register OrgRenderer
services.add_singleton_factory::<Arc<OrgRenderer>, _>(|_resolver| {
    Arc::new(OrgRenderer::new())
});

// Register LoroOrgOperations (replaces OrgHeadlineOperations as primary)
services.add_singleton_factory::<LoroOrgOperations, _>(|resolver| {
    let doc_store = resolver.get_required::<Arc<tokio::sync::RwLock<LoroDocumentStore>>>();
    let renderer = resolver.get_required::<Arc<OrgRenderer>>();
    let cache = resolver.get_required::<QueryableCache<OrgHeadline>>();
    LoroOrgOperations::new(doc_store, renderer, cache)
});

// Update the OperationProvider registration to use LoroOrgOperations
services.add_trait_factory::<dyn OperationProvider, _>(Lifetime::Singleton, |resolver| {
    // ... existing stream processing setup ...

    // Start OrgRenderer subscription (Loro → Org files)
    let renderer = resolver.get_required::<Arc<OrgRenderer>>();
    let doc_store = resolver.get_required::<Arc<tokio::sync::RwLock<LoroDocumentStore>>>();
    let write_tracker = Arc::new(tokio::sync::RwLock::new(WriteTracker::new()));
    tokio::spawn({
        let renderer = renderer.clone();
        let doc_store = doc_store.clone();
        let tracker = write_tracker.clone();
        async move {
            renderer.start_loro_subscription(doc_store, tracker).await;
        }
    });

    // Start LoroOrgBridge (Org files → Loro, for external edits)
    let loro_bridge = resolver.get_required::<LoroOrgBridge>();
    let headline_ops = resolver.get_required::<OrgHeadlineOperations>();
    tokio::spawn(async move {
        if let Err(e) = loro_bridge.start(&*headline_ops).await {
            error!("[OrgMode] Loro bridge error: {}", e);
        }
    });

    // Return LoroOrgOperations as the OperationProvider
    let loro_ops = resolver.get_required::<LoroOrgOperations>();
    let wrapped = OperationWrapper::new(Arc::new(loro_ops), Some(sync_provider.clone()));
    Arc::new(wrapped) as Arc<dyn OperationProvider>
});
```

## Key Points

1. **LoroOrgOperations** is the new primary - implements `CrudOperations<OrgHeadline>` and `TaskOperations<OrgHeadline>`

2. **Operations flow**: UI → LoroOrgOperations → LoroBackend → Loro → save .loro file

3. **OrgRenderer subscription** watches Loro changes and writes org files (Loro → Org)

4. **LoroOrgBridge** still needed for external edits (Org → Loro)

5. **WriteTracker** prevents loops: OrgRenderer marks writes, LoroOrgBridge ignores them

## Testing

```bash
cargo check -p holon-orgmode --features di
cargo test -p holon-orgmode --features di
```

## Migration Notes

- `OrgHeadlineOperations` stays for now (used by LoroOrgBridge to watch changes)
- `LoroOrgOperations` becomes the `OperationProvider` returned to UI
- Cache is still populated from org file changes (via existing stream processing)
- Future: Cache could be populated from Loro instead
