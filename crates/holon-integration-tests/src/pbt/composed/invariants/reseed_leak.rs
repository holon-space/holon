//! `inv-no-steady-reseed-leak` wiring — needs the `ReseedAttribution` read cap
//! (a process-global LoroProjection full-reseed attribution surface). Only a
//! slice that registers the cap (the `wide_e2e` slice, `otel-testing`) selects
//! it; others deselect it (disclosed, not faked). Observe-only in Inc 0 — see
//! [`crate::pbt::composed::reseed_observer`].

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::composed::reseed_observer::InvNoSteadyReseedLeak;
use crate::pbt::composed::reseed_observer::ReseedAttribution;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoSteadyReseedLeak::from_env(),
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn ReseedAttribution>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::cross_cutting(file!()),
    ))
}
