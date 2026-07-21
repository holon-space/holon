//! Dogfood 2026-07-21: some ID-only org files reingest as EMPTY-content,
//! ORPHANED `Page`s — a blank sidebar row rooted at depth 0, causing
//! parent/child indentation inversions.
//!
//! Discriminator (verified against the live vault): the on-disk file content is
//! NOT the signal — `Resources.org` and `Projects.org` are byte-identical
//! (`#+ID: <uuid>\n`, 43 bytes each), yet one renders a title and the other a
//! blank row. The difference is the STORE state at ingest time. On boot the
//! Loro tree projects to SQL first, so a `Page` that a `convert_block_to_page`
//! (or a delete-that-left-the-file-on-disk) had persisted with EMPTY content is
//! already present when org ingest runs. `FileSyncController` then resolves the
//! doc-root by its `#+ID` (`get_by_id` → `Some(empty page)`) and REUSES it
//! verbatim — the filename-title default that the create arm applies to a truly
//! new page is bypassed, so `content=""` survives and, for a nested file, the
//! `parent_id=sentinel:no_parent` orphaning survives too.
//!
//! This pins the INGEST-side heal: an empty-content `Page` is never legal, so
//! on reingest of an ID-only file the doc-root's title MUST be re-derived from
//! the filename, and an orphaned nested page MUST be reparented under its
//! folder chain — through the SAME store-mutation seam a normal block update
//! uses. Belt-and-suspenders for already-broken vaults; composes with the
//! writeback-side `#+TITLE:` persistence fix (which stops NEW title-less
//! files).
//!
//! Drives the real `FileSyncController::on_file_changed` boundary; the pure
//! parse↔render round-trip PBT cannot reach the get_by_id / store-reuse layer
//! where this bug lives.
//!
//! @pbt kind harness
//! @pbt covers idonly-title-heal — empty-content Page doc-root heals on
//! reingest (dogfood 2026-07-21)

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

/// Captures every `update_in_tree` params map so a test can assert what the
/// ingest reconciler wrote to the store.
#[derive(Clone, Default)]
struct RecordingOrdering {
    updates: Arc<Mutex<Vec<holon_api::StorageEntity>>>,
}

#[async_trait]
impl BlockOrdering for RecordingOrdering {
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
    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        self.updates.lock().unwrap().push(params);
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
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

/// Prod-faithful doc manager: `get_by_id` returns a block ONLY if it is a
/// `Page`, mirroring `LiveDocumentManager`'s `WHERE tag='Page'` matview. Uses
/// the DEFAULT `get_or_create_by_name_chain`, so the heal's folder-chain
/// derivation runs its real find/create logic against the seeded topology.
struct PageOnlyDocManager {
    by_id: HashMap<EntityUri, Block>,
}

#[async_trait]
impl DocumentManager for PageOnlyDocManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .by_id
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.by_id.get(id).filter(|b| b.is_page()).cloned())
    }
    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Seed one already-broken doc-root: a `Page` with the given id, EMPTY content,
/// and the root sentinel as parent (orphaned).
fn empty_orphan_page(id: &str) -> (EntityUri, PageOnlyDocManager) {
    let uri = EntityUri::block(id);
    let mut broken = Block::new_text(uri.clone(), EntityUri::no_parent(), String::new());
    broken.set_page(true);
    let mut by_id = HashMap::new();
    by_id.insert(uri.clone(), broken);
    (uri, PageOnlyDocManager { by_id })
}

async fn ingest_idonly_file(
    doc_manager: PageOnlyDocManager,
    rel_path: &str,
    doc_id: &str,
) -> Vec<holon_api::StorageEntity> {
    let ordering = RecordingOrdering::default();
    let updates = ordering.updates.clone();

    let tmp = tempfile::tempdir().unwrap();
    // Canonicalize so the controller's `strip_prefix(root)` sees the same
    // `/private/var/...` shape it derives on macOS (where `/var` is a symlink).
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join(rel_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // The exact live shape: a single `#+ID:` line, no `#+TITLE:`.
    std::fs::write(&path, format!("#+ID: {doc_id}\n")).unwrap();

    let mut controller = new_org_sync_controller(
        Arc::new(EmptyReader),
        Arc::new(doc_manager),
        root.clone(),
        Arc::new(ordering),
        Arc::new(RealFileSystem),
    );
    controller
        .on_file_changed(&path)
        .await
        .expect("ingest of an ID-only file must not error");

    let recorded = updates.lock().unwrap().clone();
    recorded
}

fn doc_root_update<'a>(
    recorded: &'a [holon_api::StorageEntity],
    id: &EntityUri,
) -> &'a holon_api::StorageEntity {
    recorded
        .iter()
        .find(|p| p.get("id").and_then(|v| v.as_string()) == Some(id.to_string().as_str()))
        .unwrap_or_else(|| {
            panic!(
                "no store update was written for the title-less doc-root {id} — the \
                 empty-content Page was NOT healed on ingest (blank sidebar row survives). \
                 recorded updates = {recorded:?}"
            )
        })
}

/// Nested + spaced folder: the `Agentic DPL.org` shape. The empty-content page
/// must gain the filename-derived title AND be reparented under its folder
/// chain (`Projects/DBG`) instead of staying an orphaned depth-0 root.
#[tokio::test]
async fn idonly_page_in_spaced_subdir_heals_title_and_reparents() {
    let doc_id = "9464fbf0-412c-3aa7-f63b-810108800000";
    let (uri, dm) = empty_orphan_page(doc_id);

    let recorded = ingest_idonly_file(dm, "Projects/DBG/Agentic DPL.org", doc_id).await;
    let update = doc_root_update(&recorded, &uri);

    assert_eq!(
        update.get("content").and_then(|v| v.as_string()),
        Some("Agentic DPL"),
        "doc-root content must be the filename-derived title (spaces preserved), not empty"
    );

    let expected_parent = holon_api::link_parser::PageId::for_path("Projects/DBG")
        .expect("valid page path")
        .into_entity_uri();
    let parent = update.get("parent_id").and_then(|v| v.as_string());
    assert_eq!(
        parent,
        Some(expected_parent.to_string().as_str()),
        "an orphaned nested page must be reparented under its folder chain (DBG), never \
         left at the root sentinel (which seeds a depth-0 root and inverts indentation)"
    );
    assert_ne!(
        parent,
        Some(EntityUri::no_parent().to_string().as_str()),
        "healed nested doc-root must not remain root-parented"
    );
}

/// Root-level file (the `Resources.org` shape): content heals to the filename;
/// the parent correctly STAYS the root sentinel (a top-level page has no folder
/// parent), so the heal must not invent one.
#[tokio::test]
async fn idonly_root_page_heals_title_and_stays_root() {
    let doc_id = "a9163ed8-c431-115c-ff92-3faba5400000";
    let (uri, dm) = empty_orphan_page(doc_id);

    let recorded = ingest_idonly_file(dm, "Resources.org", doc_id).await;
    let update = doc_root_update(&recorded, &uri);

    assert_eq!(
        update.get("content").and_then(|v| v.as_string()),
        Some("Resources"),
        "root-level doc-root content must be the filename-derived title, not empty"
    );
    assert_eq!(
        update.get("parent_id").and_then(|v| v.as_string()),
        Some(EntityUri::no_parent().to_string().as_str()),
        "a top-level page has no folder parent — the heal must keep it root-parented"
    );
}
