//! Sibling order is read once per distinct parent per burst.
//!
//! `home_by` asks the authority for `locate(k)` and then `prev_sibling(k)` for
//! every key in a burst. When `prev_sibling` delegates to
//! `BlockOrdering::prev_sibling`, that implementation re-reads the block row
//! (which the burst memo is already holding) and the whole ordered sibling
//! group — statements issued BELOW the memo layer, where same-key memoization
//! structurally cannot reach them. A SplitBlock puts two siblings under one
//! parent into one burst, so the sibling-group statement runs once per key
//! instead of once per parent.
//!
//! The acceptance here is structural, not a percentage: within one burst no
//! sibling-order read and no row read may be issued twice for the same
//! binding. The value assertions beside them keep the cheapness from being
//! bought with a wrong answer.

#![cfg(feature = "di")]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::live_data::home_by::HomeAuthority;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::MemoSeam;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::HomeBurstMemo;

/// Every statement the authority causes, at the granularity the SQL layer
/// issues them: `row:<id>` is a block-by-id point read, `order:<parent>` is the
/// ordered sibling-group read. Both the reader seam and the ordering seam log
/// into this one list, because in production both land on the same two
/// statements (`cache.get_by_id` and `sibling_keys`).
#[derive(Clone, Default)]
struct ReadLog(Arc<Mutex<Vec<String>>>);

impl ReadLog {
    fn record(&self, what: String) {
        self.0.lock().unwrap().push(what);
    }

    fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    /// The binding issued more times than any other, with its count.
    fn worst_repeat(&self, prefix: &str) -> (String, usize) {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for e in self.entries().into_iter().filter(|e| e.starts_with(prefix)) {
            *counts.entry(e).or_default() += 1;
        }
        counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .unwrap_or_else(|| ("<none>".into(), 0))
    }
}

/// A two-sibling document: `page` is a Page at the root, `a` and `b` are its
/// children in that order. This is the SplitBlock shape — one Update and one
/// Insert under the same parent, in one burst.
struct Vault {
    blocks: BTreeMap<EntityUri, Block>,
    children: Mutex<BTreeMap<EntityUri, Vec<EntityUri>>>,
}

impl Vault {
    fn split_shape() -> Self {
        let sentinel = EntityUri::no_parent();
        let page = EntityUri::block("page");
        let a = EntityUri::block("a");
        let b = EntityUri::block("b");

        let mut blocks = BTreeMap::new();
        // The FK anchor, self-parented exactly as `block_raw` seeds it.
        blocks.insert(
            sentinel.clone(),
            Block::new_text(sentinel.clone(), sentinel.clone(), ""),
        );
        let mut page_row = Block::new_text(page.clone(), sentinel.clone(), "page");
        page_row.set_page(true);
        blocks.insert(page.clone(), page_row);
        blocks.insert(a.clone(), Block::new_text(a.clone(), page.clone(), "a"));
        blocks.insert(b.clone(), Block::new_text(b.clone(), page.clone(), "b"));

        let mut children = BTreeMap::new();
        children.insert(page.clone(), vec![a, b]);
        children.insert(sentinel, vec![page]);

        Self {
            blocks,
            children: Mutex::new(children),
        }
    }

    /// A write landing inside a burst the memo assumes is read-only. Only the
    /// dual-read seam is allowed to see it; production never does.
    fn reorder(&self, parent: &EntityUri, order: Vec<EntityUri>) {
        self.children.lock().unwrap().insert(parent.clone(), order);
    }
}

struct RecordingReader {
    vault: Arc<Vault>,
    log: ReadLog,
}

#[async_trait]
impl BlockReader for RecordingReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        unimplemented!("not exercised by the burst")
    }

    async fn doc_block_topology(
        &self,
        _: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        unimplemented!("the Law-5 oracle is never served from the memoized side")
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        self.log.record(format!("row:{id}"));
        Ok(self.vault.blocks.get(id).cloned())
    }

    async fn resolve_link_marks(&self, _: &mut [Block]) -> anyhow::Result<()> {
        Ok(())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        unimplemented!("not exercised by the burst")
    }
}

/// Mirrors `SqlBlockOperations` statement-for-statement: `prev_sibling` costs a
/// block-by-id read plus the ordered group, and `children` costs the same
/// ordered group — one statement, one binding, whichever entry point asks.
struct RecordingOrdering {
    vault: Arc<Vault>,
    log: ReadLog,
}

impl RecordingOrdering {
    fn sibling_keys(&self, parent: &EntityUri) -> Vec<EntityUri> {
        self.log.record(format!("order:{parent}"));
        self.vault
            .children
            .lock()
            .unwrap()
            .get(parent)
            .cloned()
            .unwrap_or_default()
    }

    fn row(&self, id: &EntityUri) -> OrderingResult<Block> {
        self.log.record(format!("row:{id}"));
        self.vault.blocks.get(id).cloned().ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                format!("prev_sibling: block {id} missing").into()
            },
        )
    }
}

#[async_trait]
impl BlockOrdering for RecordingOrdering {
    async fn place(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> OrderingResult<()> {
        unimplemented!("the fold writes nothing")
    }

    async fn prev_sibling(&self, id: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        let block = self.row(id)?;
        if !block.parent_id.is_block() {
            return Ok(None);
        }
        let siblings = self.sibling_keys(&block.parent_id);
        let pos = siblings.iter().position(|s| s == id);
        Ok(pos
            .and_then(|i| i.checked_sub(1))
            .map(|i| siblings[i].clone()))
    }

    async fn next_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        unimplemented!("not exercised")
    }

    async fn first_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        unimplemented!("not exercised")
    }

    async fn last_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        unimplemented!("not exercised")
    }

    async fn children(&self, parent_id: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(self.sibling_keys(parent_id))
    }

    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        unimplemented!("the fold writes nothing")
    }

    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        unimplemented!("the fold writes nothing")
    }
}

fn arrange() -> (BlockHomeAuthority, ReadLog, Arc<Vault>) {
    let vault = Arc::new(Vault::split_shape());
    let log = ReadLog::default();
    let reader = Arc::new(RecordingReader {
        vault: vault.clone(),
        log: log.clone(),
    });
    let ordering = Arc::new(RecordingOrdering {
        vault: vault.clone(),
        log: log.clone(),
    });
    (BlockHomeAuthority::new(reader, ordering), log, vault)
}

/// The burst `home_by` runs for a SplitBlock: locate+prev_sibling for each of
/// the two siblings, then the group read the combinator does when it reassigns
/// the parent's holder.
async fn drive_split_burst(auth: &BlockHomeAuthority, memo: &mut HomeBurstMemo) {
    auth.locate("block:a", memo).await.unwrap();
    auth.prev_sibling("block:a", memo).await.unwrap();
    auth.locate("block:b", memo).await.unwrap();
    auth.prev_sibling("block:b", memo).await.unwrap();
    auth.children_of(Some("block:page"), memo).await.unwrap();
}

/// THE DEFECT. `prev_sibling` delegating below the memo issues the parent's
/// ordered-group read once per key instead of once per parent.
#[tokio::test]
async fn one_burst_reads_a_parents_sibling_order_at_most_once() {
    let (auth, log, _vault) = arrange();

    drive_split_burst(&auth, &mut HomeBurstMemo::default()).await;

    let (binding, count) = log.worst_repeat("order:");
    assert_eq!(
        count,
        1,
        "the ordered sibling-group read `{binding}` was issued {count}x in ONE burst — the \
         statement is issued below the burst memo, so same-key memoization cannot dedup it and \
         every sibling in the burst pays for the same group again. Full read log: {:?}",
        log.entries()
    );
}

/// The same statement class on the row side: the row `locate` already put in
/// the memo must not be read a second time by the ordering layer.
#[tokio::test]
async fn one_burst_reads_a_block_row_at_most_once() {
    let (auth, log, _vault) = arrange();

    drive_split_burst(&auth, &mut HomeBurstMemo::default()).await;

    let (binding, count) = log.worst_repeat("row:");
    assert_eq!(
        count,
        1,
        "the block-by-id read `{binding}` was issued {count}x in ONE burst — `locate` already \
         holds that row in the memo, and the ordering layer re-reads it underneath. Full read \
         log: {:?}",
        log.entries()
    );
}

/// Convergence: the reads may shrink, the answers may not move.
#[tokio::test]
async fn the_derived_prev_sibling_answers_exactly_as_the_ordering_seam_does() {
    let (auth, _log, _vault) = arrange();
    let mut memo = HomeBurstMemo::default();

    assert_eq!(
        auth.prev_sibling("block:a", &mut memo).await.unwrap(),
        None,
        "the first child of a parent has no predecessor"
    );
    assert_eq!(
        auth.prev_sibling("block:b", &mut memo).await.unwrap(),
        Some("block:a".to_string()),
        "the second child's predecessor is the first"
    );
}

/// The root sentinel is not a parent: a block directly under it has no sibling
/// order to read at all. This is the one semantic the delegated implementation
/// carried that a derivation could silently drop.
#[tokio::test]
async fn a_block_under_the_root_sentinel_has_no_predecessor_and_reads_no_order() {
    let (auth, log, _vault) = arrange();

    assert_eq!(
        auth.prev_sibling("block:page", &mut HomeBurstMemo::default())
            .await
            .unwrap(),
        None,
        "a root-level block reports no predecessor — the sentinel is not a parent"
    );
    assert!(
        !log.entries().iter().any(|e| e.starts_with("order:")),
        "no sibling order exists to read for a root-level block, got {:?}",
        log.entries()
    );
}

/// Fail loud: a key the authority no longer holds is an error naming the key,
/// never a silent `None` that would render the block into the wrong place.
#[tokio::test]
async fn a_missing_block_fails_loudly_and_names_itself() {
    let (auth, _log, _vault) = arrange();

    let err = auth
        .prev_sibling("block:ghost", &mut HomeBurstMemo::default())
        .await
        .expect_err("a block the authority does not hold has no defined predecessor");
    assert!(
        format!("{err:#}").contains("block:ghost"),
        "the error must name the key it could not resolve, got: {err:#}"
    );
}

/// H6, on the entry the burst now serves without asking the ordering seam. The
/// memo's soundness rests on "no write lands inside a burst"; reorder the
/// siblings mid-burst and the dual-read beside the next hit must name it rather
/// than let the fold render from it.
#[tokio::test]
async fn a_sibling_order_that_moves_mid_burst_is_disclosed_not_served() {
    let (auth, _log, vault) = arrange();
    let mut memo = HomeBurstMemo::with_seam(MemoSeam::auditing());

    assert_eq!(
        auth.prev_sibling("block:b", &mut memo).await.unwrap(),
        Some("block:a".to_string())
    );

    vault.reorder(
        &EntityUri::block("page"),
        vec![EntityUri::block("b"), EntityUri::block("a")],
    );

    let err = auth
        .prev_sibling("block:b", &mut memo)
        .await
        .expect_err("a reorder inside the burst must surface, not be served from the memo");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("stale") && msg.contains("block:b"),
        "the disclosure must name the stale key and say what happened, got: {msg}"
    );
}
