//! `inv-birth-contract-satisfied` wired into the composed catalog — a pure SUT
//! self-check (no ref) over the projection every observer reads.
//!
//! Needs `SutOrderKeys` on top of `SutBackend` because the position facet is
//! not on the domain `Block` (ADR 0005). A slice that projects no order column
//! does not register that cap and the invariant DESELECTS there — the id and
//! parentage facets are separately covered by `inv-no-orphan-blocks`, so
//! nothing goes silently unchecked.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutOrderKeys;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::birth_contract::InvBirthContractSatisfied;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBirthContractSatisfied,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutBackend>(),
                CapId::of::<dyn SutOrderKeys>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
    use crate::pbt::composed::subsystem_seed::seed_ref;

    /// Negative containment: without `SutOrderKeys` the invariant is DESELECTED
    /// — reported as a disclosed blind spot, never as a silent pass.
    ///
    /// Load-bearing for this module specifically: deselection is the reason a
    /// plain `fixture_slice` (which models no order column) does not fail every
    /// pre-existing catch test on the position facet. Without this pin, adding
    /// `SutOrderKeys` to `fixture_slice` — or dropping it from `Needs` — would
    /// go unnoticed.
    #[tokio::test]
    async fn birth_contract_deselects_without_order_keys() {
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
                .any(|d| d.0 == "inv-birth-contract-satisfied"),
            "inv-birth-contract-satisfied must be deselected without a SutOrderKeys; ran={:?} \
             deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch (doc §6 gate): a block the projection exposes without ever having
    /// been positioned — its `sort_key` is still the unkeyed SQL default, so
    /// the birth contract is unmet even though the block is otherwise sound.
    #[tokio::test]
    async fn memory_slice_catches_an_unpositioned_block() {
        let half_born = uri("local://half-born");
        let sut = fixture_slice_with_order_keys(
            vec![Block::new_text(
                half_born.clone(),
                EntityUri::no_parent(),
                "half born",
            )],
            vec![(half_born.clone(), "A0".to_string())],
        );
        let ref_state = seed_ref(vec![Block::new_text(
            half_born,
            EntityUri::no_parent(),
            "half born",
        )]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-birth-contract-satisfied"),
            "the unkeyed sort_key must be caught by inv-birth-contract-satisfied; \
             failures={failures:?}",
        );
    }
}
