//! `inv-embedded-page-collapsed-lazy` wired into the composed catalog.
//! `Needs RefBlockTree + RefLayout + RefViewSelection + RefFocus + RefToggle`
//! (ref side) + `SutRenderer` (SUT). Only the frontend slice supplies the
//! SUT cap; storage-only slices deselect honestly.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefToggle;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::embedded_page_collapsed_lazy::InvEmbeddedPageCollapsedLazy;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvEmbeddedPageCollapsedLazy,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefBlockTree>(),
                CapId::of::<dyn RefLayout>(),
                CapId::of::<dyn RefViewSelection>(),
                CapId::of::<dyn RefFocus>(),
                CapId::of::<dyn RefToggle>(),
            ],
        },
    ))
}
