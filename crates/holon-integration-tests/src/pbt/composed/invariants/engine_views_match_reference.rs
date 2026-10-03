//! `inv-engine-views-match-reference` — the views the engine released equal
//! the views recomputed from the SUT's Loro docs and focus_roots once
//! everything settles.
//! `Needs SutEngineViews` only: the loro_* invariants tie the docs to the
//! reference.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutEngineViews;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::engine_views_match_reference::InvEngineViewsMatchReference;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvEngineViewsMatchReference,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutEngineViews>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
