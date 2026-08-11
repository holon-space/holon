//! `inv-complexity-class-trend` — does a declared-O(1) transition get more
//! expensive the longer the run goes? Needs the composed [`ComposedTrend`] read
//! cap, which only a slice whose harness accumulates per-transition counters
//! registers (the `wide_e2e` keystone, through
//! [`ComposedSpanMetrics`]). Storage-only / pure slices have no counter series
//! and DESELECT — disclosed, never faked.
//!
//! The decision, the arming switch, and why the report diverges from
//! `inv-sql-budget`'s: [`crate::pbt::composed::complexity_trend`].
//!
//! [`ComposedTrend`]: crate::pbt::composed::complexity_trend::ComposedTrend
//! [`ComposedSpanMetrics`]: crate::pbt::composed::span_metrics::ComposedSpanMetrics

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::composed::complexity_trend::ComposedTrend;
use crate::pbt::composed::complexity_trend::InvComplexityTrend;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvComplexityTrend,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn ComposedTrend>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::cross_cutting(file!()),
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::complexity_trend::ComplexityClass;
    use crate::pbt::composed::fixtures::*;

    fn flat() -> FixtureTrend {
        FixtureTrend {
            kind: "ClickBlock".to_string(),
            class: ComplexityClass::Constant,
            reads: vec![9; 12],
            enforce: true,
        }
    }

    /// A declared-O(1) transition whose read count accumulates.
    fn accumulating(enforce: bool) -> FixtureTrend {
        FixtureTrend {
            reads: (0..12).map(|i| 9 + i * 3).collect(),
            enforce,
            ..flat()
        }
    }

    /// Positive: a flat counter series ⇒ selected (a `ComposedTrend` is wired)
    /// and passing.
    #[tokio::test]
    async fn trend_passes_on_a_flat_series() {
        let sut = trend_map(flat());
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report.ran_ids().contains(&"inv-complexity-class-trend"),
            "wiring ComposedTrend must select inv-complexity-class-trend; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "a flat series must pass: {:?}",
            report.failures(),
        );
    }

    /// Negative containment: without a `ComposedTrend` the invariant is
    /// DESELECTED. A slice that never accumulated a counter series must not
    /// silently "pass" the trend check.
    #[tokio::test]
    async fn trend_deselected_without_trend_cap() {
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
            report
                .deselected
                .iter()
                .any(|d| d.0 == "inv-complexity-class-trend"),
            "inv-complexity-class-trend must be deselected without a ComposedTrend; \
             ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch: a declared-O(1) transition whose read count accumulates ⇒ the
    /// invariant fires, and the failure carries the WHOLE series (the fitted
    /// trend is the evidence; a minimal counterexample would destroy it).
    #[tokio::test]
    async fn trend_catches_an_accumulating_constant_transition() {
        let sut = trend_map(accumulating(true));
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        let failures = report.failures();
        let (_, message) = failures
            .iter()
            .find(|(id, _)| *id == "inv-complexity-class-trend")
            .unwrap_or_else(|| {
                panic!("an accumulating O(1) transition must be caught; failures={failures:?}")
            });
        assert!(
            message.contains("ClickBlock.reads") && message.contains("#12"),
            "the failure must carry the fitted trend AND the full series; got:\n{message}",
        );
    }

    /// Unarmed, the same accumulation is DISCLOSED as a skip carrying the
    /// evidence — never a silent pass, never a red for a defect another lane
    /// owns.
    #[tokio::test]
    async fn trend_discloses_when_unarmed() {
        let sut = trend_map(accumulating(false));
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report.failures().is_empty(),
            "unarmed must not fail: {:?}",
            report.failures(),
        );
        let outcome = report
            .ran
            .iter()
            .find(|(id, _)| id.0 == "inv-complexity-class-trend")
            .map(|(_, r)| r)
            .expect("the invariant must have RUN — an unarmed violation is disclosed, not absent");
        match outcome {
            holon_pbt_core::invariant::InvariantResult::Skipped(reason) => assert!(
                reason.contains("ClickBlock.reads"),
                "the skip must carry the evidence; got: {reason}",
            ),
            other => panic!("an unarmed violation must be Skipped, not {other:?}"),
        }
    }
}
