//! `inv-view-model-matches-store-at-quiescence` — the UI read model published
//! at the Loro commit, the projection's private diff base `live`, and the SQL
//! index agree once everything settles (D172.a). `Needs SutReadModel` only (no
//! reference): a SUT-internal coherence property across the three layers of
//! one write path. Selected by any slice that boots a real Loro projection.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutReadModel;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::view_model_matches_store::InvViewModelMatchesStore;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewModelMatchesStore,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutReadModel>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
