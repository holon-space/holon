//! F5 (duplicate-folder-page class): a folder `Areas/` with a companion file
//! `Areas.org` (carrying a `#+ID:`) must project to EXACTLY ONE `Areas` page,
//! no matter the order the initial scan happens to ingest the folder's files
//! in. The children under `Areas/` must be parented onto the companion's
//! authoritative `#+ID`, never onto a second, path-derived phantom container.
//!
//! Root cause (dogfood 2026-07-22, real-vault reingest — Areas×2 / Music×2):
//! the vault walk order is undefined (`ignore::WalkBuilder`, no sort), so a
//! child (`Areas/Music.org`) can be ingested BEFORE its folder companion
//! (`Areas.org`). When it is, the child's parent-chain resolution mints a
//! path-derived placeholder page for the `Areas` segment
//! (`PageId::for_path("Areas")`), and the children attach to THAT. Later, the
//! companion `Areas.org` ingests under its own `#+ID` via `create_forcing_id`
//! (correct — the `#+ID` is authoritative and survives renames), producing a
//! SECOND, childless `Areas` page. Result: two top-level `Areas` pages, one
//! owning the real subtree, one empty. `Resources.org` reconciled fine only
//! because its files happened to scan companion-first.
//!
//! The fix makes parent-chain resolution companion-aware: before minting a
//! path-derived placeholder for a directory segment, the controller peeks the
//! companion file on disk (`<segment-path>.<ext>`) and adopts its `#+ID` as
//! the page identity. Whoever ingests first now creates the `Areas` page under
//! the SAME id the companion will resolve to — order no longer matters.
//!
//! Drives the real `FileSyncController::on_file_changed` boundary against a
//! doc manager that actually STORES created pages (so cross-file adoption is
//! observable, exactly as `LiveDocumentManager` behaves in prod). The pure
//! parse↔render round-trip PBT cannot reach the get_by_id / name-chain layer
//! where this bug lives.
//!
//! @pbt kind harness
//! @pbt covers f5-directory-companion-adoption — a folder companion `#+ID`
//! adopts its directory page deterministically, no phantom container
//! (dogfood 2026-07-22)

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

// Real vault ids (fidelity with the dogfood repro).
const AREAS_ID: &str = "3092ec5e-dd31-497f-938e-4cf8b26a409f";
const MUSIC_ID: &str = "a9e683b4-0cb1-4299-bcf1-67334082610e";

/// No-op ordering: `create_in_tree` runs in degraded mode (`Ok(false)`), so
/// the page store (the doc manager) is the sole authority for page identity —
/// exactly what we want to assert against.
#[derive(Default)]
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

struct EmptyReader;

#[async_trait]
impl BlockReader for EmptyReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(Vec::new())
    }
    /// Delegates to `get_blocks`: this double has no cheaper projection.
    /// Never an empty stub — an empty shape would let the write-back
    /// fold-completeness gate PASS on an incomplete document.
    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(self
            .get_blocks(doc_id)
            .await?
            .into_iter()
            .map(|b| (b.id, b.parent_id))
            .collect())
    }

    async fn get_block_authoritative(&self, _: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }
    /// No junction to resolve against — this double stores marks as given.
    async fn resolve_link_marks(&self, _: &mut [Block]) -> anyhow::Result<()> {
        Ok(())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

/// Prod-faithful, STORING doc manager: mirrors `LiveDocumentManager`.
/// `get_by_id` returns a block only if it is a `Page` (the `WHERE tag='Page'`
/// matview), `create` de-dups by `(parent, title)`, `create_forcing_id`
/// honors the supplied id verbatim. Uses the DEFAULT
/// `get_or_create_by_name_chain` so the controller's real resolution logic
/// runs.
#[derive(Clone, Default)]
struct StoringDocManager {
    by_id: Arc<Mutex<HashMap<EntityUri, Block>>>,
}

impl StoringDocManager {
    fn pages_titled(&self, title: &str) -> Vec<Block> {
        self.by_id
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.is_page() && b.title() == title)
            .cloned()
            .collect()
    }
}

#[async_trait]
impl DocumentManager for StoringDocManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .by_id
            .lock()
            .unwrap()
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        let mut map = self.by_id.lock().unwrap();
        // De-dup by (parent, title), like the live store.
        if let Some(existing) = map
            .values()
            .find(|d| d.parent_id == doc.parent_id && d.is_page() && d.title() == doc.title())
            .cloned()
        {
            return Ok(existing);
        }
        map.insert(doc.id.clone(), doc.clone());
        Ok(doc)
    }
    async fn create_forcing_id(&self, doc: Block) -> anyhow::Result<Block> {
        self.by_id
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .by_id
            .lock()
            .unwrap()
            .get(id)
            .filter(|b| b.is_page())
            .cloned())
    }
    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Build a controller over a real temp-dir vault seeded with the given files.
fn seed_vault(files: &[(&str, &str)]) -> (std::path::PathBuf, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    for (rel, content) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }
    (root, tmp)
}

async fn ingest_in_order(dm: StoringDocManager, root: &std::path::Path, order: &[&str]) {
    let mut controller = new_org_sync_controller(
        Arc::new(EmptyReader),
        Arc::new(dm),
        root.to_path_buf(),
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    );
    for rel in order {
        let path = root.join(rel);
        controller
            .on_file_changed(&path)
            .await
            .unwrap_or_else(|e| panic!("ingest of {rel} must not error: {e:#}"));
    }
}

/// THE BUG: child scanned before its folder companion. There must still be
/// exactly ONE `Areas` page, and `Music` must parent onto the companion's
/// `#+ID` — never a path-derived phantom container.
#[tokio::test]
async fn child_before_companion_yields_single_area_page() {
    let (root, _tmp) = seed_vault(&[
        ("Areas.org", &format!("#+ID: {AREAS_ID}\n")),
        ("Areas/Music.org", &format!("#+ID: {MUSIC_ID}\n")),
    ]);
    let dm = StoringDocManager::default();

    // Child FIRST, companion SECOND — the failing scan order.
    ingest_in_order(dm.clone(), &root, &["Areas/Music.org", "Areas.org"]).await;

    let areas = dm.pages_titled("Areas");
    assert_eq!(
        areas.len(),
        1,
        "expected exactly ONE `Areas` page; got {} — a path-derived phantom \
         container was minted alongside the companion `#+ID` page (F5). ids = {:?}",
        areas.len(),
        areas.iter().map(|b| b.id.to_string()).collect::<Vec<_>>()
    );
    let areas_id = areas[0].id.clone();
    assert_eq!(
        areas_id,
        EntityUri::block(AREAS_ID),
        "the surviving `Areas` page must carry the companion's authoritative `#+ID`, \
         not a path hash"
    );

    let music = dm
        .by_id
        .lock()
        .unwrap()
        .get(&EntityUri::block(MUSIC_ID))
        .cloned()
        .expect("Music page must exist");
    assert_eq!(
        music.parent_id, areas_id,
        "Music must be parented onto the single `Areas` companion page, not a phantom"
    );
}

/// Control: companion scanned before its child (the order `Resources` happened
/// to get). Already worked — this pins order-independence from the other side.
#[tokio::test]
async fn companion_before_child_yields_single_area_page() {
    let (root, _tmp) = seed_vault(&[
        ("Areas.org", &format!("#+ID: {AREAS_ID}\n")),
        ("Areas/Music.org", &format!("#+ID: {MUSIC_ID}\n")),
    ]);
    let dm = StoringDocManager::default();

    ingest_in_order(dm.clone(), &root, &["Areas.org", "Areas/Music.org"]).await;

    let areas = dm.pages_titled("Areas");
    assert_eq!(
        areas.len(),
        1,
        "companion-first must also yield exactly one `Areas` page; ids = {:?}",
        areas.iter().map(|b| b.id.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(areas[0].id, EntityUri::block(AREAS_ID));
}
