//! `inv-value-fn-provider-identity` — ViewModel value-fn provider rows agree with
//! the reference's task-state / block-tree view. `Needs SutViewSelection + RefTaskState
//! + RefBlockTree`. The ref side is the production `ReferenceState`; selection ANDs
//! the SUT and ref cap sets, so it only fires where a real ViewModel slice is wired
//! (the frontend slice). Teeth run there over the real render pipeline.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBlockTree, RefTaskState, SutFrontendEmissions};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::value_fn_provider_identity::InvValueFnProviderIdentity;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvValueFnProviderIdentity,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutFrontendEmissions>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefTaskState>(),
                CapId::of::<dyn RefBlockTree>(),
            ],
        },
    ))
}
