//! `inv-matview-consistent-with-recompute` wired into the composed catalog: an
//! IVM-vs-recompute differential — every `CREATE MATERIALIZED VIEW` maintained
//! by the Turso IVM engine must equal its OWN defining SELECT re-executed now
//! (a bounded-wait STABLE fixed point to reject transient CDC maintenance lag).
//! `Needs SutMatviews` only — no reference cap (it compares the SUT's matview
//! against the SUT's own defining query, not against the reference model, which
//! is exactly the blind spot every ref-comparison invariant has). Selected only
//! by a slice supplying `SutMatviews` — today the frontend/keystone slice over
//! the real Turso projection.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutMatviews;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::matview_recompute_matches::InvMatviewRecomputeMatches;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvMatviewRecomputeMatches,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutMatviews>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
