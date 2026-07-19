//! Inc 3 MUST-FIX regression — the shared-subtree ingest guard must key on
//! AUTHORITATIVE mount state, never on parsed drawer content.
//!
//! `:share-role: mount:` / `:shared-tree-id:` drawer properties are lifted
//! verbatim from ANY user file by the org parser, so a guard that skips ingest
//! on content alone would silently drop a hand-authored / imported / templated
//! file carrying such a drawer — a page that never loads, edits that vanish.
//!
//! These tests drive the real `ingest_file` path (via `on_file_changed`) with a
//! recording `DocumentManager` (any call ⇒ ingest proceeded past the guard;
//! zero calls ⇒ the guard skipped) and assert:
//!   * content-looks-like-mount but NOT registered  → INGESTED (no false skip);
//!   * content-looks-like-mount AND registered       → SKIPPED  (guard works);
//!   * no registry seam at all                        → INGESTED (safe
//!     default).

#![cfg(feature = "di")]

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering as AtomicOrdering;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::FileFormatAdapter;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::MountRegistry;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_format::OrgFormatAdapter;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

/// Records how many `DocumentManager` calls happened. No call fires before the
/// ingest guard, so a non-zero count means the file was ingested.
#[derive(Default)]
struct RecordingDocManager {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl DocumentManager for RecordingDocManager {
    async fn find_by_parent_and_name(
        &self,
        _: &EntityUri,
        _: &str,
    ) -> anyhow::Result<Option<Block>> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(None)
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(doc)
    }
    async fn get_by_id(&self, _: &EntityUri) -> anyhow::Result<Option<Block>> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(None)
    }
    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(())
    }
}

struct EmptyReader;

#[async_trait]
impl BlockReader for EmptyReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(Vec::new())
    }
    async fn get_block_authoritative(&self, _: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

struct NoopOrdering;

#[async_trait]
impl BlockOrdering for NoopOrdering {
    async fn place(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> OrderingResult<()> {
        Ok(())
    }
    async fn prev_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn next_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn first_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn last_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn children(&self, _: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(Vec::new())
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
}

/// Stub registry with a fixed answer — models "this id IS / IS NOT a real
/// mount".
struct StubMountRegistry {
    registered: bool,
}

#[async_trait]
impl MountRegistry for StubMountRegistry {
    async fn is_registered_mount(&self, _: &EntityUri) -> anyhow::Result<bool> {
        Ok(self.registered)
    }
}

/// Render a mount-page file exactly as the write-back would, so its content
/// carries the share markers and is guaranteed parseable.
fn mount_page_org(path: &std::path::Path) -> String {
    let doc_uri = EntityUri::block("mount-xyz");
    let mut mount = Block::new_text(doc_uri.clone(), EntityUri::no_parent(), "My Shared Page");
    mount.set_page(true);
    mount.set_property("share-role", "mount");
    mount.set_property("shared-tree-id", "stid-abc");
    mount.set_property("ID", "mount-xyz");
    let mut child = Block::new_text(
        EntityUri::block("child-1"),
        doc_uri.clone(),
        "Child under P",
    );
    child.set_property("shared-tree-id", "stid-abc");
    child.set_property("ID", "child-1");
    OrgFormatAdapter::new().render_document(&mount, &[child], path, &doc_uri)
}

async fn ingest_calls(registry: Option<Arc<dyn MountRegistry>>) -> usize {
    let tmp = tempfile::tempdir().unwrap();
    // Canonicalize so the controller's `strip_prefix(root)` matches (macOS
    // /var/folders is a symlink to /private/var/folders).
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("My Shared Page.org");
    std::fs::write(&path, mount_page_org(&path)).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let mut controller = new_org_sync_controller(
        Arc::new(EmptyReader),
        Arc::new(RecordingDocManager {
            calls: calls.clone(),
        }),
        root,
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    );
    if let Some(reg) = registry {
        controller = controller.with_mount_registry(reg);
    }
    // Ignore the result: the guard's decision is observable via `calls` (any
    // DocumentManager call fires DURING ingest, before any later post-ingest
    // consistency check the minimal stub reader cannot satisfy). A skip returns
    // early with zero calls; a proceed records calls even if the full stubbed
    // ingest later errors.
    let _ = controller.on_file_changed(&path).await;
    calls.load(AtomicOrdering::SeqCst)
}

// MUST-FIX: content looks like a mount but the id is NOT a registered mount →
// the file is INGESTED (the guard must not skip on drawer content alone).
#[tokio::test]
async fn unregistered_share_role_file_is_ingested_not_skipped() {
    let calls = ingest_calls(Some(Arc::new(StubMountRegistry { registered: false }))).await;
    assert!(
        calls > 0,
        "a `share-role` file whose id is NOT a registered mount must be ingested (no false skip)"
    );
}

// The guard still works for a REAL mount: registered → SKIPPED (no ingest).
#[tokio::test]
async fn registered_mount_file_is_skipped() {
    let calls = ingest_calls(Some(Arc::new(StubMountRegistry { registered: true }))).await;
    assert_eq!(
        calls, 0,
        "a file whose id IS a registered mount must be skipped (projection sink)"
    );
}

// Safe default: with no registry seam wired, never skip on content alone.
#[tokio::test]
async fn no_registry_seam_ingests() {
    let calls = ingest_calls(None).await;
    assert!(
        calls > 0,
        "without a mount registry the guard must never skip a share-role file"
    );
}
