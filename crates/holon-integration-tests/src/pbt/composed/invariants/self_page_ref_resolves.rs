//! A query block naming its own page yields its nearest page ancestor.
//! `Needs SutRenderer + RefBlockTree`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::self_page_ref_resolves::InvSelfPageRefResolves;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvSelfPageRefResolves,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
