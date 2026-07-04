//! `inv-frontend-root-not-error` — the rendered root ViewModel node is not an
//! `Error` widget (reads [`SutViewSelection::frontend_root_is_error`]). `Needs
//! SutViewSelection` only (no reference): a SUT-internal liveness property.
//! Selected by any slice with a ViewModel — today the frontend slice's real
//! headless `ReactiveEngine`, where it runs over the actual render pipeline.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::frontend_root_not_error::InvFrontendRootNotError;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFrontendRootNotError,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutFrontendEngine>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
