//! `inv-viewmodel-decompiled-rows-match-query` — the rendered rows decompile
//! back to the reference's query/render-expr. `Needs SutRenderer +
//! RefViewSelection`. The ref side is the production `ReferenceState`;
//! selection ANDs the SUT and ref cap sets, so it only fires where a real
//! renderer slice is wired (the frontend slice).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutQueryResults;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::viewmodel_decompiled_rows_match_query::InvViewmodelDecompiledRowsMatchQuery;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelDecompiledRowsMatchQuery,
        RunMode::Strict,
        Needs {
            // `SutQueryResults` (the full-mode query engine) is required here so this
            // full-mode twin is mutually exclusive with the degraded
            // `inv-viewmodel-shows-source-when-no-query` twin (which selects on
            // `sut_absent: [SutQueryResults]`). Body unchanged.
            sut_present: vec![
                CapId::of::<dyn SutRenderer>(),
                CapId::of::<dyn SutQueryResults>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefViewSelection>()],
        },
    ))
}
