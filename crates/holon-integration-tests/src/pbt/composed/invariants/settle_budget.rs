//! `inv-settle-budget` — per-transition interaction→projection-visible
//! latency vs the p95 200ms SLO. Needs the composed [`SettleLatency`] read
//! cap, which only a slice whose harness actually TIMES its transitions
//! registers (the `wide_e2e` keystone). A pure/storage slice has no settle
//! window to measure and deselects it — disclosed, not faked.
//!
//! [`SettleLatency`]: crate::pbt::composed::settle_latency::SettleLatency

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::composed::settle_latency::InvComposedSettleBudget;
use crate::pbt::composed::settle_latency::SettleLatency;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvComposedSettleBudget,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SettleLatency>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::pbt::composed::fixtures::*;
    use crate::pbt::invariants::bodies::settle_budget::SLO;

    /// Positive: a fast transition ⇒ selected and passing.
    #[tokio::test]
    async fn settle_budget_passes_under_slo() {
        let sut = settle_map(Some(Duration::from_millis(20)));
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report.ran_ids().contains(&"inv-settle-budget"),
            "wiring SettleLatency must select inv-settle-budget; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "a 20ms transition must pass: {:?}",
            report.failures(),
        );
    }

    /// Negative containment: no timing cap ⇒ deselected, never a silent pass.
    #[tokio::test]
    async fn settle_budget_deselected_without_timing_cap() {
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
            report.deselected.iter().any(|d| d.0 == "inv-settle-budget"),
            "inv-settle-budget must be deselected without a SettleLatency cap; ran={:?} \
             deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch: the 2026-07-28 vault-scale IVM regime (~9s per navigation focus)
    /// FIRES — and past the loosest default slack, so no machine-load
    /// allowance can explain it away.
    #[tokio::test]
    async fn settle_budget_catches_vault_scale_regime() {
        let sut = settle_map(Some(Duration::from_secs(9)));
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, msg)| *id == "inv-settle-budget" && msg.contains("9000ms")),
            "a 9s interaction must be caught with its measured duration; failures={failures:?}",
        );
        assert!(SLO < Duration::from_secs(9));
    }
}
