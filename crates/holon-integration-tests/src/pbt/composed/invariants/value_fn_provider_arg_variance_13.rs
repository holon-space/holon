//! `inv-value-fn-provider-arg-variance-13` — ViewModel value-fn provider arg
//! variance agrees with the reference's layout / global-focus view. `Needs
//! SutViewSelection + RefLayout + RefGlobalFocus`. The ref side is the
//! production `ReferenceState`; selection ANDs the SUT and ref cap sets, so it
//! only fires where a real ViewModel slice is wired (the frontend slice). Teeth
//! run there.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::SutFrontendEmissions;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::value_fn_provider_arg_variance_13::InvValueFnProviderArgVariance13;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvValueFnProviderArgVariance13,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutFrontendEmissions>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefLayout>(),
                CapId::of::<dyn RefGlobalFocus>(),
            ],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
