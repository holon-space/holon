//! `inv-no-machine-keychain-access` wired into the composed slice — reads the
//! process's keychain attempt counter, so it needs no capability and no ref.

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::no_machine_keychain_access::InvNoMachineKeychainAccess;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoMachineKeychainAccess,
        RunMode::Strict,
        Needs {
            sut_present: Vec::new(),
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
