//! `inv-every-page-has-its-own-file` wired into the composed catalog — the
//! materialization-side twin of `inv-companion-has-no-child-page-headings`.
//! Every block the reference models as a `Page` doc-root must own EXACTLY ONE
//! file on disk after settle (`#+ID: <page-id>`): neither fileless (row 137,
//! the core Fork B loss) nor double-homed.
//!
//! `Needs SutOrgRender` (SUT disk pairs) + `RefBlockTree` (page authority via
//! `is_page_block` over `all_non_seed_block_ids` — the sole page predicate,
//! OQ1). Selected only by a slice supplying both (the frontend slice over its
//! real `HeadlessFrontendComponent` disk + a block-tree ref). Vacuously green
//! on a draw whose ref models no non-seed page (the default keystone), so it
//! adds no false RED.
//!
//! NOT YET registered in the central `catalog.rs` list: doing so must be gated
//! on a full keystone run confirming the default `wide_e2e` seed has no
//! legitimately inline-only non-seed page that would false-RED it (Fork B B1
//! remaining step — see `docs/Plans/ForkB-B1-2026-07-13.md` §5 item 4). The
//! body + its positive/negative unit triad already prove the logic
//! (`invariants::bodies::every_page_has_its_own_file`).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::every_page_has_its_own_file::InvEveryPageHasItsOwnFile;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvEveryPageHasItsOwnFile,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutOrgRender>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
