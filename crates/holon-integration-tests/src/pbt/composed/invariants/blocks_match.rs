//! The `/org` store of the block-equivalence composite. Its siblings —
//! `inv-blocks-match-ref/{block_raw,matview,loro}` — moved to the
//! correspondence registry (`composed::correspondences::non_seed_blocks`);
//! `/org` stays hand-written until Phase 3 (distinct observable facet: it also
//! checks renderer-canonical sibling ORDER).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBackend, SutOrgRead};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::blocks_match_ref::InvBlocksMatchRefOrg;

/// `inv-blocks-match-ref/org` — the blocks parsed back off the on-disk org files
/// (`SutOrgRead`) vs the reference's org view (`RefBackend::org_blocks`). Selected
/// only by a slice supplying `SutOrgRead` — today the frontend slice over the
/// production `holon_orgmode` parser (E1).
pub fn wire_org() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBlocksMatchRefOrg,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutOrgRead>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBackend>()],
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::{run_with_seeded_ref, seed_ref};

    /// Catch (doc §6 gate): with the ref wired, a SUT `block_raw` whose content
    /// diverged from the reference is caught.
    #[tokio::test]
    async fn memory_slice_catches_block_raw_divergence_from_ref() {
        let id = uri("local://d");
        let sut = fixture_slice(vec![Block::new_text(
            id.clone(),
            EntityUri::no_parent(),
            "sut-content",
        )]);
        let ref_state = seed_ref(vec![Block::new_text(
            id,
            EntityUri::no_parent(),
            "ref-content",
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
                .any(|(id, _)| *id == "inv-blocks-match-ref/block_raw"),
            "the content divergence must be caught; failures={failures:?}",
        );
    }
}
