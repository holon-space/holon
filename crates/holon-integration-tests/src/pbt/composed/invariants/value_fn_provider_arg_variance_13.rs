//! `inv-value-fn-provider-arg-variance-13` — ViewModel value-fn provider arg
//! variance agrees with the reference's layout / global-focus view. `Needs
//! SutViewModel + RefLayout + RefGlobalFocus`. The ref side is the production
//! `ReferenceState`; selection ANDs the SUT and ref cap sets, so it only fires
//! where a real ViewModel slice is wired (the frontend slice). Teeth run there.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefGlobalFocus, RefLayout, SutViewModel};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::value_fn_provider_arg_variance_13::InvValueFnProviderArgVariance13;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvValueFnProviderArgVariance13,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutViewModel>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefLayout>(),
                CapId::of::<dyn RefGlobalFocus>(),
            ],
        },
    ))
}
