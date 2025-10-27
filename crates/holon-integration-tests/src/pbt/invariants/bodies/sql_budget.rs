//! `inv-sql-budget` (`otel-testing` only) — per-transition SQL/wall/memory budget.
//!
//! ## E3: relocated off `E2ESut` onto the composed slice
//! The budget was historically dispatched natively over `E2ESut` via a
//! `SutSpanMetrics` cap binding `Invariant<ReferenceState, S>`. That body could
//! not be folded into the composed catalog (a `BridgedInvariant` is
//! `Invariant<CapMap, CapMap>`, and the cap took a concrete `&ReferenceState` the
//! composed ref `CapMap` can't hand back), so the composed slice covers
//! `inv-sql-budget` through its OWN ref-less budget cap +
//! `Invariant<CapMap, CapMap>` body — see
//! [`crate::pbt::composed::span_metrics`]. With that in place, the native
//! `E2ESut` impl + dispatch were deleted (E3); this module now keeps only the two
//! pieces the composed path reuses: the canonical [`InvSqlBudget::ID`] and the
//! [`SqlBudgetReport`] that [`crate::pbt::sut_metrics::MetricsSut`] produces.

use holon_pbt_core::invariant::InvariantId;

/// Canonical id home for `inv-sql-budget`. No longer an [`Invariant`] impl — the
/// composed [`crate::pbt::composed::span_metrics::InvComposedBudget`] is the sole
/// dispatched body and reuses [`Self::ID`].
///
/// [`Invariant`]: holon_pbt_core::invariant::Invariant
pub struct InvSqlBudget;

impl InvSqlBudget {
    pub const ID: InvariantId = InvariantId("inv-sql-budget");
}

#[cfg(feature = "otel-testing")]
pub use otel::SqlBudgetReport;

#[cfg(feature = "otel-testing")]
mod otel {
    /// Pass/fail outcome of the per-transition budget check. All telemetry
    /// side-effects (summary, N+1, flamegraph, memory diagnosis) happen inside
    /// [`crate::pbt::sut_metrics::MetricsSut::sql_budget_report`]; this carries only
    /// what the invariant decides on. Consumed by the composed
    /// [`crate::pbt::composed::span_metrics::ComposedBudget`] read cap.
    pub struct SqlBudgetReport {
        /// `HOLON_PERF_BUDGET` enforcement is on (else Error violations are
        /// logged, not failed).
        pub enforce: bool,
        /// Budget Error violation messages (reads/writes/ddl/wall/rss over budget).
        pub errors: Vec<String>,
    }
}
