//! `inv-conditions-match-ref` wired into the composed slice — needs
//! `SutConditions` (the production `ConditionBus` this session's writers raise
//! on) and `RefConditions` (what the model expects to be disclosed).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefConditions;
use holon_pbt_core::capabilities::SutConditions;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::conditions_match_ref::InvConditionsMatchRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvConditionsMatchRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutConditions>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefConditions>()],
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
