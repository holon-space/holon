//! `inv-no-observed-errors` wiring — needs the `ObservedProblems` read cap (a
//! process-global ERROR-event + panic capture). Only a slice that registers the
//! cap (the `wide_e2e` slice, `otel-testing`) selects it; others deselect it
//! (disclosed, not faked). See [`crate::pbt::composed::observed_errors`].

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::composed::observed_errors::InvNoObservedErrors;
use crate::pbt::composed::observed_errors::ObservedProblems;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoObservedErrors,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn ObservedProblems>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
