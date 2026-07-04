//! Selection tests for `inv-loro-children-match-ref` — the invariant BODY +
//! `wire()` now live in the `holon-loro-testing` companion crate (co-location
//! Phase 1) and reach the composed catalog via the central fold. These
//! fixture-driven selection/catch tests stay here because they exercise the
//! central `composed_invariant_catalog()` + shared fixtures.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
    use crate::pbt::composed::subsystem_seed::seed_ref;

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
