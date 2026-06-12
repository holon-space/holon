//! `inv-frontend-root-not-error` — the rendered root ViewModel node is not an
//! `Error` widget (reads [`SutViewModel::frontend_root_is_error`]). `Needs
//! SutViewModel` only (no reference): a SUT-internal liveness property. Selected
//! by any slice with a ViewModel — today the frontend slice's real headless
//! `ReactiveEngine`, where it runs over the actual render pipeline.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutViewModel;
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::frontend_root_not_error::InvFrontendRootNotError;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFrontendRootNotError,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutViewModel>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
