//! The `home_by` ancestor walk must read each ancestor at most once.
//!
//! `BlockHomeAuthority::resolve_doc` walks a block's parent chain upward
//! looking for the nearest `Page`. Under Turso those reads land on `block_raw`,
//! which contains the **self-parented** `sentinel:no_parent` FK-anchor row
//! (`holon-turso/src/schema_modules.rs`). A chain that reaches the tree root
//! therefore stops advancing — `cur = block.parent_id` returns the sentinel to
//! itself — and the walk re-reads that one row until the depth bound, once per
//! delta, with identical bindings. `locate_batch` already terminates at the
//! root (it maps the sentinel parent to `None`); the per-delta `locate` path is
//! the one that spins.
//!
//! These tests fix the read COUNT and, separately, the resolved value — the
//! fix must remove reads without moving a single answer.

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
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::DocHome;

/// A store shaped like `block_raw`: it contains the self-parented root
/// sentinel, and it records every authoritative point read.
struct RecordingReader {
    blocks: BTreeMap<EntityUri, Block>,
    reads: Mutex<Vec<String>>,
}

impl RecordingReader {
    /// `chain` is listed root-first; each entry is `(id, is_page)`. The last
    /// entry's child is the block under test.
    fn new(chain: &[(&str, bool)]) -> Self {
        let mut blocks = BTreeMap::new();

        // The FK anchor, exactly as `CoreSchemaModule` seeds it: parented to
        // itself. Reading it is what used to spin the walk.
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

    fn reads(&self) -> Vec<String> {
        self.reads.lock().unwrap().clone()
    }

    /// The id read more times than any other, with its count.
    fn worst_repeat(&self) -> (String, usize) {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for id in self.reads() {
            *counts.entry(id).or_default() += 1;
        }
        counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .unwrap_or_else(|| ("<none>".into(), 0))
    }
}

#[async_trait]
impl BlockReader for RecordingReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        unimplemented!("not exercised by the ancestor walk")
    }

    async fn doc_block_topology(
        &self,
        _: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        unimplemented!("not exercised by the ancestor walk")
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        self.reads.lock().unwrap().push(id.as_str().to_string());
        Ok(self.blocks.get(id).cloned())
    }

    async fn resolve_link_marks(&self, _: &mut [Block]) -> anyhow::Result<()> {
        Ok(())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        unimplemented!("not exercised by the ancestor walk")
    }
}

/// `locate` consults ordering only through `prev_sibling`, which these tests
/// never call — they drive `locate`, whose cost is entirely reader-side.
struct UnusedOrdering;

#[async_trait]
impl BlockOrdering for UnusedOrdering {
    async fn place(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> OrderingResult<()> {
        unimplemented!("not exercised")
    }
    async fn prev_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        unimplemented!("not exercised")
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
    async fn children(&self, _: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        unimplemented!("not exercised")
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        unimplemented!("not exercised")
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        unimplemented!("not exercised")
    }
}

fn authority(reader: Arc<RecordingReader>) -> BlockHomeAuthority {
    BlockHomeAuthority::new(reader, Arc::new(UnusedOrdering))
}

/// THE DEFECT. A block with no `Page` ancestor walks to the root sentinel,
/// which is its own parent — so every remaining step re-reads that same row.
#[tokio::test]
async fn a_walk_to_the_tree_root_reads_no_id_twice() {
    // `orphan` sits directly under the root: nothing above it is a Page.
    let reader = Arc::new(RecordingReader::new(&[("orphan", false)]));
    let auth = authority(reader.clone());

    auth.locate("block:orphan").await.unwrap();

    let (worst_id, worst_count) = reader.worst_repeat();
    assert_eq!(
        worst_count,
        1,
        "the ancestor walk re-read `{worst_id}` {worst_count}x for ONE locate — the root sentinel \
         is self-parented in block_raw, so the walk stops advancing and burns the whole depth \
         bound on identical point reads. Full read log: {:?}",
        reader.reads()
    );
}

/// The same bound with a real document above: one read per distinct block on
/// the path, and the located block is not read twice just because `locate`
/// fetched it before handing off to the walk.
#[tokio::test]
async fn a_walk_to_a_page_reads_each_block_on_the_path_once() {
    let reader = Arc::new(RecordingReader::new(&[
        ("page", true),
        ("mid", false),
        ("leaf", false),
    ]));
    let auth = authority(reader.clone());

    auth.locate("block:leaf").await.unwrap();

    let reads = reader.reads();
    let mut unique = reads.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        reads.len(),
        unique.len(),
        "every block on the path must be read at most once per locate, got {reads:?}"
    );
    assert_eq!(
        unique,
        vec![
            "block:leaf".to_string(),
            "block:mid".to_string(),
            "block:page".to_string()
        ],
        "the walk must read exactly the path leaf→page and nothing else"
    );
}

/// Convergence: the reads may shrink, the ANSWER may not move. Both directions
/// are pinned so a fix cannot buy cheapness with a wrong home.
#[tokio::test]
async fn the_resolved_home_is_unchanged_by_how_few_reads_it_took() {
    let paged = Arc::new(RecordingReader::new(&[("page", true), ("leaf", false)]));
    let placement = authority(paged)
        .locate("block:leaf")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        placement.doc,
        DocHome::Resolved(EntityUri::block("page")),
        "a block under a Page is homed to that page"
    );
    assert_eq!(
        placement.parent,
        Some("block:page".to_string()),
        "the placement's parent is the block's own parent"
    );

    let orphan = Arc::new(RecordingReader::new(&[("orphan", false)]));
    let placement = authority(orphan)
        .locate("block:orphan")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        placement.doc,
        DocHome::Unresolved,
        "a block with no Page ancestor has no document — reaching the root sentinel is the \
         definitive answer, not a reason to keep walking"
    );
    assert_eq!(
        placement.parent, None,
        "a root-level block reports no parent (the sentinel is not a parent)"
    );
}
