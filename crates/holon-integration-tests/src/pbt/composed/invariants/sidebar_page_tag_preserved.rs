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
