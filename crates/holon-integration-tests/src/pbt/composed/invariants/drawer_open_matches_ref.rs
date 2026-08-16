//! `inv-drawer-open-matches-ref` — the rendered drawer nodes carry the
//! reference's open/closed state. `Needs SutRenderer + RefNavHistory`.
//! Selection ANDs the SUT and ref cap sets, so it only fires where a real
//! renderer slice is wired (the frontend slice).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefNavHistory;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::drawer_open_matches_ref::InvDrawerOpenMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvDrawerOpenMatchesRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefNavHistory>()],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
