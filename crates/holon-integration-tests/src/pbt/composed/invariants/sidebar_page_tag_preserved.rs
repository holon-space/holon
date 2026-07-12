//! `inv-sidebar-page-tag-preserved` wired into the composed catalog — a block
//! the reference marks as a `Page` doc-root must still carry the `Page` tag in
//! the SUT projection (the sidebar renders `tag='Page'`). Catches a page
//! silently DEMOTED (its `Page` tag stripped) even though its block row still
//! exists — the folder-companion demotion class (dogfood 2026-07-12).
//!
//! `Needs SutBackend` (SUT) + `RefBlockTree` (ref). Selects wherever a matview
//! snapshot and a block-tree ref are both wired — a storage/frontend draw. A
//! Loro-only draw with no page ever booted has nothing in the snapshot to
//! check.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::sidebar_page_tag_preserved::InvSidebarPageTagPreserved;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvSidebarPageTagPreserved,
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
    use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
    use crate::pbt::composed::subsystem_seed::seed_ref;

    fn page(id: &str) -> Block {
        let mut b = Block::new_text(uri(id), EntityUri::no_parent(), id);
        b.set_page(true);
        b
    }

    /// Catch (doc §6 gate): a block the reference models as a `Page` doc-root
    /// is present in the SUT WITHOUT its `Page` tag (the folder-companion
    /// demotion) — caught by `inv-sidebar-page-tag-preserved`.
    #[tokio::test]
    async fn catches_demoted_page() {
        let page_id = uri("local://journal");
        // SUT: the same block, but its `Page` tag was stripped (demoted).
        let sut = fixture_slice(vec![Block::new_text(
            page_id.clone(),
            EntityUri::no_parent(),
            "journal",
        )]);
        // Ref: the block IS a page.
        let ref_state = seed_ref(vec![page("local://journal")]);

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
                .any(|(id, _)| *id == "inv-sidebar-page-tag-preserved"),
            "a demoted page must be caught by inv-sidebar-page-tag-preserved; \
             failures={failures:?}",
        );
    }

    /// Positive: a page that stays a page on BOTH sides does not fire the
    /// oracle.
    #[tokio::test]
    async fn preserved_page_passes() {
        let sut = fixture_slice(vec![page("local://journal")]);
        let ref_state = seed_ref(vec![page("local://journal")]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        assert!(
            !report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-sidebar-page-tag-preserved"),
            "a page tagged on both sides must NOT fire the oracle; failures={:?}",
            report.failures(),
        );
    }
}
