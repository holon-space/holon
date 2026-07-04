//! Focused regression gate for the incremental org-writeback cache (Tier 1).
//!
//! Proves the per-edit O(N) recursive-CTE `get_blocks` is eliminated on the hot
//! content-edit path, while remaining byte-identical to a full-`get_blocks`
//! render:
//!
//! - a single **content-only** edit through `on_block_changed` fires ZERO
//!   `get_blocks` (the production recursive `WITH RECURSIVE descendants` walk) —
//!   it refreshes just the changed block via the authoritative point read
//!   (`get_block_authoritative`) — and the written file matches a full-read
//!   oracle byte-for-byte;
//! - a **tags change** (H4 `Page`-subtree prune), a **move** (`parent_id`
//!   change), and a **remove** each correctly take the full reseed
//!   (`get_blocks`).
//!
//! `get_blocks` here is the seam that in production is the recursive CTE; a
//! counter on it is the direct analog of grepping the SQL trace for
//! `WITH RECURSIVE descendants`.

#![cfg(feature = "di")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::Tags;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::{BlockDelta, BlockReader, DocumentManager, RealFileSystem};
use holon_orgmode::file_sync_controller::new_org_sync_controller;
use holon_orgmode::org_renderer::OrgRenderer;

/// Authoritative block store (stands in for `block_raw`). `get_blocks` (the
/// recursive-CTE seam) and `get_block_authoritative` (the O(1) point read) are
/// call-counted so the test can assert which path a given edit took.
struct CountingBlockReader {
    doc_id: EntityUri,
    /// Ordered children of the doc, exactly as `get_blocks` would return them.
    blocks: Mutex<Vec<Block>>,
    get_blocks_calls: AtomicUsize,
    point_read_calls: AtomicUsize,
}

impl CountingBlockReader {
    fn new(doc_id: EntityUri, blocks: Vec<Block>) -> Self {
        Self {
            doc_id,
            blocks: Mutex::new(blocks),
            get_blocks_calls: AtomicUsize::new(0),
            point_read_calls: AtomicUsize::new(0),
        }
    }

    /// Overwrite the authoritative content of a block by id.
    fn set_block(&self, block: Block) {
        let mut blocks = self.blocks.lock().unwrap();
        if let Some(existing) = blocks.iter_mut().find(|b| b.id == block.id) {
            *existing = block;
        }
    }

    fn get_blocks_calls(&self) -> usize {
        self.get_blocks_calls.load(Ordering::SeqCst)
    }
    fn point_read_calls(&self) -> usize {
        self.point_read_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BlockReader for CountingBlockReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        self.get_blocks_calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(doc_id, &self.doc_id, "test wiring: unexpected doc id");
        Ok(self.blocks.lock().unwrap().clone())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        self.point_read_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.id == *id)
            .cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(vec![(
            self.doc_id.clone(),
            self.blocks.lock().unwrap().clone(),
        )])
    }
}

/// Minimal document manager: the doc is a `Page` block; `name_chain` gives it a
/// single-segment path so `doc_id_to_path` resolves to `<root>/doc.org`.
struct StubDocManager {
    doc: Block,
}

#[async_trait]
impl DocumentManager for StubDocManager {
    async fn find_by_parent_and_name(
        &self,
        _: &EntityUri,
        _: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }

    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok((self.doc.id == *id).then(|| self.doc.clone()))
    }

    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }

    async fn name_chain(&self, _: &EntityUri) -> anyhow::Result<Vec<String>> {
        Ok(vec!["doc".to_string()])
    }
}

/// The ordering seam is untouched by `on_block_changed`; every method is inert.
struct InertOrdering;

#[async_trait]
impl BlockOrdering for InertOrdering {
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
        Ok(vec![])
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
}

struct Harness {
    controller: holon_filesystem::FileSyncController,
    reader: Arc<CountingBlockReader>,
    doc: Block,
    path: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

fn make_doc_and_blocks() -> (Block, Vec<Block>) {
    let doc_id = EntityUri::block("doc-1");
    let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "My Document");
    doc.set_page(true);
    let b1 = Block::new_text(EntityUri::block("b1"), doc_id.clone(), "First heading");
    let b2 = Block::new_text(EntityUri::block("b2"), doc_id.clone(), "Second heading");
    let b3 = Block::new_text(EntityUri::block("b3"), doc_id.clone(), "Third heading");
    (doc, vec![b1, b2, b3])
}

fn build_harness() -> Harness {
    let (doc, blocks) = make_doc_and_blocks();
    let reader = Arc::new(CountingBlockReader::new(doc.id.clone(), blocks));
    let doc_manager = Arc::new(StubDocManager { doc: doc.clone() });
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let fs = Arc::new(RealFileSystem);
    let controller = new_org_sync_controller(
        reader.clone(),
        doc_manager,
        root.clone(),
        Arc::new(InertOrdering),
        fs,
    );
    let path = holon_core::CanonicalPath::new(&root)
        .into_path_buf()
        .join("doc.org");
    Harness {
        controller,
        reader,
        doc,
        path,
        _tmp: tmp,
    }
}

/// Byte-for-byte oracle: render the doc from a full (authoritative) block read,
/// exactly as the pre-Tier-1 `render_file_by_doc_id` did.
fn full_render_oracle(h: &Harness) -> String {
    let blocks = h.reader.blocks.lock().unwrap().clone();
    OrgRenderer::render_document(&h.doc, &blocks, &h.path, &h.doc.id)
}

#[tokio::test]
async fn content_only_edit_serves_from_cache_and_fires_zero_get_blocks() {
    let mut h = build_harness();
    let upsert = |b: &Block| BlockDelta::Upsert(b.clone());

    // First edit to this doc: cold cache → one authoritative reseed (get_blocks).
    let b1_v1 = h.reader.blocks.lock().unwrap()[0].clone();
    h.controller
        .on_block_changed(&h.doc.id, &upsert(&b1_v1))
        .await
        .unwrap();
    assert_eq!(
        h.reader.get_blocks_calls(),
        1,
        "cold doc must seed the cache with exactly one get_blocks"
    );

    // Content-only edit of an already-cached block: MUST NOT call get_blocks.
    let mut b2_v2 = h.reader.blocks.lock().unwrap()[1].clone();
    b2_v2.content = "Second heading EDITED".to_string();
    h.reader.set_block(b2_v2.clone());

    let get_blocks_before = h.reader.get_blocks_calls();
    let point_reads_before = h.reader.point_read_calls();
    h.controller
        .on_block_changed(&h.doc.id, &upsert(&b2_v2))
        .await
        .unwrap();

    assert_eq!(
        h.reader.get_blocks_calls(),
        get_blocks_before,
        "content-only edit fired a recursive-CTE get_blocks — incremental cache path did not run"
    );
    assert_eq!(
        h.reader.point_read_calls(),
        point_reads_before + 1,
        "content-only edit must refresh exactly one block via the authoritative point read"
    );

    // Byte-identical to the full-read oracle.
    let written = std::fs::read_to_string(&h.path).unwrap();
    assert_eq!(
        written,
        full_render_oracle(&h),
        "incremental render diverged from the full-get_blocks render"
    );
    assert!(
        written.contains("Second heading EDITED"),
        "the edit did not reach the file"
    );
}

#[tokio::test]
async fn tags_change_takes_full_reseed() {
    let mut h = build_harness();
    let b1 = h.reader.blocks.lock().unwrap()[0].clone();
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1.clone()))
        .await
        .unwrap();
    let baseline = h.reader.get_blocks_calls();

    // Toggle a tag on a cached block — H4: a Page/tag change can re-partition
    // the doc's subtree, so it must reseed rather than upsert in place.
    let mut b1_tagged = b1.clone();
    let mut tags = Tags::default();
    tags.insert("Page");
    b1_tagged.tags = tags;
    h.reader.set_block(b1_tagged.clone());

    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1_tagged))
        .await
        .unwrap();

    assert_eq!(
        h.reader.get_blocks_calls(),
        baseline + 1,
        "a tags change must take the full reseed (get_blocks)"
    );
}

#[tokio::test]
async fn move_takes_full_reseed() {
    let mut h = build_harness();
    let b3 = h.reader.blocks.lock().unwrap()[2].clone();
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b3.clone()))
        .await
        .unwrap();
    let baseline = h.reader.get_blocks_calls();

    // Re-parent the block (structural move) — position is unknowable from the
    // delta alone, so reseed.
    let mut moved = b3.clone();
    moved.parent_id = EntityUri::block("b1");
    h.reader.set_block(moved.clone());

    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(moved))
        .await
        .unwrap();

    assert_eq!(
        h.reader.get_blocks_calls(),
        baseline + 1,
        "a parent_id move must take the full reseed (get_blocks)"
    );
}

#[tokio::test]
async fn remove_takes_full_reseed() {
    let mut h = build_harness();
    let b1 = h.reader.blocks.lock().unwrap()[0].clone();
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1))
        .await
        .unwrap();
    let baseline = h.reader.get_blocks_calls();

    // A Remove delta can never be served incrementally.
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Remove(EntityUri::block("b2")))
        .await
        .unwrap();

    assert_eq!(
        h.reader.get_blocks_calls(),
        baseline + 1,
        "a Remove must take the full reseed (get_blocks)"
    );
}
