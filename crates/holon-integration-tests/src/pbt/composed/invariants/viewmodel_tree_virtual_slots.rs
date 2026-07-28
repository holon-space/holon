//! `inv-viewmodel-tree-virtual-slots` — the rendered ViewModel tree's virtual
//! creation slots are last-child, and the focused page title uses the
//! `page_title` (bare-text) block-profile variant. `Needs SutRenderer +
//! RefBlockTree`: the renderer supplies the widget tree; `RefBlockTree`
//! supplies the `Main`-region focus roots. Selected by any slice with a
//! renderer — the frontend slice's real headless `ReactiveEngine`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::viewmodel_tree_virtual_slots::InvViewmodelTreeVirtualSlots;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelTreeVirtualSlots,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
