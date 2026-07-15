//! `inv-viewmodel-root-matches-render-expr` — the SUT root widget matches the
//! reference's expected root render-expr. `Needs SutRenderer +
//! RefViewSelection`. The ref side is the production `ReferenceState`;
//! selection ANDs the SUT and ref cap sets, so it only fires where a real
//! renderer slice is wired (the frontend slice).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::viewmodel_root_matches_render_expr::InvViewmodelRootMatchesRenderExpr;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelRootMatchesRenderExpr,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefViewSelection>()],
        },
    ))
}
