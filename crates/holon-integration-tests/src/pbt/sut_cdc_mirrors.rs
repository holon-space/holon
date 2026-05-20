//! CDC-driven `LiveData` mirrors component for the PBT SUT.
//!
//! Owns the in-memory `LiveData` mirrors that the invariant bodies read
//! instead of issuing fresh SQL on every check — the hydrated `block`
//! matview (`LiveData<Block>`) and the `focus_roots` matview
//! (`LiveData<FocusRoot>`). Both are built lazily on first use because they
//! need an async `watch_view` call after the engine has started.
//!
//! `E2ESut` holds one `CdcMirrors` and keeps same-named thin forwarding
//! methods (`live_blocks`, `live_focus_roots`, …) that pass in the started
//! engine, so the invariant-side call sites are unchanged.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use holon::api::BackendEngine;
use holon::storage::BLOCK_READ_TABLE;
use holon::sync::LiveData;
use holon_api::block::Block;

use super::sut_row_parsing::parse_block_row;

/// One row of the `focus_roots` matview. Mirrored into a `LiveData<FocusRoot>`
/// so inv-region-focus-roots-iter/8 can iterate by region in Rust without a per-region SQL query.
#[derive(Clone, Debug)]
pub(super) struct FocusRoot {
    pub(super) region: String,
    pub(super) root_id: String,
}

/// CDC-driven `LiveData` snapshots used by the invariant bodies. Initialised
/// lazily (the cells start `None`) because the underlying `watch_view` needs a
/// started engine; `RefCell` so `&self` invariant methods can populate them.
/// `wait_for_consumers` gates each check on CDC delivery, so reading from these
/// snapshots is delay-free vs the corresponding SQL.
pub(super) struct CdcMirrors {
    live_blocks_cell: RefCell<Option<Arc<LiveData<Block>>>>,
    live_focus_roots_cell: RefCell<Option<Arc<LiveData<FocusRoot>>>>,
}

impl CdcMirrors {
    pub(super) fn new() -> Self {
        Self {
            live_blocks_cell: RefCell::new(None),
            live_focus_roots_cell: RefCell::new(None),
        }
    }

    /// Lazy accessor for the CDC-driven `LiveData<Block>` mirroring the `block`
    /// matview. Built on first use because we need an async `watch_view` call and
    /// the SUT struct can't carry a started engine at construction time. The
    /// matview hydrates `tags` (and `requires`) from the junction tables, so
    /// rows are read directly into a fully-populated `Block`.
    pub(super) async fn blocks(&self, engine: &BackendEngine) -> Arc<LiveData<Block>> {
        if let Some(live) = self.live_blocks_cell.borrow().clone() {
            return live;
        }
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id, properties, tags, requires \
             FROM {BLOCK_READ_TABLE}"
        );
        let watch = engine
            .watch_view(&sql)
            .await
            .expect("watch_view(block) failed");
        let live = LiveData::new(
            watch.initial_rows,
            |row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("block row missing 'id'"))
            },
            |row| {
                parse_block_row(row)
                    .ok_or_else(|| anyhow::anyhow!("parse_block_row returned None for row {row:?}"))
            },
        );
        live.subscribe("block", watch.stream);
        *self.live_blocks_cell.borrow_mut() = Some(Arc::clone(&live));
        live
    }

    /// Lazy accessor for the CDC-driven `LiveData<FocusRoot>` mirroring the
    /// `focus_roots` matview. Keyed by `"{region}\u{1F}{root_id}"` since one
    /// region can have multiple root rows (one per child of the nav target).
    pub(super) async fn focus_roots(&self, engine: &BackendEngine) -> Arc<LiveData<FocusRoot>> {
        if let Some(live) = self.live_focus_roots_cell.borrow().clone() {
            return live;
        }
        // `focus_roots` matview filters `block_id IS NOT NULL` at projection
        // time as of nightscape@holon `aff40a84` (the IVM compound IS NOT NULL
        // fix). Chained-matview CDC propagation is 1:1 with no spurious
        // events for filtered rows (verified by
        // `crates/holon/examples/turso_ivm_chained_matview_null_cdc.rs`).
        // No watcher-level filter needed.
        let sql = "SELECT region, root_id FROM focus_roots";
        let watch = engine
            .watch_view(sql)
            .await
            .expect("watch_view(focus_roots) failed");
        let live = LiveData::new(
            watch.initial_rows,
            |row| {
                let region = row
                    .get("region")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?;
                let root_id = row
                    .get("root_id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?;
                Ok(format!("{region}\u{1F}{root_id}"))
            },
            |row| {
                Ok(FocusRoot {
                    region: row
                        .get("region")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?,
                    root_id: row
                        .get("root_id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?,
                })
            },
        );
        live.subscribe("focus_roots", watch.stream);
        *self.live_focus_roots_cell.borrow_mut() = Some(Arc::clone(&live));
        live
    }

    /// Drain both mirrors to quiescence (no new batch for `quiet_for`). Only
    /// touches mirrors that have already been built; the caller gates this on
    /// `is_running` (pre-startup transitions have no mirrors).
    pub(super) async fn wait_quiescent(&self, timeout: Duration) {
        // Quiescence semantics: each mirror has drained when no new batch
        // has arrived for `quiet_for`. The previous `wait_for_seq(target,
        // timeout)` approach compared per-stream seq against a global
        // `cdc_emitted_watermark`, which structurally always timed out
        // because other matviews emit batches the mirror never sees.
        let quiet_for = crate::test_environment::pbt_quiet_floor();
        // The `block` and `focus_roots` mirrors are independent matviews with
        // no ordering dependency, so drain them CONCURRENTLY. Sequential awaits
        // paid `2 × quiet_for` (~100ms) on every transition's apply-path settle
        // even when both were already idle; joined, a settled drain costs a
        // single `quiet_for` window. (Mirrors the `snapshot()` settle join in
        // `turso_block_query_source.rs`.) Clone the `Arc`s out of the `RefCell`
        // borrows first so no borrow is held across the await.
        let blocks = self.live_blocks_cell.borrow().clone();
        let roots = self.live_focus_roots_cell.borrow().clone();
        match (blocks, roots) {
            (Some(b), Some(r)) => {
                tokio::join!(
                    b.wait_for_quiescent(quiet_for, timeout),
                    r.wait_for_quiescent(quiet_for, timeout),
                );
            }
            (Some(b), None) => b.wait_for_quiescent(quiet_for, timeout).await,
            (None, Some(r)) => r.wait_for_quiescent(quiet_for, timeout).await,
            (None, None) => {}
        }
    }

    /// True if the `live_blocks` mirror has not yet caught up to the last
    /// emitted watermark (`db_handle().cdc_emitted_watermark()`). Returns
    /// `false` when the watermark is 0 or the mirror has not been built yet.
    /// The caller gates on `is_running` before calling (engine must be live).
    pub(super) fn blocks_cdc_stale(&self, engine: &BackendEngine) -> bool {
        let target = engine.db_handle().cdc_emitted_watermark();
        if target == 0 {
            return false;
        }
        let Some(live) = self.live_blocks_cell.borrow().clone() else {
            return false;
        };
        live.consumed_seq() < target
    }
}
