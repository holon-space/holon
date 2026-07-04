//! `inv-main-panel-rows-match-focus` — no stale rows in the main panel after
//! navigation: every ref-known block rendered inside the main-panel subtree
//! must be in the Main region's current focus-root subtree (or layout/profile
//! scaffolding). `Needs SutRenderer + RefViewSelection + RefLayout + RefFocus`.
//! The SUT cap comes only from a renderer slice (the frontend slice's headless
//! `ReactiveEngine`); storage-only slices deselect honestly.
//!
//! Added 2026-07-05 for the cross-frontend stale-row-on-navigation prod bug
//! (previous focus root's rows linger after NavigateFocus — focus_roots
//! chained-matview CDC delete propagation suspect).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::main_panel_rows_match_focus::InvMainPanelRowsMatchFocus;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvMainPanelRowsMatchFocus,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefViewSelection>(),
                CapId::of::<dyn RefLayout>(),
                CapId::of::<dyn RefFocus>(),
            ],
        },
    ))
}
