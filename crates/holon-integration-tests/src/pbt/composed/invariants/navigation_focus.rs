//! `inv-navigation-focus` wired into the composed catalog — the `current_focus`
//! matview's per-region focus matches the reference's navigation focus
//! (`RefFocus::navigation_focus_rows` vs `SutSqlProjection::current_focus_rows`).
//!
//! `Needs SutFocusProjection` (SUT) + `RefFocus` (ref). Selected only by a slice
//! that supplies both — i.e. drives navigation through a Turso `current_focus`
//! matview AND carries the focus reference (the `frontend_slice` `navigation_pbt`
//! and `full_headless`). `SutFocusProjection` is a C-5 split off `SutSqlProjection`
//! (2026-07-02): a storage-only slice (`sql_slice`/`sql_loro_slice`) drives no
//! navigation, so it does NOT register the cap and this invariant DESELECTS there
//! honestly — instead of selecting against an honest-empty focus family and
//! passing vacuously (empty focus vs an unnavigated ref).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefFocus, SutFocusProjection};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::navigation_focus::InvNavigationFocus;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNavigationFocus,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutFocusProjection>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefFocus>()],
        },
    ))
}
