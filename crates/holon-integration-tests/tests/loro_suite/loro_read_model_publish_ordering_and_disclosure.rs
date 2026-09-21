//! The two guarantees the read model makes about WHEN it publishes, both
//! driven through the real `LoroProjection` over the `projection_harness`
//! sink.
//!
//! 1. **Ordering.** Publication order equals commit order, because the publish
//!    sits inside `project()`'s `project_lock` critical section
//!    (`crates/holon-loro/src/loro_sync_controller.rs:645`, taken at `:916`,
//!    publish at `:1107`). Nothing in the repo asserted it.
//! 2. **Disclosure.** A sink write that fails leaves the read model AHEAD of
//!    the index — by design, since Loro is the authority and rolling the UI
//!    back to a stale index would be worse. That window must be disclosed and
//!    must be REPORTED by the keystone oracle, not smoothed over.
//!
//! @pbt kind harness
//! @pbt covers read-model-publish-ordering — no consumer sees a newer publish
//!   before an older one, because publishes are serialized with the commits
//! @pbt covers read-model-ahead-of-index-disclosure — a failed sink write
//!   raises `SqlProjectionFailed` and the three-way oracle reports the window

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use holon_api::ConditionBus;
use holon_api::condition_bus::ConditionKind;
use holon_core::OriginTaggedWrites;
use holon_integration_tests::pbt::invariants::bodies::view_model_matches_store::InvViewModelMatchesStore;
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

/// Bounds the test's RUNTIME, never its verdict: how long we wait for a second
/// pass to enter the sink before concluding it is blocked. The verdict rests on
/// `overlap`, a flag the sink sets when two passes are inside an apply at once
/// — a positive observation with no timing assumption in it.
const OVERLAP_WINDOW: Duration = Duration::from_millis(400);

fn projection(
    doc_store: &Arc<RwLock<LoroDocumentStore>>,
    sink: &Arc<MemorySink>,
    bus: &Arc<ConditionBus>,
    dir: &std::path::Path,
) -> Arc<LoroProjection> {
    Arc::new(LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        dir.join("sc.sync"),
        holon_api::block_read_model::BlockReadModel::new(),
        bus.clone(),
    ))
}

/// An observation with no diagnostics: this harness's sink holds only rows the
/// projection wrote, and the comparison is strict, so nothing here can mask
/// the assertions below.
fn observation(
    model: Vec<Vec<String>>,
    live: Vec<Vec<String>>,
    sql: Vec<Vec<String>>,
) -> holon_pbt_core::capabilities::ReadModelObservation {
    holon_pbt_core::capabilities::ReadModelObservation {
        model,
        live,
        sql,
        sql_only_diagnostics: Vec::new(),
    }
}

fn model_ids(projection: &LoroProjection) -> Vec<String> {
    use holon_api::block_read_model::BlockDeltaSource;
    let mut ids: Vec<String> = projection
        .read_model()
        .as_ref()
        .blocks()
        .read()
        .keys()
        .cloned()
        .collect();
    ids.sort();
    ids
}

async fn commit_and_stage(
    doc_store: &Arc<RwLock<LoroDocumentStore>>,
    projection: &LoroProjection,
    stable_id: &str,
    content: &str,
) -> Result<()> {
    let tid = insert_root_block(doc_store, stable_id, content).await?;
    projection
        .pending()
        .lock()
        .unwrap()
        .push(PendingChange::Create {
            parent: TreeParentId::Root,
            target: tid,
        });
    Ok(())
}

/// Chosen over a per-key generation counter a consumer asserts monotone.
///
/// A generation token would be new production state carrying a guarantee the
/// code already provides structurally — and it would pin the token, not the
/// property. What a consumer actually depends on is that it never observes a
/// newer publish before an older one, which holds iff publishes are serialized
/// with the commits, i.e. iff two `project()` passes never overlap.
///
/// So the assertion is an OVERLAP DETECTOR, not a timing comparison: the sink's
/// apply hook (which runs inside `project()`, after the commit-point publish)
/// flips an `inside` flag and records whether it was already set. The first
/// apply then BLOCKS on a channel until the test releases it, so a second pass
/// that is not excluded has a real window to overlap in. Nothing in the verdict
/// depends on how long that window is.
///
/// Red-for-the-right-reason: deleting the `let _guard = self.project_lock…`
/// line at `loro_sync_controller.rs:916` makes the second pass enter the sink
/// while the first is held, setting `overlap`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_projection_passes_never_publish_concurrently() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    doc_store.read().await.get_doc(DocScope::Global).await?;
    let sink = Arc::new(MemorySink::new());
    let bus = Arc::new(ConditionBus::new());
    let projection = projection(&doc_store, &sink, &bus, tempdir.path());
    projection.arm();

    // Seed `live` off an empty authority so the passes below are incremental.
    projection.project().await.expect("seed pass");

    let inside = Arc::new(AtomicBool::new(false));
    let overlap = Arc::new(AtomicBool::new(false));
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<Vec<String>>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = StdMutex::new(release_rx);
    {
        let projection = projection.clone();
        let inside = inside.clone();
        let overlap = overlap.clone();
        let hook_calls = hook_calls.clone();
        sink.set_apply_hook(Arc::new(move || {
            // A panic here would surface as an apply error, not a test
            // failure, so the overlap is RECORDED and asserted by the test.
            if inside.swap(true, Ordering::SeqCst) {
                overlap.store(true, Ordering::SeqCst);
            }
            let first = hook_calls.fetch_add(1, Ordering::SeqCst) == 0;
            entered_tx
                .send(model_ids(&projection))
                .expect("the test outlives every apply");
            if first {
                release_rx.lock().unwrap().recv().ok();
            }
            inside.store(false, Ordering::SeqCst);
        }));
    }

    commit_and_stage(&doc_store, &projection, "A-id", "block A").await?;
    let pass_a = {
        let projection = projection.clone();
        tokio::spawn(async move { projection.project().await.map(|_| ()) })
    };

    // Blocking, not timed: pass A is now inside its critical section, holding
    // the apply open, having already published its own delta.
    let a_entry = entered_rx.recv().expect("pass A must reach the sink");
    assert_eq!(
        a_entry,
        vec!["block:A-id".to_string()],
        "the held pass published its own delta before writing the sink"
    );

    commit_and_stage(&doc_store, &projection, "B-id", "block B").await?;
    let pass_b = {
        let projection = projection.clone();
        tokio::spawn(async move { projection.project().await.map(|_| ()) })
    };

    // If B is excluded it never arrives; if it is not, it arrives and `overlap`
    // is already set. Either way the wait only bounds runtime.
    let b_entered_while_a_held = entered_rx.recv_timeout(OVERLAP_WINDOW).ok();
    release_tx.send(()).expect("release pass A");
    pass_a.await??;
    pass_b.await??;

    assert!(
        !overlap.load(Ordering::SeqCst),
        "two projection passes were inside the sink apply at once, so their publishes are not \
         serialized with their commits"
    );
    assert!(
        b_entered_while_a_held.is_none(),
        "pass B reached the sink while pass A held its critical section open (it saw {:?}), so \
         nothing orders the two publishes",
        b_entered_while_a_held
    );

    // Non-vacuity: both passes really ran and both blocks landed.
    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        2,
        "both passes must have applied — otherwise the exclusion above is untested"
    );
    assert_eq!(
        model_ids(&projection),
        vec!["block:A-id".to_string(), "block:B-id".to_string()],
        "both passes must have landed by the end"
    );
    Ok(())
}

/// A failed sink write leaves the read model ahead of the index. The window is
/// legitimate — Loro is the authority — so the contract is that it is
/// DISCLOSED and REPORTED, never hidden.
///
/// Red-for-the-right-reason: deleting the `self.degraded.emit(…)` call in the
/// failed-apply arm (`loro_sync_controller.rs:1182-1194`) fails the first
/// assertion; making the read model wait for the sink write instead of
/// publishing at the commit fails the second.
#[tokio::test]
async fn a_failed_sink_write_discloses_the_read_model_ahead_of_the_index() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    doc_store.read().await.get_doc(DocScope::Global).await?;
    let sink = Arc::new(MemorySink::new());
    let bus = Arc::new(ConditionBus::new());
    let projection = projection(&doc_store, &sink, &bus, tempdir.path());
    projection.arm();

    insert_root_block(&doc_store, "X-id", "block X").await?;
    projection.project().await.expect("seed pass");
    assert!(
        bus.subscribe().current.is_empty(),
        "a healthy pass must disclose nothing"
    );

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
    projection
        .project()
        .await
        .expect_err("a failed sink write surfaces as Err");

    // 1. The condition surfaces, naming the window rather than a generic fault.
    let current = bus.subscribe().current;
    assert_eq!(
        current.len(),
        1,
        "the failed apply must raise exactly one condition; got {current:?}"
    );
    match &current[0].reason {
        ConditionKind::SqlProjectionFailed(why) => assert!(
            why.contains("index-backed views"),
            "the disclosure must name the index-lag consequence, not just a write failure: \
             {why}"
        ),
        other => panic!("expected SqlProjectionFailed, got {other:?}"),
    }

    // 2. The read model stays at the AUTHORITY's value, and `live` does not
    //    advance. Rolling the UI back to the stale index is the outcome this
    //    forbids.
    assert_eq!(
        model_ids(&projection),
        vec!["block:C-id".to_string(), "block:X-id".to_string()],
        "the read model must hold what Loro holds, failed index write or not"
    );
    assert!(
        !projection.live_snapshot().contains_key("block:C-id"),
        "`live` must not advance past the last committed sink write"
    );
    assert_eq!(sink.row_ids(), vec!["block:X-id".to_string()]);

    // 3. The keystone oracle REPORTS the window. Rows are reduced to their ids —
    //    the oracle keys on `first()` and compares whole rows, so ids alone are a
    //    faithful reduction and the assertion cannot pass on a field the failure
    //    did not touch.
    let rows = |ids: Vec<String>| -> Vec<Vec<String>> {
        let mut v: Vec<Vec<String>> = ids.into_iter().map(|id| vec![id]).collect();
        v.sort();
        v
    };
    let mut live_ids: Vec<String> = projection.live_snapshot().keys().cloned().collect();
    live_ids.sort();
    let report = InvViewModelMatchesStore::divergence(&observation(
        rows(model_ids(&projection)),
        rows(live_ids),
        rows(sink.row_ids()),
    ))
    .expect("the oracle must REPORT the read-model-ahead window, not hide it");
    assert!(
        report.contains("block:C-id"),
        "the report must name the row the index is missing: {report}"
    );

    // 4. …and it stops reporting once the reseed repairs the index.
    sink.set_fail(false);
    projection.project().await.expect("recovery pass");
    let mut live_ids: Vec<String> = projection.live_snapshot().keys().cloned().collect();
    live_ids.sort();
    assert!(
        InvViewModelMatchesStore::divergence(&observation(
            rows(model_ids(&projection)),
            rows(live_ids),
            rows(sink.row_ids()),
        ))
        .is_none(),
        "after recovery the three must agree again"
    );
    Ok(())
}

/// The same contract on the OTHER leg. `project()` publishes the commit-point
/// delta before it branches, and two of the three branches route to the full
/// walk instead of writing the sink incrementally. A failure in THAT walk's
/// `emit_ops` leaves the read model just as far ahead of the index, so it must
/// disclose too — through the one definition both legs call
/// (`LoroProjection::disclose_read_model_ahead_of_index`).
///
/// Reached by failing the sink on a pass that is NOT seeded, which is the
/// state a cold boot and every post-failure retry are in.
///
/// Red-for-the-right-reason: before the shared helper existed, disclosure sat
/// inline in the incremental arm only and this leg raised nothing.
#[tokio::test]
async fn a_failed_full_walk_discloses_the_same_window_as_the_incremental_leg() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    doc_store.read().await.get_doc(DocScope::Global).await?;
    let sink = Arc::new(MemorySink::new());
    let bus = Arc::new(ConditionBus::new());
    let projection = projection(&doc_store, &sink, &bus, tempdir.path());
    projection.arm();

    // Never seeded, so this pass takes the FULL walk (cold boot), not the
    // incremental fast path — asserted below via the sink's read counter,
    // which only `read_sql_snapshot` touches.
    assert!(!projection.is_seeded());
    insert_root_block(&doc_store, "F-id", "block F").await?;
    sink.set_fail(true);
    projection
        .project()
        .await
        .expect_err("a failed full-walk sink write surfaces as Err");
    assert!(
        sink.read_calls() > 0,
        "this pass must have taken the full walk; a 0 sink-read count means it went incremental \
         and the test is measuring the other leg"
    );

    let current = bus.subscribe().current;
    assert_eq!(
        current.len(),
        1,
        "the failed full walk must raise exactly one condition; got {current:?}"
    );
    match &current[0].reason {
        ConditionKind::SqlProjectionFailed(why) => assert!(
            why.contains("index-backed views"),
            "the full-walk leg must disclose the SAME index-lag consequence as the \
             incremental leg, not a generic write failure: {why}"
        ),
        other => panic!("expected SqlProjectionFailed, got {other:?}"),
    }
    Ok(())
}
