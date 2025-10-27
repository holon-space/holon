//! `inv-sql-budget` — per-transition SQL/wall/RSS budget. Needs the composed
//! `ComposedBudget` read cap (a span-metrics provider that captured the
//! transition + frozen oracle); ignores the reference structurally (the
//! budget's `expected_sql` is computed inside the host from the oracle it was
//! handed at `freeze_for_check`). Only a slice that registers a span-metrics
//! provider (the `wide_e2e` slice's `ComposedSpanMetrics` over the production
//! full-headless CapMap) selects it; storage-only / pure slices deselect it
//! (disclosed, not faked). The budget is OFF by default (`HOLON_PERF_BUDGET`),
//! so a clean run is `Ok` / unenforced violations are `Skipped`; the catch test
//! injects an enforced violation through the fixture.

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::composed::span_metrics::ComposedBudget;
use crate::pbt::composed::span_metrics::InvComposedBudget;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvComposedBudget,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn ComposedBudget>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;

    /// Positive: a budget surface reporting clean ⇒ selected (a
    /// `ComposedBudget` is wired) and passing.
    #[tokio::test]
    async fn sql_budget_passes_when_clean() {
        let sut = budget_map(FixtureBudget::default());
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report.ran_ids().contains(&"inv-sql-budget"),
            "wiring ComposedBudget must select inv-sql-budget; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "a clean budget must pass: {:?}",
            report.failures(),
        );
    }

    /// Negative containment: without a `ComposedBudget` the invariant is
    /// *deselected* — disclosed, not faked. A backend-only SUT must not
    /// silently "pass" the budget check.
    #[tokio::test]
    async fn sql_budget_deselected_without_budget_cap() {
        let blocks = vec![Block::new_text(
            uri("local://r"),
            EntityUri::no_parent(),
            "root",
        )];
        let report = run_selected(
            &composed_invariant_catalog(),
            &fixture_slice(blocks),
            &CapMap::new(),
        )
        .await;

        assert!(
            report.deselected.iter().any(|d| d.0 == "inv-sql-budget"),
            "inv-sql-budget must be deselected without a ComposedBudget; ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch: an enforced budget violation ⇒ the invariant fires.
    #[tokio::test]
    async fn sql_budget_catches_enforced_violation() {
        let sut = budget_map(FixtureBudget {
            enforce: true,
            errors: vec!["reads 99/3 over budget".to_string()],
        });
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        let failures = report.failures();
        assert!(
            failures.iter().any(|(id, _)| *id == "inv-sql-budget"),
            "an enforced budget violation must be caught by inv-sql-budget; failures={failures:?}",
        );
    }
}
