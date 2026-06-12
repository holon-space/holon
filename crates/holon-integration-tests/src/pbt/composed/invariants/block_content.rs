//! `inv-block-content-matches-ref/block_raw` wired into the memory slice —
//! `SutBackend` + `RefBlockTree`. The first body to read the reference through
//! `RefBlockTree::block_content`, whose `Option<&str>` return is forwarded
//! through `CapMap::expect_ref` (a *borrowed* provider, not a cloned `Arc`) —
//! the adapter shape that unblocks the whole borrow-returning `Ref*` cap family.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBlockTree, SutBackend};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::block_content_matches_ref_backend::InvBlockContentMatchesRefBackend;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBlockContentMatchesRefBackend,
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
    use crate::pbt::composed::subsystem_seed::{assert_ref_seeded, run_with_seeded_ref, seed_ref};

    /// Positive: wiring `RefBlockTree` selects the content invariant, which
    /// reads the reference through the borrow-returning `block_content` and
    /// passes when it matches `block_raw`.
    #[tokio::test]
    async fn memory_slice_runs_block_content_via_refblocktree_borrow() {
        let blocks = vec![
            Block::new_text(uri("local://r"), EntityUri::no_parent(), "root"),
            Block::new_text(uri("local://c"), uri("local://r"), "child"),
        ];
        let sut = fixture_slice(blocks.clone());
        let expected_ids: Vec<_> = blocks.iter().map(|b| b.id.clone()).collect();
        let ref_state = seed_ref(blocks);
        assert_ref_seeded(&ref_state, &expected_ids);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        assert!(
            report
                .ran_ids()
                .contains(&"inv-block-content-matches-ref/block_raw"),
            "wiring RefBlockTree must select the content invariant; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "ref content matches block_raw, so it passes: {:?}",
            report.failures(),
        );
    }

    /// Negative containment (§2): deselected — disclosed, not faked — when only
    /// `RefBackend`, not `RefBlockTree`, is wired. A ref map missing the cap must
    /// not silently pass the borrow-returning body.
    #[tokio::test]
    async fn memory_slice_block_content_deselected_without_refblocktree() {
        use holon_pbt_core::capabilities::RefBackend;
        use holon_pbt_core::composition::{CapProvider, Config};
        use std::sync::Arc;

        // A ref provider exposing ONLY RefBackend, not RefBlockTree.
        struct RefBackendOnly {
            blocks: Vec<Block>,
        }
        impl RefBackend for RefBackendOnly {
            fn non_seed_blocks(&self) -> Vec<Block> {
                self.blocks.clone()
            }
            fn seed_block_ids(&self) -> std::collections::BTreeSet<EntityUri> {
                std::collections::BTreeSet::new()
            }
            fn org_blocks(&self) -> Vec<Block> {
                self.blocks.clone()
            }
        }
        impl CapProvider for RefBackendOnly {
            fn register(self: Arc<Self>, caps: &mut CapMap) {
                caps.insert(self as Arc<dyn RefBackend>);
            }
        }

        let blocks = vec![Block::new_text(
            uri("local://r"),
            EntityUri::no_parent(),
            "root",
        )];
        let sut = fixture_slice(blocks.clone());
        let ref_ = Config::new().with(RefBackendOnly { blocks }).build();

        let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

        assert!(
            report
                .deselected
                .iter()
                .any(|id| id.0 == "inv-block-content-matches-ref/block_raw"),
            "without RefBlockTree the content invariant must be deselected; \
             ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch (doc §6 gate): a `block_raw` content that diverged from the
    /// reference's `RefBlockTree` view — the borrow-returning read driving a
    /// real failure.
    #[tokio::test]
    async fn memory_slice_block_content_catches_divergence_via_refblocktree() {
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
                .any(|(id, _)| *id == "inv-block-content-matches-ref/block_raw"),
            "the content divergence must be caught via RefBlockTree::block_content; \
             failures={failures:?}",
        );
    }
}
