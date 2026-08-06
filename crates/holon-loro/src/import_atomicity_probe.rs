//! Probe (task #25): is a REMOTE import batch atomic w.r.t. concurrent local
//! readers, or does it expose the same half-born window a LOCAL create does?
//!
//! Local creates expose it: `tree.create()` and the `STABLE_ID` meta insert are
//! two doc-state steps inside one `with_write`, and `with_write` excludes no
//! reader (`loro_backend::half_born_node_tests`). The question here is the
//! import side, because that decides whether every reader in the codebase must
//! keep the withhold-a-half-born-node guard or only the local-write paths do.
//!
//! The probe runs the REAL sync granularity: doc A writes through
//! `LoroBackend::create_block_with_properties` / `move_block`, exports
//! `ExportMode::updates(&b.oplog_vv())` — the mode `IrohSyncAdapter`'s
//! `export_delta_or_full_snapshot` uses (`iroh_sync_adapter.rs:93`) — and doc B
//! `import`s it (`iroh_sync_adapter.rs:303,417`) while a raw `std::thread`
//! hammers doc B's tree.
//!
//! VERDICT: ATOMIC BY DESIGN — half-born is a LOCAL-WRITE phenomenon only.
//! Loro applies a whole imported delta to `DocState` under ONE state-lock
//! acquisition, and every read takes that same lock, so a reader sees the
//! pre-import or the complete post-import state and nothing between. Evidence
//! in `loro` at rev `855da28b`
//! (`~/.cargo/git/checkouts/loro-e88d0f38e94f7134/855da28`):
//!
//! - `crates/loro-internal/src/loro.rs:786` — the ONLY `self.state.lock()` on
//!   the `FastUpdates` import path (what `ExportMode::updates` produces), held
//!   across the single `state.apply_diff(…)` of the WHOLE decoded delta at
//!   `:787-802`. The `OutdatedRle`/snapshot path is the same shape at `:671`.
//! - `crates/loro/src/lib.rs:880-883` — `with_state` takes
//!   `self.doc.app_state().lock()`, the same lock, and every tree/map read goes
//!   through it (`crates/loro-internal/src/handler/tree.rs:972` for
//!   `get_nodes_under`).
//! - `crates/loro-internal/src/loro.rs:818-827` — subscriber callbacks are
//!   emitted AFTER the state lock is dropped, so no reentrant reader can
//!   observe the interior either.
//!
//! The contrast that makes half-born local-only: a local create's two steps go
//! through `with_write` (`loro_document.rs:126`), which takes NO lock and
//! releases the state lock between `tree.create()` and the meta insert.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use holon_api::BlockContent;
use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::repository::CoreOperations;
use loro::LoroDoc;
use loro::TreeParentId;

use crate::LoroDocument;
use crate::loro_backend::LoroBackend;
use crate::loro_backend::STABLE_ID;
use crate::loro_backend::TREE_NAME;

/// What one reader pass saw of doc B's tree.
#[derive(Default)]
struct Observations {
    reads: AtomicU64,
    /// Reads that saw at least one live node without a `STABLE_ID` — the
    /// half-born signature.
    half_born_reads: AtomicU64,
    /// Every distinct live-node count seen. A count that is not a settled
    /// batch boundary proves the reader saw a PARTIALLY applied import even if
    /// no node was ever caught between its create and its meta.
    live_counts: Mutex<BTreeSet<usize>>,
}

/// The read that the half-born guards protect: scan every live node and read
/// its `STABLE_ID`. Mirrors `LoroTreeView::build`'s node filter
/// (`loro_backend.rs:1158-1180`).
fn scan(doc: &LoroDoc) -> (usize, usize) {
    let tree = doc.get_tree(TREE_NAME);
    let mut live = 0usize;
    let mut without_id = 0usize;
    for node in tree.get_nodes(false) {
        if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
            continue;
        }
        live += 1;
        let Ok(meta) = tree.get_meta(node.id) else {
            continue;
        };
        if meta.get(STABLE_ID).is_none() {
            without_id += 1;
        }
    }
    (live, without_id)
}

fn spawn_hammer(
    doc: Arc<LoroDoc>,
    obs: Arc<Observations>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let (live, without_id) = scan(&doc);
            obs.reads.fetch_add(1, Ordering::Relaxed);
            if without_id > 0 {
                obs.half_born_reads.fetch_add(1, Ordering::Relaxed);
            }
            obs.live_counts.lock().unwrap().insert(live);
        }
    })
}

async fn create(backend: &LoroBackend, parent: EntityUri, id: &str) {
    backend
        .create_block_with_properties(
            parent,
            BlockContent::text(id),
            Some(EntityUri::block(id)),
            &HashMap::new(),
            &Tags::default(),
            &[],
            &[],
        )
        .await
        .unwrap();
}

/// Export exactly what the sync layer ships: the delta the receiver is missing.
fn delta_for(a: &LoroDoc, b: &LoroDoc) -> Vec<u8> {
    a.commit();
    a.export(loro::ExportMode::updates(&b.oplog_vv())).unwrap()
}

/// VERDICT TEST — a multi-block import batch observed by a concurrent reader.
///
/// 120 import cycles, each carrying `BLOCKS_PER_CYCLE` creates that were each
/// committed separately on doc A (so the batch spans several Loro changes, the
/// widest realistic window). If `import` applied ops the way `with_write` does
/// — visible to readers as they land — the hammer would see both a live node
/// without `STABLE_ID` and live counts strictly between cycle boundaries.
#[tokio::test]
async fn a_multi_block_import_is_never_observed_half_applied() {
    const CYCLES: usize = 120;
    const BLOCKS_PER_CYCLE: usize = 8;

    let doc_a = Arc::new(LoroDocument::new("import-probe-a".to_string()).unwrap());
    let backend_a = LoroBackend::from_document(doc_a.clone());
    create(&backend_a, EntityUri::no_parent(), "root").await;

    let doc_b = Arc::new(LoroDocument::new("import-probe-b".to_string()).unwrap());
    let raw_a = doc_a.doc();
    let raw_b = doc_b.doc();

    // Ship the root first so every later cycle is a pure N-node delta and the
    // settled counts are exactly `1 + k * BLOCKS_PER_CYCLE`.
    raw_b.import(&delta_for(&raw_a, &raw_b)).unwrap();

    let obs = Arc::new(Observations::default());
    let stop = Arc::new(AtomicBool::new(false));
    let hammer = spawn_hammer(raw_b.clone(), obs.clone(), stop.clone());

    let mut settled: BTreeSet<usize> = BTreeSet::new();
    settled.insert(1);
    for cycle in 0..CYCLES {
        for i in 0..BLOCKS_PER_CYCLE {
            create(
                &backend_a,
                EntityUri::block("root"),
                &format!("b{cycle}-{i}"),
            )
            .await;
        }
        let delta = delta_for(&raw_a, &raw_b);
        assert!(!delta.is_empty(), "cycle {cycle} produced an empty delta");
        raw_b.import(&delta).unwrap();
        settled.insert(1 + (cycle + 1) * BLOCKS_PER_CYCLE);
    }

    stop.store(true, Ordering::Relaxed);
    hammer.join().unwrap();

    let reads = obs.reads.load(Ordering::Relaxed);
    let half_born = obs.half_born_reads.load(Ordering::Relaxed);
    let counts = obs.live_counts.lock().unwrap().clone();
    let intermediate: Vec<_> = counts.difference(&settled).copied().collect();

    println!(
        "multi-block import: {CYCLES} cycles x {BLOCKS_PER_CYCLE} blocks, {reads} concurrent \
         reads, {half_born} half-born reads, live counts seen {counts:?}"
    );
    assert!(
        reads > CYCLES as u64,
        "the hammer must out-pace the imports to have a chance at the window \
         (reads={reads}, cycles={CYCLES})"
    );
    assert_eq!(
        half_born, 0,
        "a concurrent reader observed {half_born}/{reads} reads with a live node lacking \
         STABLE_ID DURING an import — half-born is NOT local-only; see task #25"
    );
    assert!(
        intermediate.is_empty(),
        "a concurrent reader observed live-node counts {intermediate:?} that are not import \
         boundaries {settled:?} — the import batch is applied to doc state incrementally, \
         so readers CAN see it half-applied"
    );
}

/// Same probe for a batch that mixes a create with a move: the move's
/// `TreeParentId` change and the new node's meta are separate ops, so an
/// incrementally-visible import would let a reader see the moved node under
/// its old parent while the imported sibling is already live.
#[tokio::test]
async fn an_import_carrying_create_and_move_is_never_observed_half_applied() {
    const CYCLES: usize = 120;

    let doc_a = Arc::new(LoroDocument::new("import-probe-mv-a".to_string()).unwrap());
    let backend_a = LoroBackend::from_document(doc_a.clone());
    create(&backend_a, EntityUri::no_parent(), "root").await;
    create(&backend_a, EntityUri::no_parent(), "other").await;
    create(&backend_a, EntityUri::block("root"), "mover").await;

    let doc_b = Arc::new(LoroDocument::new("import-probe-mv-b".to_string()).unwrap());
    let raw_a = doc_a.doc();
    let raw_b = doc_b.doc();
    raw_b.import(&delta_for(&raw_a, &raw_b)).unwrap();

    let obs = Arc::new(Observations::default());
    let stop = Arc::new(AtomicBool::new(false));
    let hammer = spawn_hammer(raw_b.clone(), obs.clone(), stop.clone());

    let mut settled: BTreeSet<usize> = BTreeSet::new();
    settled.insert(3);
    for cycle in 0..CYCLES {
        create(&backend_a, EntityUri::block("other"), &format!("n{cycle}")).await;
        let to = if cycle % 2 == 0 { "other" } else { "root" };
        backend_a
            .move_block(&EntityUri::block("mover"), EntityUri::block(to), None)
            .await
            .unwrap();
        raw_b.import(&delta_for(&raw_a, &raw_b)).unwrap();
        settled.insert(3 + cycle + 1);
    }

    stop.store(true, Ordering::Relaxed);
    hammer.join().unwrap();

    let reads = obs.reads.load(Ordering::Relaxed);
    let half_born = obs.half_born_reads.load(Ordering::Relaxed);
    let counts = obs.live_counts.lock().unwrap().clone();
    let intermediate: Vec<_> = counts.difference(&settled).copied().collect();

    println!(
        "create+move import: {CYCLES} cycles, {reads} concurrent reads, {half_born} half-born \
         reads, live counts seen {counts:?}"
    );
    assert!(
        reads > CYCLES as u64,
        "hammer must out-pace the imports (reads={reads})"
    );
    assert_eq!(
        half_born, 0,
        "a create+move import batch exposed a node without STABLE_ID to a concurrent reader \
         ({half_born}/{reads} reads); see task #25"
    );
    assert!(
        intermediate.is_empty(),
        "create+move import observed at non-boundary live counts {intermediate:?} \
         (boundaries {settled:?})"
    );
}

/// WIDEST WINDOW: one import carrying 2000 creates. The 120-cycle probes give
/// the reader ~10 reads per import; a single import this large takes long
/// enough that the hammer completes many reads INSIDE it (asserted below), so
/// an incrementally-visible apply could not hide in timing.
///
/// Honest reading of the numbers: most of those overlapping reads land in the
/// decode/oplog phase, which precedes the single `state.lock()` — once the lock
/// is taken the reader blocks, which IS the mechanism. The test's evidentiary
/// weight is that no read ever returned a count between the endpoints; the
/// exclusion itself is proven by the lock, not by the sample.
#[tokio::test]
async fn a_single_large_import_is_never_observed_half_applied() {
    const BLOCKS: usize = 2000;

    let doc_a = Arc::new(LoroDocument::new("import-probe-big-a".to_string()).unwrap());
    let backend_a = LoroBackend::from_document(doc_a.clone());
    create(&backend_a, EntityUri::no_parent(), "root").await;

    let doc_b = Arc::new(LoroDocument::new("import-probe-big-b".to_string()).unwrap());
    let raw_a = doc_a.doc();
    let raw_b = doc_b.doc();
    raw_b.import(&delta_for(&raw_a, &raw_b)).unwrap();

    for i in 0..BLOCKS {
        create(&backend_a, EntityUri::block("root"), &format!("big{i}")).await;
    }
    let delta = delta_for(&raw_a, &raw_b);

    let obs = Arc::new(Observations::default());
    let stop = Arc::new(AtomicBool::new(false));
    let hammer = spawn_hammer(raw_b.clone(), obs.clone(), stop.clone());
    // Let the hammer spin up so reads are already in flight when the import
    // starts; otherwise "reads during import" could be a cold-start artifact.
    while obs.reads.load(Ordering::Relaxed) < 5 {
        std::hint::spin_loop();
    }

    let before = obs.reads.load(Ordering::Relaxed);
    let started = std::time::Instant::now();
    raw_b.import(&delta).unwrap();
    let elapsed = started.elapsed();
    let after = obs.reads.load(Ordering::Relaxed);

    stop.store(true, Ordering::Relaxed);
    hammer.join().unwrap();

    let half_born = obs.half_born_reads.load(Ordering::Relaxed);
    let counts = obs.live_counts.lock().unwrap().clone();
    let during = after - before;
    println!(
        "single {BLOCKS}-block import took {elapsed:?}; {during} reads overlapped it; \
         {half_born} half-born reads; live counts seen {counts:?}"
    );

    assert!(
        during > 0,
        "no read overlapped the import — the probe proves nothing about the window"
    );
    assert_eq!(
        half_born, 0,
        "a large import exposed a node without STABLE_ID"
    );
    let settled: BTreeSet<usize> = [1usize, 1 + BLOCKS].into_iter().collect();
    let intermediate: Vec<_> = counts.difference(&settled).copied().collect();
    assert!(
        intermediate.is_empty(),
        "a reader saw live counts {intermediate:?} between the import's endpoints \
         {settled:?} — the batch is applied to doc state incrementally"
    );
}

/// TEETH for the two verdict tests above: their `assert_eq!(half_born, 0)` is
/// only evidence if `scan` can see a half-born node at all. It can — this is
/// the LOCAL create window (`tree.create()` before the `STABLE_ID` insert,
/// exactly what `loro_backend::half_born_node_tests` pins) read by the same
/// scan, deterministically because the read runs between the two steps.
#[tokio::test]
async fn the_scan_detects_a_half_born_node_when_one_exists() {
    let doc = Arc::new(LoroDocument::new("import-probe-teeth".to_string()).unwrap());
    let backend = LoroBackend::from_document(doc.clone());
    create(&backend, EntityUri::no_parent(), "root").await;
    let root_tid = backend.resolve_to_tree_id("block:root").await.unwrap();

    let raw = doc.doc();
    assert_eq!(scan(&raw), (1, 0), "the settled doc has no half-born node");

    let tree = raw.get_tree(TREE_NAME);
    let node = tree.create(Some(root_tid)).unwrap();
    assert_eq!(
        scan(&raw),
        (2, 1),
        "between tree.create() and the STABLE_ID insert the scan must see the node without an id"
    );

    tree.get_meta(node)
        .unwrap()
        .insert(STABLE_ID, loro::LoroValue::from("late"))
        .unwrap();
    raw.commit();
    assert_eq!(scan(&raw), (2, 0), "the id lands and the node settles");
}
