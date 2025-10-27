//! `inv-org-render-fixed-point` wired into the composed catalog (E1 — the
//! `SutOrgRender` relocation onto the production render path): every tracked org
//! file rendered from the current SQL state must equal its on-disk bytes, else the
//! next `FileSyncController::re_render_all_tracked` would rewrite it and risk the
//! FSEvent echo-suppression loop. `Needs SutOrgRender` only — no reference cap (it
//! compares the SUT's render against the SUT's disk). Selected only by a slice
//! supplying `SutOrgRender` — today the frontend slice over its real `CacheBlockReader`
//! + `OrgRenderer`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::org_render_fixed_point::InvOrgRenderFixedPoint;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvOrgRenderFixedPoint,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutOrgRender>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
