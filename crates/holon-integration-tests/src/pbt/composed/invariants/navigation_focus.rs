//! `inv-navigation-focus` wired into the composed catalog — the `current_focus`
//! matview's per-region focus matches the reference's navigation focus
//! (`RefFocus::navigation_focus_rows` vs `SutSqlProjection::current_focus_rows`).
//!
//! `Needs SutSqlProjection` (SUT) + `RefFocus` (ref). Selected only by a slice
//! that supplies both — i.e. drives navigation through a Turso `current_focus`
//! matview AND carries the focus reference (the new `frontend_slice`
//! `navigation_pbt`). Storage slices with an honest-empty `SutSqlProjection`
//! select it but pass vacuously (empty focus vs an unnavigated ref); the
//! navigation slice's non-vacuity guard proves it ran over real data there.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefFocus, SutSqlProjection};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::navigation_focus::InvNavigationFocus;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNavigationFocus,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutSqlProjection>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefFocus>()],
        },
    ))
}
