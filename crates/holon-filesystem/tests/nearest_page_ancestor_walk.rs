//! The parent-chain walk terminates at the tree root.
//!
//! Under Turso `sentinel:no_parent` is a real, **self-parented** row in
//! `block_raw` (`holon-turso/src/schema_modules.rs` seeds it as the FK anchor).
//! A walk that reaches it therefore stops advancing — `cur = block.parent_id`
//! hands back the sentinel — and re-reads that one row until its depth bound.
//! Three separate copies of this loop had the defect; these tests pin the one
//! function they now share, the shared function is the intended home for any
//! future walk (nothing prevents a new hand-rolled copy — reviewers should
//! route walks here) rather than a fourth repeat of the same bug.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockReader;
use holon_filesystem::BlockRowMemo;
use holon_filesystem::nearest_page_ancestor;

/// A store shaped like `block_raw`: it holds the self-parented root sentinel,
/// and records every point read.
struct RecordingReader {
    blocks: BTreeMap<EntityUri, Block>,
    reads: Mutex<Vec<String>>,
}

impl RecordingReader {
    /// `chain` is root-first; each entry is `(id, is_page)`.
    fn new(chain: &[(&str, bool)]) -> Self {
        let mut blocks = BTreeMap::new();
        let sentinel = EntityUri::no_parent();
        blocks.insert(
            sentinel.clone(),
            Block::new_text(sentinel.clone(), sentinel.clone(), ""),
        );
        let mut parent = sentinel;
        for (id, is_page) in chain {
            let uri = EntityUri::block(id);
            let mut b = Block::new_text(uri.clone(), parent.clone(), *id);
            b.set_page(*is_page);
            blocks.insert(uri.clone(), b);
            parent = uri;
        }
        Self {
            blocks,
            reads: Mutex::new(Vec::new()),
        }
    }

    /// A store whose two blocks are each other's parent — corrupt parentage.
    fn cyclic() -> Self {
        let mut blocks = BTreeMap::new();
        let (a, b) = (EntityUri::block("a"), EntityUri::block("b"));
        blocks.insert(a.clone(), Block::new_text(a.clone(), b.clone(), "a"));
        blocks.insert(b.clone(), Block::new_text(b.clone(), a.clone(), "b"));
        Self {
            blocks,
            reads: Mutex::new(Vec::new()),
        }
    }

    fn reads(&self) -> Vec<String> {
        self.reads.lock().unwrap().clone()
    }
}

#[async_trait]
impl BlockReader for RecordingReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        unimplemented!("not exercised by the walk")
    }
    async fn doc_block_topology(
        &self,
        _: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        unimplemented!("not exercised by the walk")
    }
    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        self.reads.lock().unwrap().push(id.as_str().to_string());
        Ok(self.blocks.get(id).cloned())
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        unimplemented!("not exercised by the walk")
    }
}

fn no_id_read_twice(reads: &[String]) -> bool {
    let mut sorted = reads.to_vec();
    sorted.sort();
    let before = sorted.len();
    sorted.dedup();
    sorted.len() == before
}

#[tokio::test]
async fn a_chain_reaching_the_root_sentinel_stops_there() {
    let reader = Arc::new(RecordingReader::new(&[("orphan", false)]));

    let found = nearest_page_ancestor(
        reader.as_ref(),
        &EntityUri::block("orphan"),
        &mut BlockRowMemo::new(),
        None,
    )
    .await
    .unwrap();

    assert!(
        found.is_none(),
        "no Page above a root-level block, so there is no owning page"
    );
    assert!(
        no_id_read_twice(&reader.reads()),
        "the self-parented root sentinel must end the walk, not restart it: {:?}",
        reader.reads()
    );
}

#[tokio::test]
async fn a_cycle_answers_none_loudly_instead_of_spinning() {
    let reader = Arc::new(RecordingReader::cyclic());

    let found = nearest_page_ancestor(
        reader.as_ref(),
        &EntityUri::block("a"),
        &mut BlockRowMemo::new(),
        None,
    )
    .await
    .unwrap();

    assert!(
        found.is_none(),
        "a cyclic chain owns no page — and must not be an Err, which `home_by` would treat as \
         stream-fatal and use to kill write-back for the whole vault"
    );
    assert!(
        no_id_read_twice(&reader.reads()),
        "a revisited id must end the walk: {:?}",
        reader.reads()
    );
}

#[tokio::test]
async fn the_nearest_page_wins_and_each_step_is_read_once() {
    let reader = Arc::new(RecordingReader::new(&[
        ("outer", true),
        ("inner", true),
        ("leaf", false),
    ]));

    let found = nearest_page_ancestor(
        reader.as_ref(),
        &EntityUri::block("leaf"),
        &mut BlockRowMemo::new(),
        None,
    )
    .await
    .unwrap()
    .expect("leaf is inside a page");

    assert_eq!(
        found.id,
        EntityUri::block("inner"),
        "the NEAREST page owns the block — an outer page must not capture it"
    );
    assert_eq!(
        reader.reads(),
        vec!["block:leaf".to_string(), "block:inner".to_string()],
        "the walk stops at the first page and reads nothing above it"
    );
}

#[tokio::test]
async fn a_prefetched_first_row_is_not_read_again() {
    let reader = Arc::new(RecordingReader::new(&[("page", true), ("leaf", false)]));
    let leaf = reader
        .get_block_authoritative(&EntityUri::block("leaf"))
        .await
        .unwrap()
        .unwrap();
    let counter = AtomicU64::new(0);

    let mut memo = BlockRowMemo::new();
    memo.prefetch(&EntityUri::block("leaf"), leaf);
    let found = nearest_page_ancestor(
        reader.as_ref(),
        &EntityUri::block("leaf"),
        &mut memo,
        Some(&counter),
    )
    .await
    .unwrap()
    .expect("leaf is inside a page");

    assert_eq!(found.id, EntityUri::block("page"));
    assert_eq!(
        counter.load(Ordering::Relaxed),
        1,
        "only the page above it is read — the caller already had the leaf"
    );
}

/// A page is its own document, which is what makes a page-ness toggle
/// observable as a document change on the toggled block.
#[tokio::test]
async fn a_page_is_its_own_owner() {
    let reader = Arc::new(RecordingReader::new(&[("page", true)]));

    let found = nearest_page_ancestor(
        reader.as_ref(),
        &EntityUri::block("page"),
        &mut BlockRowMemo::new(),
        None,
    )
    .await
    .unwrap()
    .expect("a page owns itself");

    assert_eq!(found.id, EntityUri::block("page"));
}
