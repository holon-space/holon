//! `inv-matview-consistent-with-ref` — the rendered ViewModel's block content
//! is consistent with the reference's layout view. `Needs SutRenderer + RefLayout`.
//! The ref side is the production `ReferenceState`; selection ANDs the SUT and ref
//! cap sets, so it only fires where a real renderer slice is wired (the frontend
//! slice). Teeth run there over the real render pipeline.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefLayout, SutRenderer};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::matview_consistent_with_ref::InvMatviewConsistentWithRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvMatviewConsistentWithRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefLayout>()],
        },
    ))
}
