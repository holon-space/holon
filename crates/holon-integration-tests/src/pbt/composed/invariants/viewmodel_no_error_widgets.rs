//! `inv-viewmodel-no-error-widgets` — the rendered ViewModel FOREST has no
//! `Error` widget nodes. `Needs SutViewSelection + SutRenderer` (no
//! reference): a SUT-internal liveness property of the render pipeline.
//! Selected by any slice with a renderer/ViewModel — today the frontend
//! slice's real headless `ReactiveEngine` (where it runs over the actual
//! CDC→watch→interpret trees) and the window slice; both register both caps.
//!
//! `SutRenderer` is what lets the body reach PER-BLOCK live trees, where a
//! failed `render_entity` puts its error widget. Root-only was the escape
//! `2026-08-26-render-failure-invisible-warn-and-root-only-oracle` records.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::SutViewSelection;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::viewmodel_no_error_widgets::InvViewmodelNoErrorWidgets;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelNoErrorWidgets,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutViewSelection>(),
                CapId::of::<dyn SutRenderer>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;

    /// Positive: wiring `SutViewSelection` with a clean tree selects the
    /// invariant and passes.
    #[tokio::test]
    async fn frontend_no_error_widgets_passes_on_clean_tree() {
        let sut = viewmodel_map(Some(0));
        let ref_ = CapMap::new();

        let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

        assert!(
            report.ran_ids().contains(&"inv-viewmodel-no-error-widgets"),
            "wiring SutViewSelection must select the no-error-widgets invariant; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "a clean tree (0 error nodes) passes: {:?}",
            report.failures(),
        );
    }

    /// Negative containment (§2): deselected — disclosed, not faked — when no
    /// `SutViewSelection` is wired (a storage-only slice).
    #[tokio::test]
    async fn frontend_no_error_widgets_deselected_without_viewmodel() {
        let sut = fixture_slice(vec![Block::new_text(
            uri("local://r"),
            EntityUri::no_parent(),
            "root",
        )]);
        let ref_ = CapMap::new();

        let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

        assert!(
            report
                .deselected
                .iter()
                .any(|d| d.0 == "inv-viewmodel-no-error-widgets"),
            "without SutViewSelection the invariant must be deselected; ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch (doc §6 gate): error widgets in the rendered tree are caught.
    #[tokio::test]
    async fn frontend_no_error_widgets_catches_error_nodes() {
        let sut = viewmodel_map(Some(2));
        let ref_ = CapMap::new();

        let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-viewmodel-no-error-widgets"),
            "error widgets must be caught; failures={failures:?}",
        );
    }
}
