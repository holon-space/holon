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

    /// Replace the whole ordered block list — simulates a same-parent
    /// reorder (a new `sort_key` sequence), where `get_blocks`/`children`
    /// would now return a different order for unchanged blocks.
    fn set_blocks(&self, blocks: Vec<Block>) {
        *self.blocks.lock().unwrap() = blocks;
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
/// used by the mass-truncation tripwire test, which needs enough blocks that
/// dropping a large fraction crosses the threshold.
fn make_doc_and_n_blocks(n: usize) -> (Block, Vec<Block>) {
    let doc_id = EntityUri::block("doc-1");
    let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "My Document");
    doc.set_page(true);
    let blocks = (0..n)
        .map(|i| {
            Block::new_text(
                EntityUri::block(&format!("b{i}")),
                doc_id.clone(),
                &format!("heading {i}"),
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

/// Regression for the disk-sibling-order bug: a same-parent reorder (the
/// sort_key sequence changes but `parent_id`/`tags` do not) must NOT take the
/// cheap content-only path — the cache would otherwise keep the pre-reorder
/// `IndexMap` position forever, so the written file's sibling order would
/// diverge from the live (SQL/`sort_key`-authoritative) order even though the
/// delta's own content looks unchanged.
#[tokio::test]
async fn reorder_within_parent_takes_full_reseed() {
    let mut h = build_harness();
    let (b1, b2, b3) = {
        let blocks = h.reader.blocks.lock().unwrap();
        (blocks[0].clone(), blocks[1].clone(), blocks[2].clone())
    };
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1.clone()))
        .await
        .unwrap();
    let baseline = h.reader.get_blocks_calls();

    // Same parent, same tags, same content — only the sibling order changes
    // (b2 now sorts before b1).
    h.reader
        .set_blocks(vec![b2.clone(), b1.clone(), b3.clone()]);

    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1))
        .await
        .unwrap();

    assert_eq!(
        h.reader.get_blocks_calls(),
        baseline + 1,
        "a same-parent reorder must take the full reseed (get_blocks), not the cheap content-only \
         path"
    );

    let written = std::fs::read_to_string(&h.path).unwrap();
    assert_eq!(
        written,
        full_render_oracle(&h),
        "written file must reflect the new (post-reorder) sibling order, not the stale cached one"
    );
    assert!(
        written.find("Second heading").unwrap() < written.find("First heading").unwrap(),
        "b2 must render before b1 after the reorder: {written:?}"
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

/// **Fork B B1' (RED-first): a SINGLE ungrounded drop on the block-driven path
/// passes SILENTLY (below the mass-truncation tripwire).** After the file is on
/// disk with b1/b2/b3, the store loses b2 and a reseed renders b1/b3. On this
/// path a single ungrounded drop is indistinguishable from a routine deletion —
/// the block feed collapses every removal to a full re-render and delivers NO
/// delete op here (see `di.rs`). The mass-truncation tripwire fires only on the
/// row-28 signature (a large fraction of the file), so this 1-of-3 drop
/// PROCEEDS (disk shrinks to b1/b3), the file is NOT quarantined, and no loud
/// error is emitted (else routine deletions would flood
/// `inv-no-observed-errors`). Hard grounding of every drop is deferred to the
/// C2b history relation / removal-op delivery.
#[tokio::test]
async fn block_driven_writeback_small_drop_passes_silently() {
    let mut h = build_harness();
    let all = h.reader.blocks.lock().unwrap().clone();
    let (b1, b3) = (all[0].clone(), all[2].clone());

    // Seed disk with the full doc (b1/b2/b3).
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1.clone()))
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
    let mut tags = Tags::default();
    tags.insert("Page");
    b1_tagged.tags = tags;
    h.reader.set_block(b1_tagged.clone());

    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1_tagged))
        .await
        .expect("a single ungrounded drop is below the tripwire — proceeds, never Err");

    // The write proceeded — disk converged to b1/b3 (a routine deletion must reach
    // disk; the small-drop case passes the tripwire silently).
    let after = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        !after.contains("Second heading"),
        "the small drop must be written (deletion converges), not vetoed; got {after:?}"
    );

    // NOT quarantined: a later edit still writes normally.
    let mut b1_v2 = b1.clone();
    b1_v2.content = "First heading edited".to_string();
    h.reader.set_blocks(vec![b1_v2.clone(), b3.clone()]);
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1_v2))
        .await
        .expect("a non-quarantined file keeps accepting writes");
    let after_edit = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after_edit.contains("First heading edited"),
        "the file is not quarantined — later edits still land; got {after_edit:?}"
    );
}

/// **Fork B B1' (RED-first): a MASS TRUNCATION (row-28 signature) VETOES +
/// quarantines.** A 20-block doc is on disk; the store then loses 15 of them
/// (an FK-rollback-style truncation — NOT a user delete: the delta is an
/// `Upsert` and none has a sibling file). A reseed renders only 5 blocks;
/// writing it would DELETE 15 lines from disk. 15 dropped > `max(3, 20/4=5)=5`
/// → the mass-truncation tripwire fires: the write is REFUSED (Err), disk is
/// left intact, and the file is quarantined so a later edit is skipped too.
/// This is the row-28 protection on the block-driven path.
#[tokio::test]
async fn block_driven_writeback_vetoes_mass_truncation() {
    let mut h = build_harness_with_blocks(20);
    let all = h.reader.blocks.lock().unwrap().clone();
    let b0 = all[0].clone();

    // Seed disk with all 20 blocks.
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b0.clone()))
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
    let mut tags = Tags::default();
    tags.insert("Page");
    b0_tagged.tags = tags;
    h.reader.set_block(b0_tagged.clone());

    let veto = h
        .controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b0_tagged))
        .await;
    assert!(
        veto.is_err(),
        "a 15-of-20 mass truncation must VETO the write-back (Err), not write 5 blocks"
    );
    let msg = format!("{:#}", veto.unwrap_err());
    assert!(
        msg.contains("MASS TRUNCATION"),
        "veto error must name the mass-truncation tripwire; got {msg}"
    );

    // Disk is intact — the truncated projection was refused.
    let after_veto = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        after_veto.contains("heading 19"),
        "the vetoed write must leave all 20 blocks on disk; got {after_veto:?}"
    );

    // Quarantined: a subsequent benign edit is skipped (no write over disk).
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b0))
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
/// path — the production feed reaches it through `on_block_removed` (ADR 0025
/// root item).
#[tokio::test]
async fn block_driven_writeback_writes_sanctioned_deletion() {
    let mut h = build_harness();
    let all = h.reader.blocks.lock().unwrap().clone();
    let (b1, b3) = (all[0].clone(), all[2].clone());

    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1.clone()))
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

/// **ADR 0025 ROOT ITEM: a feed removal (`MapDiff::Remove` →
/// `on_block_removed`) is reverse-routed to the one file whose projection shows
/// the block, and the file shrinks with the removal SANCTIONED** — no tripwire,
/// no quarantine, no full-vault re-render. This is the end-to-end shape of the
/// production feed path after di.rs stopped collapsing removals to
/// `OrgRerender::All`.
#[tokio::test]
async fn on_block_removed_routes_to_owning_file() {
    let mut h = build_harness();
    let b1 = h.reader.blocks.lock().unwrap()[0].clone();

    // Warm the projection (cache + last_projection) with the full b1/b2/b3 doc.
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1))
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&h.path)
            .unwrap()
            .contains("Second heading")
    );

    // The user deletes b2: the authoritative store loses it, THEN the feed
    // delivers the bare Remove (the block is already gone from the feed map).
    let remaining: Vec<Block> = h
        .reader
        .blocks
        .lock()
        .unwrap()
        .iter()
        .filter(|b| b.id != EntityUri::block("b2"))
        .cloned()
        .collect();
    h.reader.set_blocks(remaining);

    let routed = h
        .controller
        .on_block_removed(&EntityUri::block("b2"))
        .await
        .expect("routing a sanctioned removal must never error");
    assert!(routed, "the removal must be routed to the owning file");

    let written = std::fs::read_to_string(&h.path).unwrap();
    assert!(
        !written.contains("Second heading"),
        "the sanctioned removal of b2 must shrink the file; got {written:?}"
    );
    assert_eq!(
        written,
        full_render_oracle(&h),
        "the shrunken file must match the authoritative full render"
    );
}

/// **ADR 0025: a `Remove` for a block STILL PRESENT in the authoritative store
/// is matview churn/lag, not a deletion** — `on_block_removed` treats it as
/// moot (routed, nothing written) instead of re-rendering or sanctioning.
#[tokio::test]
async fn on_block_removed_of_live_block_is_moot() {
    let mut h = build_harness();
    let b1 = h.reader.blocks.lock().unwrap()[0].clone();
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1))
        .await
        .unwrap();
    let before = std::fs::read_to_string(&h.path).unwrap();
    let baseline = h.reader.get_blocks_calls();

    let routed = h
        .controller
        .on_block_removed(&EntityUri::block("b2"))
        .await
        .unwrap();
    assert!(routed, "a live block's Remove is moot but handled");
    assert_eq!(
        std::fs::read_to_string(&h.path).unwrap(),
        before,
        "a moot Remove must not touch the file"
    );
    assert_eq!(
        h.reader.get_blocks_calls(),
        baseline,
        "a moot Remove must not pay a reseed"
    );
}

/// **ADR 0025: a `Remove` for a block NO tracked projection contains returns
/// `Ok(false)`** — the caller (di.rs) then falls back to the debounced bulk
/// pass CARRYING the id in its sanctioned set, so even the fallback stays
/// op-grounded.
#[tokio::test]
async fn on_block_removed_unknown_block_falls_back() {
    let mut h = build_harness();
    let b1 = h.reader.blocks.lock().unwrap()[0].clone();
    h.controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(b1))
        .await
        .unwrap();

    let routed = h
        .controller
        .on_block_removed(&EntityUri::block("never-projected"))
        .await
        .unwrap();
    assert!(
        !routed,
        "an unroutable removal must report false so the caller sanctions the bulk pass"
    );
}
