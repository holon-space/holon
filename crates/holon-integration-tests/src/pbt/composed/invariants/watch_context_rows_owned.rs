//! `inv-watch-context-rows-owned` wired into the composed catalog — a pure
//! `SutWatchContext` self-check (no ref). Selects wherever a SUT holds a real
//! engine with keyed watches; every other slice deselects.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutWatchContext;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::watch_context_rows_owned::InvWatchContextRowsOwned;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvWatchContextRowsOwned::default(),
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutWatchContext>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
