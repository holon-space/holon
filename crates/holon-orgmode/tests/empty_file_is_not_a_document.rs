//! A 0-byte org file is an atomic-save intermediate, never a document.
//!
//! Editors (and every `write to temp + rename` saver) make the final path
//! observable at zero length for a moment before the real bytes land. The
//! watcher sees that moment as a content change like any other, and parsing it
//! yields a document with no blocks — which the ingest diff turns into a delete
//! of every block the document had. The bytes that arrive a millisecond later
//! then have to re-create everything, and anything derived from the store in
//! between saw the document blank.
//!
//! This drives the REAL `FileSyncController::on_file_changed` with the REAL org
//! adapter, and its store doubles APPLY deletes — a double that swallows
//! `delete_in_tree` cannot see the clobber this file is about.
//!
//! @pbt kind harness
//! @pbt covers empty-doc-skip-watcher — a 0-byte file neither ingests as an
//! empty document nor clobbers the blocks of the document that lives at its
//! path

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

const DOC_ID: &str = "20260908T090000";

type Store = Arc<Mutex<HashMap<EntityUri, Block>>>;

/// Applies creates, updates AND deletes. The delete leg is the point: it is the
/// operation a 0-byte parse emits for every block of the document.
#[derive(Clone, Default)]
struct ApplyingOrdering {
    store: Store,
}

fn params_id(params: &holon_api::StorageEntity) -> EntityUri {
    let id = params
        .get("id")
        .and_then(|v| v.as_string())
        .expect("every ingest op names the block it acts on");
    EntityUri::parse(id).expect("the ingest names blocks by uri")
}

#[async_trait]
impl BlockOrdering for ApplyingOrdering {
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
    async fn children(&self, parent: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.parent_id == *parent)
            .map(|b| b.id.clone())
            .collect())
    }
    async fn create_in_tree(
        &self,
        parent_id: &EntityUri,
        _: Option<&EntityUri>,
        id: &EntityUri,
        content: holon_api::BlockContent,
        properties: &HashMap<String, holon_api::Value>,
        edges: &holon_api::BlockEdges,
    ) -> OrderingResult<bool> {
        let mut block = Block::new_text(
            id.clone(),
            parent_id.clone(),
            content.as_text().unwrap_or(""),
        );
        block.properties = properties.clone();
        edges.apply_to(&mut block);
        self.store.lock().unwrap().insert(id.clone(), block);
        Ok(false)
    }
    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        let id = params_id(&params);
        let mut store = self.store.lock().unwrap();
        if let Some(existing) = store.get_mut(&id) {
            if let Some(text) = params.get("content").and_then(|v| v.as_string()) {
                existing.content = text.to_string();
            }
        }
        Ok(())
    }
    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        self.store.lock().unwrap().remove(&params_id(&params));
        Ok(())
    }
}

struct StoreReader(Store);

#[async_trait]
impl BlockReader for StoreReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.id != *doc_id)
            .cloned()
            .collect())
    }
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
    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Default)]
struct PageStore {
    by_id: Arc<Mutex<HashMap<EntityUri, Block>>>,
}

#[async_trait]
impl DocumentManager for PageStore {
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
    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        self.by_id
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(())
    }
}

/// A document with two identified headlines, so the blocks the ingest stores
/// keep stable ids across re-ingests and a disappearance is unambiguous.
fn full_content() -> String {
    format!(
        ":PROPERTIES:\n:ID: {DOC_ID}\n:END:\n#+TITLE: Notes\n\
         * Buy milk\n:PROPERTIES:\n:ID: 20260908T090001\n:END:\n\
         * Call the dentist\n:PROPERTIES:\n:ID: 20260908T090002\n:END:\n"
    )
}

/// A clock the test advances by hand, so the grace period that separates a
/// save in flight from an emptied file is exercised without sleeping through
/// it.
#[derive(Debug, Default)]
struct ManualClock(std::sync::atomic::AtomicI64);

impl holon_api::Clock for ManualClock {
    fn now_millis(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn utc_offset_seconds_at(&self, _: i64) -> i32 {
        0
    }
}

/// Records the disclosure seam's calls, so a test asserts on the signal the
/// frontend's banner is built from rather than on a log line.
#[derive(Default)]
struct Disclosures(Mutex<Vec<String>>);

impl Disclosures {
    fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl holon_filesystem::WritebackDisclosure for Disclosures {
    fn writeback_degraded(&self, _: &str) {}
    fn ingest_refused(&self, path: &std::path::Path, _: &str, _: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("refused {}", path.display()));
    }
    fn ingest_recovered(&self, path: &std::path::Path) {
        self.0
            .lock()
            .unwrap()
            .push(format!("recovered {}", path.display()));
    }
    fn vault_file_emptied(&self, path: &std::path::Path) {
        self.0
            .lock()
            .unwrap()
            .push(format!("emptied {}", path.display()));
    }
}

struct Vault {
    controller: holon_filesystem::FileSyncController,
    store: Store,
    docs: PageStore,
    path: std::path::PathBuf,
    clock: Arc<ManualClock>,
    disclosures: Arc<Disclosures>,
    _tmp: tempfile::TempDir,
}

impl Vault {
    /// The stored blocks of the document, by id — the doc-root excluded.
    fn block_ids(&self) -> Vec<String> {
        let doc = EntityUri::block(DOC_ID);
        let mut ids: Vec<String> = self
            .store
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.id != doc)
            .map(|b| b.id.as_str().to_string())
            .collect();
        ids.sort();
        ids
    }
}

fn vault() -> Vault {
    let uri = EntityUri::block(DOC_ID);
    let mut page = Block::new_text(uri.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(uri, page);

    let ordering = ApplyingOrdering::default();
    let store = ordering.store.clone();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(&path, full_content()).unwrap();

    let clock = Arc::new(ManualClock::default());
    let disclosures = Arc::new(Disclosures::default());
    let controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs.clone()),
        root,
        Arc::new(ordering),
        Arc::new(RealFileSystem),
    )
    .with_clock(clock.clone())
    .with_writeback_disclosure(disclosures.clone());
    Vault {
        controller,
        store,
        docs,
        path,
        clock,
        disclosures,
        _tmp: tmp,
    }
}

/// THE regression: the 0-byte intermediate of an atomic save must leave the
/// document exactly as it was, and the content that lands next must ingest
/// normally.
#[tokio::test]
async fn a_zero_byte_save_intermediate_leaves_the_documents_blocks_alone() {
    let mut v = vault();
    let path = v.path.clone();
    let _ = v
        .controller
        .on_file_changed(&path)
        .await
        .expect("the document ingests");
    let before = v.block_ids();
    assert_eq!(
        before.len(),
        2,
        "the fixture must land two blocks before the empty-file step, else this test proves \
         nothing: {before:?}",
    );

    // The atomic save's first observable state.
    std::fs::write(&path, "").unwrap();
    let _ = v
        .controller
        .on_file_changed(&path)
        .await
        .expect("a 0-byte file must not be an ingest error");

    assert_eq!(
        v.block_ids(),
        before,
        "a 0-byte file was ingested as an empty document and deleted the blocks that live at \
         its path — the save's real bytes had not even landed yet",
    );

    // The bytes the editor was actually writing.
    std::fs::write(&path, full_content()).unwrap();
    let _ = v
        .controller
        .on_file_changed(&path)
        .await
        .expect("the completed save ingests");
    assert_eq!(
        v.block_ids(),
        before,
        "the completed save must restore the document to exactly what it held",
    );
}

/// The skip must not disguise itself as a write-back opportunity: nothing may
/// rewrite the file while it is empty, because the writer that emptied it is
/// still mid-save.
#[tokio::test]
async fn a_zero_byte_file_is_not_written_back() {
    let mut v = vault();
    let path = v.path.clone();
    let _ = v.controller.on_file_changed(&path).await.unwrap();

    std::fs::write(&path, "").unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "",
        "Holon rewrote a file whose save was still in flight — the bytes the editor is about \
         to write would land on top of a projection nobody asked for",
    );
}

/// A whitespace-only file is a file someone WROTE. Only `is_empty()` may
/// refuse: an atomic save's intermediate is exactly zero-length, and widening
/// the guard to `trim().is_empty()` would silently drop a real, if degenerate,
/// edit — and with it the document that file creates.
#[tokio::test]
async fn a_whitespace_only_file_is_still_ingested() {
    let mut v = vault();
    let scratch = v.path.parent().unwrap().join("Scratch.org");
    std::fs::write(&scratch, "\n   \n").unwrap();

    let _ = v
        .controller
        .on_file_changed(&scratch)
        .await
        .expect("a whitespace-only file ingests like any other");

    let titles: Vec<String> = v
        .docs
        .by_id
        .lock()
        .unwrap()
        .values()
        .map(|d| d.title().to_string())
        .collect();
    assert!(
        titles.iter().any(|t| t == "Scratch"),
        "a whitespace-only file was refused, so the document it names never came into being — \
         only a ZERO-length file is a save in flight; titles were {titles:?}",
    );
}

/// A file that is still empty long after any save would have finished is a file
/// a user emptied. Holon keeps the document it last read — an empty file names
/// no document, so nothing in it can authorize deleting one — which leaves the
/// app showing content the file no longer has. That divergence must reach the
/// user, not just the log.
#[tokio::test]
async fn a_file_that_stays_empty_is_disclosed_as_degraded() {
    let mut v = vault();
    let path = v.path.clone();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
    let before = v.block_ids();
    assert_eq!(before.len(), 2, "fixture must land two blocks: {before:?}");

    std::fs::write(&path, "").unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();

    // Still inside the window an atomic save lives in: nothing to say yet.
    v.controller.poll_new_files().await.unwrap();
    assert!(
        !v.disclosures
            .calls()
            .iter()
            .any(|c| c.starts_with("emptied")),
        "a save in flight was reported as an emptied file: {:?}",
        v.disclosures.calls(),
    );

    v.clock
        .0
        .store(60_000, std::sync::atomic::Ordering::Relaxed);
    v.controller.poll_new_files().await.unwrap();
    v.controller.poll_new_files().await.unwrap();

    let emptied: Vec<String> = v
        .disclosures
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("emptied"))
        .collect();
    assert_eq!(
        emptied.len(),
        1,
        "a file the user emptied must raise its degraded condition exactly once, naming the \
         file — silently keeping a document whose file is empty is the divergence nobody can \
         see; calls were {:?}",
        v.disclosures.calls(),
    );
    assert!(emptied[0].contains("Notes.org"), "{emptied:?}");
    assert_eq!(
        v.block_ids(),
        before,
        "the document must be KEPT: an empty file identifies no document, so nothing in it can \
         authorize deleting one",
    );

    // The all-clear: content is back, so the condition it raised is lifted.
    std::fs::write(&path, full_content()).unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
    assert!(
        v.disclosures
            .calls()
            .iter()
            .any(|c| c.starts_with("recovered")),
        "the emptiness condition is sticky and nothing lifted it: {:?}",
        v.disclosures.calls(),
    );
}
