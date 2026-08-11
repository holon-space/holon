//! Composed-slice surface for `inv-complexity-class-trend` — the within-run
//! counter-TREND oracle.
//!
//! @pbt oracle budget — per-transition-KIND trend over deduplicated SQL
//!   read/write counters, fitted first-third vs last-third of a kind's
//!   occurrences, against the complexity class its own `expected_sql` formula
//!   declares
//! @pbt covers complexity-class regression — a transition whose budget formula
//!   is a state-blind constant (an O(1) claim) whose counters nevertheless grow
//!   with sequence position / state size
//! @pbt slips-if-removed a hot transition acquires a per-item scan or a
//!   per-execution registration; each individual tick still fits a budget wide
//!   enough to hold at scale, so the app just gets slower the longer it runs
//!   with no failing test (only fires when HOLON_TREND_BUDGET enforces)
//!
//! The collection pipeline is [`crate::pbt::composed::span_metrics`]'s, not a
//! second one: [`ComposedSpanMetrics`] already freezes the per-transition
//! counters at check-start for `inv-sql-budget`, and records one
//! [`crate::pbt::complexity_trend::Sample`] from that same freeze. This module
//! owns only the read cap and the decision.
//!
//! ## Why this is a separate invariant from `inv-sql-budget`
//! The budget scores ONE transition against a formula. This scores a KIND
//! across the run. A budget wide enough to hold at 200 blocks has room for
//! linear growth from 3 blocks to 200 hidden inside it, and that growth is
//! exactly "this operation gets more expensive the longer the program runs".
//!
//! ## Reporting diverges from `inv-sql-budget` deliberately
//! A budget breach shrinks to a minimal counterexample usefully. A trend does
//! NOT — shrinking destroys the accumulation that IS the evidence. So the
//! failure carries the fitted trend plus the complete per-kind counter series,
//! and the same evidence goes to stderr the moment it is first observed, before
//! any shrinking can rewrite the case.
//!
//! ## Armed separately from `HOLON_PERF_BUDGET`
//! `HOLON_TREND_BUDGET=1` enforces; unset it observes and DISCLOSES (a
//! `Skipped` carrying the full evidence). The trend oracle has not soaked yet,
//! and a real accumulation defect is currently open (task #15, `set_field`
//! corpus scaling) — arming it on arrival would red the keystone gate for a
//! defect another lane owns, which is how gates get disarmed instead of fixed.
//!
//! [`ComposedSpanMetrics`]: crate::pbt::composed::span_metrics::ComposedSpanMetrics

use holon_pbt_core::composition::CapMap;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use crate::pbt::complexity_trend::TrendReport;

/// Read cap: the fitted trend over every transition kind seen so far in this
/// case. Ref-less for the same reason [`ComposedBudget`] is — the host already
/// holds everything the fit needs.
///
/// [`ComposedBudget`]: crate::pbt::composed::span_metrics::ComposedBudget
#[holon_macros::capmap_adapter]
pub trait ComposedTrend {
    fn trend_report(&self) -> TrendReport;
}

/// `HOLON_TREND_BUDGET` — see the module docs for why this is not
/// `HOLON_PERF_BUDGET`. Sampled by the HOST (which stamps
/// [`TrendReport::enforce`]), never inside the invariant body: the teeth then
/// arm the gate by constructing a report, with no environment mutation to race
/// a sibling test.
pub fn trend_budget_enforced() -> bool {
    std::env::var("HOLON_TREND_BUDGET")
        .map(|v| v != "0")
        .unwrap_or(false)
}

pub struct InvComplexityTrend;

impl InvComplexityTrend {
    pub const ID: InvariantId = InvariantId("inv-complexity-class-trend");
}

#[allow(async_fn_in_trait)]
impl Invariant<CapMap, CapMap> for InvComplexityTrend {
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &CapMap, sut: &CapMap) -> InvariantResult {
        let report = sut.trend_report();
        if report.violations.is_empty() {
            return InvariantResult::Ok;
        }
        // Printed here, not only returned: a returned message survives only as
        // long as this case does, and proptest is about to shrink the sequence
        // that produced the accumulation.
        eprintln!(
            "[inv-complexity-class-trend] {} declared-O(1) transition(s) grew within the \
             run:\n{}",
            report.violations.len(),
            report.evidence(),
        );
        let summary = format!(
            "[inv-complexity-class-trend] {} declared-O(1) transition(s) grew within the \
             run — a trend is evidence only as a SERIES, so the full counter table is \
             below and in the run log:\n{}",
            report.violations.len(),
            report.evidence(),
        );
        if report.enforce {
            InvariantResult::Fail(summary)
        } else {
            InvariantResult::Skipped(format!("HOLON_TREND_BUDGET off — {summary}"))
        }
    }
}
