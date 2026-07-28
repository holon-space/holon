//! `inv-display-placement-canonical-inert` wired into the composed catalog
//! (Phase 1a inert-render bit-identity gate per ADR 0015 §Evidence). Needs
//! `SutBackend` (block-id set), `SutOrgRender` (org fixed-point), and
//! `SutRenderer` (widget tree — non-vacuity source) from the frontend slice.
//! Selected only when all three caps are present.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::display_placement_canonical_inert::InvDisplayPlacementCanonicalInert;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvDisplayPlacementCanonicalInert,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutBackend>(),
                CapId::of::<dyn SutOrgRender>(),
                CapId::of::<dyn SutRenderer>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
