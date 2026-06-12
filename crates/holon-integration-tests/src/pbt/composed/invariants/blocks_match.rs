//! `inv-blocks-match-ref/block_raw` wired into the memory slice — `SutBackend`
//! + `RefBackend`, so it runs only when a reference map is also wired (§5.1);
//! absent a ref it is *deselected*, not faked.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{
    RefBackend, SutBackend, SutLoroLog, SutOrgRead, SutSqlProjection,
};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::blocks_match_ref::{
    InvBlocksMatchRefBlockRaw, InvBlocksMatchRefLoro, InvBlocksMatchRefMatview,
    InvBlocksMatchRefOrg,
};

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBlocksMatchRefBlockRaw,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBackend>()],
        },
    ))
}

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

/// `inv-blocks-match-ref/loro` — the blocks read from the live Loro tree
/// (`SutLoroLog`) vs the reference (`RefBackend`). Selected by a slice supplying
/// `SutLoroLog` — the Loro / combined / frontend slices.
pub fn wire_loro() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBlocksMatchRefLoro,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutLoroLog>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBackend>()],
        },
    ))
}

/// `inv-blocks-match-ref/matview` — the blocks read from the `block` materialized
/// view (`SutBackend` + `SutSqlProjection`) vs the reference (`RefBackend`).
/// Selected by a slice supplying both SQL caps — the SQL / frontend slices.
pub fn wire_matview() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBlocksMatchRefMatview,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutBackend>(),
                CapId::of::<dyn SutSqlProjection>(),
            ],
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
