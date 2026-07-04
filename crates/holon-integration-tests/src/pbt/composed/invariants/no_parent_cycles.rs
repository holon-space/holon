//! `inv-no-parent-cycles` wired into the memory slice — `SutBackend` only,
//! ignores the reference, so it runs whenever the SUT is wired.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::no_parent_cycles::InvNoParentCycles;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoParentCycles,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;

    /// Catch (doc §6 gate): two blocks each parenting the other — impossible
    /// through the validating API, so injected via the fixture.
    #[tokio::test]
    async fn memory_slice_catches_parent_cycle() {
        let a = uri("local://a");
        let b = uri("local://b");
        let blocks = vec![
            Block::new_text(a.clone(), b.clone(), "a"),
            Block::new_text(b, a, "b"),
        ];
        let report = run_selected(
            &composed_invariant_catalog(),
            &fixture_slice(blocks),
            &CapMap::new(),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures.iter().any(|(id, _)| *id == "inv-no-parent-cycles"),
            "the cycle must be caught by inv-no-parent-cycles; failures={failures:?}",
        );
    }
}
