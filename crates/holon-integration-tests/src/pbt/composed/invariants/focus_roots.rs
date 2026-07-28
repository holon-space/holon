//! `inv-focus-roots` wired into the composed catalog — the `focus_roots`
//! matview's per-region root set matches the reference's expected focus roots,
//! with the CDC-lag → `Skipped` downgrade the body implements
//! (`SutBackend::live_focus_root_rows` mirror vs `SutFocus::focus_roots_rows`
//! matview vs `nav_history_open_rows` base, against
//! `RefFocus::expected_focus_root_rows`).
//!
//! `Needs SutBackend + SutFocus` (SUT) + `RefFocus` (ref). Same selection
//! shape as `inv-navigation-focus`; only a navigation-driving slice supplies
//! `SutFocus` (C-5 split, 2026-07-02), so a storage-only slice deselects
//! honestly rather than running against an empty focus family.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutFocus;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::focus_roots::InvFocusRoots;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFocusRoots,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>(), CapId::of::<dyn SutFocus>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefFocus>()],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
