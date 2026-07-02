//! `inv-frontend-engine` — the reactive engine produced a usable root ViewModel
//! (reads [`SutViewModel::frontend_root_vm`]). `Needs SutViewModel` only (no
//! reference): a SUT-internal liveness property of the render pipeline. Selected
//! by any slice with a ViewModel — today the frontend slice's real headless
//! `ReactiveEngine`. Teeth run there (the fixture `FrontendRootVm` is `None`,
//! which is the not-ready case, not a clean root), mirroring how
//! `navigation_focus`/`watch_rows` carry their teeth in the frontend slice.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::frontend_engine::InvFrontendEngine;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFrontendEngine,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutFrontendEngine>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
