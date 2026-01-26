# Loro-Orgmode Wiring Implementation Handoff

## Task

Wire up the existing Loro-Orgmode sync implementation so it actually runs. All core components exist but are not connected to the application.

## Files to Modify

### 1. `crates/holon-orgmode/src/di.rs`

Add DI registration for Loro components. Follow the existing patterns in this file.

**Add these imports at top:**
```rust
use crate::loro_org_bridge::LoroOrgBridge;
use holon::sync::LoroDocumentStore;
```

**Add to `OrgModeConfig`:**
```rust
pub struct OrgModeConfig {
    pub root_directory: PathBuf,
    pub loro_storage_dir: PathBuf,  // ADD THIS - where .loro files are stored
}

impl OrgModeConfig {
    pub fn new(root_directory: PathBuf) -> Self {
        // Default loro storage to root_directory/.loro
        let loro_storage_dir = root_directory.join(".loro");
        Self { root_directory, loro_storage_dir }
    }

    pub fn with_loro_storage(root_directory: PathBuf, loro_storage_dir: PathBuf) -> Self {
        Self { root_directory, loro_storage_dir }
    }
}
```

**Add to `register_services` method (after OrgHeadlineOperations registration):**
```rust
// Register LoroDocumentStore
services.add_singleton_factory::<LoroDocumentStore, _>(|resolver| {
    let config = resolver.get_required::<OrgModeConfig>();
    LoroDocumentStore::new(config.loro_storage_dir.clone())
});

// Register LoroOrgBridge
services.add_singleton_factory::<LoroOrgBridge, _>(|resolver| {
    let doc_store = resolver.get_required::<LoroDocumentStore>();
    LoroOrgBridge::new(Arc::new(RwLock::new((*doc_store).clone())))
});
```

**Add bridge startup in the existing spawn block (find `tokio::spawn` that subscribes to streams):**
```rust
// Start Loro bridge (add after the existing stream subscriptions)
let loro_bridge = resolver.get_required::<LoroOrgBridge>();
let headline_ops_for_loro = resolver.get_required::<OrgHeadlineOperations>();
tokio::spawn(async move {
    loro_bridge.start(&headline_ops_for_loro).await;
});
```

### 2. `crates/holon-orgmode/src/lib.rs`

Add re-exports for external use. Add after line 28 (`pub use orgmode_sync_provider::OrgModeSyncProvider;`):

```rust
pub use loro_org_bridge::{LoroOrgBridge, WriteTracker};
pub use loro_renderer::OrgRenderer;
pub use loro_diff::{BlockDiff, ParsedBlock, compute_block_diffs};
pub use link_parser::{Link, extract_links};
```

### 3. `crates/holon-orgmode/src/loro_org_bridge.rs`

The `start` method signature expects `&OrgHeadlineOperations`. Check that it matches what we pass from DI. Current signature at line ~95:
```rust
pub async fn start(&self, headline_ops: &OrgHeadlineOperations)
```

If it needs `Arc<OrgHeadlineOperations>`, adjust accordingly.

## Existing Components (DO NOT MODIFY unless necessary)

- `crates/holon/src/sync/loro_document_store.rs` - Manages LoroDoc per org file
- `crates/holon/src/sync/collaborative_doc.rs` - Has `save_to_file()` and `load_from_file()`
- `crates/holon-orgmode/src/loro_renderer.rs` - Renders Block → org text
- `crates/holon-orgmode/src/loro_diff.rs` - Diff algorithm
- `crates/holon-orgmode/src/loro_org_bridge.rs` - Bridge with `start()` method
- `crates/holon-orgmode/src/link_parser.rs` - Link extraction

## Missing: Loro → Org Write Path

When Loro changes (from P2P sync), org files need updating. This is NOT implemented yet.

**Option A (Simple - recommended for now):** Skip this. Only Org → Loro direction works initially. P2P sync updates Loro but org files won't reflect changes until explicit render is called.

**Option B (Full bidirectional):** Add Loro change subscription. Would require:
1. Subscribe to `LoroDoc::subscribe_deep()` in `LoroOrgBridge`
2. On change, call `OrgRenderer::render_blocks()`
3. Write result to org file with `WriteTracker` marking

Go with Option A for now - the Org → Loro direction is the critical path (external editing of org files).

## Testing After Implementation

```bash
cargo check -p holon-orgmode --features di
cargo test -p holon-orgmode
```

## Key Patterns to Follow

Look at how `OrgModeSyncProvider` is registered and used in `di.rs` - follow the same pattern for `LoroDocumentStore` and `LoroOrgBridge`.

The DI system uses `ferrous_di`. Key methods:
- `services.add_singleton_factory::<Type, _>(|resolver| { ... })` - registers a singleton
- `resolver.get_required::<Type>()` - gets dependency (panics if missing)
- `resolver.get::<Type>()` - gets dependency (returns Result)
