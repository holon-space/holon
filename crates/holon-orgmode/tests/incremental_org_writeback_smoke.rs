//! Focused regression gate for the incremental org-writeback cache (Tier 1).
//!
//! Proves the per-edit O(N) recursive-CTE `get_blocks` is eliminated on the hot
//! content-edit path, while remaining byte-identical to a full-`get_blocks`
//! render:
//!
//! - a single **content-only** edit through `on_block_changed` fires ZERO
//!   `get_blocks` (the production recursive `WITH RECURSIVE descendants` walk)
//!   — it refreshes just the changed block via the authoritative point read
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

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::Tags;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockDelta;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
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
    /// Counted SEPARATELY from `get_blocks`: the fold-completeness gate's
    /// id-only membership read is a different seam with a different cost, and
    /// folding it into `get_blocks_calls` would silently retire that pin.
    doc_block_topology_calls: AtomicUsize,
    point_read_calls: AtomicUsize,
}

impl CountingBlockReader {
    fn new(doc_id: EntityUri, blocks: Vec<Block>) -> Self {
        Self {
            doc_id,
            blocks: Mutex::new(blocks),
            get_blocks_calls: AtomicUsize::new(0),
            doc_block_topology_calls: AtomicUsize::new(0),
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

    /// Replace the whole ordered block list — simulates a same-parent
    /// reorder (a new `sort_key` sequence), where `get_blocks`/`children`
    /// would now return a different order for unchanged blocks.
    fn set_blocks(&self, blocks: Vec<Block>) {
        *self.blocks.lock().unwrap() = blocks;
    }

    fn get_blocks_calls(&self) -> usize {
        self.get_blocks_calls.load(Ordering::SeqCst)
    }
    fn doc_block_topology_calls(&self) -> usize {
        self.doc_block_topology_calls.load(Ordering::SeqCst)
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

    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        self.doc_block_topology_calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(doc_id, &self.doc_id, "test wiring: unexpected doc id");
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .map(|b| (b.id.clone(), b.parent_id.clone()))
            .collect())
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

/// `place`/`update_in_tree`/`delete_in_tree` are untouched by
/// `on_block_changed` and stay inert. `children` is NOT inert: the cheap
/// content-only-edit path in `render_with_cache` compares the cache's
/// same-parent sibling order against this live read to detect a same-parent
/// reorder (sort_key moved without a `parent_id`/`tags` change), so this stub
/// mirrors the authoritative store — exactly like production, where
/// `BlockOrdering::children` and `BlockReader::get_blocks` both read the same
/// underlying order.
struct LiveOrderOrdering {
    reader: Arc<CountingBlockReader>,
}

#[async_trait]
impl BlockOrdering for LiveOrderOrdering {
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
    async fn children(&self, parent_id: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(self
            .reader
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id.clone())
            .collect())
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

/// A `Page` doc with `n` leaf children `b0..b{n-1}` (content `heading {i}`) —
/// used by the mass-truncation and name_chain-failure cases, which need more
/// blocks than the three-block fixture above.
fn make_doc_and_n_blocks(n: usize) -> (Block, Vec<Block>) {
    let doc_id = EntityUri::block("doc-1");
    let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "My Document");
    doc.set_page(true);
    let blocks = (0..n)
        .map(|i| {
            Block::new_text(
                EntityUri::block(&format!("b{i}")),
                doc_id.clone(),
                format!("heading {i}"),
            )
        })
        .collect();
    (doc, blocks)
}

fn build_harness() -> Harness {
    let (doc, blocks) = make_doc_and_blocks();
    build_harness_from(doc, blocks)
}

fn build_harness_with_blocks(n: usize) -> Harness {
    let (doc, blocks) = make_doc_and_n_blocks(n);
    build_harness_from(doc, blocks)
}

fn build_harness_from(doc: Block, blocks: Vec<Block>) -> Harness {
    let reader = Arc::new(CountingBlockReader::new(doc.id.clone(), blocks));
    let doc_manager = Arc::new(StubDocManager { doc: doc.clone() });
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let fs = Arc::new(RealFileSystem);
    let controller = new_org_sync_controller(
        reader.clone(),
        doc_manager,
        root.clone(),
        Arc::new(LiveOrderOrdering {
            reader: reader.clone(),
        }),
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

/// A benign, non-`Page` tag used PURELY to force a full reseed: the render
/// cache reseeds whenever a block's `tags` differ from the cached copy (see
/// `render_with_cache`), regardless of WHICH tag changed. It must NOT be
/// `Page`: since the 2026-07-19 daily-note ruling, `on_block_changed` runs
/// `materialize_page_identity_file` on any authoritative page upsert, which
/// `authoritative_name_chain`-bails on these single-doc mocks (whose doc-root
/// block is absent from `get_block_authoritative`) — an interaction unrelated
/// to the removal-guard behaviour these tests pin.
fn reseed_lever_tags() -> Tags {
    let mut tags = Tags::default();
    tags.insert("Todo");
    tags
}

/// Seed the write-back holder the way production does at boot: production folds
/// the block feed's initial snapshot through `home_by`, a controller-level test
/// folds the same content from the authority. Every test that drives
/// `on_block_changed` must do this first — the holder IS the render input now,
/// and an unseeded document renders as an empty one.
async fn seed(h: &mut Harness) {
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
}

/// The `Upsert` `home_by` would emit for `block`: its value plus the previous
/// sibling the authority currently gives it. Derived rather than hardcoded so a
/// fixture reorder cannot silently desynchronise the delta from the oracle.
fn upsert(h: &Harness, block: &Block) -> BlockDelta {
    let blocks = h.reader.blocks.lock().unwrap();
    let prev = blocks
        .iter()
        .take_while(|b| b.id != block.id)
        .filter(|b| b.parent_id == block.parent_id)
        .last()
        .map(|b| b.id.clone());
    BlockDelta::Upsert {
        block: block.clone(),
        prev,
    }
}

/// The write-back path issues NO document read of its own. Under the deleted
/// cache this was true only for the "cheap" content-only branch and had to be
/// defended by a three-part guard; the holder makes it unconditional, because
/// the combinator — not the controller — decides what the document contains.
#[tokio::test]
async fn edits_render_from_the_holder_and_fire_zero_get_blocks() {
    let mut h = build_harness();
    seed(&mut h).await;
    let after_seed = h.reader.get_blocks_calls();
    let topo_after_seed = h.reader.doc_block_topology_calls();

    let mut b2_v2 = h.reader.blocks.lock().unwrap()[1].clone();
    b2_v2.content = "Second heading EDITED".to_string();
    h.reader.set_block(b2_v2.clone());

    let delta = upsert(&h, &b2_v2);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();

    assert_eq!(
        h.reader.get_blocks_calls(),
        after_seed,
        "the holder path re-read the document — write-back must render from folded feed state, \
         never from a document-scoped authority read"
    );
    // The fold-completeness gate's membership read, pinned EXACTLY. Zero would
    // mean the gate stopped checking and a partially-folded holder can reach
    // disk again; more than one would mean a render costs several doc-scoped
    // round-trips, which is the cost this whole seam exists to avoid.
    assert_eq!(
        h.reader.doc_block_topology_calls() - topo_after_seed,
        1,
        "one edit must cost EXACTLY one topology read: the gate compares the holder against \
         the authority once per render"
    );

    let written = std::fs::read_to_string(&h.path).unwrap();
    assert_eq!(
        written,
        full_render_oracle(&h),
        "holder render diverged from the full-get_blocks render"
    );
    assert!(
        written.contains("Second heading EDITED"),
        "the edit did not reach the file"
    );
}

/// A `tags` change is no longer a reason to re-read anything — it is one upsert
/// like any other. What must hold is that the new tags reach disk.
#[tokio::test]
async fn a_tags_change_reaches_the_file() {
    let mut h = build_harness();
    seed(&mut h).await;
    let after_seed = h.reader.get_blocks_calls();

    let mut b1_tagged = h.reader.blocks.lock().unwrap()[0].clone();
    b1_tagged.tags = reseed_lever_tags();
    h.reader.set_block(b1_tagged.clone());

    let delta = upsert(&h, &b1_tagged);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();

    assert_eq!(
        h.reader.get_blocks_calls(),
        after_seed,
        "a tags change must not re-read the document"
    );
    assert_eq!(
        std::fs::read_to_string(&h.path).unwrap(),
        full_render_oracle(&h),
        "the tags change did not reach the file"
    );
}

/// A re-parent renders the block under its new parent. The old cache could only
/// discover this by re-reading the whole document ("position is unknowable from
/// the delta alone"); the homed delta states it outright.
#[tokio::test]
async fn a_reparent_delta_renders_under_the_new_parent() {
    let mut h = build_harness();
    seed(&mut h).await;

    let b3 = h.reader.blocks.lock().unwrap()[2].clone();
    let mut moved = b3.clone();
    moved.parent_id = EntityUri::block("b1");
    h.reader.set_block(moved.clone());

    // Re-parenting empties b3's old sibling slot, so the authority now gives it
    // no predecessor under b1.
    let delta = BlockDelta::Upsert {
        block: moved,
        prev: None,
    };
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(&h.path).unwrap(),
        full_render_oracle(&h),
        "the re-parented block did not render under its new parent"
    );
}

/// Regression for the disk-sibling-order bug. Under the deleted cache a
/// same-parent reorder was invisible to a `parent_id`/`tags` comparison, so the
/// file kept the pre-reorder order forever unless a `children()` cross-read
/// caught it (guard 3). The holder carries position IN the delta, so the same
/// scenario is now pinned directly on the order rather than on the read that
/// used to detect it: `home_by` re-emits every neighbour whose predecessor
/// moved, and the render must follow that chain.
#[tokio::test]
async fn a_reorder_delta_reorders_the_written_file() {
    let mut h = build_harness();
    seed(&mut h).await;

    let (b1, b2, b3) = {
        let blocks = h.reader.blocks.lock().unwrap();
        (blocks[0].clone(), blocks[1].clone(), blocks[2].clone())
    };
    // b2 now sorts before b1; b3 keeps the tail.
    h.reader
        .set_blocks(vec![b2.clone(), b1.clone(), b3.clone()]);

    // The three diffs the combinator emits for this move: the new head, the
    // moved block, and the neighbour whose predecessor changed.
    for block in [&b2, &b1, &b3] {
        let delta = upsert(&h, block);
        h.controller
            .on_block_changed(&h.doc.id, &delta)
            .await
            .unwrap();
    }

    let written = std::fs::read_to_string(&h.path).unwrap();
    assert_eq!(
        written,
        full_render_oracle(&h),
        "written file must reflect the new sibling order, not the stale holder one"
    );
    assert!(
        written.find("Second heading").unwrap() < written.find("First heading").unwrap(),
        "b2 must render before b1 after the reorder: {written:?}"
    );
}

/// A sanctioned `Remove` drops the block from the file — and does NOT trip the
/// now-unconditional removal guard, because the holder's own retraction is what
/// grounds it.
#[tokio::test]
async fn a_remove_delta_drops_the_block_from_the_file() {
    let mut h = build_harness();
    seed(&mut h).await;

    let b1 = h.reader.blocks.lock().unwrap()[0].clone();
    let delta = upsert(&h, &b1);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();

    let surviving: Vec<Block> = h
        .reader
        .blocks
        .lock()
        .unwrap()
        .iter()
        .filter(|b| b.id != EntityUri::block("b2"))
        .cloned()
        .collect();
    h.reader.set_blocks(surviving);
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Remove(EntityUri::block("b2")))
        .await
        .unwrap();

    let written = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        !written.contains("Second heading"),
        "the removed block is still on disk: {written:?}"
    );
    assert_eq!(
        written,
        full_render_oracle(&h),
        "the post-removal file diverged from the full-read render"
    );
}

/// **A SINGLE ungrounded drop on the block-driven path VETOES + quarantines.**
/// After the file is on disk with b1/b2/b3, the store loses b2 and a reseed
/// renders b1/b3 while the delta is an `Upsert` — no `Remove` op sanctions b2's
/// disappearance and no sibling file carries it. One destroyed block is still
/// destroyed, so the write is REFUSED (ADR 0025), disk keeps all three blocks,
/// and the file is quarantined. A genuine user deletion is the neighbouring
/// case: it arrives as `Remove(b2)` and writes cleanly
/// (`block_driven_writeback_writes_sanctioned_deletion`).
#[tokio::test]
async fn block_driven_writeback_vetoes_single_ungrounded_drop() {
    let mut h = build_harness();
    let all = h.reader.blocks.lock().unwrap().clone();
    let (b1, b3) = (all[0].clone(), all[2].clone());

    // Seed disk with the full doc (b1/b2/b3).
    // Production seeds the holder from the block feed's snapshot; a
    // controller-level driver seeds the same content from the authority,
    // so the delta's position and the render input share one source.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b1);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();
    let on_disk = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        on_disk.contains("Second heading"),
        "precondition: b2 must be on disk; got {on_disk:?}"
    );

    // The store loses b2. Force a reseed via a tags change on b1 so the render is
    // b1/b3 — b2 dropped, delta is an Upsert (no delete op, no sibling →
    // ungrounded).
    h.reader.set_blocks(vec![b1.clone(), b3.clone()]);
    let mut b1_tagged = b1.clone();
    b1_tagged.tags = reseed_lever_tags();
    h.reader.set_block(b1_tagged.clone());

    // Re-seed from the authority the way a fresh combinator
    // subscription would: the block that vanished from the store is
    // gone from the holder too, so the render shrinks and the removal
    // guard sees an absence nothing sanctioned.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b1_tagged);
    let veto = h.controller.on_block_changed(&h.doc.id, &delta).await;
    assert!(
        veto.is_err(),
        "an ungrounded drop must VETO the write-back (Err), however small"
    );
    let msg = format!("{:#}", veto.unwrap_err());
    assert!(
        msg.contains("UNGROUNDED WRITE-BACK REMOVAL"),
        "veto error must name the removal guard; got {msg}"
    );

    // Disk is intact — the lossy projection was refused.
    let after = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after.contains("Second heading"),
        "the vetoed write must leave b2 on disk; got {after:?}"
    );

    // Quarantined: a later edit is skipped rather than written.
    let mut b1_v2 = b1.clone();
    b1_v2.content = "First heading edited".to_string();
    h.reader.set_blocks(vec![b1_v2.clone(), b3.clone()]);
    // Production seeds the holder from the block feed's snapshot; a
    // controller-level driver seeds the same content from the authority,
    // so the delta's position and the render input share one source.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b1_v2);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .expect("a quarantined file's later edit is skipped (Ok), not an error");
    let after_edit = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after_edit.contains("Second heading"),
        "the file stays quarantined — no lossy write over disk; got {after_edit:?}"
    );
}

/// **A MASS TRUNCATION (the row-28 signature) VETOES + quarantines.** A
/// 20-block doc is on disk; the store then loses 15 of them (an
/// FK-rollback-style truncation — NOT a user delete: the delta is an `Upsert`
/// and none has a sibling file). A reseed renders only 5 blocks; writing it
/// would DELETE 15 lines from disk. The write is REFUSED (Err), disk is left
/// intact, and the file is quarantined so a later edit is skipped too. The
/// single-drop sibling case above proves the guard is not size-gated; this one
/// pins the original row-28 shape end-to-end.
#[tokio::test]
async fn block_driven_writeback_vetoes_mass_truncation() {
    let mut h = build_harness_with_blocks(20);
    let all = h.reader.blocks.lock().unwrap().clone();
    let b0 = all[0].clone();

    // Seed disk with all 20 blocks.
    // Production seeds the holder from the block feed's snapshot; a
    // controller-level driver seeds the same content from the authority,
    // so the delta's position and the render input share one source.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b0);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();
    let seeded = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        seeded.contains("heading 19"),
        "precondition: all 20 blocks on disk; got {seeded:?}"
    );

    // The store TRUNCATES to the first 5 blocks (15 lost). Force a reseed via a
    // tags change on b0 — delta is an Upsert (no sanctioned removal).
    let keep: Vec<Block> = all[..5].to_vec();
    h.reader.set_blocks(keep);
    let mut b0_tagged = b0.clone();
    b0_tagged.tags = reseed_lever_tags();
    h.reader.set_block(b0_tagged.clone());

    // Re-seed from the authority the way a fresh combinator
    // subscription would: the block that vanished from the store is
    // gone from the holder too, so the render shrinks and the removal
    // guard sees an absence nothing sanctioned.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b0_tagged);
    let veto = h.controller.on_block_changed(&h.doc.id, &delta).await;
    assert!(
        veto.is_err(),
        "a 15-of-20 mass truncation must VETO the write-back (Err), not write 5 blocks"
    );
    let msg = format!("{:#}", veto.unwrap_err());
    assert!(
        msg.contains("UNGROUNDED WRITE-BACK REMOVAL"),
        "veto error must name the removal guard; got {msg}"
    );

    // Disk is intact — the truncated projection was refused.
    let after_veto = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after_veto.contains("heading 19"),
        "the vetoed write must leave all 20 blocks on disk; got {after_veto:?}"
    );

    // Quarantined: a subsequent benign edit is skipped (no write over disk).
    // Production seeds the holder from the block feed's snapshot; a
    // controller-level driver seeds the same content from the authority,
    // so the delta's position and the render input share one source.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b0);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .expect("a quarantined file's later edit is skipped (Ok), not an error");
    let after_second = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after_second.contains("heading 19"),
        "the file stays quarantined — no truncated write over disk; got {after_second:?}"
    );
}

/// **Fork B B1' (RED-first): a GENUINE user deletion writes the shrunken file
/// WITHOUT any drop warning.** Same drop shape as above, but the triggering
/// delta is `Remove(b2)` — an op that SANCTIONS b2's disappearance (ADR 0025).
/// The guard grounds b2 in the delta's `Remove` set, so the shrunken b1/b3 file
/// is written cleanly (no drop surfaced). This is the controller's own Remove
/// path — the production block-feed resolver (di.rs) reaches it by routing both
/// a genuine feed removal and a cross-doc DEPARTURE (`LiveData::group_by`'s
/// `Remove{old doc}`) as an `on_block_changed` `Remove` delta.
#[tokio::test]
async fn block_driven_writeback_writes_sanctioned_deletion() {
    let mut h = build_harness();
    let all = h.reader.blocks.lock().unwrap().clone();
    let (b1, b3) = (all[0].clone(), all[2].clone());

    // Production seeds the holder from the block feed's snapshot; a
    // controller-level driver seeds the same content from the authority,
    // so the delta's position and the render input share one source.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b1);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();

    // The user deletes b2: the store loses it AND the delta carries Remove(b2).
    h.reader.set_blocks(vec![b1, b3]);
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Remove(EntityUri::block("b2")))
        .await
        .expect("a sanctioned deletion (Remove delta) must write, never veto");

    let written = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        !written.contains("Second heading"),
        "the sanctioned deletion of b2 must shrink the file; got {written:?}"
    );
    assert!(
        written.contains("First heading") && written.contains("Third heading"),
        "the surviving blocks b1/b3 must remain; got {written:?}"
    );
}

/// A `DocumentManager` that resolves a de-inlined CHILD PAGE to its own file
/// (`doc/Child.org`), alongside the document itself. The doc record outlives
/// the block: `name_chain` keeps answering for the child id even after the
/// block store has lost it, which is what makes the row-28 shape expressible.
struct ChildPageDocManager {
    doc: Block,
    child_page: EntityUri,
}

#[async_trait]
impl DocumentManager for ChildPageDocManager {
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

    async fn name_chain(&self, id: &EntityUri) -> anyhow::Result<Vec<String>> {
        if *id == self.child_page {
            Ok(vec!["doc".to_string(), "Child".to_string()])
        } else {
            Ok(vec!["doc".to_string()])
        }
    }
}

/// `n` leaf children plus a de-inlined child PAGE that owns `doc/Child.org`.
fn build_harness_with_child_page(n: usize) -> (Harness, EntityUri) {
    let (doc, mut blocks) = make_doc_and_n_blocks(n);
    let child_page_id = EntityUri::block("child-page");
    let mut child_page = Block::new_text(child_page_id.clone(), doc.id.clone(), "Child Page");
    child_page.set_page(true);
    blocks.push(child_page);

    let reader = Arc::new(CountingBlockReader::new(doc.id.clone(), blocks));
    let doc_manager = Arc::new(ChildPageDocManager {
        doc: doc.clone(),
        child_page: child_page_id.clone(),
    });
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let fs = Arc::new(RealFileSystem);
    let controller = new_org_sync_controller(
        reader.clone(),
        doc_manager,
        root.clone(),
        Arc::new(LiveOrderOrdering {
            reader: reader.clone(),
        }),
        fs,
    );
    let path = holon_core::CanonicalPath::new(&root)
        .into_path_buf()
        .join("doc.org");
    let h = Harness {
        controller,
        reader,
        doc,
        path,
        _tmp: tmp,
    };
    (h, child_page_id)
}

/// **A PAGE block LOST from the authority still vetoes, even though its doc
/// record resolves to a file.** The grounding witness for a cross-file move is
/// "the authority places this block in another file" — but a doc/alias record
/// can outlive the block itself (the row-28 shape: the store loses a de-inlined
/// child page while its document row survives). Resolving a path must therefore
/// never ground a removal on its own; the authority has to still HOLD the
/// block. Otherwise this exact truncation writes the parent file with the
/// child's heading silently deleted.
#[tokio::test]
async fn lost_child_page_with_surviving_doc_record_still_vetoes() {
    let (mut h, child_page_id) = build_harness_with_child_page(3);
    let all = h.reader.blocks.lock().unwrap().clone();
    let b0 = all[0].clone();

    // Production seeds the holder from the block feed's snapshot; a
    // controller-level driver seeds the same content from the authority,
    // so the delta's position and the render input share one source.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b0);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();
    let seeded = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        seeded.contains("Child Page"),
        "precondition: the child page heading is on disk; got {seeded:?}"
    );

    // The store loses the child page; its doc record still resolves a path.
    let survivors: Vec<Block> = all
        .iter()
        .filter(|b| b.id != child_page_id)
        .cloned()
        .collect();
    h.reader.set_blocks(survivors);
    let mut b0_tagged = b0.clone();
    b0_tagged.tags = reseed_lever_tags();
    h.reader.set_block(b0_tagged.clone());

    // Re-seed from the authority the way a fresh combinator
    // subscription would: the block that vanished from the store is
    // gone from the holder too, so the render shrinks and the removal
    // guard sees an absence nothing sanctioned.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b0_tagged);
    let veto = h.controller.on_block_changed(&h.doc.id, &delta).await;
    assert!(
        veto.is_err(),
        "a page block absent from the AUTHORITY is a loss, not a move — it must veto"
    );
    let msg = format!("{:#}", veto.unwrap_err());
    assert!(
        msg.contains("UNGROUNDED WRITE-BACK REMOVAL"),
        "veto error must name the removal guard; got {msg}"
    );

    let after = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after.contains("Child Page"),
        "the vetoed write must leave the child heading on disk; got {after:?}"
    );
}

/// A `DocumentManager` whose `name_chain` resolves the DOCUMENT (so
/// `on_block_changed` can find the file to write back) but FAILS LOUD for every
/// other id. This is the exact shape of the real-vault first-boot corruption
/// (BugFunnel row 23/29): a same-named subdir page (`Projects/Holon.org`)
/// collided with a `* Holon` heading in `Projects.org`, the re-homed subtree
/// blocks landed in a prohibited page-under-non-page topology, and
/// `name_chain(child)` bailed (`sync_ports.rs`) — a 749-strong error storm.
struct FailingChildNameChainDocManager {
    doc: Block,
}

#[async_trait]
impl DocumentManager for FailingChildNameChainDocManager {
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

    async fn name_chain(&self, id: &EntityUri) -> anyhow::Result<Vec<String>> {
        if *id == self.doc.id {
            Ok(vec!["doc".to_string()])
        } else {
            anyhow::bail!(
                "name_chain({id}): non-page ancestor found while walking to root — pages under \
                 non-pages are prohibited (test stand-in for the real-vault first-boot topology)"
            )
        }
    }
}

fn build_harness_with_failing_child_name_chain(n: usize) -> Harness {
    let (doc, blocks) = make_doc_and_n_blocks(n);
    let reader = Arc::new(CountingBlockReader::new(doc.id.clone(), blocks));
    let doc_manager = Arc::new(FailingChildNameChainDocManager { doc: doc.clone() });
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let fs = Arc::new(RealFileSystem);
    let controller = new_org_sync_controller(
        reader.clone(),
        doc_manager,
        root.clone(),
        Arc::new(LiveOrderOrdering {
            reader: reader.clone(),
        }),
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

/// **BugFunnel row 23/29: a drop whose blocks FAIL `name_chain` (a prohibited
/// page-under-non-page topology) is reported as UNRESOLVABLE, not as a plain
/// ungrounded drop.** This is the first-boot Projects.org destruction: the
/// re-homed subtree blocks were absent from the projection and their
/// `name_chain` failed loud (the 749-error storm). Both arms veto, but the
/// grounding failure must say so — otherwise the diagnosis of a topology bug
/// reads as a routine removal refusal.
#[tokio::test]
async fn name_chain_failed_ungrounded_drop_hard_vetoes() {
    // 8 blocks on disk, 2 dropped. The dropped blocks' own-file path cannot be
    // resolved, so the veto must come from the name_chain grounding failure and
    // carry its message.
    let mut h = build_harness_with_failing_child_name_chain(8);
    let all = h.reader.blocks.lock().unwrap().clone();
    let b0 = all[0].clone();

    // Seed disk with all 8 blocks.
    // Production seeds the holder from the block feed's snapshot; a
    // controller-level driver seeds the same content from the authority,
    // so the delta's position and the render input share one source.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b0);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();
    let seeded = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        seeded.contains("heading 7"),
        "precondition: all 8 blocks on disk; got {seeded:?}"
    );

    // The store loses the last 2 blocks. Force a reseed via a tags change on b0
    // (Upsert, no sanctioned removal). The dropped blocks' `name_chain` fails
    // loud → UNRESOLVABLE, so that error wins over the plain-removal one.
    let keep: Vec<Block> = all[..6].to_vec();
    h.reader.set_blocks(keep);
    let mut b0_tagged = b0.clone();
    b0_tagged.tags = reseed_lever_tags();
    h.reader.set_block(b0_tagged.clone());

    // Re-seed from the authority the way a fresh combinator
    // subscription would: the block that vanished from the store is
    // gone from the holder too, so the render shrinks and the removal
    // guard sees an absence nothing sanctioned.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b0_tagged);
    let veto = h.controller.on_block_changed(&h.doc.id, &delta).await;
    assert!(
        veto.is_err(),
        "a name_chain-failed ungrounded drop must HARD-VETO the write (Err)"
    );
    let msg = format!("{:#}", veto.unwrap_err());
    assert!(
        msg.contains("UNRESOLVABLE"),
        "veto error must name the unresolvable-drop guard; got {msg}"
    );

    // Disk intact — the truncated projection was refused.
    let after = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after.contains("heading 7"),
        "the vetoed write must leave all 8 blocks on disk; got {after:?}"
    );

    // Quarantined: a subsequent benign edit is skipped (no truncated write).
    // Production seeds the holder from the block feed's snapshot; a
    // controller-level driver seeds the same content from the authority,
    // so the delta's position and the render input share one source.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b0);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .expect("a quarantined file's later edit is skipped (Ok), not an error");
    let after_second = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after_second.contains("heading 7"),
        "the file stays quarantined — no truncated write over disk; got {after_second:?}"
    );
}

// ── Fork B / LogSeq-parity daily-note ruling (2026-07-19)
// ───────────────────── A page CREATED AT RUNTIME with NO children MUST still
// materialize its OWN identity file (`#+ID:` header) —
// `inv-every-page-has-its-own-file` is unconditional; a page's file existence
// must never depend on it having children. Before the fix, `on_block_changed`
// resolved a page's path + doc-root header through the documents registry (a
// lagging `WHERE tag='Page'` matview), so a just-minted childless page (a
// rule-minted journal date, `convert_block_to_page` on a childless block) was
// skipped and left FILELESS. The fix resolves BOTH from the authoritative block
// store. This models the registry MISS explicitly (the doc-manager stub does
// NOT know the page).

/// Authoritative store for exactly ONE childless page: `get_blocks` returns no
/// children, `get_block_authoritative` returns the page block.
struct ChildlessPageReader {
    page: Block,
}

#[async_trait]
impl BlockReader for ChildlessPageReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(vec![])
    }
    /// Empty is the TRUTH for this double, not a stub: it models a page with no
    /// children, which the gate must treat as complete and render.
    async fn doc_block_topology(
        &self,
        _: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(vec![])
    }
    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok((self.page.id == *id).then(|| self.page.clone()))
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(vec![(self.page.id.clone(), vec![])])
    }
}

/// No children for anyone — the childless-page ordering.
struct NoChildrenOrdering;

#[async_trait]
impl BlockOrdering for NoChildrenOrdering {
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

#[tokio::test]
async fn childless_runtime_page_materializes_its_identity_file() {
    let page_id = EntityUri::block("jp");
    let mut page = Block::new_text(page_id.clone(), EntityUri::no_parent(), "2026-07-19");
    page.set_page(true);

    let reader = Arc::new(ChildlessPageReader { page: page.clone() });
    // The doc-manager deliberately does NOT know the page (models the lagging
    // page registry): the fix must resolve path + header from the block store.
    let doc_manager = Arc::new(StubDocManager {
        doc: Block::new_text(
            EntityUri::block("unrelated"),
            EntityUri::no_parent(),
            "unrelated",
        ),
    });
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let fs = Arc::new(RealFileSystem);
    let mut controller = new_org_sync_controller(
        reader,
        doc_manager,
        root.clone(),
        Arc::new(NoChildrenOrdering),
        fs,
    );

    // The reactive per-block writeback fires with the page as the upserted block.
    controller
        .seed_holder_from_authority(&page_id)
        .await
        .expect("seeding the holder must not error");
    controller
        .on_block_changed(
            &page_id,
            // A childless page is the only member of its own document, so it
            // has no previous sibling.
            &BlockDelta::Upsert {
                block: page.clone(),
                prev: None,
            },
        )
        .await
        .expect("on_block_changed must not error");

    // The page owns its own file at its authoritative name-chain path, carrying
    // the `#+ID:` identity header even though it is childless.
    let path = holon_core::CanonicalPath::new(&root)
        .into_path_buf()
        .join("2026-07-19.org");
    let disk = std::fs::read_to_string(&path)
        .expect("childless runtime-created page must materialize its own identity file");
    assert!(
        disk.contains("#+ID: jp"),
        "materialized file must carry the page identity header, got:\n{disk}"
    );
}

// ── Copy-on-write default-layout seeding (F4 stale-seed remedy) ──────────────
// The bundled default layout (`block:__default__`,
// `holon_api::is_seed_layout_doc`) is a VIRTUAL seed doc: boot seeds/re-seeds
// it into the store from the shipped asset, but those writes must NOT
// auto-materialize a vault `.org` file. That auto-materialization crept in via
// the Fork B B2 fileless-page sweep (a92d7eb7, 2026-07-12) and pinned Martin's
// vault to a stale Jul-12 seed (the F4 backlinks-invisible saga). The
// controller now keeps the seed doc VIRTUAL through boot
// (`gate_virtual_seed_write` records the pristine asset render and
// skips the write) and only the FIRST user-originated edit — a render that
// DIVERGES from the recorded pristine baseline — materializes the file
// (copy-on-write). Once on disk that file is the durable "user owns the layout"
// marker; the seed BOUNDARY reads its presence (`default_org_exists`) and
// suppresses re-seeding, so a subsequent shipped-asset change never overrides
// it (proven at the seed layer by
// `holon-app::seed_copy_on_write::present_default_file_suppresses_default_layout_seed`).

/// The default-layout doc-root (`block:__default__`) as a `Page` with two leaf
/// children — the seed-doc shape. Distinct from `make_doc_and_blocks` only in
/// its id: this is the doc `is_seed_layout_doc` gates.
fn make_seed_doc_and_blocks() -> (Block, Vec<Block>) {
    let doc_id = holon_api::default_doc_block_uri();
    let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "__default__");
    doc.set_page(true);
    let b1 = Block::new_text(EntityUri::block("layout-a"), doc_id.clone(), "Layout A");
    let b2 = Block::new_text(EntityUri::block("layout-b"), doc_id.clone(), "Layout B");
    (doc, vec![b1, b2])
}

/// A doc-manager that resolves `block:__default__` to the vault-marker path
/// `__default__.org` (single-segment name chain), so the gate's canonical key
/// and the on-disk filename match the real seed-doc marker the seed boundary
/// checks. Every other id fails loud (never exercised here).
struct SeedDocManager {
    doc: Block,
}

#[async_trait]
impl DocumentManager for SeedDocManager {
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
    async fn name_chain(&self, id: &EntityUri) -> anyhow::Result<Vec<String>> {
        if *id == self.doc.id {
            Ok(vec!["__default__".to_string()])
        } else {
            anyhow::bail!("name_chain({id}): unexpected id in seed-doc harness")
        }
    }
}

fn build_seed_harness() -> Harness {
    let (doc, blocks) = make_seed_doc_and_blocks();
    let reader = Arc::new(CountingBlockReader::new(doc.id.clone(), blocks));
    let doc_manager = Arc::new(SeedDocManager { doc: doc.clone() });
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let fs = Arc::new(RealFileSystem);
    let controller = new_org_sync_controller(
        reader.clone(),
        doc_manager,
        root.clone(),
        Arc::new(LiveOrderOrdering {
            reader: reader.clone(),
        }),
        fs,
    );
    let path = holon_core::CanonicalPath::new(&root)
        .into_path_buf()
        .join("__default__.org");
    Harness {
        controller,
        reader,
        doc,
        path,
        _tmp: tmp,
    }
}

/// **PAYOFF #2 (copy-on-write materialization moment).** The VIRTUAL default
/// layout (`block:__default__`) stays off disk through the boot seed/re-seed
/// phase, and the FIRST user-originated edit — a render that diverges from the
/// pristine shipped-asset baseline — MATERIALIZES `__default__.org`, after
/// which the on-disk file exists as the durable "user owns the layout" marker.
///
/// Mirrors payoff #1 (`content_only_edit_...`, this harness): drive the real
/// `on_block_changed` write-back over `RealFileSystem`, assert on disk truth.
/// The complementary half — a subsequent shipped-asset/seed change does NOT
/// override the materialized file (file-wins) — is enforced at the SEED
/// boundary and proven by
/// `holon-app::seed_copy_on_write::present_default_file_suppresses_default_layout_seed`
/// (a present `__default__.org` → `default_org_exists` → no re-seed).
#[tokio::test]
async fn user_edit_materializes_virtual_seed_doc_copy_on_write() {
    assert!(
        holon_api::is_seed_layout_doc(&holon_api::default_doc_block_uri()),
        "test premise: block:__default__ must be a copy-on-write seed doc"
    );

    let mut h = build_seed_harness();

    // 1) BOOT seed write (`boot_seeding` = true, the ctor default): seeding the
    //    default layout into the store must NOT materialize the vault file. The
    //    gate records the pristine asset render and skips the write.
    let b1_pristine = h.reader.blocks.lock().unwrap()[0].clone();
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b1_pristine);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();
    assert!(
        !h.path.exists(),
        "boot seed of the VIRTUAL default layout must NOT auto-materialize \
         __default__.org (F4 stale-seed pin); found {}",
        h.path.display()
    );

    // 2) Boot phase is over.
    h.controller.finish_boot_seeding();

    // 3) RACE-FREE: a late boot-seed delta arriving after `boot_seeding` flipped
    //    still renders == pristine, so it is NOT mistaken for a user edit — the doc
    //    stays virtual (no file).
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b1_pristine);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();
    assert!(
        !h.path.exists(),
        "a post-boot re-seed whose render still equals the pristine asset must \
         stay VIRTUAL (content baseline, not a time flag); found {}",
        h.path.display()
    );

    // 4) USER EDIT: a real edit routed to the seed doc diverges from the pristine
    //    baseline → copy-on-write materializes the file with the user's content.
    let mut b2_edited = h.reader.blocks.lock().unwrap()[1].clone();
    b2_edited.content = "Layout B EDITED BY USER".to_string();
    h.reader.set_block(b2_edited.clone());
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b2_edited);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();

    let materialized = std::fs::read_to_string(&h.path).expect(
        "the first user edit to the seed doc must MATERIALIZE __default__.org (copy-on-write)",
    );
    assert!(
        materialized.contains("Layout B EDITED BY USER"),
        "the materialized file must carry the user's edit; got:\n{materialized}"
    );

    // 5) FILE WINS (at this layer): the file now exists and is a normal tracked doc
    //    — a subsequent identical edit keeps it on disk (no re-virtualizing, no
    //    clobber). Its presence is the `default_org_exists` marker the seed
    //    boundary reads to suppress re-seeding.
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .unwrap();
    let delta = upsert(&h, &b2_edited);
    h.controller
        .on_block_changed(&h.doc.id, &delta)
        .await
        .unwrap();
    assert!(
        h.path.exists(),
        "once materialized, the seed doc's file must persist (it now wins); \
         missing {}",
        h.path.display()
    );
    let after = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after.contains("Layout B EDITED BY USER"),
        "the materialized file must keep the user's content; got:\n{after}"
    );
}
