//! `inv-block-parent-matches-ref/block_raw` wired into the memory slice —
//! `SutBackend` + `RefBlockTree` via `RefBlockTree::parent_of`. Closes the
//! re-parent-divergence gap the other block-tree invariants leave open
//! (`blocks-match` skips the `Parent` facet; `no_orphan`/`no_parent_cycles`
//! only check existence/termination). Sound here because the memory slice has
//! no doc-id remapping.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBlockTree, SutBackend};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::block_parent_matches_ref_backend::InvBlockParentMatchesRefBackend;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBlockParentMatchesRefBackend,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::{run_with_seeded_ref, seed_ref};

    /// Catch (doc §6 gate): a block whose SUT parent is a *different but valid*
    /// (present, acyclic) block than the reference says. `no_orphan`/
    /// `no_parent_cycles` pass (the wrong parent exists and the chain
    /// terminates) and `blocks-match` passes (it skips the `Parent` facet), so
    /// only `block-parent-matches-ref` fails — exercising `parent_of`.
    #[tokio::test]
    async fn memory_slice_block_parent_catches_reparent_via_refblocktree() {
        let x = uri("local://x");
        let p1 = uri("local://p1");
        let p2 = uri("local://p2");
        // SUT: X parented under P2 (which exists → not an orphan, no cycle).
        let sut = fixture_slice(vec![
            Block::new_text(p1.clone(), EntityUri::no_parent(), "p1"),
            Block::new_text(p2.clone(), EntityUri::no_parent(), "p2"),
            Block::new_text(x.clone(), p2, "x"),
        ]);
        // Ref: same blocks/content/id-set, but X belongs under P1.
        let ref_state = seed_ref(vec![
            Block::new_text(p1.clone(), EntityUri::no_parent(), "p1"),
            Block::new_text(uri("local://p2"), EntityUri::no_parent(), "p2"),
            Block::new_text(x, p1, "x"),
        ]);

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
                .any(|(id, _)| *id == "inv-block-parent-matches-ref/block_raw"),
            "the re-parent must be caught by the parent invariant; failures={failures:?}",
        );
        // Isolation: the existence/termination/content invariants must NOT fire —
        // the wrong parent is valid and only the parent linkage diverged.
        for clean in [
            "inv-no-orphan-blocks",
            "inv-no-parent-cycles",
            "inv-blocks-match-ref/block_raw",
        ] {
            assert!(
                !failures.iter().any(|(id, _)| *id == clean),
                "{clean} must stay green (only parent linkage diverged); failures={failures:?}",
            );
        }
    }
}
