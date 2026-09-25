//! `inv-loro-ui-rows-match-ref` wired into the composed catalog. Selected by
//! every draw with a Loro store.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutLoroUiRows;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::loro_ui_rows_match_ref::InvLoroUiRowsMatchRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLoroUiRowsMatchRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutLoroUiRows>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefBlockTree>(),
                CapId::of::<dyn RefBackend>(),
            ],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
