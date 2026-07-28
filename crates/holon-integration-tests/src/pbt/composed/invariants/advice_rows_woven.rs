//! `inv-advice-rows-woven` — the rendered widget tree weaves each anchor's
//! expected advice rows (top-K, suppression-filtered, occurrence-stamped).
//! `Needs SutRenderer + RefAdvice`: the renderer supplies the widget tree;
//! `RefAdvice` supplies the per-anchor expectation. Selected by any slice with
//! a renderer + the advice ref cap (the frontend slice's headless
//! `ReactiveEngine`
//! + `ReferenceState`). EXPECTED RED between step 4 (generator mints rules) and
//! step 6 (renderer weaves them) — see the body doc.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefAdvice;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::advice_rows_woven::InvAdviceRowsWoven;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvAdviceRowsWoven,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefAdvice>()],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
