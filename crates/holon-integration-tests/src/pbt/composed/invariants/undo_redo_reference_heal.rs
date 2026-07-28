//! `inv-undo-redo-reference-heal` wired into the composed catalog.
//!
//! Needs `SutBackend + SutSqlProjection + SutFocus` (all base-table reads) and
//! the ref-side `RefUndoRedoBurned` gate. Only a slice that both drives real
//! navigation data and runs the harness reconcile supplies all four — the
//! keystone/frontend slice. The burned set is empty on every draw without a
//! completed undo→redo round trip, so the invariant is vacuously green there
//! and adds no cost beyond one `BTreeSet::is_empty`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefUndoRedoBurned;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutFocus;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::undo_redo_reference_heal::InvUndoRedoReferenceHeal;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvUndoRedoReferenceHeal::<CapMap>::from_env(),
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutBackend>(),
                CapId::of::<dyn SutSqlProjection>(),
                CapId::of::<dyn SutFocus>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefUndoRedoBurned>()],
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;

    /// Negative containment: a backend-only slice must DESELECT this invariant
    /// (disclosed, not faked) — it cannot see navigation history or the
    /// junction tables, so a silent "pass" there would be a lie.
    #[tokio::test]
    async fn undo_redo_reference_heal_deselected_without_focus_and_sql_caps() {
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
                .any(|d| d.0 == "inv-undo-redo-reference-heal"),
            "inv-undo-redo-reference-heal must be deselected on a backend-only slice; ran={:?} \
             deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }
}
