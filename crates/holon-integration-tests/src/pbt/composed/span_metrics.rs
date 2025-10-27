//! Composed-slice coverage for `inv-sql-budget` (per-transition SQL/wall/RSS
//! budget).
//!
//! ## Why this is separate from the native [`SutSpanMetrics`] cap
//! The native body binds `Invariant<ReferenceState, S>` and its cap takes a
//! concrete `&ReferenceState`. A composed [`BridgedInvariant`] is
//! `Invariant<CapMap, CapMap>` — its ref side is a `CapMap`, which can't hand
//! back a `ReferenceState`. So the composed path can't reuse that body. Instead
//! this module owns a ref-LESS budget cap: the harness hands the host the
//! transition ([`SutMetricsLifecycle::note_transition_start`]) and the
//! post-transition oracle ([`SutMetricsLifecycle::freeze_for_check`]), and the
//! read cap ([`ComposedBudget::budget_report`]) returns the decision computed
//! from that stored state via the SAME [`MetricsSut`] machinery `E2ESut` uses.
//!
//! `CapMap` is single-threaded (`!Send`/`!Sync`, accessed under the harness's
//! `block_on`), so plain `RefCell` interior mutability suffices.
//!
//! [`SutSpanMetrics`]: crate::pbt::invariants::bodies::sql_budget::SutSpanMetrics
//! [`BridgedInvariant`]: holon_pbt_core::composition::BridgedInvariant

use std::cell::RefCell;

use holon_pbt_core::composition::CapMap;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use crate::pbt::invariants::bodies::sql_budget::InvSqlBudget;
use crate::pbt::invariants::bodies::sql_budget::SqlBudgetReport;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::sut_metrics::MetricsSut;
use crate::pbt::transitions::E2ETransition;

/// Read cap: the per-transition budget decision. Ref-less — the host already
/// holds the post-transition oracle (captured at `freeze_for_check`), so the
/// composed invariant body doesn't need to thread a `ReferenceState` it can't
/// reach through the ref `CapMap`.
#[holon_macros::capmap_adapter]
pub trait ComposedBudget {
    fn budget_report(&self) -> SqlBudgetReport;
}

/// Lifecycle the composed [`ComposedSut`] harness drives so the budget measures
/// exactly ONE transition. `E2ESut` drives the identical lifecycle natively
/// (its proptest apply hook + `freeze_at_check_start`); this is the composed
/// analog.
///
/// [`ComposedSut`]: crate::pbt::composed::harness::ComposedSut
#[holon_macros::capmap_adapter]
pub trait SutMetricsLifecycle {
    /// Reset the span collector and record the wall/RSS baseline for the
    /// transition about to run. Called at the start of `apply_transition`.
    fn note_transition_start(&self, transition: &E2ETransition);
    /// Freeze span/wall/RSS at invariant-check start (the budget window is the
    /// transition apply + settle, not the invariant bodies' own reads) and
    /// capture the post-transition oracle the budget's `expected_sql`
    /// inspects.
    fn freeze_for_check(&self, ref_state: &ReferenceState);
}

/// Composed span-metrics provider, hosting the same [`MetricsSut`] `E2ESut`
/// uses.
pub struct ComposedSpanMetrics {
    metrics: RefCell<MetricsSut>,
    last_transition: RefCell<Option<E2ETransition>>,
    frozen_ref: RefCell<Option<ReferenceState>>,
}

impl ComposedSpanMetrics {
    pub fn new() -> Self {
        Self {
            metrics: RefCell::new(MetricsSut::new()),
            last_transition: RefCell::new(None),
            frozen_ref: RefCell::new(None),
        }
    }
}

impl Default for ComposedSpanMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl SutMetricsLifecycle for ComposedSpanMetrics {
    fn note_transition_start(&self, transition: &E2ETransition) {
        *self.last_transition.borrow_mut() = Some(transition.clone());
        self.metrics.borrow_mut().on_transition_start();
    }

    fn freeze_for_check(&self, ref_state: &ReferenceState) {
        self.metrics.borrow().freeze_at_check_start();
        *self.frozen_ref.borrow_mut() = Some(ref_state.clone());
    }
}

impl ComposedBudget for ComposedSpanMetrics {
    fn budget_report(&self) -> SqlBudgetReport {
        // The harness runs an initial `check_invariants` BEFORE the first transition
        // (proptest-state-machine semantics), so on that tick no transition has been
        // noted — there is no per-transition cost to budget, so the report is trivially
        // clean. Every subsequent tick has a noted transition + frozen snapshot.
        let Some(last) = self.last_transition.borrow().clone() else {
            return SqlBudgetReport {
                enforce: false,
                errors: Vec::new(),
            };
        };
        let ref_state = self.frozen_ref.borrow();
        let ref_state = ref_state
            .as_ref()
            .expect("freeze_for_check must run before budget_report once a transition is noted");
        self.metrics.borrow().sql_budget_report(&last, ref_state)
    }
}

/// Composed twin of [`InvSqlBudget`] (same id), dispatched over a `CapMap`:
/// reads the budget decision from the [`ComposedBudget`] cap and applies the
/// identical enforce/skip/ok decision. The budget is OFF by default
/// (`HOLON_PERF_BUDGET`), so a clean run is `Ok`; unenforced violations are
/// `Skipped`; only enforced violations `Fail`.
pub struct InvComposedBudget;

#[allow(async_fn_in_trait)]
impl Invariant<CapMap, CapMap> for InvComposedBudget {
    fn id(&self) -> InvariantId {
        InvSqlBudget::ID
    }

    async fn check(&self, _: &CapMap, sut: &CapMap) -> InvariantResult {
        let report = sut.budget_report();
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
