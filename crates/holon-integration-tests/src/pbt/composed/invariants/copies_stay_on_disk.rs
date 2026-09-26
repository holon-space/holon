//! `inv-copies-stay-on-disk` wired into the composed slice — needs
//! `SutEditorSaves` (the vault files and what the external editor pasted) and
//! `RefCopies` (the model's copy map).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefCopies;
use holon_pbt_core::capabilities::SutEditorSaves;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::copies_stay_on_disk::InvCopiesStayOnDisk;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvCopiesStayOnDisk,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutEditorSaves>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefCopies>(),
                CapId::of::<dyn RefBlockTree>(),
            ],
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
