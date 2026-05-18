//! Phase 7 — `inv-watch-rows-match-ref` (DEFERRED).
//!
//! Inline body lives at `sut.rs:4415–4631`.
//!
//! # Why deferred
//!
//! The body iterates `self.ui_model` (private `HashMap<String, LiveData>` keyed
//! by query_id) and for each watch compares CDC-delivered rows against the reference
//! model's `ref_state.query_results(watch_spec)`. Key blockers:
//! - `self.ui_model` is private
//! - `ref_state.active_watches` + `ref_state.query_results(watch_spec)` — no Ref* cap
//! - `watch_spec.query.to_block_raw_sql()` — internal query-model type
//! - CDC field-level comparison (normalize_content, per-field block_raw truth-check)
//!
//! Also tagged `RunMode::Warn`; the WARN/skip path needs `SutCdc::cdc_in_flight`
//! which is itself unimplemented on `E2ESut`. Wire when: `SutSqlProjection` grows
//! `watch_field_value(query_id, block_id, field)`, a Ref* cap covers
//! `active_watches` / `query_results`, and `SutCdc::cdc_in_flight` is wired.

pub struct InvWatchRowsMatchRef;

impl InvWatchRowsMatchRef {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-watch-rows-match-ref");
}

// Status: ref-side partially unblocked (RefWatches::active_watch_ids added).
// SUT-side: ui_model is pub on TestContext (E2ESut derefs); SutCdc::cdc_in_flight
// already wired. Remaining blocker: ref_state.query_results(watch_spec) +
// CdcAccumulator per-field truth-check require new RefWatchQueries cap and
// SutSqlProjection::watch_field_value. Not promoteable without those caps.
