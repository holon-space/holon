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
use holon_filesystem::MemoSeam;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::DocHome;
use holon_orgmode::home_authority::HomeBurstMemo;

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

    auth.locate("block:orphan", &mut HomeBurstMemo::default())
        .await
        .unwrap();

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

    auth.locate("block:leaf", &mut HomeBurstMemo::default())
        .await
        .unwrap();

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
        .locate("block:leaf", &mut HomeBurstMemo::default())
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
        .locate("block:orphan", &mut HomeBurstMemo::default())
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

// ---- the burst memo, and the seams that prove it is audited ---------------

/// The memo's whole point: a repeated lookup inside one burst costs nothing.
/// Across bursts it costs full price, which is what keeps the memo's soundness
/// argument ("no write can land inside a burst") load-bearing rather than
/// decorative.
#[tokio::test]
async fn a_burst_memo_serves_a_repeated_locate_without_re_reading() {
    let reader = Arc::new(RecordingReader::new(&[
        ("page", true),
        ("mid", false),
        ("leaf", false),
    ]));
    let auth = authority(reader.clone());

    let mut memo = HomeBurstMemo::default();
    auth.locate("block:leaf", &mut memo).await.unwrap();
    let after_first = reader.reads().len();
    auth.locate("block:leaf", &mut memo).await.unwrap();
    assert_eq!(
        reader.reads().len(),
        after_first,
        "a second locate of the same block in the SAME burst must issue no read at all, got {:?}",
        reader.reads()
    );

    auth.locate("block:leaf", &mut HomeBurstMemo::default())
        .await
        .unwrap();
    assert!(
        reader.reads().len() > after_first,
        "a new burst starts from an empty memo — otherwise a write between bursts would never be \
         seen"
    );
}

/// Law 5. Poison one memo entry and the dual-read beside the next hit must
/// name it, loudly, instead of letting the fold render from it. A memo whose
/// corruption cannot be made to fail is an unfalsifiable claim.
#[tokio::test]
async fn a_poisoned_burst_memo_is_disclosed_and_not_served() {
    let reader = Arc::new(RecordingReader::new(&[
        ("page", true),
        ("mid", false),
        ("leaf", false),
    ]));
    let err = authority(reader)
        .locate(
            "block:leaf",
            &mut HomeBurstMemo::with_seam(MemoSeam::armed()),
        )
        .await
        .expect_err("an armed memo poison must surface as an error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("stale") && msg.contains("block:leaf"),
        "the disclosure must name the stale key and say what happened, got: {msg}"
    );
}

/// The poison is not a no-op: with the audit half disarmed the SAME corruption
/// silently moves the block to another document. This is what the test above
/// prevents, and why the seam is not ceremony.
#[tokio::test]
async fn without_the_dual_read_a_poisoned_memo_silently_rehomes_the_block() {
    let chain: &[(&str, bool)] = &[("page", true), ("mid", false), ("leaf", false)];

    let clean = authority(Arc::new(RecordingReader::new(chain)))
        .locate("block:leaf", &mut HomeBurstMemo::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(clean.doc, DocHome::Resolved(EntityUri::block("page")));

    let poisoned = authority(Arc::new(RecordingReader::new(chain)))
        .locate(
            "block:leaf",
            &mut HomeBurstMemo::with_seam(MemoSeam {
                poison: true,
                dual_read: false,
            }),
        )
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        poisoned.doc, clean.doc,
        "a poisoned row must actually change the fold's input, or the disclosure test above \
         proves nothing"
    );
    assert_eq!(poisoned.doc, DocHome::Unresolved);
}

// ─── What the derived home-profile lookup costs ──────────────────────────
//
// The home-profile binding is DERIVED on every read: resolve the block's home,
// then map it to a profile id. The question this measures is whether that
// derivation needs a stored column beside it.
//
// The unit is READS, not wall-clock. This reader is a `BTreeMap`, so its
// wall-clock excludes every byte of Turso I/O and would be a floor, not a
// measurement — quoting it against the 200 ms p95 interaction SLO would be
// dishonest. What this harness CAN establish honestly is the shape: how many
// authoritative point reads one lookup costs, and how that grows with depth.
// The SLO answer is that count multiplied by whatever one `block_raw` point
// read costs on the target device.

/// Depths that bracket the keystone's own tree. Its working set is a page with
/// leaf children (`structural-page` → `parent`/`c1`/`c2`) and the journals page
/// with its rule-minted day and children, so 1–4 is the real range; 16 is
/// included to show the growth law rather than to claim a vault is that deep.
const MEASURED_DEPTHS: &[usize] = &[1, 2, 4, 16];

/// One lookup's read cost at `depth` intermediate blocks under a page.
async fn derived_lookup_reads(depth: usize) -> usize {
    let mut chain: Vec<(String, bool)> = vec![("page".to_string(), true)];
    for i in 0..depth {
        chain.push((format!("mid{i}"), false));
    }
    let borrowed: Vec<(&str, bool)> = chain.iter().map(|(s, p)| (s.as_str(), *p)).collect();
    let reader = Arc::new(RecordingReader::new(&borrowed));
    let target = format!("block:{}", chain.last().unwrap().0);

    authority(reader.clone())
        .locate(&target, &mut HomeBurstMemo::default())
        .await
        .unwrap()
        .unwrap();
    reader.reads().len()
}

/// A stored column would answer in ONE read. This is that arm: the same reader,
/// asked for the block itself and nothing above it.
async fn stored_column_reads(depth: usize) -> usize {
    let mut chain: Vec<(String, bool)> = vec![("page".to_string(), true)];
    for i in 0..depth {
        chain.push((format!("mid{i}"), false));
    }
    let borrowed: Vec<(&str, bool)> = chain.iter().map(|(s, p)| (s.as_str(), *p)).collect();
    let reader = Arc::new(RecordingReader::new(&borrowed));
    let target = EntityUri::block(&chain.last().unwrap().0);

    reader.get_block_authoritative(&target).await.unwrap();
    reader.reads().len()
}

/// The measurement 2b.4b rests on: what the derived lookup costs per depth,
/// against the one read a column would cost.
///
/// Arms ALTERNATE within each depth so any drift in the harness lands on both,
/// and every arm's run count is asserted — a measurement that quietly ran fewer
/// iterations than it reports is the failure mode worth guarding here.
#[tokio::test]
async fn the_derived_home_profile_lookup_costs_one_read_per_ancestor() {
    const RUNS_PER_ARM: usize = 8;

    let mut report = String::from("derived home-profile lookup, reads per lookup\n");
    for &depth in MEASURED_DEPTHS {
        let mut derived: Vec<usize> = Vec::new();
        let mut stored: Vec<usize> = Vec::new();
        for _ in 0..RUNS_PER_ARM {
            // A then B, every iteration.
            derived.push(derived_lookup_reads(depth).await);
            stored.push(stored_column_reads(depth).await);
        }

        assert_eq!(
            (derived.len(), stored.len()),
            (RUNS_PER_ARM, RUNS_PER_ARM),
            "both arms must have run {RUNS_PER_ARM} times at depth {depth}"
        );
        // The fixture is real: a lookup that read nothing measured nothing.
        assert!(
            derived.iter().all(|&r| r > 0) && stored.iter().all(|&r| r > 0),
            "every run must have issued reads at depth {depth}: {derived:?} / {stored:?}"
        );

        // The DISTRIBUTION, not a ratio: min/max of each arm, both stated.
        let d_min = *derived.iter().min().unwrap();
        let d_max = *derived.iter().max().unwrap();
        let s_min = *stored.iter().min().unwrap();
        let s_max = *stored.iter().max().unwrap();
        report.push_str(&format!(
            "  depth {depth:>2}: derived {d_min}..{d_max} reads · stored-column {s_min}..{s_max} \
             reads\n"
        ));

        assert_eq!(
            (d_min, d_max),
            (depth + 1, depth + 1),
            "the walk reads the block plus each ancestor up to the page — one read per step, no \
             spread"
        );
        assert_eq!((s_min, s_max), (1, 1), "a column answers in one read");
    }
    println!("{report}");
}
