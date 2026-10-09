//! `inv-crash-history-matches-ref` wired into the composed slice: needs
//! `SutCrashHistory` (the production history over the session's record dir)
//! and `RefCrashHistory` (the records the model knows were written).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefCrashHistory;
use holon_pbt_core::capabilities::SutCrashHistory;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::crash_history_matches_ref::InvCrashHistoryMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvCrashHistoryMatchesRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutCrashHistory>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefCrashHistory>()],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
