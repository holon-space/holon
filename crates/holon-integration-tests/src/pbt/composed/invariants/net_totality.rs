//! `inv-net-totality` wired into any `SutDerivedNet`-bearing slice — reads the
//! derived net and the run's fired operations from the same SUT, ignores the
//! reference.

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::net_totality::InvNetTotality;
use crate::pbt::net_cap::SutDerivedNet;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNetTotality,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutDerivedNet>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
