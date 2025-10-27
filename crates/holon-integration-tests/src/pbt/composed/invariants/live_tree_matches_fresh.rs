//! `inv-live-tree-matches-fresh` — the live (incrementally-updated) ViewModel
//! tree equals a fresh rebuild (reads
//! [`SutViewSelection::live_vs_fresh_tree_diff`]): catches `set_data` failing
//! to propagate updated props to children. `Needs SutViewSelection` only (no
//! reference): a SUT-internal coherence property. Selected by any slice with a
//! ViewModel — today the frontend slice's real headless `ReactiveEngine`, where
//! the live/fresh diff is meaningful.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutFrontendEmissions;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::live_tree_matches_fresh::InvLiveTreeMatchesFresh;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLiveTreeMatchesFresh,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutFrontendEmissions>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
