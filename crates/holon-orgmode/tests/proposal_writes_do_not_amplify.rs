//! #102 — one untrusted proposal write must not re-render the whole vault.
//!
//! The trust gate coerces a sub-threshold operation into a *proposal block*
//! under `block:proposals` (`holon_api::proposal`). That block is an ordinary
//! block, so it enters the write-back layer through the same feed as any edit.
//!
//! The routing decision that matters is the one `di.rs` makes per feed item:
//! a block homes to a document via `nearest_page_ancestor`, and a block with
//! NO `Page` ancestor yields `DocHome::Unresolved`, which routes to
//! `OrgRerender::All` — the debounced `re_render_all_tracked` over every
//! tracked file. The proposal place root is created parentless and is not a
//! page, so every proposal block takes exactly that arm.
//!
//! These tests transcribe that routing literally and assert the vault stays
//! untouched: no tracked file written, no tracked document re-rendered.

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockDelta;
use holon_filesystem::BlockReader;
use holon_filesystem::BlockRowMemo;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSystem;
use holon_filesystem::InMemoryFileSystem;
use holon_filesystem::nearest_page_ancestor;
use holon_orgmode::di::BlockRoute;
use holon_orgmode::di::OrgRerender;
use holon_orgmode::di::route_homed_block;
use holon_orgmode::di::route_upsert;
use holon_orgmode::file_sync_controller::new_org_sync_controller;
use holon_orgmode::home_authority::DocHome;

const ROOT: &str = "/holon-virtual/proposal-amp";

/// The three tracked vault files, as `(relative path, page title)`.
const TRACKED: [(&str, &str); 3] = [
    ("alpha.org", "alpha"),
    ("beta.org", "beta"),
    ("gamma.org", "gamma"),
];

// ── Authority double. ──────────────────────────────────────────────────────

/// Authoritative block store. `get_blocks` is the per-document render read:
/// counting it measures how many documents a pass re-rendered, which is the
/// amplification itself and stays observable even when a re-render happens to
/// be byte-stable and writes nothing.
struct StoreReader {
    blocks: Mutex<Vec<Block>>,
    get_blocks_calls: AtomicUsize,
}

impl StoreReader {
    fn new(blocks: Vec<Block>) -> Self {
        Self {
            blocks: Mutex::new(blocks),
            get_blocks_calls: AtomicUsize::new(0),
        }
    }

    fn get_blocks_calls(&self) -> usize {
        self.get_blocks_calls.load(Ordering::SeqCst)
    }

    /// Add a block the authority did not hold at boot — what the trust gate
    /// does when it mints a proposal mid-session.
    fn insert(&self, block: Block) {
        self.blocks.lock().unwrap().push(block);
    }
}

#[async_trait]
impl BlockReader for StoreReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        self.get_blocks_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.parent_id == *doc_id)
            .cloned()
            .collect())
    }

    /// Delegates to the same store: an empty shape would let the write-back
    /// fold-completeness gate pass on an incomplete document.
    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.parent_id == *doc_id)
            .map(|b| (b.id.clone(), b.parent_id.clone()))
            .collect())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.id == *id)
            .cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

/// Page store pre-populated with the three tracked documents, so
/// `find_by_name_chain` and `get_by_id` resolve each file to its page exactly
/// as production does.
#[derive(Clone, Default)]
struct StoreDocManager {
    by_id: Arc<Mutex<HashMap<EntityUri, Block>>>,
}

impl StoreDocManager {
    fn with_pages(pages: &[Block]) -> Self {
        let by_id = pages.iter().map(|p| (p.id.clone(), p.clone())).collect();
        Self {
            by_id: Arc::new(Mutex::new(by_id)),
        }
    }
}

#[async_trait]
impl DocumentManager for StoreDocManager {
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

    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }

    async fn name_chain(&self, id: &EntityUri) -> anyhow::Result<Vec<String>> {
        let by_id = self.by_id.lock().unwrap();
        let doc = by_id
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("name_chain({id}): no such page"))?;
        Ok(vec![doc.title().to_string()])
    }
}

/// Sibling order straight off the authority, mirroring production where
/// `BlockOrdering::children` and `BlockReader::get_blocks` read one store.
struct StoreOrdering {
    reader: Arc<StoreReader>,
}

#[async_trait]
impl BlockOrdering for StoreOrdering {
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

// ── Fixture. ───────────────────────────────────────────────────────────────

/// The proposal block the trust gate emits, shaped exactly as
/// `OperationEngine::coerce_to_proposal` shapes it: parented at the
/// (parentless, non-page) proposal place root and carrying `_proposal`.
fn proposal_fixture() -> (Block, Block) {
    let root = Block::new_text(
        EntityUri::block(holon_api::PROPOSALS_ROOT_ID),
        EntityUri::no_parent(),
        "Proposals",
    );
    let mut proposal = Block::new_text(
        EntityUri::block("prop-1"),
        root.id.clone(),
        "Proposal: create on block (by agent)",
    );
    let record = holon_api::proposal::ProposalRecord::pending(
        holon_api::EntityName::new("block"),
        "create",
        HashMap::new(),
    );
    proposal
        .properties
        .insert(holon_api::PROPOSAL_PROPERTY.into(), record.to_value());
    (root, proposal)
}

/// A proposal parented at a real PAGE instead of the proposal place. Only the
/// `_proposal` property can classify it, so it is the fixture that keeps the
/// property predicate load-bearing.
fn stray_proposal_fixture() -> Block {
    let mut stray = Block::new_text(
        EntityUri::block("stray-proposal"),
        EntityUri::block("alpha"),
        "Proposal: create on block (by agent)",
    );
    let record = holon_api::proposal::ProposalRecord::pending(
        holon_api::EntityName::new("block"),
        "create",
        HashMap::new(),
    );
    stray
        .properties
        .insert(holon_api::PROPOSAL_PROPERTY.into(), record.to_value());
    stray
}

struct Harness {
    controller: holon_filesystem::FileSyncController,
    reader: Arc<StoreReader>,
    fs: Arc<InMemoryFileSystem>,
}

/// A vault of three tracked org files, each ingested through `on_file_changed`
/// so `last_projection` holds it — the exact set `re_render_all_tracked` walks.
async fn build_harness() -> Harness {
    let (proposals_root, proposal) = proposal_fixture();

    let mut store: Vec<Block> = vec![proposals_root, proposal];
    let mut pages: Vec<Block> = Vec::new();
    for (i, (_, title)) in TRACKED.iter().enumerate() {
        let page_id = EntityUri::block(title);
        let mut page = Block::new_text(page_id.clone(), EntityUri::no_parent(), *title);
        page.set_page(true);
        store.push(Block::new_text(
            EntityUri::block(&format!("child-{i}")),
            page_id,
            format!("body of {title}"),
        ));
        pages.push(page);
    }
    store.extend(pages.iter().cloned());

    let reader = Arc::new(StoreReader::new(store));
    let fs = Arc::new(InMemoryFileSystem::new());
    fs.mkdir_all(std::path::Path::new(ROOT));
    // Ids are authored into the bytes so ingest projects onto the SAME blocks
    // the store holds; minted ids would make write-back refuse the file as an
    // unresolvable ingest drop before any test reached its assertion.
    for (i, (rel, title)) in TRACKED.iter().enumerate() {
        let path = std::path::Path::new(ROOT).join(rel);
        let bytes = format!(
            "#+TITLE: {title}\n#+ID: {title}\n\n* body of {title}\n\
             :PROPERTIES:\n:ID: child-{i}\n:END:\n"
        );
        fs.write(&path, bytes.as_bytes()).await.unwrap();
    }

    let mut controller = new_org_sync_controller(
        reader.clone(),
        Arc::new(StoreDocManager::with_pages(&pages)),
        std::path::PathBuf::from(ROOT),
        Arc::new(StoreOrdering {
            reader: reader.clone(),
        }),
        fs.clone(),
    );

    for (rel, _) in TRACKED {
        let path = std::path::Path::new(ROOT).join(rel);
        controller.on_file_changed(&path).await.unwrap();
    }

    // Seed the write-back holder the way boot does. Without it the holder's
    // membership disagrees with the authority and the fold-completeness gate
    // REFUSES every write — which would mask a wrongly-routed proposal behind
    // an unrelated guard instead of letting it reach a file.
    for (_, title) in TRACKED {
        controller
            .seed_holder_from_authority(&EntityUri::block(title))
            .await
            .unwrap();
    }

    Harness {
        controller,
        reader,
        fs,
    }
}

// ── Tests. ─────────────────────────────────────────────────────────────────

/// THE ROUTING FACT the amplification rests on: a proposal block has no `Page`
/// ancestor, so `home_by` cannot home it to a document. In `di.rs` that is
/// `DocHome::Unresolved`, which sends `OrgRerender::All`.
#[tokio::test]
async fn a_proposal_block_homes_to_no_document() {
    let h = build_harness().await;
    let (_, proposal) = proposal_fixture();

    let home = nearest_page_ancestor(
        h.reader.as_ref(),
        &proposal.id,
        &mut BlockRowMemo::new(),
        None,
    )
    .await
    .unwrap();

    assert!(
        home.is_none(),
        "a proposal block resolved to page {home:?}; the amplification path in this \
         test file assumes the proposal place has no Page ancestor"
    );
}

/// Every tracked file's bytes, concatenated. The direct oracle for "a
/// sub-threshold write reached the vault" — independent of whether any
/// particular guard happened to suppress the write.
async fn vault_bytes(fs: &InMemoryFileSystem) -> String {
    let mut all = String::new();
    for (rel, _) in TRACKED {
        let path = std::path::Path::new(ROOT).join(rel);
        all.push_str(&fs.read_to_string(&path).await.unwrap());
    }
    all
}

/// The document a block homes to, exactly as `BlockHomeAuthority::walk_doc`
/// derives it.
async fn home_of(reader: &StoreReader, id: &EntityUri) -> DocHome {
    match nearest_page_ancestor(reader, id, &mut BlockRowMemo::new(), None)
        .await
        .unwrap()
    {
        Some(page) => DocHome::Resolved(page.id),
        None => DocHome::Unresolved,
    }
}

/// Act on one feed message exactly as the `di.rs` loop does: `Block` renders
/// its document, `Seed` folds without rendering, `All` arms the debounced
/// vault-wide pass, `Reset` drops the derived state.
async fn dispatch(controller: &mut holon_filesystem::FileSyncController, msg: Option<OrgRerender>) {
    match msg {
        None => {}
        Some(OrgRerender::Block { doc, delta }) => {
            let verdicts = controller
                .on_block_changed_coalesced(&[(doc, *delta)])
                .await;
            // A document that resolved to no tracked file escalates to the
            // bulk pass. Dropping this verdict would hide an amplification.
            if verdicts.iter().any(|(_, v)| matches!(v, Ok(false))) {
                controller
                    .re_render_all_tracked(&HashSet::new())
                    .await
                    .unwrap();
            }
        }
        Some(OrgRerender::Seed { doc, delta }) => {
            controller.apply_block_delta(&doc, &delta);
        }
        Some(OrgRerender::All) => {
            controller
                .re_render_all_tracked(&HashSet::new())
                .await
                .unwrap();
        }
        Some(OrgRerender::Reset) => controller.reset_holder(),
    }
}

/// One proposal write must leave the vault alone: no tracked file written and
/// no tracked document re-rendered.
///
/// The feed message is produced by PRODUCTION's `route_upsert` — the loop's
/// own call site for the routing decision — and only its dispatch is mirrored
/// here. Reverting either the routing or that call site therefore reds this
/// test; the residual it cannot see is the `select!` plumbing that hands the
/// message to the controller.
#[tokio::test]
async fn a_proposal_write_renders_no_vault_file() {
    let mut h = build_harness().await;
    let (_, proposal) = proposal_fixture();

    let writes_before = h.fs.write_targets().len();
    let renders_before = h.reader.get_blocks_calls();

    let home = home_of(h.reader.as_ref(), &proposal.id).await;
    let msg = route_upsert(&home, &proposal, None, false);
    dispatch(&mut h.controller, msg).await;

    let writes_after = h.fs.write_targets().len();
    let renders_after = h.reader.get_blocks_calls();

    assert_eq!(
        writes_after - writes_before,
        0,
        "one proposal write wrote {} vault file(s); a proposal is not vault content \
         and must reach no tracked file",
        writes_after - writes_before,
    );
    assert_eq!(
        renders_after - renders_before,
        0,
        "one proposal write re-rendered {} tracked document(s); a proposal write \
         must not amplify into a vault-wide pass",
        renders_after - renders_before,
    );
    assert!(
        !vault_bytes(&h.fs).await.contains("Proposal:"),
        "the proposal reached the vault bytes"
    );
}

/// ANTI-OVERCORRECTION. The proposal drop must not weaken the recovery pass
/// for real vault content: an ordinary block the authority cannot home still
/// escalates to the bulk pass, and an ordinary homed block still renders into
/// its own document.
#[tokio::test]
async fn ordinary_blocks_keep_their_routing() {
    let h = build_harness().await;

    let orphan = Block::new_text(
        EntityUri::block("orphan"),
        EntityUri::block("gone"),
        "no page above me",
    );
    assert_eq!(
        route_homed_block(
            &DocHome::Unresolved,
            &orphan.id,
            &orphan.parent_id,
            &orphan.properties,
        ),
        BlockRoute::Recover,
        "an ordinary un-homed block must still arm the recovery pass — that \
         recovery is designed for vault content with a faulted authority"
    );

    let child = EntityUri::block("child-0");
    let home = home_of(h.reader.as_ref(), &child).await;
    let block = h
        .reader
        .get_block_authoritative(&child)
        .await
        .unwrap()
        .expect("fixture holds child-0");
    assert_eq!(
        route_homed_block(&home, &block.id, &block.parent_id, &block.properties),
        BlockRoute::Document(EntityUri::block("alpha")),
        "an ordinary homed block must still render into its own document"
    );
}

/// The proposal place ROOT carries no `_proposal` property of its own, so the
/// id predicate — not the property one — has to catch it. Without this the
/// very first proposal ever minted would amplify when the root is created.
#[tokio::test]
async fn the_proposals_root_itself_routes_nowhere() {
    let (root, _) = proposal_fixture();
    assert_eq!(
        route_homed_block(
            &DocHome::Unresolved,
            &root.id,
            &root.parent_id,
            &root.properties,
        ),
        BlockRoute::Drop,
        "the proposal place root must route nowhere"
    );
}

/// A proposal whose parent is NOT the proposal place — so ONLY the
/// `_proposal` property can classify it. Its home resolves to a real tracked
/// page, so without the property predicate it routes to `Document` and the
/// sub-threshold write lands in that page's file.
///
/// The property, not the placement, is what makes a block a proposal: the id
/// predicate cannot reach a mis-parented one, and a write is exactly what must
/// not happen.
#[tokio::test]
async fn a_proposal_parented_outside_the_place_still_routes_nowhere() {
    let mut h = build_harness().await;

    // The gate mints it mid-session, AFTER the holder was seeded — so the
    // Upsert below is a real arrival, not a no-op restatement of seeded state.
    let stray = stray_proposal_fixture();
    h.reader.insert(stray.clone());

    let home = home_of(h.reader.as_ref(), &stray.id).await;
    assert_eq!(
        home,
        DocHome::Resolved(EntityUri::block("alpha")),
        "fixture wiring: this block must HOME to a tracked page, else the id \
         predicate could be what saves it"
    );

    let writes_before = h.fs.write_targets().len();

    let msg = route_upsert(&home, &stray, None, false);
    dispatch(&mut h.controller, msg).await;

    assert!(
        !vault_bytes(&h.fs).await.contains("Proposal:"),
        "a mis-parented proposal reached the vault bytes — the `_proposal` \
         property, not the placement, decides what is vault content"
    );
    assert_eq!(
        h.fs.write_targets().len() - writes_before,
        0,
        "a mis-parented proposal wrote {} vault file(s)",
        h.fs.write_targets().len() - writes_before,
    );
}
