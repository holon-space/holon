//! `inv-task-state-matches-ref` — the SQL projection's `task_state` vs the
//! reference model's, for every block.
//!
//! Its sibling `inv-task-state-storage-coherence` additionally needs
//! `SutLoroTaskState`, so it selects only where a Loro projection is wired: it
//! runs (and catches) in the Loro arm, and DESELECTS in the SqlOnly arm — the
//! `crdt.enabled = false` opt-out. This arm needs only the SQL projection, in
//! BOTH wirings, so a live authoring gesture's task-state effect is compared
//! against the model in both modes rather than in one.

use std::marker::PhantomData;

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::task_state_matches_ref::InvTaskStateMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvTaskStateMatchesRef::<CapMap>(PhantomData),
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutSqlProjection>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefTaskState>()],
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;

    /// Catch: the reference predicts a task state the SUT does not carry — the
    /// exact shape a live keyword promotion produces while only the model
    /// implements it.
    #[tokio::test]
    async fn task_state_matches_ref_catches_an_unhonoured_promotion() {
        let a = uri("block:a");
        // The block exists in the projection but carries no task_state.
        let sut = sql_projection_map(vec![(a.clone(), "buy milk")]);
        let ref_ = ref_task_state(vec![(a, "TODO")]);

        let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;
        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-task-state-matches-ref"),
            "a ref-predicted task_state the SUT lacks must be caught; failures={failures:?}",
        );
    }

    /// Positive: agreement ⇒ selected and green.
    #[tokio::test]
    async fn task_state_matches_ref_passes_when_they_agree() {
        let a = uri("block:a");
        let sut = task_state_maps(vec![(a.clone(), "TODO")], Vec::new());
        let ref_ = ref_task_state(vec![(a, "TODO")]);

        let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;
        assert!(
            report.ran_ids().contains(&"inv-task-state-matches-ref"),
            "SutSqlProjection + RefTaskState must select it; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            !report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-task-state-matches-ref"),
            "agreement must be green; failures={:?}",
            report.failures(),
        );
    }
}
