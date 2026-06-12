//! `inv-live-tree-matches-fresh` — the live (incrementally-updated) ViewModel
//! tree equals a fresh rebuild (reads [`SutViewModel::live_vs_fresh_tree_diff`]):
//! catches `set_data` failing to propagate updated props to children. `Needs
//! SutViewModel` only (no reference): a SUT-internal coherence property. Selected
//! by any slice with a ViewModel — today the frontend slice's real headless
//! `ReactiveEngine`, where the live/fresh diff is meaningful.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutViewModel;
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::live_tree_matches_fresh::InvLiveTreeMatchesFresh;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLiveTreeMatchesFresh,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutViewModel>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
