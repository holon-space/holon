//! `inv-focus-roots` wired into the composed catalog — the `focus_roots`
//! matview's per-region root set matches the reference's expected focus roots,
//! with the CDC-lag → `Skipped` downgrade the body implements
//! (`SutBackend::live_focus_root_rows` mirror vs `SutSqlProjection::focus_roots_rows`
//! matview vs `nav_history_open_rows` base, against `RefFocus::expected_focus_root_rows`).
//!
//! `Needs SutBackend + SutSqlProjection` (SUT) + `RefFocus` (ref). Same selection
//! shape as `inv-navigation-focus`; only the navigation slice drives real focus
//! data through it.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefFocus, SutBackend, SutSqlProjection};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::focus_roots::InvFocusRoots;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFocusRoots,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutBackend>(),
                CapId::of::<dyn SutSqlProjection>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefFocus>()],
        },
    ))
}
