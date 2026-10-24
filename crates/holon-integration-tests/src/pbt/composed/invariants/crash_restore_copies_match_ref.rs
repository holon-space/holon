//! `inv-crash-restore-copies-match-ref` wired into the composed slice — needs
//! `SutEditorSaves` (the conflict copies in the vault) and `RefCopies` (the
//! copies the model's crash-restores left).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefCopies;
use holon_pbt_core::capabilities::SutEditorSaves;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::crash_restore_copies_match_ref::InvCrashRestoreCopiesMatchRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvCrashRestoreCopiesMatchRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutEditorSaves>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefCopies>()],
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
