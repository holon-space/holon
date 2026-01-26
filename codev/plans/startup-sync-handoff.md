# Startup Bidirectional Sync Implementation Handoff

## Goal

Implement bidirectional sync between Loro and OrgMode on application startup with merge and conflict detection.

## Requirements

1. **Load existing .loro files** on startup (resume from persisted state)
2. **Parse org files** to get current file system state
3. **Merge with conflict detection** - compare timestamps, apply newer changes, flag conflicts
4. **Store conflicts in QueryableCache** - queryable via PRQL, shown in UI

## Architecture

```
Startup Sequence:
1. Initialize LoroDocumentStore (empty)
2. Load all existing .loro files → populate doc store
3. Scan all .org files → parse to OrgHeadline
4. For each file pair:
   a. Match blocks by :ID: property
   b. Compare timestamps (updated_at)
   c. Determine resolution per block
5. Apply resolutions:
   - UseLoro → render to org file
   - UseOrg → apply to Loro doc
   - Conflict → add to SyncConflict cache
6. Start normal streaming tasks (LoroOrgBridge, OrgRenderer)
```

## Files to Create/Modify

### 1. NEW: `crates/holon-orgmode/src/startup_sync.rs`

```rust
//! Startup synchronization between Loro and OrgMode.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use holon_macros::Entity;
use tracing::{debug, info, warn, error};
use anyhow::Result;

use holon_api::block::Block;
use holon::sync::LoroDocumentStore;
use holon::api::{CoreOperations, LoroBackend};
use holon::api::types::Traversal;
use crate::models::OrgHeadline;
use crate::loro_org_bridge::WriteTracker;
use crate::loro_renderer::OrgRenderer;
use crate::loro_diff::{ParsedBlock, headlines_to_block_map};
use crate::OrgModeConfig;

/// Sync conflict entity for QueryableCache storage.
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "sync_conflicts", short_name = "conflict")]
pub struct SyncConflict {
    #[primary_key]
    pub id: String,
    #[indexed]
    pub block_id: String,
    pub file_path: String,
    pub conflict_type: String,  // "BothModified", "DeletedInLoro", "DeletedInOrg"
    pub loro_content: Option<String>,
    pub org_content: Option<String>,
    pub loro_updated_at: Option<i64>,
    pub org_updated_at: Option<i64>,
    pub resolved: bool,
    pub created_at: i64,
}

/// Result of startup synchronization.
#[derive(Debug, Default)]
pub struct StartupSyncResult {
    pub files_processed: usize,
    pub loro_to_org_synced: usize,
    pub org_to_loro_synced: usize,
    pub conflicts: Vec<SyncConflict>,
    pub errors: Vec<String>,
}

/// Resolution strategy for a block.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// Use Loro state (render to org file)
    UseLoro,
    /// Use Org state (apply to Loro doc)
    UseOrg,
    /// Content matches, no action needed
    Merged,
    /// Conflict detected, requires manual resolution
    Conflict,
}

/// Info about a file pair (org + loro).
struct FilePairInfo {
    org_path: PathBuf,
    loro_exists: bool,
    org_exists: bool,
}

/// Orchestrates startup synchronization.
pub struct StartupSyncOrchestrator {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    config: OrgModeConfig,
    write_tracker: Arc<RwLock<WriteTracker>>,
    renderer: Arc<OrgRenderer>,
}

impl StartupSyncOrchestrator {
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        config: OrgModeConfig,
        write_tracker: Arc<RwLock<WriteTracker>>,
        renderer: Arc<OrgRenderer>,
    ) -> Self {
        Self { doc_store, config, write_tracker, renderer }
    }

    /// Perform startup synchronization.
    pub async fn sync_on_startup(&self) -> Result<StartupSyncResult> {
        let mut result = StartupSyncResult::default();

        info!("[StartupSync] Starting bidirectional sync...");

        // Step 1: Load all existing .loro files
        let loaded_loro = {
            let mut store = self.doc_store.write().await;
            store.load_all_existing().await?
        };
        info!("[StartupSync] Loaded {} existing .loro files", loaded_loro.len());

        // Step 2: Discover all .org files
        let org_files = self.discover_org_files().await?;
        info!("[StartupSync] Found {} .org files", org_files.len());

        // Step 3: Build unified file list
        let file_pairs = self.build_file_pairs(&loaded_loro, &org_files);

        // Step 4: Process each file pair
        for file_info in file_pairs {
            match self.sync_file_pair(&file_info).await {
                Ok(file_result) => {
                    result.files_processed += 1;
                    result.loro_to_org_synced += file_result.loro_to_org_synced;
                    result.org_to_loro_synced += file_result.org_to_loro_synced;
                    result.conflicts.extend(file_result.conflicts);
                }
                Err(e) => {
                    result.errors.push(format!("{}: {}", file_info.org_path.display(), e));
                }
            }
        }

        info!(
            "[StartupSync] Complete: {} files, {} loro→org, {} org→loro, {} conflicts",
            result.files_processed,
            result.loro_to_org_synced,
            result.org_to_loro_synced,
            result.conflicts.len()
        );

        Ok(result)
    }

    /// Discover all .org files in the root directory.
    async fn discover_org_files(&self) -> Result<Vec<PathBuf>> {
        let mut org_files = Vec::new();
        let root = &self.config.root_directory;

        if !root.exists() {
            return Ok(org_files);
        }

        fn walk_dir(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    // Skip hidden directories
                    if !path.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(false) {
                        walk_dir(&path, files)?;
                    }
                } else if path.extension().map(|e| e == "org").unwrap_or(false) {
                    files.push(path);
                }
            }
            Ok(())
        }

        walk_dir(root, &mut org_files)?;
        Ok(org_files)
    }

    /// Build file pairs from loro and org file lists.
    fn build_file_pairs(&self, loro_files: &[PathBuf], org_files: &[PathBuf]) -> Vec<FilePairInfo> {
        let mut pairs = HashMap::new();

        // Add org files
        for org_path in org_files {
            pairs.insert(org_path.clone(), FilePairInfo {
                org_path: org_path.clone(),
                loro_exists: false,
                org_exists: true,
            });
        }

        // Mark which have loro files
        for loro_org_path in loro_files {
            if let Some(info) = pairs.get_mut(loro_org_path) {
                info.loro_exists = true;
            } else {
                // Loro exists but org doesn't
                pairs.insert(loro_org_path.clone(), FilePairInfo {
                    org_path: loro_org_path.clone(),
                    loro_exists: true,
                    org_exists: false,
                });
            }
        }

        pairs.into_values().collect()
    }

    /// Sync a single file pair.
    async fn sync_file_pair(&self, file_info: &FilePairInfo) -> Result<StartupSyncResult> {
        let mut result = StartupSyncResult::default();

        // Load Loro blocks (if loro exists)
        let loro_blocks = if file_info.loro_exists {
            self.load_loro_blocks(&file_info.org_path).await?
        } else {
            HashMap::new()
        };

        // Parse Org headlines (if org exists)
        let org_blocks = if file_info.org_exists {
            self.parse_org_blocks(&file_info.org_path).await?
        } else {
            HashMap::new()
        };

        // Compute resolutions
        let resolutions = self.compute_resolutions(&loro_blocks, &org_blocks, file_info);

        // Apply resolutions
        for (block_id, resolution, loro_block, org_block) in resolutions {
            match resolution {
                Resolution::UseLoro => {
                    if let Some(block) = loro_block {
                        // Render Loro to org (handled in batch at end)
                        result.loro_to_org_synced += 1;
                    }
                }
                Resolution::UseOrg => {
                    if let Some(block) = org_block {
                        // Apply org to Loro
                        self.apply_org_to_loro(&file_info.org_path, &block).await?;
                        result.org_to_loro_synced += 1;
                    }
                }
                Resolution::Conflict => {
                    let conflict = SyncConflict {
                        id: uuid::Uuid::new_v4().to_string(),
                        block_id: block_id.clone(),
                        file_path: file_info.org_path.to_string_lossy().to_string(),
                        conflict_type: "BothModified".to_string(),
                        loro_content: loro_block.map(|b| b.content.clone()),
                        org_content: org_block.map(|b| format!("{}\n{}", b.title, b.body.unwrap_or_default())),
                        loro_updated_at: loro_block.map(|b| b.updated_at),
                        org_updated_at: org_block.and_then(|b| b.properties.get("UPDATED_AT").and_then(|s| s.parse().ok())),
                        resolved: false,
                        created_at: chrono::Utc::now().timestamp_millis(),
                    };
                    result.conflicts.push(conflict);
                }
                Resolution::Merged => {
                    // No action needed
                }
            }
        }

        // Render Loro to org file if any loro_to_org changes
        if result.loro_to_org_synced > 0 {
            self.render_loro_to_org(&file_info.org_path).await?;
        }

        Ok(result)
    }

    /// Load blocks from Loro document.
    async fn load_loro_blocks(&self, org_path: &Path) -> Result<HashMap<String, LoroBlockState>> {
        let store = self.doc_store.read().await;
        let mut blocks = HashMap::new();

        if let Some(collab_doc) = store.get(org_path).await {
            let file_id = org_path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            let backend = LoroBackend::from_collaborative_doc(collab_doc, file_id);

            if let Ok(all_blocks) = backend.get_all_blocks(Traversal::ALL_BUT_ROOT).await {
                for block in all_blocks {
                    let state = LoroBlockState {
                        id: block.id.clone(),
                        content: match &block.content {
                            holon_api::block::BlockContent::Text { raw } => raw.clone(),
                            holon_api::block::BlockContent::Source(s) => s.source.clone(),
                        },
                        properties: block.properties.clone(),
                        updated_at: block.metadata.updated_at,
                        parent_id: block.parent_id.clone(),
                    };
                    blocks.insert(block.id, state);
                }
            }
        }

        Ok(blocks)
    }

    /// Parse blocks from org file.
    async fn parse_org_blocks(&self, org_path: &Path) -> Result<HashMap<String, ParsedBlock>> {
        // Read and parse org file
        let content = tokio::fs::read_to_string(org_path).await?;
        let headlines = crate::parser::parse_org_content(&content, org_path)?;
        Ok(headlines_to_block_map(&headlines))
    }

    /// Compute resolution for each block.
    fn compute_resolutions(
        &self,
        loro_blocks: &HashMap<String, LoroBlockState>,
        org_blocks: &HashMap<String, ParsedBlock>,
        _file_info: &FilePairInfo,
    ) -> Vec<(String, Resolution, Option<&LoroBlockState>, Option<&ParsedBlock>)> {
        let mut resolutions = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Process blocks in Loro
        for (id, loro_block) in loro_blocks {
            seen.insert(id.clone());
            let org_block = org_blocks.get(id);

            let resolution = match org_block {
                Some(org) => {
                    // Both exist - compare content
                    let loro_content = &loro_block.content;
                    let org_content = format!("{}\n{}", org.title, org.body.as_deref().unwrap_or(""));

                    if loro_content.trim() == org_content.trim() {
                        Resolution::Merged
                    } else {
                        // Compare timestamps
                        let loro_ts = loro_block.updated_at;
                        let org_ts = org.properties.get("UPDATED_AT")
                            .and_then(|s| s.parse::<i64>().ok())
                            .unwrap_or(0);

                        if loro_ts > org_ts + 1000 {
                            Resolution::UseLoro
                        } else if org_ts > loro_ts + 1000 {
                            Resolution::UseOrg
                        } else {
                            Resolution::Conflict
                        }
                    }
                }
                None => {
                    // Only in Loro - render to org
                    Resolution::UseLoro
                }
            };

            resolutions.push((id.clone(), resolution, Some(loro_block), org_block));
        }

        // Process blocks only in Org
        for (id, org_block) in org_blocks {
            if !seen.contains(id) {
                resolutions.push((id.clone(), Resolution::UseOrg, None, Some(org_block)));
            }
        }

        resolutions
    }

    /// Apply org block to Loro document.
    async fn apply_org_to_loro(&self, org_path: &Path, block: &ParsedBlock) -> Result<()> {
        let store = self.doc_store.write().await;

        let collab_doc = store.get_or_load(org_path).await?;
        let file_id = org_path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        let backend = LoroBackend::from_collaborative_doc(collab_doc, file_id);

        // Check if block exists
        if backend.get_block(&block.id).await.is_ok() {
            // Update existing
            backend.update_block_text(&block.id, &block.content).await?;
        } else {
            // Create new
            let content = holon_api::block::BlockContent::text(block.content.clone());
            backend.create_block(block.parent_id.clone(), content, Some(block.id.clone())).await?;
        }

        // Save
        store.save(org_path).await?;
        Ok(())
    }

    /// Render Loro blocks to org file.
    async fn render_loro_to_org(&self, org_path: &Path) -> Result<()> {
        let store = self.doc_store.read().await;

        if let Some(collab_doc) = store.get(org_path).await {
            let file_id = org_path.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            let backend = LoroBackend::from_collaborative_doc(collab_doc, file_id.clone());

            let blocks = backend.get_all_blocks(Traversal::ALL_BUT_ROOT).await?;
            let org_content = OrgRenderer::render_blocks(&blocks, org_path, &file_id);

            // Mark write to prevent sync loop
            self.write_tracker.write().await.mark_our_write(&org_path.to_string_lossy());

            tokio::fs::write(org_path, org_content).await?;
        }

        Ok(())
    }
}

/// State of a block from Loro document.
#[derive(Debug, Clone)]
struct LoroBlockState {
    id: String,
    content: String,
    properties: HashMap<String, holon_api::Value>,
    updated_at: i64,
    parent_id: String,
}
```

### 2. MODIFY: `crates/holon/src/sync/loro_document_store.rs`

Add these methods to `impl LoroDocumentStore`:

```rust
/// Load all existing .loro files from storage directory.
pub async fn load_all_existing(&mut self) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
    let mut loaded = Vec::new();

    if !self.storage_dir.exists() {
        return Ok(loaded);
    }

    for entry in std::fs::read_dir(&self.storage_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "loro").unwrap_or(false) {
            // Derive org path from loro path
            let org_path = self.org_path_from_loro_path(&path);
            if self.get_or_load(&org_path).await.is_ok() {
                loaded.push(org_path);
            }
        }
    }

    Ok(loaded)
}

/// Get org file path from loro file path.
fn org_path_from_loro_path(&self, loro_path: &Path) -> PathBuf {
    let stem = loro_path.file_stem().unwrap_or_default();
    // Loro files are in storage_dir, org files are relative to root
    // Just change extension for now
    loro_path.with_extension("org")
}
```

### 3. MODIFY: `crates/holon-orgmode/src/lib.rs`

Add export:

```rust
pub mod startup_sync;
pub use startup_sync::{StartupSyncOrchestrator, StartupSyncResult, SyncConflict};
```

### 4. MODIFY: `crates/holon-orgmode/src/di.rs`

Add to imports:

```rust
use crate::startup_sync::{StartupSyncOrchestrator, SyncConflict, StartupSyncResult};
```

Add to `register_services()`, after registering other caches but BEFORE the `add_trait_factory::<dyn OperationProvider>`:

```rust
// Register QueryableCache for SyncConflict
services.add_singleton_factory::<QueryableCache<SyncConflict>, _>(|r| create_queryable_cache(r));
```

Inside the `add_trait_factory::<dyn OperationProvider>` closure, add BEFORE spawning streaming tasks (before line ~210):

```rust
// Perform startup synchronization
let conflict_cache = resolver.get_required::<QueryableCache<SyncConflict>>();
let startup_sync = StartupSyncOrchestrator::new(
    Arc::new(tokio::sync::RwLock::new((*doc_store).clone())),
    (*config).clone(),
    write_tracker.clone(),
    (*renderer).clone(),
);

// Run startup sync (in blocking context since we need it before streams start)
let sync_result = tokio::task::block_in_place(|| {
    tokio::runtime::Handle::current().block_on(async {
        startup_sync.sync_on_startup().await
    })
});

match sync_result {
    Ok(result) => {
        info!(
            "[OrgMode] Startup sync: {} files, {} loro→org, {} org→loro, {} conflicts",
            result.files_processed,
            result.loro_to_org_synced,
            result.org_to_loro_synced,
            result.conflicts.len()
        );

        // Store conflicts in cache
        if !result.conflicts.is_empty() {
            let changes: Vec<_> = result.conflicts.into_iter()
                .map(|c| holon_api::streaming::Change::Created {
                    data: c,
                    origin: holon_api::streaming::ChangeOrigin::Local {
                        operation_id: None,
                        trace_id: None,
                    },
                })
                .collect();

            if let Err(e) = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(
                    conflict_cache.apply_batch(&changes, None)
                )
            }) {
                error!("[OrgMode] Failed to store conflicts: {}", e);
            }
        }
    }
    Err(e) => {
        error!("[OrgMode] Startup sync failed: {}", e);
    }
}
```

### 5. MODIFY: `crates/holon-orgmode/src/loro_renderer.rs`

Make `render_blocks` public if not already:

```rust
/// Render blocks to org-mode text format.
/// This method is pub so it can be used by StartupSyncOrchestrator.
pub fn render_blocks(blocks: &[Block], org_path: &Path, file_id: &str) -> String {
    // ... existing implementation
}
```

## Key Existing Code to Leverage

1. **`loro_diff.rs`**:
   - `ParsedBlock` struct - unified block representation
   - `headlines_to_block_map()` - convert OrgHeadlines to HashMap
   - `diff_blocks()` - comparison algorithm (reference for logic)

2. **`WriteTracker`** in `loro_org_bridge.rs`:
   - `mark_our_write()` - prevent sync loops
   - Already shared via `Arc<RwLock<WriteTracker>>`

3. **`OrgRenderer::render_blocks()`** - Block → org text conversion

4. **`LoroBackend::get_all_blocks(Traversal::ALL_BUT_ROOT)`** - get blocks from Loro doc

## Resolution Logic Summary

| Loro State | Org State | Timestamps | Resolution |
|------------|-----------|------------|------------|
| Exists | Exists | Same content | `Merged` |
| Exists | Exists | Loro >1s newer | `UseLoro` |
| Exists | Exists | Org >1s newer | `UseOrg` |
| Exists | Exists | Within 1s, different | `Conflict` |
| Exists | Missing | - | `UseLoro` |
| Missing | Exists | - | `UseOrg` |

## Testing

After implementation, verify:

1. Fresh start with only .org files → creates .loro files
2. Fresh start with only .loro files → renders to .org files
3. Both exist, same content → no changes
4. Both exist, Loro newer → updates org file
5. Both exist, Org newer → updates Loro doc
6. Both modified within 1s → creates SyncConflict entry
