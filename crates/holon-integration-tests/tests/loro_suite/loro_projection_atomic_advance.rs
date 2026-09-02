//! Atomic base-advance fault-injection test for the incremental Loro→SQL
//! projection (`LoroProjection::project`).
//!
//! Contract under test: when the sink write fails (`consolidator.apply` →
//! `Err`, driven here by a `MemorySink` whose `execute_batch_with_origin`
//! returns `Err`), `project()`:
//!   1. returns `Err`,
//!   2. does NOT advance the incremental diff base `live`,
//!   3. does NOT advance the `last_synced` watermark,
//!   4. flips `seeded` to `false` (forcing a full reseed next pass), and
//!   5. RE-EMITS the change on the next successful pass — never silently drops
//!      it.
//!
//! The harness is `projection_harness`.
//!
//! @pbt kind harness
//! @pbt covers loro-projection-atomic — atomic base-advance fault injection for
//! Loro→SQL projection

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
use holon_core::OriginTaggedWrites;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroProjection;
use holon_loro::PendingChange;
use holon_loro::SinkReader;
use loro::Frontiers;
use loro::TreeParentId;
use tokio::sync::RwLock;

use crate::projection_harness::MemorySink;
use crate::projection_harness::insert_root_block;

#[tokio::test]
async fn failed_sink_write_neither_advances_base_nor_drops_change() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    // Force global doc init (schema: fractional index enabled) so the tree
    // accepts nodes and the projection has something to read.
    doc_store.read().await.get_doc(DocScope::Global).await?;

    let sink = Arc::new(MemorySink::new());
    let projection = LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        tempdir.path().join("sc.sync"),
    );
    projection.arm();

    // ── (a) fail=false: insert B, seed `live` via a full (cold-boot) pass ──────
    sink.set_fail(false);
    insert_root_block(&doc_store, "B-id", "block B").await?;
    projection.project().await.expect("seed pass succeeds");

    assert!(projection.is_seeded(), "first successful pass seeds `live`");
    assert!(
        projection.live_snapshot().contains_key("block:B-id"),
        "`live` holds B after the seed pass: {:?}",
        projection.live_snapshot().keys().collect::<Vec<_>>()
    );
    let l1 = projection.last_synced_value();
    let calls1 = sink.apply_calls();
    assert_eq!(calls1, 1, "one apply (the seed create) so far");

    // ── (b) insert C, stage its create fact, arm the failure ──────────────────
    let c_tid = insert_root_block(&doc_store, "C-id", "block C").await?;
    projection
        .pending()
        .lock()
        .unwrap()
        .push(PendingChange::Create {
            parent: TreeParentId::Root,
            target: c_tid,
        });
    sink.set_fail(true);

    // ── (c) failing incremental pass: Err, base untouched, seeded flipped ──────
    let r = projection.project().await;
    assert!(r.is_err(), "a failed sink write surfaces as Err");
    assert!(
        !projection.is_seeded(),
        "failure flips `seeded` false to force a full reseed"
    );
    assert!(
        !projection.live_snapshot().contains_key("block:C-id"),
        "staging is NOT applied on failure — `live` must not gain C: {:?}",
        projection.live_snapshot().keys().collect::<Vec<_>>()
    );
    assert_eq!(
        projection.last_synced_value(),
        l1,
        "`last_synced` must not advance past the last committed sink write"
    );
    assert!(
        sink.apply_calls() > calls1,
        "the sink apply was ATTEMPTED (and failed): {} !> {}",
        sink.apply_calls(),
        calls1
    );
    let calls_after_fail = sink.apply_calls();

    // ── (d) recovery: next successful pass RE-EMITS C (never silently dropped) ─
    sink.set_fail(false);
    projection.project().await.expect("recovery pass succeeds");
    assert!(
        projection.live_snapshot().contains_key("block:C-id"),
        "C is re-emitted on recovery — the change was never dropped: {:?}",
        projection.live_snapshot().keys().collect::<Vec<_>>()
    );
    assert!(
        sink.apply_calls() > calls_after_fail,
        "the recovery pass re-attempted the apply: {} !> {}",
        sink.apply_calls(),
        calls_after_fail
    );

    Ok(())
}
