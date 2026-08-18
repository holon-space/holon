//! `inv-filter-spec-resolves` wired into the memory slice — `SutBackend` only,
//! ignores the reference. FLT-1.b: a `holon_filter` source block resolves to a
//! typed `FilterSpec` over the projected block set.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::filter_spec_resolves::InvFilterSpecResolves;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFilterSpecResolves,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
