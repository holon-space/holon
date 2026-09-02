//! Shared 3-projection convergence wait — the signal-level core behind both the
//! composed per-transition settle (`wide_e2e::converge_projections`) and the
//! `HeadlessFrontendComponent` boot settle. One implementation so the two
//! settles can never drift apart on which projections count as "drained".

use std::sync::Arc;
use std::time::Duration;

use holon::api::BackendEngine;
use holon_frontend::reactive::ReactiveEngine;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroSyncControllerHandle;
use holon_orgmode::OrgSyncIdleSignal;

use crate::test_environment::pbt_quiet_floor;

/// Wait — capped at `budget` — for every projection the invariants read to
/// reach quiescence. Absent signals make the corresponding stage a no-op
/// (those stores are synchronous or not wired, so there is nothing to wait
/// for):
///
/// 1. **Turso CDC** — `cdc_emitted_watermark` stable for one quiet floor (the
///    `block_raw` matview the block invariants query is CDC-fed).
/// 2. **Loro** — the sync controller's `last_synced_frontiers()` catches up to
///    the authority doc's `oplog_frontiers()` (a peer/merge write projects
///    asynchronously).
/// 3. **block-matview mirror** — the `BlockFeed`'s `consumed_seq` stops
///    advancing (the org write-back reads that mirror, so its ARRIVAL, not
///    CDC's emission, is when the write-back's work becomes reachable).
/// 4. **org** — the file-sync controller's `OrgSyncIdleSignal` goes quiescent
///    (the org re-render `inv-blocks-match-ref/org` reads has drained).
///
/// A CDC-only signal (the reverted lever 2) under-settled — Loro/org lagged
/// and the block/org invariants diverged; this covers all three.
///
/// Unlike the earlier SEQUENTIAL version (drain CDC, THEN wait for Loro, THEN
/// org), this polls all three to a single COMBINED fixed point in Loro -> CDC
/// -> org order: a not-yet-caught-up Loro (whose projection still owes
/// `block_raw` its reordered `sort_key`) counts as activity and resets the
/// quiet window, so CDC/org only read "quiet" once that write has landed and
/// re-drained. The old order let stage-1 CDC drain before stage-2 ever wrote
/// the sort_key, and every stage broke on `deadline` returning silently — an
/// unconverged SUT looked settled, the direct cause of the org-render
/// sibling-order flake. Returns `true` iff the fixed point was reached within
/// `budget`; `false` = gave up, and the caller MUST decide (the composed settle
/// fails loud on `false`).
pub(crate) async fn converge_signals(
    engine: Option<&Arc<BackendEngine>>,
    sync: Option<Arc<LoroSyncControllerHandle>>,
    store: Option<LoroDocumentStore>,
    org_idle: Option<Arc<OrgSyncIdleSignal>>,
    block_feed: Option<Arc<holon_api::live_data::LiveData<holon_api::Block>>>,
    reactive: Option<&Arc<ReactiveEngine>>,
    budget: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    let quiet = pbt_quiet_floor();
    let poll = Duration::from_millis(2);

    // Baselines for the change-detected signals (CDC watermark, org tick,
    // reactive consumer apply-epoch).
    let mut last_cdc = engine.map(|e| e.db_handle().cdc_emitted_watermark());
    let mut last_tick = org_idle.as_ref().map(|s| s.current_tick());
    let mut last_feed_seq = block_feed.as_ref().map(|f| f.consumed_seq());
    let mut last_epoch = reactive.map(|r| r.apply_epoch());
    // The last instant ANY signal showed activity OR Loro was not yet caught up.
    // Convergence = all three quiet AND Loro caught up, held for one `quiet` floor.
    let mut last_activity = tokio::time::Instant::now();

    loop {
        tokio::time::sleep(poll).await;
        let mut active = false;

        // Loro FIRST. This is the projection that writes the reordered `sort_key`
        // into `block_raw` (which then fires CDC). Until `last_synced` has caught
        // the authority `oplog_frontiers`, the CDC/org checks below are premature,
        // so treat not-caught-up as activity: it resets the quiet window. That is
        // what turns the three independent signals into one COMBINED fixed point
        // in Loro -> CDC -> org order, instead of the old sequential race where
        // stage-1 CDC drained before stage-2 ever wrote the sort_key. (An absent
        // sync/store = a Loro-off draw = nothing to wait for.)
        if let (Some(sync), Some(store)) = (&sync, &store) {
            let current = store
                .get_doc(DocScope::Global)
                .await
                .expect("converge_signals: get_doc(Global) failed")
                .doc()
                .oplog_frontiers();
            if sync.last_synced_frontiers() != current {
                active = true;
            }
        }

        // Turso CDC watermark. The projection's SQL write bumps this, so it only
        // reads stable AFTER Loro caught up and that write actually landed.
        if let Some(engine) = engine {
            let now_cdc = engine.db_handle().cdc_emitted_watermark();
            if last_cdc != Some(now_cdc) {
                last_cdc = Some(now_cdc);
                active = true;
            }
        }

        // Block-matview mirror delivery. The org write-back is driven off this
        // mirror, and CDC EMISSION (above) is not its arrival: the broadcast hop
        // between the commit that bumped the watermark and the mirror applying
        // the batch is a window every other signal reads as quiet. Counting the
        // mirror's own `consumed_seq` advance as activity restarts the quiet
        // window at ARRIVAL, so the org stage below is judged from there.
        //
        // Change-detected, deliberately NOT `consumed_seq < emitted_watermark`:
        // the emitted watermark counts every commit, including ones whose CDC
        // touched no `block` row, and this mirror is delivered only its own
        // matview's batches — so it trails the global counter permanently and a
        // catch-up test would never converge.
        if let Some(feed) = &block_feed {
            let now_seq = feed.consumed_seq();
            if last_feed_seq != Some(now_seq) {
                last_feed_seq = Some(now_seq);
                active = true;
            }
        }

        // org re-render loop: `current_tick` bumps after every processed change.
        //
        // The tick alone is not enough. Between the mirror above delivering a
        // batch and this loop being handed anything sits the write-back's
        // `home_by` fold, which is one authority read per block: tens of
        // milliseconds during which the mirror is quiet and the tick has not
        // moved, so a quiet floor expires INSIDE the fold. `home_by`'s own
        // in-flight flag is the only signal that names that window, so it
        // counts as activity exactly like a not-caught-up Loro does.
        if let Some(idle) = &org_idle {
            let now_tick = idle.current_tick();
            if last_tick != Some(now_tick) {
                last_tick = Some(now_tick);
                active = true;
            }
            // Two windows the tick cannot show, because it moves only when a
            // pass COMPLETES: the `home_by` fold that has not handed the loop
            // anything yet, and a pass already running whose renders outlast a
            // quiet floor. Both count as activity, exactly as a not-caught-up
            // Loro does.
            if idle.writeback_fold_in_flight() || idle.pass_in_flight() {
                active = true;
            }
        }

        // Reactive watch CONSUMER drain: the async task that applies emitted CDC
        // into `ReactiveRenderedRows` bumps `apply_epoch` per change. CDC
        // emission going quiet (above) does NOT imply the consumer has APPLIED
        // it — the ViewModel `snapshot()` the invariants read lags until it
        // does. Treating a still-rising epoch as activity holds the quiet window
        // open until the consumer catches the last delta, closing the
        // `inv-displayed-text/viewmodel` split-block staleness race. (Absent
        // reactive = a Loro-only / no-frontend draw = nothing to wait for.)
        if let Some(reactive) = reactive {
            let now_epoch = reactive.apply_epoch();
            if last_epoch != Some(now_epoch) {
                last_epoch = Some(now_epoch);
                active = true;
            }
        }

        let now = tokio::time::Instant::now();
        if active {
            last_activity = now;
        } else if now.duration_since(last_activity) >= quiet {
            // All three signals quiet AND Loro caught up for a full quiet floor.
            return true;
        }

        if now >= deadline {
            // FAIL to converge. The caller decides whether that is fatal — the
            // per-transition composed settle fails loud (reading invariants on an
            // unconverged SUT is a race, not a flake to swallow); the one-time
            // boot settle tolerates it (signals resolve on a spawned task).
            return false;
        }
    }
}
