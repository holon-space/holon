//! Phase 7 — `inv-sql-budget` (DEFERRED).
//!
//! Inline body lives at `sut.rs:5889–6021` (behind `#[cfg(feature = "otel-testing")]`).
//!
//! # Why deferred
//!
//! The body requires:
//! - `self.span_collector` (private) — an OTel span accumulator that records
//!   per-transition SQL counts, durations, and flamegraph data
//! - `self.last_transition` (private) — the most recent transition variant,
//!   used by `transition_budgets::transition_key` and `expected_sql`
//! - `self.last_transition_start` (private) — wall-time start for the transition
//! - `self.rss_before`, `self.rss_baseline` (private) — RSS memory deltas
//! - `super::transition_budgets::*` — internal budget table module
//! - `ref_state.block_state.blocks` — via `expected_sql(transition, ref_state)`
//!
//! The entire sub-system is feature-gated on `otel-testing` which is not in
//! `holon-pbt-core`. There is no `Sut*` capability trait covering OTel span
//! collection today. A new `SutSpanMetrics` trait would be needed. Wire when
//! the OTel telemetry surface is refactored into a testable capability.

pub struct InvSqlBudget;

impl InvSqlBudget {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-sql-budget");
}

// `Invariant` impl intentionally omitted — body migration deferred.
