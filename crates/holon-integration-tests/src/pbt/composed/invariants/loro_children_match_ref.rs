//! `inv-loro-children-match-ref` — `SutLoroLog` + `RefBlockTree`. Per-parent
//! sibling-order equality between the **Loro** adapter (the tree's fractional
//! index, via `loro_children_of`) and the reference model's document order
//! (`sorted_children`). The Loro-side companion to `inv-live-children-match-ref`
//! (SQL projection). Real teeth in the Loro slice: a CRDT reorder bug surfaces
//! as a per-parent order divergence against the independent reference.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBlockTree, SutLoroLog};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::loro_children_match_ref::InvLoroChildrenMatchRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLoroChildrenMatchRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutLoroLog>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::{run_with_seeded_ref, seed_ref};

    /// A parent `p` with two children `c1, c2` in document order, plus the Loro
    /// double's reported child order — `agree` ⇒ same as the reference. Returns
    /// the SUT cap map and the reference blocks (the caller seeds them into a
    /// `ReferenceState` via [`seed_ref`] when a ref is wanted).
    fn scenario(loro_order: [&str; 2]) -> (CapMap, Vec<Block>) {
        let p = uri("local://p");
        let c1 = uri("local://c1");
        let c2 = uri("local://c2");
        let mut children = HashMap::new();
        children.insert(
            p.to_string(),
            loro_order.iter().map(|s| uri(s).to_string()).collect(),
        );
        let sut = loro_log_map(FixtureLoroLog {
            had_errors: false,
            children,
        });
        let ref_blocks = vec![
            Block::new_text(p.clone(), EntityUri::no_parent(), "p"),
            Block::new_text(c1, p.clone(), "c1"),
            Block::new_text(c2, p, "c2"),
        ];
        (sut, ref_blocks)
    }

    /// Positive: Loro reports the children in the reference's document order ⇒
    /// selected (`SutLoroLog` + `RefBlockTree` wired) and passing.
    #[tokio::test]
    async fn loro_children_match_ref_passes_when_order_agrees() {
        let (sut, ref_blocks) = scenario(["local://c1", "local://c2"]);
        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(seed_ref(ref_blocks)),
        )
        .await;

        assert!(
            report.ran_ids().contains(&"inv-loro-children-match-ref"),
            "SutLoroLog + RefBlockTree must select the invariant; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "matching sibling order must pass: {:?}",
            report.failures(),
        );
    }

    /// Negative containment (§2): with a `SutLoroLog` but **no** `RefBlockTree`
    /// reference, the invariant is deselected — disclosed, not faked.
    #[tokio::test]
    async fn loro_children_match_ref_deselected_without_ref() {
        let (sut, _ref_blocks) = scenario(["local://c1", "local://c2"]);
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report
                .deselected
                .iter()
                .any(|d| d.0 == "inv-loro-children-match-ref"),
            "must be deselected without a RefBlockTree; ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch: Loro reports the *same* children in the *wrong* order ⇒ the
    /// per-parent order check fires. The fixture injects a reorder a monotone
    /// fractional index can't produce.
    #[tokio::test]
    async fn loro_children_match_ref_catches_reordered_siblings() {
        let (sut, ref_blocks) = scenario(["local://c2", "local://c1"]);
        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(seed_ref(ref_blocks)),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-loro-children-match-ref"),
            "the sibling reorder must be caught; failures={failures:?}",
        );
    }
}
