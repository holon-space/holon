//! `inv-companion-has-no-child-page-headings` wired into the composed catalog —
//! the WRITEBACK-side twin of `inv-sidebar-page-tag-preserved`. A folder-companion
//! `.org` must not retain, on disk after settle, a heading for a block the
//! reference models as a `Page` doc-root owning its own file: writeback must
//! de-inline it (companion retains nothing of child pages; OQ3, 2026-07-12).
//!
//! `Needs SutOrgRender` (SUT disk pairs) + `RefBlockTree` (page authority via
//! `is_page_block` — the sole child-page predicate, OQ1). Selected only by a
//! slice supplying both — the frontend slice over its real `CacheBlockReader` +
//! `OrgRenderer`, with a block-tree ref. Inert on any draw whose companion files
//! inline no `Page` doc-root (the default keystone), so it adds no false RED.
//!
//! B0 red-first: this is RED today (the per-file `ensure_ingest_lossless` guard
//! refuses the de-inline as apparent block loss); B1's `SurvivingProjection`
//! union makes it green. The deterministic reproduction lives in
//! `frontend_slice::structural_pbt::folder_companion_writeback_deinlines_child_page`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBlockTree, SutOrgRender};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::companion_has_no_child_page_headings::InvCompanionHasNoChildPageHeadings;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvCompanionHasNoChildPageHeadings,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutOrgRender>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
    ))
}
