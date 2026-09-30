//! `find_foreign_blocks` answers from the blocks it is asked
//! about, not from the whole store.
//!
//! Every file ingest asks it about the file's new block ids, so a cost that
//! grows with the store makes a vault's boot quadratic in its size (BugFunnel
//! `2026-09-30-a-deep-vault-keeps-the-org-sync-busy-for-minutes-after-boot`).
//! The answer is compared against `blocks_by_document` over the whole store,
//! which is what "owned by another document" means.
//!
//! @pbt kind property
//! @pbt covers find-foreign-blocks-cost — a file's ingest reads O(its blocks),
//!   not O(the vault)

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::blocks_by_document;
use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockReader;
use holon_filesystem::find_foreign_blocks;
use proptest::prelude::*;

/// A store that counts every row it hands out, whichever read asked for it.
/// `merged` ids answer with their survivor's row, as a merged-away block does.
struct CountingStore {
    blocks: BTreeMap<EntityUri, Block>,
    merged: BTreeMap<EntityUri, EntityUri>,
    rows_served: AtomicUsize,
}

#[async_trait]
impl BlockReader for CountingStore {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        unimplemented!("not a find_foreign_blocks read")
    }
    async fn doc_block_topology(
        &self,
        _: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        unimplemented!("not a find_foreign_blocks read")
    }
    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        let row = self.blocks.get(self.merged.get(id).unwrap_or(id)).cloned();
        if row.is_some() {
            self.rows_served.fetch_add(1, Ordering::Relaxed);
        }
        Ok(row)
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        let all: Vec<Block> = self.blocks.values().cloned().collect();
        self.rows_served.fetch_add(all.len(), Ordering::Relaxed);
        Ok(blocks_by_document(&all))
    }
}

/// Where a generated block's parent points.
#[derive(Debug, Clone)]
enum Parent {
    Root,
    /// An earlier block: the acyclic case.
    Earlier(usize),
    /// Any block, itself included: this draws parent cycles.
    Any(usize),
    /// A row the store does not hold.
    Missing,
}

#[derive(Debug, Clone)]
struct Node {
    parent: Parent,
    page: bool,
}

fn forest(max: usize) -> impl Strategy<Value = Vec<Node>> {
    (1..=max).prop_flat_map(move |n| {
        (0..n)
            .map(|i| {
                let parent = if i == 0 {
                    prop_oneof![4 => Just(Parent::Root), 1 => Just(Parent::Missing)].boxed()
                } else {
                    prop_oneof![
                        2 => Just(Parent::Root),
                        12 => (0..i).prop_map(Parent::Earlier),
                        1 => (0..n).prop_map(Parent::Any),
                        1 => Just(Parent::Missing),
                    ]
                    .boxed()
                };
                (parent, prop::bool::weighted(0.2)).prop_map(|(parent, page)| Node { parent, page })
            })
            .collect::<Vec<_>>()
    })
}

fn id(i: usize) -> EntityUri {
    EntityUri::block(&format!("b{i:05}"))
}

fn store(nodes: &[Node]) -> CountingStore {
    let blocks = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let parent = match n.parent {
                Parent::Root => EntityUri::no_parent(),
                Parent::Earlier(p) | Parent::Any(p) => id(p),
                Parent::Missing => EntityUri::block(&format!("missing-parent-of-{i}")),
            };
            let mut b = Block::new_text(id(i), parent, format!("b{i}"));
            b.set_page(n.page);
            (id(i), b)
        })
        .collect();
    CountingStore {
        blocks,
        merged: BTreeMap::new(),
        rows_served: AtomicUsize::new(0),
    }
}

/// The most rows one walk from any block can read: its chain up to the root,
/// a missing parent, a repeat, or the walk bound.
fn depth(store: &CountingStore) -> usize {
    store
        .blocks
        .keys()
        .map(|start| {
            let mut seen = BTreeSet::new();
            let mut cur = start;
            while let Some(b) = store.blocks.get(cur) {
                if !seen.insert(cur) {
                    break;
                }
                cur = &b.parent_id;
            }
            seen.len()
        })
        .max()
        .unwrap_or(0)
}

fn reference(
    store: &CountingStore,
    asked: &[EntityUri],
    expected: &EntityUri,
) -> BTreeSet<(EntityUri, EntityUri)> {
    let all: Vec<Block> = store.blocks.values().cloned().collect();
    let asked: BTreeSet<&EntityUri> = asked.iter().collect();
    blocks_by_document(&all)
        .into_iter()
        .filter(|(doc, _)| doc != expected)
        .flat_map(|(doc, blocks)| {
            blocks
                .into_iter()
                .filter(|b| asked.contains(&b.id))
                .map(move |b| (b.id, doc.clone()))
        })
        .collect()
}

fn run(
    store: &CountingStore,
    asked: &[EntityUri],
    expected: &EntityUri,
) -> Vec<(Block, EntityUri)> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(find_foreign_blocks(store, asked, expected))
        .expect("find_foreign_blocks over an in-memory store")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The answer is the whole-store attribution restricted to the asked ids,
    /// and reading it costs at most one row per asked id and ancestor level.
    /// Asked ids may repeat, be absent, or be merged away into another block.
    #[test]
    fn answers_the_whole_store_question_from_the_asked_blocks(
        nodes in forest(45),
        picks in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
        absent in 0usize..4,
        merged in prop::collection::vec((any::<prop::sample::Index>(), any::<prop::sample::Index>()), 0..3),
        expected_pick in any::<prop::sample::Index>(),
    ) {
        let mut store = store(&nodes);
        let mut asked: Vec<EntityUri> = picks.iter().map(|p| id(p.index(nodes.len()))).collect();
        asked.extend((0..absent).map(|k| EntityUri::block(&format!("absent-{k}"))));
        for (k, (survivor, _)) in merged.iter().enumerate() {
            let gone = EntityUri::block(&format!("merged-away-{k}"));
            store.merged.insert(gone.clone(), id(survivor.index(nodes.len())));
            asked.push(gone);
        }
        let pages: Vec<EntityUri> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.page)
            .map(|(i, _)| id(i))
            .chain([EntityUri::no_parent()])
            .collect();
        let expected = pages[expected_pick.index(pages.len())].clone();

        let found = run(&store, &asked, &expected);

        let got: BTreeSet<(EntityUri, EntityUri)> =
            found.iter().map(|(b, doc)| (b.id.clone(), doc.clone())).collect();
        prop_assert_eq!(got.len(), found.len(), "a block reported twice: {:?}", found);
        prop_assert_eq!(got, reference(&store, &asked, &expected));
        let distinct: BTreeSet<&EntityUri> = asked.iter().collect();
        let bound = distinct.len() * depth(&store);
        let served = store.rows_served.load(Ordering::Relaxed);
        prop_assert!(
            served <= bound,
            "find_foreign_blocks read {served} rows to answer {} ids in a store of {} (bound \
             {bound} = ids x deepest chain)",
            distinct.len(),
            nodes.len()
        );
    }

    /// A new file's ids are in no document yet. Asking about them must not
    /// read the vault it is joining, however large that vault is.
    #[test]
    fn a_new_files_ids_read_nothing_from_the_vault(
        vault in 50usize..2000,
        new_ids in 1usize..60,
    ) {
        let nodes: Vec<Node> = (0..vault)
            .map(|i| Node {
                parent: if i % 40 != 0 { Parent::Earlier(i - 1) } else { Parent::Root },
                page: i % 40 == 0,
            })
            .collect();
        let store = store(&nodes);
        let asked: Vec<EntityUri> =
            (0..new_ids).map(|k| EntityUri::block(&format!("new-{k}"))).collect();

        let found = run(&store, &asked, &EntityUri::block("new-doc"));

        prop_assert!(found.is_empty());
        prop_assert_eq!(
            store.rows_served.load(Ordering::Relaxed),
            0,
            "asking about {} ids that exist nowhere read rows of a {}-block vault",
            new_ids,
            vault
        );
    }
}

type Conflict = (Block, EntityUri);

/// `MAX_PAGE_WALK`: the most blocks, the page included, a walk reads.
const WALK_BOUND: usize = 50;

/// A chain of `non_pages` plain blocks under one page; the last block asked
/// about with an `expected` document that is not the page.
fn chain_answer(non_pages: usize) -> (Vec<Conflict>, BTreeSet<(EntityUri, EntityUri)>) {
    let nodes: Vec<Node> = (0..=non_pages)
        .map(|i| Node {
            parent: if i == 0 {
                Parent::Root
            } else {
                Parent::Earlier(i - 1)
            },
            page: i == 0,
        })
        .collect();
    let store = store(&nodes);
    let asked = [id(non_pages)];
    let expected = EntityUri::block("other-doc");
    (
        run(&store, &asked, &expected),
        reference(&store, &asked, &expected),
    )
}

/// The one place the answer departs from `blocks_by_document`: the ancestor
/// walk gives up after `MAX_PAGE_WALK` (50) blocks and attributes the block
/// to `no_parent`.
#[test]
fn a_chain_past_the_walk_bound_is_attributed_to_no_parent() {
    let page = id(0);
    let (found, oracle) = chain_answer(WALK_BOUND);
    let got: BTreeSet<_> = found
        .iter()
        .map(|(b, d)| (b.id.clone(), d.clone()))
        .collect();
    assert_eq!(got, oracle, "within the bound the page owns the block");
    assert_eq!(found[0].1, page);

    let (found, oracle) = chain_answer(WALK_BOUND + 1);
    assert_eq!(
        oracle.iter().next().unwrap().1,
        page,
        "oracle still names the page"
    );
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].1,
        EntityUri::no_parent(),
        "past the bound: no owner"
    );
}

/// Repeated asked ids yield one conflict each, in first-seen order.
#[test]
fn repeated_ids_are_answered_once_in_first_seen_order() {
    let nodes = [
        Node {
            parent: Parent::Root,
            page: true,
        },
        Node {
            parent: Parent::Earlier(0),
            page: false,
        },
        Node {
            parent: Parent::Earlier(0),
            page: false,
        },
    ];
    let store = store(&nodes);
    let asked = [id(2), id(1), id(2), id(1), id(2)];

    let found = run(&store, &asked, &EntityUri::no_parent());

    let ids: Vec<EntityUri> = found.into_iter().map(|(b, _)| b.id).collect();
    assert_eq!(ids, vec![id(2), id(1)]);
}
