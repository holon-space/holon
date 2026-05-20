//! `inv-sql-budget` (`otel-testing` only).
//!
//! Per-transition SQL/wall/memory budget. Unlike the other bodies, the
//! budget computation is irreducibly tied to integration-tests-local types
//! (`transition_budgets`, the SUT's `last_transition`, the OTel
//! `span_collector`) and the concrete [`ReferenceState`] (the transition's
//! `expected_sql` inspects it), so [`SutSpanMetrics`] is an integration-tests
//! trait rather than a `holon-pbt-core` cap, and the body binds the concrete
//! `Invariant<ReferenceState, S>` (the runner already uses `ReferenceState`).
//!
//! The cap emits all telemetry side-effects (summary line, N+1 list,
//! flamegraph, memory diagnosis) and returns only the pass/fail decision:
//! budget Error violations fail the run *only* when enforcement is opted in
//! via `HOLON_PERF_BUDGET` (default off → logged as warnings). Everything is
//! `#[cfg(feature = "otel-testing")]`; the `pbt` feature always enables it, so
//! the wide PBT always runs it while non-otel slices simply omit it.

use holon_pbt_core::invariant::InvariantId;

pub struct InvSqlBudget;

impl InvSqlBudget {
    pub const ID: InvariantId = InvariantId("inv-sql-budget");
}

#[cfg(feature = "otel-testing")]
pub use otel::{SqlBudgetReport, SutSpanMetrics};

#[cfg(feature = "otel-testing")]
mod otel {
    use super::InvSqlBudget;
    use crate::pbt::reference_state::ReferenceState;
    use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

    /// Pass/fail outcome of the per-transition budget check. All telemetry
    /// side-effects (summary, N+1, flamegraph, memory diagnosis) happen
    /// inside [`SutSpanMetrics::sql_budget_report`]; this carries only what
    /// the invariant decides on.
    pub struct SqlBudgetReport {
        /// `HOLON_PERF_BUDGET` enforcement is on (else Error violations are
        /// logged, not failed).
        pub enforce: bool,
        /// Budget Error violation messages (reads/writes/ddl/wall/rss over budget).
        pub errors: Vec<String>,
    }

    /// SUT-side OTel span/budget surface. Integration-tests-local (not a
    /// `holon-pbt-core` cap) because the budget is computed from
    /// `transition_budgets` + the SUT's `last_transition` + the concrete
    /// `ReferenceState`.
    #[allow(async_fn_in_trait)]
    pub trait SutSpanMetrics {
        /// Snapshot span metrics for the last transition, emit the telemetry
        /// side-effects, and return the budget pass/fail decision.
        async fn sql_budget_report(&self, ref_state: &ReferenceState) -> SqlBudgetReport;
    }

    #[allow(async_fn_in_trait)]
    impl<S> Invariant<ReferenceState, S> for InvSqlBudget
    where
        S: SutSpanMetrics,
    {
        fn id(&self) -> InvariantId {
            Self::ID
        }

        async fn check(&self, ref_: &ReferenceState, sut: &S) -> InvariantResult {
            let report = sut.sql_budget_report(ref_).await;
            match (report.enforce, report.errors.is_empty()) {
                (true, false) => InvariantResult::Fail(format!(
                    "[inv-sql-budget] {} budget violation(s):\n  {}",
                    report.errors.len(),
                    report.errors.join("\n  "),
                )),
                (false, false) => InvariantResult::Skipped(format!(
                    "HOLON_PERF_BUDGET off — {} unenforced violation(s): {}",
                    report.errors.len(),
                    report.errors.join("; "),
                )),
                (_, true) => InvariantResult::Ok,
            }
        }
    }
}
