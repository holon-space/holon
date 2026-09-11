//! "No routing applies" and "the routing is missing" are different facts.
//!
//! The block feed homes every block it carries, and a block that resolves to no
//! document used to collapse both facts into one `DocHome::Unresolved`. That
//! single value armed `OrgRerender::All` — a re-render of EVERY tracked file —
//! for two populations that could not be less alike:
//!
//! - a block under the root sentinel with no `Page` above it. Nothing owns it,
//!   nothing is broken, and no file on disk contains it. Re-rendering the vault
//!   changes nothing and costs the whole vault.
//! - a block whose parent chain the authority cannot follow — a parent row it
//!   does not hold, a cycle, the depth bound. A document may well own this
//!   block; the walk simply could not say which. That IS the case the bulk
//!   recovery pass exists for.
//!
//! The authority now parses the two apart at the walk, so the routing function
//! matches on the distinction instead of re-deriving it, and neither population
//! can drift into the other's behaviour unnoticed.
//!
//! @pbt kind harness
//! @pbt covers routing-applicability-vs-missing — an untracked block arms no
//! bulk pass; a block whose routing is missing still does

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::live_data::home_by::HomeAuthority;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::PageWalkBreak;
use holon_orgmode::di::BlockRoute;
use holon_orgmode::di::OrgRerender;
use holon_orgmode::di::route_homed_block;
use holon_orgmode::di::route_remove;
use holon_orgmode::di::route_upsert;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::DocHome;
use holon_orgmode::home_authority::HomeBurstMemo;
use holon_orgmode::home_authority::UnresolvedHome;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

// ── Level-tagged tracing capture (dependency-free). ─────────────────────────

#[derive(Clone, Default)]
struct LogCapture(Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>);

impl LogCapture {
    /// Every captured line at `level`, joined — what a reader of the log would
    /// have in front of them.
    fn at(&self, level: tracing::Level) -> String {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(l, _)| *l == level)
            .map(|(_, m)| m.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct MsgVisitor<'a>(&'a mut String);
impl Visit for MsgVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

impl<S: tracing::Subscriber> Layer<S> for LogCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut buf = String::new();
        event.record(&mut MsgVisitor(&mut buf));
        self.0
            .lock()
            .unwrap()
            .push((*event.metadata().level(), buf));
    }
}

/// A store shaped like `block_raw`: it holds the self-parented root sentinel,
/// so a walk that reaches it terminates the way production's does.
#[derive(Default)]
struct Store {
    blocks: BTreeMap<EntityUri, Block>,
}

impl Store {
    fn new() -> Self {
        let sentinel = EntityUri::no_parent();
        let mut blocks = BTreeMap::new();
        blocks.insert(
            sentinel.clone(),
            Block::new_text(sentinel.clone(), sentinel, ""),
        );
        Self { blocks }
    }

    fn add(&mut self, id: &str, parent: EntityUri, is_page: bool) -> Block {
        let uri = EntityUri::block(id);
        let mut block = Block::new_text(uri.clone(), parent, id);
        block.set_page(is_page);
        self.blocks.insert(uri, block.clone());
        block
    }
}

#[async_trait]
impl BlockReader for Store {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(Vec::new())
    }
    async fn doc_block_topology(
        &self,
        _: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(Vec::new())
    }
    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.blocks.get(id).cloned())
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

/// `locate` reads no sibling order, so this exists only to satisfy the
/// authority's constructor.
struct NoOrdering;

#[async_trait]
impl BlockOrdering for NoOrdering {
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

/// The home PRODUCTION derives for `id` — `BlockHomeAuthority::locate`, the
/// call the block feed makes — so a test asserting on the answer is asserting
/// on the real decision, not on a transcription of it.
async fn home_of(store: Store, id: &EntityUri) -> DocHome {
    let authority = BlockHomeAuthority::new(Arc::new(store), Arc::new(NoOrdering));
    authority
        .locate(id.as_str(), &mut HomeBurstMemo::default())
        .await
        .expect("the walk answers rather than failing the stream")
        .expect("the authority holds the block")
        .doc
}

/// The `Unresolvable` a broken chain produces — the population the vault-wide
/// recovery pass exists for.
fn unresolvable() -> DocHome {
    DocHome::Unresolvable(UnresolvedHome::Walk(PageWalkBreak::ChainLeftTheStore))
}

fn is_bulk_pass(msg: &Option<OrgRerender>) -> bool {
    matches!(msg, Some(OrgRerender::All))
}

// ── No routing applies ──────────────────────────────────────────────────────

/// A top-level block with no `Page` above it: the chain reaches the root
/// sentinel cleanly. Nothing is missing, so nothing is to recover.
#[tokio::test]
async fn a_block_under_the_root_sentinel_has_no_applicable_routing() {
    let mut store = Store::new();
    let block = store.add("loose", EntityUri::no_parent(), false);

    assert_eq!(
        home_of(store, &block.id).await,
        DocHome::Untracked,
        "a clean walk to the root sentinel means NO document owns this block — reporting it \
         as an unresolved routing makes an ordinary untracked block indistinguishable from a \
         broken parent chain",
    );
}

/// The whole point of the distinction: an untracked block must not arm the
/// vault-wide recovery pass. This is the 5 Hz ERROR storm in the symptom
/// record — every such event re-rendered every tracked file.
#[tokio::test]
async fn an_untracked_block_arms_no_bulk_pass() {
    let mut store = Store::new();
    let block = store.add("loose", EntityUri::no_parent(), false);

    assert_eq!(
        route_homed_block(
            &DocHome::Untracked,
            &block.id,
            &block.parent_id,
            &block.properties,
        ),
        BlockRoute::Drop,
        "a block no document owns routes nowhere",
    );
    let msg = route_upsert(&DocHome::Untracked, &block, None, false);
    assert!(
        !is_bulk_pass(&msg),
        "an untracked block's write armed a re-render of every tracked file; it appears in \
         none of them",
    );
    assert!(
        route_remove(&DocHome::Untracked, block.id.as_str()).is_none(),
        "a departure FROM an untracked home has no document to re-render either",
    );
}

// ── The routing is missing ──────────────────────────────────────────────────

/// The chain leaves the store: the authority does not hold `orphan`'s parent.
/// A document may own this block — the walk cannot say — so the bulk pass is
/// the designed recovery and must still fire.
#[tokio::test]
async fn a_chain_that_leaves_the_store_is_a_missing_routing() {
    let mut store = Store::new();
    let block = store.add("orphan", EntityUri::block("never-stored"), false);

    assert_eq!(
        home_of(store, &block.id).await,
        DocHome::Unresolvable(UnresolvedHome::Walk(PageWalkBreak::ChainLeftTheStore)),
        "a parent the authority does not hold is a BROKEN chain, not an absent one — \
         collapsing it into `Untracked` would silently stop recovering the case the bulk \
         pass exists for",
    );
}

/// A cyclic parent chain is corrupt parentage, not an absence.
#[tokio::test]
async fn a_parent_cycle_is_a_missing_routing() {
    let mut store = Store::new();
    store.add("a", EntityUri::block("b"), false);
    let b = store.add("b", EntityUri::block("a"), false);

    assert_eq!(
        home_of(store, &b.id).await,
        DocHome::Unresolvable(UnresolvedHome::Walk(PageWalkBreak::ParentCycle)),
        "a cycle leaves the owning document unknown, which is exactly what the recovery \
         pass converges",
    );
}

/// ANTI-OVERCORRECTION: splitting the enum must not weaken the recovery.
#[tokio::test]
async fn a_missing_routing_still_arms_the_bulk_pass() {
    let block = Block::new_text(
        EntityUri::block("orphan"),
        EntityUri::block("never-stored"),
        "a document may own me",
    );

    assert_eq!(
        route_homed_block(
            &unresolvable(),
            &block.id,
            &block.parent_id,
            &block.properties,
        ),
        BlockRoute::Recover,
        "a block whose routing could not be resolved still recovers through the bulk pass",
    );
    assert!(
        is_bulk_pass(&route_upsert(&unresolvable(), &block, None, false)),
        "a write whose routing is missing must still converge the vault",
    );
    assert!(
        is_bulk_pass(&route_remove(&unresolvable(), block.id.as_str())),
        "a departure whose source document is unknown must still converge the vault",
    );
}

// ── The recovery is disclosed WITH its cause ────────────────────────────────

/// A vault-wide re-render costs every tracked file, and the reader of the log
/// has to be able to act on it. "No document could be resolved" names the
/// symptom; only the BREAK says whether to repair a parent row, a cycle, or a
/// chain longer than the walk allows.
#[tokio::test]
async fn the_recovery_warn_names_the_break_that_caused_it() {
    let mut store = Store::new();
    let block = store.add("orphan", EntityUri::block("never-stored"), false);
    let home = home_of(store, &block.id).await;

    let cap = LogCapture::default();
    let route = {
        let _g = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));
        route_homed_block(&home, &block.id, &block.parent_id, &block.properties)
    };
    assert_eq!(route, BlockRoute::Recover);

    let warns = cap.at(tracing::Level::WARN);
    assert!(
        warns.contains(block.id.as_str()),
        "the vault-wide recovery must name the block that armed it; log was: {warns}",
    );
    assert!(
        warns.contains("not in the store"),
        "the recovery WARN dropped the reason the walk broke, so the reader cannot tell a \
         missing parent row from a cycle or a too-long chain — and those need different \
         repairs; log was: {warns}",
    );
}

/// `ChainLeftTheStore` is the likeliest break in production — a parent row the
/// authority does not hold yet — and it was the one break the walk answered
/// silently. A silent break that fires a vault-wide re-render is
/// indistinguishable from working routing.
#[tokio::test]
async fn a_chain_that_leaves_the_store_is_disclosed_at_the_walk() {
    let mut store = Store::new();
    let block = store.add("orphan", EntityUri::block("never-stored"), false);

    let cap = LogCapture::default();
    let home = {
        let _g = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));
        home_of(store, &block.id).await
    };
    assert!(!matches!(home, DocHome::Resolved(_) | DocHome::Untracked));

    let warns = cap.at(tracing::Level::WARN);
    assert!(
        warns.contains("never-stored"),
        "the walk left the store at a row it does not hold and said nothing — the parent cycle \
         and depth-bound breaks beside it both shout; log was: {warns}",
    );
}

// ── The ordinary case is untouched ──────────────────────────────────────────

/// A block inside a page still routes into that page's document, through the
/// real authority walk.
#[tokio::test]
async fn a_homed_block_still_routes_to_its_document() {
    let mut store = Store::new();
    let page = store.add("page", EntityUri::no_parent(), true);
    let leaf = store.add("leaf", page.id.clone(), false);

    let home = home_of(store, &leaf.id).await;
    assert_eq!(home, DocHome::Resolved(page.id.clone()));
    assert_eq!(
        route_homed_block(&home, &leaf.id, &leaf.parent_id, &leaf.properties),
        BlockRoute::Document(page.id),
        "the ordinary routing must be untouched by the split",
    );
}

// ── The batch path names the condition it actually found ────────────────────

/// `locate_batch` covers a block's parent from its own snapshot when the batch
/// carries it. When it does not, it pays one authoritative walk — and that walk
/// starts at the PARENT id, which nobody asked about and which the store may
/// not hold. A block with a dangling parent reaches exactly that branch.
///
/// The answer must name the chain, not the burst: an orphan with a parent the
/// vault lost is an ordinary vault condition the recovery pass exists for.
/// Reporting it as "the authority no longer holds the row the walk started
/// from" blames an internal defect for a real repair the reader has to make —
/// the same misleading fail-loud message entry
/// `2026-09-11-the-boot-scan-announces-a-vault-wide-re-render-it-never-runs`
/// set out to remove.
#[tokio::test]
async fn a_batch_orphan_with_a_dangling_parent_names_the_chain_not_the_burst() {
    let mut store = Store::new();
    let block = store.add("orphan", EntityUri::block("never-stored"), false);
    let authority = BlockHomeAuthority::new(Arc::new(store), Arc::new(NoOrdering));

    let mut memo = HomeBurstMemo::default();
    let placed = authority
        .locate_batch(&[block.id.as_str().to_string()], &mut memo)
        .await
        .expect("the batch answers rather than failing the stream");
    let home = placed
        .get(block.id.as_str())
        .expect("the batch places every id it was handed")
        .doc
        .clone();

    assert_eq!(
        home,
        DocHome::Unresolvable(UnresolvedHome::Walk(PageWalkBreak::ChainLeftTheStore)),
        "the batch walked from the PARENT id — an id it never read — so a missing row there \
         is the chain leaving the store, not a burst dropping a row it was holding",
    );

    // The reader of a vault-wide re-render has to act on it, so check the
    // sentence they get, not only the variant behind it.
    let cap = LogCapture::default();
    let route = {
        let _g = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));
        route_homed_block(&home, &block.id, &block.parent_id, &block.properties)
    };
    let warns = cap.at(tracing::Level::WARN);
    assert!(
        warns.contains("ancestor row"),
        "the disclosure must send the reader to the parent chain; log was: {warns}",
    );
    assert!(
        !warns.contains("walk started from"),
        "the disclosure blamed the burst for a parent the vault lost; log was: {warns}",
    );

    // ANTI-OVERCORRECTION: naming the condition honestly must not change what
    // the block routes to.
    assert_eq!(
        route,
        BlockRoute::Recover,
        "an orphan whose owning document cannot be named still recovers through the bulk pass",
    );
}
