//! Shared 3-projection convergence wait — the signal-level core behind both the
//! composed per-transition settle (`wide_e2e::converge_projections`) and the
//! `HeadlessFrontendComponent` boot settle. One implementation so the two
//! settles can never drift apart on which projections count as "drained".

use std::sync::Arc;
use std::time::Duration;

use holon::api::BackendEngine;
use holon::sync::{LoroDocumentStore, LoroSyncControllerHandle};
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
/// 3. **org** — the file-sync controller's `OrgSyncIdleSignal` goes quiescent
///    (the org re-render `inv-blocks-match-ref/org` reads has drained).
///
/// A CDC-only signal (the reverted lever 2) under-settled — Loro/org lagged
/// and the block/org invariants diverged; this covers all three. Each stage is
/// bounded by the shared `deadline`, so the whole wait never exceeds `budget`.
pub(crate) async fn converge_signals(
    engine: Option<&Arc<BackendEngine>>,
    sync: Option<Arc<LoroSyncControllerHandle>>,
    store: Option<LoroDocumentStore>,
    org_idle: Option<Arc<OrgSyncIdleSignal>>,
    budget: Duration,
) {
    let deadline = tokio::time::Instant::now() + budget;
    let quiet = pbt_quiet_floor();

    // 1. Turso CDC drain: watermark stable for `quiet`, bounded by `deadline`.
    if let Some(engine) = engine {
        let db = engine.db_handle();
        let mut last = db.cdc_emitted_watermark();
        let mut stable_since = tokio::time::Instant::now();
        loop {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
            let now = db.cdc_emitted_watermark();
            if now == last {
                if stable_since.elapsed() >= quiet {
                    break;
                }
            } else {
                last = now;
                stable_since = tokio::time::Instant::now();
            }
        }
    }

    // 2. Loro sync controller catches up to the authority doc's frontiers.
    if let (Some(sync), Some(store)) = (sync, store) {
        loop {
            let current = store
                .get_global_doc()
                .await
                .expect("converge_signals: get_global_doc failed")
                .doc()
                .oplog_frontiers();
            if sync.last_synced_frontiers() == current {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    // 3. org re-render drain: the file-sync loop idle for `quiet`, bounded by remaining.
    if let Some(idle) = org_idle {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        idle.wait_quiescent(quiet, remaining).await;
    }
}
