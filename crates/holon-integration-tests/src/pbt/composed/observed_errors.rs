//! `inv-no-observed-errors` — no SWALLOWED error/panic escaped this transition.
//!
//! ## Why this exists (the advice `No id found` escape, 2026-07-09)
//! The advice weaver's suppression watch enriched an id-less junction row and
//! panicked on a background `tokio-rt-worker`. Task isolation SWALLOWED it: the
//! deterministic `advice_step6` stayed green with the panic sitting in its log,
//! and the keystone only felt it indirectly (dropped blocks). The existing
//! `inv-no-errors` watches the frontend publish-error tracker only, so it was
//! blind to a raw panic / `tracing::error!`.
//!
//! This invariant closes that class directly with a PROCESS-WIDE observability
//! surface: [`crate::test_tracing`] captures every ERROR-level tracing EVENT and
//! every PANIC on any thread (spawned workers included) into the global span
//! collector, reset per transition by the same metrics lifecycle. The read cap
//! drains them at check time and the invariant asserts the set is empty.
//!
//! Ref-less (mirrors [`crate::pbt::composed::span_metrics::ComposedBudget`]): the
//! problems live in the global collector, not the reference `CapMap`.

use holon_pbt_core::composition::CapMap;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use crate::test_tracing::SpanCollector;

/// Read cap: the swallowable problems (ERROR events + panics) captured since the
/// last per-transition reset, pre-formatted for the failure message.
#[holon_macros::capmap_adapter]
pub trait ObservedProblems {
    fn observed_problems(&self) -> Vec<String>;
}

/// Provider reading the process-global [`SpanCollector`].
#[derive(Default)]
pub struct ComposedObservedErrors;

impl ComposedObservedErrors {
    pub fn new() -> Self {
        Self
    }
}

impl ObservedProblems for ComposedObservedErrors {
    fn observed_problems(&self) -> Vec<String> {
        SpanCollector::global()
            .captured_problems()
            .iter()
            .map(|p| p.to_string())
            .collect()
    }
}

/// Fails when any ERROR-level tracing event or panic was captured during the
/// transition window — the swallowed-error guard the advice panic slipped past.
pub struct InvNoObservedErrors;

impl InvNoObservedErrors {
    pub const ID: InvariantId = InvariantId("inv-no-observed-errors");
}

#[allow(async_fn_in_trait)]
impl Invariant<CapMap, CapMap> for InvNoObservedErrors {
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &CapMap, sut: &CapMap) -> InvariantResult {
        let problems = sut.observed_problems();
        if problems.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[inv-no-observed-errors] {} swallowed problem(s) (ERROR log / panic) \
                 during this transition — a green run is hiding a real failure:\n  {}",
                problems.len(),
                problems.join("\n  "),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Fixture provider — canned problems, NO process-global state, so it can't
    /// pollute other tests reading the shared collector (the trap the first cut
    /// of these tests fell into).
    struct FixtureProblems(Vec<String>);
    impl ObservedProblems for FixtureProblems {
        fn observed_problems(&self) -> Vec<String> {
            self.0.clone()
        }
    }

    fn map(problems: Vec<String>) -> CapMap {
        let mut caps = CapMap::new();
        caps.insert(Arc::new(FixtureProblems(problems)) as Arc<dyn ObservedProblems>);
        caps
    }

    /// Clean surface (no problems captured) ⇒ passes.
    #[tokio::test]
    async fn ok_when_no_problems() {
        let sut = map(Vec::new());
        assert!(matches!(
            InvNoObservedErrors.check(&CapMap::new(), &sut).await,
            InvariantResult::Ok
        ));
    }

    /// Catch: a captured problem (the shape a swallowed background-worker panic
    /// like the advice `No id found` would produce) ⇒ the invariant fails loud.
    #[tokio::test]
    async fn fails_when_a_problem_was_captured() {
        let sut = map(vec![
            "[PANIC] tokio-rt-worker (crates/holon-profiles/src/lib.rs:770): No id found"
                .to_string(),
        ]);
        match InvNoObservedErrors.check(&CapMap::new(), &sut).await {
            InvariantResult::Fail(msg) => {
                assert!(
                    msg.contains("No id found"),
                    "message must surface the problem: {msg}"
                );
            }
            other => panic!("expected Fail on a captured problem, got {other:?}"),
        }
    }
}
