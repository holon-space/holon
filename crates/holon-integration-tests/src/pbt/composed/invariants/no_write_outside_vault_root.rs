//! `inv-no-write-outside-vault-root` wired into the composed slice — needs
//! `SutFsWrites` (the in-memory FS write log), no reference.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutFsWrites;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::no_write_outside_vault_root::InvNoWriteOutsideVaultRoot;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoWriteOutsideVaultRoot,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutFsWrites>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
